use std::{
    net::SocketAddr,
    sync::{Arc, Weak},
};

use log::{debug, info, trace, warn};
use rocket::{
    async_trait,
    futures::{
        stream::{SplitSink, SplitStream},
        SinkExt, StreamExt,
    },
    tokio,
    tokio::{
        net::TcpStream,
        select,
        sync::{
            mpsc::{self, UnboundedReceiver},
            oneshot, Mutex,
        },
        task,
    },
};
use tokio_tungstenite::WebSocketStream;
use tungstenite::{
    protocol::{frame::coding::CloseCode, CloseFrame},
    Message,
};

use super::messages::{request, response, C2SMessage, S2CMessage};

/// Temporary assigned client id for a connected client.
pub type ClientId = u64;

pub struct SignalingClient {
    pub client_id: ClientId,

    weak_ref: Weak<Mutex<Self>>,
    pub subscriber: Vec<Arc<dyn SignalingClientSubscriber>>,

    pub address: SocketAddr,

    request_handler: Arc<dyn ClientRequestHandler>,
    connection: ClientConnection,
}

enum ClientConnection {
    /// Client is connected.
    Connected {
        sender: mpsc::UnboundedSender<tungstenite::Message>,
        shutdown_read: oneshot::Sender<()>,
        shutdown_write: oneshot::Sender<()>,
    },
    /// Client is in disconnecting state (Flushing pending data; Read has been shut down).
    /// As soon sender is flushed it will switch to Unconnected.
    Disconnecting {
        sender: mpsc::UnboundedSender<tungstenite::Message>,
        shutdown_write: oneshot::Sender<()>,
    },
    Unconnected,
}

impl SignalingClient {
    pub async fn from_socket(
        request_handler: Arc<dyn ClientRequestHandler>,
        client_id: ClientId,
        address: SocketAddr,
        socket: TcpStream,
    ) -> Arc<Mutex<Self>> {
        let (sender, sender_recv) = mpsc::unbounded_channel();

        let (shutdown_read_tx, mut shutdown_read_rx) = oneshot::channel();
        let (shutdown_write_tx, mut shutdown_write_rx) = oneshot::channel();

        let connection = ClientConnection::Connected {
            sender,
            shutdown_read: shutdown_read_tx,
            shutdown_write: shutdown_write_tx,
        };

        let instance = Arc::new(Mutex::new(Self {
            client_id,
            weak_ref: Weak::new(),
            subscriber: Vec::new(),

            address,

            request_handler,
            connection,
        }));

        instance.lock().await.weak_ref = Arc::downgrade(&instance);

        let weak_client = Arc::downgrade(&instance);

        task::spawn(async move {
            let accept_result = tokio::select! {
                result = tokio_tungstenite::accept_async(socket) => Some(result),
                _reason = &mut shutdown_read_rx => None,
                _reason = &mut shutdown_write_rx => None,
            };

            let stream = match accept_result {
                Some(Ok(stream)) => stream,
                Some(Err(error)) => {
                    warn!("Failed to accept client: {}.", error);
                    if let Some(client) = weak_client.upgrade() {
                        // Flush our buffers and close the connection.
                        client.lock().await.close_connection();
                    }
                    return;
                }
                None => {
                    /*
                     * Client has been disconnected, but we can't yet properly signal it.
                     * Just close the connection.
                     */
                    trace!("Received client disconnected while accepting.");
                    return;
                }
            };

            let (write, read) = stream.split();
            tokio::spawn(Self::spawn_read_loop(
                weak_client.clone(),
                read,
                shutdown_read_rx,
            ));
            tokio::spawn(Self::spawn_write_loop(
                weak_client.clone(),
                write,
                sender_recv,
                shutdown_write_rx,
            ));

            if let Some(client) = weak_client.upgrade() {
                let client = client.lock().await;
                client.subscriber
                    .iter()
                    .for_each(|s| s.connected());
            }
        });

        instance
    }

    async fn spawn_write_loop(
        weak_ref: Weak<Mutex<Self>>,
        mut stream: SplitSink<WebSocketStream<TcpStream>, Message>,
        mut message_rx: UnboundedReceiver<Message>,
        mut shutdown_rx: oneshot::Receiver<()>,
    ) {
        loop {
            let message = select! {
                _ = &mut shutdown_rx => break,
                m = message_rx.recv() => m,
            };

            match message {
                Some(message) => {
                    let result = select! {
                        _ = &mut shutdown_rx => break,
                        result = stream.send(message) => result,
                    };

                    if let Err(error) = result {
                        warn!("Failed to send payload: {:?}", error);
                        if let Some(client) = weak_ref.upgrade() {
                            // We're unable to send any more data
                            client.lock().await.close_connection();
                        }
                        break;
                    }
                }
                None => {
                    /* We got nothing more to write. */
                    break;
                }
            }
        }
    }

    async fn spawn_read_loop(
        weak_ref: Weak<Mutex<Self>>,
        mut stream: SplitStream<WebSocketStream<TcpStream>>,
        mut shutdown_rx: oneshot::Receiver<()>,
    ) {
        loop {
            let message = select! {
                _ = &mut shutdown_rx => break,
                m = stream.next() => m
            };

            let message = match message {
                Some(Ok(message)) => message,
                Some(Err(error)) => {
                    warn!("Failed to receive message: {}", error);
                    if let Some(client) = weak_ref.upgrade() {
                        // Flush our buffers and close the connection.
                        client.lock().await.disconnect(Some(CloseFrame {
                            code: CloseCode::Error,
                            reason: "recv failed".into(),
                        }));
                    }
                    break;
                }
                None => {
                    info!("Client disconnected");
                    if let Some(client) = weak_ref.upgrade() {
                        // Flush our buffers and close the connection.
                        client.lock().await.disconnect(None);
                    }
                    break;
                }
            };

            let client = match weak_ref.upgrade() {
                Some(client) => client,
                None => break, // We don't need to read anything any more.
            };

            match message {
                Message::Ping(payload) => {
                    client.lock().await.send_raw(Message::Pong(payload));
                }
                Message::Pong(_payload) => {
                    // TODO: But we're currently not sending pings.
                }
                Message::Text(payload) => {
                    let message: C2SMessage = match serde_json::from_str(&payload) {
                        Ok(message) => message,
                        Err(error) => {
                            debug!("Client send an invalid message: {}", error);
                            continue;
                        }
                    };

                    let request_handler = client.lock().await.request_handler.clone();
                    match message {
                        C2SMessage::Request {
                            request_id,
                            request,
                        } => {
                            let response = request_handler
                                .handle_request(&client, request, request_id)
                                .await;

                            client
                                .lock()
                                .await
                                .send_message(&S2CMessage::Response(request_id, response));
                        }
                    }
                }
                _ => {}
            };
        }
    }

    /// Disconnect the client, but flush all messages.
    /// If client already gets disconnected, nothing happens.
    pub fn disconnect(&mut self, payload: Option<CloseFrame<'static>>) {
        if !matches!(self.connection, ClientConnection::Connected { .. }) {
            // Client already disconnected/is the process of disconnecting.
            return;
        }

        let old_state = std::mem::replace(&mut self.connection, ClientConnection::Unconnected);
        match old_state {
            ClientConnection::Connected {
                sender,
                shutdown_read,
                shutdown_write,
            } => {
                let _ = shutdown_read.send(());

                let weak_ref = self.weak_ref.clone();
                self.connection = ClientConnection::Disconnecting {
                    shutdown_write,
                    sender: sender.clone(),
                };

                // Note: While executing this task, close_connection might already been called!
                task::spawn(async move {
                    let _ = sender.send(tungstenite::Message::Close(payload));
                    // FIXME: Await that all messages have been send!
                    if let Some(instance) = weak_ref.upgrade() {
                        instance.lock().await.close_connection();
                    }
                });
            }
            _ => panic!(),
        }

        self.subscriber
            .iter()
            .for_each(|s| s.connection_closing());
    }

    /// Close the client connection.
    /// Will not flush any pending data.
    pub fn close_connection(&mut self) {
        match std::mem::replace(&mut self.connection, ClientConnection::Unconnected) {
            ClientConnection::Connected {
                shutdown_read,
                shutdown_write,
                ..
            } => {
                let _ = shutdown_read.send(());
                let _ = shutdown_write.send(());
                
                self.subscriber
                    .iter()
                    .for_each(|s| s.connection_closing());
                    
                self.subscriber
                    .iter()
                    .for_each(|s| s.connection_closed());
            }
            ClientConnection::Disconnecting { shutdown_write, .. } => {
                let _ = shutdown_write.send(());
                self.subscriber
                    .iter()
                    .for_each(|s| s.connection_closed());
            }
            _ => return,
        }
    }

    pub fn send_message(&mut self, message: &S2CMessage) {
        let payload = match serde_json::to_string(message) {
            Err(error) => {
                warn!("Failed to serialize message: {}", error);
                return;
            }
            Ok(payload) => payload,
        };

        self.send_raw(Message::Text(payload));
    }

    pub fn send_raw(&mut self, payload: Message) {
        if let ClientConnection::Connected { sender, .. } = &mut self.connection {
            let _ = sender.send(payload);
        }
    }
}

pub trait SignalingClientSubscriber: Sync + Send {
    /// Note: This might never be called since the client can already be connected when the subscriber gets registered.
    fn connected(&self) {}
    fn connection_closing(&self) {}
    fn connection_closed(&self) {}
}

#[async_trait]
pub trait ClientRequestHandler: Sync + Send {
    async fn handle_request(
        &self,
        client: &Arc<Mutex<SignalingClient>>,
        request: request::C2SRequest,
        request_id: u16,
    ) -> response::S2CResponse;
}
