//! The world and the players survive a restart.

mod common;

use common::{
    Client, click, enter_world, hold, join, read_matching, start_server, test_config, walk,
};
use ferrocraft::{Config, Timing};
use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    BlockChangedAck, ChatCommand, ContainerSetContent, Disconnect, SetHeldSlot, SetTime, SystemChat,
};

const STONE_ITEM: i32 = 1;

/// A world directory of its own, removed again when dropped.
struct TempWorld(std::path::PathBuf);

impl TempWorld {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ferrocraft-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }
}

impl Drop for TempWorld {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn blocks_position_and_inventory_survive_a_restart() {
    let world = TempWorld::new("restart");
    let config = Config {
        world: world.0.to_string_lossy().into_owned(),
        ..test_config()
    };

    let (server, addr) = start_server(config.clone(), Timing::default()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    hold(&mut alex, STONE_ITEM).await;
    alex.send(&click(BlockPos { x: 2, y: -61, z: 0 }, 1, 1))
        .await;
    assert_eq!(alex.read_until::<BlockChangedAck>().await.sequence, 1);
    walk(&mut alex, [5.5, -60.0, 2.5], Some([45.0, 10.0])).await;
    // The tick thread handles events in order: once it answers, it has the
    // position.
    alex.send(&ChatCommand {
        command: "list".into(),
    })
    .await;
    read_matching(&mut alex, |c: &SystemChat| {
        c.content.get("translate") == Some(&Nbt::String("commands.list.players".into()))
    })
    .await;

    server.shutdown().await;
    let bye: Disconnect = alex.read_until().await;
    assert!(format!("{:?}", bye.reason).contains("Server geschlossen"));
    let region = world
        .0
        .join("dimensions/minecraft/overworld/region/r.0.0.mca");
    assert!(region.exists(), "vanilla's 26.x layout");
    let players = std::fs::read_dir(world.0.join("players/data")).unwrap();
    assert_eq!(players.count(), 1);
    assert!(world.0.join("level.dat").exists());
    assert!(world.0.join("data/minecraft/world_clocks.dat").exists());

    let (_server, addr) = start_server(config, Timing::default()).await;
    let (mut alex, _) = Client::login(addr, "Alex").await;
    alex.configure(2).await;
    let (_, position, chunks) = enter_world(&mut alex).await;
    assert_eq!(position.pos, [5.5, -60.0, 2.5]);
    // The clocks go on where they stopped instead of starting at 0.
    let time: SetTime = alex.read_until().await;
    assert!(time.game_time > 0 && !time.clocks.is_empty(), "{time:?}");
    assert_eq!((position.yaw, position.pitch), (45.0, 10.0));
    let sections = |x, z| {
        &chunks
            .iter()
            .find(|c| (c.x, c.z) == (x, z))
            .unwrap()
            .sections
    };
    assert_ne!(sections(0, 0), sections(1, 1), "the stone is still there");
}

#[tokio::test]
async fn the_inventory_comes_back() {
    let world = TempWorld::new("inventory");
    let config = Config {
        world: world.0.to_string_lossy().into_owned(),
        ..test_config()
    };
    let (server, addr) = start_server(config.clone(), Timing::default()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    hold(&mut alex, STONE_ITEM).await;
    drop(alex);
    // Leaving saves the player; a second login reads it back.
    let (mut alex, inventory) = loop {
        let (mut client, _) = Client::login(addr, "Alex").await;
        client.configure(2).await;
        let inventory: ContainerSetContent = client.read_until().await;
        if inventory.items.0[36].0.is_some() {
            break (client, inventory);
        }
        // The first session may not have been saved yet.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    let item = inventory.items.0[36].0.as_ref().unwrap();
    assert_eq!((item.item, item.count), (STONE_ITEM, 64));
    assert_eq!(alex.read::<SetHeldSlot>().await.slot, 0);
    server.shutdown().await;
}

#[tokio::test]
async fn chest_contents_survive_a_restart() {
    use ferrocraft::data::GameData;
    use ferrocraft_protocol::packets::play::{ContainerClick, OpenScreen, container_input};

    let data = GameData::load().unwrap();
    let chest = data.item_id("minecraft:chest").unwrap();
    let world = TempWorld::new("chest");
    let config = Config {
        world: world.0.to_string_lossy().into_owned(),
        ..test_config()
    };
    let pos = BlockPos { x: 2, y: -60, z: 0 };
    let (server, addr) = start_server(config.clone(), Timing::default()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    hold(&mut alex, chest).await;
    alex.send(&click(BlockPos { y: -61, ..pos }, 1, 1)).await;
    alex.read_until::<BlockChangedAck>().await;
    // Into the chest: the stack of chests in hand (menu slot 54) goes to
    // slot 5.
    hold(&mut alex, STONE_ITEM).await;
    alex.send(&click(pos, 1, 2)).await;
    let screen: OpenScreen = alex.read_until().await;
    for slot in [54, 5] {
        alex.send(&ContainerClick {
            container_id: screen.container_id,
            state_id: 0,
            slot,
            button: 0,
            input: container_input::PICKUP,
            changed: Vec::new(),
            carried: None,
        })
        .await;
        alex.read_until::<ContainerSetContent>().await;
    }
    server.shutdown().await;
    alex.read_until::<Disconnect>().await;

    let (_server, addr) = start_server(config, Timing::default()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    alex.send(&click(pos, 1, 1)).await;
    alex.read_until::<OpenScreen>().await;
    let content: ContainerSetContent = alex.read_until().await;
    let stone = content.items.0[5].0.as_ref().unwrap();
    assert_eq!((stone.item, stone.count), (STONE_ITEM, 64));
}

#[tokio::test]
async fn a_broken_player_file_does_not_keep_the_player_out() {
    let world = TempWorld::new("broken-player");
    let config = Config {
        world: world.0.to_string_lossy().into_owned(),
        ..test_config()
    };
    let (server, addr) = start_server(config.clone(), Timing::default()).await;
    let (alex, _) = join(addr, "Alex", 2).await;
    drop(alex);
    server.shutdown().await;
    // As a hard shutdown can leave it: empty, and no backup.
    let players = world.0.join("players/data");
    for entry in std::fs::read_dir(&players).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "dat") {
            std::fs::write(&path, b"").unwrap();
        } else {
            std::fs::remove_file(&path).unwrap();
        }
    }

    let (server, addr) = start_server(config, Timing::default()).await;
    let (alex, _) = join(addr, "Alex", 2).await;
    drop(alex);
    // Leaving counts as leaving.
    for _ in 0..100 {
        if server
            .online_players
            .load(std::sync::atomic::Ordering::Relaxed)
            == 0
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("the player still counts as online");
}

#[tokio::test]
async fn animals_and_monsters_survive_a_restart() {
    use ferrocraft::data::GameData;
    use ferrocraft_protocol::packets::play::AddEntity;

    let data = GameData::load().unwrap();
    let (cow_type, _) = data.entity("minecraft:cow").unwrap();
    let (zombie_type, _) = data.entity("minecraft:zombie").unwrap();
    let world = TempWorld::new("entities");
    let config = Config {
        world: world.0.to_string_lossy().into_owned(),
        operators: vec!["Alex".into()],
        ..test_config()
    };
    let (server, addr) = start_server(config.clone(), Timing::default()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    for command in [
        "time set midnight",
        "summon cow 4 -60 4",
        "summon zombie -4 -60 4",
    ] {
        alex.send(&ChatCommand {
            command: command.into(),
        })
        .await;
    }
    let cow = read_matching(&mut alex, |a: &AddEntity| a.entity_type == cow_type).await;
    read_matching(&mut alex, |a: &AddEntity| a.entity_type == zombie_type).await;
    server.shutdown().await;
    assert!(
        world
            .0
            .join("dimensions/minecraft/overworld/entities/r.0.0.mca")
            .exists(),
        "vanilla's entity storage"
    );

    let (_server, addr) = start_server(config, Timing::default()).await;
    let (mut alex, _) = join(addr, "Alex", 4).await;
    // Both come back, in no particular order.
    let (mut cow_back, mut zombie_back) = (None, false);
    while cow_back.is_none() || !zombie_back {
        let added: AddEntity = alex.read_until().await;
        if added.entity_type == cow_type {
            cow_back = Some(added);
        } else if added.entity_type == zombie_type {
            zombie_back = true;
        }
    }
    let back = cow_back.unwrap();
    let distance = (back.pos[0] - cow.pos[0]).hypot(back.pos[2] - cow.pos[2]);
    assert!(distance < 4.0, "{:?} vs {:?}", back.pos, cow.pos);
    assert_eq!(back.uuid, cow.uuid, "the same cow");
}

#[tokio::test]
async fn flowing_water_goes_on_after_a_restart() {
    use ferrocraft::data::GameData;
    use ferrocraft_protocol::packets::play::BlockUpdate;
    let data = GameData::load().unwrap();
    let water = data.block_named("minecraft:water").unwrap().first;
    let world = TempWorld::new("ticks");
    let config = Config {
        world: world.0.to_string_lossy().into_owned(),
        operators: vec!["Alex".into()],
        ..test_config()
    };
    let (server, addr) = start_server(config.clone(), Timing::default()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    alex.send(&ChatCommand {
        command: "setblock 0 -60 4 water".into(),
    })
    .await;
    // Stopped after the first step: its next ticks are waiting.
    read_matching(&mut alex, |u: &BlockUpdate| {
        u.pos == BlockPos { x: 1, y: -60, z: 4 } && u.state as u32 == water + 1
    })
    .await;
    server.shutdown().await;

    let (_server, addr) = start_server(config, Timing::default()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    // The saved ticks run on: the water reaches its seventh block.
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        read_matching(&mut alex, |u: &BlockUpdate| {
            u.pos == BlockPos { x: 7, y: -60, z: 4 } && u.state as u32 == water + 7
        }),
    )
    .await
    .expect("the water stood still after the restart");
}
