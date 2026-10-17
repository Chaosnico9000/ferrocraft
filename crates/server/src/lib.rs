mod auth;
pub mod block_entity;
pub mod config;
mod configuration;
mod connection;
mod crafting;
pub mod data;
mod game;
mod inventory;
pub mod java_random;
pub mod level;
mod light;
mod login;
mod loot;
mod placement;
mod play;
pub mod rules;
pub mod stack;
mod status;
pub mod storage;
mod tree;
pub mod world;

use std::path::Path;

use anyhow::Context as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

use tokio::net::TcpListener;
use tracing::{debug, info};
use uuid::Uuid;

pub use config::Config;

use crate::auth::Keys;
use crate::data::GameData;
pub use crate::game::TICK;
use crate::game::{Event, TickStats};
use crate::level::Level;
use crate::storage::Storage;
use crate::world::World;
use ferrocraft_protocol::codec::BlockPos;

/// What an enchanting table with `bookcases` shelves offers for an item to
/// a player with `seed` (`EnchantmentMenu.slotsChanged`): the costs, the
/// enchantment ids shown as clues and their levels.
pub fn enchanting_offers(
    data: &GameData,
    seed: i32,
    item: &ferrocraft_protocol::packets::play::ItemStack,
    bookcases: i32,
) -> ([i32; 3], [i32; 3], [i32; 3]) {
    let offers = game::enchanting_offers(data, seed, item, bookcases);
    (offers.costs, offers.clues, offers.levels)
}

/// Timings that tests want shorter than vanilla's.
#[derive(Debug, Clone)]
pub struct Timing {
    /// How often a keep-alive goes out, and how long the client has to answer.
    pub keep_alive: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            keep_alive: Duration::from_secs(15),
        }
    }
}

/// State shared by all connections.
#[derive(Debug)]
pub struct Server {
    pub config: Config,
    pub online_players: AtomicU32,
    pub data: Arc<GameData>,
    pub world: Arc<World>,
    /// `None` when the world is not saved.
    pub storage: Option<Arc<Storage>>,
    /// The key pair for online mode's key exchange; `None` in offline mode.
    keys: Option<Keys>,
    http: reqwest::Client,
    /// Where new players appear, from `level.dat`.
    pub spawn: BlockPos,
    pub spawn_angle: [f32; 2],
    /// For the client's biome noise.
    pub hashed_seed: i64,
    pub default_game_mode: ferrocraft_protocol::packets::play::GameMode,
    pub tick_stats: Arc<TickStats>,
    /// Random per start, like vanilla's; the client gets it in `login_finished`.
    session_id: Uuid,
    next_entity_id: Arc<AtomicI32>,
    tick: Sender<Event>,
}

impl Server {
    pub fn new(config: Config) -> anyhow::Result<Arc<Self>> {
        Self::with_timing(config, Timing::default())
    }

    /// Loads the saved world, if any, and starts the tick thread.
    pub fn with_timing(config: Config, timing: Timing) -> anyhow::Result<Arc<Self>> {
        // The data is embedded at build time and covered by tests, so failing
        // here is a bug, not an operating error.
        let data = Arc::new(GameData::load().expect("embedded game data"));
        let world =
            Arc::new(World::superflat(Arc::clone(&data)).expect("building the superflat world"));
        let mut entities = game::StoredEntities::default();
        let storage = if config.world.is_empty() {
            None
        } else {
            let storage = Storage::open(Path::new(&config.world))?;
            let chunks = storage.load_chunks(&data)?;
            entities = game::StoredEntities::load(&storage)?;
            info!(
                world = %config.world,
                chunks = chunks.len(),
                entities = entities.len(),
                "loaded the world"
            );
            for chunk in chunks {
                world.load_chunk(chunk);
            }
            world.light_loaded();
            Some(Arc::new(storage))
        };
        let difficulty = game::parse_difficulty(&config.difficulty).with_context(|| {
            format!(
                "difficulty = \"{}\" is not peaceful, easy, normal or hard",
                config.difficulty
            )
        })?;
        let default_game_mode = game::parse_mode(&config.game_mode)
            .with_context(|| format!("game_mode = \"{}\" is not a game mode", config.game_mode))?;
        let spawn = BlockPos {
            x: 0,
            y: world.surface_y,
            z: 0,
        };
        let level = match &storage {
            Some(storage) => Level::load(storage)?,
            None => None,
        }
        .unwrap_or_else(|| Level::new(spawn, default_game_mode as i32));
        let (rules, weather) = match &storage {
            Some(storage) => (
                rules::GameRules::load(storage)?,
                game::Weather::load(storage)?,
            ),
            None => Default::default(),
        };
        let keys = if config.online_mode {
            Some(Keys::generate()?)
        } else {
            None
        };
        let tick_stats = Arc::new(TickStats::default());
        let next_entity_id = Arc::new(AtomicI32::new(1));
        let tick = game::spawn(game::Context {
            keep_alive: timing.keep_alive,
            max_players: config.max_players,
            stats: Arc::clone(&tick_stats),
            world: Arc::clone(&world),
            data: Arc::clone(&data),
            storage: storage.clone(),
            level: level.clone(),
            entity_ids: Arc::clone(&next_entity_id),
            animals: config.animals,
            monsters: config.monsters,
            allow_flight: config.allow_flight,
            difficulty,
            entities,
            rules,
            weather,
        });
        Ok(Arc::new(Self {
            config,
            online_players: AtomicU32::new(0),
            data,
            world,
            tick_stats,
            session_id: Uuid::new_v4(),
            next_entity_id,
            tick,
            storage,
            keys,
            http: reqwest::Client::new(),
            spawn: level.spawn,
            spawn_angle: level.spawn_angle,
            hashed_seed: level::hashed_seed(level.seed),
            default_game_mode,
        }))
    }

    /// Disconnects everyone and saves; the tick thread stops.
    pub async fn shutdown(&self) {
        let (done, saved) = tokio::sync::oneshot::channel();
        self.tick_event(Event::Shutdown(done));
        let _ = saved.await;
    }

    /// Runs a line typed into the server console as an operator would;
    /// the answer as the console shows it.
    pub async fn console(&self, line: &str) -> String {
        let (reply, answer) = tokio::sync::oneshot::channel();
        self.tick_event(Event::Console {
            line: line.to_owned(),
            reply,
        });
        answer.await.unwrap_or_default()
    }

    /// Hands a console line to the game without waiting for the answer,
    /// which goes to the log; lines typed together run in the same tick,
    /// as in vanilla.
    pub fn console_line(&self, line: &str) {
        let (reply, _) = tokio::sync::oneshot::channel();
        self.tick_event(Event::Console {
            line: line.to_owned(),
            reply,
        });
    }

    fn next_entity_id(&self) -> i32 {
        self.next_entity_id.fetch_add(1, Ordering::Relaxed)
    }

    /// The permission level of a player: operators are listed by name.
    fn permission(&self, name: &str) -> u8 {
        if self
            .config
            .operators
            .iter()
            .any(|op| op.eq_ignore_ascii_case(name))
        {
            game::OPERATOR
        } else {
            0
        }
    }

    fn tick_event(&self, event: Event) {
        // The tick thread only stops when the server is dropped.
        let _ = self.tick.send(event);
    }

    /// Accepts connections until the listener fails; each connection runs on
    /// its own task.
    pub async fn serve(self: Arc<Self>, listener: TcpListener) -> anyhow::Result<()> {
        info!(addr = %listener.local_addr()?, "listening");
        loop {
            let (stream, peer) = listener.accept().await?;
            let server = Arc::clone(&self);
            tokio::spawn(async move {
                if let Err(err) = connection::handle(server, stream).await {
                    debug!(%peer, "connection closed: {err:#}");
                }
            });
        }
    }
}
