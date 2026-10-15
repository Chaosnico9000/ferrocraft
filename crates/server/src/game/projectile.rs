//! Arrows (`AbstractArrow`): they fly with gravity and drag, stick in the
//! first block they hit and hurt the first player or mob in their way by
//! how fast they fly. Skeletons' arrows cannot be picked up; stuck ones
//! vanish after a minute.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    EntityData, EntityDataValue, EntityDataValues, LpVec3, SOUND_SOURCE_NEUTRAL, SetEntityData,
    SetEntityMotion, text,
};
use rand::Rng;
use uuid::Uuid;

use super::physics;
use super::survival::Hit;
use super::tracker::{Pose, Tracker};
use super::{Encoded, EntityState, Game, Outbound};
use crate::data::Aabb;

/// `AbstractArrow.IN_GROUND`, after `ID_FLAGS` (8) and `PIERCE_LEVEL` (9).
const IN_GROUND: u8 = 10;
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
    /// Where it sticks, and the block it sticks in.
    stuck: Option<(BlockPos, u32)>,
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
            data: if self.stuck.is_some() {
                vec![EntityDataValue {
                    index: IN_GROUND,
                    value: EntityData::Boolean(true),
                }]
            } else {
                Vec::new()
            },
            equipment: Vec::new(),
            spectator: false,
            range: RANGE,
        });
        self.state = Some(Arc::clone(&state));
        state
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
    pub(super) fn shoot_arrow(
        &mut self,
        owner: Option<i32>,
        from: [f64; 3],
        direction: [f64; 3],
        speed: f64,
        inaccuracy: f64,
        base_damage: f64,
    ) {
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
            stuck: None,
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
                if arrow.life >= LIFETIME {
                    return self.remove_arrow(entity_id);
                }
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
                arrow.stuck = Some((hit.block, world.block(hit.block)));
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
                arrow.pos = to;
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
        let damage = (speed * arrow.base_damage)
            .clamp(0.0, f64::from(i32::MAX))
            .ceil() as f32;
        let (owner, at) = (arrow.owner, arrow.pos);
        let hurt = if self.mobs.contains_key(&victim) {
            self.hurt_mob(victim, damage, Some((entity_id, at)), "minecraft:arrow")
        } else if let Some(player) = self.players.get(&victim) {
            let shooter = owner.and_then(|o| self.mob_name(o));
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
        if hurt {
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
                values: EntityDataValues(vec![EntityDataValue {
                    index: IN_GROUND,
                    value: EntityData::Boolean(arrow.stuck.is_some()),
                }]),
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
