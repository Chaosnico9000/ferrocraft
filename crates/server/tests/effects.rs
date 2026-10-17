//! Status effects and potions: `/effect` gives and takes them, the client
//! hears of them and their attributes, poison stops at half a heart, milk
//! cures, golden apples give golden hearts, potions are drunk, thrown and
//! leave clouds.

mod common;

use std::time::Duration;

use common::{Client, join, read_matching, start, test_config, walk};
use ferrocraft::Config;
use ferrocraft::data::GameData;
use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::packets::play::{
    AddEntity, ChatCommand, ContainerSetSlot, EntityData, GameEvent, RemoveMobEffect,
    SetEntityData, SetHealth, SystemChat, UpdateAttributes, UpdateMobEffect, UseItem, data_index,
    effect_flags,
};

fn operators() -> Config {
    Config {
        operators: vec!["Alex".into(), "Steve".into()],
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

/// The next answer to a command (not news of who joined).
async fn answer(client: &mut Client) -> String {
    let answer = read_matching(client, |c: &SystemChat| {
        !format!("{:?}", c.content).contains("multiplayer.player")
    })
    .await;
    format!("{:?}", answer.content)
}

async fn survival(client: &mut Client) {
    run(client, "gamemode survival").await;
    read_matching(client, |e: &GameEvent| e.event == 3).await;
}

/// Uses the item in hand looking straight down.
async fn use_item(client: &mut Client, sequence: i32) {
    client
        .send(&UseItem {
            hand: 0,
            sequence,
            yaw: 0.0,
            pitch: 90.0,
        })
        .await;
}

fn effect(data: &GameData, name: &str) -> i32 {
    data.effect(name).unwrap().0 as i32
}

#[tokio::test]
async fn effects_come_with_their_attributes_and_go() {
    let data = GameData::load().unwrap();
    let speed = effect(&data, "minecraft:speed");
    let movement = data.attribute("minecraft:movement_speed").unwrap().0 as i32;
    let addr = start(operators()).await;
    let (mut alex, login) = join(addr, "Alex", 2).await;
    run(&mut alex, "effect give @s speed 30 1").await;
    let given = read_matching(&mut alex, |e: &UpdateMobEffect| e.effect == speed).await;
    assert_eq!(
        (given.entity_id, given.amplifier, given.duration),
        (login.entity_id, 1, 600)
    );
    assert_ne!(given.flags & effect_flags::VISIBLE, 0);
    // Speed II: 40 % more on the total.
    let attributes = read_matching(&mut alex, |u: &UpdateAttributes| {
        u.attributes.0.iter().any(|a| a.attribute == movement)
    })
    .await;
    let snapshot = attributes
        .attributes
        .0
        .iter()
        .find(|a| a.attribute == movement)
        .unwrap();
    assert_eq!(snapshot.modifiers[0].id, "minecraft:effect.speed");
    assert!((snapshot.modifiers[0].amount - 0.4).abs() < 1e-6);
    assert_eq!(snapshot.modifiers[0].operation, 2);
    assert!(
        answer(&mut alex)
            .await
            .contains("commands.effect.give.success.single")
    );

    run(&mut alex, "effect clear @s speed").await;
    read_matching(&mut alex, |e: &RemoveMobEffect| e.effect == speed).await;
    let attributes = read_matching(&mut alex, |u: &UpdateAttributes| {
        u.attributes.0.iter().any(|a| a.attribute == movement)
    })
    .await;
    assert!(attributes.attributes.0[0].modifiers.is_empty());
    assert!(
        answer(&mut alex)
            .await
            .contains("commands.effect.clear.specific.success")
    );
    run(&mut alex, "effect clear @s").await;
    assert!(
        answer(&mut alex)
            .await
            .contains("commands.effect.clear.everything.failed")
    );
}

#[tokio::test]
async fn poison_stops_at_half_a_heart_and_milk_cures() {
    let data = GameData::load().unwrap();
    let poison = effect(&data, "minecraft:poison");
    let bucket = data.item_id("minecraft:bucket").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    survival(&mut alex).await;
    // Instant damage II: twelve points at once.
    run(&mut alex, "effect give @s instant_damage 1 1").await;
    read_matching(&mut alex, |h: &SetHealth| h.health == 8.0).await;
    // Poison V wants to hurt every tick, the hurt cooldown lets it twice a
    // second; it stops before the last point.
    run(&mut alex, "effect give @s poison 30 4").await;
    // (Eating heals a little in between, so the last hit may leave a
    // fraction.)
    let low = tokio::time::timeout(
        Duration::from_secs(8),
        read_matching(&mut alex, |h: &SetHealth| h.health <= 1.0),
    )
    .await
    .expect("poisoned down to half a heart");
    assert!(low.health > 0.0, "poison does not kill");
    tokio::time::sleep(Duration::from_millis(300)).await;
    run(&mut alex, "give @s milk_bucket").await;
    use_item(&mut alex, 1).await;
    // Drinking takes 1.6 seconds; then the poison is gone and the bucket
    // is back.
    tokio::time::timeout(
        Duration::from_secs(4),
        read_matching(&mut alex, |e: &RemoveMobEffect| e.effect == poison),
    )
    .await
    .expect("the milk cured");
    read_matching(&mut alex, |s: &ContainerSetSlot| {
        s.stack.0.as_ref().is_some_and(|s| s.item == bucket)
    })
    .await;
}

#[tokio::test]
async fn golden_apples_give_golden_hearts() {
    let data = GameData::load().unwrap();
    let absorption = effect(&data, "minecraft:absorption");
    let regeneration = effect(&data, "minecraft:regeneration");
    let addr = start(operators()).await;
    let (mut alex, login) = join(addr, "Alex", 2).await;
    survival(&mut alex).await;
    run(&mut alex, "give @s golden_apple").await;
    use_item(&mut alex, 1).await;
    let regen = read_matching(&mut alex, |e: &UpdateMobEffect| e.effect == regeneration).await;
    assert_eq!((regen.amplifier, regen.duration), (1, 100));
    read_matching(&mut alex, |e: &UpdateMobEffect| e.effect == absorption).await;
    // Four golden half hearts, to the player's own client.
    let hearts = read_matching(&mut alex, |d: &SetEntityData| {
        d.entity_id == login.entity_id
            && d.values.0.iter().any(|v| v.index == data_index::ABSORPTION)
    })
    .await;
    let value = hearts
        .values
        .0
        .iter()
        .find(|v| v.index == data_index::ABSORPTION)
        .unwrap();
    assert_eq!(value.value, EntityData::Float(4.0));
}

#[tokio::test]
async fn potions_are_drunk_and_leave_their_bottle() {
    let data = GameData::load().unwrap();
    let speed = effect(&data, "minecraft:speed");
    let bottle = data.item_id("minecraft:glass_bottle").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    survival(&mut alex).await;
    run(
        &mut alex,
        r#"give @s potion[potion_contents={potion:"minecraft:swiftness"}]"#,
    )
    .await;
    // The potion in hand, as the client sees it.
    let potion = read_matching(&mut alex, |s: &ContainerSetSlot| s.stack.0.is_some()).await;
    let stack = potion.stack.0.unwrap();
    let contents = ferrocraft::stack::get(&data, &stack, "minecraft:potion_contents").unwrap();
    assert_eq!(
        contents.get("potion"),
        Some(&Nbt::String("minecraft:swiftness".into()))
    );
    use_item(&mut alex, 1).await;
    let drunk = tokio::time::timeout(
        Duration::from_secs(4),
        read_matching(&mut alex, |e: &UpdateMobEffect| e.effect == speed),
    )
    .await
    .expect("swift");
    assert_eq!(drunk.duration, 3600);
    read_matching(&mut alex, |s: &ContainerSetSlot| {
        s.stack.0.as_ref().is_some_and(|s| s.item == bottle)
    })
    .await;
}

#[tokio::test]
async fn splash_potions_hurt_those_close_by() {
    let data = GameData::load().unwrap();
    let splash_type = data.entity_type("minecraft:splash_potion").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    let (mut steve, _) = join(addr, "Steve", 2).await;
    survival(&mut steve).await;
    walk(&mut steve, [1.5, -60.0, 0.5], None).await;
    run(
        &mut alex,
        r#"give @s splash_potion[potion_contents={potion:"minecraft:harming"}]"#,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    // Thrown at the feet, it bursts at once; Steve a block away takes most
    // of six points.
    use_item(&mut alex, 1).await;
    read_matching(&mut alex, |a: &AddEntity| a.entity_type == splash_type).await;
    let hurt = tokio::time::timeout(
        Duration::from_secs(3),
        read_matching(&mut steve, |h: &SetHealth| h.health < 20.0),
    )
    .await
    .expect("harmed");
    assert!((14.0..18.0).contains(&hurt.health), "{}", hurt.health);
}

#[tokio::test]
async fn lingering_potions_leave_a_cloud_that_gives_effects() {
    let data = GameData::load().unwrap();
    let cloud_type = data.entity_type("minecraft:area_effect_cloud").unwrap();
    let night_vision = effect(&data, "minecraft:night_vision");
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    run(
        &mut alex,
        r#"give @s lingering_potion[potion_contents={potion:"minecraft:night_vision"}]"#,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    use_item(&mut alex, 1).await;
    tokio::time::timeout(
        Duration::from_secs(3),
        read_matching(&mut alex, |a: &AddEntity| a.entity_type == cloud_type),
    )
    .await
    .expect("a cloud");
    // Standing in it after its half second of waiting: a quarter of the
    // potion's three minutes.
    let given = tokio::time::timeout(
        Duration::from_secs(3),
        read_matching(&mut alex, |e: &UpdateMobEffect| e.effect == night_vision),
    )
    .await
    .expect("night vision from the cloud");
    assert!((800..=900).contains(&given.duration), "{}", given.duration);
}
