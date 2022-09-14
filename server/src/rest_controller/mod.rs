use std::sync::Arc;

use jsonwebtoken::{DecodingKey, EncodingKey};
use mongodb::Database;
use rocket::tokio::sync::Mutex;

use crate::rtp_server::RtpRoomManager;

pub mod auth;
pub mod routes;

type UserId = u64;

pub struct RocketRoomManager(pub Arc<Mutex<dyn RtpRoomManager>>);
pub struct MongoDatabase(pub Database);
pub struct JwtDecodingKey(pub DecodingKey);
pub struct JwtEncodingKey(pub EncodingKey);
