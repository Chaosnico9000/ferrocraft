//! Furnaces: fuel burns, the block glows, the item cooks, taking the
//! result out pays experience.

mod common;

use std::time::Duration;

use common::{click, hold, join, read_matching, start, test_config};
use ferrocraft::data::GameData;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    BlockUpdate, ComponentPatch, ContainerClick, ContainerSetContent, ContainerSetData, ItemStack,
    OpenScreen, SetCreativeModeSlot, SetExperience, UntrustedSlot, container_input,
};

#[tokio::test]
async fn a_blast_furnace_smelts_and_pays_experience() {
    let data = GameData::load().unwrap();
    let id = |name: &str| data.item_id(name).unwrap();
    let blast_furnace = data.block_named("minecraft:blast_furnace").unwrap();
    let addr = start(test_config()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;

    // A blast furnace at (2, -60, 0).
    hold(&mut alex, id("minecraft:blast_furnace")).await;
    let pos = BlockPos { x: 2, y: -60, z: 0 };
    alex.send(&click(BlockPos { y: -61, ..pos }, 1, 1)).await;
    read_matching(&mut alex, |u: &BlockUpdate| u.pos == pos).await;

    // Two raw iron and a coal in the hotbar, moved in with shift clicks.
    for (slot, item, count) in [(36, "minecraft:raw_iron", 2), (37, "minecraft:coal", 1)] {
        alex.send(&SetCreativeModeSlot {
            slot,
            stack: UntrustedSlot(Some(ItemStack {
                item: id(item),
                count,
                patch: ComponentPatch::default(),
            })),
        })
        .await;
    }
    alex.send(&click(pos, 1, 2)).await;
    let screen: OpenScreen = alex.read_until().await;
    assert_eq!(screen.menu, data.menu("minecraft:blast_furnace").unwrap());
    let content: ContainerSetContent = alex.read_until().await;
    assert_eq!(content.items.0.len(), 3 + 36);
    let shift = |slot| ContainerClick {
        container_id: screen.container_id,
        state_id: 0,
        slot,
        button: 0,
        input: container_input::QUICK_MOVE,
        changed: Vec::new(),
        carried: None,
    };
    // The hotbar starts at 30: ore on top, coal below.
    alex.send(&shift(30)).await;
    alex.send(&shift(31)).await;
    // It lights up and shows its fire and how long an ore takes.
    read_matching(&mut alex, |u: &BlockUpdate| {
        u.pos == pos && blast_furnace.value(u.state as u32, "lit") == Some("true")
    })
    .await;
    // The coal lasts 800 ticks here, an ore takes 100.
    read_matching(&mut alex, |d: &ContainerSetData| {
        d.id == 1 && d.value == 800
    })
    .await;
    read_matching(&mut alex, |d: &ContainerSetData| {
        d.id == 3 && d.value == 100
    })
    .await;

    // Twice as fast as a furnace: two ingots in ten seconds.
    let done = tokio::time::timeout(
        Duration::from_secs(15),
        read_matching(&mut alex, |c: &ContainerSetContent| {
            c.items.0[2].0.as_ref().is_some_and(|s| s.count == 2)
        }),
    )
    .await
    .expect("nothing cooked");
    assert_eq!(
        done.items.0[2].0.as_ref().unwrap().item,
        id("minecraft:iron_ingot")
    );
    assert!(done.items.0[0].0.is_none(), "the ore is used up");

    // Taking the ingots: 0.7 experience each, at least one point.
    alex.send(&shift(2)).await;
    let content = read_matching(&mut alex, |c: &ContainerSetContent| {
        c.items.0[2].0.is_none()
    })
    .await;
    assert!(content.items.0[3..].iter().any(|s| {
        s.0.as_ref()
            .is_some_and(|s| s.item == id("minecraft:iron_ingot") && s.count == 2)
    }));
    let experience = tokio::time::timeout(
        Duration::from_secs(5),
        read_matching(&mut alex, |e: &SetExperience| e.total > 0),
    )
    .await
    .expect("no experience");
    assert!((1..=2).contains(&experience.total));
}
