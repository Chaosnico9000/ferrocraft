//! Item entities: thrown, seen, picked up.

mod common;

use common::{hold, join, read_matching, start, test_config, walk};
use ferrocraft::data::GameData;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    AddEntity, ContainerSetSlot, EntityData, PlayerAction, SetEntityData, TakeItemEntity,
    data_index, player_action,
};

#[tokio::test]
async fn thrown_items_are_seen_and_picked_up_again() {
    let data = GameData::load().unwrap();
    let item_type = data.entity_type("minecraft:item").unwrap();
    let stone = data.item_id("minecraft:stone").unwrap();
    let addr = start(test_config()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    let (mut steve, _) = join(addr, "Steve", 2).await;
    hold(&mut alex, stone).await;

    // Q throws one stone.
    alex.send(&PlayerAction {
        action: player_action::DROP_ITEM,
        pos: BlockPos { x: 0, y: 0, z: 0 },
        face: 0,
        sequence: 0,
    })
    .await;
    let left = read_matching(&mut alex, |s: &ContainerSetSlot| s.slot == 36).await;
    assert_eq!(left.stack.0.unwrap().count, 63);
    let thrown = read_matching(&mut steve, |a: &AddEntity| a.entity_type == item_type).await;
    let shown = read_matching(&mut steve, |d: &SetEntityData| {
        d.entity_id == thrown.entity_id
    })
    .await;
    let EntityData::Item(slot) = &shown.values.0[0].value else {
        panic!("{shown:?}");
    };
    assert_eq!(shown.values.0[0].index, data_index::ITEM);
    assert_eq!(slot.0.as_ref().unwrap().item, stone);

    // After the two seconds it cannot be picked up, Alex walks along the
    // way it flew (south, where Alex looks).
    tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
    for step in 0..12 {
        let z = thrown.pos[2] + f64::from(step) * 0.5;
        walk(&mut alex, [thrown.pos[0], -60.0, z], None).await;
        // One step per tick, as a client walks.
        tokio::time::sleep(ferrocraft::TICK).await;
    }
    let take = read_matching(&mut steve, |t: &TakeItemEntity| {
        t.item_id == thrown.entity_id
    })
    .await;
    assert_eq!(take.amount, 1);
    let back = read_matching(&mut alex, |s: &ContainerSetSlot| s.slot == 36).await;
    assert_eq!(back.stack.0.unwrap().count, 64);
}
