//! Fire burns and spreads, rain falls and wets farmland, lightning strikes,
//! lava burns, water puts fire out and takes the breath away.

mod common;

use std::time::Duration;

use common::{Client, click, hold_some, join, read_matching, start, test_config, walk};
use ferrocraft::Config;
use ferrocraft::data::GameData;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    AddEntity, BlockUpdate, ChatCommand, EntityData, GameEvent, SetEntityData, SetHealth,
    data_index,
};

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
}

fn at(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos { x, y, z }
}

async fn becomes(
    client: &mut Client,
    pos: BlockPos,
    seconds: u64,
    wanted: impl Fn(u32) -> bool,
) -> u32 {
    tokio::time::timeout(
        Duration::from_secs(seconds),
        read_matching(client, |u: &BlockUpdate| {
            u.pos == pos && wanted(u.state as u32)
        }),
    )
    .await
    .unwrap_or_else(|_| panic!("the block at {pos:?} did not change as expected"))
    .state as u32
}

/// The shared flags a player's own client is told of.
fn on_fire(data: &SetEntityData) -> Option<bool> {
    data.values.0.iter().find_map(|v| match v.value {
        EntityData::Byte(flags) if v.index == data_index::SHARED_FLAGS => Some(flags & 1 != 0),
        _ => None,
    })
}

fn air(data: &SetEntityData) -> Option<i32> {
    data.values.0.iter().find_map(|v| match v.value {
        EntityData::Int(air) if v.index == data_index::AIR_SUPPLY => Some(air),
        _ => None,
    })
}

#[tokio::test]
async fn flint_and_steel_lights_a_fire_that_eats_wool() {
    let data = GameData::load().unwrap();
    let flint = data.item_id("minecraft:flint_and_steel").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "fill -2 -61 5 2 -61 9 white_wool").await;
    walk(&mut alex, [0.5, -60.0, 3.5], None).await;
    hold_some(&mut alex, flint, 1).await;
    alex.send(&click(at(0, -61, 7), 1, 1)).await;
    becomes(&mut alex, at(0, -60, 7), 2, |s| {
        data.state_is(s, "minecraft:fire")
    })
    .await;
    // Sooner or later the wool burns away or catches fire.
    tokio::time::timeout(
        Duration::from_secs(40),
        read_matching(&mut alex, |u: &BlockUpdate| {
            u.pos.y == -61
                && (-2..=2).contains(&u.pos.x)
                && (5..=9).contains(&u.pos.z)
                && !data.state_is(u.state as u32, "minecraft:white_wool")
        }),
    )
    .await
    .expect("the wool burns");
}

#[tokio::test]
async fn rain_starts_for_everyone_and_wets_farmland() {
    let data = GameData::load().unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "setblock 5 -61 -5 farmland").await;
    run(&mut alex, "weather rain").await;
    // It fades in; once it counts as rain, clients hear it starts.
    let started = tokio::time::timeout(
        Duration::from_secs(3),
        read_matching(&mut alex, |e: &GameEvent| e.event == 1),
    )
    .await;
    assert!(started.is_ok(), "rain started");
    run(&mut alex, "gamerule random_tick_speed 1000").await;
    becomes(&mut alex, at(5, -61, -5), 5, |s| {
        data.block_of(s).and_then(|(_, b)| b.value(s, "moisture")) == Some("7")
    })
    .await;
    run(&mut alex, "gamerule random_tick_speed 0").await;
    run(&mut alex, "weather clear").await;
    let stopped = tokio::time::timeout(
        Duration::from_secs(3),
        read_matching(&mut alex, |e: &GameEvent| e.event == 2),
    )
    .await;
    assert!(stopped.is_ok(), "rain stopped");
}

#[tokio::test]
async fn lightning_sets_fire_and_hurts() {
    let data = GameData::load().unwrap();
    let bolt = data.entity_type("minecraft:lightning_bolt").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "gamemode survival").await;
    run(&mut alex, "summon lightning_bolt").await;
    read_matching(&mut alex, |a: &AddEntity| a.entity_type == bolt).await;
    becomes(&mut alex, at(0, -60, 0), 2, |s| {
        data.state_is(s, "minecraft:fire")
    })
    .await;
    let health = read_matching(&mut alex, |h: &SetHealth| h.health < 20.0).await;
    assert!(
        health.health <= 15.0,
        "five points or more: {}",
        health.health
    );
}

#[tokio::test]
async fn lava_burns_and_water_puts_it_out() {
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "gamemode survival").await;
    run(&mut alex, "setblock 0 -60 0 lava").await;
    read_matching(&mut alex, |d: &SetEntityData| on_fire(d) == Some(true)).await;
    let health = read_matching(&mut alex, |h: &SetHealth| h.health < 20.0).await;
    assert!(health.health <= 16.0, "lava does four: {}", health.health);
    run(&mut alex, "setblock 0 -60 0 water").await;
    tokio::time::timeout(
        Duration::from_secs(2),
        read_matching(&mut alex, |d: &SetEntityData| on_fire(d) == Some(false)),
    )
    .await
    .expect("the water puts the fire out");
}

#[tokio::test]
async fn under_water_the_air_runs_out() {
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "gamemode survival").await;
    run(&mut alex, "fill 0 -60 0 0 -59 0 water").await;
    let less = read_matching(&mut alex, |d: &SetEntityData| {
        air(d).is_some_and(|a| a < 300)
    })
    .await;
    assert!(air(&less).unwrap() >= 290);
    // Out of the water it comes back.
    run(&mut alex, "fill 0 -60 0 0 -59 0 air").await;
    read_matching(&mut alex, |d: &SetEntityData| air(d) == Some(300)).await;
}

#[tokio::test]
async fn mobs_burn_in_lava() {
    let data = GameData::load().unwrap();
    let cow = data.entity_type("minecraft:cow").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "setblock 4 -60 4 lava").await;
    run(&mut alex, "summon cow 4.5 -60 4.5").await;
    let added = read_matching(&mut alex, |a: &AddEntity| a.entity_type == cow).await;
    let lava = data
        .registry_id("minecraft:damage_type", "minecraft:lava")
        .unwrap();
    read_matching(
        &mut alex,
        |d: &ferrocraft_protocol::packets::play::DamageEvent| {
            d.entity_id == added.entity_id && d.source_type == lava
        },
    )
    .await;
    read_matching(&mut alex, |d: &SetEntityData| {
        d.entity_id == added.entity_id && on_fire(d) == Some(true)
    })
    .await;
}
