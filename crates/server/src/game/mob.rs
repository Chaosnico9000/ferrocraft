//! Mobs: the animals (cows, pigs, sheep, chickens) and the monsters
//! (zombies, skeletons, creepers, spiders). What they share lives here: their
//! body, how they move along a path, get hurt, burn, die and drop their loot
//! and experience. What they want is in [`animal`] and [`monster`], where and
//! when they appear in [`spawn`], how they are saved in [`save`].

mod animal;
mod monster;
pub(super) mod save;
mod spawn;

pub(super) use monster::triangle;
pub use save::StoredEntities;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    Animate, DamageEvent, EntityData, EntityDataValue, EntityDataValues, EntityEvent, GameMode,
    ItemStack, LpVec3, SOUND_SOURCE_HOSTILE, SOUND_SOURCE_NEUTRAL, SetEntityData, SetEntityMotion,
    data_index, equipment_slot,
};
use rand::Rng;
use uuid::Uuid;

use super::path::{self, Walker};
use super::physics;
use super::tracker::{Pose, Tracker};
use super::{Encoded, EntityState, Game, Outbound};
use crate::data::{Aabb, GameData};
use crate::loot;
use crate::world::{MIN_Y, World};

/// `Sheep.DATA_WOOL_ID`, after `LivingEntity`'s values 8–14, `Mob`'s 15 and
/// `AgeableMob`'s 16–17.
const SHEEP_WOOL: u8 = 18;
/// `Mob.DATA_MOB_FLAGS_ID`: 0x04 aggressive, arms up.
const MOB_FLAGS: u8 = 15;
/// `Zombie.DATA_BABY_ID`.
const ZOMBIE_BABY: u8 = 16;
/// `Creeper.DATA_SWELL_DIR` and `DATA_IS_IGNITED`.
const CREEPER_SWELL_DIR: u8 = 16;
const CREEPER_IGNITED: u8 = 18;
/// `Spider.DATA_FLAGS_ID`: 0x01 climbing.
const SPIDER_FLAGS: u8 = 16;
/// Ticks a mob shows its death before it goes (`LivingEntity.tickDeath`).
const DEATH_TICKS: u32 = 20;
/// `invulnerableTime` after a hit; hits within the first half do nothing.
const HURT_COOLDOWN: u32 = 10;
/// How long a hit by a player counts for loot and experience
/// (`lastHurtByPlayerMemoryTime`).
const PLAYER_MEMORY: u64 = 100;
const AIR_DRAG: f64 = 0.98;
/// `LivingEntity.getFrictionInfluencedSpeed`'s constant.
const GROUND_ACCELERATION: f64 = 0.216_000_02;
/// `Entity.getJumpPower` with `jump_strength` 0.42.
const JUMP: f64 = 0.42;
/// `Attributes.STEP_HEIGHT` of every mob here.
const STEP_HEIGHT: f64 = 0.6;
/// `Attributes.SAFE_FALL_DISTANCE`.
const SAFE_FALL: f64 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Cow,
    Pig,
    Sheep,
    Chicken,
    Zombie,
    Skeleton,
    Creeper,
    Spider,
}

impl Kind {
    pub const ALL: [Kind; 8] = [
        Kind::Cow,
        Kind::Pig,
        Kind::Sheep,
        Kind::Chicken,
        Kind::Zombie,
        Kind::Skeleton,
        Kind::Creeper,
        Kind::Spider,
    ];

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Cow => "minecraft:cow",
            Kind::Pig => "minecraft:pig",
            Kind::Sheep => "minecraft:sheep",
            Kind::Chicken => "minecraft:chicken",
            Kind::Zombie => "minecraft:zombie",
            Kind::Skeleton => "minecraft:skeleton",
            Kind::Creeper => "minecraft:creeper",
            Kind::Spider => "minecraft:spider",
        }
    }

    fn sound(self) -> &'static str {
        self.name().trim_start_matches("minecraft:")
    }

    /// `MobCategory.MONSTER`.
    pub fn hostile(self) -> bool {
        matches!(
            self,
            Kind::Zombie | Kind::Skeleton | Kind::Creeper | Kind::Spider
        )
    }

    /// `Mob.getSoundVolume`: cows are quieter.
    fn volume(self) -> f32 {
        match self {
            Kind::Cow => 0.4,
            _ => 1.0,
        }
    }

    /// Creepers make no sound of their own until they hiss.
    fn ambient(self) -> bool {
        self != Kind::Creeper
    }

    /// `#minecraft:burn_in_daylight`.
    fn burns_in_daylight(self) -> bool {
        matches!(self, Kind::Zombie | Kind::Skeleton)
    }

    /// The speed of a wander (`WaterAvoidingRandomStrollGoal`).
    fn stroll_speed(self) -> f64 {
        match self {
            Kind::Creeper | Kind::Spider => 0.8,
            _ => 1.0,
        }
    }
}

/// `DyeColor` names by id.
const DYES: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

/// `Sheep.getRandomSheepColor` in a temperate biome.
fn random_wool(rng: &mut impl Rng) -> u8 {
    let roll = rng.random_range(0..100);
    if roll < 5 {
        15
    } else if roll < 10 {
        7
    } else if roll < 15 {
        8
    } else if roll < 18 {
        12
    } else if rng.random_range(0..500) == 0 {
        6
    } else {
        0
    }
}

/// Where a mob is heading.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Goal {
    Idle,
    /// Walking somewhere along a path, with a speed factor, until it gets
    /// there or gives up.
    Walk {
        target: [f64; 3],
        speed: f64,
    },
}

/// Where the head points.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Look {
    Ahead,
    At { player: i32, ticks: u32 },
    Around { yaw: f32, ticks: u32 },
}

/// A way to walk (`PathNavigation`).
#[derive(Debug, Clone, Default)]
struct Navigation {
    nodes: Vec<BlockPos>,
    index: usize,
    /// Where it leads.
    goal: Option<BlockPos>,
    /// Ticks spent on it, to give up when stuck.
    ticks: u32,
}

pub struct Mob {
    pub entity_id: i32,
    uuid: Uuid,
    pub kind: Kind,
    type_id: i32,
    pub pos: [f64; 3],
    pub velocity: [f64; 3],
    yaw: f32,
    head_yaw: f32,
    pitch: f32,
    on_ground: bool,
    horizontal_collision: bool,
    pub health: f32,
    max_health: f32,
    movement_speed: f64,
    gravity: f64,
    half_width: f64,
    height: f64,
    eye_height: f64,
    range: f64,
    follow_range: f64,
    armor: f32,
    attack_damage: f32,
    goal: Goal,
    look: Look,
    navigation: Navigation,
    /// This tick's walking speed factor, set by [`Game::steer`].
    walking: f64,
    /// Ticks of panic left after being hurt.
    panic: u32,
    invulnerable: u32,
    /// Counts up once dead.
    death: Option<u32>,
    ambient: i32,
    /// Sheep: colour and whether shorn.
    wool: u8,
    sheared: bool,
    /// Chickens: ticks to the next egg.
    egg_in: u32,
    /// Zombies: a small, fast one.
    baby: bool,
    /// Ticks left burning (`remainingFireTicks`).
    fire: i32,
    fall_distance: f64,
    /// The player it is after.
    target: Option<i32>,
    /// Ticks the target has been out of sight.
    unseen: u32,
    /// Ticks until the next melee hit, and until the path is planned anew.
    attack_cooldown: i32,
    repath: u32,
    /// Creepers: how far the fuse has burnt, and whether it burns.
    swell: i32,
    swell_dir: i32,
    ignited: bool,
    /// Skeletons: ticks the bow has been drawn, ticks until it may draw
    /// again, ticks the target has been seen (negative: unseen).
    drawing: Option<u32>,
    attack_time: i32,
    seen: i32,
    /// Spiders: against a wall.
    climbing: bool,
    /// What it holds and wears.
    equipment: Vec<(u8, ItemStack)>,
    /// The last player that hurt it, and when.
    hurt_by_player: Option<(i32, u64)>,
    /// Ticks without anything happening, for monsters to despawn.
    no_action: u32,
    /// Never despawns (`PersistenceRequired`).
    persistent: bool,
    /// The entity as loaded, to keep what Ferrocraft does not model.
    stored: Option<Nbt>,
    tracker: Tracker,
    sent_velocity: [f64; 3],
    data_dirty: bool,
    state: Option<Arc<EntityState>>,
}

impl Mob {
    fn new(
        entity_id: i32,
        kind: Kind,
        pos: [f64; 3],
        data: &GameData,
        rng: &mut impl Rng,
    ) -> Option<Self> {
        let (type_id, info) = data.entity(kind.name()).ok()?;
        let yaw = rng.random_range(0.0..360.0);
        let pose = Pose {
            pos,
            yaw,
            pitch: 0.0,
            head_yaw: yaw,
            on_ground: true,
        };
        let health = info.max_health.unwrap_or(10.0);
        let bow = data.item_id("minecraft:bow");
        Some(Self {
            entity_id,
            uuid: Uuid::new_v4(),
            kind,
            type_id,
            pos,
            velocity: [0.0; 3],
            yaw,
            head_yaw: yaw,
            pitch: 0.0,
            on_ground: true,
            horizontal_collision: false,
            health,
            max_health: health,
            movement_speed: info.movement_speed.unwrap_or(0.25),
            gravity: info.gravity.unwrap_or(0.08),
            half_width: info.width / 2.0,
            height: info.height,
            eye_height: info.eye_height,
            range: f64::from(info.tracking_range * 16),
            follow_range: info.follow_range.unwrap_or(16.0),
            armor: info.armor.unwrap_or(0.0),
            attack_damage: info.attack_damage.unwrap_or(2.0),
            goal: Goal::Idle,
            look: Look::Ahead,
            navigation: Navigation::default(),
            walking: 0.0,
            panic: 0,
            invulnerable: 0,
            death: None,
            ambient: -80,
            wool: if kind == Kind::Sheep {
                random_wool(rng)
            } else {
                0
            },
            sheared: false,
            egg_in: rng.random_range(6000..12000),
            baby: false,
            fire: 0,
            fall_distance: 0.0,
            target: None,
            unseen: 0,
            attack_cooldown: 0,
            repath: 0,
            swell: 0,
            swell_dir: -1,
            ignited: false,
            drawing: None,
            attack_time: -1,
            seen: 0,
            climbing: false,
            // `AbstractSkeleton.populateDefaultEquipmentSlots`.
            equipment: match (kind, bow) {
                (Kind::Skeleton, Some(bow)) => vec![(
                    equipment_slot::MAIN_HAND,
                    ItemStack {
                        item: bow,
                        count: 1,
                        patch: Default::default(),
                    },
                )],
                _ => Vec::new(),
            },
            hurt_by_player: None,
            no_action: 0,
            persistent: false,
            stored: None,
            tracker: Tracker::new(&pose, info.update_interval.max(1)),
            sent_velocity: [0.0; 3],
            data_dirty: false,
            state: None,
        })
    }

    /// A baby zombie: half the size, half again as fast
    /// (`Zombie.setBaby`).
    fn make_baby(&mut self) {
        self.baby = true;
        self.half_width /= 2.0;
        self.height /= 2.0;
        self.eye_height /= 2.0;
        self.movement_speed *= 1.5;
    }

    pub fn bounds(&self) -> Aabb {
        let [x, y, z] = self.pos;
        let w = self.half_width;
        [x - w, y, z - w, x + w, y + self.height, z + w]
    }

    fn eyes(&self) -> [f64; 3] {
        [self.pos[0], self.pos[1] + self.eye_height, self.pos[2]]
    }

    fn walker(&self) -> Walker {
        Walker {
            width: self.half_width * 2.0,
            height: self.height,
            max_drop: 3,
        }
    }

    fn pose(&self) -> Pose {
        Pose {
            pos: self.pos,
            yaw: self.yaw,
            pitch: self.pitch,
            head_yaw: self.head_yaw,
            on_ground: self.on_ground,
        }
    }

    pub fn alive(&self) -> bool {
        self.death.is_none()
    }

    /// `Entity.DATA_SHARED_FLAGS_ID`: 0x01 on fire.
    fn shared_flags(&self) -> u8 {
        u8::from(self.fire > 0)
    }

    /// `LivingEntity.DATA_LIVING_ENTITY_FLAGS`: 0x01 using an item.
    fn living_flags(&self) -> u8 {
        u8::from(self.drawing.is_some())
    }

    fn aggressive(&self) -> bool {
        self.target.is_some() && self.kind.hostile()
    }

    /// The synchronized values that differ from the defaults; with `all`,
    /// every value that can change, for an update.
    fn data(&self, all: bool) -> Vec<EntityDataValue> {
        let mut data = Vec::new();
        let mut push = |index, value, changed: bool| {
            if all || changed {
                data.push(EntityDataValue { index, value });
            }
        };
        push(
            data_index::SHARED_FLAGS,
            EntityData::Byte(self.shared_flags()),
            self.shared_flags() != 0,
        );
        push(
            data_index::LIVING_ENTITY_FLAGS,
            EntityData::Byte(self.living_flags()),
            self.living_flags() != 0,
        );
        push(data_index::HEALTH, EntityData::Float(self.health), true);
        if self.kind.hostile() {
            let flags = if self.aggressive() { 0x04 } else { 0 };
            push(MOB_FLAGS, EntityData::Byte(flags), flags != 0);
        }
        match self.kind {
            Kind::Sheep => push(
                SHEEP_WOOL,
                EntityData::Byte(self.wool | if self.sheared { 0x10 } else { 0 }),
                true,
            ),
            Kind::Zombie => push(ZOMBIE_BABY, EntityData::Boolean(self.baby), self.baby),
            Kind::Creeper => {
                push(
                    CREEPER_SWELL_DIR,
                    EntityData::Int(self.swell_dir),
                    self.swell_dir != -1,
                );
                push(
                    CREEPER_IGNITED,
                    EntityData::Boolean(self.ignited),
                    self.ignited,
                );
            }
            Kind::Spider => push(
                SPIDER_FLAGS,
                EntityData::Byte(u8::from(self.climbing)),
                self.climbing,
            ),
            _ => {}
        }
        data
    }

    pub fn state(&mut self) -> Arc<EntityState> {
        if let Some(state) = &self.state {
            return Arc::clone(state);
        }
        let state = Arc::new(EntityState {
            entity_id: self.entity_id,
            uuid: self.uuid,
            entity_type: self.type_id,
            pos: self.tracker.sent_pos,
            yaw: self.tracker.sent_yaw,
            pitch: self.tracker.sent_pitch,
            head_yaw: self.tracker.sent_head,
            velocity: self.sent_velocity,
            object_data: 0,
            data: self.data(false),
            equipment: self.equipment.clone(),
            spectator: false,
            range: self.range,
        });
        self.state = Some(Arc::clone(&state));
        state
    }

    /// Facts for the loot table.
    fn facts(&self) -> loot::EntityFacts<'static> {
        loot::EntityFacts {
            sheep_color: (self.kind == Kind::Sheep).then(|| DYES[usize::from(self.wool & 15)]),
            sheared: self.sheared,
            baby: self.baby,
            on_fire: self.fire > 0,
        }
    }

    /// `Mob.getBaseExperienceReward`: animals 1–3, monsters 5 and 1–3 for
    /// each piece of equipment, baby zombies two and a half times as much.
    fn experience(&self, rng: &mut impl Rng) -> i32 {
        if !self.kind.hostile() {
            return 1 + rng.random_range(0..3);
        }
        let mut xp = 5;
        for _ in &self.equipment {
            xp += 1 + rng.random_range(0..3);
        }
        if self.baby {
            xp = (xp as f32 * 2.5) as i32;
        }
        xp
    }

    /// `PathNavigation.followThePath`: the next waypoint, moving on when
    /// close to the current one.
    fn waypoint(&mut self) -> Option<[f64; 3]> {
        let walker = self.walker();
        let width = walker.width;
        let reach = if width > 0.75 {
            width / 2.0
        } else {
            0.75 - width / 2.0
        };
        while let Some(&node) = self.navigation.nodes.get(self.navigation.index) {
            let center = path::node_center(node, &walker);
            let close = (center[0] - self.pos[0]).abs() < reach
                && (center[2] - self.pos[2]).abs() < reach
                && (f64::from(node.y) - self.pos[1]).abs() < 1.0;
            if !close {
                return Some(center);
            }
            self.navigation.index += 1;
        }
        None
    }

    fn stop(&mut self) {
        self.navigation = Navigation::default();
    }

    fn has_path(&self) -> bool {
        self.navigation.index < self.navigation.nodes.len()
    }
}

/// The angle from one yaw to another, -180 to 180.
fn wrap(degrees: f32) -> f32 {
    (degrees + 180.0).rem_euclid(360.0) - 180.0
}

/// `Mth.approachDegrees`: turns towards `target` by at most `step`.
fn approach(current: f32, target: f32, step: f32) -> f32 {
    current + wrap(target - current).clamp(-step, step)
}

fn overlaps(a: &Aabb, b: &Aabb) -> bool {
    (0..3).all(|i| a[i] < b[i + 3] && a[i + 3] > b[i])
}

fn block_at(pos: [f64; 3]) -> BlockPos {
    BlockPos {
        x: pos[0].floor() as i32,
        y: pos[1].floor() as i32,
        z: pos[2].floor() as i32,
    }
}

/// `CombatRules.getDamageAfterAbsorb`.
pub fn after_armor(damage: f32, armor: f32, toughness: f32) -> f32 {
    let f = 2.0 + toughness / 4.0;
    let r = (armor - damage / f).clamp(armor * 0.2, 20.0);
    damage * (1.0 - r / 25.0)
}

impl Game {
    fn broadcast_mob(&mut self, entity_id: i32, packets: Vec<Encoded>) {
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return;
        };
        let message = Outbound::Entity {
            state: mob.state(),
            packets: packets.into(),
        };
        self.broadcast(None, &message);
    }

    /// `Mob.playSound` with the mob's own pitch scatter.
    fn mob_sound(&self, mob: &Mob, event: &str, volume: f32) {
        let mut rng = rand::rng();
        let pitch = (rng.random::<f32>() - rng.random::<f32>()) * 0.2 + 1.0;
        let pitch = if mob.baby { pitch + 0.5 } else { pitch };
        let name = format!("minecraft:entity.{}.{event}", mob.kind.sound());
        let source = if mob.kind.hostile() {
            SOUND_SOURCE_HOSTILE
        } else {
            SOUND_SOURCE_NEUTRAL
        };
        self.play_sound_from(None, &name, mob.pos, source, volume, pitch);
    }

    /// Spawns a mob; it appears to everyone near it.
    pub(super) fn spawn_mob(&mut self, kind: Kind, pos: [f64; 3]) -> Option<i32> {
        let entity_id = self.context.entity_ids.fetch_add(1, Ordering::Relaxed);
        let mut rng = rand::rng();
        let mut mob = Mob::new(entity_id, kind, pos, &self.context.data, &mut rng)?;
        // `Zombie.getSpawnAsBabyOdds`.
        if kind == Kind::Zombie && rng.random::<f32>() < 0.05 {
            mob.make_baby();
        }
        Some(self.add_mob(mob))
    }

    fn add_mob(&mut self, mut mob: Mob) -> i32 {
        let entity_id = mob.entity_id;
        let state = mob.state();
        self.mobs.insert(entity_id, mob);
        self.broadcast(None, &Outbound::EntityAdded(state));
        entity_id
    }

    fn remove_mob(&mut self, entity_id: i32) {
        if self.mobs.remove(&entity_id).is_some() {
            self.broadcast(None, &Outbound::EntityRemoved(entity_id));
        }
    }

    /// Every mob for one tick, then the spawner and the entities that come
    /// and go with the players.
    pub(super) fn tick_mobs(&mut self) {
        let ids: Vec<i32> = self.mobs.keys().copied().collect();
        for entity_id in ids {
            self.tick_mob(entity_id);
        }
        self.spawn_mobs();
        self.load_and_unload_entities();
    }

    fn tick_mob(&mut self, entity_id: i32) {
        let Some(mob) = self.mobs.get(&entity_id) else {
            return;
        };
        let hostile = mob.kind.hostile();
        if let Some(death) = mob.death {
            if death >= DEATH_TICKS {
                return self.remove_mob(entity_id);
            }
            if let Some(mob) = self.mobs.get_mut(&entity_id) {
                mob.death = Some(death + 1);
            }
            return;
        }
        if self.despawns(entity_id) {
            return self.remove_mob(entity_id);
        }
        if hostile {
            self.think_monster(entity_id);
        } else {
            self.think_animal(entity_id);
        }
        self.steer(entity_id);
        self.move_mob(entity_id);
        self.live(entity_id);
        if hostile {
            self.after_move_monster(entity_id);
        }
        self.sync_mob(entity_id);
    }

    /// `Mob.checkDespawn`: monsters go far from every player, and in
    /// peaceful; animals stay.
    fn despawns(&mut self, entity_id: i32) -> bool {
        let peaceful = self.context.difficulty == 0;
        let nearest = self.nearest_player_distance_sq(self.mobs[&entity_id].pos);
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return false;
        };
        if !mob.kind.hostile() {
            return false;
        }
        if peaceful {
            return true;
        }
        if mob.persistent {
            return false;
        }
        mob.no_action += 1;
        let Some(distance) = nearest else {
            return false;
        };
        if distance > 128.0 * 128.0 {
            return true;
        }
        if mob.no_action > 600 && rand::rng().random_range(0..800) == 0 && distance > 32.0 * 32.0 {
            return true;
        }
        if distance < 32.0 * 32.0 {
            mob.no_action = 0;
        }
        false
    }

    /// The squared distance from `pos` to the nearest player who is not a
    /// spectator.
    fn nearest_player_distance_sq(&self, pos: [f64; 3]) -> Option<f64> {
        self.players
            .values()
            .filter(|p| p.game_mode() != GameMode::Spectator)
            .map(|p| (0..3).map(|i| (p.pos[i] - pos[i]).powi(2)).sum::<f64>())
            .min_by(f64::total_cmp)
    }

    /// Plans a path towards `to`; `false` if the mob cannot get closer.
    fn navigate(&mut self, entity_id: i32, to: [f64; 3], reach: f64, max_nodes: usize) -> bool {
        let data = Arc::clone(&self.context.data);
        let world = Arc::clone(&self.context.world);
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return false;
        };
        let walker = mob.walker();
        let goal = block_at(to);
        let start = path::node_at(mob.pos, &walker);
        match path::find(&world, &data, walker, start, goal, reach, max_nodes) {
            Some(nodes) => {
                mob.navigation = Navigation {
                    nodes,
                    index: 0,
                    goal: Some(goal),
                    ticks: 0,
                };
                true
            }
            None => {
                mob.navigation = Navigation {
                    goal: Some(goal),
                    ..Navigation::default()
                };
                false
            }
        }
    }

    /// `MoveControl`: towards the next waypoint of the goal's path.
    fn steer(&mut self, entity_id: i32) {
        let Some(mob) = self.mobs.get(&entity_id) else {
            return;
        };
        let Goal::Walk { target, speed } = mob.goal else {
            return;
        };
        if !mob.has_path() && mob.navigation.goal != Some(block_at(target)) {
            let max = ((mob.follow_range * 16.0) as usize).max(200);
            if !self.navigate(entity_id, target, 0.5, max) {
                if let Some(mob) = self.mobs.get_mut(&entity_id) {
                    mob.goal = Goal::Idle;
                }
                return;
            }
        }
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return;
        };
        mob.navigation.ticks += 1;
        let Some(next) = mob.waypoint() else {
            mob.goal = Goal::Idle;
            return;
        };
        let (dx, dy, dz) = (
            next[0] - mob.pos[0],
            next[1] - mob.pos[1],
            next[2] - mob.pos[2],
        );
        let wanted = (-dx).atan2(dz).to_degrees() as f32;
        mob.yaw = approach(mob.yaw, wanted, 90.0);
        mob.walking = speed;
        if dy > STEP_HEIGHT && dx * dx + dz * dz < (mob.half_width * 2.0).max(1.0) && mob.on_ground
        {
            mob.velocity[1] = JUMP;
        }
        // Stuck for long: give up.
        if mob.navigation.ticks > 100 + 40 * mob.navigation.nodes.len() as u32 {
            mob.stop();
            mob.goal = Goal::Idle;
        }
    }

    /// `LivingEntity.travel`: the walking speed set by the goals, gravity,
    /// friction, a jump when something is in the way, climbing for spiders.
    fn move_mob(&mut self, entity_id: i32) {
        let data = Arc::clone(&self.context.data);
        let world = Arc::clone(&self.context.world);
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return;
        };
        let below = BlockPos {
            x: mob.pos[0].floor() as i32,
            y: (mob.pos[1] - 0.5).floor() as i32,
            z: mob.pos[2].floor() as i32,
        };
        let friction = if mob.on_ground {
            physics::friction(&data, world.block(below)) * 0.91
        } else {
            0.91
        };
        let speed = std::mem::take(&mut mob.walking);
        if speed > 0.0 {
            // `Mob.setSpeed`: the forward input and the walking speed both
            // are the goal's factor times `movement_speed`.
            let forward = speed * mob.movement_speed;
            let amount = if mob.on_ground {
                forward * (GROUND_ACCELERATION / (friction * friction * friction))
            } else {
                0.02
            };
            let yaw = f64::from(mob.yaw).to_radians();
            mob.velocity[0] += -yaw.sin() * forward * amount;
            mob.velocity[2] += yaw.cos() * forward * amount;
            if mob.horizontal_collision && mob.on_ground && !mob.climbing {
                mob.velocity[1] = JUMP;
            }
        }
        if mob.climbing {
            // `LivingEntity.handleOnClimbable`.
            mob.velocity[0] = mob.velocity[0].clamp(-0.15, 0.15);
            mob.velocity[2] = mob.velocity[2].clamp(-0.15, 0.15);
            mob.velocity[1] = mob.velocity[1].max(-0.15);
            mob.fall_distance = 0.0;
        }
        let falling = mob.velocity[1] < 0.0;
        let moved = physics::collide(&world, &data, mob.bounds(), mob.velocity);
        for (axis, distance) in moved.movement.iter().enumerate() {
            mob.pos[axis] += distance;
            if *distance != mob.velocity[axis] {
                mob.velocity[axis] = 0.0;
            }
        }
        mob.on_ground = moved.on_ground;
        mob.horizontal_collision = moved.horizontal_collision;
        if mob.kind == Kind::Spider {
            // `Spider.tick`: a wall in the way is a ladder.
            let climbing = moved.horizontal_collision;
            if climbing != mob.climbing {
                mob.climbing = climbing;
                mob.data_dirty = true;
            }
            if climbing {
                mob.velocity[1] = 0.2;
            }
        }
        mob.velocity[0] *= friction;
        mob.velocity[2] *= friction;
        mob.velocity[1] = (mob.velocity[1] - mob.gravity) * AIR_DRAG;
        // `Chicken.aiStep`: flapping, it falls slowly and never hurts itself.
        if mob.kind == Kind::Chicken && !mob.on_ground && mob.velocity[1] < 0.0 {
            mob.velocity[1] *= 0.6;
        }
        // `Entity.checkFallDamage`.
        let mut fall_damage = 0.0;
        if moved.movement[1] < 0.0 && mob.kind != Kind::Chicken && !mob.climbing {
            mob.fall_distance -= moved.movement[1];
        }
        if mob.on_ground && falling {
            fall_damage = (mob.fall_distance - SAFE_FALL).ceil().max(0.0);
            mob.fall_distance = 0.0;
        }
        let void = mob.pos[1] < f64::from(MIN_Y - 64);
        if fall_damage > 0.0 {
            self.hurt_mob(entity_id, fall_damage as f32, None, "minecraft:fall");
        }
        if void {
            self.hurt_mob(entity_id, f32::MAX, None, "minecraft:out_of_world");
        }
    }

    /// Sounds, eggs, fire, recovering from hits.
    fn live(&mut self, entity_id: i32) {
        let mut rng = rand::rng();
        let world = Arc::clone(&self.context.world);
        let data = Arc::clone(&self.context.data);
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return;
        };
        mob.invulnerable = mob.invulnerable.saturating_sub(1);
        // `Mob.baseTick`: the ambient sound, on average every few seconds.
        let ambient = mob.kind.ambient() && rng.random_range(0..1000) < mob.ambient;
        mob.ambient += 1;
        if ambient {
            mob.ambient = -80;
        }
        let egg = mob.kind == Kind::Chicken && {
            mob.egg_in = mob.egg_in.saturating_sub(1);
            mob.egg_in == 0
        };
        if egg {
            mob.egg_in = rng.random_range(6000..12000);
        }
        // `Entity.baseTick`: fire goes out in water and burns a point a
        // second.
        let mut burn = false;
        if mob.fire > 0 {
            if data.physics(world.block(block_at(mob.pos))).fluid == 1 {
                mob.fire = 0;
                mob.data_dirty = true;
            } else {
                burn = mob.fire % 20 == 0;
                mob.fire -= 1;
                if mob.fire == 0 {
                    mob.data_dirty = true;
                }
            }
        }
        let pos = mob.pos;
        if ambient && let Some(mob) = self.mobs.get(&entity_id) {
            self.mob_sound(mob, "ambient", mob.kind.volume());
        }
        if egg {
            if let Some(mob) = self.mobs.get(&entity_id) {
                self.mob_sound(mob, "egg", 1.0);
            }
            if let Some(egg) = self.context.data.item_id("minecraft:egg") {
                self.spawn_item(
                    pos,
                    [0.0, 0.2, 0.0],
                    ItemStack {
                        item: egg,
                        count: 1,
                        patch: Default::default(),
                    },
                    10,
                );
            }
        }
        if burn {
            self.hurt_mob(entity_id, 1.0, None, "minecraft:on_fire");
        }
    }

    /// Sets a mob alight for `seconds` (`Entity.igniteForSeconds`).
    fn ignite(&mut self, entity_id: i32, seconds: i32) {
        if let Some(mob) = self.mobs.get_mut(&entity_id) {
            let ticks = seconds * 20;
            if mob.fire < ticks {
                if mob.fire == 0 {
                    mob.data_dirty = true;
                }
                mob.fire = ticks;
            }
        }
    }

    fn sync_mob(&mut self, entity_id: i32) {
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return;
        };
        let dirty = std::mem::take(&mut mob.data_dirty);
        let pose = mob.pose();
        let mut packets = mob.tracker.sync(entity_id, &pose, dirty);
        let velocity_changed: f64 = mob
            .velocity
            .iter()
            .zip(mob.sent_velocity)
            .map(|(a, b)| (a - b) * (a - b))
            .sum();
        if !packets.is_empty() && velocity_changed > 1.0e-7 {
            mob.sent_velocity = mob.velocity;
            packets.push(Encoded::new(&SetEntityMotion {
                entity_id,
                velocity: LpVec3(mob.velocity),
            }));
        }
        if dirty {
            packets.push(Encoded::new(&SetEntityData {
                entity_id,
                values: EntityDataValues(mob.data(true)),
            }));
        }
        if !packets.is_empty() {
            mob.state = None;
            self.broadcast_mob(entity_id, packets);
        }
    }

    /// A hit: `LivingEntity.hurtServer` with armor and knockback away from
    /// `from`. Returns whether it took damage.
    pub(super) fn hurt_mob(
        &mut self,
        entity_id: i32,
        amount: f32,
        from: Option<(i32, [f64; 3])>,
        damage_type: &str,
    ) -> bool {
        let data = Arc::clone(&self.context.data);
        let source_type = data
            .registry_id("minecraft:damage_type", damage_type)
            .unwrap_or(0);
        let armored = !data.tag_contains(
            "minecraft:damage_type",
            "minecraft:bypasses_armor",
            source_type,
        );
        let by_player = from
            .map(|(id, _)| id)
            .filter(|id| self.players.contains_key(id));
        let tick = self.tick;
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return false;
        };
        if !mob.alive() || mob.invulnerable > HURT_COOLDOWN / 2 {
            return false;
        }
        let amount = if armored {
            after_armor(amount, mob.armor, 0.0)
        } else {
            amount
        };
        mob.health = (mob.health - amount).max(0.0);
        mob.invulnerable = HURT_COOLDOWN;
        mob.no_action = 0;
        mob.data_dirty = true;
        if let Some(player) = by_player {
            mob.hurt_by_player = Some((player, tick));
            // `HurtByTargetGoal`.
            if mob.kind.hostile() {
                mob.target = Some(player);
                mob.unseen = 0;
            }
        }
        if !mob.kind.hostile() {
            mob.panic = 100;
            mob.goal = Goal::Idle;
            mob.stop();
        }
        if let Some((_, at)) = from {
            // `LivingEntity.knockback(0.4, ...)`.
            let (dx, dz) = (at[0] - mob.pos[0], at[2] - mob.pos[2]);
            let length = dx.hypot(dz).max(1.0e-4);
            let strength = 0.4;
            mob.velocity = [
                mob.velocity[0] / 2.0 - dx / length * strength,
                if mob.on_ground {
                    (mob.velocity[1] / 2.0 + strength).min(0.4)
                } else {
                    mob.velocity[1]
                },
                mob.velocity[2] / 2.0 - dz / length * strength,
            ];
            mob.on_ground = false;
        }
        let damage = Encoded::new(&DamageEvent {
            entity_id,
            source_type,
            cause: from.map(|(id, _)| id),
            direct: from.map(|(id, _)| id),
            source_pos: None,
        });
        let dead = mob.health <= 0.0;
        self.broadcast_mob(entity_id, vec![damage]);
        if dead {
            self.kill_mob(entity_id, by_player);
        }
        true
    }

    /// `LivingEntity.die`: the death animation, the loot and, if a player
    /// had a hand in it, experience.
    fn kill_mob(&mut self, entity_id: i32, killer: Option<i32>) {
        let tick = self.tick;
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return;
        };
        if !mob.alive() {
            return;
        }
        mob.death = Some(0);
        mob.health = 0.0;
        mob.state = None;
        mob.drawing = None;
        mob.stop();
        let pos = mob.pos;
        let facts = mob.facts();
        let table = loot::entity_table(mob.kind.name());
        // A player hit it recently: its loot and experience count as theirs.
        let player = killer.or(mob
            .hurt_by_player
            .filter(|&(_, at)| tick.saturating_sub(at) < PLAYER_MEMORY)
            .map(|(p, _)| p));
        let experience = mob.experience(&mut rand::rng());
        let death = Encoded::new(&EntityEvent {
            entity_id,
            event: 3,
        });
        self.broadcast_mob(entity_id, vec![death]);
        let tool = player
            .and_then(|k| self.players.get(&k))
            .and_then(|p| p.inventory.equipment(equipment_slot::MAIN_HAND).cloned());
        let world = &self.context.world;
        let air = self.air;
        let drops = loot::Context {
            data: &self.context.data,
            table,
            state: air,
            pos: block_at(pos),
            tool: tool.as_ref(),
            block_at: &|p| world.block(p),
            entity: Some(facts),
            by_player: player.is_some(),
            explosion: None,
        }
        .drops(&mut rand::rng());
        for stack in drops {
            self.spawn_item([pos[0], pos[1] + 0.5, pos[2]], [0.0, 0.2, 0.0], stack, 10);
        }
        if player.is_some() {
            self.award_experience(pos, experience);
        }
    }

    /// Extra knockback in the direction a sprinting attacker looks
    /// (`Player.causeExtraKnockback`).
    pub(super) fn push_mob(&mut self, entity_id: i32, yaw: f64, strength: f64) {
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return;
        };
        let s = strength * 0.5;
        mob.velocity = [
            mob.velocity[0] / 2.0 - yaw.sin() * s,
            if mob.on_ground {
                (mob.velocity[1] / 2.0 + s).min(0.4)
            } else {
                mob.velocity[1]
            },
            mob.velocity[2] / 2.0 + yaw.cos() * s,
        ];
    }

    /// Pushes a mob, as an explosion does.
    pub(super) fn fling_mob(&mut self, entity_id: i32, by: [f64; 3]) {
        if let Some(mob) = self.mobs.get_mut(&entity_id) {
            for (axis, v) in by.iter().enumerate() {
                mob.velocity[axis] += v;
            }
            mob.on_ground = false;
        }
    }

    /// The mob a player's click hits, if it is one.
    pub(super) fn mob_bounds(&self, entity_id: i32) -> Option<Aabb> {
        self.mobs
            .get(&entity_id)
            .filter(|m| m.alive())
            .map(Mob::bounds)
    }

    /// Living mobs and their boxes, for explosions and arrows.
    pub(super) fn mob_boxes(&self) -> Vec<(i32, Aabb)> {
        self.mobs
            .values()
            .filter(|m| m.alive())
            .map(|m| (m.entity_id, m.bounds()))
            .collect()
    }

    /// Where a mob's eyes are.
    pub(super) fn mob_eyes(&self, entity_id: i32) -> Option<[f64; 3]> {
        self.mobs.get(&entity_id).map(Mob::eyes)
    }

    /// The name of a mob for death messages.
    pub(super) fn mob_name(&self, entity_id: i32) -> Option<Nbt> {
        let mob = self.mobs.get(&entity_id)?;
        let key = format!("entity.{}", mob.kind.name().replace(':', "."));
        Some(ferrocraft_protocol::packets::play::text::translate(
            &key,
            Vec::new(),
            None,
        ))
    }

    /// Whether a block with this shape at `pos` would stick into a mob.
    pub(super) fn mob_obstructs(&self, pos: BlockPos, shape: &[Aabb]) -> bool {
        let origin = [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)];
        self.mobs.values().filter(|m| m.alive()).any(|m| {
            let b = m.bounds();
            shape.iter().any(|s| {
                overlaps(
                    &b,
                    &[
                        origin[0] + s[0],
                        origin[1] + s[1],
                        origin[2] + s[2],
                        origin[0] + s[3],
                        origin[1] + s[4],
                        origin[2] + s[5],
                    ],
                )
            })
        })
    }

    /// The swing of a mob's arm (`LivingEntity.swing`).
    fn mob_swing(&mut self, entity_id: i32) {
        let packet = Encoded::new(&Animate {
            entity_id,
            action: 0,
        });
        self.broadcast_mob(entity_id, vec![packet]);
    }

    /// The light at a place, as `getMaxLocalRawBrightness` sees it: the
    /// sky's, darkened by the time of day, or a block's.
    pub(super) fn brightness(&self, pos: BlockPos) -> i32 {
        let world = &self.context.world;
        let sky = i32::from(world.light(crate::light::Kind::Sky, pos)) - self.sky_darken;
        let block = i32::from(world.light(crate::light::Kind::Block, pos));
        sky.max(block).max(0)
    }

    /// `LevelReader.getLightLevelDependentMagicValue` at the eyes of a mob.
    fn light_value(&self, mob: &Mob) -> f32 {
        let v = self.brightness(block_at(mob.eyes())) as f32 / 15.0;
        v / (4.0 - 3.0 * v)
    }
}

/// Whether the sky is straight above (`Level.canSeeSky`): nothing that
/// stops movement or holds a fluid on the way up.
fn can_see_sky(world: &World, data: &GameData, pos: BlockPos) -> bool {
    let top = world.top_y(pos.x, pos.z);
    (pos.y + 1..=top).all(|y| {
        let state = world.block(BlockPos { y, ..pos });
        data.collision_shape(state).is_empty() && data.physics(state).fluid == 0
    })
}
