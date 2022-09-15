use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{Arc, Weak},
    time::{Duration, Instant},
};

use log::{debug, info, trace, warn};
use rand::{distributions::Alphanumeric, thread_rng, Rng};
use rocket::{
    async_trait,
    futures::channel::oneshot,
    tokio::{net::TcpListener, select, sync::Mutex, task, time, self},
};

use crate::SERVER_REQUEST_VERSION;

use super::{
    client::{ClientId, ClientRequestHandler, ConnectedClientSubscriber, SignalingClient},
    client_rtp::RtpClient,
    messages::{
        request::{self, C2SRequest},
        response::{self, S2CResponse},
    },
    room::{Room, RoomId},
    JoinTokenError, RoomJoinToken, RtpRoomManager,
};

#[derive(Debug)]
pub struct RoomToken {
    user_id: u64,
    target_room: RoomId,

    issued_at: Instant,
    expires_after: Duration,
}

impl RoomToken {
    pub fn is_expired(&self) -> bool {
        self.issued_at.elapsed() > self.expires_after
    }
}

/// Single instance of a RTP server where clients
/// can connect to.
pub struct RtpServer {
    weak_ref: Weak<Mutex<Self>>,
    address: SocketAddr,

    client_id_index: ClientId,
    client_request_handler: Option<Arc<ServerClientHandler>>,
    clients: BTreeMap<ClientId, Arc<Mutex<SignalingClient>>>,
    client_rooms: BTreeMap<ClientId, RoomId>,
    rtp_clients: BTreeMap<ClientId, Arc<Mutex<RtpClient>>>,

    shutdown_accept: Option<oneshot::Sender<()>>,
    shutdown_tick: Option<oneshot::Sender<()>>,

    tokens: BTreeMap<String, RoomToken>,

    room_id_index: RoomId,
    rooms: BTreeMap<RoomId, Arc<Mutex<Room>>>,
}

impl RtpServer {
    pub async fn new(address: SocketAddr) -> Arc<Mutex<Self>> {
        let instance = Self {
            weak_ref: Default::default(),
            address,

            shutdown_accept: None,
            shutdown_tick: None,

            client_id_index: 0,
            client_request_handler: None,
            clients: Default::default(),
            client_rooms: Default::default(),
            rtp_clients: Default::default(),

            tokens: Default::default(),

            room_id_index: 0,
            rooms: Default::default(),
        };

        let instance = Arc::new(Mutex::new(instance));
        {
            let mut server = instance.lock().await;
            server.weak_ref = Arc::downgrade(&instance);
            server.client_request_handler = Some(Arc::new(ServerClientHandler {
                weak_server: Arc::downgrade(&instance),
            }));
            server.create_room(3165431348352514738).await.unwrap(); // FIXME: Remove, just for testing!
        }

        instance
    }

    pub fn spawn_tick(&mut self) -> anyhow::Result<()> {
        if self.shutdown_tick.is_some() {
            anyhow::bail!("server ticking is already running")
        }

        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        self.shutdown_tick = Some(shutdown_tx);

        let rtp_server = self.weak_ref.upgrade().unwrap();
        task::spawn(async move {
            let mut interval = time::interval(Duration::from_millis(500));
            loop {
                select! {
                    _ = interval.tick() => (),
                    _ = &mut shutdown_rx => break,
                };

                let mut server = rtp_server.lock().await;
                server.tick().await;
            }
        });

        Ok(())
    }

    pub async fn start_server(&mut self) -> anyhow::Result<()> {
        if self.shutdown_accept.is_some() {
            anyhow::bail!("server already running")
        }

        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        self.shutdown_accept = Some(shutdown_tx);

        let rtp_server = self.weak_ref.upgrade().unwrap();
        let server = TcpListener::bind(&self.address).await?;
        task::spawn(async move {
            loop {
                let (stream, address) = select! {
                    _ = &mut shutdown_rx => break,
                    result = server.accept() => match result {
                        Ok(result) => result,
                        Err(error) => {
                            warn!("Failed to accept new client: {}", error);
                            continue;
                        }
                    }
                };

                let (client_id, request_handler) = {
                    let mut server = rtp_server.lock().await;

                    server.client_id_index = server.client_id_index.wrapping_add(1);
                    (
                        server.client_id_index,
                        server
                            .client_request_handler
                            .clone()
                            .expect("missing request handler"),
                    )
                };
                info!("Received new client form {} -> {}", address, client_id);

                let client =
                    SignalingClient::from_socket(request_handler, client_id, address, stream).await;
                client
                    .lock()
                    .await
                    .subscriber
                    .push(Arc::new(ServerClientSubscriber {
                        weak_client: Arc::downgrade(&client),
                        weak_server: Arc::downgrade(&rtp_server),
                    }));

                {
                    let mut rtp_server = rtp_server.lock().await;
                    rtp_server.clients.insert(client_id, client);
                }
            }
            debug!("RtpServer accept loop stopped.");
        });

        Ok(())
    }

    async fn tick(&mut self) {
        let removed_tokens = self
            .tokens
            .drain_filter(|_, token| token.is_expired())
            .count();
        if removed_tokens > 0 {
            trace!("Removed {} expired token(s).", removed_tokens);
        }
        // TODO: Clean up empty rooms.
    }
}

#[async_trait]
impl RtpRoomManager for RtpServer {
    async fn create_room(&mut self, _owner_id: u64) -> anyhow::Result<RoomId> {
        // FIXME: Generate a random 61 bit id
        self.room_id_index = self.room_id_index.wrapping_add(1);
        let room_id = self.room_id_index;

        let room = Room::new(room_id).await;
        self.rooms.insert(room_id, room);

        Ok(room_id)
    }

    async fn delete_room(&mut self, room_id: RoomId) -> anyhow::Result<bool> {
        // FIXME: Proper shutdown!
        Ok(self.rooms.remove(&room_id).is_some())
    }

    async fn create_join_token(
        &mut self,
        user_id: u64,
        room_id: RoomId,
        expires_after: Duration,
    ) -> Result<RoomJoinToken, JoinTokenError> {
        if !self.rooms.contains_key(&room_id) {
            return Err(JoinTokenError::RoomDoesNotExist);
        }

        let token: String = thread_rng()
            .sample_iter(&Alphanumeric)
            .take(30)
            .map(char::from)
            .collect();

        self.tokens.insert(
            token.clone(),
            RoomToken {
                user_id,
                target_room: room_id,
                issued_at: Instant::now(),
                expires_after,
            },
        );

        // FIXME: Get public server URL!
        Ok(RoomJoinToken {
            server_url: "ws://localhost:8088".to_string(),
            token,
        })
    }
}

struct ServerClientSubscriber {
    weak_server: Weak<Mutex<RtpServer>>,
    weak_client: Weak<Mutex<SignalingClient>>,
}

impl ConnectedClientSubscriber for ServerClientSubscriber {
    fn connection_closing(&self) {
        // TODO: Unregister the client from all rooms etc.
    }

    fn connection_closed(&self) {
        let client = match self.weak_client.upgrade() {
            Some(client) => client,
            None => return,
        };
        let server = match self.weak_server.upgrade() {
            Some(server) => server,
            None => return,
        };

        tokio::spawn(async move {
            let client_id = client.lock().await.client_id;
            let address = format!("{}", client.lock().await.address);

            let mut server = server.lock().await;
            match server.clients.remove(&client_id) {
                Some(_client) => trace!("Removed client {}.", address),
                None => warn!("Having client connection closed event of unregistered client."),
            };

            if let Some(room_id) = server.client_rooms.remove(&client_id) {
                if let Some(room) = server.rooms.get(&room_id) {
                    let room = room.clone();
                    tokio::spawn(async move {
                        let mut room = room.lock().await;
                        room.remove_client(client_id).await;
                    });
                }
            }

            if let Some(rtp_client) = server.rtp_clients.remove(&client_id) {
                // Async since closing might aquire some locks.
                tokio::spawn(async move {
                    let mut rtp_client = rtp_client.lock().await;
                    rtp_client.close().await;
                });
            }
        });
    }
}

macro_rules! handler_get_current_room {
    ($server:expr, $client_id:expr) => {
        {
            let room = {
                let server = $server.lock().await;
                server
                    .client_rooms
                    .get(&$client_id)
                    .map(|room_id| server.rooms.get(room_id))
                    .flatten()
                    .cloned()
            };
    
            match room {
                Some(room) => room,
                None => return S2CResponse::SessionNotInitialized,
            }
        }
    };
}

// TODO: Make individual for each client and store the client id as well as a reference (maybe weak?)
struct ServerClientHandler {
    weak_server: Weak<Mutex<RtpServer>>,
}

// Attention: First lock the server, then the client!
#[async_trait]
impl ClientRequestHandler for ServerClientHandler {
    async fn handle_request(
        &self,
        client: &Arc<Mutex<SignalingClient>>,
        request: request::C2SRequest,
        _request_id: u16,
    ) -> response::S2CResponse {
        let server = match self.weak_server.upgrade() {
            Some(server) => server,
            None => {
                return S2CResponse::InternalError {
                    message: "server went away".to_string(),
                };
            }
        };

        let client_id = client.lock().await.client_id;
        match request {
            C2SRequest::InitializeSesstion(payload) => {
                if payload.version != SERVER_REQUEST_VERSION {
                    return S2CResponse::SessionUnsupportedVersion {
                        server_version: SERVER_REQUEST_VERSION,
                    };
                }

                let token_info = {
                    let mut server = server.lock().await;
                    if server.client_rooms.contains_key(&0) {
                        // FIXME: Proper check and allow the client to leave a session but still stay connected (in cases of switches).
                        return S2CResponse::SessionAlreadyInitialized {};
                    }

                    let token_info = match server.tokens.remove(&payload.token) {
                        Some(info) => info,
                        None => return S2CResponse::SessionInvalidToken,
                    };

                    if token_info.is_expired() {
                        return S2CResponse::SessionInvalidToken;
                    }

                    token_info
                };

                let rtp_client = {
                    let mut server = server.lock().await;
                    if let Some(rtp_client) = server.rtp_clients.get(&client_id) {
                        // FIXME: Apply offer & renegotiate / reset RtpClient?

                        rtp_client.clone()
                    } else {
                        // TODO: Depending on how long this requires may not lock the server.
                        let rtp_client = match RtpClient::new(client_id, &client).await {
                            Err(error) => {
                                return S2CResponse::InternalError {
                                    message: format!("failed to create rtp client: {:#}", error),
                                }
                            }
                            Ok(client) => client,
                        };
                        server.rtp_clients.insert(client_id, rtp_client.clone());
                        rtp_client
                    }
                };

                let sdp_answer = {
                    let mut rtp_client = rtp_client.lock().await;
                    match rtp_client.apply_offer(payload.offer).await {
                        Err(error) => {
                            return S2CResponse::InternalError {
                                message: format!("failed to apply offer: {:#}", error),
                            }
                        }
                        Ok(sdp_answer) => sdp_answer,
                    }
                };

                {
                    let mut server = server.lock().await;
                    let room_id = {
                        let room = match server.rooms.get(&token_info.target_room) {
                            Some(room) => room,
                            None => return S2CResponse::SessionRoomClosed,
                        };

                        let mut room = room.lock().await;
                        room.add_client(client, token_info.user_id).await;
                        room.id
                    };
                    server.client_rooms.insert(client_id, room_id);
                }

                return S2CResponse::SessionInitializeSuccess { answer: sdp_answer };
            }
            C2SRequest::NegotiationOffer(payload) => {
                let rtp_client = match server.lock().await.rtp_clients.get(&client_id).cloned() {
                    Some(rtp_client) => rtp_client,
                    None => return S2CResponse::RtpNotInitialized,
                };

                {
                    let mut rtp_client = rtp_client.lock().await;
                    match rtp_client.apply_offer(payload.offer).await {
                        Err(error) => S2CResponse::InternalError {
                            message: format!("failed to create offer: {:#}", error),
                        },
                        Ok(sdp_answer) => {
                            S2CResponse::NegotiationOfferSuccess { answer: sdp_answer }
                        }
                    }
                }
            }
            C2SRequest::NegotiationAnswer(payload) => {
                let rtp_client = match server.lock().await.rtp_clients.get(&client_id).cloned() {
                    Some(rtp_client) => rtp_client,
                    None => return S2CResponse::RtpNotInitialized,
                };

                let mut rtp_client = rtp_client.lock().await;
                if let Err(error) = rtp_client.apply_answer(payload.answer).await {
                    S2CResponse::InternalError {
                        message: format!("failed to apply answer: {:#}", error),
                    }
                } else {
                    S2CResponse::Success
                }
            }
            C2SRequest::IceCandidates(payload) => {
                let rtp_client = match server.lock().await.rtp_clients.get(&client_id).cloned() {
                    Some(rtp_client) => rtp_client,
                    None => return S2CResponse::RtpNotInitialized,
                };

                let mut rtp_client = rtp_client.lock().await;
                if let Err(error) = rtp_client.apply_ice_candidates(&payload.candidates).await {
                    return S2CResponse::InternalError {
                        message: format!("failed to apply ice candidates: {:#}", error),
                    };
                }

                if payload.finished {
                    if let Err(error) = rtp_client.apply_ice_candidates_finished().await {
                        return S2CResponse::InternalError {
                            message: format!("failed to apply ice candidate finish: {:#}", error),
                        };
                    }
                }
                return S2CResponse::Success;
            }
            C2SRequest::BroadcastStart(payload) => {
                let rtp_client = match server.lock().await.rtp_clients.get(&client_id).cloned() {
                    Some(rtp_client) => rtp_client,
                    None => return S2CResponse::RtpNotInitialized,
                };

                let room = {
                    let server = server.lock().await;
                    server
                        .client_rooms
                        .get(&client_id)
                        .map(|room_id| server.rooms.get(room_id))
                        .flatten()
                        .cloned()
                };

                let room = match room {
                    Some(room) => room,
                    None => return S2CResponse::SessionNotInitialized,
                };

                let source = {
                    let mut rtp_client = rtp_client.lock().await;
                    rtp_client.create_rtc_source(&payload.source)
                };

                let source = match source {
                    Some(source) => source,
                    None => return S2CResponse::BroadcastSourceUnknownId,
                };

                let broadcast_id = {
                    let mut room = room.lock().await;
                    let result = room
                        .client_broadcast_start(client_id, payload.name.clone(), source)
                        .await;
                    if let S2CResponse::BroadcastStarted { broadcast_id } = &result {
                        *broadcast_id
                    } else {
                        return result;
                    }
                };

                return S2CResponse::BroadcastStarted { broadcast_id };
            },
            C2SRequest::BroadcastSubscribe { broadcast_id } => {
                let room = handler_get_current_room!(server, client_id);

                let rtp_client = match server.lock().await.rtp_clients.get(&client_id).cloned() {
                    Some(rtp_client) => rtp_client,
                    None => return S2CResponse::RtpNotInitialized,
                };
                
                let target = {
                    let mut rtp_client = rtp_client.lock().await;
                    rtp_client.create_rtc_target().await
                };

                {
                    let mut room = room.lock().await;

                    let result = room.client_broadcast_subscribe(client_id, broadcast_id, target).await;
                    if !matches!(result, S2CResponse::Success) {
                        return result;
                    }
                };

                // TODO: Return the stream id...
                S2CResponse::Success
            },
            C2SRequest::BroadcastUnsubscribe { broadcast_id } => {
                let room = handler_get_current_room!(server, client_id);

                {
                    let mut room = room.lock().await;
                    room.client_broadcast_unsubscribe(client_id, broadcast_id).await
                }
            },
            C2SRequest::BroadcastStop { broadcast_id } => {
                let room = handler_get_current_room!(server, client_id);

                {
                    let mut room = room.lock().await;
                    room.client_broadcast_stop(client_id, broadcast_id).await
                }
            },

            #[allow(unreachable_patterns)]
            _ => S2CResponse::UnknownRequest,
        }
    }
}
