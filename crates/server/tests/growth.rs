//! Farming and growth: hoes till, seeds need farmland, crops grow (faster
//! with bone meal), saplings become trees, leaves without a log decay,
//! grass spreads, plants without ground pop off.

mod common;

use std::time::Duration;

use common::{Client, click, hold, hold_some, join, read_matching, start, test_config, walk};
use ferrocraft::Config;
use ferrocraft::data::GameData;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{AddEntity, BlockUpdate, ChatCommand, LevelEvent};

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

fn is(data: &GameData, state: u32, name: &str) -> bool {
    data.state_is(state, name)
}

fn age(data: &GameData, state: u32) -> i32 {
    let (_, block) = data.block_of(state).unwrap();
    block.value(state, "age").unwrap().parse().unwrap()
}

#[tokio::test]
async fn hoes_till_seeds_grow_into_wheat() {
    let data = GameData::load().unwrap();
    let id = |name: &str| data.item_id(name).unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    let ground = at(2, -61, 2);
    let crop = at(2, -60, 2);

    // Seeds do not take on grass.
    hold(&mut alex, id("minecraft:wheat_seeds")).await;
    alex.send(&click(ground, 1, 1)).await;
    let refused = read_matching(&mut alex, |u: &BlockUpdate| u.pos == crop).await;
    assert_eq!(refused.state, 0, "no wheat on grass");

    // The hoe turns grass into dry farmland.
    hold_some(&mut alex, id("minecraft:wooden_hoe"), 1).await;
    alex.send(&click(ground, 1, 2)).await;
    let tilled = becomes(&mut alex, ground, 2, |s| is(&data, s, "minecraft:farmland")).await;
    let (_, farmland) = data.block_of(tilled).unwrap();
    assert_eq!(farmland.value(tilled, "moisture"), Some("0"));

    hold(&mut alex, id("minecraft:wheat_seeds")).await;
    alex.send(&click(ground, 1, 3)).await;
    let planted = becomes(&mut alex, crop, 2, |s| is(&data, s, "minecraft:wheat")).await;
    assert_eq!(age(&data, planted), 0);

    // Bone meal makes it grow a few stages at once.
    hold(&mut alex, id("minecraft:bone_meal")).await;
    alex.send(&click(crop, 1, 4)).await;
    read_matching(&mut alex, |e: &LevelEvent| e.event == 1505 && e.pos == crop).await;
    let grown = becomes(&mut alex, crop, 2, |s| is(&data, s, "minecraft:wheat")).await;
    assert!(
        (2..=5).contains(&age(&data, grown)),
        "age {}",
        age(&data, grown)
    );

    // Random ticks do the rest; fast here.
    run(&mut alex, "gamerule random_tick_speed 20000").await;
    becomes(&mut alex, crop, 10, |s| {
        is(&data, s, "minecraft:wheat") && age(&data, s) == 7
    })
    .await;
    run(&mut alex, "gamerule random_tick_speed 0").await;

    // Without farmland below the wheat pops off; its drops come first, as
    // in `Level.destroyBlock`.
    let item = data.entity_type("minecraft:item").unwrap();
    run(&mut alex, "setblock 2 -61 2 stone").await;
    read_matching(&mut alex, |a: &AddEntity| a.entity_type == item).await;
    becomes(&mut alex, crop, 2, |s| s == 0).await;
}

#[tokio::test]
async fn farmland_gets_wet_near_water_and_dries_out() {
    let data = GameData::load().unwrap();
    let moisture = |s: u32| -> Option<i32> {
        let (_, b) = data.block_of(s)?;
        (b.name == "minecraft:farmland").then(|| b.value(s, "moisture").unwrap().parse().unwrap())
    };
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "gamerule random_tick_speed 1000").await;
    run(&mut alex, "setblock 5 -61 5 water").await;
    run(&mut alex, "setblock 7 -61 5 farmland").await;
    becomes(&mut alex, at(7, -61, 5), 5, |s| moisture(s) == Some(7)).await;
    // Far from water, bare farmland turns back to dirt.
    let dirt = data.default_state("minecraft:dirt").unwrap();
    run(&mut alex, "setblock -20 -61 -20 farmland").await;
    becomes(&mut alex, at(-20, -61, -20), 5, |s| s == dirt).await;
}

#[tokio::test]
async fn saplings_grow_into_trees_and_lone_leaves_decay() {
    let data = GameData::load().unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "setblock 0 -60 8 oak_sapling").await;
    run(&mut alex, "setblock 10 -55 8 oak_leaves").await;
    run(&mut alex, "setblock -10 -55 8 oak_leaves[persistent=true]").await;
    run(&mut alex, "gamerule random_tick_speed 1000").await;
    // An oak: its trunk takes the sapling's place. Leaves far from a log
    // rot away meanwhile; placed ones stay.
    let (mut tree, mut rotted) = (false, false);
    tokio::time::timeout(Duration::from_secs(10), async {
        while !(tree && rotted) {
            let update: BlockUpdate = alex.read_until().await;
            let state = update.state as u32;
            tree |= update.pos == at(0, -60, 8) && is(&data, state, "minecraft:oak_log");
            rotted |= update.pos == at(10, -55, 8) && state == 0;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("tree {tree}, rotted {rotted}"));
    let kept = tokio::time::timeout(
        Duration::from_secs(1),
        read_matching(&mut alex, |u: &BlockUpdate| u.pos == at(-10, -55, 8)),
    )
    .await;
    assert!(kept.is_err(), "persistent leaves decayed");
}

#[tokio::test]
async fn grass_spreads_onto_dirt_in_the_light() {
    let data = GameData::load().unwrap();
    let grass = data.default_state("minecraft:grass_block").unwrap();
    let dirt = data.default_state("minecraft:dirt").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "setblock 3 -61 -3 dirt").await;
    becomes(&mut alex, at(3, -61, -3), 2, |s| s == dirt).await;
    run(&mut alex, "gamerule random_tick_speed 1000").await;
    becomes(&mut alex, at(3, -61, -3), 10, |s| s == grass).await;
    // Under stone it dies.
    run(&mut alex, "setblock 3 -60 -3 stone").await;
    becomes(&mut alex, at(3, -61, -3), 10, |s| s == dirt).await;
}

#[tokio::test]
async fn sugar_cane_needs_water_and_grows() {
    let data = GameData::load().unwrap();
    let cane = data.default_state("minecraft:sugar_cane").unwrap();
    let id = |name: &str| data.item_id(name).unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    walk(&mut alex, [-3.5, -60.0, 3.5], None).await;
    hold(&mut alex, id("minecraft:sugar_cane")).await;
    // Dry ground: nothing.
    alex.send(&click(at(-5, -61, 5), 1, 1)).await;
    let refused = read_matching(&mut alex, |u: &BlockUpdate| u.pos == at(-5, -60, 5)).await;
    assert_eq!(refused.state, 0);
    // Next to water it takes, and grows up to three high.
    run(&mut alex, "setblock -4 -61 5 water").await;
    alex.send(&click(at(-5, -61, 5), 1, 2)).await;
    becomes(&mut alex, at(-5, -60, 5), 2, |s| s == cane).await;
    run(&mut alex, "gamerule random_tick_speed 4000").await;
    becomes(&mut alex, at(-5, -58, 5), 15, |s| s == cane).await;
    let fourth = tokio::time::timeout(
        Duration::from_secs(2),
        read_matching(&mut alex, |u: &BlockUpdate| u.pos == at(-5, -57, 5)),
    )
    .await;
    assert!(fourth.is_err(), "sugar cane grew four high");
    run(&mut alex, "gamerule random_tick_speed 0").await;
    // Without its bottom the cane breaks, all of it.
    run(&mut alex, "setblock -5 -60 5 air").await;
    becomes(&mut alex, at(-5, -58, 5), 2, |s| s == 0).await;
}
