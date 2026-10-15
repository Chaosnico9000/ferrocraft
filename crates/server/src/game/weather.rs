//! Weather as `ServerLevel.advanceWeatherCycle` runs it: clear spells,
//! rain and thunderstorms come and go at random; rain and thunder fade in
//! and out over five seconds, which clients are told. During a storm
//! lightning strikes now and then near players (`tickThunder`): it sets
//! fire and hurts and ignites what it hits. Rain fills cauldrons, keeps
//! farmland wet and puts out fire and burning entities. `/weather` sets
//! it; it is saved in `data/minecraft/weather.dat`.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{GameEvent, GameEventKind};
use rand::Rng;
use uuid::Uuid;

use super::blocks::{SIMULATION_DISTANCE, below};
use super::tracker::{Pose, Tracker};
use super::{EntityState, Game, Outbound};
use crate::data::Aabb;
use crate::storage::{DATA_VERSION, Storage, read_nbt_file, write_nbt_file};
use crate::world::{MAX_Y, MIN_Y};

/// `RAIN_DELAY`, `RAIN_DURATION`, `THUNDER_DELAY`, `THUNDER_DURATION`.
const RAIN_DELAY: (i32, i32) = (12000, 180000);
const RAIN_DURATION: (i32, i32) = (12000, 24000);
const THUNDER_DELAY: (i32, i32) = (12000, 180000);
const THUNDER_DURATION: (i32, i32) = (3600, 15600);

/// `WeatherData`: what is saved.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Weather {
    pub clear_time: i32,
    pub rain_time: i32,
    pub thunder_time: i32,
    pub raining: bool,
    pub thundering: bool,
}

impl Weather {
    const FILE: &'static str = "data/minecraft/weather.dat";

    pub fn load(storage: &Storage) -> anyhow::Result<Self> {
        let Some(root) = read_nbt_file(&storage.path(Self::FILE))? else {
            return Ok(Self::default());
        };
        let data = root.get("data").unwrap_or(&root);
        let int = |key| match data.get(key) {
            Some(Nbt::Int(v)) => *v,
            _ => 0,
        };
        let flag = |key| matches!(data.get(key), Some(Nbt::Byte(1)));
        Ok(Self {
            clear_time: int("clear_weather_time"),
            rain_time: int("rain_time"),
            thunder_time: int("thunder_time"),
            raining: flag("raining"),
            thundering: flag("thundering"),
        })
    }

    pub fn save(&self, storage: &Storage) -> anyhow::Result<()> {
        let nbt = Nbt::Compound(vec![
            (
                "data".into(),
                Nbt::Compound(vec![
                    ("clear_weather_time".into(), Nbt::Int(self.clear_time)),
                    ("rain_time".into(), Nbt::Int(self.rain_time)),
                    ("thunder_time".into(), Nbt::Int(self.thunder_time)),
                    ("raining".into(), Nbt::Byte(i8::from(self.raining))),
                    ("thundering".into(), Nbt::Byte(i8::from(self.thundering))),
                ]),
            ),
            ("DataVersion".into(), Nbt::Int(DATA_VERSION)),
        ]);
        write_nbt_file(&storage.path(Self::FILE), &nbt)
    }
}

/// How far rain and thunder have faded in, 0 to 1.
#[derive(Debug, Clone, Copy, Default)]
pub struct Levels {
    pub rain: f32,
    pub thunder: f32,
}

/// A lightning bolt (`LightningBolt`): it flashes a few times and is gone.
pub struct Lightning {
    entity_id: i32,
    pos: [f64; 3],
    life: i32,
    flashes: i32,
    visual_only: bool,
    hit: Vec<i32>,
}

fn sample(range: (i32, i32)) -> i32 {
    rand::rng().random_range(range.0..=range.1)
}

impl Game {
    /// `Level.isRaining`.
    pub(super) fn raining(&self) -> bool {
        self.weather_levels.rain > 0.2
    }

    /// `Level.isThundering`.
    pub(super) fn thundering(&self) -> bool {
        self.weather_levels.thunder > 0.9
    }

    /// The top of the motion-blocking heightmap: the first position above
    /// every block that stops motion or holds a fluid.
    pub(super) fn motion_blocking_height(&self, x: i32, z: i32) -> i32 {
        let world = &self.context.world;
        let data = &self.context.data;
        let mut y = world.top_y(x, z).min(MAX_Y);
        while y >= MIN_Y {
            let state = world.block(BlockPos { x, y, z });
            if data.state_in_tag(state, "minecraft:blocks_motion_in_heightmap")
                || data.physics(state).fluid != 0
            {
                return y + 1;
            }
            y -= 1;
        }
        MIN_Y
    }

    /// `Level.isRainingAt`: rain falls on this block. The plains are never
    /// cold enough for snow.
    pub(super) fn is_raining_at(&self, pos: BlockPos) -> bool {
        self.raining() && self.motion_blocking_height(pos.x, pos.z) <= pos.y
    }

    /// `ServerLevel.advanceWeatherCycle`.
    pub(super) fn advance_weather(&mut self) {
        let was_raining = self.raining();
        if self.rules.flag("minecraft:advance_weather") {
            let w = &mut self.weather;
            if w.clear_time > 0 {
                w.clear_time -= 1;
                w.thunder_time = if w.thundering { 0 } else { 1 };
                w.rain_time = if w.raining { 0 } else { 1 };
                w.thundering = false;
                w.raining = false;
            } else {
                if w.thunder_time > 0 {
                    w.thunder_time -= 1;
                    if w.thunder_time == 0 {
                        w.thundering = !w.thundering;
                    }
                } else if w.thundering {
                    w.thunder_time = sample(THUNDER_DURATION);
                } else {
                    w.thunder_time = sample(THUNDER_DELAY);
                }
                if w.rain_time > 0 {
                    w.rain_time -= 1;
                    if w.rain_time == 0 {
                        w.raining = !w.raining;
                    }
                } else if w.raining {
                    w.rain_time = sample(RAIN_DURATION);
                } else {
                    w.rain_time = sample(RAIN_DELAY);
                }
            }
        }
        let before = self.weather_levels;
        let step = |level: f32, on: bool| (level + if on { 0.01 } else { -0.01 }).clamp(0.0, 1.0);
        self.weather_levels.thunder = step(before.thunder, self.weather.thundering);
        self.weather_levels.rain = step(before.rain, self.weather.raining);
        let now = self.weather_levels;
        if now.rain != before.rain {
            self.weather_event(GameEventKind::RainLevelChange, now.rain);
        }
        if now.thunder != before.thunder {
            self.weather_event(GameEventKind::ThunderLevelChange, now.thunder);
        }
        if was_raining != self.raining() {
            let kind = if was_raining {
                GameEventKind::StopRaining
            } else {
                GameEventKind::StartRaining
            };
            self.weather_event(kind, 0.0);
            self.weather_event(GameEventKind::RainLevelChange, now.rain);
            self.weather_event(GameEventKind::ThunderLevelChange, now.thunder);
        }
    }

    fn weather_event(&self, kind: GameEventKind, value: f32) {
        self.broadcast_packet(None, &GameEvent::new(kind, value));
    }

    /// `PlayerList.sendLevelInfo`: a player who joins in the rain sees it.
    pub(super) fn send_weather(&self, entity_id: i32) {
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        if !self.raining() {
            return;
        }
        let levels = self.weather_levels;
        for (event, value) in [
            (GameEventKind::StartRaining, 0.0),
            (GameEventKind::RainLevelChange, levels.rain),
            (GameEventKind::ThunderLevelChange, levels.thunder),
        ] {
            player.send(Outbound::Packet(Arc::new(super::Encoded::new(
                &GameEvent::new(event, value),
            ))));
        }
    }

    /// `/weather clear|rain|thunder`: for `duration` ticks, or a random
    /// time (`WeatherCommand`, `setWeatherParameters`).
    pub(super) fn set_weather(&mut self, kind: &str, duration: Option<i32>) {
        let w = &mut self.weather;
        match kind {
            "clear" => {
                w.clear_time = duration.unwrap_or_else(|| sample((12000, 180000)));
                w.rain_time = 0;
                w.thunder_time = 0;
                w.raining = false;
                w.thundering = false;
            }
            "rain" => {
                let time = duration.unwrap_or_else(|| sample(RAIN_DURATION));
                w.clear_time = 0;
                w.rain_time = time;
                w.thunder_time = time;
                w.raining = true;
                w.thundering = false;
            }
            _ => {
                let time = duration.unwrap_or_else(|| sample(THUNDER_DURATION));
                w.clear_time = 0;
                w.rain_time = time;
                w.thunder_time = time;
                w.raining = true;
                w.thundering = true;
            }
        }
    }

    pub(super) fn save_weather(&self, storage: &Storage) {
        if let Err(err) = self.weather.save(storage) {
            tracing::warn!("saving the weather failed: {err:#}");
        }
    }

    /// `tickPrecipitation` and `tickThunder` for the chunks near players:
    /// cauldrons fill in the rain, lightning strikes in storms.
    pub(super) fn tick_precipitation(&mut self) {
        if self.players.is_empty() || !self.raining() {
            return;
        }
        let mut rng = rand::rng();
        let mut chunks: Vec<(i32, i32)> = Vec::new();
        for player in self.players.values() {
            let (cx, cz) = (
                (player.pos[0].floor() as i32) >> 4,
                (player.pos[2].floor() as i32) >> 4,
            );
            for x in cx - SIMULATION_DISTANCE..=cx + SIMULATION_DISTANCE {
                for z in cz - SIMULATION_DISTANCE..=cz + SIMULATION_DISTANCE {
                    chunks.push((x, z));
                }
            }
        }
        chunks.sort_unstable();
        chunks.dedup();
        let thundering = self.thundering();
        for (cx, cz) in chunks {
            if thundering && rng.random_range(0..100_000) == 0 {
                let x = cx * 16 + rng.random_range(0..16);
                let z = cz * 16 + rng.random_range(0..16);
                let target = self.lightning_target(x, z);
                if self.is_raining_at(target) {
                    self.strike(target, false);
                }
            }
            if rng.random_range(0..48) == 0 {
                let x = cx * 16 + rng.random_range(0..16);
                let z = cz * 16 + rng.random_range(0..16);
                let top = self.motion_blocking_height(x, z);
                self.rain_on(below(BlockPos { x, y: top, z }));
            }
        }
    }

    /// `handlePrecipitation`: a cauldron catches some rain.
    fn rain_on(&mut self, pos: BlockPos) {
        let state = self.context.world.block(pos);
        if rand::rng().random::<f32>() >= 0.05 {
            return;
        }
        if self.is(state, "minecraft:cauldron") {
            let water = self.default_of("minecraft:water_cauldron");
            self.set_block(pos, water);
        } else if self.is(state, "minecraft:water_cauldron") {
            let level = self.int_value(state, "level").unwrap_or(3);
            if level < 3 {
                let fuller = self.with(state, "level", &(level + 1).to_string());
                self.set_block(pos, fuller);
            }
        }
    }

    /// `findLightningTargetAround` without lightning rods: a living thing
    /// out in the rain near the column, or the top of the column.
    fn lightning_target(&self, x: i32, z: i32) -> BlockPos {
        let top = BlockPos {
            x,
            y: self.motion_blocking_height(x, z),
            z,
        };
        let (fx, fz) = (f64::from(x), f64::from(z));
        let area: Aabb = [
            fx - 3.0,
            f64::from(top.y) - 3.0,
            fz - 3.0,
            fx + 4.0,
            f64::from(MAX_Y) + 5.0,
            fz + 4.0,
        ];
        let inside = |b: Aabb| (0..3).all(|i| b[i] < area[i + 3] && b[i + 3] > area[i]);
        let mut candidates: Vec<BlockPos> = self
            .players
            .values()
            .filter(|p| !p.dead && inside(p.bounds()))
            .map(|p| block_of(p.pos))
            .chain(
                self.mobs
                    .values()
                    .filter(|m| m.alive() && inside(m.bounds()))
                    .map(|m| block_of(m.pos)),
            )
            .filter(|&p| self.motion_blocking_height(p.x, p.z) <= p.y)
            .collect();
        candidates.sort_unstable_by_key(|p| (p.x, p.y, p.z));
        if candidates.is_empty() {
            return top;
        }
        candidates[rand::rng().random_range(0..candidates.len())]
    }

    /// A lightning bolt at the bottom centre of `pos`.
    pub(super) fn strike(&mut self, pos: BlockPos, visual_only: bool) {
        let entity_id = self.context.entity_ids.fetch_add(1, Ordering::Relaxed);
        let at = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        let Ok(entity_type) = self.context.data.entity_type("minecraft:lightning_bolt") else {
            return;
        };
        let pose = Pose {
            pos: at,
            yaw: 0.0,
            pitch: 0.0,
            head_yaw: 0.0,
            on_ground: false,
        };
        let tracker = Tracker::new(&pose, 1);
        let state = Arc::new(EntityState {
            entity_id,
            uuid: Uuid::new_v4(),
            entity_type,
            pos: tracker.sent_pos,
            yaw: 0,
            pitch: 0,
            head_yaw: 0,
            velocity: [0.0; 3],
            object_data: 0,
            data: Vec::new(),
            equipment: Vec::new(),
            spectator: false,
            // `clientTrackingRange` 16 chunks.
            range: 16.0 * 16.0,
        });
        self.broadcast(None, &Outbound::EntityAdded(state));
        self.lightning.push(Lightning {
            entity_id,
            pos: at,
            life: 2,
            flashes: rand::rng().random_range(0..3) + 1,
            visual_only,
            hit: Vec::new(),
        });
    }

    /// `LightningBolt.tick` for every bolt.
    pub(super) fn tick_lightning(&mut self) {
        let mut bolts = std::mem::take(&mut self.lightning);
        let mut rng = rand::rng();
        bolts.retain_mut(|bolt| {
            let pos = block_of(bolt.pos);
            if bolt.life == 2 && !bolt.visual_only && self.context.difficulty >= 2 {
                self.lightning_fire(pos, 4);
            }
            bolt.life -= 1;
            if bolt.life < 0 {
                if bolt.flashes == 0 {
                    self.broadcast(None, &Outbound::EntityRemoved(bolt.entity_id));
                    return false;
                } else if bolt.life < -rng.random_range(0..10) {
                    bolt.flashes -= 1;
                    bolt.life = 1;
                    if !bolt.visual_only {
                        self.lightning_fire(pos, 0);
                    }
                }
            }
            if bolt.life >= 0 && !bolt.visual_only {
                let [x, y, z] = bolt.pos;
                let area: Aabb = [x - 3.0, y - 3.0, z - 3.0, x + 3.0, y + 9.0, z + 3.0];
                self.thunder_hit_all(area, &mut bolt.hit);
            }
            true
        });
        bolts.append(&mut self.lightning);
        self.lightning = bolts;
    }

    /// `spawnFire`: fire where it struck and around.
    fn lightning_fire(&mut self, pos: BlockPos, extra: usize) {
        if !self.fire_may_spread(pos) {
            return;
        }
        let mut rng = rand::rng();
        let mut targets = vec![pos];
        for _ in 0..extra {
            targets.push(super::blocks::offset(
                pos,
                rng.random_range(-1..=1),
                rng.random_range(-1..=1),
                rng.random_range(-1..=1),
            ));
        }
        for target in targets {
            let fire = self.fire_state_at(target);
            if self.is_air(self.context.world.block(target)) && self.fire_survives(target, fire) {
                self.set_block(target, fire);
            }
        }
    }

    /// `Entity.thunderHit` for everything in `area` not hit before.
    fn thunder_hit_all(&mut self, area: Aabb, hit: &mut Vec<i32>) {
        let inside = |b: Aabb| (0..3).all(|i| b[i] < area[i + 3] && b[i + 3] > area[i]);
        let players: Vec<i32> = self
            .players
            .values()
            .filter(|p| !p.dead && inside(p.bounds()))
            .map(|p| p.entity_id)
            .collect();
        let mobs: Vec<i32> = self
            .mobs
            .values()
            .filter(|m| m.alive() && inside(m.bounds()))
            .map(|m| m.entity_id)
            .collect();
        let items: Vec<i32> = self
            .items
            .values()
            .filter(|i| inside(i.bounds()))
            .map(|i| i.entity_id)
            .collect();
        for id in players {
            if !hit.contains(&id) {
                hit.push(id);
            }
            self.player_thunder_hit(id);
        }
        for id in mobs {
            if !hit.contains(&id) {
                hit.push(id);
            }
            self.mob_thunder_hit(id);
        }
        for id in items {
            // Items have five health; lightning does five.
            self.remove_item(id);
        }
    }
}

fn block_of(pos: [f64; 3]) -> BlockPos {
    BlockPos {
        x: pos[0].floor() as i32,
        y: pos[1].floor() as i32,
        z: pos[2].floor() as i32,
    }
}
