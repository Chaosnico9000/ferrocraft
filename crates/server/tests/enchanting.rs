//! Enchantments: given with `/give` and `/enchant`, offered by the
//! enchanting table for levels and lapis, combined and named on the anvil,
//! taken off at the grindstone; sharpness kills sooner, silk touch keeps
//! stone whole, feather falling softens falls, mending repairs with
//! experience.

mod common;

use std::time::Duration;

use common::{Client, click, join, read_matching, start, test_config, walk};
use ferrocraft::Config;
use ferrocraft::data::GameData;
use ferrocraft::stack;
use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    AddEntity, Attack, ChatCommand, ContainerButtonClick, ContainerClick, ContainerSetContent,
    ContainerSetData, ContainerSetSlot, EntityData, EntityEvent, GameEvent, ItemStack, MovePlayer,
    OpenScreen, PlayerAction, RenameItem, SetCarriedItem, SetCreativeModeSlot, SetEntityData,
    SetExperience, SetHealth, UntrustedSlot, container_input, player_action,
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

async fn survival(client: &mut Client) {
    run(client, "gamemode survival").await;
    read_matching(client, |e: &GameEvent| e.event == 3).await;
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

fn pickup(container_id: i32, slot: i16) -> ContainerClick {
    ContainerClick {
        input: container_input::PICKUP,
        ..shift(container_id, slot)
    }
}

/// An item with enchantments, as `/give` would make it.
fn enchanted(data: &GameData, item: &str, enchantments: &[(&str, i32)]) -> ItemStack {
    let mut stack = stack::plain(data.item_id(item).unwrap(), 1);
    let list: Vec<(String, i32)> = enchantments
        .iter()
        .map(|(n, l)| (format!("minecraft:{n}"), *l))
        .collect();
    stack::set_enchantments_in(data, &mut stack, "minecraft:enchantments", &list);
    stack
}

#[tokio::test]
async fn sharpness_from_the_command_kills_a_cow_in_one_blow() {
    let data = GameData::load().unwrap();
    let cow = data.entity_type("minecraft:cow").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    run(&mut alex, "give @s diamond_sword").await;
    run(&mut alex, "enchant @s sharpness 5").await;
    // The sword now carries the enchantment, which the client can read.
    let sword = read_matching(&mut alex, |s: &ContainerSetSlot| {
        s.stack
            .0
            .as_ref()
            .is_some_and(|s| stack::enchantment_level(&data, s, "minecraft:sharpness") == 5)
    })
    .await;
    assert_eq!(sword.slot, 36);
    run(&mut alex, "summon cow 1.5 -60 0.5").await;
    let added = read_matching(&mut alex, |a: &AddEntity| a.entity_type == cow).await;
    // A full swing: seven of the sword, three of sharpness V, as much as a
    // cow has.
    tokio::time::sleep(Duration::from_millis(800)).await;
    alex.send(&Attack {
        entity_id: added.entity_id,
    })
    .await;
    tokio::time::timeout(
        Duration::from_secs(2),
        read_matching(&mut alex, |e: &EntityEvent| {
            e.entity_id == added.entity_id && e.event == 3
        }),
    )
    .await
    .expect("one blow");
}

#[tokio::test]
async fn the_enchanting_table_takes_levels_and_lapis() {
    let data = GameData::load().unwrap();
    let book = data.item_id("minecraft:enchanted_book").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    // Fifteen bookshelves around a table, with air between.
    run(&mut alex, "fill -2 -60 2 2 -59 6 bookshelf").await;
    run(&mut alex, "fill -1 -60 3 1 -59 5 air").await;
    run(&mut alex, "setblock 0 -60 4 enchanting_table").await;
    survival(&mut alex).await;
    run(&mut alex, "experience add @s 30 levels").await;
    run(&mut alex, "give @s book").await;
    run(&mut alex, "give @s lapis_lazuli 3").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    alex.send(&click(BlockPos { x: 0, y: -60, z: 4 }, 1, 1))
        .await;
    let screen: OpenScreen = alex.read_until().await;
    assert_eq!(screen.menu, data.menu("minecraft:enchantment").unwrap());
    // The hotbar starts at 29: the book onto the table, the lapis beside.
    alex.send(&shift(screen.container_id, 29)).await;
    alex.send(&shift(screen.container_id, 30)).await;
    let cost = read_matching(&mut alex, |d: &ContainerSetData| d.id == 2 && d.value > 0).await;
    assert!(
        cost.value >= 30,
        "fifteen shelves offer 30 levels: {}",
        cost.value
    );
    read_matching(&mut alex, |d: &ContainerSetData| d.id == 6 && d.value >= 0).await;
    alex.send(&ContainerButtonClick {
        container_id: screen.container_id,
        button: 2,
    })
    .await;
    // Three levels for the third offer, then the book on the table.
    let experience = read_matching(&mut alex, |x: &SetExperience| x.level < 30).await;
    assert_eq!(experience.level, 27);
    let content = read_matching(&mut alex, |c: &ContainerSetContent| {
        c.items.0[0].0.as_ref().is_some_and(|s| s.item == book)
    })
    .await;
    let enchanted = content.items.0[0].0.as_ref().unwrap();
    assert!(!stack::enchantments_in(&data, enchanted, "minecraft:stored_enchantments").is_empty());
    assert!(content.items.0[1].0.is_none(), "three lapis used");
}

#[tokio::test]
async fn the_anvil_names_for_a_level() {
    let data = GameData::load().unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    run(&mut alex, "setblock 1 -60 2 anvil").await;
    survival(&mut alex).await;
    run(&mut alex, "experience add @s 5 levels").await;
    run(&mut alex, "give @s diamond_sword").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    alex.send(&click(BlockPos { x: 1, y: -60, z: 2 }, 1, 1))
        .await;
    let screen: OpenScreen = alex.read_until().await;
    assert_eq!(screen.menu, data.menu("minecraft:anvil").unwrap());
    alex.send(&shift(screen.container_id, 30)).await;
    alex.send(&RenameItem {
        name: "Excalibur".into(),
    })
    .await;
    read_matching(&mut alex, |d: &ContainerSetData| d.id == 0 && d.value == 1).await;
    alex.send(&pickup(screen.container_id, 2)).await;
    // The level is paid first, then the screen shows the sword in hand.
    let experience = read_matching(&mut alex, |x: &SetExperience| x.level < 5).await;
    assert_eq!(experience.level, 4);
    let content = read_matching(&mut alex, |c: &ContainerSetContent| c.carried.0.is_some()).await;
    let named = content.carried.0.unwrap();
    assert_eq!(
        stack::get(&data, &named, "minecraft:custom_name"),
        Some(Nbt::String("Excalibur".into()))
    );
    assert!(
        content.items.0[0].0.is_none(),
        "the sword went into the result"
    );
}

#[tokio::test]
async fn the_grindstone_takes_enchantments_off_for_experience() {
    let data = GameData::load().unwrap();
    let orb = data.entity_type("minecraft:experience_orb").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    run(&mut alex, "setblock 1 -60 2 grindstone").await;
    run(
        &mut alex,
        "give @s diamond_sword[enchantments={sharpness:3}]",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    alex.send(&click(BlockPos { x: 1, y: -60, z: 2 }, 1, 1))
        .await;
    let screen: OpenScreen = alex.read_until().await;
    assert_eq!(screen.menu, data.menu("minecraft:grindstone").unwrap());
    alex.send(&shift(screen.container_id, 30)).await;
    let content = read_matching(&mut alex, |c: &ContainerSetContent| {
        c.items.0[2].0.is_some()
    })
    .await;
    let result = content.items.0[2].0.as_ref().unwrap();
    assert!(stack::enchantments(&data, result).is_empty());
    alex.send(&pickup(screen.container_id, 2)).await;
    tokio::time::timeout(
        Duration::from_secs(2),
        read_matching(&mut alex, |a: &AddEntity| a.entity_type == orb),
    )
    .await
    .expect("experience for the enchantment");
}

async fn dig(client: &mut Client, action: i32, pos: BlockPos, sequence: i32) {
    client
        .send(&PlayerAction {
            action,
            pos,
            face: 1,
            sequence,
        })
        .await;
}

#[tokio::test]
async fn silk_touch_keeps_stone_whole() {
    let data = GameData::load().unwrap();
    let item_type = data.entity_type("minecraft:item").unwrap();
    let stone = data.item_id("minecraft:stone").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    let pos = BlockPos { x: 1, y: -60, z: 1 };
    run(&mut alex, "setblock 1 -60 1 stone").await;
    run(
        &mut alex,
        "give @s diamond_pickaxe[enchantments={silk_touch:1}]",
    )
    .await;
    survival(&mut alex).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    dig(&mut alex, player_action::START_DESTROY_BLOCK, pos, 1).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    dig(&mut alex, player_action::STOP_DESTROY_BLOCK, pos, 2).await;
    let drop = read_matching(&mut alex, |a: &AddEntity| a.entity_type == item_type).await;
    let shown = read_matching(&mut alex, |d: &SetEntityData| d.entity_id == drop.entity_id).await;
    let EntityData::Item(slot) = &shown.values.0[0].value else {
        panic!("{shown:?}");
    };
    assert_eq!(slot.0.as_ref().unwrap().item, stone, "not cobblestone");
}

async fn fall_to(client: &mut Client, pos: [f64; 3], on_ground: bool) {
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
async fn feather_falling_softens_a_fall() {
    let data = GameData::load().unwrap();
    let boots = enchanted(&data, "minecraft:diamond_boots", &[("feather_falling", 4)]);
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    alex.send(&SetCreativeModeSlot {
        slot: 8,
        stack: UntrustedSlot(Some(boots)),
    })
    .await;
    survival(&mut alex).await;
    // Ten blocks: seven points of damage, feather falling IV takes 12/25 of
    // them, the boots' armor nothing (falls bypass armor).
    fall_to(&mut alex, [0.5, -54.0, 0.5], false).await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    fall_to(&mut alex, [0.5, -50.0, 0.5], false).await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    fall_to(&mut alex, [0.5, -55.0, 0.5], false).await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    fall_to(&mut alex, [0.5, -60.0, 0.5], true).await;
    let health = read_matching(&mut alex, |h: &SetHealth| h.health < 20.0).await;
    assert!(
        (health.health - (20.0 - 7.0 * 13.0 / 25.0)).abs() < 0.01,
        "{}",
        health.health
    );
}

#[tokio::test]
async fn mending_repairs_with_experience() {
    let data = GameData::load().unwrap();
    let cow = data.entity_type("minecraft:cow").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    run(
        &mut alex,
        "give @s diamond_sword[damage=500,enchantments={mending:1,sharpness:5}]",
    )
    .await;
    alex.send(&SetCarriedItem { slot: 0 }).await;
    survival(&mut alex).await;
    run(&mut alex, "summon cow 1.5 -60 0.5").await;
    let added = read_matching(&mut alex, |a: &AddEntity| a.entity_type == cow).await;
    // One full swing with sharpness V kills a cow (it would run from a
    // weaker one); its experience goes to the sword.
    tokio::time::sleep(Duration::from_millis(800)).await;
    alex.send(&Attack {
        entity_id: added.entity_id,
    })
    .await;
    walk(&mut alex, [1.0, -60.0, 0.5], None).await;
    let repaired = tokio::time::timeout(
        Duration::from_secs(5),
        read_matching(&mut alex, |s: &ContainerSetSlot| {
            s.slot == 36
                && s.stack
                    .0
                    .as_ref()
                    .is_some_and(|s| stack::damage(&data, s) < 500)
        }),
    )
    .await
    .expect("repaired");
    // Two durability a point, one to three points from a cow; the swing
    // wore it by one.
    let damage = stack::damage(&data, repaired.stack.0.as_ref().unwrap());
    assert!((495..=499).contains(&damage), "{damage}");
}
