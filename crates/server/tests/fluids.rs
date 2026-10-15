//! Water and lava flow, meet and harden; sand falls; buckets scoop and
//! pour.

mod common;

use std::time::Duration;

use common::{Client, hold_some, join, read_matching, start, test_config, walk};
use ferrocraft::Config;
use ferrocraft::data::GameData;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    AddEntity, BlockUpdate, ChatCommand, ContainerSetSlot, GameEvent, UseItem,
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

/// Waits up to `seconds` for the block at `pos` to become a state `wanted`
/// accepts.
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

/// A state of `block` with `level`.
fn level(data: &GameData, block: &str, level: u32) -> u32 {
    data.block_named(block).unwrap().first + level
}

#[tokio::test]
async fn water_flows_seven_blocks_and_dries_up() {
    let data = GameData::load().unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "setblock 0 -60 0 water").await;
    // One level less per block, flowing every 5 ticks.
    becomes(&mut alex, at(1, -60, 0), 3, |s| {
        s == level(&data, "minecraft:water", 1)
    })
    .await;
    becomes(&mut alex, at(7, -60, 0), 6, |s| {
        s == level(&data, "minecraft:water", 7)
    })
    .await;
    // Seven blocks is as far as it gets.
    let beyond = tokio::time::timeout(
        Duration::from_secs(1),
        read_matching(&mut alex, |u: &BlockUpdate| u.pos == at(8, -60, 0)),
    )
    .await;
    assert!(beyond.is_err(), "water flowed eight blocks");
    // Without its source the water runs dry.
    run(&mut alex, "setblock 0 -60 0 air").await;
    becomes(&mut alex, at(7, -60, 0), 8, |s| s == 0).await;
}

#[tokio::test]
async fn two_water_sources_make_a_third() {
    let data = GameData::load().unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "setblock 10 -60 0 water").await;
    run(&mut alex, "setblock 12 -60 0 water").await;
    becomes(&mut alex, at(11, -60, 0), 3, |s| {
        s == level(&data, "minecraft:water", 0)
    })
    .await;
}

#[tokio::test]
async fn water_falls_and_finds_the_hole() {
    let data = GameData::load().unwrap();
    let water = data.block_named("minecraft:water").unwrap();
    let is_water = |s: u32| (water.first..water.first + 16).contains(&s);
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    // A hole two blocks east: the water heads for it, not west.
    run(&mut alex, "setblock 2 -61 5 air").await;
    run(&mut alex, "setblock 0 -60 5 water").await;
    // Falling water (level 8 and up) fills the hole.
    becomes(&mut alex, at(2, -61, 5), 4, |s| {
        is_water(s) && s - water.first >= 8
    })
    .await;
    let west = tokio::time::timeout(
        Duration::from_millis(500),
        read_matching(&mut alex, |u: &BlockUpdate| {
            u.pos == at(-1, -60, 5) && is_water(u.state as u32)
        }),
    )
    .await;
    assert!(west.is_err(), "the water also went the long way");
}

#[tokio::test]
async fn lava_meeting_water_hardens() {
    let data = GameData::load().unwrap();
    let obsidian = data.default_state("minecraft:obsidian").unwrap();
    let cobblestone = data.default_state("minecraft:cobblestone").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    // Water flowing next to a lava source turns it to obsidian.
    run(&mut alex, "setblock 20 -60 0 lava").await;
    run(&mut alex, "setblock 20 -60 2 water").await;
    becomes(&mut alex, at(20, -60, 0), 3, |s| s == obsidian).await;
    // Flowing lava next to water becomes cobblestone: lava first spreads
    // a block (amount 6, legacy level 2, after 30 ticks), then water comes.
    run(&mut alex, "setblock 30 -60 -10 lava").await;
    becomes(&mut alex, at(31, -60, -10), 4, |s| {
        s == level(&data, "minecraft:lava", 2)
    })
    .await;
    run(&mut alex, "setblock 33 -60 -10 water").await;
    becomes(&mut alex, at(31, -60, -10), 3, |s| s == cobblestone).await;
    // Lava flowing over water turns it to stone.
    let stone = data.default_state("minecraft:stone").unwrap();
    run(&mut alex, "setblock 55 -61 -32 water").await;
    run(&mut alex, "setblock 55 -60 -30 lava").await;
    becomes(&mut alex, at(55, -61, -32), 8, |s| s == stone).await;
}

#[tokio::test]
async fn sand_falls_and_concrete_powder_hardens_in_water() {
    let data = GameData::load().unwrap();
    let sand = data.default_state("minecraft:sand").unwrap();
    let falling = data.entity_type("minecraft:falling_block").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "setblock 5 -52 5 sand").await;
    let entity = read_matching(&mut alex, |a: &AddEntity| a.entity_type == falling).await;
    assert_eq!(entity.data, sand as i32, "the client draws sand");
    becomes(&mut alex, at(5, -60, 5), 4, |s| s == sand).await;

    let concrete = data.default_state("minecraft:white_concrete").unwrap();
    run(&mut alex, "setblock 8 -60 5 water").await;
    run(&mut alex, "setblock 8 -55 5 white_concrete_powder").await;
    becomes(&mut alex, at(8, -60, 5), 5, |s| s == concrete).await;
}

#[tokio::test]
async fn buckets_scoop_and_pour() {
    let data = GameData::load().unwrap();
    let id = |name: &str| data.item_id(name).unwrap();
    let source = level(&data, "minecraft:water", 0);
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    // Creative hands out the bucket; survival uses it up.
    hold_some(&mut alex, id("minecraft:bucket"), 1).await;
    run(&mut alex, "gamemode survival").await;
    read_matching(&mut alex, |e: &GameEvent| e.event == 3).await;
    run(&mut alex, "setblock 0 -61 3 water").await;
    becomes(&mut alex, at(0, -61, 3), 2, |s| s == source).await;
    // Standing next to the pool, looking down into it.
    walk(&mut alex, [0.5, -60.0, 2.4], Some([0.0, 60.0])).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    alex.send(&UseItem {
        hand: 0,
        sequence: 1,
        yaw: 0.0,
        pitch: 60.0,
    })
    .await;
    becomes(&mut alex, at(0, -61, 3), 2, |s| s == 0).await;
    let filled = read_matching(&mut alex, |s: &ContainerSetSlot| s.slot == 36).await;
    assert_eq!(filled.stack.0.unwrap().item, id("minecraft:water_bucket"));
    // Poured out again on the grass beside it.
    alex.send(&UseItem {
        hand: 0,
        sequence: 2,
        yaw: 0.0,
        pitch: 60.0,
    })
    .await;
    becomes(&mut alex, at(0, -61, 3), 2, |s| s == source).await;
    let emptied = read_matching(&mut alex, |s: &ContainerSetSlot| s.slot == 36).await;
    assert_eq!(emptied.stack.0.unwrap().item, id("minecraft:bucket"));
}
