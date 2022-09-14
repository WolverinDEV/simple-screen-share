use std::{
    collections::{btree_map::Entry, BTreeMap},
    pin::Pin,
    sync::{Arc, Weak},
    task::{Context, Poll, Waker},
};

use crate::rest_controller::auth::UserId;
use futures::{future, future::FutureExt, Future, StreamExt};
use log::info;
use rand::{thread_rng, Rng};
use rocket::tokio::{sync::Mutex, task};
use tokio_stream::Stream;

use super::{
    client::{ClientId, SignalingClient},
    messages::{notify, response::S2CResponse},
};

pub type RoomId = u64;

pub enum RtpSourceEvent {
    Media(webrtc::rtp::packet::Packet),
}

pub type BroadcastId = u32;

#[derive(Debug)]
pub enum RtpTargetEvent {
    RequestPli,
}

type SyncMutex<T> = std::sync::Mutex<T>;
pub trait RtpSource: Stream<Item = RtpSourceEvent> + Send {
    fn pli_request(self: Pin<&mut Self>);
}

pub trait RtpTarget: Stream<Item = RtpTargetEvent> + Send {
    fn send_rtp(&self, _packet: &mut webrtc::rtp::packet::Packet) {}
}

struct ClientBroadcast {
    id: BroadcastId,
    name: String,

    client_id: ClientId,
    client: Arc<Mutex<SignalingClient>>,

    waker: Option<Waker>,
    source: Pin<Box<dyn RtpSource>>,
    targets: BTreeMap<ClientId, Pin<Box<dyn RtpTarget>>>,
}

impl Future for ClientBroadcast {
    type Output = ();

    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        use Poll::*;

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
                    RtpTargetEvent::RequestPli => pli_request = true,

                    #[allow(unreachable_patterns)]
                    event => info!("Client {} event: {:#?}", client_id, event),
                }
            }

            return false;
        });

        if pli_request {
            // TODO: Throttle pli requests so the source does not gets spammed.
            self.source.as_mut().pli_request();
        }

        while let Ready(event) = self.source.poll_next_unpin(cx) {
            let event = match event {
                Some(event) => event,
                None => {
                    /* the stream has ended... */
                    // TODO: Handle this properly!
                    return Poll::Ready(());
                }
            };

            match event {
                RtpSourceEvent::Media(mut packet) => {
                    /* TODO: Send to everybody! */
                    //trace!("TODO: Handle media event!");
                    for client in self.targets.values() {
                        client.send_rtp(&mut packet);
                    }
                }
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
    client_broadcasts: BTreeMap<ClientId, Vec<(String, BroadcastId)>>,

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
                .send_message(&notify::S2CNotify::NotifyUserJoined(client_id, user_id).into());
        }

        let clients = self
            .client_user_ids
            .iter()
            .filter(|(clid, _uid)| **clid != client_id)
            .map(|(clid, uid)| (*clid, *uid))
            .collect::<Vec<_>>();

        client
            .lock()
            .await
            .send_message(&notify::S2CNotify::NotifyUsers(clients).into());
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
            for (_broadcast_name, broadcast_id) in broadcasts {
                let _broadcast = match self.broadcasts.remove(&broadcast_id) {
                    Some(broadcast) => broadcast,
                    None => continue,
                };

                // TODO: Proper shutdown.
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

    pub async fn client_broadcast_start(
        &mut self,
        client_id: ClientId,
        name: String,
        source: Pin<Box<dyn RtpSource>>,
    ) -> S2CResponse {
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

        for (_broadcast_name, broadcast_id) in client_broadcasts.iter() {
            let broadcast = match self.broadcasts.get(broadcast_id) {
                Some(broadcast) => broadcast,
                None => continue, // Should no happen.
            };

            let broadcast = broadcast.lock().unwrap();
            if broadcast.name.to_lowercase() == name.to_lowercase() {
                return S2CResponse::BroadcastAlreadyRunning {
                    broadcast_id: *broadcast_id,
                };
            }
        }

        let broadcast = ClientBroadcast {
            id: broadcast_id,
            name: name.clone(),

            client: client.clone(),
            client_id,

            waker: None,
            source,
            targets: Default::default(),
        };
        let broadcast = Arc::new(SyncMutex::new(broadcast));

        client_broadcasts.push((name.clone(), broadcast_id));
        self.broadcasts.insert(broadcast_id, broadcast.clone());

        task::spawn(async move {
            future::poll_fn(move |cx| {
                let mut broadcast = broadcast.lock().unwrap();
                broadcast.poll_unpin(cx)
            })
            .await;

            info!("Client broadcast {} ended.", broadcast_id);
        });

        self.subscriber
            .iter()
            .for_each(|s| s.client_broadcast_started(self, client_id, &name));

        S2CResponse::BroadcastStarted { broadcast_id }
    }

    pub async fn client_broadcast_stop(
        &mut self,
        client_id: ClientId,
        broadcast_id: BroadcastId,
    ) -> Result<(), S2CResponse> {
        if let Some(broadcasts) = self.client_broadcasts.get_mut(&client_id) {
            let index = broadcasts
                .iter()
                .enumerate()
                .find(|(_idx, (_name, id))| *id == broadcast_id)
                .map(|(idx, _)| idx);

            let index = match index {
                Some(index) => index,
                None => return Err(S2CResponse::BroadcastUnknownId),
            };

            broadcasts.remove(index);
        } else {
            /* Client has no broadcasts running or is unkown... */
            return Err(S2CResponse::BroadcastUnknownId);
        }

        let _broadcast = match self.broadcasts.remove(&broadcast_id) {
            Some(broadcast) => broadcast,
            // Should not happen since the broadcast was registered...
            None => return Err(S2CResponse::BroadcastUnknownId),
        };

        /* FIXME: Shutdown! */
        Ok(())
    }

    pub async fn client_broadcast_subscribe(
        &mut self,
        client_id: ClientId,
        broadcast_id: BroadcastId,
        target: Pin<Box<dyn RtpTarget>>,
    ) -> Result<(), S2CResponse> {
        let broadcast = match self.broadcasts.get(&broadcast_id) {
            Some(broadcast) => broadcast,
            None => return Err(S2CResponse::BroadcastUnknownId),
        };

        let mut broadcast = broadcast.lock().unwrap();
        if broadcast.targets.contains_key(&client_id) {
            return Err(S2CResponse::BroadcastAlreadySubscribed);
        }

        broadcast.targets.insert(client_id, target);
        if let Some(waker) = &broadcast.waker {
            /* Wake the waker so we poll on the new target as well */
            waker.wake_by_ref();
        }

        Ok(())
    }

    pub async fn client_broadcast_unsubscribe(
        &mut self,
        client_id: ClientId,
        broadcast_id: BroadcastId,
    ) -> Result<(), S2CResponse> {
        let broadcast = match self.broadcasts.get(&broadcast_id) {
            Some(broadcast) => broadcast,
            None => return Err(S2CResponse::BroadcastUnknownId),
        };

        let mut broadcast = broadcast.lock().unwrap();
        let _target = match broadcast.targets.remove(&client_id) {
            Some(target) => target,
            // The client isn't subscribed. Therefor technically he "unsubscribed".
            None => return Ok(()),
        };

        if let Some(waker) = &broadcast.waker {
            /* Wake the waker so we notice that a client has gone. */
            waker.wake_by_ref();
        }

        Ok(())
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

    // TODO: Broadcast type!
    fn client_broadcast_started(
        &self,
        _room: &Room,
        _client_id: ClientId,
        _broadcast_name: &String,
    ) {
    }
    fn client_broadcast_ended(&self, _room: &Room, _client_id: ClientId, _broadcast_name: &String) {
    }
}
