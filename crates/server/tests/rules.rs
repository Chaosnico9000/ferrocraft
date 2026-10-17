//! Game rules that change how the game goes: keeping the inventory on
//! death, no fighting between players, TNT that will not light, blocks
//! without drops, deaths nobody hears of, respawning at once.

mod common;

use std::time::Duration;

use common::{Client, click, hold_some, join, read_matching, start, test_config, walk};
use ferrocraft::Config;
use ferrocraft::data::GameData;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    AddEntity, Attack, BlockUpdate, ChatCommand, ClientCommand, ContainerSetContent, GameEvent,
    PlayerAction, PlayerCombatKill, SetHealth, SystemChat, player_action,
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

async fn survival(client: &mut Client) {
    run(client, "gamemode survival").await;
    read_matching(client, |e: &GameEvent| e.event == 3).await;
}

#[tokio::test]
async fn keep_inventory_keeps_the_inventory() {
    let data = GameData::load().unwrap();
    let diamond = data.item_id("minecraft:diamond").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    run(&mut alex, "gamerule keep_inventory true").await;
    run(&mut alex, "give @s diamond 5").await;
    survival(&mut alex).await;
    run(&mut alex, "kill @s").await;
    read_matching(&mut alex, |_: &PlayerCombatKill| true).await;
    alex.send(&ClientCommand { action: 0 }).await;
    // After respawning, the inventory comes back with the diamonds.
    let content = read_matching(&mut alex, |c: &ContainerSetContent| c.container_id == 0).await;
    assert!(content.items.0.iter().any(|s| {
        s.0.as_ref()
            .is_some_and(|s| s.item == diamond && s.count == 5)
    }));
}

#[tokio::test]
async fn without_pvp_players_do_not_hurt_each_other() {
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    let (mut steve, steve_login) = join(addr, "Steve", 2).await;
    run(&mut alex, "gamerule pvp false").await;
    survival(&mut steve).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    alex.send(&Attack {
        entity_id: steve_login.entity_id,
    })
    .await;
    let hurt = tokio::time::timeout(
        Duration::from_millis(800),
        read_matching(&mut steve, |h: &SetHealth| h.health < 20.0),
    )
    .await;
    assert!(hurt.is_err(), "no harm done");
}

#[tokio::test]
async fn tnt_that_may_not_explode_stays_a_block() {
    let data = GameData::load().unwrap();
    let tnt_type = data.entity_type("minecraft:tnt").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    run(&mut alex, "gamerule tnt_explodes false").await;
    run(&mut alex, "setblock 1 -60 1 tnt").await;
    hold_some(
        &mut alex,
        data.item_id("minecraft:flint_and_steel").unwrap(),
        1,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    alex.send(&click(BlockPos { x: 1, y: -60, z: 1 }, 1, 1))
        .await;
    // The player hears why, and no TNT comes alive.
    let told = read_matching(&mut alex, |c: &SystemChat| c.overlay).await;
    assert!(format!("{:?}", told.content).contains("block.minecraft.tnt.disabled"));
    let lit = tokio::time::timeout(
        Duration::from_millis(500),
        read_matching(&mut alex, |a: &AddEntity| a.entity_type == tnt_type),
    )
    .await;
    assert!(lit.is_err());
}

#[tokio::test]
async fn blocks_drop_nothing_without_block_drops() {
    let data = GameData::load().unwrap();
    let item_type = data.entity_type("minecraft:item").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    run(&mut alex, "gamerule block_drops false").await;
    survival(&mut alex).await;
    // Grass breaks in a second by hand and would drop dirt.
    let pos = BlockPos { x: 1, y: -61, z: 0 };
    for (action, sequence) in [
        (player_action::START_DESTROY_BLOCK, 1),
        (player_action::STOP_DESTROY_BLOCK, 2),
    ] {
        alex.send(&PlayerAction {
            action,
            pos,
            face: 1,
            sequence,
        })
        .await;
        tokio::time::sleep(Duration::from_millis(1000)).await;
    }
    read_matching(&mut alex, |u: &BlockUpdate| u.pos == pos && u.state == 0).await;
    let dropped = tokio::time::timeout(
        Duration::from_millis(500),
        read_matching(&mut alex, |a: &AddEntity| a.entity_type == item_type),
    )
    .await;
    assert!(dropped.is_err(), "no dirt");
}

#[tokio::test]
async fn deaths_go_unheard_and_the_respawn_is_at_once() {
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    let (mut steve, _) = join(addr, "Steve", 2).await;
    run(&mut alex, "gamerule show_death_messages false").await;
    run(&mut alex, "gamerule immediate_respawn true").await;
    // Everyone's client learns that there is no death screen any more.
    read_matching(&mut steve, |e: &GameEvent| e.event == 11 && e.param == 1.0).await;
    survival(&mut steve).await;
    walk(&mut steve, [2.5, -60.0, 0.5], None).await;
    run(&mut steve, "kill @s").await;
    let death = read_matching(&mut steve, |_: &PlayerCombatKill| true).await;
    assert!(!format!("{:?}", death.message).contains("death."));
    let heard = tokio::time::timeout(
        Duration::from_millis(500),
        read_matching(&mut alex, |c: &SystemChat| {
            format!("{:?}", c.content).contains("death.")
        }),
    )
    .await;
    assert!(heard.is_err(), "nobody hears of it");
}
