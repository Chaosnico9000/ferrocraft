//! Online mode, against a stand-in for Mojang's session server.

mod common;

use std::sync::{Arc, Mutex};

use common::{Client, enter_world, start, test_config};
use ferrocraft::Config;
use ferrocraft_protocol::packets::Packet;
use ferrocraft_protocol::packets::login::LoginDisconnect;
use ferrocraft_protocol::packets::play::PlayerInfoUpdate;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// What the stand-in answers to `hasJoined`.
#[derive(Clone, Copy)]
enum Answer {
    /// A profile with a skin.
    Joined,
    /// 204: no such join.
    NotJoined,
    /// 500: the service is down.
    Broken,
}

const UUID: &str = "069a79f444e94726a5befca90e38aaf5";

/// Serves one answer for every request and records the query strings.
async fn session_server(answer: Answer) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/session/minecraft/hasJoined",
        listener.local_addr().unwrap()
    );
    let queries = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&queries);
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                if stream.read(&mut byte).await.unwrap() == 0 {
                    break;
                }
                request.push(byte[0]);
            }
            let line = String::from_utf8_lossy(&request)
                .lines()
                .next()
                .unwrap_or("")
                .to_owned();
            let query = line.split(['?', ' ']).nth(2).unwrap_or("").to_owned();
            let name = query
                .split('&')
                .find_map(|p| p.strip_prefix("username="))
                .unwrap_or("")
                .to_owned();
            seen.lock().unwrap().push(query);
            // Notch keeps his real id; everyone else gets one made of their
            // name.
            let id = if name == "Notch" {
                UUID.to_owned()
            } else {
                format!(
                    "{:0>32}",
                    name.bytes().map(|b| format!("{b:02x}")).collect::<String>()
                )
            };
            let (status, body) = match answer {
                Answer::Joined => (
                    "200 OK",
                    format!(
                        r#"{{"id":"{id}","name":"{name}","properties":[{{"name":"textures","value":"e30=","signature":"c2ln"}}]}}"#
                    ),
                ),
                Answer::NotJoined => ("204 No Content", String::new()),
                Answer::Broken => ("500 Internal Server Error", String::new()),
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });
    (url, queries)
}

fn online(session_server: String) -> Config {
    Config {
        online_mode: true,
        session_server,
        ..test_config()
    }
}

#[tokio::test]
async fn online_login_takes_the_profile_from_mojang() {
    let (url, queries) = session_server(Answer::Joined).await;
    let addr = start(online(url)).await;

    // The client claims a name; the session server decides who it is.
    let (mut client, finished) = Client::login(addr, "Notch").await;
    assert_eq!(finished.profile.uuid.simple().to_string(), UUID);
    assert_eq!(finished.profile.properties[0].name, "textures");
    assert_eq!(
        finished.profile.properties[0].signature.as_deref(),
        Some("c2ln")
    );
    let hash = client.server_hash.clone().unwrap();
    assert_eq!(
        queries.lock().unwrap().as_slice(),
        [format!("username=Notch&serverId={hash}")]
    );

    // Everything from here on is encrypted, and the skin reaches the tab list.
    client.configure(2).await;
    let (login, _, chunks) = enter_world(&mut client).await;
    assert!(login.online_mode);
    assert!(!chunks.is_empty());
}

#[tokio::test]
async fn others_see_the_skin() {
    let (url, _) = session_server(Answer::Joined).await;
    let addr = start(online(url)).await;
    let (mut alex, _) = Client::login(addr, "Alex").await;
    alex.configure(2).await;
    enter_world(&mut alex).await;
    let (mut steve, _) = Client::login(addr, "Steve").await;
    steve.configure(2).await;
    enter_world(&mut steve).await;

    // The tab list entry carries the signed textures, so the client can
    // show the skin.
    let info = common::read_matching(&mut alex, |i: &PlayerInfoUpdate| {
        i.entries[0].profile.name == "Steve"
    })
    .await;
    let textures = &info.entries[0].profile.properties[0];
    assert_eq!(textures.name, "textures");
    assert_eq!(textures.signature.as_deref(), Some("c2ln"));
}

#[tokio::test]
async fn unverified_players_are_turned_away() {
    for (answer, key) in [
        (
            Answer::NotJoined,
            "multiplayer.disconnect.unverified_username",
        ),
        (Answer::Broken, "multiplayer.disconnect.authservers_down"),
    ] {
        let (url, _) = session_server(answer).await;
        let addr = start(online(url)).await;
        let (_, result) = Client::try_login(addr, "Notch").await;
        let raw = result.expect_err("logged in without Mojang's blessing");
        let bye = LoginDisconnect::from_raw(&raw).unwrap();
        let reason: serde_json::Value = serde_json::from_str(&bye.reason_json).unwrap();
        assert_eq!(reason["translate"], key);
    }
}
