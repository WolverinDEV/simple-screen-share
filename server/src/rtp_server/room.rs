use std::{
    collections::{btree_map::Entry, BTreeMap},
    pin::Pin,
    sync::{Arc, Weak},
    task::{Context, Poll, Waker},
};

use crate::rest_controller::auth::UserId;
use futures::{future, future::FutureExt, Future, StreamExt};
use log::{info};
use rand::{thread_rng, Rng};
use rocket::tokio::{sync::{Mutex, oneshot}, task};
use serde::{Serialize, Deserialize};
use tokio_stream::Stream;
use typescript_type_def::TypeDef;

use super::{
    client::{ClientId, SignalingClient},
    messages::{notify::{self, BroadcastEntry, UserEntry}, response::S2CResponse},
};

pub type RoomId = u64;

pub type BroadcastId = u32;
type SyncMutex<T> = std::sync::Mutex<T>;


pub enum BroadcastSourceEvent {
    Media(webrtc::rtp::packet::Packet),
    End,
}

#[derive(Debug)]
pub enum BroadcastTargetEvent {
    RequestPli,
}

/// Source stream for a client broadcast
pub trait RoomBroadcastSource: Stream<Item = BroadcastSourceEvent> + Send {
    fn send_pli(self: Pin<&mut Self>);

    fn kind(&self) -> BroadcastKind;
}


/// Target stream for a client broadcast
pub trait RoomBroadcastTarget: Stream<Item = BroadcastTargetEvent> + Send {
    fn send_rtp(&self, _packet: &mut webrtc::rtp::packet::Packet) {}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TypeDef)]
pub enum BroadcastKind {
    Audio,
    Video,
}

struct ClientBroadcast {
    kind: BroadcastKind,
    id: BroadcastId,
    name: String,

    client_id: ClientId,
    client: Arc<Mutex<SignalingClient>>,

    waker: Option<Waker>,
    source: Pin<Box<dyn RoomBroadcastSource>>,
    targets: BTreeMap<ClientId, Pin<Box<dyn RoomBroadcastTarget>>>,

    shutdown_tx: Option<oneshot::Sender<()>>,
    shutdown_rx: oneshot::Receiver<()>,
}

impl Future for ClientBroadcast {
    type Output = ();

    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        use Poll::*;

        if let Poll::Ready(_) = self.shutdown_rx.poll_unpin(cx) {
            return Poll::Ready(());
        }

        let mut pli_request = false;
        self.targets.drain_filter(|client_id, client| {
            while let Ready(event) = client.as_mut().poll_next(cx) {
                let event = match event {
                    Some(event) => event,
                    None => {
                        /* TODO: Notify that the client has been removed */
                        info!("Client {} events ended.", client_id);
                        return true;
                    }
                };

                match event {
                    BroadcastTargetEvent::RequestPli => pli_request = true,

                    #[allow(unreachable_patterns)]
                    event => info!("Client {} event: {:#?}", client_id, event),
                }
            }

            return false;
        });

        if pli_request {
            // TODO: Throttle pli requests so the source does not gets spammed.
            self.source.as_mut().send_pli();
        }

        while let Ready(event) = self.source.poll_next_unpin(cx) {
            let event = match event {
                Some(event) => event,
                None => {
                    /* the stream has ended... */
                    return Poll::Ready(());
                }
            };

            match event {
                BroadcastSourceEvent::Media(mut packet) => {
                    for client in self.targets.values() {
                        client.send_rtp(&mut packet);
                    }
                },
                BroadcastSourceEvent::End => return Poll::Ready(())
            }
        }

        self.waker = Some(cx.waker().clone());
        Pending
    }
}

pub struct Room {
    pub id: RoomId,
    weak_ref: Weak<Mutex<Self>>,

    pub subscriber: Vec<Arc<dyn RoomSubscriber>>,
    pub clients: BTreeMap<ClientId, Arc<Mutex<SignalingClient>>>,
    client_user_ids: BTreeMap<ClientId, UserId>,
    client_broadcasts: BTreeMap<ClientId, Vec<BroadcastId>>,

    broadcasts: BTreeMap<BroadcastId, Arc<SyncMutex<ClientBroadcast>>>,
}

// TODO: May disconnect the room from the SignalingClient and only use ClientId?
/// Note:
/// Room will be locked before clients.
impl Room {
    pub async fn new(id: RoomId) -> Arc<Mutex<Self>> {
        let instance = Self {
            id,
            weak_ref: Weak::new(),

            subscriber: Vec::new(),

            clients: Default::default(),
            client_user_ids: Default::default(),
            client_broadcasts: Default::default(),

            broadcasts: Default::default(),
        };
        let instance = Arc::new(Mutex::new(instance));
        instance.lock().await.weak_ref = Arc::downgrade(&instance);
        instance
    }

    pub async fn add_client(&mut self, client: &Arc<Mutex<SignalingClient>>, user_id: UserId) {
        let client_id = client.lock().await.client_id;
        if self.clients.contains_key(&client_id) {
            return;
        }

        self.client_user_ids.insert(client_id, user_id);
        self.clients.insert(client_id, client.clone());
        self.subscriber
            .iter()
            .for_each(|s| s.client_joined(self, client_id, client));

        for (other_client_id, other_client) in self.clients.iter() {
            if *other_client_id == client_id {
                continue;
            }

            let mut other_client = other_client.lock().await;
            other_client
                .send_message(&notify::S2CNotify::NotifyUserJoined(UserEntry{ client_id, user_id }).into());
        }

        {
            let clients = self.clients
                .keys()
                .map(|client_id| {
                    let user_id = self.client_user_ids
                        .get(client_id)
                        .cloned()
                        .unwrap_or(0);

                    notify::UserEntry{
                        client_id: *client_id,
                        user_id,
                    }
                })
                .collect::<Vec<_>>();

            client
                .lock()
                .await
                .send_message(&notify::S2CNotify::NotifyUsers(clients).into());
        }

        {
            let broadcasts = self.broadcasts
                .values()
                .map(|broadcast| {
                    let broadcast = broadcast.lock().unwrap();
                    
                    BroadcastEntry{
                        client_id: broadcast.client_id,
                        broadcast_id: broadcast.id,
                        kind: broadcast.kind,
                        name: broadcast.name.clone()
                    }
                })
                .collect::<Vec<_>>();

            client
                .lock()
                .await
                .send_message(&notify::S2CNotify::NotifyBroadcasts(broadcasts).into());
        }
    }

    pub async fn remove_client(&mut self, client_id: ClientId) {
        let client = match self.clients.remove(&client_id) {
            Some(client) => client,
            None => return,
        };

        self.subscriber
            .iter()
            .for_each(|s| s.client_left(self, client_id, &client));
        self.client_user_ids.remove(&client_id);

        if let Some(broadcasts) = self.client_broadcasts.remove(&client_id) {
            for broadcast_id in broadcasts {
                self.shutdown_broadcast(&broadcast_id);
            }
        }

        for other_client in self.clients.values() {
            let mut other_client = other_client.lock().await;
            other_client.send_message(&notify::S2CNotify::NotifyUserLeft(client_id).into());
        }
    }

    fn generate_broadcast_id(&self) -> Option<BroadcastId> {
        for _ in 1..1000 {
            let id = thread_rng().gen::<BroadcastId>();
            if !self.broadcasts.contains_key(&id) {
                return Some(id);
            }
        }

        None
    }

    pub fn broadcast_kind(&self, broadcast_id: BroadcastId) -> Option<BroadcastKind> {
        self.broadcasts
            .get(&broadcast_id)
            .map(|b| b.lock().unwrap().kind)
    }

    pub async fn client_broadcast_start(
        &mut self,
        client_id: ClientId,
        name: String,
        source: Pin<Box<dyn RoomBroadcastSource>>,
    ) -> S2CResponse {
        let broadcast_type = source.kind();

        let client = match self.clients.get(&client_id) {
            Some(client) => client,
            None => return S2CResponse::RoomNotJoined,
        };

        let broadcast_id = match self.generate_broadcast_id() {
            Some(id) => id,
            None => {
                return S2CResponse::InternalError {
                    message: "failed to generate broadcast id".to_owned(),
                }
            }
        };

        let client_broadcasts = match self.client_broadcasts.entry(client_id) {
            Entry::Occupied(value) => value.into_mut(),
            Entry::Vacant(v) => v.insert(vec![]),
        };

        for broadcast_id in client_broadcasts.iter() {
            let broadcast = match self.broadcasts.get(broadcast_id) {
                Some(broadcast) => broadcast,
                None => continue, // Should no happen.
            };

            let broadcast = broadcast.lock().unwrap();
            if broadcast.kind == broadcast_type && broadcast.name.to_lowercase() == name.to_lowercase() {
                return S2CResponse::BroadcastAlreadyRunning {
                    broadcast_id: *broadcast_id,
                };
            }
        }

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let broadcast = ClientBroadcast {
            kind: broadcast_type,
            id: broadcast_id,
            name: name.clone(),

            client: client.clone(),
            client_id,

            waker: None,
            source,
            targets: Default::default(),

            shutdown_tx: Some(shutdown_tx),
            shutdown_rx,
        };
        let broadcast = Arc::new(SyncMutex::new(broadcast));

        client_broadcasts.push(broadcast_id);
        self.broadcasts.insert(broadcast_id, broadcast.clone());

        let weak_room = self.weak_ref.clone();
        task::spawn(async move {
            future::poll_fn(move |cx| {
                let mut broadcast = broadcast.lock().unwrap();
                broadcast.poll_unpin(cx)
            })
            .await;

            info!("Client broadcast {} ended.", broadcast_id);
            if let Some(room) = weak_room.upgrade() {
                let mut room = room.lock().await;
                room.broadcasts.remove(&broadcast_id);

                for client in room.clients.values() {
                    let mut other_client = client.lock().await;
                    other_client.send_message(&notify::S2CNotify::NotifyBroadcastEnded(broadcast_id).into());
                }
                
                room.subscriber
                    .iter()
                    .for_each(|s| s.client_broadcast_ended(&room, client_id, broadcast_id));
            }
        });

        self.subscriber
            .iter()
            .for_each(|s| s.client_broadcast_started(self, client_id, broadcast_id));


        for client in self.clients.values() {
            let mut other_client = client.lock().await;
            other_client.send_message(&notify::S2CNotify::NotifyBroadcastStarted(BroadcastEntry{
                broadcast_id,
                client_id,
                name: name.clone(),
                kind: broadcast_type,
            }).into());
        }

        S2CResponse::BroadcastStarted { broadcast_id }
    }

    pub async fn client_broadcast_stop(
        &mut self,
        client_id: ClientId,
        broadcast_id: BroadcastId,
    ) -> S2CResponse {
        if let Some(broadcasts) = self.client_broadcasts.get_mut(&client_id) {
            let index = broadcasts
                .iter()
                .position(|id| *id == broadcast_id);

            let index = match index {
                Some(index) => index,
                None => return S2CResponse::BroadcastUnknownId,
            };

            broadcasts.remove(index);
        } else {
            /* Client has no broadcasts running or is unkown... */
            return S2CResponse::BroadcastUnknownId;
        }

        self.shutdown_broadcast(&broadcast_id);
        S2CResponse::Success
    }

    pub async fn client_broadcast_subscribe(
        &mut self,
        client_id: ClientId,
        broadcast_id: BroadcastId,
        target: Pin<Box<dyn RoomBroadcastTarget>>,
    ) -> S2CResponse {
        let broadcast = match self.broadcasts.get(&broadcast_id) {
            Some(broadcast) => broadcast,
            None => return S2CResponse::BroadcastUnknownId,
        };

        let mut broadcast = broadcast.lock().unwrap();
        if broadcast.targets.contains_key(&client_id) {
            return S2CResponse::BroadcastAlreadySubscribed;
        }

        broadcast.targets.insert(client_id, target);
        if let Some(waker) = &broadcast.waker {
            /* Wake the waker so we poll on the new target as well */
            waker.wake_by_ref();
        }

        S2CResponse::Success
    }

    pub async fn client_broadcast_unsubscribe(
        &mut self,
        client_id: ClientId,
        broadcast_id: BroadcastId,
    ) -> S2CResponse {
        let broadcast = match self.broadcasts.get(&broadcast_id) {
            Some(broadcast) => broadcast,
            None => return S2CResponse::BroadcastUnknownId,
        };

        let mut broadcast = broadcast.lock().unwrap();
        let _target = match broadcast.targets.remove(&client_id) {
            Some(target) => target,
            // The client isn't subscribed. Therefor technically he "unsubscribed".
            None => return S2CResponse::Success,
        };

        if let Some(waker) = &broadcast.waker {
            /* Wake the waker so we notice that a client has gone. */
            waker.wake_by_ref();
        }

        S2CResponse::Success
    }

    fn shutdown_broadcast(&self, broadcast_id: &BroadcastId) {
        let broadcast = match self.broadcasts.get(broadcast_id) {
            Some(broadcast) => broadcast,
            None => return,
        };

        if let Some(tx) = broadcast.lock().unwrap().shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

pub trait RoomSubscriber: Sync + Send {
    /// Will be called after registering the client and it's user id mapping.
    fn client_joined(
        &self,
        _room: &Room,
        _client_id: ClientId,
        _client: &Arc<Mutex<SignalingClient>>,
    ) {
    }

    /// Will be called after the client has been removed but the user id mapping of the room is still valid.
    fn client_left(
        &self,
        _room: &Room,
        _client_id: ClientId,
        _client: &Arc<Mutex<SignalingClient>>,
    ) {
    }

    fn client_broadcast_started(
        &self,
        _room: &Room,
        _client_id: ClientId,
        _broadcast_id: BroadcastId,
    ) {
    }

    fn client_broadcast_ended(&self, _room: &Room, _client_id: ClientId, _broadcast_id: BroadcastId) {
    }
}
