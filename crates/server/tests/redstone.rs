//! Redstone: levers, buttons and pressure plates power wire, which loses a
//! level per block; lamps light, torches invert, repeaters delay, doors
//! open, TNT goes off.

mod common;

use std::time::{Duration, Instant};

use common::{Client, click, join, read_matching, start, test_config, walk};
use ferrocraft::Config;
use ferrocraft::data::GameData;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{AddEntity, BlockUpdate, ChatCommand};

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

fn value<'a>(data: &'a GameData, state: u32, key: &str) -> Option<&'a str> {
    data.block_of(state)?.1.value(state, key)
}

/// Waits until the setup commands are through.
async fn settle(client: &mut Client) {
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = client;
}

#[tokio::test]
async fn a_lever_powers_wire_and_lights_a_lamp() {
    let data = GameData::load().unwrap();
    let wire = |s: u32| data.state_is(s, "minecraft:redstone_wire");
    let power = |s: u32| -> i32 { value(&data, s, "power").unwrap().parse().unwrap() };
    let lit = |s: u32| value(&data, s, "lit") == Some("true");
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "setblock 0 -60 3 lever[face=floor,facing=east]").await;
    run(&mut alex, "fill 1 -60 3 5 -60 3 redstone_wire").await;
    run(&mut alex, "setblock 6 -60 3 redstone_lamp").await;
    settle(&mut alex).await;

    alex.send(&click(at(0, -60, 3), 1, 1)).await;
    // One level less per block of wire.
    becomes(&mut alex, at(1, -60, 3), 2, |s| wire(s) && power(s) == 15).await;
    becomes(&mut alex, at(5, -60, 3), 2, |s| wire(s) && power(s) == 11).await;
    becomes(&mut alex, at(6, -60, 3), 2, |s| {
        data.state_is(s, "minecraft:redstone_lamp") && lit(s)
    })
    .await;

    // Off again: the wire goes dark at once, the lamp four ticks later.
    alex.send(&click(at(0, -60, 3), 1, 2)).await;
    let (mut dark, mut out) = (None, None);
    tokio::time::timeout(Duration::from_secs(2), async {
        while dark.is_none() || out.is_none() {
            let update: BlockUpdate = alex.read_until().await;
            let state = update.state as u32;
            if update.pos == at(5, -60, 3) && wire(state) && power(state) == 0 {
                dark = Some(Instant::now());
            }
            if update.pos == at(6, -60, 3) && !lit(state) {
                out = Some(Instant::now());
            }
        }
    })
    .await
    .expect("not off");
    // Wall-clock gaps shrink when the server catches up on ticks; the
    // order is what is certain.
    assert!(
        out.unwrap() > dark.unwrap(),
        "the lamp went out before the wire"
    );
}

#[tokio::test]
async fn a_wire_placed_alone_is_a_cross_and_joins_its_neighbours() {
    let data = GameData::load().unwrap();
    let redstone = data.item_id("minecraft:redstone").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    common::hold(&mut alex, redstone).await;
    alex.send(&click(at(2, -61, -2), 1, 1)).await;
    let placed = becomes(&mut alex, at(2, -60, -2), 2, |s| s != 0).await;
    for side in ["north", "south", "east", "west"] {
        assert_eq!(value(&data, placed, side), Some("side"), "{side}");
    }
    // A second wire beside it: the first turns into a line towards it.
    alex.send(&click(at(3, -61, -2), 1, 2)).await;
    let joined = becomes(&mut alex, at(2, -60, -2), 2, |s| {
        value(&data, s, "north") == Some("none")
    })
    .await;
    assert_eq!(value(&data, joined, "east"), Some("side"));
    assert_eq!(value(&data, joined, "west"), Some("side"));
}

#[tokio::test]
async fn a_redstone_torch_goes_out_when_its_block_is_powered() {
    let data = GameData::load().unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "setblock 10 -60 3 stone").await;
    run(&mut alex, "setblock 10 -59 3 redstone_torch").await;
    run(&mut alex, "setblock 11 -60 3 lever[face=wall,facing=east]").await;
    walk(&mut alex, [12.5, -60.0, 1.5], None).await;
    settle(&mut alex).await;
    alex.send(&click(at(11, -60, 3), 1, 1)).await;
    becomes(&mut alex, at(10, -59, 3), 2, |s| {
        data.state_is(s, "minecraft:redstone_torch") && value(&data, s, "lit") == Some("false")
    })
    .await;
    alex.send(&click(at(11, -60, 3), 1, 2)).await;
    becomes(&mut alex, at(10, -59, 3), 2, |s| {
        data.state_is(s, "minecraft:redstone_torch") && value(&data, s, "lit") == Some("true")
    })
    .await;
}

#[tokio::test]
async fn a_repeater_passes_power_on_after_its_delay() {
    let data = GameData::load().unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    // Lever south of the repeater, lamp north; the repeater takes its input
    // from the side it faces.
    run(&mut alex, "setblock 20 -60 6 lever[face=floor,facing=east]").await;
    run(
        &mut alex,
        "setblock 20 -60 5 repeater[facing=south,delay=4]",
    )
    .await;
    run(&mut alex, "setblock 20 -60 4 redstone_lamp").await;
    walk(&mut alex, [22.5, -60.0, 6.5], None).await;
    settle(&mut alex).await;
    alex.send(&click(at(20, -60, 6), 1, 1)).await;
    // The lever moves first; eight game ticks later the repeater, then the
    // lamp.
    becomes(&mut alex, at(20, -60, 6), 2, |s| {
        value(&data, s, "powered") == Some("true")
    })
    .await;
    becomes(&mut alex, at(20, -60, 5), 2, |s| {
        value(&data, s, "powered") == Some("true")
    })
    .await;
    becomes(&mut alex, at(20, -60, 4), 2, |s| {
        value(&data, s, "lit") == Some("true")
    })
    .await;
}

#[tokio::test]
async fn a_button_opens_an_iron_door_for_a_moment() {
    let data = GameData::load().unwrap();
    let open = |s: u32| value(&data, s, "open") == Some("true");
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(
        &mut alex,
        "setblock 2 -60 -5 iron_door[half=lower,facing=south]",
    )
    .await;
    run(
        &mut alex,
        "setblock 2 -59 -5 iron_door[half=upper,facing=south]",
    )
    .await;
    run(
        &mut alex,
        "setblock 3 -60 -5 stone_button[face=floor,facing=south]",
    )
    .await;
    settle(&mut alex).await;
    // An iron door does not open by hand.
    alex.send(&click(at(2, -60, -5), 3, 1)).await;
    let refreshed = read_matching(&mut alex, |u: &BlockUpdate| u.pos == at(2, -60, -5)).await;
    assert!(!open(refreshed.state as u32));
    alex.send(&click(at(3, -60, -5), 1, 2)).await;
    becomes(&mut alex, at(2, -59, -5), 2, open).await;
    // A stone button stays in for 20 ticks.
    becomes(&mut alex, at(2, -60, -5), 3, |s| !open(s)).await;
}

#[tokio::test]
async fn a_redstone_block_sets_off_tnt() {
    let data = GameData::load().unwrap();
    let tnt = data.entity_type("minecraft:tnt").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "setblock 30 -60 -10 tnt").await;
    settle(&mut alex).await;
    run(&mut alex, "setblock 31 -60 -10 redstone_block").await;
    becomes(&mut alex, at(30, -60, -10), 2, |s| s == 0).await;
    read_matching(&mut alex, |a: &AddEntity| a.entity_type == tnt).await;
}

#[tokio::test]
async fn a_pressure_plate_feels_a_player() {
    let data = GameData::load().unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "setblock -3 -60 -3 stone_pressure_plate").await;
    run(&mut alex, "setblock -4 -60 -3 redstone_lamp").await;
    settle(&mut alex).await;
    walk(&mut alex, [-2.5, -60.0, -2.5], None).await;
    becomes(&mut alex, at(-3, -60, -3), 2, |s| {
        value(&data, s, "powered") == Some("true")
    })
    .await;
    becomes(&mut alex, at(-4, -60, -3), 2, |s| {
        value(&data, s, "lit") == Some("true")
    })
    .await;
    // Stepping off, it clicks up again after its check.
    walk(&mut alex, [0.5, -60.0, 0.5], None).await;
    becomes(&mut alex, at(-3, -60, -3), 3, |s| {
        value(&data, s, "powered") == Some("false")
    })
    .await;
}
