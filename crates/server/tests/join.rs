//! A client goes through login, configuration and play like the vanilla client.

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use common::{
    Client, accept_spawn, chunks_in_view, enter_world, in_view, start, start_with_timing,
    test_config,
};
use ferrocraft::{Server, Timing};
use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::packets::Packet;
use ferrocraft_protocol::packets::configuration::{
    ClientboundSelectKnownPacks, Disconnect as ConfigDisconnect, KnownPack,
    ServerboundSelectKnownPacks,
};
use ferrocraft_protocol::packets::play::{
    ChunkBatchFinished, ChunkBatchStart, ClientboundKeepAlive, Disconnect, ForgetLevelChunk,
    GameMode, LevelChunkWithLight, MovePlayer, ServerboundKeepAlive, SetChunkCacheCenter,
};
use tokio::net::TcpListener;

#[tokio::test]
async fn player_joins_a_superflat_world() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = Server::new(test_config()).unwrap();
    tokio::spawn(std::sync::Arc::clone(&server).serve(listener));

    let (mut client, finished) = Client::login(addr, "Notch").await;
    assert_eq!(finished.profile.name, "Notch");
    assert_eq!(
        finished.profile.uuid.to_string(),
        "b50ad385-829d-3141-a216-7e7d7539ba7f",
        "vanilla's offline UUID"
    );

    let configured = client.configure(4).await;
    assert_eq!(configured.registries.len(), 32);
    assert!(
        configured
            .registries
            .iter()
            .all(|r| r.entries.iter().all(|e| e.data.is_none()))
    );
    assert_eq!(configured.tags.registries.len(), 15);

    let (login, position, chunks) = enter_world(&mut client).await;
    assert_eq!(login.spawn.dimension, "minecraft:overworld");
    assert_eq!(login.spawn.game_mode, GameMode::Creative);
    assert!(login.spawn.flat);
    assert_eq!(position.pos, [0.5, -60.0, 0.5]);
    assert_eq!(chunks.len(), chunks_in_view(4));
    assert_eq!(
        chunks_in_view(10),
        473,
        "what vanilla sends at view distance 10"
    );
    assert_eq!(
        (chunks[0].x, chunks[0].z),
        (0, 0),
        "the player's chunk first"
    );

    accept_spawn(&mut client, &position).await;
    assert_eq!(server.online_players.load(Ordering::Relaxed), 1);

    drop(client);
    tokio::time::timeout(Duration::from_secs(5), async {
        while server.online_players.load(Ordering::Relaxed) != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("player still counted after leaving");
}

#[tokio::test]
async fn view_distance_is_clamped_between_two_and_the_server() {
    let addr = start(test_config()).await;
    for (requested, expected) in [(0, 2), (3, 3), (32, 10)] {
        let (mut client, _) = Client::login(addr, "Alex").await;
        client.configure(requested).await;
        let (_, _, chunks) = enter_world(&mut client).await;
        assert_eq!(
            chunks.len(),
            chunks_in_view(expected),
            "requested {requested}"
        );
    }
}

#[tokio::test]
async fn walking_loads_and_forgets_chunks() {
    let addr = start(test_config()).await;
    let (mut client, _) = Client::login(addr, "Alex").await;
    client.configure(2).await;
    let (_, position, chunks) = enter_world(&mut client).await;
    accept_spawn(&mut client, &position).await;

    let mut body = Vec::new();
    let id = MovePlayer {
        pos: Some([16.0 * 5.0 + 0.5, -60.0, 0.5]),
        rot: None,
        flags: 1,
    }
    .encode_variant(&mut body);
    client.send_raw(id, &body).await;

    // Chunks that left the circle around (5, 0) are forgotten, those that
    // entered it are sent.
    let mut forgotten = 0;
    let center = loop {
        let raw = client.read_raw().await.unwrap();
        match raw.id {
            ForgetLevelChunk::ID => {
                let chunk = ForgetLevelChunk::from_raw(&raw).unwrap();
                assert!(!in_view((5, 0), 2, (chunk.x, chunk.z)), "forgot {chunk:?}");
                forgotten += 1;
            }
            SetChunkCacheCenter::ID => break SetChunkCacheCenter::from_raw(&raw).unwrap(),
            // The join message and the like.
            _ => {}
        }
    };
    assert_eq!(center, SetChunkCacheCenter { x: 5, z: 0 });
    let left = chunks.iter().filter(|c| !in_view((5, 0), 2, (c.x, c.z)));
    assert_eq!(forgotten, left.count());
    client.read::<ChunkBatchStart>().await;
    let mut loaded = 0;
    loop {
        let raw = client.read_raw().await.unwrap();
        match raw.id {
            LevelChunkWithLight::ID => {
                let chunk = LevelChunkWithLight::from_raw(&raw).unwrap();
                let pos = (chunk.x, chunk.z);
                assert!(
                    in_view((5, 0), 2, pos) && !in_view((0, 0), 2, pos),
                    "sent {pos:?}"
                );
                loaded += 1;
            }
            ChunkBatchFinished::ID => break,
            // The join message and the like.
            _ => {}
        }
    }
    assert_eq!(loaded, forgotten, "the view keeps its size");
}

#[tokio::test]
async fn keep_alive_keeps_the_player_until_it_goes_unanswered() {
    let addr = start_with_timing(
        test_config(),
        Timing {
            keep_alive: Duration::from_millis(200),
        },
    )
    .await;
    let (mut client, _) = Client::login(addr, "Alex").await;
    client.configure(2).await;
    let (_, position, _) = enter_world(&mut client).await;
    accept_spawn(&mut client, &position).await;

    for _ in 0..3 {
        let ping: ClientboundKeepAlive = client.read_until().await;
        client.send(&ServerboundKeepAlive { id: ping.id }).await;
    }
    client.read_until::<ClientboundKeepAlive>().await;
    let bye: Disconnect = client.read_until().await;
    assert_eq!(bye.reason, Nbt::text("Zeitüberschreitung"));
    assert!(client.read_raw().await.is_none(), "connection closed");
}

#[tokio::test]
async fn client_without_the_vanilla_pack_is_turned_away() {
    let addr = start(test_config()).await;
    let (mut client, _) = Client::login(addr, "Alex").await;
    let offer: ClientboundSelectKnownPacks = client.read_until().await;
    assert_eq!(offer.packs, [KnownPack::vanilla_core()]);
    client
        .send(&ServerboundSelectKnownPacks { packs: Vec::new() })
        .await;
    let bye: ConfigDisconnect = client.read().await;
    let Some(Nbt::String(text)) = bye.reason.get("text") else {
        panic!("not a text component: {:?}", bye.reason);
    };
    assert!(text.contains("Vanilla-Datenpaket"), "{text}");
}
