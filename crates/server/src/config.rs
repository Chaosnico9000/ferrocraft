use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Address and port to listen on.
    pub bind: String,
    /// Text shown under the server name in the multiplayer list.
    pub motd: String,
    pub max_players: u32,
    /// Radius in chunks around the player that gets sent; the client's own
    /// setting caps it further.
    pub view_distance: u32,
    /// Packets of at least this many bytes are compressed; negative turns
    /// compression off. Vanilla's `network-compression-threshold`.
    pub compression_threshold: i32,
    /// Directory of the saved world, in vanilla's layout. Empty keeps the
    /// world in memory only.
    pub world: String,
    /// Check with Mojang that players own their account; otherwise anyone
    /// can join under any name.
    pub online_mode: bool,
    /// Where online mode checks players; Mojang's session server unless a
    /// test or a mirror needs another.
    pub session_server: String,
    /// Game mode of new players: survival, creative, adventure or spectator.
    pub game_mode: String,
    /// Names of the players who may use operator commands.
    pub operators: Vec<String>,
    /// Cows, pigs, sheep and chickens spawn near players.
    pub animals: bool,
    /// Zombies, skeletons, creepers and spiders spawn in the dark.
    pub monsters: bool,
    /// peaceful, easy, normal or hard, like vanilla's `difficulty`.
    pub difficulty: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:25565".into(),
            motd: "Ein Ferrocraft-Server".into(),
            max_players: 20,
            view_distance: 10,
            compression_threshold: 256,
            world: "world".into(),
            online_mode: true,
            session_server: crate::auth::SESSION_SERVER.into(),
            game_mode: "survival".into(),
            operators: Vec::new(),
            animals: true,
            monsters: true,
            difficulty: "easy".into(),
        }
    }
}

impl Config {
    /// Reads the config, writing the defaults first if the file does not exist,
    /// the way vanilla creates `server.properties` on first start.
    pub fn load_or_create(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            let config = Self::default();
            std::fs::write(path, toml::to_string_pretty(&config)?)
                .with_context(|| format!("writing default config to {}", path.display()))?;
            return Ok(config);
        }
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }
}
