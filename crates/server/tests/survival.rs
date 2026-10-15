//! Survival: digging takes time and drops the block, falling hurts, dying,
//! respawning, eating.

mod common;

use std::time::Duration;

use common::{join, read_matching, start, test_config};
use ferrocraft::Config;
use ferrocraft::data::GameData;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    AddEntity, BlockChangedAck, BlockUpdate, ChatCommand, ClientCommand, ContainerSetSlot,
    EntityData, EntityEvent, GameEvent, MovePlayer, PlayerAction, PlayerCombatKill, PlayerPosition,
    Respawn, SetEntityData, SetHealth, UseItem, player_action,
};

fn operators() -> Config {
    Config {
        operators: vec!["Alex".into()],
        ..test_config()
    }
}

async fn run(client: &mut common::Client, command: &str) {
    client
        .send(&ChatCommand {
            command: command.into(),
        })
        .await;
}

/// Joins as an operator and switches to survival.
async fn survivor(addr: std::net::SocketAddr) -> (common::Client, i32) {
    let (mut alex, login) = join(addr, "Alex", 2).await;
    run(&mut alex, "gamemode survival").await;
    read_matching(&mut alex, |e: &GameEvent| e.event == 3).await;
    (alex, login.entity_id)
}

async fn dig(client: &mut common::Client, action: i32, pos: BlockPos, sequence: i32) {
    client
        .send(&PlayerAction {
            action,
            pos,
            face: 1,
            sequence,
        })
        .await;
}

async fn fall_to(client: &mut common::Client, pos: [f64; 3], on_ground: bool) {
    let mut body = Vec::new();
    let id = MovePlayer {
        pos: Some(pos),
        rot: None,
        flags: u8::from(on_ground),
    }
    .encode_variant(&mut body);
    client.send_raw(id, &body).await;
}

#[tokio::test]
async fn digging_takes_time_and_drops_the_block() {
    let data = GameData::load().unwrap();
    let item_type = data.entity_type("minecraft:item").unwrap();
    let dirt_item = data.item_id("minecraft:dirt").unwrap();
    let grass = data.default_state("minecraft:grass_block").unwrap() as i32;
    let addr = start(operators()).await;
    let (mut alex, _) = survivor(addr).await;
    let (mut steve, _) = join(addr, "Steve", 2).await;
    let pos = BlockPos { x: 1, y: -61, z: 0 };

    // Grass by hand: 0.6 hardness, 1/0.6/30 per tick, 18 ticks. Claiming to
    // be done at once does not work.
    dig(&mut alex, player_action::START_DESTROY_BLOCK, pos, 1).await;
    dig(&mut alex, player_action::STOP_DESTROY_BLOCK, pos, 2).await;
    let refreshed = read_matching(&mut alex, |u: &BlockUpdate| u.pos == pos).await;
    assert_eq!(refreshed.state, grass);
    read_matching(&mut alex, |a: &BlockChangedAck| a.sequence == 2).await;

    // Taking the time breaks it; grass drops dirt.
    dig(&mut alex, player_action::ABORT_DESTROY_BLOCK, pos, 3).await;
    let pos = BlockPos {
        x: -1,
        y: -61,
        z: 0,
    };
    dig(&mut alex, player_action::START_DESTROY_BLOCK, pos, 4).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    dig(&mut alex, player_action::STOP_DESTROY_BLOCK, pos, 5).await;
    let broken = read_matching(&mut steve, |u: &BlockUpdate| u.pos == pos).await;
    assert_eq!(broken.state, 0, "air");
    let drop = read_matching(&mut steve, |a: &AddEntity| a.entity_type == item_type).await;
    let shown = read_matching(&mut steve, |d: &SetEntityData| {
        d.entity_id == drop.entity_id
    })
    .await;
    let EntityData::Item(slot) = &shown.values.0[0].value else {
        panic!("{shown:?}");
    };
    assert_eq!(slot.0.as_ref().unwrap().item, dirt_item);
}

#[tokio::test]
async fn falling_hurts_and_dying_respawns() {
    let addr = start(operators()).await;
    let (mut alex, entity_id) = survivor(addr).await;

    // Up ten blocks, then down onto the grass: 10 - 3 = 7 damage.
    fall_to(&mut alex, [0.5, -50.0, 0.5], false).await;
    fall_to(&mut alex, [0.5, -60.0, 0.5], true).await;
    let health = read_matching(&mut alex, |h: &SetHealth| h.health < 20.0).await;
    assert_eq!(health.health, 13.0);

    // Thirty blocks are too many. Up and down in steps of six a tick, as
    // the server turns back bigger jumps.
    for y in (-54..=-30).step_by(6) {
        fall_to(&mut alex, [0.5, f64::from(y), 0.5], false).await;
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    for y in (-60..=-36).rev().step_by(6) {
        fall_to(&mut alex, [0.5, f64::from(y), 0.5], y == -60).await;
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    let death: PlayerCombatKill = alex.read_until().await;
    assert_eq!(death.player_id, entity_id);
    read_matching(&mut alex, |e: &EntityEvent| e.event == 3).await;

    alex.send(&ClientCommand { action: 0 }).await;
    let respawn: Respawn = alex.read_until().await;
    assert_eq!(respawn.keep, 0);
    let position: PlayerPosition = alex.read_until().await;
    assert_eq!(position.pos[1], -60.0);
    let loading = read_matching(&mut alex, |e: &GameEvent| e.event == 13).await;
    assert_eq!(loading.param, 0.0);
    let health: SetHealth = alex.read_until().await;
    assert_eq!((health.health, health.food), (20.0, 20));
}

#[tokio::test]
async fn eating_takes_its_time() {
    let data = GameData::load().unwrap();
    let apple = data.item_id("minecraft:golden_apple").unwrap();
    let addr = start(operators()).await;
    let (mut alex, entity_id) = survivor(addr).await;
    run(&mut alex, "give Alex minecraft:golden_apple 2").await;
    read_matching(&mut alex, |s: &ContainerSetSlot| {
        s.stack.0.as_ref().is_some_and(|s| s.item == apple)
    })
    .await;

    // Golden apples may always be eaten; it takes 1.6 seconds.
    alex.send(&UseItem {
        hand: 0,
        sequence: 1,
        yaw: 0.0,
        pitch: 0.0,
    })
    .await;
    let started = std::time::Instant::now();
    let done = read_matching(&mut alex, |e: &EntityEvent| e.event == 9).await;
    assert_eq!(done.entity_id, entity_id);
    assert!(
        started.elapsed() >= Duration::from_millis(1400),
        "{:?}",
        started.elapsed()
    );
    let left = read_matching(&mut alex, |s: &ContainerSetSlot| s.slot == 36).await;
    assert_eq!(left.stack.0.unwrap().count, 1);
}
