use log::{debug, error, info, trace, warn};
use rand::{thread_rng, distributions::Alphanumeric, Rng};
use rocket::{
    async_trait,
    tokio::{
        self, select,
        sync::{mpsc, Mutex, oneshot},
        task,
    },
};
use std::{
    collections::VecDeque,
    ops::{DerefMut, Deref},
    pin::Pin,
    sync::{Arc, Weak, atomic::{AtomicBool}},
    task::{Context, Poll},
};
use tokio_stream::Stream;
use webrtc::{
    api::{
        interceptor_registry::register_default_interceptors,
        media_engine::{MediaEngine, MIME_TYPE_OPUS, MIME_TYPE_VP8},
        setting_engine::SettingEngine,
        APIBuilder,
    },
    ice::network_type::NetworkType,
    ice_transport::{ice_candidate::RTCIceCandidateInit, ice_server::RTCIceServer},
    interceptor::registry::Registry,
    peer_connection::{
        configuration::RTCConfiguration,
        sdp::{sdp_type::RTCSdpType, session_description::RTCSessionDescription},
        signaling_state::RTCSignalingState,
        RTCPeerConnection,
    },
    rtcp::{
        payload_feedbacks::{picture_loss_indication::PictureLossIndication, receiver_estimated_maximum_bitrate::ReceiverEstimatedMaximumBitrate},
        receiver_report::ReceiverReport,
        transport_feedbacks::{
            transport_layer_cc::TransportLayerCc, transport_layer_nack::TransportLayerNack,
        }, sender_report::SenderReport, source_description::SourceDescription,
    },
    rtp_transceiver::{
        rtp_codec::{
            RTCRtpCodecCapability, RTCRtpCodecParameters, RTCRtpHeaderExtensionCapability,
            RTPCodecType,
        },
        rtp_receiver::RTCRtpReceiver,
        PayloadType, SSRC, rtp_sender::RTCRtpSender,
    },
    track::{
        track_local::{TrackLocal, TrackLocalContext, TrackLocalWriter},
        track_remote::TrackRemote,
    },
};

use crate::rtp_server::messages::notify;

use super::{
    client::{ClientId, SignalingClient},
    messages::{IceCandidate, S2CMessage},
    room::{RtpSource, RtpSourceEvent, RtpTarget, RtpTargetEvent},
};

struct RtpClientTrackSource {
    weak_track: Weak<RegisteredReceivingTrack>,

    ssrc: SSRC,
    events: mpsc::Receiver<RtpSourceEvent>,
    rtcp_sender: RtcpSender,

    recv_handler: Arc<RtpSourceHandler>,
}

impl RtpSource for RtpClientTrackSource {
    fn pli_request(self: Pin<&mut Self>) {
        let sender = self.rtcp_sender.clone();
        let ssrc = self.ssrc;
        tokio::spawn(async move {
            let _ = sender
                .send(vec![Box::new(PictureLossIndication {
                    sender_ssrc: 0,
                    media_ssrc: ssrc,
                })])
                .await;
        });
    }
}

impl Stream for RtpClientTrackSource {
    type Item = RtpSourceEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.events.poll_recv(cx)
    }
}

impl Drop for RtpClientTrackSource {
    fn drop(&mut self) {
        if let Some(track) = self.weak_track.upgrade() {
            // Remove the handler from the track.
            let handler = self.recv_handler.clone() as Arc<dyn ReceivingTrackHandler>;
            track.remove_handler(&handler);
        }
    }
}

struct RtpSourceHandler {
    events: mpsc::Sender<RtpSourceEvent>,
}

#[async_trait]
impl ReceivingTrackHandler for RtpSourceHandler {
    async fn handle_rtp_packet(&self, packet: webrtc::rtp::packet::Packet) {
        let _ = self.events.send(RtpSourceEvent::Media(packet)).await;
    }

    async fn handle_rtcp_packet(&self, packet: Box<dyn webrtc::rtcp::packet::Packet + Send + Sync>) {
        if let Some(_sr) = packet.as_any().downcast_ref::<SenderReport>() {

        } else if let Some(_sd) = packet.as_any().downcast_ref::<SourceDescription>() {

        } else {
            trace!("Having RTCP {:#}.", packet);
        }
    }

    async fn handle_rtp_error(&self, _error: &webrtc::Error) -> bool { false }
    async fn handle_rtcp_error(&self, _error: &webrtc::Error) -> bool { false }


    async fn handle_unmount(&self) {
        let _ = self.events.send(RtpSourceEvent::End).await;
    }

    async fn handle_closed(&self) {
        let _ = self.events.send(RtpSourceEvent::End).await;
    }
}

/// Receiving track handler.
/// Note: Callback methods are blocking the handle method.
#[async_trait]
trait ReceivingTrackHandler: Send + Sync {
    async fn handle_rtp_packet(&self, packet: webrtc::rtp::packet::Packet);
    async fn handle_rtcp_packet(&self, packet: Box<dyn webrtc::rtcp::packet::Packet + Send + Sync>);

    /// Handle a rtp read error.
    /// If returned `true` the error will be consumed and not default handled
    /// which most likely will cause the connection to close.
    async fn handle_rtp_error(&self, error: &webrtc::Error) -> bool;

    /// Handle a rtcp read error.
    /// If returned `true` the error will be consumed and not default handled
    /// which most likely will cause the connection to close.
    async fn handle_rtcp_error(&self, error: &webrtc::Error) -> bool;

    async fn handle_unmount(&self);
    async fn handle_closed(&self);
}

struct RegisteredReceivingTrack {
    stream_id: String,
    weak_client: Weak<Mutex<RtpClient>>,

    track: Arc<TrackRemote>,
    receiver: Arc<RTCRtpReceiver>,
    rtcp_sender: RtcpSender,

    closed: Arc<AtomicBool>,
    handler: parking_lot::Mutex<Option<Arc<dyn ReceivingTrackHandler>>>,
}

impl RegisteredReceivingTrack {
    fn get_handler(&self) -> Option<Arc<dyn ReceivingTrackHandler>> {
        let handler = self.handler.lock();
        handler.as_ref().cloned()
    }
}

fn webrtc_error_is_disconnect(error: &webrtc::Error) -> bool {
    use webrtc::Error;


    let text = error.to_string();
    if text.ends_with("buffer: closed") {
        trace!("Buffer-Error: {:#?}", error);
        return true;
    }
    error == &Error::ErrDataChannelNotOpen || error == &Error::ErrClosedPipe
}

async fn registered_receive_track_io_loop(receiving_track: Arc<RegisteredReceivingTrack>) {
    loop {
        select! {
            rtp = receiving_track.track.read_rtp() => {
                let (rtp, _attributes) = match rtp {
                    Err(error) => {
                        if let Some(handler) = receiving_track.get_handler() {
                            if handler.handle_rtp_error(&error).await {
                                // Error handled, we can continue.
                                continue;
                            }
                        }

                        if !webrtc_error_is_disconnect(&error) {
                            error!("RTP read error: {:#?}. Track {} closed.", error, receiving_track.stream_id);
                            /* TODO: Improve handling */
                        } else {
                            // TODO: Figure out why sometimes we get an Srtp Util ErrBufferClosed here.
                            trace!("Receiving track {} closed (rtp disconnect error).", receiving_track.stream_id);
                        }
                        break;
                    },
                    Ok(payload) => payload
                };

                if let Some(handler) = receiving_track.get_handler() {
                    handler.handle_rtp_packet(rtp).await;
                }
            },
            rtcp = receiving_track.receiver.read_rtcp() => {
                let (rtcps, _attributes) = match rtcp {
                    Err(error) => {
                        if let Some(handler) = receiving_track.get_handler() {
                            if handler.handle_rtcp_error(&error).await {
                                // Error handled, we can continue.
                                continue;
                            }
                        }

                        if !webrtc_error_is_disconnect(&error) {
                            error!("RTCP read error: {:#?}. Track {} closed.", error, receiving_track.stream_id);
                            /* TODO: Improve handling */
                        } else {
                            trace!("Receiving track {} closed (rtcp disconnect error).", receiving_track.stream_id);
                        }
                        break;
                    },
                    Ok(rtcp) => rtcp
                };
                
                for rtcp in rtcps {
                    if let Some(handler) = receiving_track.get_handler() {
                        handler.handle_rtcp_packet(rtcp).await;
                    }
                }
            }
        }
    }
}

impl RegisteredReceivingTrack {
    /// Update the handler for this track.
    /// If not handler is specified, all data will be voided. 
    fn update_handler(&self, new_handler: Option<Arc<dyn ReceivingTrackHandler>>) {
        let mut handler = self.handler.lock();
        if let Some(old_handler) = handler.take() {
            tokio::task::spawn(async move {
                old_handler.handle_unmount().await;
            });
        }

        *handler = new_handler;
    }
    
    fn remove_handler(&self, handler: &Arc<dyn ReceivingTrackHandler>) {
        let mut current_handler = self.handler.lock();
        let handler_equal = current_handler
            .as_ref()
            .map(|h| Arc::ptr_eq(h, handler))
            .unwrap_or(false);
            
        if handler_equal {
            if let Some(old_handler) = current_handler.take() {
                tokio::task::spawn(async move {
                    old_handler.handle_unmount().await;
                });
            }
        }
    }
}

impl Drop for RegisteredReceivingTrack {
    fn drop(&mut self) {
        trace!("Dropping ReceivingTrack {}", self.stream_id);
    }
}

struct RtpClientTarget {
    weak_client: Weak<Mutex<RtpClient>>,
    track: Arc<RegisteredSendingTrack>,
    events: mpsc::Receiver<RtpTargetEvent>,
}

impl RtpTarget for RtpClientTarget {
    fn send_rtp(&self, packet: &mut webrtc::rtp::packet::Packet) {
        let _ = self.track.rtp_send_channel.try_send(packet.clone());
    }
}

impl Stream for RtpClientTarget {
    type Item = RtpTargetEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.events.poll_recv(cx)
    }
}

impl Drop for RtpClientTarget {
    fn drop(&mut self) {
        // TODO: Remove the handler from the track and allow other clients to use that track
    }
}

struct SendingTrackBinding {
    id: String,
    ssrc: SSRC,
    payload_type: PayloadType,
    write_stream: Arc<dyn TrackLocalWriter + Send + Sync>,
}

struct RegisteredSendingTrack {
    id: String,
    binding: Mutex<Option<SendingTrackBinding>>,

    shutdown_tx: parking_lot::Mutex<Option<oneshot::Sender<()>>>,
    event_handler: parking_lot::Mutex<Option<mpsc::Sender<RtpTargetEvent>>>,

    rtp_send_channel: mpsc::Sender<webrtc::rtp::packet::Packet>,
}

impl RegisteredSendingTrack {
    fn get_event_handler(&self) -> Option<mpsc::Sender<RtpTargetEvent>> {
        let handler = self.event_handler.lock();
        handler.deref().clone()
    }

    /// Returns true if the error has been consumed
    /// else false if it aborts the read loop.
    async fn handle_rtcp_error(&self, _error: &webrtc::Error) -> bool {
        false
    }

    async fn handle_rtcp(&self, event: Box<dyn webrtc::rtcp::packet::Packet + Send + Sync>) {
        if let Some(_) = event.as_any().downcast_ref::<TransportLayerCc>() {
            //trace!("Received TransportLayerCc");
        } else if let Some(pli) = event.as_any().downcast_ref::<PictureLossIndication>() {
            trace!(
                "Received PLI request from {} for {}.",
                pli.sender_ssrc,
                pli.media_ssrc
            );

            if let Some(handler) = self.get_event_handler() {
                let _ = handler.send(RtpTargetEvent::RequestPli).await;
            }
        } else if let Some(_rr) = event.as_any().downcast_ref::<ReceiverReport>() {
            // TODO: Get current track id and extract own report.
            trace!("Received ReceiverReport.");
        } else if let Some(_nack) = event.as_any().downcast_ref::<TransportLayerNack>() {
            /* Nack handling itself already done by an interceptor. */
        } else if let Some(_remb) = event.as_any().downcast_ref::<ReceiverEstimatedMaximumBitrate>() {
            /* Thanks for the information. We may later want to increase/decrese video quality based on that information. */
        } else if let Some(_remb) = event.as_any().downcast_ref::<SourceDescription>() {
            /* TODO: I'm not sure why we get this, needs more investigation. */
        } else {
            trace!("RTCP send read rtcp: {:#?}", event);
        }
    }
}

#[async_trait]
impl TrackLocal for RegisteredSendingTrack {
    async fn bind(&self, ctx: &TrackLocalContext) -> webrtc::error::Result<RTCRtpCodecParameters> {
        let write_stream = ctx
            .write_stream()
            .ok_or(webrtc::error::Error::ErrRTPSenderTrackNil)?;

        let codec = ctx
            .codec_parameters()
            .iter()
            .find(|p| p.capability.mime_type == MIME_TYPE_VP8)
            .ok_or(webrtc::error::Error::ErrCodecNotFound)?;

        *self.binding.lock().await = Some(SendingTrackBinding {
            id: ctx.id().clone(),
            ssrc: ctx.ssrc(),

            payload_type: codec.payload_type,
            write_stream,
        });

        Ok(codec.clone())
    }

    async fn unbind(&self, _ctx: &TrackLocalContext) -> webrtc::error::Result<()> {
        self.binding.lock().await.take();
        Ok(())
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn stream_id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> RTPCodecType {
        RTPCodecType::Video
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

async fn registered_send_track_io_loop(track: Arc<RegisteredSendingTrack>, sender: Arc<RTCRtpSender>, mut rtp_rx: mpsc::Receiver<webrtc::rtp::packet::Packet>) {
    enum LoopEvent {
        Shutdown,
        RtpSend(webrtc::rtp::packet::Packet),
        RtcpReceive(Vec<Box<dyn webrtc::rtcp::packet::Packet + Send + Sync>>),
        RtcpReceiveError(webrtc::Error),

        Noop,
    }

    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

    let old_shutdown_tx = {
        let mut shutdown_tx_handle = track.shutdown_tx.lock();
        shutdown_tx_handle.replace(shutdown_tx)
    };
    if let Some(old_shutdown_tx) = old_shutdown_tx {
        let _ = old_shutdown_tx.send(());
    }

    loop {
        let event = select! {
            _ = &mut shutdown_rx => {
                LoopEvent::Shutdown
            },
            packet = rtp_rx.recv() => {
                if let Some(packet) = packet {
                    LoopEvent::RtpSend(packet)
                } else {
                    LoopEvent::Noop
                }
            },
            event = sender.read_rtcp() => {
                match event {
                    Err(error) => LoopEvent::RtcpReceiveError(error),
                    Ok((packets, _attributes)) => LoopEvent::RtcpReceive(packets),
                }
            },
        };

        match event {
            LoopEvent::Shutdown => break,
            LoopEvent::RtcpReceive(packets) => {
                for event in packets {
                    track.handle_rtcp(event).await;
                }
            },
            LoopEvent::RtcpReceiveError(error) => {
                if track.handle_rtcp_error(&error).await {
                    continue
                }

                if !webrtc_error_is_disconnect(&error) {
                    warn!("RTP Sender rtcp read error: {:#}. Stopping sender.", error);
                }
                break;
            },
            LoopEvent::RtpSend(mut packet) => {
                let binding = track.binding.lock().await;
                if let Some(binding) = binding.as_ref() {
                    packet.header.ssrc = binding.ssrc;
                    packet.header.payload_type = binding.payload_type;

                    let _ = binding.write_stream.write_rtp(&packet).await;
                }
            },
            LoopEvent::Noop => {}
        }
    }
}

type RtcpSender = mpsc::Sender<Vec<Box<dyn webrtc::rtcp::packet::Packet + Send + Sync>>>;
pub struct RtpClient {
    weak_ref: Weak<Mutex<Self>>,
    signaling_client_ref: Weak<Mutex<SignalingClient>>,

    client_id: ClientId,
    peer: webrtc::peer_connection::RTCPeerConnection,

    receiving_tracks: VecDeque<Arc<RegisteredReceivingTrack>>,
    sending_tracks: VecDeque<Arc<RegisteredSendingTrack>>,

    rtcp_channel: RtcpSender,
}

impl RtpClient {
    async fn create_peer() -> anyhow::Result<RTCPeerConnection> {
        let mut media = MediaEngine::default();
        //media.register_default_codecs()?;

        media.register_codec(
            RTCRtpCodecParameters {
                capability: RTCRtpCodecCapability {
                    mime_type: MIME_TYPE_OPUS.to_owned(),
                    clock_rate: 48000,
                    channels: 2,
                    sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                    rtcp_feedback: vec![],
                },
                payload_type: 111,
                ..Default::default()
            },
            RTPCodecType::Audio,
        )?;

        {
            // RTCP feedback will automatically be registered by interceptors.
            let video_rtcp_feedback = vec![];
            for codec in vec![
                RTCRtpCodecParameters {
                    capability: RTCRtpCodecCapability {
                        mime_type: MIME_TYPE_VP8.to_owned(),
                        clock_rate: 90000,
                        channels: 0,
                        sdp_fmtp_line: "x-google-max-bitrate=780000".to_owned(),
                        rtcp_feedback: video_rtcp_feedback.clone(),
                    },
                    payload_type: 96,
                    ..Default::default()
                },
                // RTCRtpCodecParameters {
                //     capability: RTCRtpCodecCapability {
                //         mime_type: MIME_TYPE_VP9.to_owned(),
                //         clock_rate: 90000,
                //         channels: 0,
                //         sdp_fmtp_line: "x-google-max-bitrate=780000;profile-id=0".to_owned(),
                //         rtcp_feedback: video_rtcp_feedback.clone(),
                //     },
                //     payload_type: 98,
                //     ..Default::default()
                // },
                // RTCRtpCodecParameters {
                //     capability: RTCRtpCodecCapability {
                //         mime_type: MIME_TYPE_VP9.to_owned(),
                //         clock_rate: 90000,
                //         channels: 0,
                //         sdp_fmtp_line: "x-google-max-bitrate=780000;profile-id=1".to_owned(),
                //         rtcp_feedback: video_rtcp_feedback.clone(),
                //     },
                //     payload_type: 100,
                //     ..Default::default()
                // },
            ] {
                media.register_codec(codec, RTPCodecType::Video)?;
            }
        }

        for extension in vec![
            "urn:ietf:params:rtp-hdrext:sdes:mid",
            "urn:ietf:params:rtp-hdrext:sdes:rtp-stream-id",
            "urn:ietf:params:rtp-hdrext:sdes:repaired-rtp-stream-id",
        ] {
            media.register_header_extension(
                RTCRtpHeaderExtensionCapability {
                    uri: extension.to_owned(),
                },
                RTPCodecType::Video,
                vec![],
            )?;
        }

        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut media)?;
        // TODO: Add seq no & timestamp fixup interceptor

        let mut setting_engine = SettingEngine::default();

        setting_engine.set_network_types(vec![
            NetworkType::Udp4,
            NetworkType::Udp6,
            NetworkType::Tcp4,
            NetworkType::Tcp6,
        ]);

        let api = APIBuilder::new()
            .with_interceptor_registry(registry)
            .with_media_engine(media)
            .with_setting_engine(setting_engine)
            .build();

        let config = RTCConfiguration {
            ice_servers: vec![RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_owned()],
                ..Default::default()
            }],
            ..Default::default()
        };

        Ok(api.new_peer_connection(config).await?)
    }

    pub async fn new(
        client_id: ClientId,
        signaling_client: &Arc<Mutex<SignalingClient>>,
    ) -> anyhow::Result<Arc<Mutex<Self>>> {
        let (rtcp_tx, mut rtcp_rx) = mpsc::channel(16);
        let instance = Arc::new(Mutex::new(Self {
            weak_ref: Default::default(),
            signaling_client_ref: Arc::downgrade(signaling_client),

            client_id,
            peer: Self::create_peer().await?,

            receiving_tracks: Default::default(),
            sending_tracks: Default::default(),

            rtcp_channel: rtcp_tx,
        }));

        {
            let mut client = instance.lock().await;
            client.weak_ref = Arc::downgrade(&instance);
            client.initialize_peer().await?;
        }

        {
            let weak_instance = Arc::downgrade(&instance);
            tokio::spawn(async move {
                while let Some(packets) = rtcp_rx.recv().await {
                    let instance = match weak_instance.upgrade() {
                        Some(instance) => instance,
                        None => break,
                    };

                    let instance = instance.lock().await;
                    let _ = instance.peer.write_rtcp(packets.as_slice()).await;
                }
            });
        }

        Ok(instance)
    }

    pub async fn apply_offer(&mut self, offer: String) -> Result<String, webrtc::Error> {
        let mut desc: RTCSessionDescription = Default::default();
        desc.sdp_type = RTCSdpType::Offer;
        desc.sdp = offer;
        self.peer.set_remote_description(desc).await?;

        let local_desc = self.peer.create_answer(None).await?;
        self.peer.set_local_description(local_desc.clone()).await?;
        Ok(local_desc.sdp)
    }

    pub async fn apply_answer(&mut self, answer: String) -> Result<(), webrtc::Error> {
        let mut desc: RTCSessionDescription = Default::default();
        desc.sdp_type = RTCSdpType::Answer;
        desc.sdp = answer;
        self.peer.set_remote_description(desc).await?;
        Ok(())
    }

    pub async fn apply_ice_candidates(
        &mut self,
        candidates: &[IceCandidate],
    ) -> Result<(), webrtc::Error> {
        for candidate in candidates {
            self.peer
                .add_ice_candidate(candidate.clone().into())
                .await?;
        }

        Ok(())
    }

    async fn execute_renegotiation(&mut self, force: bool) -> Result<(), webrtc::Error> {
        debug!(
            "[{}] Nego? {:#?} (force: {})",
            self.client_id,
            self.peer.signaling_state(),
            force
        );
        // if !force && self.peer.signaling_state() == RTCSignalingState::Stable {
        //     // No need to signal anything.
        //     return Ok(());
        // }

        debug!("[{}] Executing renegotiation.", self.client_id);
        let offer = self.peer.create_offer(None).await?;
        self.peer.set_local_description(offer.clone()).await?;
        Self::signaling_notify(
            &self.signaling_client_ref,
            notify::S2CNotify::NotifyNegotiationOffer(offer.sdp),
        );
        Ok(())
    }

    pub async fn apply_ice_candidates_finished(&mut self) -> Result<(), webrtc::Error> {
        let init: RTCIceCandidateInit = Default::default();
        self.peer.add_ice_candidate(init).await?;
        Ok(())
    }

    async fn initialize_peer(&mut self) -> anyhow::Result<()> {
        self.peer
            .on_peer_connection_state_change(Box::new(move |state| {
                Box::pin(async move {
                    info!("Connection state changed to {}", state);
                })
            }))
            .await;
        self.peer
            .on_signaling_state_change(Box::new(move |state| {
                Box::pin(async move {
                    info!("Signaling state changed to {}", state);
                })
            }))
            .await;

        self.peer
            .on_ice_connection_state_change(Box::new(move |state| {
                Box::pin(async move {
                    info!("ICE connection state changed to {}", state);
                })
            }))
            .await;

        self.peer
            .on_ice_gathering_state_change(Box::new(move |state| {
                Box::pin(async move {
                    info!("ICE gathering state changed to {}", state);
                })
            }))
            .await;

        let signaling_client = self.signaling_client_ref.clone();
        self.peer
            .on_ice_candidate(Box::new(move |candidate| {
                let signaling_client = signaling_client.clone();

                // Sending candidates must be in order (especially since we might send a candidate finish).
                Box::pin(async move {
                    if let Some(candidate) = candidate {
                        let candidate = match candidate.to_json().await {
                            Ok(candidate) => candidate,
                            Err(error) => {
                                error!("Failed to encode ICE candidate as JSON: {}", error);
                                return;
                            }
                        };

                        Self::signaling_notify(
                            &signaling_client,
                            notify::S2CNotify::NotifyIceCandidate(candidate.into()),
                        );
                    } else {
                        Self::signaling_notify(
                            &signaling_client,
                            notify::S2CNotify::NotifyIceCandidateFinished,
                        );
                    }
                })
            }))
            .await;

        let weak_ref = self.weak_ref.clone();
        self.peer
            .on_negotiation_needed(Box::new(move || {
                let weak_ref = weak_ref.clone();
                Box::pin(async move {
                    let rtp_client = match weak_ref.upgrade() {
                        Some(rtp_client) => rtp_client,
                        None => return,
                    };

                    let mut rtp_client = rtp_client.lock().await;
                    if let Err(error) = rtp_client.execute_renegotiation(false).await {
                        error!("Failed to execute negotiation: {}", error);
                        // TODO: Clone the connection?
                    }
                })
            }))
            .await;

        let weak_ref = self.weak_ref.clone();
        let rtcp_channel = self.rtcp_channel.clone();
        self.peer
            .on_track(Box::new(move |track, receiver| {
                let weak_ref = weak_ref.clone();
                let rtcp_channel = rtcp_channel.clone();
                Box::pin(async move {
                    let ref_self = match weak_ref.upgrade() {
                        Some(value) => value,
                        None => return,
                    };

                    let track = match track {
                        Some(track) => track,
                        None => {
                            error!("Having track event without a track.");
                            return;
                        }
                    };

                    let receiver = match receiver {
                        Some(receiver) => receiver,
                        None => {
                            error!("Having track event without a receiver.");
                            return;
                        }
                    };

                    let stream_id = track.stream_id().await;

                    info!(
                        "Received track ssrc = {}, id = {}, sid = {}, rid = {}",
                        track.ssrc(),
                        track.id().await,
                        track.stream_id().await,
                        track.rid()
                    );
                    let track = RegisteredReceivingTrack {
                        stream_id,
                        weak_client: weak_ref.clone(),
                        
                        receiver,
                        track,
                        rtcp_sender: rtcp_channel.clone(),
                        closed: Arc::new(AtomicBool::new(false)),

                        handler: Default::default(),
                    };
                    let track = Arc::new(track);
                    ref_self.lock().await.receiving_tracks.push_back(track.clone());
                    
                    let weak_self = weak_ref.clone();
                    tokio::spawn(async move {
                        registered_receive_track_io_loop(track.clone()).await;
                        if let Some(handler) = track.get_handler() {
                            handler.handle_closed().await;
                        }

                        if let Some(_self_ref) = weak_self.upgrade() {
                            /* TODO: Unregister that track since it has ended. */
                            trace!("Track {} ended.", track.stream_id);
                        }
                    });
                })
            }))
            .await;

        // TODO: Proper preallocate alg (the client needs to preallocate as well).
        // for _ in 0..1 {
        //     self.create_sending_track().await?;
        // }

        Ok(())
    }

    fn signaling_notify(
        signaling_client: &Weak<Mutex<SignalingClient>>,
        notify: notify::S2CNotify,
    ) {
        let signaling_client = match signaling_client.upgrade() {
            Some(signaling_client) => signaling_client,
            None => return,
        };

        // Make locking the client into a new task (no deadlock).
        task::spawn(async move {
            signaling_client
                .lock()
                .await
                .send_message(&S2CMessage::Notify(notify));
        });
    }

    pub async fn close(&mut self) {
        if let Err(error) = self.peer.close().await {
            warn!("Failed to close peer connection: {}", error);
        }
        trace!("Peer connection closed");
    }

    pub fn create_rtc_source(&mut self, stream_id: &str) -> Option<Pin<Box<dyn RtpSource>>> {
        let stream = self.receiving_tracks.iter_mut().find(|t| t.stream_id == stream_id);
        let stream = match stream {
            Some(stream) => stream,
            None => return None,
        };

        
        let (events_tx, events_rx) = mpsc::channel(16);
        let handler = Arc::new(RtpSourceHandler{
            events: events_tx
        });
        let source = Box::pin(RtpClientTrackSource{
            weak_track: Arc::downgrade(stream),
            events: events_rx,
            recv_handler: handler.clone(),
            rtcp_sender: self.rtcp_channel.clone(),
            ssrc: stream.track.ssrc()
        });
        stream.update_handler(Some(handler));
        Some(source)
    }

    async fn create_sending_track(&mut self) -> webrtc::error::Result<Arc<RegisteredSendingTrack>> {
        let id: String = thread_rng()
            .sample_iter(&Alphanumeric)
            .take(16)
            .map(char::from)
            .collect();

        let (rtp_tx, rtp_rx) = mpsc::channel(1024);
        let track = Arc::new(RegisteredSendingTrack {
            binding: Default::default(),
            id,

            shutdown_tx: Default::default(),
            event_handler: Default::default(),
            rtp_send_channel: rtp_tx
        });

        let sender = self.peer.add_track(track.clone()).await?;
        {
            let track = track.clone();
            let sender = sender.clone();

            let weak_ref = self.weak_ref.clone();
            tokio::spawn(async move {
                registered_send_track_io_loop(track.clone(), sender.clone(), rtp_rx).await;

                // Track finished, remove it from the local track list and from the peer.
                if let Some(ref_self) = weak_ref.upgrade() {
                    let ref_self = ref_self.lock().await;
                    let _ = ref_self.peer.remove_track(&sender).await;
                }

                trace!("Local track {} ended.", track.id());
            });
        }

        // TODO: Error after 10s if the track never binds.


        self.sending_tracks.push_back(track.clone());
        Ok(track)
    }

    pub async fn create_rtc_target(&mut self) -> Pin<Box<dyn RtpTarget>> {
        let track = self.sending_tracks
            .iter()
            .find(|track| track.event_handler.lock().is_none())
            .cloned();

        let track = match track {
            Some(track) => track,
            None => {
                let track = self.create_sending_track().await.unwrap();

                // Sadly currently manually needed...
                //let _ = self.execute_renegotiation(true).await;

                track
            }
        };

        let (events_tx, events_rx) = mpsc::channel(16);
        let target = RtpClientTarget {
            events: events_rx,
            track: track.clone(),
            weak_client: self.weak_ref.clone(),
        };

        {
            let mut handler = track.event_handler.lock();
            *handler = Some(events_tx);
        }

        Box::pin(target)
    }
}

impl Drop for RtpClient {
    fn drop(&mut self) {
        debug!("RtpClient {} dropped.", self.client_id);
    }
}