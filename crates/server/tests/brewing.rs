//! Brewing stands: blaze powder and nether wart turn water bottles into
//! awkward potions in twenty seconds; the block shows the bottles; hoppers
//! feed the reagent from above.

mod common;

use std::time::Duration;

use common::{Client, click, join, read_matching, start, test_config};
use ferrocraft::Config;
use ferrocraft::data::GameData;
use ferrocraft::stack;
use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    BlockUpdate, ChatCommand, ContainerClick, ContainerSetContent, ContainerSetData, OpenScreen,
    container_input,
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

fn shift(container_id: i32, slot: i16) -> ContainerClick {
    ContainerClick {
        container_id,
        state_id: 0,
        slot,
        button: 0,
        input: container_input::QUICK_MOVE,
        changed: Vec::new(),
        carried: None,
    }
}

fn potion_name(data: &GameData, slot: &ferrocraft_protocol::packets::play::Slot) -> Option<String> {
    let stack = slot.0.as_ref()?;
    match stack::get(data, stack, "minecraft:potion_contents")?.get("potion") {
        Some(Nbt::String(name)) => Some(name.clone()),
        _ => None,
    }
}

#[tokio::test]
async fn water_and_wart_brew_awkward_potions() {
    let data = GameData::load().unwrap();
    let stand = data.block_named("minecraft:brewing_stand").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    let pos = BlockPos { x: 1, y: -60, z: 1 };
    run(&mut alex, "setblock 1 -60 1 brewing_stand").await;
    run(
        &mut alex,
        r#"give @s potion[potion_contents={potion:"minecraft:water"}] 2"#,
    )
    .await;
    run(&mut alex, "give @s nether_wart").await;
    run(&mut alex, "give @s blaze_powder").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    alex.send(&click(pos, 1, 1)).await;
    let screen: OpenScreen = alex.read_until().await;
    assert_eq!(screen.menu, data.menu("minecraft:brewing_stand").unwrap());
    // The hotbar starts at 32: two water bottles (potions do not stack),
    // wart and powder.
    for slot in [32, 33, 34, 35] {
        alex.send(&shift(screen.container_id, slot)).await;
    }
    // Two places show bottles.
    read_matching(&mut alex, |u: &BlockUpdate| {
        u.pos == pos
            && stand.value(u.state as u32, "has_bottle_0") == Some("true")
            && stand.value(u.state as u32, "has_bottle_1") == Some("true")
    })
    .await;
    // The brew takes 400 ticks; the powder gives twenty.
    read_matching(&mut alex, |d: &ContainerSetData| d.id == 0 && d.value > 390).await;
    read_matching(&mut alex, |d: &ContainerSetData| d.id == 1 && d.value == 19).await;
    let done = tokio::time::timeout(
        Duration::from_secs(25),
        read_matching(&mut alex, |c: &ContainerSetContent| {
            potion_name(&data, &c.items.0[0]).as_deref() == Some("minecraft:awkward")
        }),
    )
    .await
    .expect("brewed");
    assert_eq!(
        potion_name(&data, &done.items.0[1]).as_deref(),
        Some("minecraft:awkward")
    );
    assert!(done.items.0[3].0.is_none(), "the wart is used up");
}

#[tokio::test]
async fn hoppers_feed_the_reagent_from_above() {
    let data = GameData::load().unwrap();
    let wart = data.item_id("minecraft:nether_wart").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    run(&mut alex, "setblock 1 -60 1 brewing_stand").await;
    run(&mut alex, "setblock 1 -59 1 hopper").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    // Wart into the hopper by hand.
    run(&mut alex, "give @s nether_wart 4").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    alex.send(&click(BlockPos { x: 1, y: -59, z: 1 }, 1, 1))
        .await;
    let screen: OpenScreen = alex.read_until().await;
    alex.send(&shift(screen.container_id, 32)).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    alex.send(
        &ferrocraft_protocol::packets::play::ServerboundContainerClose {
            container_id: screen.container_id,
        },
    )
    .await;
    alex.send(&click(BlockPos { x: 1, y: -60, z: 1 }, 2, 2))
        .await;
    let screen = read_matching(&mut alex, |o: &OpenScreen| {
        o.menu == data.menu("minecraft:brewing_stand").unwrap()
    })
    .await;
    let content = read_matching(&mut alex, |c: &ContainerSetContent| {
        c.container_id == screen.container_id
    })
    .await;
    let reagent = content.items.0[3].0.as_ref().expect("wart on top");
    assert_eq!(reagent.item, wart);
}
