//! Operators, game modes and commands.

mod common;

use common::{join, read_matching, start, test_config};
use ferrocraft::Config;
use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::packets::play::{
    AddEntity, ChatCommand, ContainerSetSlot, GameEvent, PlayerAbilities, PlayerInfoGameMode,
    PlayerPosition, RemoveEntities, SetTime, SystemChat,
};

fn operators() -> Config {
    Config {
        operators: vec!["alex".into()],
        ..test_config()
    }
}

fn key(chat: &SystemChat) -> &str {
    match chat.content.get("translate") {
        Some(Nbt::String(key)) => key,
        _ => "",
    }
}

async fn run(client: &mut common::Client, command: &str) {
    client
        .send(&ChatCommand {
            command: command.into(),
        })
        .await;
}

#[tokio::test]
async fn only_operators_change_game_modes() {
    let addr = start(operators()).await;
    let (mut steve, _) = join(addr, "Steve", 2).await;
    run(&mut steve, "gamemode survival").await;
    let reply = read_matching(&mut steve, |c: &SystemChat| {
        !key(c).starts_with("multiplayer")
    })
    .await;
    assert_eq!(key(&reply), "command.unknown.command");

    // Names are matched case-insensitively.
    let (mut alex, alex_login) = join(addr, "Alex", 2).await;
    let alex_id = alex_login.entity_id;
    run(&mut alex, "gamemode spectator").await;
    let event = read_matching(&mut alex, |e: &GameEvent| e.event == 3).await;
    assert_eq!(event.param, 3.0, "spectator");
    let abilities: PlayerAbilities = alex.read_until().await;
    assert_eq!(abilities.flags, 1 | 2 | 4, "invulnerable, flying, may fly");
    let reply = read_matching(&mut alex, |c: &SystemChat| key(c).starts_with("commands")).await;
    assert_eq!(key(&reply), "commands.gamemode.success.self");

    // Steve sees the change in the tab list, and no longer sees Alex.
    let info: PlayerInfoGameMode = steve.read_until().await;
    assert_eq!(info.entries[0].1, 3);
    read_matching(&mut steve, |r: &RemoveEntities| r.entity_ids.0 == [alex_id]).await;
    run(&mut alex, "gamemode creative").await;
    read_matching(&mut steve, |a: &AddEntity| a.entity_id == alex_id).await;
}

#[tokio::test]
async fn time_teleport_and_give() {
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;

    run(&mut alex, "time set night").await;
    let time = read_matching(&mut alex, |t: &SetTime| {
        !t.clocks.is_empty() && t.clocks[0].1.total_ticks % 24000 == 13000
    })
    .await;
    assert_eq!(time.clocks[0].0, 0, "minecraft:overworld");

    run(&mut alex, "tp 10 -60 ~3").await;
    let to: PlayerPosition = alex.read_until().await;
    assert_eq!(to.pos, [10.5, -60.0, 3.5]);
    assert_eq!(to.teleport_id, 2, "a new teleport after the spawn one");

    // 70 diamonds: a full stack of 64 and 6 more.
    run(&mut alex, "give @s diamond 70").await;
    let first: ContainerSetSlot = alex.read_until().await;
    let second: ContainerSetSlot = alex.read_until().await;
    let count = |s: &ContainerSetSlot| s.stack.0.as_ref().map(|i| i.count);
    assert_eq!((first.slot, count(&first)), (36, Some(64)));
    assert_eq!((second.slot, count(&second)), (37, Some(6)));
    assert!(second.state_id > first.state_id);
    let reply = read_matching(&mut alex, |c: &SystemChat| key(c).starts_with("commands")).await;
    assert_eq!(key(&reply), "commands.give.success.single");
}

#[tokio::test]
async fn clear_takes_items_away() {
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    run(&mut alex, "give @s diamond 5").await;
    run(&mut alex, "give @s stick 2").await;
    let answer =
        |c: &SystemChat| key(c).starts_with("commands.clear") || key(c).starts_with("clear.");
    // Counting only, then two diamonds, then everything else.
    run(&mut alex, "clear @s diamond 0").await;
    let counted = read_matching(&mut alex, answer).await;
    assert_eq!(key(&counted), "commands.clear.test.single");
    run(&mut alex, "clear @s diamond 2").await;
    let cleared = read_matching(&mut alex, answer).await;
    assert_eq!(key(&cleared), "commands.clear.success.single");
    assert_eq!(
        cleared.content.get("with").and_then(|w| match w {
            Nbt::List(with) => with.first().cloned(),
            _ => None,
        }),
        Some(Nbt::text("2"))
    );
    run(&mut alex, "clear").await;
    let all = read_matching(&mut alex, answer).await;
    assert!(
        format!("{:?}", all.content).contains("\"5\""),
        "{:?}",
        all.content
    );
    run(&mut alex, "clear").await;
    let none = read_matching(&mut alex, answer).await;
    assert_eq!(key(&none), "clear.failed.single");
}
