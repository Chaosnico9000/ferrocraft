//! The game: a thread that owns the game state and advances it 20 times a
//! second, like vanilla's server thread. Connection tasks talk to it through
//! channels; it never touches a socket.
//!
//! The state is the players (inventories, game modes, keep-alives, the tab
//! list, what the others see of each), time, and every change to the world,
//! so that block changes and chat reach everyone in the same order. The
//! connections only decide what their client can see.

mod combat;
mod command;
mod explosion;
mod furnace;
mod interact;
mod item;
mod menu;
mod mob;
mod orb;
mod path;
mod physics;
mod player;
mod projectile;
mod survival;
mod tracker;

pub use command::{OPERATOR, parse_difficulty, parse_mode, tree as command_tree};
use menu::{Drag, Menu};
pub use mob::StoredEntities;
pub use player::Experience;
use player::{ENDER_SLOTS, Food, PLAYER_UPDATE_INTERVAL};
pub use player::{Player, abilities};
use tracker::Tracker;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::Packet;
use ferrocraft_protocol::packets::login::GameProfile;
use ferrocraft_protocol::packets::play::{
    ChangeDifficulty, ClockState, ContainerClick, EntityData, EntityDataValue, EntityDataValues,
    GameMode, ItemStack, LightUpdate, PlayerAction, PlayerInfoEntry, PlayerInfoLatency,
    SetEntityData, SetEquipment, SetTime, SignUpdate, SwingAnimation, SystemChat, UseItemOn,
    data_index, text,
};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::data::GameData;
use crate::inventory::Inventory;
use crate::level::Level;
use crate::storage::Storage;
use crate::world::{BIOME, ChunkPos, LAYERS, World};

pub const TICK: Duration = Duration::from_millis(50);
/// Vanilla warns when it falls this far behind and then skips the missed ticks.
const MAX_LAG: Duration = Duration::from_secs(2);
/// How often the tab list's ping bars are refreshed, like `PlayerList.tick`.
const LATENCY_INTERVAL: u64 = 600;
/// Vanilla saves every five minutes.
const AUTOSAVE_INTERVAL: u64 = 6000;

/// What a connection task tells the game.
#[derive(Debug)]
pub enum Event {
    Joined(Box<Joined>),
    Left {
        entity_id: i32,
    },
    KeepAlive {
        entity_id: i32,
        id: i64,
    },
    Moved {
        entity_id: i32,
        pos: Option<[f64; 3]>,
        /// Yaw and pitch in degrees.
        rot: Option<[f32; 2]>,
        on_ground: bool,
    },
    Sneaking {
        entity_id: i32,
        sneaking: bool,
    },
    Sprinting {
        entity_id: i32,
        sprinting: bool,
    },
    Flying {
        entity_id: i32,
        flying: bool,
    },
    Settings {
        entity_id: i32,
        skin_parts: u8,
        main_hand: i32,
    },
    Swing {
        entity_id: i32,
    },
    /// `set_creative_mode_slot`, only honoured in creative.
    CreativeSlot {
        entity_id: i32,
        slot: i16,
        stack: Option<ItemStack>,
    },
    SelectSlot {
        entity_id: i32,
        slot: i16,
    },
    UseItemOn {
        entity_id: i32,
        click: UseItemOn,
    },
    Action {
        entity_id: i32,
        action: PlayerAction,
    },
    /// The F3+F4 switcher.
    ChangeGameMode {
        entity_id: i32,
        mode: i32,
    },
    Chat {
        entity_id: i32,
        message: String,
    },
    Command {
        entity_id: i32,
        command: String,
    },
    ContainerClick {
        entity_id: i32,
        click: ContainerClick,
    },
    ContainerClose {
        entity_id: i32,
        container_id: i32,
    },
    SignUpdate {
        entity_id: i32,
        update: SignUpdate,
    },
    /// The middle click on a block.
    PickBlock {
        entity_id: i32,
        pos: BlockPos,
        include_data: bool,
    },
    /// A right click with an item in the air, e.g. to eat.
    UseItem {
        entity_id: i32,
        hand: i32,
        sequence: i32,
    },
    /// The respawn button on the death screen.
    Respawn {
        entity_id: i32,
    },
    /// A left click on an entity.
    Attack {
        entity_id: i32,
        target: i32,
    },
    /// A right click on an entity.
    Interact {
        entity_id: i32,
        target: i32,
        hand: i32,
    },
    /// Disconnect everyone, save, stop the game thread, then answer.
    Shutdown(tokio::sync::oneshot::Sender<()>),
}

/// A new player, after its join packets.
#[derive(Debug)]
pub struct Joined {
    pub entity_id: i32,
    pub profile: GameProfile,
    pub game_mode: GameMode,
    /// 0, or [`OPERATOR`].
    pub permission: u8,
    pub pos: [f64; 3],
    /// Yaw and pitch.
    pub rot: [f32; 2],
    pub inventory: Inventory,
    pub ender: Vec<(usize, ItemStack)>,
    pub health: f32,
    pub experience: Experience,
    pub skin_parts: u8,
    pub main_hand: i32,
    pub outbound: UnboundedSender<Outbound>,
}

/// An encoded packet body, ready to be framed.
#[derive(Debug, Clone, PartialEq)]
pub struct Encoded {
    pub id: i32,
    pub body: Vec<u8>,
}

impl Encoded {
    pub fn new<P: Packet>(packet: &P) -> Self {
        let mut body = Vec::new();
        packet.encode(&mut body);
        Self { id: P::ID, body }
    }
}

/// Everything a client needs to spawn an entity: the last position and
/// rotation sent, and its data and equipment.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityState {
    pub entity_id: i32,
    pub uuid: Uuid,
    /// Network id in `minecraft:entity_type`.
    pub entity_type: i32,
    pub pos: [f64; 3],
    pub yaw: u8,
    pub pitch: u8,
    pub head_yaw: u8,
    pub velocity: [f64; 3],
    /// `add_entity`'s extra int.
    pub object_data: i32,
    /// Values that differ from the defaults.
    pub data: Vec<EntityDataValue>,
    /// Slots that are not empty.
    pub equipment: Vec<(u8, ItemStack)>,
    /// Only other spectators see a spectator.
    pub spectator: bool,
    /// How far away clients see it at most, in blocks
    /// (`EntityType.clientTrackingRange`).
    pub range: f64,
}

/// Players are seen up to 32 chunks away; the viewer's chunk view limits
/// it further.
pub const PLAYER_TRACKING_RANGE: f64 = 32.0 * 16.0;

/// What the tick thread asks a connection task to send.
#[derive(Debug, Clone)]
pub enum Outbound {
    KeepAlive(i64),
    Disconnect(String),
    /// Sent as it is.
    Packet(Arc<Encoded>),
    /// Another player came online: add them to the tab list and spawn them
    /// once they are in view.
    PlayerAdded {
        info: Box<PlayerInfoEntry>,
        state: Arc<EntityState>,
    },
    PlayerRemoved {
        entity_id: i32,
        uuid: Uuid,
    },
    /// Changes to another player, for clients that see them; `state` is how
    /// they look afterwards, for clients that start seeing them now.
    Entity {
        state: Arc<EntityState>,
        packets: Arc<[Encoded]>,
    },
    /// For clients that have the chunk.
    BlockChanged {
        pos: BlockPos,
        state: u32,
    },
    BlockAck(i32),
    /// For clients that have the chunk: light, block entity data, block
    /// events.
    InChunk {
        chunk: (i32, i32),
        packet: Arc<Encoded>,
    },
    /// The player's own game mode changed.
    GameMode(GameMode),
    /// An entity other than a player appeared; clients near it spawn it.
    EntityAdded(Arc<EntityState>),
    EntityRemoved(i32),
    /// Move the player; the connection sends the teleport and waits for the
    /// client to confirm it.
    Teleport {
        pos: [f64; 3],
        rot: [f32; 2],
    },
    /// The player comes back to life at `pos`.
    Respawn {
        pos: [f64; 3],
        rot: [f32; 2],
        game_mode: GameMode,
    },
    /// Another player respawned: clients that see them spawn them anew.
    Respawned(Arc<EntityState>),
}

/// Tick time of the last [`STATS_WINDOW`] ticks, readable from any thread.
#[derive(Debug, Default)]
pub struct TickStats {
    /// Mean milliseconds per tick, as `f32` bits.
    mspt_bits: AtomicU32,
}

const STATS_WINDOW: usize = 100;

impl TickStats {
    pub fn mspt(&self) -> f32 {
        f32::from_bits(self.mspt_bits.load(Ordering::Relaxed))
    }
}

/// What the tick thread needs from the server.
pub struct Context {
    pub keep_alive: Duration,
    pub max_players: u32,
    pub stats: Arc<TickStats>,
    pub world: Arc<World>,
    pub data: Arc<GameData>,
    /// Where the world is saved; `None` keeps it in memory only.
    pub storage: Option<Arc<Storage>>,
    /// Spawn and time, saved as `level.dat`.
    pub level: Level,
    /// Hands out entity ids, shared with the connections.
    pub entity_ids: Arc<AtomicI32>,
    /// Whether animals and monsters spawn by themselves.
    pub animals: bool,
    pub monsters: bool,
    /// 0 peaceful, 1 easy, 2 normal, 3 hard.
    pub difficulty: u8,
    /// Entities saved in the world, to come back near players.
    pub entities: StoredEntities,
}

/// Starts the tick thread. It runs until every [`Sender`] is dropped, i.e.
/// until the server is.
pub fn spawn(context: Context) -> Sender<Event> {
    let (events, rx) = channel();
    std::thread::Builder::new()
        .name("tick".into())
        .spawn(move || Game::new(context).run(&rx))
        .expect("spawning the tick thread");
    events
}

struct Game {
    context: Context,
    players: HashMap<i32, Player>,
    air: u32,
    started: Instant,
    tick: u64,
    /// `minecraft:world_clock` ids.
    overworld_clock: i32,
    end_clock: i32,
    player_type: i32,
    item_type: i32,
    items: HashMap<i32, item::ItemEntity>,
    mobs: HashMap<i32, mob::Mob>,
    arrows: HashMap<i32, projectile::Arrow>,
    orbs: HashMap<i32, orb::Orb>,
    /// Entities of chunks no player is near, and those Ferrocraft cannot
    /// run, kept for saving.
    stored: StoredEntities,
    foreign: HashMap<ChunkPos, Vec<Nbt>>,
    /// Players an explosion threw this tick, and how far.
    explosion_knockback: Vec<(i32, [f64; 3])>,
    /// `Level.skyDarken` and `monsters_burn` at the time of day.
    sky_darken: i32,
    monsters_burn: bool,
    /// Block changes due later, like a button popping back out.
    scheduled: Vec<Scheduled>,
    /// How many players look into each open container.
    openers: HashMap<BlockPos, usize>,
}

/// A block that changes from one state to another at a game tick, if it is
/// still in the first.
#[derive(Debug, Clone, Copy)]
struct Scheduled {
    tick: u64,
    pos: BlockPos,
    from: u32,
    to: u32,
}

impl Game {
    fn new(mut context: Context) -> Self {
        let air = context
            .data
            .default_state("minecraft:air")
            .expect("air exists");
        let clock = |name| {
            context
                .data
                .registry_id("minecraft:world_clock", name)
                .expect("vanilla clocks")
        };
        Self {
            overworld_clock: clock("minecraft:overworld"),
            end_clock: clock("minecraft:the_end"),
            player_type: context
                .data
                .entity_type("minecraft:player")
                .expect("vanilla entity types"),
            item_type: context
                .data
                .entity_type("minecraft:item")
                .expect("vanilla entity types"),
            items: HashMap::new(),
            mobs: HashMap::new(),
            arrows: HashMap::new(),
            orbs: HashMap::new(),
            stored: std::mem::take(&mut context.entities),
            foreign: HashMap::new(),
            explosion_knockback: Vec::new(),
            sky_darken: 0,
            monsters_burn: false,
            scheduled: Vec::new(),
            openers: HashMap::new(),
            context,
            players: HashMap::new(),
            air,
            started: Instant::now(),
            tick: 0,
        }
    }

    fn run(mut self, events: &Receiver<Event>) {
        let mut durations = Vec::with_capacity(STATS_WINDOW);
        let mut next = Instant::now();
        loop {
            let begin = Instant::now();
            loop {
                match events.try_recv() {
                    Ok(Event::Shutdown(done)) => {
                        self.shut_down();
                        let _ = done.send(());
                        return;
                    }
                    Ok(event) => self.handle(event, begin),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.save_all();
                        return;
                    }
                }
            }
            self.keep_alives(begin);
            self.tick_players();
            self.tick_scheduled();
            self.tick_items();
            self.tick_mobs();
            self.tick_arrows();
            self.tick_orbs();
            self.tick_furnaces();
            self.send_light_updates();
            self.sync_entities();
            self.advance_time();
            if self.tick.is_multiple_of(LATENCY_INTERVAL) && !self.players.is_empty() {
                self.send_latencies();
            }
            if self.tick > 0 && self.tick.is_multiple_of(AUTOSAVE_INTERVAL) {
                self.save_all();
            }

            if durations.len() == STATS_WINDOW {
                durations.remove(0);
            }
            durations.push(begin.elapsed());
            let mean = durations.iter().sum::<Duration>() / durations.len() as u32;
            let stats = &self.context.stats;
            stats
                .mspt_bits
                .store((mean.as_secs_f32() * 1000.0).to_bits(), Ordering::Relaxed);
            self.tick += 1;
            if self.tick.is_multiple_of(600) {
                debug!(
                    tick = self.tick,
                    mspt = stats.mspt(),
                    players = self.players.len(),
                    monsters = self.mobs.values().filter(|m| m.kind.hostile()).count(),
                    animals = self.mobs.values().filter(|m| !m.kind.hostile()).count(),
                    items = self.items.len(),
                    stored = self.stored.len(),
                    "tick stats"
                );
            }

            next += TICK;
            let now = Instant::now();
            if now > next + MAX_LAG {
                let behind = now - next;
                warn!(
                    "Can't keep up! Running {}ms or {} ticks behind",
                    behind.as_millis(),
                    behind.as_millis() / TICK.as_millis()
                );
                next = now;
            }
            std::thread::sleep(next.saturating_duration_since(now));
        }
    }

    /// Sends to every player but `except`.
    fn broadcast(&self, except: Option<i32>, message: &Outbound) {
        for player in self.players.values() {
            if Some(player.entity_id) != except {
                player.send(message.clone());
            }
        }
    }

    fn broadcast_packet<P: Packet>(&self, except: Option<i32>, packet: &P) {
        self.broadcast(except, &Outbound::Packet(Arc::new(Encoded::new(packet))));
    }

    /// Packets about `entity_id` for everyone who sees it.
    fn broadcast_entity(&mut self, entity_id: i32, packets: Vec<Encoded>) {
        let player_type = self.player_type;
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        let message = Outbound::Entity {
            state: player.state(player_type),
            packets: packets.into(),
        };
        self.broadcast(Some(entity_id), &message);
    }

    fn handle(&mut self, event: Event, now: Instant) {
        match event {
            Event::Joined(joined) => self.join(*joined, now),
            Event::Left { entity_id } => self.remove(entity_id),
            Event::KeepAlive { entity_id, id } => {
                let Some(player) = self.players.get_mut(&entity_id) else {
                    return;
                };
                if player.pending == Some(id) {
                    player.pending = None;
                    // Vanilla smooths the ping like this.
                    let rtt = (now - player.last_keep_alive).as_millis() as i32;
                    player.latency = (player.latency * 3 + rtt) / 4;
                } else {
                    // Vanilla kicks for a wrong id; an answer to an id we never
                    // sent can only come from a broken client.
                    player.kick("Ungültiges Keep-Alive");
                }
            }
            Event::Moved {
                entity_id,
                pos,
                rot,
                on_ground,
            } => {
                let Some(player) = self.players.get_mut(&entity_id) else {
                    return;
                };
                let before = player.pos;
                let was_on_ground = player.on_ground;
                if let Some(pos) = pos {
                    player.pos = pos;
                }
                if let Some([yaw, pitch]) = rot {
                    player.yaw = yaw;
                    player.pitch = pitch;
                }
                player.on_ground = on_ground;
                let delta = std::array::from_fn(|i| player.pos[i] - before[i]);
                self.moved(entity_id, delta, was_on_ground, on_ground);
            }
            Event::Sneaking {
                entity_id,
                sneaking,
            } => self.update_data(entity_id, |p| p.sneaking = sneaking),
            Event::Sprinting {
                entity_id,
                sprinting,
            } => self.update_data(entity_id, |p| p.sprinting = sprinting),
            Event::Flying { entity_id, flying } => {
                self.update_data(entity_id, |p| p.flying = flying)
            }
            Event::Settings {
                entity_id,
                skin_parts,
                main_hand,
            } => self.update_data(entity_id, |p| {
                p.skin_parts = skin_parts;
                p.main_hand = main_hand;
            }),
            Event::Swing { entity_id } => {
                let tick = self.tick;
                if let Some(player) = self.players.get_mut(&entity_id) {
                    player.last_swing = tick;
                }
                self.swing(entity_id);
            }
            Event::CreativeSlot {
                entity_id,
                slot,
                stack,
            } => self.creative_slot(entity_id, slot, stack),
            Event::SelectSlot { entity_id, slot } => {
                if let Ok(slot @ 0..=8) = usize::try_from(slot) {
                    self.stop_using(entity_id);
                    // A new item in hand starts its swing anew.
                    let tick = self.tick;
                    if let Some(player) = self.players.get_mut(&entity_id) {
                        player.last_swing = tick;
                    }
                    self.update_inventory(entity_id, |i| i.select(slot));
                }
            }
            Event::UseItemOn { entity_id, click } => self.use_item_on(entity_id, &click),
            Event::Action { entity_id, action } => self.player_action(entity_id, &action),
            Event::ChangeGameMode { entity_id, mode } => {
                let allowed = self
                    .players
                    .get(&entity_id)
                    .is_some_and(|p| p.permission >= OPERATOR);
                if let (true, Ok(mode)) = (allowed, GameMode::from_id(mode)) {
                    self.set_game_mode(entity_id, mode);
                }
            }
            Event::Chat { entity_id, message } => self.chat(entity_id, &message),
            Event::Command { entity_id, command } => self.command(entity_id, &command),
            Event::ContainerClick { entity_id, click } => self.container_click(entity_id, &click),
            Event::ContainerClose {
                entity_id,
                container_id,
            } => {
                let open = self
                    .players
                    .get(&entity_id)
                    .map(|p| p.menu.as_ref().map_or(0, |m| m.id));
                if open == Some(container_id) {
                    self.close_menu(entity_id, true);
                }
            }
            Event::SignUpdate { entity_id, update } => self.sign_update(entity_id, &update),
            Event::PickBlock {
                entity_id,
                pos,
                include_data,
            } => self.pick_block(entity_id, pos, include_data),
            Event::UseItem {
                entity_id,
                hand,
                sequence,
            } => self.use_item(entity_id, hand, sequence),
            Event::Respawn { entity_id } => self.respawn(entity_id),
            Event::Attack { entity_id, target } => self.attack(entity_id, target),
            Event::Interact {
                entity_id,
                target,
                hand,
            } => self.interact(entity_id, target, hand),
            Event::Shutdown(_) => unreachable!("handled by the game loop"),
        }
    }

    fn join(&mut self, joined: Joined, now: Instant) {
        let uuid = joined.profile.uuid;
        // Like vanilla, a second login with the same profile replaces the first.
        let twins: Vec<i32> = self
            .players
            .values()
            .filter(|p| p.uuid() == uuid)
            .map(|p| p.entity_id)
            .collect();
        for twin in twins {
            if let Some(player) = self.players.get_mut(&twin) {
                player.kick("Du hast dich von einem anderen Ort aus angemeldet.");
            }
            self.remove(twin);
        }

        let name = joined.profile.name.clone();
        info!(%name, entity_id = joined.entity_id, "joined the game");
        let mut player = Player {
            entity_id: joined.entity_id,
            info: PlayerInfoEntry {
                profile: joined.profile,
                game_mode: joined.game_mode,
                listed: true,
                latency: 0,
                list_order: 0,
                show_hat: true,
            },
            outbound: joined.outbound,
            last_keep_alive: now,
            pending: None,
            kicked: false,
            latency: 0,
            permission: joined.permission,
            pos: joined.pos,
            yaw: joined.rot[0],
            pitch: joined.rot[1],
            on_ground: true,
            sneaking: false,
            sprinting: false,
            flying: false,
            skin_parts: joined.skin_parts,
            main_hand: joined.main_hand,
            inventory: joined.inventory,
            tracker: Tracker::new(
                &tracker::Pose {
                    pos: joined.pos,
                    yaw: joined.rot[0],
                    pitch: joined.rot[1],
                    head_yaw: joined.rot[0],
                    on_ground: true,
                },
                PLAYER_UPDATE_INTERVAL,
            ),
            data_dirty: false,
            state: None,
            state_id: 0,
            menu: None,
            inventory_menu: Menu::inventory(),
            carried: None,
            drag: Drag::default(),
            ender: {
                let mut ender = vec![None; ENDER_SLOTS];
                for (slot, stack) in joined.ender {
                    if let Some(target) = ender.get_mut(slot) {
                        *target = Some(stack);
                    }
                }
                ender
            },
            window: 0,
            digging: None,
            health: joined.health,
            food: Food::default(),
            fall_distance: 0.0,
            invulnerable: 0,
            dead: joined.health <= 0.0,
            using: None,
            last_swing: 0,
            experience: joined.experience,
            xp_delay: 0,
            last_level_up: 0,
        };

        // The newcomer learns about everyone, everyone about the newcomer.
        let player_type = self.player_type;
        for other in self.players.values_mut() {
            player.send(Outbound::PlayerAdded {
                info: Box::new(other.info.clone()),
                state: other.state(player_type),
            });
        }
        let item_type = self.item_type;
        for item in self.items.values_mut() {
            player.send(Outbound::EntityAdded(item.state(item_type)));
        }
        for mob in self.mobs.values_mut().filter(|m| m.alive()) {
            player.send(Outbound::EntityAdded(mob.state()));
        }
        for state in self.projectile_states() {
            player.send(Outbound::EntityAdded(state));
        }
        let added = Outbound::PlayerAdded {
            info: Box::new(player.info.clone()),
            state: player.state(player_type),
        };
        self.broadcast(None, &added);
        let uuid = player.uuid();
        player.send(Outbound::Packet(Arc::new(self.time_packet())));
        let entity_id = player.entity_id;
        self.players.insert(entity_id, player);
        self.send_health(entity_id);
        self.send_experience(entity_id);
        self.send_difficulty(Some(entity_id));
        if self.players.get(&entity_id).is_some_and(|p| p.dead) {
            // Saved dead: straight to the death screen.
            self.die(
                entity_id,
                text::translate(
                    "death.attack.generic",
                    vec![text::player_name(&name, uuid)],
                    None,
                ),
            );
        }
        self.broadcast_packet(
            None,
            &SystemChat {
                content: text::translate(
                    "multiplayer.player.joined",
                    vec![text::player_name(&name, uuid)],
                    Some("yellow"),
                ),
                overlay: false,
            },
        );
    }

    fn remove(&mut self, entity_id: i32) {
        let Some(player) = self.players.remove(&entity_id) else {
            return;
        };
        info!(name = %player.name(), "left the game");
        let mut player = player;
        // What is in the crafting grid or on the mouse goes back into the
        // inventory, as when closing the screen.
        if let Some(menu) = player.menu.take() {
            for stack in menu.grid.into_iter().flatten() {
                let max = self
                    .context
                    .data
                    .item(stack.item)
                    .map_or(64, |i| i.stack.max(1));
                player.inventory.add(stack, max);
            }
        }
        for stack in player
            .inventory_menu
            .grid
            .iter_mut()
            .filter_map(Option::take)
            .chain(player.carried.take())
        {
            let max = self
                .context
                .data
                .item(stack.item)
                .map_or(64, |i| i.stack.max(1));
            player.inventory.add(stack, max);
        }
        self.save_player(&player);
        self.broadcast(
            None,
            &Outbound::PlayerRemoved {
                entity_id,
                uuid: player.uuid(),
            },
        );
        self.broadcast_packet(
            None,
            &SystemChat {
                content: text::translate(
                    "multiplayer.player.left",
                    vec![text::player_name(player.name(), player.uuid())],
                    Some("yellow"),
                ),
                overlay: false,
            },
        );
    }

    fn update_data(&mut self, entity_id: i32, change: impl FnOnce(&mut Player)) {
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        let before = player.changeable_data();
        let skin_parts = player.skin_parts;
        change(player);
        if player.changeable_data() == before {
            return;
        }
        player.data_dirty = true;
        player.state = None;
        if player.skin_parts != skin_parts {
            // The player's own client takes its skin layers in third person
            // from here too.
            player.send(Outbound::Packet(Arc::new(Encoded::new(&SetEntityData {
                entity_id,
                values: EntityDataValues(vec![EntityDataValue {
                    index: data_index::SKIN_PARTS,
                    value: EntityData::Byte(player.skin_parts),
                }]),
            }))));
        }
    }

    /// Changes the inventory and shows others what that changed of the
    /// player's equipment.
    fn update_inventory(&mut self, entity_id: i32, change: impl FnOnce(&mut Inventory)) {
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        let before = player.inventory.visible();
        change(&mut player.inventory);
        let changed: Vec<_> = player
            .inventory
            .visible()
            .into_iter()
            .zip(before)
            .filter(|(now, before)| now != before)
            .map(|(now, _)| now)
            .collect();
        if !changed.is_empty() {
            player.state = None;
            let packet = Encoded::new(&SetEquipment {
                entity_id,
                slots: changed,
            });
            self.broadcast_entity(entity_id, vec![packet]);
        }
    }

    fn save_player(&self, player: &Player) {
        let Some(storage) = &self.context.storage else {
            return;
        };
        if let Err(err) = storage.save_player(player.uuid(), &player.saved(), &self.context.data) {
            warn!(name = %player.name(), "saving the player failed: {err:#}");
        }
    }

    /// Vanilla's autosave: every player, the changed chunks and the
    /// entities.
    fn save_all(&mut self) {
        let Some(storage) = self.context.storage.clone() else {
            return;
        };
        self.save_entities(&storage);
        for player in self.players.values() {
            self.save_player(player);
        }
        let difficulty = command::DIFFICULTIES[usize::from(self.context.difficulty.min(3))];
        if let Err(err) = self.context.level.save(&storage, LAYERS, BIOME, difficulty) {
            warn!("saving level.dat failed: {err:#}");
        }
        match self.context.world.save(&storage, &self.context.data) {
            Ok(0) => {}
            Ok(chunks) => debug!(chunks, "saved the world"),
            Err(err) => warn!("saving the world failed: {err:#}"),
        }
    }

    fn shut_down(&mut self) {
        for player in self.players.values_mut() {
            player.kick("Server geschlossen");
        }
        self.save_all();
        info!("saved, stopping");
        self.players.clear();
    }

    fn swing(&mut self, entity_id: i32) {
        // A whack of the main hand, as `SwingAnimation.DEFAULT`.
        let packet = Encoded::new(&SwingAnimation {
            entity_id,
            hand: 0,
            animation: 1,
            duration: 6,
        });
        self.broadcast_entity(entity_id, vec![packet]);
    }

    fn sync_entities(&mut self) {
        let ids: Vec<i32> = self.players.keys().copied().collect();
        for entity_id in ids {
            let packets = match self.players.get_mut(&entity_id) {
                Some(player) => {
                    let pose = player.pose();
                    let dirty = std::mem::take(&mut player.data_dirty);
                    let mut packets = player.tracker.sync(entity_id, &pose, dirty);
                    if dirty {
                        packets.push(Encoded::new(&SetEntityData {
                            entity_id,
                            values: EntityDataValues(player.changeable_data()),
                        }));
                    }
                    if !packets.is_empty() {
                        player.state = None;
                    }
                    packets
                }
                None => continue,
            };
            if !packets.is_empty() {
                self.broadcast_entity(entity_id, packets);
            }
        }
    }

    fn send_latencies(&self) {
        let entries = self
            .players
            .values()
            .map(|p| (p.uuid(), p.latency))
            .collect();
        self.broadcast_packet(None, &PlayerInfoLatency { entries });
    }

    /// Day and night go on; every second the clients hear the game time, as
    /// from vanilla, and keep their clocks running in between.
    fn advance_time(&mut self) {
        let level = &mut self.context.level;
        level.game_time += 1;
        level.day_time += 1;
        let time = level.day_time.rem_euclid(24000);
        self.sky_darken = sky_darken(time);
        self.monsters_burn = monsters_burn(time);
        let level = &self.context.level;
        if level.game_time % 20 == 0 && !self.players.is_empty() {
            let packet = SetTime {
                game_time: level.game_time,
                clocks: Vec::new(),
            };
            self.broadcast_packet(None, &packet);
        }
    }

    /// The full time with the clocks, for a new player or after `/time`.
    fn time_packet(&self) -> Encoded {
        let level = &self.context.level;
        let clock = |ticks| ClockState {
            total_ticks: ticks,
            partial_tick: 0.0,
            rate: 1.0,
        };
        Encoded::new(&SetTime {
            game_time: level.game_time,
            clocks: vec![
                (self.overworld_clock, clock(level.day_time)),
                (self.end_clock, clock(level.game_time)),
            ],
        })
    }

    /// Light that block changes moved, to the clients that have the chunk.
    fn send_light_updates(&self) {
        for ((x, z), light) in self.context.world.take_light_updates() {
            let packet = Arc::new(Encoded::new(&LightUpdate { x, z, light }));
            self.broadcast(
                None,
                &Outbound::InChunk {
                    chunk: (x, z),
                    packet,
                },
            );
        }
    }

    fn set_block(&self, pos: BlockPos, state: u32) {
        if self.context.world.set_block(pos, state) {
            self.broadcast(None, &Outbound::BlockChanged { pos, state });
        }
    }

    /// Like vanilla after a block action: the true state of the blocks the
    /// client may have predicted wrongly, then the acknowledgement.
    fn finish_block_action(&self, entity_id: i32, sequence: i32, refresh: &[BlockPos]) {
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        for &pos in refresh {
            player.send(Outbound::BlockChanged {
                pos,
                state: self.context.world.block(pos),
            });
        }
        player.send(Outbound::BlockAck(sequence));
    }

    fn chat(&mut self, entity_id: i32, message: &str) {
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        // `ServerGamePacketListenerImpl.isChatMessageIllegal`.
        if message
            .chars()
            .any(|c| c == '§' || c < ' ' || c == '\u{7f}')
        {
            player.kick("Unerlaubte Zeichen im Chat");
            return;
        }
        info!("<{}> {message}", player.name());
        let content = text::translate(
            "chat.type.text",
            vec![
                text::player_name(player.name(), player.uuid()),
                text::plain(message),
            ],
            None,
        );
        self.broadcast_packet(
            None,
            &SystemChat {
                content,
                overlay: false,
            },
        );
    }

    /// The difficulty, to one player or everyone.
    fn send_difficulty(&self, to: Option<i32>) {
        let packet = Outbound::Packet(Arc::new(Encoded::new(&ChangeDifficulty {
            difficulty: i32::from(self.context.difficulty),
            locked: false,
        })));
        match to {
            Some(id) => {
                if let Some(player) = self.players.get(&id) {
                    player.send(packet);
                }
            }
            None => self.broadcast(None, &packet),
        }
    }

    /// Everything thrown or dropped that a new player should see besides
    /// items and mobs: arrows and experience orbs.
    fn projectile_states(&mut self) -> Vec<Arc<EntityState>> {
        let arrows = self.arrow_states();
        arrows.into_iter().chain(self.orb_states()).collect()
    }

    /// Vanilla's rule (`ServerCommonPacketListenerImpl.keepConnectionAlive`):
    /// send a keep-alive every interval; if the last one is still unanswered
    /// after another interval, the player timed out.
    fn keep_alives(&mut self, now: Instant) {
        let interval = self.context.keep_alive;
        for player in self.players.values_mut() {
            if now - player.last_keep_alive < interval {
                continue;
            }
            if player.pending.is_some() {
                player.kick("Zeitüberschreitung");
                continue;
            }
            let id = (now - self.started).as_millis() as i64;
            player.pending = Some(id);
            player.last_keep_alive = now;
            player.send(Outbound::KeepAlive(id));
        }
    }
}

/// A track of `timeline/day.json`: keyframes `(tick, value)` over a day,
/// looping, linearly between them (`KeyframeTrackSampler`).
fn sample(keyframes: &[(i64, f32)], time: i64) -> f32 {
    let period = 24000;
    let (first, last) = (keyframes[0], keyframes[keyframes.len() - 1]);
    let mut segments = vec![((last.0 - period, last.1), first)];
    segments.extend(keyframes.windows(2).map(|w| (w[0], w[1])));
    segments.push((last, (first.0 + period, first.1)));
    let ((from, a), (to, b)) = segments
        .iter()
        .copied()
        .find(|&(_, (to, _))| time < to)
        .unwrap_or(segments[segments.len() - 1]);
    if time <= from {
        a
    } else if time >= to {
        b
    } else {
        a + (b - a) * ((time - from) as f32 / (to - from) as f32)
    }
}

/// `Level.updateSkyBrightness`: 15 less the sky light level, which
/// `gameplay/sky_light_level` scales from full by day to 4 at night.
fn sky_darken(time: i64) -> i32 {
    const SKY_LIGHT: [(i64, f32); 4] = [
        (133, 1.0),
        (11867, 1.0),
        (13670, 0.266_666_68),
        (22330, 0.266_666_68),
    ];
    (15.0 - 15.0 * sample(&SKY_LIGHT, time)) as i32
}

/// `gameplay/monsters_burn`: from just before sunrise until dusk.
fn monsters_burn(time: i64) -> bool {
    !(12542..23460).contains(&time)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_and_night_like_the_timeline() {
        assert_eq!(sky_darken(6000), 0, "noon");
        assert_eq!(sky_darken(18000), 11, "midnight");
        assert_eq!(sky_darken(0), 0, "just before the day starts");
        assert!((1..11).contains(&sky_darken(12800)), "dusk");
        assert!(monsters_burn(1000) && monsters_burn(23500));
        assert!(!monsters_burn(13000) && !monsters_burn(18000));
    }
}
