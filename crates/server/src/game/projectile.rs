//! Arrows (`AbstractArrow`): they fly with gravity and drag, stick in the
//! first block they hit and hurt the first player or mob in their way by
//! how fast they fly; a fully drawn bow's arrows hit harder at random. A
//! player's arrows can be picked up again once stuck (in creative they just
//! go), skeletons' cannot; stuck ones vanish after a minute.

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
/// `PIERCE_LEVEL` (9) between them.
const FLAGS: u8 = 8;
const IN_GROUND: u8 = 10;
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
                .filter(|v| v.value != EntityData::Byte(0) && v.value != EntityData::Boolean(false))
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
    fn arrow_type(&self) -> i32 {
        self.context
            .data
            .entity_type("minecraft:arrow")
            .unwrap_or(0)
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
        } = shot;
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
            fire: 0,
            shake: 0,
            life: 0,
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
        let state = arrow.state(self.arrow_type());
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
                let mut entries = vec![
                    ("id".to_owned(), Nbt::String("minecraft:arrow".into())),
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
                    (
                        "item".into(),
                        Nbt::Compound(vec![
                            ("id".into(), Nbt::String("minecraft:arrow".into())),
                            ("count".into(), Nbt::Int(1)),
                        ]),
                    ),
                ];
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
            tracker: Tracker::new(&pose, UPDATE_INTERVAL),
            sent_velocity: velocity,
            data_dirty: false,
            state: None,
        };
        let state = arrow.state(self.arrow_type());
        self.arrows.insert(entity_id, arrow);
        self.broadcast(None, &Outbound::EntityAdded(state));
    }

    /// How the arrows look now, for a player who just joined.
    pub(super) fn arrow_states(&mut self) -> Vec<Arc<EntityState>> {
        let arrow_type = self.arrow_type();
        self.arrows
            .values_mut()
            .map(|a| a.state(arrow_type))
            .collect()
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
                    if may {
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
        let mut damage = (speed * arrow.base_damage)
            .clamp(0.0, f64::from(i32::MAX))
            .ceil() as i32;
        if arrow.crit {
            damage += rand::rng().random_range(0..damage / 2 + 2);
        }
        let damage = damage as f32;
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
            let Some(item) = self.context.data.item_id("minecraft:arrow") else {
                return;
            };
            let stack = ItemStack {
                item,
                count: 1,
                patch: Default::default(),
            };
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
        let arrow_type = self.arrow_type();
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
