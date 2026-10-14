//! Packet compression, negotiated at the end of login.

mod common;

use common::{Client, accept_spawn, enter_world, start, test_config};
use ferrocraft::Config;
use ferrocraft_protocol::Encode;
use ferrocraft_protocol::VarInt;
use ferrocraft_protocol::packets::play::MovePlayer;

#[tokio::test]
async fn login_turns_on_compression() {
    let addr = start(test_config()).await;
    let (mut client, _) = Client::login(addr, "Alex").await;
    assert_eq!(client.compression_threshold(), Some(256));
    client.configure(2).await;
    let (_, position, chunks) = enter_world(&mut client).await;
    assert_eq!(chunks[0].sections.len(), 2244, "chunks unpack intact");
    accept_spawn(&mut client, &position).await;
}

/// Bytes on the wire for a whole join at view distance 10.
async fn join_bytes(config: Config) -> usize {
    let addr = start(config).await;
    let (mut client, _) = Client::login(addr, "Alex").await;
    client.configure(10).await;
    let (_, _, chunks) = enter_world(&mut client).await;
    assert_eq!(chunks.len(), 473);
    client.received
}

#[tokio::test]
async fn compression_shrinks_a_join() {
    let plain = join_bytes(Config {
        compression_threshold: -1,
        ..test_config()
    })
    .await;
    let compressed = join_bytes(test_config()).await;
    println!("join at view distance 10: {plain} bytes plain, {compressed} compressed");
    assert!(plain > 3_000_000, "{plain}");
    assert!(compressed * 10 < plain, "{compressed} of {plain}");
}

#[tokio::test]
async fn compression_can_be_turned_off() {
    let addr = start(Config {
        compression_threshold: -1,
        ..test_config()
    })
    .await;
    let (mut client, _) = Client::login(addr, "Alex").await;
    assert_eq!(client.compression_threshold(), None);
    client.configure(2).await;
    enter_world(&mut client).await;
}

#[tokio::test]
async fn badly_compressed_packet_closes_the_connection() {
    let addr = start(test_config()).await;
    let (mut client, _) = Client::login(addr, "Alex").await;
    client.configure(2).await;
    let (_, position, _) = enter_world(&mut client).await;
    accept_spawn(&mut client, &position).await;

    // A movement packet that claims to be compressed although it is far below
    // the threshold, as vanilla's decoder would refuse it.
    let mut packet = Vec::new();
    let id = MovePlayer {
        pos: None,
        rot: None,
        flags: 1,
    }
    .encode_variant(&mut Vec::new());
    VarInt(id).encode(&mut packet);
    packet.push(1);
    let mut frame = Vec::new();
    VarInt(packet.len() as i32 + 1).encode(&mut frame);
    VarInt(packet.len() as i32).encode(&mut frame);
    frame.extend_from_slice(&packet);
    client.send_bytes(&frame).await;

    while client.read_raw().await.is_some() {}
}
