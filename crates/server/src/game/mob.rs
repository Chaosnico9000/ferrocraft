//! Animals: cows, pigs, sheep and chickens. They spawn on grass near players,
//! stroll about, look at players, follow someone holding their food, panic
//! when hit and drop their loot when they die (`Animal` and its goals:
//! `RandomStrollGoal`, `LookAtPlayerGoal`, `RandomLookAroundGoal`,
//! `TemptGoal`, `PanicGoal`). Sheep can be shorn, cows milked, and chickens
//! lay eggs.

use std::f64::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    DamageEvent, EntityData, EntityDataValue, EntityDataValues, EntityEvent, GameMode, ItemStack,
    LpVec3, SOUND_SOURCE_NEUTRAL, SOUND_SOURCE_PLAYERS, SetEntityData, SetEntityMotion, data_index,
    equipment_slot,
};
use rand::Rng;
use uuid::Uuid;

use super::physics;
use super::tracker::{Pose, Tracker};
use super::{Encoded, EntityState, Game, Outbound};
use crate::data::{Aabb, GameData};
use crate::loot;
use crate::world::{MAX_Y, MIN_Y};

/// `Sheep.DATA_WOOL_ID`, after `LivingEntity`'s values 8–14, `Mob`'s 15 and
/// `AgeableMob`'s 16–17.
const SHEEP_WOOL: u8 = 18;
/// Ticks a mob shows its death before it goes (`LivingEntity.tickDeath`).
const DEATH_TICKS: u32 = 20;
/// `invulnerableTime` after a hit; hits within the first half do nothing.
const HURT_COOLDOWN: u32 = 10;
/// How many animals a player may have around them, and how far.
const ANIMALS_PER_PLAYER: usize = 12;
const NEARBY: f64 = 96.0;
/// Animals farther than this from every player go; they are not saved yet.
const DESPAWN_DISTANCE: f64 = 160.0;
const AIR_DRAG: f64 = 0.98;
/// `LivingEntity.getFrictionInfluencedSpeed`'s constant.
const GROUND_ACCELERATION: f64 = 0.216_000_02;
/// `Entity.getJumpPower` with `jump_strength` 0.42.
const JUMP: f64 = 0.42;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Cow,
    Pig,
    Sheep,
    Chicken,
}

impl Kind {
    const ALL: [Kind; 4] = [Kind::Cow, Kind::Pig, Kind::Sheep, Kind::Chicken];

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Cow => "minecraft:cow",
            Kind::Pig => "minecraft:pig",
            Kind::Sheep => "minecraft:sheep",
            Kind::Chicken => "minecraft:chicken",
        }
    }

    fn sound(self) -> &'static str {
        match self {
            Kind::Cow => "cow",
            Kind::Pig => "pig",
            Kind::Sheep => "sheep",
            Kind::Chicken => "chicken",
        }
    }

    /// `Animal.getSoundVolume`: cows are quieter.
    fn volume(self) -> f32 {
        match self {
            Kind::Cow => 0.4,
            _ => 1.0,
        }
    }

    /// Spawn weights in plains (`BiomeDefaultFeatures.farmAnimals`).
    fn weight(self) -> u32 {
        match self {
            Kind::Sheep => 12,
            Kind::Pig | Kind::Chicken => 10,
            Kind::Cow => 8,
        }
    }

    /// The speed factors of their goals: panic, tempt.
    fn speeds(self) -> (f64, f64) {
        match self {
            Kind::Cow => (2.0, 1.25),
            Kind::Pig => (1.25, 1.2),
            Kind::Sheep => (1.25, 1.1),
            Kind::Chicken => (1.4, 1.0),
        }
    }

    /// What makes them follow (`*_FOOD` tags).
    fn food_tag(self) -> &'static str {
        match self {
            Kind::Cow => "minecraft:cow_food",
            Kind::Pig => "minecraft:pig_food",
            Kind::Sheep => "minecraft:sheep_food",
            Kind::Chicken => "minecraft:chicken_food",
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

/// What a mob is about to do.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Goal {
    Idle,
    /// Walking somewhere, with a speed factor, until it gets there or gives
    /// up.
    Walk {
        target: [f64; 2],
        speed: f64,
        ticks: u32,
    },
}

/// Where the head points.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Look {
    Ahead,
    At { player: i32, ticks: u32 },
    Around { yaw: f32, ticks: u32 },
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
    movement_speed: f64,
    gravity: f64,
    half_width: f64,
    height: f64,
    eye_height: f64,
    range: f64,
    goal: Goal,
    look: Look,
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
            health: info.max_health.unwrap_or(10.0),
            movement_speed: info.movement_speed.unwrap_or(0.25),
            gravity: info.gravity.unwrap_or(0.08),
            half_width: info.width / 2.0,
            height: info.height,
            eye_height: info.eye_height,
            range: f64::from(info.tracking_range * 16),
            goal: Goal::Idle,
            look: Look::Ahead,
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
            tracker: Tracker::new(&pose, info.update_interval.max(1)),
            sent_velocity: [0.0; 3],
            data_dirty: false,
            state: None,
        })
    }

    pub fn bounds(&self) -> Aabb {
        let [x, y, z] = self.pos;
        let w = self.half_width;
        [x - w, y, z - w, x + w, y + self.height, z + w]
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

    fn data(&self) -> Vec<EntityDataValue> {
        let mut data = vec![EntityDataValue {
            index: data_index::HEALTH,
            value: EntityData::Float(self.health),
        }];
        if self.kind == Kind::Sheep {
            data.push(EntityDataValue {
                index: SHEEP_WOOL,
                value: EntityData::Byte(self.wool | if self.sheared { 0x10 } else { 0 }),
            });
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
            data: self.data(),
            equipment: Vec::new(),
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
            baby: false,
        }
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
        let name = format!("minecraft:entity.{}.{event}", mob.kind.sound());
        self.play_sound_from(None, &name, mob.pos, SOUND_SOURCE_NEUTRAL, volume, pitch);
    }

    /// Spawns an animal; it appears to everyone near it.
    pub(super) fn spawn_mob(&mut self, kind: Kind, pos: [f64; 3]) -> Option<i32> {
        let entity_id = self.context.entity_ids.fetch_add(1, Ordering::Relaxed);
        let mut mob = Mob::new(entity_id, kind, pos, &self.context.data, &mut rand::rng())?;
        let state = mob.state();
        self.mobs.insert(entity_id, mob);
        self.broadcast(None, &Outbound::EntityAdded(state));
        Some(entity_id)
    }

    fn remove_mob(&mut self, entity_id: i32) {
        if self.mobs.remove(&entity_id).is_some() {
            self.broadcast(None, &Outbound::EntityRemoved(entity_id));
        }
    }

    /// Where an animal could stand at this column: on grass, with room above
    /// (`Animal.checkAnimalSpawnRules`).
    fn spawn_spot(&self, x: i32, z: i32) -> Option<[f64; 3]> {
        let data = &self.context.data;
        let world = &self.context.world;
        let mut y = MAX_Y;
        while y > MIN_Y
            && data
                .collision_shape(world.block(BlockPos { x, y, z }))
                .is_empty()
        {
            y -= 1;
        }
        let ground = world.block(BlockPos { x, y, z });
        let on_grass = data
            .block_of(ground)
            .is_some_and(|(id, _)| data.block_in_tag("minecraft:animals_spawnable_on", id));
        let free = (1..=2).all(|dy| {
            let state = world.block(BlockPos { x, y: y + dy, z });
            data.collision_shape(state).is_empty() && data.physics(state).fluid == 0
        });
        (on_grass && free && y < MAX_Y)
            .then(|| [f64::from(x) + 0.5, f64::from(y + 1), f64::from(z) + 0.5])
    }

    /// `NaturalSpawner` for animals, simplified: every second, a player with
    /// too few animals around gets a herd somewhere 24 to 64 blocks away.
    fn spawn_animals(&mut self) {
        if !self.context.animals || !self.tick.is_multiple_of(20) {
            return;
        }
        let players: Vec<[f64; 3]> = self
            .players
            .values()
            .filter(|p| p.game_mode() != GameMode::Spectator && !p.dead)
            .map(|p| p.pos)
            .collect();
        let mut rng = rand::rng();
        for at in players {
            let nearby = self
                .mobs
                .values()
                .filter(|m| (m.pos[0] - at[0]).hypot(m.pos[2] - at[2]) < NEARBY)
                .count();
            if nearby >= ANIMALS_PER_PLAYER {
                continue;
            }
            let angle = rng.random_range(0.0..TAU);
            let distance = rng.random_range(24.0..64.0);
            let x = (at[0] + angle.cos() * distance).floor() as i32;
            let z = (at[2] + angle.sin() * distance).floor() as i32;
            let total: u32 = Kind::ALL.iter().map(|k| k.weight()).sum();
            let mut pick = rng.random_range(0..total);
            let kind = *Kind::ALL
                .iter()
                .find(|k| {
                    if pick < k.weight() {
                        true
                    } else {
                        pick -= k.weight();
                        false
                    }
                })
                .unwrap_or(&Kind::Sheep);
            // A herd of up to four around the spot.
            for _ in 0..rng.random_range(2..=4) {
                let (dx, dz) = (rng.random_range(-3..=3), rng.random_range(-3..=3));
                if let Some(pos) = self.spawn_spot(x + dx, z + dz) {
                    self.spawn_mob(kind, pos);
                }
            }
        }
    }

    /// Every animal for one tick, and the spawner.
    pub(super) fn tick_mobs(&mut self) {
        self.spawn_animals();
        let ids: Vec<i32> = self.mobs.keys().copied().collect();
        for entity_id in ids {
            self.tick_mob(entity_id);
        }
    }

    fn tick_mob(&mut self, entity_id: i32) {
        let Some(mob) = self.mobs.get(&entity_id) else {
            return;
        };
        if let Some(death) = mob.death {
            if death >= DEATH_TICKS {
                return self.remove_mob(entity_id);
            }
            if let Some(mob) = self.mobs.get_mut(&entity_id) {
                mob.death = Some(death + 1);
            }
            return;
        }
        let far = self
            .players
            .values()
            .all(|p| (p.pos[0] - mob.pos[0]).hypot(p.pos[2] - mob.pos[2]) > DESPAWN_DISTANCE);
        if far {
            return self.remove_mob(entity_id);
        }
        self.think(entity_id);
        self.move_mob(entity_id);
        self.live(entity_id);
        self.sync_mob(entity_id);
    }

    /// The goals, by priority: panic, follow food, look, stroll.
    fn think(&mut self, entity_id: i32) {
        let data = Arc::clone(&self.context.data);
        let Some(mob) = self.mobs.get(&entity_id) else {
            return;
        };
        let mut rng = rand::rng();
        let (panic_speed, tempt_speed) = mob.kind.speeds();
        let eyes = [mob.pos[0], mob.pos[1] + mob.eye_height, mob.pos[2]];
        // `TemptGoal`: the nearest player within 10 blocks holding food.
        let food = data.item_tag(mob.kind.food_tag()).unwrap_or(&[]);
        let tempter = self
            .players
            .values()
            .filter(|p| p.game_mode() != GameMode::Spectator && !p.dead)
            .filter(|p| {
                [equipment_slot::MAIN_HAND, equipment_slot::OFF_HAND]
                    .iter()
                    .any(|&hand| {
                        p.inventory
                            .equipment(hand)
                            .is_some_and(|s| food.contains(&s.item))
                    })
            })
            .map(|p| {
                (
                    p.entity_id,
                    p.pos,
                    (p.pos[0] - mob.pos[0]).hypot(p.pos[2] - mob.pos[2]),
                )
            })
            .filter(|&(_, _, d)| d < 10.0)
            .min_by(|a, b| a.2.total_cmp(&b.2));
        let watcher = self
            .players
            .values()
            .filter(|p| p.game_mode() != GameMode::Spectator)
            .map(|p| {
                (
                    p.entity_id,
                    (p.pos[0] - mob.pos[0]).hypot(p.pos[2] - mob.pos[2]),
                )
            })
            .filter(|&(_, d)| d < 6.0)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let looked_at = match mob.look {
            Look::At { player, .. } => self.players.get(&player).map(|p| p.eyes()),
            _ => None,
        };
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return;
        };
        if mob.panic > 0 {
            mob.panic -= 1;
            let arrived = match mob.goal {
                Goal::Walk { target, .. } => {
                    (target[0] - mob.pos[0]).hypot(target[1] - mob.pos[2]) < 1.0
                }
                Goal::Idle => true,
            };
            if arrived || !matches!(mob.goal, Goal::Walk { speed, .. } if speed == panic_speed) {
                let angle = rng.random_range(0.0..TAU);
                let distance = rng.random_range(3.0..6.0);
                mob.goal = Goal::Walk {
                    target: [
                        mob.pos[0] + angle.cos() * distance,
                        mob.pos[2] + angle.sin() * distance,
                    ],
                    speed: panic_speed,
                    ticks: 0,
                };
            }
        } else if let Some((player, at, distance)) = tempter {
            mob.goal = if distance > 2.5 {
                Goal::Walk {
                    target: [at[0], at[2]],
                    speed: tempt_speed,
                    ticks: 0,
                }
            } else {
                Goal::Idle
            };
            mob.look = Look::At { player, ticks: 2 };
        } else {
            match mob.goal {
                Goal::Idle => {
                    // `RandomStrollGoal`: now and then, somewhere within ten
                    // blocks.
                    if rng.random_range(0..120) == 0 {
                        mob.goal = Goal::Walk {
                            target: [
                                mob.pos[0] + rng.random_range(-10.0..10.0),
                                mob.pos[2] + rng.random_range(-10.0..10.0),
                            ],
                            speed: 1.0,
                            ticks: 0,
                        };
                    }
                }
                Goal::Walk { target, ticks, .. } => {
                    let arrived = (target[0] - mob.pos[0]).hypot(target[1] - mob.pos[2]) < 0.5;
                    if arrived || ticks > 200 {
                        mob.goal = Goal::Idle;
                    }
                }
            }
        }
        // Where the head points.
        mob.look = match mob.look {
            Look::At { player, ticks } if ticks > 0 && looked_at.is_some() => Look::At {
                player,
                ticks: ticks - 1,
            },
            Look::Around { yaw, ticks } if ticks > 0 => Look::Around {
                yaw,
                ticks: ticks - 1,
            },
            _ => {
                if let Some((player, _)) = watcher.filter(|_| rng.random::<f32>() < 0.02) {
                    Look::At {
                        player,
                        ticks: 40 + rng.random_range(0..40),
                    }
                } else if rng.random::<f32>() < 0.02 {
                    Look::Around {
                        yaw: mob.yaw + rng.random_range(-90.0..90.0),
                        ticks: 20 + rng.random_range(0..20),
                    }
                } else {
                    Look::Ahead
                }
            }
        };
        let (head_yaw, pitch) = match (mob.look, looked_at) {
            (Look::At { .. }, Some(target)) => {
                let [dx, dy, dz] = [
                    target[0] - eyes[0],
                    target[1] - eyes[1],
                    target[2] - eyes[2],
                ];
                (
                    (-dx).atan2(dz).to_degrees() as f32,
                    (-dy.atan2(dx.hypot(dz))).to_degrees() as f32,
                )
            }
            (Look::Around { yaw, .. }, _) => (yaw, 0.0),
            _ => (mob.yaw, 0.0),
        };
        // `LookControl`: ten degrees a tick, the head at most 75 degrees off
        // the body.
        let head = approach(mob.head_yaw, head_yaw, 10.0);
        mob.head_yaw = mob.yaw + wrap(head - mob.yaw).clamp(-75.0, 75.0);
        mob.pitch = approach(mob.pitch, pitch, 10.0);
    }

    /// `MoveControl` and `LivingEntity.travel`: towards the goal, with
    /// gravity, friction and a jump when something is in the way.
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
        if let Goal::Walk {
            target,
            speed,
            ticks,
        } = mob.goal
        {
            let (dx, dz) = (target[0] - mob.pos[0], target[1] - mob.pos[2]);
            let wanted = (-dx).atan2(dz).to_degrees() as f32;
            mob.yaw = approach(mob.yaw, wanted, 90.0);
            // `Mob.setSpeed`: the forward input and the walking speed both
            // are the goal's factor times `movement_speed`.
            let forward = speed * mob.movement_speed;
            let amount = if mob.on_ground {
                forward * (GROUND_ACCELERATION / (friction * friction * friction))
            } else {
                0.02
            };
            let yaw = f64::from(mob.yaw).to_radians();
            // `moveRelative`: forward by `forward`, scaled by `amount`.
            mob.velocity[0] += -yaw.sin() * forward * amount;
            mob.velocity[2] += yaw.cos() * forward * amount;
            if mob.horizontal_collision && mob.on_ground {
                mob.velocity[1] = JUMP;
            }
            mob.goal = Goal::Walk {
                target,
                speed,
                ticks: ticks + 1,
            };
        }
        let moved = physics::collide(&world, &data, mob.bounds(), mob.velocity);
        for (axis, distance) in moved.movement.iter().enumerate() {
            mob.pos[axis] += distance;
            if *distance != mob.velocity[axis] {
                mob.velocity[axis] = 0.0;
            }
        }
        mob.on_ground = moved.on_ground;
        mob.horizontal_collision = moved.horizontal_collision;
        mob.velocity[0] *= friction;
        mob.velocity[2] *= friction;
        mob.velocity[1] = (mob.velocity[1] - mob.gravity) * AIR_DRAG;
        // `Chicken.aiStep`: flapping, it falls slowly.
        if mob.kind == Kind::Chicken && !mob.on_ground && mob.velocity[1] < 0.0 {
            mob.velocity[1] *= 0.6;
        }
        if mob.pos[1] < f64::from(MIN_Y - 64) {
            mob.health = 0.0;
        }
    }

    /// Sounds, eggs, recovering from hits.
    fn live(&mut self, entity_id: i32) {
        let mut rng = rand::rng();
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return;
        };
        mob.invulnerable = mob.invulnerable.saturating_sub(1);
        // `Mob.baseTick`: the ambient sound, on average every few seconds.
        let ambient = rng.random_range(0..1000) < mob.ambient;
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
        let pos = mob.pos;
        let dead = mob.health <= 0.0;
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
        if dead {
            self.kill_mob(entity_id, None);
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
                values: EntityDataValues(mob.data()),
            }));
        }
        if !packets.is_empty() {
            mob.state = None;
            self.broadcast_mob(entity_id, packets);
        }
    }

    /// A hit: `LivingEntity.hurtServer` with knockback away from `from`.
    /// Returns whether it took damage.
    pub(super) fn hurt_mob(
        &mut self,
        entity_id: i32,
        amount: f32,
        from: Option<(i32, [f64; 3])>,
        damage_type: &str,
    ) -> bool {
        let source_type = self
            .context
            .data
            .registry_id("minecraft:damage_type", damage_type)
            .unwrap_or(0);
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return false;
        };
        if !mob.alive() || mob.invulnerable > HURT_COOLDOWN / 2 {
            return false;
        }
        mob.health = (mob.health - amount).max(0.0);
        mob.invulnerable = HURT_COOLDOWN;
        mob.panic = 100;
        mob.goal = Goal::Idle;
        mob.data_dirty = true;
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
            self.kill_mob(entity_id, from.map(|(id, _)| id));
        }
        true
    }

    /// `LivingEntity.die`: the death animation and the loot.
    fn kill_mob(&mut self, entity_id: i32, killer: Option<i32>) {
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return;
        };
        if !mob.alive() {
            return;
        }
        mob.death = Some(0);
        mob.health = 0.0;
        mob.state = None;
        let pos = mob.pos;
        let facts = mob.facts();
        let table = loot::entity_table(mob.kind.name());
        let death = Encoded::new(&EntityEvent {
            entity_id,
            event: 3,
        });
        self.broadcast_mob(entity_id, vec![death]);
        let tool = killer
            .and_then(|k| self.players.get(&k))
            .and_then(|p| p.inventory.equipment(equipment_slot::MAIN_HAND).cloned());
        let world = &self.context.world;
        let air = self.air;
        let drops = loot::Context {
            data: &self.context.data,
            table,
            state: air,
            pos: BlockPos {
                x: pos[0].floor() as i32,
                y: pos[1].floor() as i32,
                z: pos[2].floor() as i32,
            },
            tool: tool.as_ref(),
            block_at: &|p| world.block(p),
            entity: Some(facts),
            by_player: killer.is_some(),
        }
        .drops(&mut rand::rng());
        for stack in drops {
            self.spawn_item([pos[0], pos[1] + 0.5, pos[2]], [0.0, 0.2, 0.0], stack, 10);
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

    /// The mob a player's click hits, if it is one.
    pub(super) fn mob_bounds(&self, entity_id: i32) -> Option<Aabb> {
        self.mobs
            .get(&entity_id)
            .filter(|m| m.alive())
            .map(Mob::bounds)
    }

    /// Whether a block with this shape at `pos` would stick into an animal.
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

    /// A right click on an animal with shears or a bucket
    /// (`Sheep.mobInteract`, `AbstractCow.mobInteract`). Returns whether
    /// something happened.
    pub(super) fn interact_mob(&mut self, entity_id: i32, player_id: i32, hand: i32) -> bool {
        let data = Arc::clone(&self.context.data);
        let Some(player) = self.players.get(&player_id) else {
            return false;
        };
        let slot = if hand == 1 {
            crate::inventory::OFF_HAND
        } else {
            player.inventory.selected_slot()
        };
        let creative = player.game_mode() == GameMode::Creative;
        let Some(held) = player.inventory.get(slot).cloned() else {
            return false;
        };
        let held_name = data.item(held.item).map_or("", |i| i.name.as_str());
        let Some(mob) = self.mobs.get_mut(&entity_id).filter(|m| m.alive()) else {
            return false;
        };
        match (mob.kind, held_name) {
            (Kind::Sheep, "minecraft:shears") if !mob.sheared => {
                mob.sheared = true;
                mob.data_dirty = true;
                let wool = data.item_id(&format!(
                    "minecraft:{}_wool",
                    DYES[usize::from(mob.wool & 15)]
                ));
                let pos = mob.pos;
                let count = rand::rng().random_range(1..=3);
                if let Some(mob) = self.mobs.get(&entity_id) {
                    self.mob_sound(mob, "shear", 1.0);
                }
                if let Some(wool) = wool {
                    for _ in 0..count {
                        let mut rng = rand::rng();
                        let velocity = [
                            (rng.random::<f64>() - rng.random::<f64>()) * 0.1,
                            rng.random::<f64>() * 0.05,
                            (rng.random::<f64>() - rng.random::<f64>()) * 0.1,
                        ];
                        self.spawn_item(
                            [pos[0], pos[1] + 1.0, pos[2]],
                            velocity,
                            ItemStack {
                                item: wool,
                                count: 1,
                                patch: Default::default(),
                            },
                            10,
                        );
                    }
                }
                if !creative {
                    self.wear(player_id, slot, 1);
                }
                true
            }
            (Kind::Cow, "minecraft:bucket") => {
                let at = mob.pos;
                self.play_sound_from(
                    None,
                    "minecraft:entity.cow.milk",
                    at,
                    SOUND_SOURCE_PLAYERS,
                    1.0,
                    1.0,
                );
                let Some(milk) = data.item_id("minecraft:milk_bucket") else {
                    return false;
                };
                let milk = ItemStack {
                    item: milk,
                    count: 1,
                    patch: Default::default(),
                };
                // `ItemUtils.createFilledResult`.
                if creative {
                    let has = self.players.get(&player_id).is_some_and(|p| {
                        p.inventory
                            .slots()
                            .iter()
                            .flatten()
                            .any(|s| s.item == milk.item)
                    });
                    if !has {
                        self.give(player_id, milk);
                    }
                } else if held.count == 1 {
                    self.update_inventory(player_id, |inventory| inventory.set(slot, Some(milk)));
                    self.send_slots(player_id, &[slot]);
                } else {
                    self.update_inventory(player_id, |inventory| {
                        inventory.take(slot, 1);
                    });
                    self.send_slots(player_id, &[slot]);
                    self.give(player_id, milk);
                }
                true
            }
            _ => false,
        }
    }
}
