//! Pistons push up to twelve blocks, sticky ones pull one back, slime
//! takes its neighbours along; observers pulse when the block in front
//! changes.

mod common;

use std::time::Duration;

use common::{Client, click, join, read_matching, start, test_config, walk};
use ferrocraft::Config;
use ferrocraft::data::GameData;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{BlockEvent, BlockUpdate, ChatCommand};

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

/// Waits for blocks to become what is wanted, in whatever order they
/// come; what each became.
async fn all_become(
    client: &mut Client,
    seconds: u64,
    wanted: &[(BlockPos, &dyn Fn(u32) -> bool)],
) -> Vec<u32> {
    let mut found = vec![None; wanted.len()];
    tokio::time::timeout(Duration::from_secs(seconds), async {
        while found.iter().any(Option::is_none) {
            let update: BlockUpdate = client.read_until().await;
            for (i, (pos, test)) in wanted.iter().enumerate() {
                if update.pos == *pos && test(update.state as u32) {
                    found[i] = Some(update.state as u32);
                }
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("not all blocks changed: {found:?}"));
    found.into_iter().flatten().collect()
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(300)).await;
}

#[tokio::test]
async fn a_piston_pushes_and_a_sticky_one_pulls_back() {
    let data = GameData::load().unwrap();
    let stone = data.default_state("minecraft:stone").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "setblock 0 -60 3 piston[facing=east]").await;
    run(&mut alex, "setblock 1 -60 3 stone").await;
    run(&mut alex, "setblock 0 -60 4 lever[face=floor,facing=east]").await;
    settle().await;
    alex.send(&click(at(0, -60, 4), 1, 1)).await;
    // Clients animate it from the block event...
    let event = read_matching(&mut alex, |e: &BlockEvent| e.pos == at(0, -60, 3)).await;
    assert_eq!((event.event, event.param), (0, 5), "extend towards east");
    // ...and get the blocks where they end up.
    let landed = all_become(
        &mut alex,
        2,
        &[
            (at(2, -60, 3), &|s| s == stone),
            (at(1, -60, 3), &|s| {
                data.state_is(s, "minecraft:piston_head")
            }),
        ],
    )
    .await;
    assert_eq!(value(&data, landed[1], "facing"), Some("east"));

    // A normal piston leaves the block where it pushed it.
    alex.send(&click(at(0, -60, 4), 1, 2)).await;
    let event = read_matching(&mut alex, |e: &BlockEvent| e.pos == at(0, -60, 3)).await;
    assert_eq!(event.event, 1, "contract");
    becomes(&mut alex, at(0, -60, 3), 2, |s| {
        data.state_is(s, "minecraft:piston") && value(&data, s, "extended") == Some("false")
    })
    .await;

    // A sticky one brings it back.
    run(&mut alex, "setblock 0 -60 8 sticky_piston[facing=east]").await;
    run(&mut alex, "setblock 1 -60 8 stone").await;
    run(&mut alex, "setblock 0 -60 9 lever[face=floor,facing=east]").await;
    walk(&mut alex, [2.5, -60.0, 6.5], None).await;
    settle().await;
    alex.send(&click(at(0, -60, 9), 1, 3)).await;
    becomes(&mut alex, at(2, -60, 8), 2, |s| s == stone).await;
    alex.send(&click(at(0, -60, 9), 1, 4)).await;
    becomes(&mut alex, at(1, -60, 8), 2, |s| s == stone).await;
}

#[tokio::test]
async fn twelve_blocks_move_thirteen_do_not() {
    let data = GameData::load().unwrap();
    let stone = data.default_state("minecraft:stone").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "setblock -10 -60 -6 piston[facing=east]").await;
    run(&mut alex, "fill -9 -60 -6 3 -60 -6 stone").await;
    run(
        &mut alex,
        "setblock -10 -60 -5 lever[face=floor,facing=east]",
    )
    .await;
    walk(&mut alex, [-7.5, -60.0, -3.5], None).await;
    settle().await;
    alex.send(&click(at(-10, -60, -5), 1, 1)).await;
    let moved = tokio::time::timeout(
        Duration::from_secs(1),
        read_matching(&mut alex, |e: &BlockEvent| e.pos == at(-10, -60, -6)),
    )
    .await;
    assert!(moved.is_err(), "thirteen blocks are too many");
    // One less, and the lever is pulled anew.
    run(&mut alex, "setblock 3 -60 -6 air").await;
    alex.send(&click(at(-10, -60, -5), 1, 2)).await;
    alex.send(&click(at(-10, -60, -5), 1, 3)).await;
    becomes(&mut alex, at(3, -60, -6), 3, |s| s == stone).await;
}

#[tokio::test]
async fn slime_takes_its_neighbours_along() {
    let data = GameData::load().unwrap();
    let stone = data.default_state("minecraft:stone").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    // Up in the air: on the ground the slime would hold on to the grass
    // below and try to take the whole row along.
    run(&mut alex, "setblock 0 -60 -4 lever[face=floor,facing=east]").await;
    run(&mut alex, "setblock 0 -59 -4 piston[facing=east]").await;
    run(&mut alex, "setblock 1 -59 -4 slime_block").await;
    run(&mut alex, "setblock 1 -58 -4 stone").await;
    settle().await;
    alex.send(&click(at(0, -60, -4), 2, 1)).await;
    // The stone on top moves with the slime.
    all_become(
        &mut alex,
        3,
        &[
            (at(2, -58, -4), &|s| s == stone),
            (at(2, -59, -4), &|s| {
                data.state_is(s, "minecraft:slime_block")
            }),
        ],
    )
    .await;
}

#[tokio::test]
async fn an_observer_pulses_when_its_front_changes() {
    let data = GameData::load().unwrap();
    let lit = |s: u32| value(&data, s, "lit") == Some("true");
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    // It looks west at x = 4 and powers the lamp east of it.
    run(&mut alex, "setblock 5 -60 12 observer[facing=west]").await;
    run(&mut alex, "setblock 6 -60 12 redstone_lamp").await;
    settle().await;
    run(&mut alex, "setblock 4 -60 12 stone").await;
    becomes(&mut alex, at(5, -60, 12), 2, |s| {
        value(&data, s, "powered") == Some("true")
    })
    .await;
    becomes(&mut alex, at(6, -60, 12), 2, lit).await;
    becomes(&mut alex, at(6, -60, 12), 2, |s| !lit(s)).await;
}
