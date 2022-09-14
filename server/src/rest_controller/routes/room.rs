use std::time::Duration;

use log::info;
use rocket::{serde::json::Json, State};
use serde::Serialize;
use typescript_type_def::TypeDef;

use crate::{
    rest_controller::{auth::JwtClaims, RocketRoomManager},
    rtp_server::{room::RoomId, JoinTokenError},
};

#[rocket::post("/room/create")]
pub async fn post_room_create(
    room_manager: &State<RocketRoomManager>,
    jwt_token: JwtClaims,
) -> Json<ResponseRoomCreate> {
    let mut manager = room_manager.0.lock().await;
    Json(
        match manager
            .create_room(jwt_token.user_id().expect("valid user id"))
            .await
        {
            Ok(room_id) => ResponseRoomCreate::Success { room_id },
            Err(error) => ResponseRoomCreate::InternalError {
                message: format!("{}", error),
            },
        },
    )
}

#[rocket::post("/room/<room_id>/join")]
pub async fn post_room_join(
    room_manager: &State<RocketRoomManager>,
    jwt_token: JwtClaims,
    room_id: u64,
) -> Json<ResponseRoomJoin> {
    info!("Joining room {}", room_id);

    let expires_after = Duration::from_secs(120);
    let mut manager = room_manager.0.lock().await;
    Json(
        match manager
            .create_join_token(
                jwt_token.user_id().expect("valid user id"),
                room_id,
                expires_after.clone(),
            )
            .await
        {
            Ok(token) => ResponseRoomJoin::Success {
                server_url: token.server_url,
                client_token: token.token,
                client_token_valid_ms: expires_after.as_millis() as u64,
            },
            Err(JoinTokenError::RoomDoesNotExist) => ResponseRoomJoin::RoomNotFound {},
        },
    )
}

#[derive(Debug, Serialize, TypeDef)]
#[serde(tag = "status")]
pub enum ResponseRoomJoin {
    Success {
        server_url: String,
        client_token: String,
        client_token_valid_ms: u64,
    },
    RoomNotFound {},
    InternalError {
        message: String,
    },
}

#[derive(Debug, Serialize, TypeDef)]
#[serde(tag = "status")]
pub enum ResponseRoomCreate {
    Success { room_id: RoomId },
    InternalError { message: String },
}
