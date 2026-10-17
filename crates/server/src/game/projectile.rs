//! Arrows (`AbstractArrow`): they fly with gravity and drag, stick in the
//! first block they hit and hurt the first player or mob in their way by
//! how fast they fly; a fully drawn bow's arrows hit harder at random. A
//! player's arrows can be picked up again once stuck (in creative they just
//! go), skeletons' cannot; stuck ones vanish after a minute. Tipped arrows
//! give their potion's effects at an eighth of the time, spectral ones make
//! glow; the bow's enchantments add damage and knockback.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    EntityData, EntityDataValue, EntityDataValues, GameEvent, GameMode, ItemStack, LpVec3,
    SOUND_SOURCE_NEUTRAL, SetEntityData, SetEntityMotion, TakeItemEntity, data_index, text,
};
use rand::Rng;
use uuid::Uuid;

use ferrocraft_protocol::Nbt;

use super::mob::save::{doubles, read_doubles, uuid_from_ints, uuid_from_nbt, uuid_to_nbt};
use super::physics;
use super::survival::Hit;
use super::tracker::{Pose, Tracker};
use super::{Encoded, EntityState, Game, Outbound};
use crate::data::Aabb;
use crate::storage::{state_from_nbt, state_to_nbt};

/// `AbstractArrow.ID_FLAGS` (0x01 critical) and `IN_GROUND`, with
/// `PIERCE_LEVEL` (9) between them; `Arrow.ID_EFFECT_COLOR`.
const FLAGS: u8 = 8;
const IN_GROUND: u8 = 10;
const EFFECT_COLOR: u8 = 11;
/// Ticks in the ground after which a tipped arrow is a plain one again
/// (`Arrow.EXPOSED_POTION_DECAY_TIME`).
const POTION_DECAY: u32 = 600;
/// Ticks a stuck arrow quivers and cannot be picked up (`shakeTime`).
const SHAKE: u32 = 7;
/// `ClientboundGameEventPacket.PLAY_ARROW_HIT_SOUND`, for the shooter.
const ARROW_HIT_PLAYER: u8 = 6;

/// Who may pick an arrow up (`AbstractArrow.Pickup`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pickup {
    Disallowed,
    Allowed,
    /// Shot in creative: only a creative player takes it, and gets nothing.
    CreativeOnly,
}

/// An arrow about to be shot.
pub struct Shot {
    pub owner: Option<i32>,
    pub from: [f64; 3],
    pub direction: [f64; 3],
    /// Blocks a tick.
    pub speed: f64,
    pub inaccuracy: f64,
    pub base_damage: f64,
    pub crit: bool,
    pub pickup: Pickup,
    /// The arrow as an item: plain, tipped or spectral.
    pub item: ItemStack,
    /// The bow it came from, for its enchantments.
    pub weapon: Option<ItemStack>,
}
/// Seen up to 4 chunks away, updated once a second.
const RANGE: f64 = 4.0 * 16.0;
const UPDATE_INTERVAL: u32 = 20;
const GRAVITY: f64 = 0.05;
const DRAG: f64 = 0.99;
const WATER_DRAG: f64 = 0.6;
/// A stuck arrow goes after this many ticks.
const LIFETIME: u32 = 1200;
/// Entities count as hit this far around their box
/// (`ProjectileUtil.getEntityHitResult`'s margin).
const MARGIN: f64 = 0.3;

pub struct Arrow {
    pub entity_id: i32,
    uuid: Uuid,
    owner: Option<i32>,
    pub pos: [f64; 3],
    velocity: [f64; 3],
    yaw: f32,
    pitch: f32,
    base_damage: f64,
    crit: bool,
    pickup: Pickup,
    /// Where it sticks, and the block it sticks in.
    stuck: Option<(BlockPos, u32)>,
    /// Ticks left burning, after flying through fire or lava.
    fire: i32,
    shake: u32,
    life: u32,
    /// What it gives back when picked up, with its potion.
    item: ItemStack,
    weapon: Option<ItemStack>,
    spectral: bool,
    /// The potion's colour, -1 for none.
    color: i32,
    tracker: Tracker,
    sent_velocity: [f64; 3],
    data_dirty: bool,
    state: Option<Arc<EntityState>>,
}

impl Arrow {
    fn pose(&self) -> Pose {
        Pose {
            pos: self.pos,
            yaw: self.yaw,
            pitch: self.pitch,
            head_yaw: 0.0,
            on_ground: false,
        }
    }

    fn state(&mut self, arrow_type: i32) -> Arc<EntityState> {
        if let Some(state) = &self.state {
            return Arc::clone(state);
        }
        let state = Arc::new(EntityState {
            entity_id: self.entity_id,
            uuid: self.uuid,
            entity_type: arrow_type,
            pos: self.tracker.sent_pos,
            yaw: self.tracker.sent_yaw,
            pitch: self.tracker.sent_pitch,
            head_yaw: 0,
            velocity: self.sent_velocity,
            // `Projectile.getAddEntityPacket`: the shooter.
            object_data: self.owner.unwrap_or(0),
            data: self
                .data()
                .into_iter()
                .filter(|v| {
                    v.value != EntityData::Byte(0)
                        && v.value != EntityData::Boolean(false)
                        && v.value != EntityData::Int(-1)
                })
                .collect(),
            equipment: Vec::new(),
            spectator: false,
            range: RANGE,
        });
        self.state = Some(Arc::clone(&state));
        state
    }

    fn data(&self) -> Vec<EntityDataValue> {
        vec![
            EntityDataValue {
                index: data_index::SHARED_FLAGS,
                value: EntityData::Byte(u8::from(self.fire > 0)),
            },
            EntityDataValue {
                index: FLAGS,
                value: EntityData::Byte(u8::from(self.crit)),
            },
            EntityDataValue {
                index: IN_GROUND,
                value: EntityData::Boolean(self.stuck.is_some()),
            },
        ]
        .into_iter()
        .chain((!self.spectral).then_some(EntityDataValue {
            index: EFFECT_COLOR,
            value: EntityData::Int(self.color),
        }))
        .collect()
    }

    fn bounds(&self) -> Aabb {
        let [x, y, z] = self.pos;
        [x - 0.25, y, z - 0.25, x + 0.25, y + 0.5, z + 0.25]
    }

    /// `Projectile.updateRotation`: pointing where it flies.
    fn face_velocity(&mut self) {
        let [x, y, z] = self.velocity;
        self.yaw = x.atan2(z).to_degrees() as f32;
        self.pitch = y.atan2(x.hypot(z)).to_degrees() as f32;
    }
}

/// Where a line from `from` to `to` meets a box grown by `margin`.
fn line_hits(from: [f64; 3], to: [f64; 3], b: &Aabb, margin: f64) -> Option<f64> {
    let grown = [
        b[0] - margin,
        b[1] - margin,
        b[2] - margin,
        b[3] + margin,
        b[4] + margin,
        b[5] + margin,
    ];
    physics::clip_box(from, to, &grown)
}

impl Game {
    fn arrow_type(&self, spectral: bool) -> i32 {
        let name = if spectral {
            "minecraft:spectral_arrow"
        } else {
            "minecraft:arrow"
        };
        self.context.data.entity_type(name).unwrap_or(0)
    }

    /// The colour of a tipped arrow's potion, -1 for a plain one.
    fn arrow_color(&self, item: &ItemStack) -> i32 {
        let data = &self.context.data;
        match crate::stack::get(data, item, "minecraft:potion_contents") {
            Some(contents) if contents != Nbt::Compound(Vec::new()) => {
                super::potion::potion_color(data, &contents)
            }
            _ => -1,
        }
    }

    /// `Projectile.shoot`: off towards `direction` at `speed` blocks a tick,
    /// scattered by `inaccuracy`.
    pub(super) fn shoot_arrow(&mut self, shot: Shot) {
        let Shot {
            owner,
            from,
            direction,
            speed,
            inaccuracy,
            base_damage,
            crit,
            pickup,
            item,
            weapon,
        } = shot;
        let spectral = self
            .context
            .data
            .item(item.item)
            .is_some_and(|i| i.name == "minecraft:spectral_arrow");
        let color = self.arrow_color(&item);
        // `onProjectileSpawned`: flame sets it alight.
        let fire = weapon.as_ref().map_or(0, |w| self.flame(w));
        let mut rng = rand::rng();
        let length = direction
            .iter()
            .map(|v| v * v)
            .sum::<f64>()
            .sqrt()
            .max(1.0e-6);
        let velocity = direction.map(|v| {
            (v / length + super::mob::triangle(&mut rng, 0.0, 0.017_227_5 * inaccuracy)) * speed
        });
        let entity_id = self.context.entity_ids.fetch_add(1, Ordering::Relaxed);
        let mut arrow = Arrow {
            entity_id,
            uuid: Uuid::new_v4(),
            owner,
            pos: from,
            velocity,
            yaw: 0.0,
            pitch: 0.0,
            base_damage,
            crit,
            pickup,
            stuck: None,
            fire,
            shake: 0,
            life: 0,
            item,
            weapon,
            spectral,
            color,
            tracker: Tracker::new(
                &Pose {
                    pos: from,
                    yaw: 0.0,
                    pitch: 0.0,
                    head_yaw: 0.0,
                    on_ground: false,
                },
                UPDATE_INTERVAL,
            ),
            sent_velocity: velocity,
            data_dirty: false,
            state: None,
        };
        arrow.face_velocity();
        arrow.tracker = Tracker::new(&arrow.pose(), UPDATE_INTERVAL);
        let state = arrow.state(self.arrow_type(spectral));
        self.arrows.insert(entity_id, arrow);
        self.broadcast(None, &Outbound::EntityAdded(state));
    }

    /// `AbstractArrow.addAdditionalSaveData` after the entity's and the
    /// projectile's own.
    pub(super) fn arrow_nbt(&self) -> Vec<([f64; 3], Nbt)> {
        let data = &self.context.data;
        self.arrows
            .values()
            .map(|a| {
                let pickup = match a.pickup {
                    Pickup::Disallowed => 0,
                    Pickup::Allowed => 1,
                    Pickup::CreativeOnly => 2,
                };
                let id = if a.spectral {
                    "minecraft:spectral_arrow"
                } else {
                    "minecraft:arrow"
                };
                let mut entries = vec![
                    ("id".to_owned(), Nbt::String(id.into())),
                    ("Pos".into(), doubles(&a.pos)),
                    ("Motion".into(), doubles(&a.velocity)),
                    (
                        "Rotation".into(),
                        Nbt::List(vec![Nbt::Float(a.yaw), Nbt::Float(a.pitch)]),
                    ),
                    ("OnGround".into(), Nbt::Byte(0)),
                    ("UUID".into(), uuid_to_nbt(a.uuid)),
                    ("Fire".into(), Nbt::Short(a.fire.min(32767) as i16)),
                    ("HasBeenShot".into(), Nbt::Byte(1)),
                    ("LeftOwner".into(), Nbt::Byte(1)),
                    ("life".into(), Nbt::Short(a.life.min(32767) as i16)),
                    ("shake".into(), Nbt::Byte(a.shake.min(127) as i8)),
                    ("inGround".into(), Nbt::Byte(i8::from(a.stuck.is_some()))),
                    ("pickup".into(), Nbt::Byte(pickup)),
                    ("damage".into(), Nbt::Double(a.base_damage)),
                    ("crit".into(), Nbt::Byte(i8::from(a.crit))),
                    ("PierceLevel".into(), Nbt::Byte(0)),
                    (
                        "SoundEvent".into(),
                        Nbt::String("minecraft:entity.arrow.hit".into()),
                    ),
                ];
                if let Some(item) = crate::storage::item_to_nbt(&a.item, data) {
                    entries.push(("item".into(), item));
                }
                if let Some(weapon) = a
                    .weapon
                    .as_ref()
                    .and_then(|w| crate::storage::item_to_nbt(w, data))
                {
                    entries.push(("weapon".into(), weapon));
                }
                if let Some((_, state)) = a.stuck {
                    entries.push(("inBlockState".into(), state_to_nbt(state, data)));
                }
                if let Some(owner) = a.owner.and_then(|o| self.uuid_of(o)) {
                    entries.push(("Owner".into(), uuid_to_nbt(owner)));
                }
                (a.pos, Nbt::Compound(entries))
            })
            .collect()
    }

    /// An arrow from the entity store: flying, or stuck in the block it is
    /// at (`shouldFall` looks 0.06 around it).
    pub(super) fn revive_arrow(&mut self, entity_id: i32, nbt: &Nbt) {
        let data = Arc::clone(&self.context.data);
        let world = Arc::clone(&self.context.world);
        let Some(pos) = read_doubles(nbt, "Pos") else {
            return;
        };
        let velocity = read_doubles(nbt, "Motion").unwrap_or_default();
        let byte = |key| match nbt.get(key) {
            Some(Nbt::Byte(b)) => i32::from(*b),
            _ => 0,
        };
        let pickup = match byte("pickup") {
            1 => Pickup::Allowed,
            2 => Pickup::CreativeOnly,
            _ => Pickup::Disallowed,
        };
        let stuck = if byte("inGround") != 0 {
            let near = [
                pos[0] - 0.06,
                pos[1] - 0.06,
                pos[2] - 0.06,
                pos[0] + 0.06,
                pos[1] + 0.06,
                pos[2] + 0.06,
            ];
            let saved = nbt
                .get("inBlockState")
                .and_then(|s| state_from_nbt(s, &data));
            let mut found = None;
            'search: for x in near[0].floor() as i32..=near[3].floor() as i32 {
                for y in near[1].floor() as i32..=near[4].floor() as i32 {
                    for z in near[2].floor() as i32..=near[5].floor() as i32 {
                        let block = BlockPos { x, y, z };
                        let state = world.block(block);
                        let touches = data.collision_shape(state).iter().any(|b| {
                            let b = [
                                b[0] + f64::from(x),
                                b[1] + f64::from(y),
                                b[2] + f64::from(z),
                                b[3] + f64::from(x),
                                b[4] + f64::from(y),
                                b[5] + f64::from(z),
                            ];
                            (0..3).all(|i| b[i] < near[i + 3] && b[i + 3] > near[i])
                        });
                        if touches && saved.is_none_or(|s| s == state) {
                            found = Some((block, state));
                            break 'search;
                        }
                    }
                }
            }
            found
        } else {
            None
        };
        let rotation = match nbt.get("Rotation") {
            Some(Nbt::List(r)) => match r[..] {
                [Nbt::Float(yaw), Nbt::Float(pitch)] => [yaw, pitch],
                _ => [0.0, 0.0],
            },
            _ => [0.0, 0.0],
        };
        let owner = nbt
            .get("Owner")
            .and_then(uuid_from_ints)
            .and_then(|u| self.entity_of(u));
        let pose = Pose {
            pos,
            yaw: rotation[0],
            pitch: rotation[1],
            head_yaw: 0.0,
            on_ground: false,
        };
        let spectral =
            matches!(nbt.get("id"), Some(Nbt::String(id)) if id == "minecraft:spectral_arrow");
        let default = if spectral {
            "minecraft:spectral_arrow"
        } else {
            "minecraft:arrow"
        };
        let item = nbt
            .get("item")
            .and_then(|i| crate::storage::item_from_nbt(i, &data))
            .or_else(|| Some(crate::stack::plain(data.item_id(default)?, 1)));
        let Some(item) = item else {
            return;
        };
        let weapon = nbt
            .get("weapon")
            .and_then(|w| crate::storage::item_from_nbt(w, &data));
        let color = self.arrow_color(&item);
        let mut arrow = Arrow {
            entity_id,
            uuid: uuid_from_nbt(nbt).unwrap_or_else(Uuid::new_v4),
            owner,
            pos,
            velocity,
            yaw: rotation[0],
            pitch: rotation[1],
            base_damage: match nbt.get("damage") {
                Some(Nbt::Double(d)) => *d,
                _ => 2.0,
            },
            crit: byte("crit") != 0,
            pickup,
            stuck,
            fire: match nbt.get("Fire") {
                Some(Nbt::Short(f)) => i32::from(*f).max(0),
                _ => 0,
            },
            shake: byte("shake").max(0) as u32,
            life: match nbt.get("life") {
                Some(Nbt::Short(l)) => (*l).max(0) as u32,
                _ => 0,
            },
            item,
            weapon,
            spectral,
            color,
            tracker: Tracker::new(&pose, UPDATE_INTERVAL),
            sent_velocity: velocity,
            data_dirty: false,
            state: None,
        };
        let state = arrow.state(self.arrow_type(spectral));
        self.arrows.insert(entity_id, arrow);
        self.broadcast(None, &Outbound::EntityAdded(state));
    }

    /// How the arrows look now, for a player who just joined.
    pub(super) fn arrow_states(&mut self) -> Vec<Arc<EntityState>> {
        let (arrow, spectral) = (self.arrow_type(false), self.arrow_type(true));
        self.arrows
            .values_mut()
            .map(|a| a.state(if a.spectral { spectral } else { arrow }))
            .collect()
    }

    /// `Entity.push`: a mob moves at once, a player's client is told.
    pub(super) fn push_living(&mut self, entity_id: i32, by: [f64; 3]) {
        if let Some(mob) = self.mobs.get_mut(&entity_id) {
            for (v, d) in mob.velocity.iter_mut().zip(by) {
                *v += d;
            }
            mob.mark_dirty();
        } else if let Some(player) = self.players.get(&entity_id) {
            player.send(Outbound::Packet(Arc::new(Encoded::new(&SetEntityMotion {
                entity_id,
                velocity: LpVec3(by),
            }))));
        }
    }

    /// `minecraft:projectile_spawned` of a weapon: flame's fire, in ticks.
    fn flame(&self, weapon: &ItemStack) -> i32 {
        let data = &self.context.data;
        crate::stack::enchantments(data, weapon)
            .into_iter()
            .filter_map(|(name, level)| {
                let (_, json) = data.enchantment(&name)?;
                let effects = json["effects"]["minecraft:projectile_spawned"].as_array()?;
                effects
                    .iter()
                    .filter(|e| e["effect"]["type"] == "minecraft:ignite")
                    .map(|e| {
                        (super::enchant::level_value(&e["effect"]["duration"], level) * 20.0) as i32
                    })
                    .max()
            })
            .max()
            .unwrap_or(0)
    }

    fn remove_arrow(&mut self, entity_id: i32) {
        if self.arrows.remove(&entity_id).is_some() {
            self.broadcast(None, &Outbound::EntityRemoved(entity_id));
        }
    }

    pub(super) fn tick_arrows(&mut self) {
        let ids: Vec<i32> = self.arrows.keys().copied().collect();
        for entity_id in ids {
            self.tick_arrow(entity_id);
        }
    }

    fn tick_arrow(&mut self, entity_id: i32) {
        let data = Arc::clone(&self.context.data);
        let world = Arc::clone(&self.context.world);
        let Some(arrow) = self.arrows.get_mut(&entity_id) else {
            return;
        };
        if let Some((block, state)) = arrow.stuck {
            // The block it stuck in is gone: it falls again.
            if world.block(block) != state {
                arrow.stuck = None;
                arrow.life = 0;
                let mut rng = rand::rng();
                arrow.velocity = [
                    rng.random::<f64>() * 0.2,
                    rng.random::<f64>() * 0.2,
                    rng.random::<f64>() * 0.2,
                ];
                arrow.data_dirty = true;
            } else {
                arrow.life += 1;
                arrow.shake = arrow.shake.saturating_sub(1);
                if arrow.life >= LIFETIME {
                    return self.remove_arrow(entity_id);
                }
                // `Arrow.tick`: the potion wears off a stuck tipped arrow.
                if arrow.color != -1 && arrow.life >= POTION_DECAY && !arrow.spectral {
                    arrow.color = -1;
                    arrow.data_dirty = true;
                    if let Some(plain) = self.context.data.item_id("minecraft:arrow") {
                        arrow.item = crate::stack::plain(plain, 1);
                    }
                    let puff = Encoded::new(&ferrocraft_protocol::packets::play::EntityEvent {
                        entity_id,
                        event: 0,
                    });
                    self.broadcast(None, &Outbound::Packet(Arc::new(puff)));
                }
                self.pick_up_arrow(entity_id);
                return self.sync_arrow(entity_id);
            }
        }
        let from = arrow.pos;
        let mut to = [
            from[0] + arrow.velocity[0],
            from[1] + arrow.velocity[1],
            from[2] + arrow.velocity[2],
        ];
        let block_hit = physics::clip(&world, &data, from, to);
        if let Some(hit) = block_hit {
            to = hit.at;
        }
        let owner = arrow.owner;
        // The first player or mob on the way, but not the shooter.
        let mut victims: Vec<(i32, f64)> = self
            .players
            .values()
            .filter(|p| !p.dead && Some(p.entity_id) != owner)
            .filter(|p| p.game_mode() != ferrocraft_protocol::packets::play::GameMode::Spectator)
            .filter_map(|p| Some((p.entity_id, line_hits(from, to, &p.bounds(), MARGIN)?)))
            .collect();
        victims.extend(
            self.mob_boxes()
                .into_iter()
                .filter(|&(id, _)| Some(id) != owner)
                .filter_map(|(id, b)| Some((id, line_hits(from, to, &b, MARGIN)?))),
        );
        let victim = victims
            .into_iter()
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id);
        if let Some(victim) = victim {
            return self.arrow_hits(entity_id, victim);
        }
        let Some(arrow) = self.arrows.get_mut(&entity_id) else {
            return;
        };
        match block_hit {
            Some(hit) => {
                // `AbstractArrow.onHitBlock`: a little short of the face.
                let speed = arrow.velocity.iter().map(|v| v * v).sum::<f64>().sqrt();
                let back = arrow.velocity.map(|v| v / speed.max(1.0e-6) * 0.05);
                arrow.pos = [
                    hit.at[0] - back[0],
                    hit.at[1] - back[1],
                    hit.at[2] - back[2],
                ];
                arrow.velocity = [0.0; 3];
                let (burning, shooter) = (arrow.fire > 0, arrow.owner);
                // `TntBlock.onProjectileHit`: a burning arrow lights it.
                if burning && self.kind(world.block(hit.block)) == super::blocks::Kind::Tnt {
                    let may = shooter.is_none_or(|o| {
                        self.players.contains_key(&o) || self.rules.flag("mob_griefing")
                    });
                    if may && self.rules.flag("tnt_explodes") {
                        self.remove_block_keeping_fluid(hit.block);
                        self.prime_tnt(hit.block, shooter, super::tnt::DEFAULT_FUSE);
                    }
                }
                let Some(arrow) = self.arrows.get_mut(&entity_id) else {
                    return;
                };
                arrow.stuck = Some((hit.block, world.block(hit.block)));
                arrow.shake = SHAKE;
                arrow.crit = false;
                arrow.life = 0;
                arrow.data_dirty = true;
                let at = arrow.pos;
                let pitch = 1.2 / (rand::rng().random::<f32>() * 0.2 + 0.9);
                self.play_sound_from(
                    None,
                    "minecraft:entity.arrow.hit",
                    at,
                    SOUND_SOURCE_NEUTRAL,
                    1.0,
                    pitch,
                );
            }
            None => {
                let start = arrow.bounds();
                arrow.pos = to;
                let burning = arrow.fire;
                // The blocks along the way (`checkInsideBlocks` over the
                // movement), start and end.
                let end = arrow.bounds();
                let bounds: Aabb = std::array::from_fn(|i| {
                    if i < 3 {
                        start[i].min(end[i])
                    } else {
                        start[i].max(end[i])
                    }
                });
                let pos = arrow.pos;
                let fire = self.burning_thing(bounds, pos, burning);
                let Some(arrow) = self.arrows.get_mut(&entity_id) else {
                    return;
                };
                if (fire > 0) != (burning > 0) {
                    arrow.data_dirty = true;
                }
                arrow.fire = fire;
                let in_water = data.physics(world.block(BlockPos {
                    x: to[0].floor() as i32,
                    y: to[1].floor() as i32,
                    z: to[2].floor() as i32,
                }));
                let drag = if in_water.fluid == 1 {
                    WATER_DRAG
                } else {
                    DRAG
                };
                arrow.velocity = arrow.velocity.map(|v| v * drag);
                arrow.velocity[1] -= GRAVITY;
                arrow.face_velocity();
                arrow.life += 1;
                // Lost in the void or flying forever.
                if arrow.pos[1] < f64::from(crate::world::MIN_Y - 64) || arrow.life > LIFETIME {
                    return self.remove_arrow(entity_id);
                }
            }
        }
        self.sync_arrow(entity_id);
    }

    /// `AbstractArrow.onHitEntity`: damage by speed, then the arrow is
    /// gone; one that does no harm bounces off.
    fn arrow_hits(&mut self, entity_id: i32, victim: i32) {
        let Some(arrow) = self.arrows.get(&entity_id) else {
            return;
        };
        let speed = arrow.velocity.iter().map(|v| v * v).sum::<f64>().sqrt();
        let weapon = arrow.weapon.clone();
        let base = arrow.base_damage;
        let (crit, velocity, item, spectral) = (
            arrow.crit,
            arrow.velocity,
            arrow.item.clone(),
            arrow.spectral,
        );
        let owner = arrow.owner;
        // `EnchantmentHelper.modifyDamage`: power.
        let base = weapon.as_ref().map_or(base, |w| {
            f64::from(self.enchanted_damage(
                w,
                victim,
                "minecraft:arrow",
                owner,
                Some(entity_id),
                base as f32,
            ))
        });
        let mut damage = (speed * base).clamp(0.0, f64::from(i32::MAX)).ceil() as i32;
        if crit {
            damage += rand::rng().random_range(0..damage / 2 + 2);
        }
        let damage = damage as f32;
        let Some(arrow) = self.arrows.get(&entity_id) else {
            return;
        };
        let (owner, at, burning) = (arrow.owner, arrow.pos, arrow.fire > 0);
        let hurt = if self.mobs.contains_key(&victim) {
            self.hurt_mob_by(
                victim,
                damage,
                owner,
                Some((entity_id, at)),
                "minecraft:arrow",
            )
        } else if let Some(player) = self.players.get(&victim) {
            let shooter = owner.and_then(|o| self.entity_name(o));
            let message = text::translate(
                "death.attack.arrow",
                std::iter::once(text::player_name(player.name(), player.uuid()))
                    .chain(shooter)
                    .collect(),
                None,
            );
            self.hurt_player(
                victim,
                Hit {
                    amount: damage,
                    damage_type: "minecraft:arrow",
                    attacker: owner,
                    direct: Some(entity_id),
                    knockback_from: Some(at),
                    message,
                },
            )
        } else {
            false
        };
        if hurt && burning {
            // `onHitEntity`: a burning arrow sets what it hits alight.
            if self.mobs.contains_key(&victim) {
                self.ignite_mob(victim, 5);
            } else {
                self.ignite_player(victim, 5);
            }
        }
        if hurt && let Some(living) = self.living(victim) {
            // `doKnockback`: punch, along the flight.
            let knockback = weapon.as_ref().map_or(0.0, |w| {
                self.enchanted_knockback(w, victim, "minecraft:arrow", owner, 0.0)
            });
            if knockback > 0.0 {
                let resistance =
                    (1.0 - self.attribute(living, "minecraft:knockback_resistance")).max(0.0);
                let length = velocity[0].hypot(velocity[2]).max(1.0e-6);
                let push = f64::from(knockback) * 0.6 * resistance;
                let movement = [
                    velocity[0] / length * push,
                    0.1,
                    velocity[2] / length * push,
                ];
                self.push_living(victim, movement);
            }
            // Thorns and the like.
            self.post_attack(living, owner, Some(entity_id), "minecraft:arrow");
            // `doPostHurtEffects`: the potion of a tipped arrow, the glow of
            // a spectral one.
            let data = Arc::clone(&self.context.data);
            if spectral {
                if let Some((glowing, _)) = data.effect("minecraft:glowing") {
                    self.add_effect(living, super::effect::EffectInstance::new(glowing, 200, 0));
                }
            } else if let Some(contents) =
                crate::stack::get(&data, &item, "minecraft:potion_contents")
            {
                let scale = crate::stack::float(&data, &item, "minecraft:potion_duration_scale")
                    .unwrap_or(1.0);
                for effect in super::consume::potion_effects(&data, &contents, scale) {
                    let info = &data.alchemy.effects[effect.effect];
                    if info.instant {
                        let name = info.name.clone();
                        self.apply_instant(living, &name, effect.amplifier, 1.0, owner);
                    } else {
                        self.add_effect(living, effect);
                    }
                }
            }
        }
        if hurt {
            // The shooter hears it when a player is hit.
            if self.players.contains_key(&victim)
                && let Some(shooter) = owner.and_then(|o| self.players.get(&o))
            {
                shooter.send(Outbound::Packet(Arc::new(Encoded::new(&GameEvent {
                    event: ARROW_HIT_PLAYER,
                    param: 0.0,
                }))));
            }
            let pitch = 1.2 / (rand::rng().random::<f32>() * 0.2 + 0.9);
            self.play_sound_from(
                None,
                "minecraft:entity.arrow.hit",
                at,
                SOUND_SOURCE_NEUTRAL,
                1.0,
                pitch,
            );
            self.remove_arrow(entity_id);
        } else if let Some(arrow) = self.arrows.get_mut(&entity_id) {
            arrow.velocity = arrow.velocity.map(|v| v * -0.1);
            arrow.yaw += 180.0;
            self.sync_arrow(entity_id);
        }
    }

    /// `AbstractArrow.playerTouch`: a stuck arrow that has stopped quivering
    /// goes to a player who touches it, if it may.
    fn pick_up_arrow(&mut self, entity_id: i32) {
        let Some(arrow) = self.arrows.get(&entity_id) else {
            return;
        };
        if arrow.shake > 0 || arrow.pickup == Pickup::Disallowed {
            return;
        }
        let bounds = arrow.bounds();
        let pickup = arrow.pickup;
        let Some(taker) = self
            .players
            .values()
            .filter(|p| !p.dead && p.game_mode() != GameMode::Spectator)
            .filter(|p| pickup == Pickup::Allowed || p.game_mode() == GameMode::Creative)
            .find(|p| {
                let b = p.bounds();
                let reach = [
                    b[0] - 1.0,
                    b[1] - 0.5,
                    b[2] - 1.0,
                    b[3] + 1.0,
                    b[4] + 0.5,
                    b[5] + 1.0,
                ];
                (0..3).all(|i| reach[i] < bounds[i + 3] && reach[i + 3] > bounds[i])
            })
            .map(|p| p.entity_id)
        else {
            return;
        };
        if pickup == Pickup::Allowed {
            let stack = arrow.item.clone();
            let mut leftover = None;
            let mut changed = Vec::new();
            self.update_inventory(taker, |inventory| {
                (leftover, changed) = inventory.add(stack, 64);
            });
            if leftover.is_some() {
                return;
            }
            self.send_slots(taker, &changed);
        }
        let take = Encoded::new(&TakeItemEntity {
            item_id: entity_id,
            player_id: taker,
            amount: 1,
        });
        self.broadcast(None, &Outbound::Packet(Arc::new(take)));
        self.remove_arrow(entity_id);
    }

    fn sync_arrow(&mut self, entity_id: i32) {
        let spectral = self.arrows.get(&entity_id).is_some_and(|a| a.spectral);
        let arrow_type = self.arrow_type(spectral);
        let Some(arrow) = self.arrows.get_mut(&entity_id) else {
            return;
        };
        let dirty = std::mem::take(&mut arrow.data_dirty);
        let pose = arrow.pose();
        let mut packets = arrow.tracker.sync(entity_id, &pose, dirty);
        if !packets.is_empty() {
            arrow.sent_velocity = arrow.velocity;
            packets.push(Encoded::new(&SetEntityMotion {
                entity_id,
                velocity: LpVec3(arrow.velocity),
            }));
        }
        if dirty {
            packets.push(Encoded::new(&SetEntityData {
                entity_id,
                values: EntityDataValues(arrow.data()),
            }));
        }
        if !packets.is_empty() {
            arrow.state = None;
            let message = Outbound::Entity {
                state: arrow.state(arrow_type),
                packets: packets.into(),
            };
            self.broadcast(None, &message);
        }
    }
}
