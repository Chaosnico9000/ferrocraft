//! Animals: summoned, seen walking, hit, killed for their loot, shorn; and
//! players hitting each other.

mod common;

use std::time::Duration;

use common::{hold_some, join, read_matching, start, test_config};
use ferrocraft::Config;
use ferrocraft::data::GameData;
use ferrocraft_protocol::packets::Packet;
use ferrocraft_protocol::packets::play::{
    AddEntity, Attack, ChatCommand, DamageEvent, EntityData, EntityEvent, EntityPositionSync,
    GameEvent, Interact, LpVec3, MoveEntityPos, MoveEntityPosRot, RemoveEntities, SetEntityData,
    SetEntityData as Data, SetEntityMotion, SetHealth, data_index,
};

fn operators() -> Config {
    Config {
        operators: vec!["Alex".into(), "Steve".into()],
        ..test_config()
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
async fn cows_walk_bleed_and_drop_beef() {
    let data = GameData::load().unwrap();
    let (cow_type, _) = data.entity("minecraft:cow").unwrap();
    let item_type = data.entity_type("minecraft:item").unwrap();
    let beef = data.item_id("minecraft:beef").unwrap();
    let addr = start(operators()).await;
    let (mut alex, alex_login) = join(addr, "Alex", 2).await;
    let (mut steve, _) = join(addr, "Steve", 2).await;

    run(&mut alex, "summon minecraft:cow 2 -60 0").await;
    let cow = read_matching(&mut steve, |a: &AddEntity| a.entity_type == cow_type).await;
    let shown = read_matching(&mut steve, |d: &SetEntityData| d.entity_id == cow.entity_id).await;
    assert!(
        shown
            .values
            .0
            .iter()
            .any(|v| v.index == data_index::HEALTH && v.value == EntityData::Float(10.0)),
        "{shown:?}"
    );

    // A diamond sword: 1 + 6 damage once fully charged. The cow runs off
    // after the first hit, so the second follows soon, charged enough for
    // the three health left.
    hold_some(
        &mut alex,
        data.item_id("minecraft:diamond_sword").unwrap(),
        1,
    )
    .await;
    for wait in [700, 350] {
        tokio::time::sleep(Duration::from_millis(wait)).await;
        alex.send(&Attack {
            entity_id: cow.entity_id,
        })
        .await;
        let hit = read_matching(&mut steve, |d: &DamageEvent| d.entity_id == cow.entity_id).await;
        assert_eq!(hit.cause, Some(alex_login.entity_id));
    }
    read_matching(&mut steve, |e: &EntityEvent| {
        e.entity_id == cow.entity_id && e.event == 3
    })
    .await;
    let drop = read_matching(&mut steve, |a: &AddEntity| a.entity_type == item_type).await;
    let _ = drop;
    let mut got_beef = false;
    for _ in 0..3 {
        let item = read_matching(&mut steve, |d: &Data| {
            d.values.0.iter().any(|v| v.index == data_index::ITEM)
        })
        .await;
        if let EntityData::Item(slot) = &item.values.0[0].value
            && slot.0.as_ref().is_some_and(|s| s.item == beef)
        {
            got_beef = true;
            break;
        }
    }
    assert!(got_beef, "a cow always drops beef");
    read_matching(&mut steve, |r: &RemoveEntities| {
        r.entity_ids.0.contains(&cow.entity_id)
    })
    .await;
}

#[tokio::test]
async fn animals_stroll_by_themselves() {
    let data = GameData::load().unwrap();
    let (pig_type, _) = data.entity("minecraft:pig").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    for _ in 0..4 {
        run(&mut alex, "summon pig ~3 ~ ~").await;
    }
    let pig = read_matching(&mut alex, |a: &AddEntity| a.entity_type == pig_type).await;
    let _ = pig;
    // Within a few seconds one of them sets off.
    let moved = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let raw = alex.read_raw().await.unwrap();
            if raw.id == MoveEntityPos::ID
                || raw.id == MoveEntityPosRot::ID
                || raw.id == EntityPositionSync::ID
            {
                break;
            }
        }
    })
    .await;
    assert!(moved.is_ok(), "no pig moved");
}

#[tokio::test]
async fn sheep_can_be_shorn() {
    let data = GameData::load().unwrap();
    let (sheep_type, _) = data.entity("minecraft:sheep").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    run(&mut alex, "summon sheep 2 -60 0").await;
    let sheep = read_matching(&mut alex, |a: &AddEntity| a.entity_type == sheep_type).await;
    hold_some(&mut alex, data.item_id("minecraft:shears").unwrap(), 1).await;
    alex.send(&Interact {
        entity_id: sheep.entity_id,
        hand: 0,
        location: LpVec3([0.0, 0.5, 0.0]),
        sneaking: false,
    })
    .await;
    let shorn = read_matching(&mut alex, |d: &SetEntityData| {
        d.entity_id == sheep.entity_id
            && d.values.0.iter().any(
                |v| matches!(v.value, EntityData::Byte(wool) if v.index == 18 && wool & 0x10 != 0),
            )
    })
    .await;
    let _ = shorn;
}

#[tokio::test]
async fn players_hurt_each_other_in_survival() {
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    let (mut steve, steve_login) = join(addr, "Steve", 2).await;
    run(&mut steve, "gamemode survival").await;
    read_matching(&mut steve, |e: &GameEvent| e.event == 3).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    alex.send(&Attack {
        entity_id: steve_login.entity_id,
    })
    .await;
    // Bare hands: one point of damage, knocked back.
    let health = read_matching(&mut steve, |h: &SetHealth| h.health < 20.0).await;
    assert_eq!(health.health, 19.0);
    let push = read_matching(&mut steve, |m: &SetEntityMotion| {
        m.entity_id == steve_login.entity_id
    })
    .await;
    assert!(push.velocity.0[1] > 0.0);
}
