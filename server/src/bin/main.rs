#![feature(proc_macro_hygiene, decl_macro)]
#![feature(core_intrinsics)]
#![allow(dead_code)]

use jsonwebtoken::{DecodingKey, EncodingKey};
use log::{info, LevelFilter};
use mongodb::{options::ClientOptions, Client};
use rocket::{
    config::Shutdown,
    http::Method,
    tokio::{signal, task},
    Config,
};
use rocket_cors::{AllowedOrigins, CorsOptions};
use simple_screen_share::{
    rest_controller::{routes, JwtDecodingKey, JwtEncodingKey, MongoDatabase, RocketRoomManager},
    rtp_server::server::RtpServer,
};

#[rocket::main]
async fn main() {
    env_logger::Builder::new()
        //.filter(None, LevelFilter::Trace)
        .filter(Some("tungstenite"), LevelFilter::Info)
        .filter(Some("simple_screen_share"), LevelFilter::Trace)
        .init();

    let config = Config {
        port: 3030,
        shutdown: Shutdown {
            ctrlc: false,
            ..Shutdown::default()
        },
        ..Config::debug_default()
    };

    let rtp_server = RtpServer::new("127.0.0.1:8088".parse().unwrap()).await;
    {
        let mut server = rtp_server.lock().await;
        server.spawn_tick().expect("failed to start ticking");
        server
            .start_server()
            .await
            .expect("failed to start rtp server");
    }

    let client_options = ClientOptions::parse("mongodb://root:9Dr6e3SeWTwHrT@localhost:27017")
        .await
        .unwrap();
    let client = Client::with_options(client_options).unwrap();
    let database = client.database("simple-screen-share");

    let cors = CorsOptions::default()
        .allowed_origins(AllowedOrigins::all())
        .allowed_methods(
            vec![Method::Get, Method::Post, Method::Patch]
                .into_iter()
                .map(From::from)
                .collect(),
        )
        .allow_credentials(true)
        .to_cors()
        .unwrap();

    let rocket = rocket::custom(&config)
        .mount(
            "/",
            rocket::routes![
                routes::post_room_create,
                routes::post_room_join,
                routes::post_authenticate
            ],
        )
        .manage(RocketRoomManager(rtp_server))
        .manage(MongoDatabase(database))
        .manage(JwtDecodingKey(DecodingKey::from_secret(
            "WolverinDEVIsTheBest!".as_bytes(),
        ))) // FIXME: Make this configurable!
        .manage(JwtEncodingKey(EncodingKey::from_secret(
            "WolverinDEVIsTheBest!".as_bytes(),
        ))) // FIXME: Make this configurable!
        .attach(cors)
        .ignite()
        .await
        .expect("failed to create REST server");

    console_subscriber::init();

    let shutdown_handle = rocket.shutdown();
    task::spawn(async {
        info!("Press ctrl-c to terminate server");
        signal::ctrl_c().await.unwrap();
        info!("Shutting down server.");
        shutdown_handle.notify();
    });

    let _ = rocket.launch().await.expect("failed to launch REST server");
}
