//! Monsters: zombies hunt and hit, skeletons shoot, creepers blow up,
//! the sun burns the undead, the dark brings monsters out; experience from
//! killing them.

mod common;

use std::time::Duration;

use common::{Client, hold_some, join, read_matching, start, test_config};
use ferrocraft::Config;
use ferrocraft::data::GameData;
use ferrocraft_protocol::packets::Packet;
use ferrocraft_protocol::packets::play::{
    AddEntity, Attack, BlockUpdate, ChatCommand, DamageEvent, EntityData, Explode, GameEvent,
    RemoveEntities, SetEntityData, SetEquipment, SetExperience, SetHealth, data_index,
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
async fn zombies_hunt_and_hit_players_in_survival() {
    let data = GameData::load().unwrap();
    let (zombie_type, _) = data.entity("minecraft:zombie").unwrap();
    let addr = start(operators()).await;
    let (mut alex, alex_login) = join(addr, "Alex", 4).await;
    // At night, so that it does not burn on the way.
    run(&mut alex, "time set midnight").await;
    survival(&mut alex).await;
    run(&mut alex, "summon zombie 6 -60 0").await;
    let zombie = read_matching(&mut alex, |a: &AddEntity| a.entity_type == zombie_type).await;
    // It sees Alex, raises its arms and comes for them.
    read_matching(&mut alex, |d: &SetEntityData| {
        d.entity_id == zombie.entity_id
            && d.values
                .0
                .iter()
                .any(|v| v.index == 15 && v.value == EntityData::Byte(0x04))
    })
    .await;
    let hit = tokio::time::timeout(
        Duration::from_secs(15),
        read_matching(&mut alex, |d: &DamageEvent| {
            d.entity_id == alex_login.entity_id
        }),
    )
    .await
    .expect("the zombie never hit");
    assert_eq!(hit.cause, Some(zombie.entity_id));
    // Three points on normal, no armor.
    let health = read_matching(&mut alex, |h: &SetHealth| h.health < 20.0).await;
    assert_eq!(health.health, 17.0);
}

#[tokio::test]
async fn skeletons_shoot_arrows() {
    let data = GameData::load().unwrap();
    let (skeleton_type, _) = data.entity("minecraft:skeleton").unwrap();
    let arrow_type = data.entity_type("minecraft:arrow").unwrap();
    let bow = data.item_id("minecraft:bow").unwrap();
    let addr = start(operators()).await;
    let (mut alex, alex_login) = join(addr, "Alex", 4).await;
    run(&mut alex, "time set midnight").await;
    // On hard they aim best.
    run(&mut alex, "difficulty hard").await;
    survival(&mut alex).await;
    run(&mut alex, "summon skeleton 0.5 -60 8.5").await;
    let skeleton = read_matching(&mut alex, |a: &AddEntity| a.entity_type == skeleton_type).await;
    let gear = read_matching(&mut alex, |e: &SetEquipment| {
        e.entity_id == skeleton.entity_id
    })
    .await;
    assert!(
        gear.slots
            .iter()
            .any(|(_, s)| s.as_ref().is_some_and(|s| s.item == bow))
    );
    let arrow = tokio::time::timeout(
        Duration::from_secs(10),
        read_matching(&mut alex, |a: &AddEntity| a.entity_type == arrow_type),
    )
    .await
    .expect("no arrow");
    assert_eq!(arrow.data, skeleton.entity_id, "the shooter");
    let hit = tokio::time::timeout(
        Duration::from_secs(20),
        read_matching(&mut alex, |d: &DamageEvent| {
            d.entity_id == alex_login.entity_id
        }),
    )
    .await
    .expect("no arrow hit");
    assert_eq!(hit.cause, Some(skeleton.entity_id));
    let arrow_damage = data
        .registry_id("minecraft:damage_type", "minecraft:arrow")
        .unwrap();
    assert_eq!(hit.source_type, arrow_damage);
}

#[tokio::test]
async fn creepers_blow_up_next_to_players() {
    let data = GameData::load().unwrap();
    let (creeper_type, _) = data.entity("minecraft:creeper").unwrap();
    let air = data.default_state("minecraft:air").unwrap();
    let addr = start(operators()).await;
    let (mut alex, alex_login) = join(addr, "Alex", 4).await;
    run(&mut alex, "time set midnight").await;
    survival(&mut alex).await;
    run(&mut alex, "summon creeper 2 -60 0.5").await;
    let creeper = read_matching(&mut alex, |a: &AddEntity| a.entity_type == creeper_type).await;
    // It hisses (swell direction 1), then blows up.
    read_matching(&mut alex, |d: &SetEntityData| {
        d.entity_id == creeper.entity_id
            && d.values
                .0
                .iter()
                .any(|v| v.index == 16 && v.value == EntityData::Int(1))
    })
    .await;
    // The creeper goes, Alex is hurt, the grass under it is gone, then
    // the blast shows.
    let (mut removed, mut hurt, mut holes) = (false, None, 0);
    let blast = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let raw = alex.read_raw().await.unwrap();
            match raw.id {
                RemoveEntities::ID => {
                    let r = RemoveEntities::from_raw(&raw).unwrap();
                    removed |= r.entity_ids.0.contains(&creeper.entity_id);
                }
                DamageEvent::ID => {
                    let d = DamageEvent::from_raw(&raw).unwrap();
                    if d.entity_id == alex_login.entity_id {
                        hurt = Some(d);
                    }
                }
                BlockUpdate::ID => {
                    let u = BlockUpdate::from_raw(&raw).unwrap();
                    if u.state as u32 == air {
                        holes += 1;
                    }
                }
                Explode::ID => break Explode::from_raw(&raw).unwrap(),
                _ => {}
            }
        }
    })
    .await
    .expect("no explosion");
    assert_eq!(blast.radius, 3.0);
    assert!(blast.block_count > 0 && holes > 0);
    assert!(blast.knockback.is_some(), "Alex is thrown");
    assert!(removed, "the creeper is gone with the blast");
    assert_eq!(hurt.expect("Alex is hurt").cause, Some(creeper.entity_id));
}

#[tokio::test]
async fn zombies_burn_in_the_sun() {
    let data = GameData::load().unwrap();
    let (zombie_type, _) = data.entity("minecraft:zombie").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    run(&mut alex, "time set noon").await;
    run(&mut alex, "summon zombie 10 -60 10").await;
    let zombie = read_matching(&mut alex, |a: &AddEntity| a.entity_type == zombie_type).await;
    tokio::time::timeout(
        Duration::from_secs(15),
        read_matching(&mut alex, |d: &SetEntityData| {
            d.entity_id == zombie.entity_id
                && d.values.0.iter().any(|v| {
                    v.index == data_index::SHARED_FLAGS && v.value == EntityData::Byte(0x01)
                })
        }),
    )
    .await
    .expect("the zombie never caught fire");
}

#[tokio::test]
async fn monsters_come_out_at_night_and_go_in_peaceful() {
    let data = GameData::load().unwrap();
    let hostile: Vec<i32> = [
        "minecraft:zombie",
        "minecraft:skeleton",
        "minecraft:creeper",
        "minecraft:spider",
    ]
    .iter()
    .map(|n| data.entity_type(n).unwrap())
    .collect();
    let addr = start(Config {
        monsters: true,
        ..operators()
    })
    .await;
    let (mut alex, _) = join(addr, "Alex", 8).await;
    run(&mut alex, "time set midnight").await;
    let monster = tokio::time::timeout(
        Duration::from_secs(20),
        read_matching(&mut alex, |a: &AddEntity| hostile.contains(&a.entity_type)),
    )
    .await
    .expect("no monster spawned at night");
    run(&mut alex, "difficulty peaceful").await;
    read_matching(&mut alex, |r: &RemoveEntities| {
        r.entity_ids.0.contains(&monster.entity_id)
    })
    .await;
}

#[tokio::test]
async fn killing_a_monster_gives_experience() {
    let data = GameData::load().unwrap();
    let (spider_type, _) = data.entity("minecraft:spider").unwrap();
    let orb_type = data.entity_type("minecraft:experience_orb").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    // By day the spider leaves Alex alone.
    run(&mut alex, "summon spider 1.5 -60 0.5").await;
    let spider = read_matching(&mut alex, |a: &AddEntity| a.entity_type == spider_type).await;
    hold_some(
        &mut alex,
        data.item_id("minecraft:netherite_sword").unwrap(),
        1,
    )
    .await;
    // 8 damage a full swing: two for 16 health.
    for _ in 0..2 {
        tokio::time::sleep(Duration::from_millis(700)).await;
        alex.send(&Attack {
            entity_id: spider.entity_id,
        })
        .await;
    }
    let orb = read_matching(&mut alex, |a: &AddEntity| a.entity_type == orb_type).await;
    let _ = orb;
    // The orbs fly to Alex: five points, level 0 needs seven.
    let mut experience: SetExperience = alex.read_until().await;
    while experience.total < 5 {
        experience = alex.read_until().await;
    }
    assert_eq!((experience.level, experience.total), (0, 5));
    assert!((experience.progress - 5.0 / 7.0).abs() < 1e-6);
}

#[tokio::test]
async fn experience_command() {
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    run(&mut alex, "xp add @s 10").await;
    let experience = read_matching(&mut alex, |e: &SetExperience| e.total > 0).await;
    // Seven points to level 1, three of nine towards level 2.
    assert_eq!((experience.level, experience.total), (1, 10));
    assert!((experience.progress - 3.0 / 9.0).abs() < 1e-6);
    run(&mut alex, "xp add @s 4 levels").await;
    let experience = read_matching(&mut alex, |e: &SetExperience| e.level == 5).await;
    assert_eq!(experience.total, 10);
}
