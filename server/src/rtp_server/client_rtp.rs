use log::{debug, error, info, trace, warn};
use rocket::{
    async_trait,
    tokio::{
        self, select,
        sync::{mpsc, oneshot, Mutex},
        task,
    },
};
use std::{
    collections::VecDeque,
    ops::DerefMut,
    pin::Pin,
    sync::{Arc, Weak},
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
        payload_feedbacks::picture_loss_indication::PictureLossIndication,
        receiver_report::ReceiverReport,
        transport_feedbacks::{
            transport_layer_cc::TransportLayerCc, transport_layer_nack::TransportLayerNack,
        },
    },
    rtp_transceiver::{
        rtp_codec::{
            RTCRtpCodecCapability, RTCRtpCodecParameters, RTCRtpHeaderExtensionCapability,
            RTPCodecType,
        },
        rtp_receiver::RTCRtpReceiver,
        PayloadType, SSRC,
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
    ssrc: SSRC,
    events: mpsc::Receiver<RtpSourceEvent>,
    rtcp_sender: RtcpSender,
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

struct RtpClientTrackHandler {
    events: mpsc::Sender<RtpSourceEvent>,
}

impl RtpClientTrackHandler {
    async fn handle_rtp_packet(&mut self, packet: webrtc::rtp::packet::Packet) {
        let _ = self.events.send(RtpSourceEvent::Media(packet)).await;
    }

    async fn handle_rtcp_packet(
        &mut self,
        packet: Box<dyn webrtc::rtcp::packet::Packet + Send + Sync>,
    ) {
        trace!("Having RTCP {}.", packet.header().packet_type.to_string());
    }
}

struct ReceivingTrack {
    stream_id: String,
    weak_client: Weak<Mutex<RtpClient>>,

    track: Arc<TrackRemote>,
    receiver: Arc<RTCRtpReceiver>,
    rtcp_sender: RtcpSender,

    unmount_sender: Option<oneshot::Sender<()>>,
}

impl ReceivingTrack {
    /// Accquire the track and hold it as loong the we don't receive an unmount
    /// notification or we drop the receiver.
    fn accquire(&mut self) -> oneshot::Receiver<()> {
        let (mut recv_tx, recv_rx) = oneshot::channel();
        let (unmount_tx, mut unmount_rx) = oneshot::channel();

        /* Stop the last source from reading of this source. */
        if let Some(sender) = self.unmount_sender.replace(unmount_tx) {
            let _ = sender.send(());
        }

        let track = self.track.clone();
        let weak_client = self.weak_client.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = recv_tx.closed() => {
                    /*
                     * The unmount handle has been dropped.
                     * Redirect this track to void.
                     */
                    if let Some(client) = weak_client.upgrade() {
                        let mut client = client.lock().await;
                        client.handle_track_recv_dropped(&track).await;
                    }
                },
                _ = &mut unmount_rx => {
                    /*
                     * We requested an unmount.
                     * Forward this to the receiver and abort this drop watcher.
                     */
                    let _ = recv_tx.send(());
                }
            }
        });

        recv_rx
    }

    fn create_rtp_source(&mut self) -> Pin<Box<dyn RtpSource>> {
        let mut unmount_rx = self.accquire();

        let track = self.track.clone();
        let receiver = self.receiver.clone();

        let (tx, rx) = mpsc::channel(8);
        let mut handler = RtpClientTrackHandler { events: tx };
        task::spawn(async move {
            loop {
                select! {
                    _ = handler.events.closed() => {
                        /* receiver disconnected */
                        break;
                    },
                    _ = &mut unmount_rx => {
                        /* stream ended. */
                        break;
                    },
                    rtp = track.read_rtp() => {
                        let (rtp, _attributes) = match rtp {
                            Err(_error) => {
                                /* FIXME: Error handling! */
                                continue;
                            },
                            Ok(payload) => payload
                        };

                        handler.handle_rtp_packet(rtp).await;
                    },
                    rtcp = receiver.read_rtcp() => {
                        let (rtcps, _attributes) = match rtcp {
                            Err(_error) => {
                                /* FIXME: Error handling! */
                                continue;
                            },
                            Ok(rtcp) => rtcp
                        };

                        for rtcp in rtcps {
                            handler.handle_rtcp_packet(rtcp).await;
                        }
                    }
                }
            }
        });

        Box::pin(RtpClientTrackSource {
            events: rx,
            ssrc: self.track.ssrc(),
            rtcp_sender: self.rtcp_sender.clone(),
        })
    }

    // Consume everything and do nothing about this track.
    fn accquire_void(&mut self) {
        let mut unmount_rx = self.accquire();

        let track = self.track.clone();
        let receiver = self.receiver.clone();
        task::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut unmount_rx => {
                        // No more receiving...
                        trace!("Voice receiver stop!");
                        return;
                    },
                    read = track.read_rtp() => {
                        if let Err(error) = read {
                            warn!("Void receiver encountered error: {:#}", error);
                            // TODO: Handle error and may invalidate track or whatever?
                        }
                    },
                    read = receiver.read_rtcp() => {
                        if let Err(error) = read {
                            warn!("Void receiver encountered rtcp error: {:#}", error);
                            // TODO: Handle error and may invalidate track or whatever?
                        }
                    }
                };
            }
        });
    }
}

struct RtpClientTarget {
    weak_client: Weak<Mutex<RtpClient>>,
    track: Arc<SendingTrack>,
    events: mpsc::Receiver<RtpTargetEvent>,
}

impl RtpTarget for RtpClientTarget {
    fn send_rtp(&self, packet: &mut webrtc::rtp::packet::Packet) {
        let track = self.track.clone();

        let mut packet = packet.clone();
        tokio::spawn(async move {
            let mut binding = track.binding.lock().await;

            let stream = match binding.deref_mut() {
                Some(stream) => stream,
                None => return,
            };

            packet.header.ssrc = stream.ssrc;
            packet.header.payload_type = stream.payload_type;
            let _ = stream.write_stream.write_rtp(&packet).await;
        });
    }
}

impl Stream for RtpClientTarget {
    type Item = RtpTargetEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.events.poll_recv(cx)
    }
}

struct SendingTrackBinding {
    id: String,
    ssrc: SSRC,
    payload_type: PayloadType,
    write_stream: Arc<dyn TrackLocalWriter + Send + Sync>,
}

struct SendingTrack {
    id: String,
    binding: Mutex<Option<SendingTrackBinding>>,
}

#[async_trait]
impl TrackLocal for SendingTrack {
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

type RtcpSender = mpsc::Sender<Vec<Box<dyn webrtc::rtcp::packet::Packet + Send + Sync>>>;
pub struct RtpClient {
    weak_ref: Weak<Mutex<Self>>,
    signaling_client_ref: Weak<Mutex<SignalingClient>>,

    client_id: ClientId,
    peer: webrtc::peer_connection::RTCPeerConnection,
    tracks: VecDeque<ReceivingTrack>,

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
            tracks: Default::default(),

            rtcp_channel: rtcp_tx,
        }));

        {
            let mut client = instance.lock().await;
            client.weak_ref = Arc::downgrade(&instance);
            client.bind_peer_listener().await;
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
            "[{}] Nego? {:#?}",
            self.client_id,
            self.peer.signaling_state()
        );
        if !force && self.peer.signaling_state() == RTCSignalingState::Stable {
            // No need to signal anything.
            return Ok(());
        }

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

    async fn bind_peer_listener(&mut self) {
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
                    let mut track = ReceivingTrack {
                        stream_id,
                        weak_client: weak_ref.clone(),
                        unmount_sender: None,
                        receiver,
                        track,
                        rtcp_sender: rtcp_channel.clone(),
                    };
                    track.accquire_void();
                    ref_self.lock().await.tracks.push_back(track);
                })
            }))
            .await;

        // let local_track = Arc::new(TrackLocalStaticRTP::new(
        //     RTCRtpCodecCapability{
        //         mime_type: MIME_TYPE_VP8.to_owned(),
        //         ..Default::default()
        //     },
        //     "video-01".to_owned(),
        //     "video-01".to_owned(),
        // ));

        // let rtp_sender = self.peer
        //     .add_track(Arc::clone(&local_track) as Arc<dyn TrackLocal + Send + Sync>)
        //     .await.unwrap();

        // tokio::spawn(async move {
        //     loop {
        //         match rtp_sender.read_rtcp().await {
        //             Ok((rtcp_packet, _attributes)) => {
        //                 debug!("RTCP packet {:?}", rtcp_packet);
        //             },
        //             Err(error) => {
        //                 warn!("failed to receive rtcp: {}", error);
        //                 break;
        //             },
        //         };
        //     }
        // });
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
    }

    async fn handle_track_recv_dropped(&mut self, track: &Arc<TrackRemote>) {
        let client_track = self
            .tracks
            .iter_mut()
            .find(|t| Arc::ptr_eq(&t.track, track));
        let client_track = match client_track {
            Some(client_track) => client_track,
            None => return,
        };

        trace!("Client track dropped. Voiding it!");
        client_track.accquire_void();
    }

    pub fn create_rtc_source(&mut self, stream_id: &str) -> Option<Pin<Box<dyn RtpSource>>> {
        let stream = self.tracks.iter_mut().find(|t| t.stream_id == stream_id);

        stream.map(|stream| stream.create_rtp_source())
    }

    pub async fn create_rtc_target(&mut self) -> Pin<Box<dyn RtpTarget>> {
        let track = Arc::new(SendingTrack {
            binding: Default::default(),
            id: "THIS_IS_VIDEO!".to_owned(),
        });

        // TODO: Some kind of drop mechanism!
        let sender = self.peer.add_track(track.clone()).await.unwrap();
        sender.send(&sender.get_parameters().await).await.unwrap();

        let (tx, rx) = mpsc::channel(8);

        tokio::spawn(async move {
            loop {
                // TODO: break when tx is closed!
                let event = sender.read_rtcp().await;
                let event = match event {
                    Err(error) => {
                        warn!("RTCP send read error: {:#}", error);
                        break;
                    }
                    Ok(event) => event,
                };

                for event in event.0 {
                    if let Some(_) = event.as_any().downcast_ref::<TransportLayerCc>() {
                        //trace!("Received TransportLayerCc");
                    } else if let Some(pli) = event.as_any().downcast_ref::<PictureLossIndication>()
                    {
                        trace!(
                            "Received PLI request from {} for {}.",
                            pli.sender_ssrc,
                            pli.media_ssrc
                        );
                        let _ = tx.send(RtpTargetEvent::RequestPli).await;
                    } else if let Some(_rr) = event.as_any().downcast_ref::<ReceiverReport>() {
                        // TODO: Get current track id and extract own report.
                        trace!("Received ReceiverReport.");
                    } else if let Some(nack) = event.as_any().downcast_ref::<TransportLayerNack>() {
                        /* Nack handling itself already done by an interceptor. */
                        let lost_packets: u64 =
                            nack.nacks.iter().map(|n| n.lost_packets as u64).sum();
                        trace!(
                            "Received TransportLayerNack. Packets lost: {}",
                            lost_packets
                        );
                    } else {
                        trace!("RTCP send read rtcp: {:#?}", event);
                    }
                }
            }

            drop(tx);
        });

        // TODO: Error after 10s if the track never binds.

        let target = RtpClientTarget {
            events: rx,
            track,
            weak_client: self.weak_ref.clone(),
        };

        // Sadly currently manually needed...
        let _ = self.execute_renegotiation(true).await;

        Box::pin(target)
    }
}

impl Drop for RtpClient {
    fn drop(&mut self) {
        debug!("RtpClient {} dropped.", self.client_id);
    }
}
