use std::time::Duration;

use rocket::async_trait;

use self::room::RoomId;

pub mod client;
pub mod client_rtp;
pub mod messages;
pub mod room;
pub mod server;

#[async_trait]
pub trait RtpRoomManager: Send + Sync {
    async fn create_room(&mut self, owner_id: u64) -> anyhow::Result<RoomId>;
    async fn delete_room(&mut self, room_id: RoomId) -> anyhow::Result<bool>;

    async fn create_join_token(
        &mut self,
        user_id: u64,
        room_id: RoomId,
        expires_after: Duration,
    ) -> Result<RoomJoinToken, JoinTokenError>;
}

pub struct RoomJoinToken {
    pub server_url: String,
    pub token: String,
}

pub enum JoinTokenError {
    RoomDoesNotExist,
}
