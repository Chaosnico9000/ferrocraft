//! Screens: crafting in the inventory, chests seen by two players, signs,
//! the middle click.

mod common;

use common::{click, hold, hold_some, join, read_matching, start, test_config};
use ferrocraft::data::GameData;
use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    AddEntity, BlockEntityData, BlockEvent, BlockUpdate, ContainerClick, ContainerSetContent,
    ContainerSetSlot, OpenScreen, OpenSignEditor, PickItemFromBlock, PlayerAction,
    ServerboundContainerClose, SetHeldSlot, SignUpdate, container_input, player_action,
};

fn pickup(container_id: i32, slot: i16, button: i8) -> ContainerClick {
    ContainerClick {
        container_id,
        state_id: 0,
        slot,
        button,
        input: container_input::PICKUP,
        changed: Vec::new(),
        carried: None,
    }
}

#[tokio::test]
async fn the_inventory_grid_crafts() {
    let data = GameData::load().unwrap();
    let log = data.item_id("minecraft:oak_log").unwrap();
    let planks = data.item_id("minecraft:oak_planks").unwrap();
    let addr = start(test_config()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    hold(&mut alex, log).await;

    // Take the logs, put one into the grid, take the result.
    alex.send(&pickup(0, 36, 0)).await;
    let content = read_matching(&mut alex, |c: &ContainerSetContent| c.carried.0.is_some()).await;
    assert_eq!(content.carried.0.as_ref().unwrap().count, 64);
    alex.send(&pickup(0, 1, 1)).await;
    let content: ContainerSetContent = alex.read_until().await;
    assert_eq!(
        content.items.0[1].0.as_ref().unwrap().count,
        1,
        "one log in the grid"
    );
    let result = content.items.0[0].0.as_ref().unwrap();
    assert_eq!((result.item, result.count), (planks, 4));
    assert_eq!(content.carried.0.as_ref().unwrap().count, 63);

    // Put the logs back first, then take the planks.
    alex.send(&pickup(0, 36, 0)).await;
    alex.read_until::<ContainerSetContent>().await;
    alex.send(&pickup(0, 0, 0)).await;
    let content: ContainerSetContent = alex.read_until().await;
    let carried = content.carried.0.unwrap();
    assert_eq!((carried.item, carried.count), (planks, 4));
    assert!(content.items.0[1].0.is_none(), "the log is used up");
    assert!(content.items.0[0].0.is_none(), "nothing more to craft");

    // Closing the inventory puts the planks into it.
    alex.send(&ServerboundContainerClose { container_id: 0 })
        .await;
    let content: ContainerSetContent = alex.read_until().await;
    assert!(content.carried.0.is_none());
    assert!(content.items.0.iter().any(|s| {
        s.0.as_ref()
            .is_some_and(|s| s.item == planks && s.count == 4)
    }));
}

#[tokio::test]
async fn chests_open_for_two_and_drop_their_content() {
    let data = GameData::load().unwrap();
    let chest = data.item_id("minecraft:chest").unwrap();
    let diamond = data.item_id("minecraft:diamond").unwrap();
    let item_type = data.entity_type("minecraft:item").unwrap();
    let generic_9x3 = data.menu("minecraft:generic_9x3").unwrap();
    let generic_9x6 = data.menu("minecraft:generic_9x6").unwrap();
    let addr = start(test_config()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    let (mut steve, _) = join(addr, "Steve", 2).await;

    // A chest at (2, -60, 0), on the grass.
    hold(&mut alex, chest).await;
    let pos = BlockPos { x: 2, y: -60, z: 0 };
    alex.send(&click(BlockPos { y: -61, ..pos }, 1, 1)).await;
    read_matching(&mut steve, |u: &BlockUpdate| u.pos == pos).await;

    // Alex opens it and puts a diamond into slot 13.
    hold(&mut alex, diamond).await;
    alex.send(&click(pos, 1, 2)).await;
    let screen: OpenScreen = alex.read_until().await;
    assert_eq!(screen.menu, generic_9x3);
    let lid = read_matching(&mut steve, |e: &BlockEvent| e.pos == pos).await;
    assert_eq!(
        (lid.event, lid.param),
        (1, 1),
        "the lid opens for one viewer"
    );
    let content: ContainerSetContent = alex.read_until().await;
    assert_eq!(content.items.0.len(), 27 + 36);
    // The hotbar's first slot comes after the chest and the main inventory.
    alex.send(&pickup(screen.container_id, 27 + 27, 1)).await;
    let content = read_matching(&mut alex, |c: &ContainerSetContent| c.carried.0.is_some()).await;
    assert_eq!(content.carried.0.unwrap().count, 32);
    alex.send(&pickup(screen.container_id, 13, 0)).await;
    let content: ContainerSetContent = alex.read_until().await;
    assert_eq!(content.items.0[13].0.as_ref().unwrap().count, 32);

    // Steve looks inside too and sees it; the lid counts two.
    steve.send(&click(pos, 1, 1)).await;
    let screen_steve: OpenScreen = steve.read_until().await;
    let content: ContainerSetContent = steve.read_until().await;
    assert_eq!(content.items.0[13].0.as_ref().unwrap().item, diamond);
    let lid = read_matching(&mut alex, |e: &BlockEvent| e.pos == pos).await;
    assert_eq!(lid.param, 2);

    // Steve takes half; Alex's screen follows.
    steve.send(&pickup(screen_steve.container_id, 13, 1)).await;
    let content = read_matching(&mut alex, |c: &ContainerSetContent| {
        c.items.0[13].0.as_ref().is_some_and(|s| s.count == 16)
    })
    .await;
    assert!(content.carried.0.is_none(), "Alex's hand stays empty");
    steve
        .send(&ServerboundContainerClose {
            container_id: screen_steve.container_id,
        })
        .await;
    alex.send(&ServerboundContainerClose {
        container_id: screen.container_id,
    })
    .await;
    let lid = read_matching(&mut steve, |e: &BlockEvent| e.pos == pos && e.param == 0).await;
    assert_eq!(lid.event, 1);

    // A second chest next to it makes a double chest.
    hold(&mut alex, chest).await;
    let east = BlockPos { x: 3, ..pos };
    alex.send(&click(BlockPos { y: -61, ..east }, 1, 3)).await;
    read_matching(&mut steve, |u: &BlockUpdate| u.pos == east).await;
    alex.send(&click(east, 1, 4)).await;
    let screen: OpenScreen = alex.read_until().await;
    assert_eq!(screen.menu, generic_9x6);
    let content: ContainerSetContent = alex.read_until().await;
    assert_eq!(content.items.0.len(), 54 + 36);
    assert_eq!(
        content
            .items
            .0
            .iter()
            .filter(|s| s.0.as_ref().is_some_and(|s| s.item == diamond))
            .count(),
        1
    );

    // Breaking the first chest throws out the 16 diamonds; Alex's screen
    // closes.
    alex.send(&PlayerAction {
        action: player_action::START_DESTROY_BLOCK,
        pos,
        face: 1,
        sequence: 5,
    })
    .await;
    let dropped = read_matching(&mut steve, |a: &AddEntity| a.entity_type == item_type).await;
    assert!((dropped.pos[0] - 2.5).abs() < 0.5);
    read_matching(
        &mut alex,
        |c: &ferrocraft_protocol::packets::play::ClientboundContainerClose| {
            c.container_id == screen.container_id
        },
    )
    .await;
}

#[tokio::test]
async fn signs_are_written_for_everyone() {
    let data = GameData::load().unwrap();
    let sign = data.item_id("minecraft:oak_sign").unwrap();
    let addr = start(test_config()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    let (mut steve, _) = join(addr, "Steve", 2).await;
    hold_some(&mut alex, sign, 16).await;
    let pos = BlockPos { x: 1, y: -60, z: 1 };
    alex.send(&click(BlockPos { y: -61, ..pos }, 1, 1)).await;
    let editor: OpenSignEditor = alex.read_until().await;
    assert_eq!((editor.pos, editor.side), (pos, 1));
    alex.send(&SignUpdate {
        pos,
        lines: [
            "Hallo".into(),
            "§cWelt".into(),
            String::new(),
            String::new(),
        ],
        side: 1,
    })
    .await;
    let text = read_matching(&mut steve, |d: &BlockEntityData| d.pos == pos).await;
    let Some(Nbt::List(messages)) = text.tag.get("front_text").and_then(|t| t.get("messages"))
    else {
        panic!("{text:?}");
    };
    assert_eq!(messages[0], Nbt::String("Hallo".into()));
    assert_eq!(
        messages[1],
        Nbt::String("Welt".into()),
        "formatting codes are stripped"
    );
}

#[tokio::test]
async fn the_middle_click_picks_blocks() {
    let data = GameData::load().unwrap();
    let grass = data.item_id("minecraft:grass_block").unwrap();
    let addr = start(test_config()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;

    // Creative: not in the inventory, so it appears in the hand.
    alex.send(&PickItemFromBlock {
        pos: BlockPos { x: 0, y: -61, z: 1 },
        include_data: false,
    })
    .await;
    let held: SetHeldSlot = alex.read_until().await;
    assert_eq!(held.slot, 0);
    let slot = read_matching(&mut alex, |s: &ContainerSetSlot| s.slot == 36).await;
    assert_eq!(slot.stack.0.unwrap().item, grass);
}

#[tokio::test]
async fn doors_and_levers_toggle() {
    let data = GameData::load().unwrap();
    let door = data.item_id("minecraft:oak_door").unwrap();
    let lever = data.item_id("minecraft:lever").unwrap();
    let door_block = data.block_named("minecraft:oak_door").unwrap();
    let lever_block = data.block_named("minecraft:lever").unwrap();
    let addr = start(test_config()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    let (mut steve, _) = join(addr, "Steve", 2).await;

    hold(&mut alex, door).await;
    let lower = BlockPos { x: 2, y: -60, z: 2 };
    let upper = BlockPos { y: -59, ..lower };
    alex.send(&click(BlockPos { y: -61, ..lower }, 1, 1)).await;
    read_matching(&mut steve, |u: &BlockUpdate| u.pos == upper).await;
    // Both halves open.
    alex.send(&click(lower, 2, 2)).await;
    for pos in [lower, upper] {
        let update = read_matching(&mut steve, |u: &BlockUpdate| u.pos == pos).await;
        assert_eq!(door_block.value(update.state as u32, "open"), Some("true"));
    }

    hold(&mut alex, lever).await;
    let at = BlockPos {
        x: -2,
        y: -60,
        z: 2,
    };
    alex.send(&click(BlockPos { y: -61, ..at }, 1, 3)).await;
    read_matching(&mut steve, |u: &BlockUpdate| u.pos == at).await;
    alex.send(&click(at, 1, 4)).await;
    let update = read_matching(&mut steve, |u: &BlockUpdate| u.pos == at).await;
    assert_eq!(
        lever_block.value(update.state as u32, "powered"),
        Some("true")
    );
}
