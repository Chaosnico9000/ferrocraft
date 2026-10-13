//! Drives a real server on a random port the way a vanilla client would.

use ferrocraft::{Config, Server};
use ferrocraft_protocol::frame::decode_frame;
use ferrocraft_protocol::packets::Packet;
use ferrocraft_protocol::packets::handshake::{Intent, Intention};
use ferrocraft_protocol::packets::login::{Hello, LoginDisconnect};
use ferrocraft_protocol::packets::status::{
    PingRequest, PongResponse, StatusRequest, StatusResponse,
};
use ferrocraft_protocol::{GAME_VERSION, PROTOCOL_VERSION};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

async fn start() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let config = Config {
        motd: "Testwelt".into(),
        ..Config::default()
    };
    tokio::spawn(Server::new(config).serve(listener));
    addr
}

struct Client {
    stream: TcpStream,
    buf: Vec<u8>,
}

impl Client {
    async fn connect(addr: std::net::SocketAddr, intent: Intent, protocol_version: i32) -> Self {
        let mut client = Self {
            stream: TcpStream::connect(addr).await.unwrap(),
            buf: Vec::new(),
        };
        client
            .send(&Intention {
                protocol_version,
                host: "localhost".into(),
                port: addr.port(),
                intent,
            })
            .await;
        client
    }

    async fn send<P: Packet>(&mut self, packet: &P) {
        let mut out = Vec::new();
        packet.to_frame(&mut out);
        self.stream.write_all(&out).await.unwrap();
    }

    async fn read<P: Packet>(&mut self) -> P {
        loop {
            if let Some((raw, used)) = decode_frame(&self.buf).unwrap() {
                self.buf.drain(..used);
                return P::from_raw(&raw).unwrap();
            }
            let n = self.stream.read_buf(&mut self.buf).await.unwrap();
            assert_ne!(n, 0, "server closed the connection");
        }
    }
}

#[tokio::test]
async fn server_list_ping() {
    let addr = start().await;
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
async fn login_ends_with_a_readable_disconnect() {
    let addr = start().await;
    let mut client = Client::connect(addr, Intent::Login, PROTOCOL_VERSION).await;

    client
        .send(&Hello {
            name: "Steve".into(),
            uuid: Uuid::nil(),
        })
        .await;
    let bye: LoginDisconnect = client.read().await;
    let json: serde_json::Value = serde_json::from_str(&bye.reason_json).unwrap();
    assert!(json["text"].as_str().unwrap().contains("Steve"));
}

#[tokio::test]
async fn wrong_protocol_version_is_turned_away() {
    let addr = start().await;
    let mut client = Client::connect(addr, Intent::Login, PROTOCOL_VERSION - 1).await;

    let bye: LoginDisconnect = client.read().await;
    assert!(bye.reason_json.contains("zu alt"));
}

#[tokio::test]
async fn ping_works_without_status_request() {
    let addr = start().await;
    let mut client = Client::connect(addr, Intent::Status, PROTOCOL_VERSION).await;

    client.send(&PingRequest { payload: -7 }).await;
    let pong: PongResponse = client.read().await;
    assert_eq!(pong.payload, -7);
}
