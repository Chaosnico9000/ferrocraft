//! Drives a real server on a random port the way a vanilla client would.

mod common;

use common::{Client, start, test_config};
use ferrocraft::Config;
use ferrocraft_protocol::packets::handshake::Intent;
use ferrocraft_protocol::packets::login::{Hello, LoginDisconnect};
use ferrocraft_protocol::packets::status::{
    PingRequest, PongResponse, StatusRequest, StatusResponse,
};
use ferrocraft_protocol::{GAME_VERSION, PROTOCOL_VERSION};
use uuid::Uuid;

#[tokio::test]
async fn server_list_ping() {
    let addr = start(test_config()).await;
    let mut client = Client::connect(addr, Intent::Status, PROTOCOL_VERSION).await;

    client.send(&StatusRequest).await;
    let status: StatusResponse = client.read().await;
    let json: serde_json::Value = serde_json::from_str(&status.json).unwrap();
    assert_eq!(json["version"]["protocol"], PROTOCOL_VERSION);
    assert_eq!(json["version"]["name"], GAME_VERSION);
    assert_eq!(json["description"]["text"], "Testwelt");
    assert_eq!(json["players"]["max"], 20);

    client
        .send(&PingRequest {
            payload: 0x1234_5678_9abc,
        })
        .await;
    let pong: PongResponse = client.read().await;
    assert_eq!(pong.payload, 0x1234_5678_9abc);
}

#[tokio::test]
async fn full_server_turns_players_away() {
    let addr = start(Config {
        max_players: 1,
        ..test_config()
    })
    .await;
    let (mut first, _) = Client::login(addr, "Alex").await;
    first.configure(2).await;
    // The first player counts once they are in play.
    first
        .read_until::<ferrocraft_protocol::packets::play::Login>()
        .await;

    let mut second = Client::connect(addr, Intent::Login, PROTOCOL_VERSION).await;
    second
        .send(&Hello {
            name: "Steve".into(),
            uuid: Uuid::nil(),
        })
        .await;
    let bye: LoginDisconnect = second.read().await;
    assert!(bye.reason_json.contains("voll"), "{}", bye.reason_json);
}

#[tokio::test]
async fn wrong_protocol_version_is_turned_away() {
    let addr = start(test_config()).await;
    let mut client = Client::connect(addr, Intent::Login, PROTOCOL_VERSION - 1).await;

    let bye: LoginDisconnect = client.read().await;
    assert!(bye.reason_json.contains("zu alt"));
}

#[tokio::test]
async fn ping_works_without_status_request() {
    let addr = start(test_config()).await;
    let mut client = Client::connect(addr, Intent::Status, PROTOCOL_VERSION).await;

    client.send(&PingRequest { payload: -7 }).await;
    let pong: PongResponse = client.read().await;
    assert_eq!(pong.payload, -7);
}
