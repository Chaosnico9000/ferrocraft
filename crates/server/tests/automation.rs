//! Hoppers pass items on, comparators read how full a container is,
//! droppers and dispensers throw, shoot and pour when powered.

mod common;

use std::time::Duration;

use common::{Client, click, hold, hold_some, join, read_matching, start, test_config, walk};
use ferrocraft::Config;
use ferrocraft::data::GameData;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    AddEntity, BlockUpdate, ChatCommand, ContainerClick, ContainerSetContent, OpenScreen,
    ServerboundContainerClose, container_input,
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

fn power(data: &GameData, state: u32) -> Option<i32> {
    data.block_of(state)?.1.value(state, "power")?.parse().ok()
}

fn pickup(container_id: i32, slot: i16) -> ContainerClick {
    ContainerClick {
        container_id,
        state_id: 0,
        slot,
        button: 0,
        input: container_input::PICKUP,
        changed: Vec::new(),
        carried: None,
    }
}

/// Opens the container at `pos` and moves the stack in the first hotbar
/// slot into its first slot; `size` is how many slots it has.
async fn fill(client: &mut Client, pos: BlockPos, size: i16, sequence: i32) {
    client.send(&click(pos, 1, sequence)).await;
    let screen: OpenScreen = client.read_until().await;
    let id = screen.container_id;
    client.read_until::<ContainerSetContent>().await;
    client.send(&pickup(id, size + 27)).await;
    client.read_until::<ContainerSetContent>().await;
    client.send(&pickup(id, 0)).await;
    client.read_until::<ContainerSetContent>().await;
    client
        .send(&ServerboundContainerClose { container_id: id })
        .await;
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(300)).await;
}

#[tokio::test]
async fn a_comparator_reads_a_hopper_and_the_chest_it_fills() {
    let data = GameData::load().unwrap();
    let diamond = data.item_id("minecraft:diamond").unwrap();
    let wire = |s: u32| data.state_is(s, "minecraft:redstone_wire");
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    // A hopper pointing east into a chest; a comparator behind each,
    // feeding a wire.
    run(&mut alex, "setblock 2 -60 3 hopper[facing=east]").await;
    run(&mut alex, "setblock 3 -60 3 chest").await;
    run(&mut alex, "setblock 4 -60 3 comparator[facing=west]").await;
    run(&mut alex, "setblock 5 -60 3 redstone_wire").await;
    run(&mut alex, "setblock 2 -60 4 comparator[facing=north]").await;
    run(&mut alex, "setblock 2 -60 5 redstone_wire").await;
    // Powered, the hopper keeps its items for now.
    run(&mut alex, "setblock 1 -60 3 redstone_block").await;
    settle().await;
    hold(&mut alex, diamond).await;
    fill(&mut alex, at(2, -60, 3), 5, 1).await;
    // 64 diamonds fill one of five slots: 3.
    becomes(&mut alex, at(2, -60, 5), 3, |s| {
        wire(s) && power(&data, s) == Some(3)
    })
    .await;
    // Unpowered it passes them on one by one; one diamond in a chest is 1.
    run(&mut alex, "setblock 1 -60 3 air").await;
    becomes(&mut alex, at(5, -60, 3), 3, |s| {
        wire(s) && power(&data, s) == Some(1)
    })
    .await;
}

#[tokio::test]
async fn a_hopper_takes_items_lying_on_it() {
    let data = GameData::load().unwrap();
    let diamond = data.item_id("minecraft:diamond").unwrap();
    let wire = |s: u32| data.state_is(s, "minecraft:redstone_wire");
    let item_type = data.entity_type("minecraft:item").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "setblock 2 -60 -3 hopper").await;
    run(&mut alex, "setblock 2 -60 -2 comparator[facing=north]").await;
    run(&mut alex, "setblock 2 -60 -1 redstone_wire").await;
    // A dropper above the hopper throws a diamond onto it.
    run(&mut alex, "setblock 2 -58 -3 dropper[facing=down]").await;
    settle().await;
    walk(&mut alex, [4.5, -60.0, -1.5], None).await;
    hold(&mut alex, diamond).await;
    fill(&mut alex, at(2, -58, -3), 9, 1).await;
    run(&mut alex, "setblock 3 -58 -3 redstone_block").await;
    read_matching(&mut alex, |a: &AddEntity| a.entity_type == item_type).await;
    becomes(&mut alex, at(2, -60, -1), 4, |s| {
        wire(s) && power(&data, s) == Some(1)
    })
    .await;
}

#[tokio::test]
async fn a_dispenser_shoots_arrows_and_pours_water() {
    let data = GameData::load().unwrap();
    let arrow = data.item_id("minecraft:arrow").unwrap();
    let water_bucket = data.item_id("minecraft:water_bucket").unwrap();
    let arrow_type = data.entity_type("minecraft:arrow").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "setblock -3 -60 3 dispenser[facing=west]").await;
    run(&mut alex, "setblock -3 -60 5 dispenser[facing=west]").await;
    settle().await;
    hold(&mut alex, arrow).await;
    fill(&mut alex, at(-3, -60, 3), 9, 1).await;
    run(&mut alex, "setblock -3 -59 3 redstone_block").await;
    read_matching(&mut alex, |a: &AddEntity| a.entity_type == arrow_type).await;

    hold_some(&mut alex, water_bucket, 1).await;
    fill(&mut alex, at(-3, -60, 5), 9, 2).await;
    run(&mut alex, "setblock -3 -59 5 redstone_block").await;
    becomes(&mut alex, at(-4, -60, 5), 2, |s| {
        data.state_is(s, "minecraft:water")
    })
    .await;
}

#[tokio::test]
async fn hoppers_feed_a_furnace_from_above_and_the_side() {
    let data = GameData::load().unwrap();
    let raw_iron = data.item_id("minecraft:raw_iron").unwrap();
    let coal = data.item_id("minecraft:coal").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "setblock 6 -60 -6 furnace").await;
    run(&mut alex, "setblock 6 -59 -6 hopper").await;
    run(&mut alex, "setblock 5 -60 -6 hopper[facing=east]").await;
    walk(&mut alex, [4.5, -60.0, -3.5], None).await;
    settle().await;
    hold(&mut alex, raw_iron).await;
    fill(&mut alex, at(6, -59, -6), 5, 1).await;
    hold(&mut alex, coal).await;
    fill(&mut alex, at(5, -60, -6), 5, 2).await;
    // Ore from above, coal from the side: it lights up.
    becomes(&mut alex, at(6, -60, -6), 3, |s| {
        data.block_of(s).and_then(|(_, b)| b.value(s, "lit")) == Some("true")
    })
    .await;
}

#[tokio::test]
async fn a_burning_arrow_lights_tnt() {
    let data = GameData::load().unwrap();
    let arrow = data.item_id("minecraft:arrow").unwrap();
    let tnt = data.entity_type("minecraft:tnt").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    // Shot east through fire into TNT.
    run(&mut alex, "setblock -3 -60 -3 dispenser[facing=east]").await;
    run(&mut alex, "fill -2 -60 -3 -1 -60 -3 fire").await;
    run(&mut alex, "setblock 2 -60 -3 tnt").await;
    settle().await;
    hold(&mut alex, arrow).await;
    fill(&mut alex, at(-3, -60, -3), 9, 1).await;
    run(&mut alex, "setblock -3 -59 -3 redstone_block").await;
    // The block goes, then the lit TNT appears (`TntBlock.prime`).
    becomes(&mut alex, at(2, -60, -3), 2, |s| s == 0).await;
    read_matching(&mut alex, |a: &AddEntity| a.entity_type == tnt).await;
}
