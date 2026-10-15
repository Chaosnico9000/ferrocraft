//! The server console runs commands as an operator and answers in
//! English; game rules, weather, lit TNT and falling blocks survive a
//! restart.

mod common;

use std::time::Duration;

use common::{Client, enter_world, join, read_matching, start_server, test_config};
use ferrocraft::{Config, Timing};
use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::packets::play::{AddEntity, ChatCommand, GameEvent, SystemChat};

/// A world directory of its own, removed again when dropped.
struct TempWorld(std::path::PathBuf);

impl TempWorld {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ferrocraft-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }

    fn config(&self) -> Config {
        Config {
            world: self.0.to_string_lossy().into_owned(),
            operators: vec!["Alex".into()],
            ..test_config()
        }
    }
}

impl Drop for TempWorld {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn the_console_answers_in_english_and_rules_are_kept() {
    let world = TempWorld::new("console");
    let (server, addr) = start_server(world.config(), Timing::default()).await;
    assert_eq!(
        server.console("gamerule random_tick_speed 7").await,
        "Game rule random_tick_speed is now set to 7"
    );
    assert_eq!(
        server.console("gamerule random_tick_speed -1").await,
        "Integer must not be less than 0: found -1"
    );
    assert_eq!(
        server.console("gamemode creative").await,
        "A player is required to run this command here"
    );
    let (mut alex, _) = join(addr, "Alex", 2).await;
    assert_eq!(
        server.console("gamemode survival Alex").await,
        "Set Alex's game mode to Survival Mode"
    );
    server.console("gamemode creative Alex").await;
    server.console("say Hallo zusammen").await;
    let said = read_matching(&mut alex, |c: &SystemChat| {
        c.content.get("translate") == Some(&Nbt::String("chat.type.announcement".into()))
    })
    .await;
    assert!(format!("{:?}", said.content).contains("Hallo zusammen"));
    assert_eq!(
        server.console("weather thunder").await,
        "Set the weather to rain and thunder"
    );
    server.shutdown().await;

    let (server, addr) = start_server(world.config(), Timing::default()).await;
    assert_eq!(
        server.console("gamerule random_tick_speed").await,
        "Game rule random_tick_speed is currently set to 7"
    );
    // It still rains: at full strength right away for whoever joins.
    let (mut alex, _) = Client::login(addr, "Alex").await;
    alex.configure(2).await;
    enter_world(&mut alex).await;
    let rain = read_matching(&mut alex, |e: &GameEvent| e.event == 7).await;
    assert_eq!(rain.param, 1.0);
}

#[tokio::test]
async fn lit_tnt_and_falling_sand_survive_a_restart() {
    let world = TempWorld::new("flying");
    let config = world.config();
    let (server, addr) = start_server(config.clone(), Timing::default()).await;
    let data = ferrocraft::data::GameData::load().unwrap();
    let tnt = data.entity_type("minecraft:tnt").unwrap();
    let falling = data.entity_type("minecraft:falling_block").unwrap();
    let (mut alex, _) = join(addr, "Alex", 2).await;
    for command in [
        "setblock 3 -60 3 tnt",
        "setblock 4 -60 3 redstone_block",
        "setblock 3 -30 -3 sand",
    ] {
        alex.send(&ChatCommand {
            command: command.into(),
        })
        .await;
    }
    let mut seen = (false, false);
    tokio::time::timeout(Duration::from_secs(3), async {
        while seen != (true, true) {
            let added: AddEntity = alex.read_until().await;
            seen.0 |= added.entity_type == tnt;
            seen.1 |= added.entity_type == falling;
        }
    })
    .await
    .expect("lit TNT and falling sand");
    server.shutdown().await;

    let (_server, addr) = start_server(config, Timing::default()).await;
    let (mut alex, _) = join(addr, "Alex", 2).await;
    let mut seen = (false, false);
    tokio::time::timeout(Duration::from_secs(3), async {
        while seen != (true, true) {
            let added: AddEntity = alex.read_until().await;
            seen.0 |= added.entity_type == tnt;
            seen.1 |= added.entity_type == falling;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("back after the restart: {seen:?}"));
}
