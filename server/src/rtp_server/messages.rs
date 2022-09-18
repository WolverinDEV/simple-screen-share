use serde::{Deserialize, Serialize};
use typescript_type_def::TypeDef;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;

#[derive(Debug, Deserialize, TypeDef)]
#[serde(tag = "type", content = "payload")]
pub enum C2SMessage {
    Request {
        request_id: u16,
        request: request::C2SRequest,
    },
}

#[derive(Debug, Serialize, Deserialize, TypeDef, Clone)]
pub struct IceCandidate {
    /// Candidate string without "candidate:" prefix.
    candidate: String,

    /// Username fragment.
    ufrag: String,
}

impl Into<RTCIceCandidateInit> for IceCandidate {
    fn into(self) -> RTCIceCandidateInit {
        let mut init: RTCIceCandidateInit = Default::default();
        init.candidate = self.candidate;
        init.username_fragment = Some(self.ufrag);
        init
    }
}

impl From<RTCIceCandidateInit> for IceCandidate {
    fn from(init: RTCIceCandidateInit) -> Self {
        let candidate = if init.candidate.starts_with("candidate:") {
            init.candidate[10..].to_string()
        } else {
            init.candidate
        };

        Self {
            candidate,
            ufrag: init.username_fragment.unwrap_or_default(),
        }
    }
}

pub mod request {
    use serde::Deserialize;
    use typescript_type_def::TypeDef;

    use crate::rtp_server::room::{BroadcastId, BroadcastKind};

    use super::IceCandidate;

    #[derive(Debug, Deserialize, TypeDef)]
    #[serde(tag = "type")]
    pub enum C2SRequest {
        InitializeSesstion(RequestInitializeSession),
        IceCandidates(RequestIceCandidates),
        NegotiationOffer(RequestNegotiationOffer),
        NegotiationAnswer(RequestNegotiationAnswer),

        BroadcastStart(RequestBroadcastStart),
        BroadcastStop { broadcast_id: BroadcastId },

        BroadcastSubscribe { broadcast_id: BroadcastId },
        BroadcastUnsubscribe { broadcast_id: BroadcastId },
    }

    #[derive(Debug, Deserialize, TypeDef)]
    pub struct RequestInitializeSession {
        pub version: u64,
        pub token: String,
        pub offer: String,
    }

    #[derive(Debug, Deserialize, TypeDef)]
    pub struct RequestNegotiationOffer {
        pub offer: String,
    }

    #[derive(Debug, Deserialize, TypeDef)]
    pub struct RequestNegotiationAnswer {
        pub answer: String,
    }

    #[derive(Debug, Deserialize, TypeDef)]
    pub struct RequestIceCandidates {
        pub candidates: Vec<IceCandidate>,
        pub finished: bool,
    }

    #[derive(Debug, Deserialize, TypeDef)]
    pub struct RequestBroadcastStart {
        pub name: String,
        pub source: String,
        pub kind: BroadcastKind,
    }
}

pub mod response {
    use serde::Serialize;
    use typescript_type_def::TypeDef;

    use crate::{rtp_server::{room::BroadcastId, client::ClientId}, rest_controller::auth::UserId};

    #[derive(Debug, Serialize, TypeDef)]
    #[serde(tag = "type", content = "payload")]
    pub enum S2CResponse {
        /// Request has been succesfully executed.
        /// No response data.
        Success,

        /* General purpose response types */
        InvalidRequest,
        UnknownRequest,
        NotImplemented,
        /// The target client hasn't join the room
        RoomNotJoined,
        /// The connection has been closed, before the request
        /// could have been handled.
        /// Note: This will never be send by the server!
        ConnectionClosed,
        /// The request timed out.
        /// Note; This will never be send by the server!
        RequestTimeout,
        InternalError {
            message: String,
        },
        RtpNotInitialized,
        SessionNotInitialized,

        /* Response for the InitializeSession request. */
        SessionInitializeSuccess {
            answer: String,
            own_client_id: ClientId,
            own_user_id: UserId,
        },
        SessionAlreadyInitialized,
        SessionInvalidToken,
        SessionRoomClosed,
        SessionUnsupportedVersion {
            server_version: u64,
        },

        /* Responses for the NegotiationOffer request. */
        NegotiationOfferSuccess {
            answer: String,
        },
        /// The server already created a new offer which first needs to be accepted.
        NegotiationServerOfferPending,

        BroadcastSourceUnknownId,
        BroadcastAlreadySubscribed,
        BroadcastAlreadyRunning {
            broadcast_id: BroadcastId,
        },
        BroadcastStarted {
            broadcast_id: BroadcastId,
        },
        BroadcastUnknownId,
        BroadcastSubscribed {
            stream_id: String,
        }
    }
}

pub mod notify {
    use serde::Serialize;
    use typescript_type_def::TypeDef;

    use crate::{rest_controller::auth::UserId, rtp_server::{client::ClientId, room::{BroadcastId, BroadcastKind}}};

    use super::IceCandidate;

    #[derive(Debug, Serialize, TypeDef)]
    #[serde(tag = "type", content = "payload")]
    pub enum S2CNotify {
        NotifyNegotiationOffer(String),
        NotifyIceCandidate(IceCandidate),
        NotifyIceCandidateFinished,

        /// Will be send when joining a room.
        /// Contains all clients.
        NotifyUsers(Vec<UserEntry>),
        NotifyBroadcasts(Vec<BroadcastEntry>),

        /// Will be send when a new user joins the room.
        NotifyUserJoined(UserEntry),

        /// Will be send when a user leaves the room.
        NotifyUserLeft(ClientId),

        NotifyBroadcastStarted(BroadcastEntry),

        NotifyBroadcastEnded(BroadcastId),
    }

    #[derive(Debug, Serialize, TypeDef)]
    pub struct UserEntry {
        pub client_id: ClientId,
        pub user_id: UserId,
    }

    #[derive(Debug, Serialize, TypeDef)]
    pub struct BroadcastEntry {
        pub client_id: ClientId,
        pub broadcast_id: BroadcastId,
        pub name: String,
        pub kind: BroadcastKind,
    }
}
#[derive(Debug, Serialize, TypeDef)]
#[serde(tag = "type", content = "payload")]
pub enum S2CMessage {
    Response(u16, response::S2CResponse),
    Notify(notify::S2CNotify),
}

impl From<notify::S2CNotify> for S2CMessage {
    fn from(notify: notify::S2CNotify) -> Self {
        S2CMessage::Notify(notify)
    }
}
