//! The server refuses moves no client can make: too far in one go, into
//! blocks, floating without the right to fly; ordinary steps onto a slab
//! pass.

mod common;

use std::time::Duration;

use common::{Client, join, read_matching, start, test_config, walk};
use ferrocraft::Config;
use ferrocraft_protocol::packets::play::{ChatCommand, Disconnect, PlayerPosition, SystemChat};

fn operators() -> Config {
    Config {
        operators: vec!["Alex".into()],
        ..test_config()
    }
}

async fn run(client: &mut Client, command: &str) {
    client
        .send(&ChatCommand {
            command: command.into(),
        })
        .await;
    // The answer says it is done.
    client.read_until::<SystemChat>().await;
}

async fn sent_back(client: &mut Client, millis: u64) -> Option<PlayerPosition> {
    tokio::time::timeout(Duration::from_millis(millis), client.read_until())
        .await
        .ok()
}

#[tokio::test]
async fn a_jump_of_twenty_blocks_is_sent_back() {
    let addr = start(test_config()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    walk(&mut alex, [20.5, -60.0, 0.5], None).await;
    let back = sent_back(&mut alex, 1000).await.expect("sent back");
    assert_eq!(back.pos, [0.5, -60.0, 0.5]);
}

#[tokio::test]
async fn walls_stop_even_creative_players() {
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    run(&mut alex, "fill 2 -60 -1 2 -59 1 stone").await;
    walk(&mut alex, [2.5, -60.0, 0.5], None).await;
    let back = sent_back(&mut alex, 1000).await.expect("sent back");
    assert_eq!(back.pos, [0.5, -60.0, 0.5]);
}

#[tokio::test]
async fn survival_players_step_onto_slabs() {
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    run(&mut alex, "setblock 2 -60 0 stone_slab").await;
    run(&mut alex, "gamemode survival").await;
    for (x, y) in [(1.1, -60.0), (1.5, -60.0), (1.9, -59.5), (2.3, -59.5)] {
        walk(&mut alex, [x, y, 0.5], None).await;
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    assert!(sent_back(&mut alex, 500).await.is_none(), "the steps stand");
}

#[tokio::test]
async fn floating_in_survival_ends_the_connection() {
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    run(&mut alex, "gamemode survival").await;
    for y in [-57.0, -54.0, -51.0] {
        walk(&mut alex, [0.5, y, 0.5], None).await;
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    let bye = tokio::time::timeout(
        Duration::from_secs(8),
        read_matching(&mut alex, |_: &Disconnect| true),
    )
    .await
    .expect("disconnected for floating");
    assert!(format!("{:?}", bye.reason).contains("Fliegen"));
}
