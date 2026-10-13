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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:25565".into(),
            motd: "Ein Ferrocraft-Server".into(),
            max_players: 20,
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
