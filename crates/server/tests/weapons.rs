//! Bows and TNT: arrows from a drawn bow hit, use up ammunition and can be
//! picked up again; TNT lit with flint and steel blows up and sets off its
//! neighbours.

mod common;

use std::time::Duration;

use common::{Client, click, hold, hold_some, join, read_matching, start, test_config, walk};
use ferrocraft::Config;
use ferrocraft::data::GameData;
use ferrocraft_protocol::codec::{BlockPos, Decode, VarInt};
use ferrocraft_protocol::packets::Packet;
use ferrocraft_protocol::packets::play::{
    AddEntity, BlockUpdate, ChatCommand, ComponentPatch, ContainerSetSlot, DamageEvent, EntityData,
    Explode, GameEvent, ItemStack, PlayerAction, SetCreativeModeSlot, SetEntityData,
    TakeItemEntity, UntrustedSlot, UseItem, player_action,
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

/// Draws the bow in hand for `millis` and lets go, looking `pitch` down.
async fn shoot(client: &mut Client, millis: u64, sequence: i32, pitch: f32) {
    client
        .send(&UseItem {
            hand: 0,
            sequence,
            yaw: 0.0,
            pitch,
        })
        .await;
    tokio::time::sleep(Duration::from_millis(millis)).await;
    client
        .send(&PlayerAction {
            action: player_action::RELEASE_USE_ITEM,
            pos: BlockPos { x: 0, y: 0, z: 0 },
            face: 0,
            sequence: 0,
        })
        .await;
}

#[tokio::test]
async fn a_drawn_bow_shoots_hits_and_uses_up_arrows() {
    let data = GameData::load().unwrap();
    let id = |name: &str| data.item_id(name).unwrap();
    let arrow_type = data.entity_type("minecraft:arrow").unwrap();
    let addr = start(operators()).await;
    let (mut alex, alex_login) = join(addr, "Alex", 4).await;
    // The target: Steve, six blocks ahead in survival. A cow would stroll
    // off now and then before the arrow arrives.
    let (mut steve, steve_login) = join(addr, "Steve", 4).await;
    walk(&mut steve, [0.5, -60.0, 6.5], None).await;
    // A bow and three arrows, then survival, where both are used up.
    hold_some(&mut alex, id("minecraft:bow"), 1).await;
    alex.send(&SetCreativeModeSlot {
        slot: 20,
        stack: UntrustedSlot(Some(ItemStack {
            item: id("minecraft:arrow"),
            count: 3,
            patch: ComponentPatch::default(),
        })),
    })
    .await;
    run(&mut alex, "gamemode survival").await;
    read_matching(&mut alex, |e: &GameEvent| e.event == 3).await;
    run(&mut alex, "gamemode survival Steve").await;
    read_matching(&mut steve, |e: &GameEvent| e.event == 3).await;
    // Alex looks a little down at Steve.
    walk(&mut alex, [0.5, -60.0, 0.5], Some([0.0, 10.0])).await;

    shoot(&mut alex, 1100, 1, 10.0).await;
    let arrow = read_matching(&mut alex, |a: &AddEntity| a.entity_type == arrow_type).await;
    assert_eq!(arrow.data, alex_login.entity_id, "Alex's arrow");
    // One arrow less, and the bow wears.
    let ammo = read_matching(&mut alex, |s: &ContainerSetSlot| s.slot == 20).await;
    assert_eq!(ammo.stack.0.unwrap().count, 2);
    // The worn bow carries a `damage` component, which the test client
    // cannot decode: its slot number is enough.
    loop {
        let raw = alex.read_raw().await.unwrap();
        if raw.id == ContainerSetSlot::ID {
            let mut body = raw.body.as_slice();
            let _container = VarInt::decode(&mut body).unwrap();
            let _state = VarInt::decode(&mut body).unwrap();
            if i16::decode(&mut body).unwrap() == 36 {
                break;
            }
        }
    }
    let hit = read_matching(&mut alex, |d: &DamageEvent| {
        d.entity_id == steve_login.entity_id
    })
    .await;
    assert_eq!(hit.cause, Some(alex_login.entity_id));
    assert_eq!(hit.direct, Some(arrow.entity_id));

    // Shot steeply down at Alex's feet, the arrow sticks and comes back.
    walk(&mut alex, [0.5, -60.0, 0.5], Some([0.0, 75.0])).await;
    shoot(&mut alex, 1100, 2, 75.0).await;
    let arrow = read_matching(&mut alex, |a: &AddEntity| a.entity_type == arrow_type).await;
    read_matching(&mut alex, |d: &SetEntityData| {
        d.entity_id == arrow.entity_id
            && d.values
                .0
                .iter()
                .any(|v| v.index == 10 && v.value == EntityData::Boolean(true))
    })
    .await;
    let taken = tokio::time::timeout(
        Duration::from_secs(5),
        read_matching(&mut alex, |t: &TakeItemEntity| t.item_id == arrow.entity_id),
    )
    .await
    .expect("the arrow was not picked up");
    assert_eq!(taken.player_id, alex_login.entity_id);

    // A tap does not shoot.
    shoot(&mut alex, 50, 3, 75.0).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    run(&mut alex, "list").await;
    let late = tokio::time::timeout(
        Duration::from_millis(500),
        read_matching(&mut alex, |a: &AddEntity| a.entity_type == arrow_type),
    )
    .await;
    assert!(late.is_err(), "no arrow from a tap");
}

#[tokio::test]
async fn tnt_blows_up_and_sets_off_its_neighbour() {
    let data = GameData::load().unwrap();
    let id = |name: &str| data.item_id(name).unwrap();
    let tnt_type = data.entity_type("minecraft:tnt").unwrap();
    let item_type = data.entity_type("minecraft:item").unwrap();
    let air = data.default_state("minecraft:air").unwrap();
    let addr = start(operators()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;

    // Two TNT next to each other, four blocks off.
    hold(&mut alex, id("minecraft:tnt")).await;
    let first = BlockPos { x: 4, y: -60, z: 0 };
    let second = BlockPos { x: 5, ..first };
    for (pos, sequence) in [(first, 1), (second, 2)] {
        alex.send(&click(BlockPos { y: -61, ..pos }, 1, sequence))
            .await;
        read_matching(&mut alex, |u: &BlockUpdate| {
            u.pos == pos && u.state as u32 != air
        })
        .await;
    }
    hold_some(&mut alex, id("minecraft:flint_and_steel"), 1).await;
    alex.send(&click(first, 1, 3)).await;
    read_matching(&mut alex, |u: &BlockUpdate| {
        u.pos == first && u.state as u32 == air
    })
    .await;
    let lit = read_matching(&mut alex, |a: &AddEntity| a.entity_type == tnt_type).await;
    assert!((lit.pos[0] - 4.5).abs() < 1e-9);

    // Four seconds later a blast of power 4, which lights the other one
    // with a short fuse; that one goes off too.
    let (mut lit_count, mut drops, mut blasts) = (1, 0, Vec::new());
    tokio::time::timeout(Duration::from_secs(8), async {
        while blasts.len() < 2 {
            let raw = alex.read_raw().await.unwrap();
            match raw.id {
                AddEntity::ID => {
                    let added = AddEntity::from_raw(&raw).unwrap();
                    if added.entity_type == tnt_type {
                        lit_count += 1;
                    } else if added.entity_type == item_type {
                        drops += 1;
                    }
                }
                Explode::ID => blasts.push(Explode::from_raw(&raw).unwrap()),
                _ => {}
            }
        }
    })
    .await
    .expect("not both went off");
    assert_eq!(lit_count, 2);
    assert_eq!(blasts[0].radius, 4.0);
    assert!(blasts[0].block_count > 10);
    // Without decay every broken block drops: lots of dirt.
    assert!(drops > 10, "{drops} drops");
}
