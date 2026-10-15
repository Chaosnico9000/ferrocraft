//! Several players at once: seeing each other, chat, blocks.

mod common;

use common::{Client, click, hold, join, read_matching, start, test_config, walk, walk_steps};
use ferrocraft::Config;
use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    AddEntity, BlockChangedAck, BlockUpdate, BundleDelimiter, Chat, ChatCommand, Disconnect,
    EntityData, EntityPositionSync, LevelChunkWithLight, LightUpdate, MoveEntityPosRot,
    PlayerAction, PlayerInfoRemove, PlayerInfoUpdate, PlayerInput, Pose, RemoveEntities,
    RotateHead, SetEntityData, SetEquipment, SystemChat, VecDelta, data_index, equipment_slot,
    player_action,
};

/// Item and block state ids from the 26.3 reports.
const STONE_ITEM: i32 = 1;
const STICK_ITEM: i32 = 1060;
const COMMAND_BLOCK_ITEM: i32 = 529;
const STONE: i32 = 1;
const AIR: i32 = 0;

fn translate_key(text: &Nbt) -> &str {
    match text.get("translate") {
        Some(Nbt::String(key)) => key,
        _ => panic!("not a translated text: {text:?}"),
    }
}

/// The plain text arguments of a translated text.
fn arguments(text: &Nbt) -> Vec<String> {
    let Some(Nbt::List(with)) = text.get("with") else {
        return Vec::new();
    };
    with.iter()
        .map(|arg| match arg.get("text") {
            Some(Nbt::String(s)) => s.clone(),
            _ => panic!("argument without text: {arg:?}"),
        })
        .collect()
}

async fn chat_message(client: &mut Client, key: &str) -> Vec<String> {
    let chat = read_matching(client, |c: &SystemChat| translate_key(&c.content) == key).await;
    arguments(&chat.content)
}

#[tokio::test]
async fn players_see_each_other_come_move_and_go() {
    let addr = start(test_config()).await;
    let (mut alex, alex_login) = join(addr, "Alex", 4).await;
    assert_eq!(
        chat_message(&mut alex, "multiplayer.player.joined").await,
        ["Alex"]
    );

    let (mut steve, steve_login) = join(addr, "Steve", 4).await;
    let steve_id = steve_login.entity_id;

    // Alex gets Steve into the tab list, then spawned as one bundle.
    let info: PlayerInfoUpdate = alex.read_until().await;
    assert_eq!(info.entries[0].profile.name, "Steve");
    let steve_uuid = info.entries[0].profile.uuid;
    alex.read::<BundleDelimiter>().await;
    let spawn: AddEntity = alex.read().await;
    assert_eq!(spawn.entity_id, steve_id);
    assert_eq!(spawn.entity_type, 159, "minecraft:player");
    assert_eq!(spawn.pos, [0.5, -60.0, 0.5]);
    let data: SetEntityData = alex.read().await;
    assert!(
        data.values
            .0
            .iter()
            .any(|v| v.index == data_index::SKIN_PARTS && v.value == EntityData::Byte(0x7f))
    );
    alex.read::<BundleDelimiter>().await;
    assert_eq!(
        chat_message(&mut alex, "multiplayer.player.joined").await,
        ["Steve"]
    );

    // And Steve sees Alex, who was there first.
    let info: PlayerInfoUpdate = steve.read_until().await;
    assert_eq!(info.entries[0].profile.name, "Alex");
    let spawn: AddEntity = steve.read_until().await;
    assert_eq!(spawn.entity_id, alex_login.entity_id);

    // Steve walks three blocks east: a delta of 3 × 4096, turned to yaw 90.
    walk(&mut steve, [3.5, -60.0, 0.5], Some([90.0, 0.0])).await;
    let step = read_matching(&mut alex, |m: &MoveEntityPosRot| m.entity_id == steve_id).await;
    assert_eq!(step.delta, VecDelta::Stepped(vec![(2, [3 * 4096, 0, 0])]));
    assert_eq!(step.yaw, 64);
    let head = read_matching(&mut alex, |h: &RotateHead| h.entity_id == steve_id).await;
    assert_eq!(head.head_yaw, 64);

    // Out of Alex's view (4 chunks), Steve disappears; back, he reappears
    // where he is.
    walk_steps(&mut steve, [3.5, -60.0, 0.5], [200.5, -60.0, 0.5]).await;
    let gone = read_matching(&mut alex, |r: &RemoveEntities| r.entity_ids.0 == [steve_id]).await;
    assert_eq!(gone.entity_ids.0, [steve_id]);
    walk_steps(&mut steve, [200.5, -60.0, 0.5], [10.5, -60.0, 0.5]).await;
    let back = read_matching(&mut alex, |a: &AddEntity| a.entity_id == steve_id).await;
    // Where he is when he comes back into view, on the way.
    assert!(
        (10.5..120.0).contains(&back.pos[0]) && back.pos[1..] == [-60.0, 0.5],
        "{:?}",
        back.pos
    );

    // Sneaking shows as the crouching pose.
    steve.send(&PlayerInput { flags: 0x20 }).await;
    read_matching(&mut alex, |d: &SetEntityData| {
        d.entity_id == steve_id
            && d.values.0.iter().any(|v| {
                v.index == data_index::POSE && v.value == EntityData::Pose(Pose::Crouching)
            })
    })
    .await;

    drop(steve);
    let gone = read_matching(&mut alex, |r: &RemoveEntities| r.entity_ids.0 == [steve_id]).await;
    assert_eq!(gone.entity_ids.0, [steve_id]);
    let off: PlayerInfoRemove = alex.read_until().await;
    assert_eq!(off.uuids, [steve_uuid]);
    assert_eq!(
        chat_message(&mut alex, "multiplayer.player.left").await,
        ["Steve"]
    );
}

#[tokio::test]
async fn a_far_jump_is_sent_as_an_absolute_position() {
    let addr = start(Config {
        operators: vec!["Steve".into()],
        ..test_config()
    })
    .await;
    let (mut alex, _) = join(addr, "Alex", 10).await;
    let (mut steve, steve_login) = join(addr, "Steve", 10).await;
    read_matching(&mut alex, |a: &AddEntity| {
        a.entity_id == steve_login.entity_id
    })
    .await;

    // Twelve blocks do not fit in a delta of shorts (at most 8 blocks).
    // The server lets such a jump through only without its movement check.
    steve
        .send(&ChatCommand {
            command: "gamerule player_movement_check false".into(),
        })
        .await;
    read_matching(&mut steve, |c: &SystemChat| {
        c.content.get("translate") == Some(&Nbt::String("commands.gamerule.set".into()))
    })
    .await;
    walk(&mut steve, [12.5, -60.0, 0.5], None).await;
    let sync = read_matching(&mut alex, |s: &EntityPositionSync| {
        s.entity_id == steve_login.entity_id
    })
    .await;
    assert_eq!(sync.pos, [12.5, -60.0, 0.5]);
}

#[tokio::test]
async fn a_second_login_replaces_the_first() {
    let addr = start(test_config()).await;
    let (mut first, _) = join(addr, "Alex", 2).await;
    let (_second, _) = join(addr, "Alex", 2).await;
    let bye: Disconnect = first.read_until().await;
    let Some(Nbt::String(text)) = bye.reason.get("text") else {
        panic!("{:?}", bye.reason);
    };
    assert!(text.contains("anderen Ort"), "{text}");
}

#[tokio::test]
async fn chat_reaches_everyone_and_commands_answer() {
    let addr = start(test_config()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    let (mut steve, _) = join(addr, "Steve", 2).await;

    steve.send(&Chat::unsigned("Hallo Alex!")).await;
    for client in [&mut alex, &mut steve] {
        assert_eq!(
            chat_message(client, "chat.type.text").await,
            ["Steve", "Hallo Alex!"]
        );
    }

    alex.send(&ChatCommand {
        command: "list".into(),
    })
    .await;
    assert_eq!(
        chat_message(&mut alex, "commands.list.players").await,
        ["2", "20", "Alex, Steve"]
    );
    alex.send(&ChatCommand {
        command: "fly".into(),
    })
    .await;
    chat_message(&mut alex, "command.unknown.command").await;

    // A section sign could smuggle in formatting codes; vanilla kicks for it.
    steve.send(&Chat::unsigned("§cRot")).await;
    let bye: Disconnect = steve.read_until().await;
    assert!(format!("{:?}", bye.reason).contains("Unerlaubte Zeichen"));
}

#[tokio::test]
async fn placed_and_broken_blocks_reach_everyone() {
    let addr = start(test_config()).await;
    let (mut alex, alex_login) = join(addr, "Alex", 2).await;
    let (mut steve, _) = join(addr, "Steve", 2).await;

    // Others see what Alex holds.
    hold(&mut alex, STONE_ITEM).await;
    let equipment = read_matching(&mut steve, |e: &SetEquipment| {
        e.entity_id == alex_login.entity_id
    })
    .await;
    assert_eq!(equipment.slots[0].0, equipment_slot::MAIN_HAND);
    assert_eq!(
        equipment.slots[0].1.as_ref().map(|s| s.item),
        Some(STONE_ITEM)
    );

    // A click on top of the grass at (2, -61, 0) places stone above it.
    let placed = BlockPos { x: 2, y: -60, z: 0 };
    alex.send(&click(BlockPos { x: 2, y: -61, z: 0 }, 1, 1))
        .await;
    let update = read_matching(&mut steve, |u: &BlockUpdate| u.pos == placed).await;
    assert_eq!(update.state, STONE);
    read_matching(&mut alex, |u: &BlockUpdate| {
        u.pos == placed && u.state == STONE
    })
    .await;
    assert_eq!(alex.read_until::<BlockChangedAck>().await.sequence, 1);

    // A newcomer gets the chunk with the stone in it.
    let (mut newcomer, _) = Client::login(addr, "Neu").await;
    newcomer.configure(2).await;
    let (_, _, chunks) = common::enter_world(&mut newcomer).await;
    let sections = |x, z| -> &Vec<u8> {
        &chunks
            .iter()
            .find(|c: &&LevelChunkWithLight| (c.x, c.z) == (x, z))
            .unwrap()
            .sections
    };
    assert_ne!(sections(0, 0), sections(1, 1), "chunk (0, 0) has the stone");

    // Steve breaks it again.
    steve
        .send(&PlayerAction {
            action: player_action::START_DESTROY_BLOCK,
            pos: placed,
            face: 1,
            sequence: 7,
        })
        .await;
    read_matching(&mut alex, |u: &BlockUpdate| {
        u.pos == placed && u.state == AIR
    })
    .await;
    assert_eq!(steve.read_until::<BlockChangedAck>().await.sequence, 7);
}

#[tokio::test]
async fn blocks_are_not_placed_out_of_reach_or_into_players() {
    let addr = start(test_config()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    hold(&mut alex, STONE_ITEM).await;

    // Far away: the client gets the true state back, then the ack.
    let far = BlockPos {
        x: 20,
        y: -61,
        z: 0,
    };
    alex.send(&click(far, 1, 1)).await;
    let update: BlockUpdate = alex.read_until().await;
    assert_eq!(update.pos, far);
    let update: BlockUpdate = alex.read().await;
    assert_eq!((update.pos.y, update.state), (-60, AIR));
    assert_eq!(alex.read::<BlockChangedAck>().await.sequence, 1);

    // Where Alex stands (0.5, -60, 0.5): refused, the air stays.
    alex.send(&click(BlockPos { x: 0, y: -61, z: 0 }, 1, 2))
        .await;
    let update = read_matching(&mut alex, |u: &BlockUpdate| u.pos.y == -60).await;
    assert_eq!(update.state, AIR);
    assert_eq!(alex.read_until::<BlockChangedAck>().await.sequence, 2);

    // A torch has no collision, so it may go where Alex stands, and its
    // light reaches the client.
    let data = ferrocraft::data::GameData::load().unwrap();
    hold(&mut alex, data.item_id("minecraft:torch").unwrap()).await;
    alex.send(&click(BlockPos { x: 0, y: -61, z: 0 }, 1, 5))
        .await;
    let torch = data.default_state("minecraft:torch").unwrap() as i32;
    read_matching(&mut alex, |u: &BlockUpdate| {
        u.pos.y == -60 && u.state == torch
    })
    .await;
    let light = read_matching(&mut alex, |l: &LightUpdate| (l.x, l.z) == (0, 0)).await;
    assert!(
        light.light.block_mask.get(1),
        "block light in the section of the torch"
    );

    // Items that are not blocks place nothing, and blocks only operators may
    // place need operators.
    for (sequence, item) in [(3, STICK_ITEM), (4, COMMAND_BLOCK_ITEM)] {
        hold(&mut alex, item).await;
        alex.send(&click(BlockPos { x: 2, y: -61, z: 0 }, 1, sequence))
            .await;
        let update = read_matching(&mut alex, |u: &BlockUpdate| u.pos.y == -60).await;
        assert_eq!(update.state, AIR, "item {item}");
        assert_eq!(
            alex.read_until::<BlockChangedAck>().await.sequence,
            sequence
        );
    }
}
