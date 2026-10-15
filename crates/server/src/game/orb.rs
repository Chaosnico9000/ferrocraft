//! Experience: the orbs that killed mobs and furnaces leave behind
//! (`ExperienceOrb`), which drift to the nearest player and are taken up,
//! and the player's level and progress (`Player.giveExperiencePoints`).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    EntityData, EntityDataValue, GameMode, LpVec3, SOUND_SOURCE_PLAYERS, SetEntityMotion,
    SetExperience, TakeItemEntity,
};
use rand::Rng;
use uuid::Uuid;

use super::mob::save::{doubles, read_doubles, uuid_from_nbt, uuid_to_nbt};
use super::physics;
use super::tracker::{Pose, Tracker};
use super::{Encoded, EntityState, Game, Outbound};
use crate::data::Aabb;

/// `ExperienceOrb.DATA_VALUE`, after `Entity`'s 0–7.
const VALUE: u8 = 8;
/// Seen up to 6 chunks away, updated once a second (`EntityTypes`).
const RANGE: f64 = 6.0 * 16.0;
const UPDATE_INTERVAL: u32 = 20;
const LIFETIME: i32 = 6000;
const HALF: f64 = 0.25;
const GRAVITY: f64 = 0.03;
/// How far an orb notices a player.
const FOLLOW: f64 = 8.0;

pub struct Orb {
    pub entity_id: i32,
    uuid: Uuid,
    pub pos: [f64; 3],
    velocity: [f64; 3],
    on_ground: bool,
    value: i32,
    /// Orbs of the same value merge into one that counts several.
    count: i32,
    age: i32,
    tracker: Tracker,
    sent_velocity: [f64; 3],
    state: Option<Arc<EntityState>>,
}

impl Orb {
    fn bounds(&self) -> Aabb {
        let [x, y, z] = self.pos;
        [x - HALF, y, z - HALF, x + HALF, y + 2.0 * HALF, z + HALF]
    }

    fn pose(&self) -> Pose {
        Pose {
            pos: self.pos,
            yaw: 0.0,
            pitch: 0.0,
            head_yaw: 0.0,
            on_ground: self.on_ground,
        }
    }

    fn state(&mut self, orb_type: i32) -> Arc<EntityState> {
        if let Some(state) = &self.state {
            return Arc::clone(state);
        }
        let state = Arc::new(EntityState {
            entity_id: self.entity_id,
            uuid: self.uuid,
            entity_type: orb_type,
            pos: self.tracker.sent_pos,
            yaw: 0,
            pitch: 0,
            head_yaw: 0,
            velocity: self.sent_velocity,
            object_data: 0,
            data: vec![EntityDataValue {
                index: VALUE,
                value: EntityData::Int(self.value),
            }],
            equipment: Vec::new(),
            spectator: false,
            range: RANGE,
        });
        self.state = Some(Arc::clone(&state));
        state
    }

    pub fn to_nbt(&self) -> Nbt {
        Nbt::Compound(vec![
            ("id".into(), Nbt::String("minecraft:experience_orb".into())),
            ("Pos".into(), doubles(&self.pos)),
            ("Motion".into(), doubles(&self.velocity)),
            (
                "Rotation".into(),
                Nbt::List(vec![Nbt::Float(0.0), Nbt::Float(0.0)]),
            ),
            ("OnGround".into(), Nbt::Byte(i8::from(self.on_ground))),
            ("UUID".into(), uuid_to_nbt(self.uuid)),
            ("Health".into(), Nbt::Short(5)),
            ("Age".into(), Nbt::Short(self.age as i16)),
            ("Value".into(), Nbt::Short(self.value as i16)),
            ("Count".into(), Nbt::Int(self.count)),
        ])
    }
}

/// `ExperienceOrb.getExperienceValue`: the largest orb size that fits.
pub fn orb_value(left: i32) -> i32 {
    [2477, 1237, 617, 307, 149, 73, 37, 17, 7, 3]
        .into_iter()
        .find(|&v| left >= v)
        .unwrap_or(1)
}

/// `Player.getXpNeededForNextLevel`.
pub fn needed_for(level: i32) -> i32 {
    if level >= 30 {
        112 + (level - 30) * 9
    } else if level >= 15 {
        37 + (level - 15) * 5
    } else {
        7 + level * 2
    }
}

fn overlaps(a: &Aabb, b: &Aabb) -> bool {
    (0..3).all(|i| a[i] < b[i + 3] && a[i + 3] > b[i])
}

impl Game {
    fn orb_type(&self) -> i32 {
        self.context
            .data
            .entity_type("minecraft:experience_orb")
            .unwrap_or(0)
    }

    /// `ExperienceOrb.award`: orbs worth `amount` together, merged into
    /// orbs of the same value nearby where there are some.
    pub(super) fn award_experience(&mut self, pos: [f64; 3], mut amount: i32) {
        let mut rng = rand::rng();
        while amount > 0 {
            let value = orb_value(amount);
            amount -= value;
            let area = [
                pos[0] - 0.5,
                pos[1] - 0.5,
                pos[2] - 0.5,
                pos[0] + 0.5,
                pos[1] + 0.5,
                pos[2] + 0.5,
            ];
            if let Some(orb) = self
                .orbs
                .values_mut()
                .find(|o| o.value == value && overlaps(&o.bounds(), &area))
            {
                orb.count += 1;
                orb.age = 0;
                continue;
            }
            let velocity = [
                (rng.random::<f64>() * 0.2 - 0.1) * 2.0,
                rng.random::<f64>() * 0.2 * 2.0,
                (rng.random::<f64>() * 0.2 - 0.1) * 2.0,
            ];
            let entity_id = self.context.entity_ids.fetch_add(1, Ordering::Relaxed);
            self.add_orb(Orb {
                entity_id,
                uuid: Uuid::new_v4(),
                pos,
                velocity,
                on_ground: false,
                value,
                count: 1,
                age: 0,
                tracker: Tracker::new(
                    &Pose {
                        pos,
                        yaw: 0.0,
                        pitch: 0.0,
                        head_yaw: 0.0,
                        on_ground: false,
                    },
                    UPDATE_INTERVAL,
                ),
                sent_velocity: velocity,
                state: None,
            });
        }
    }

    fn add_orb(&mut self, mut orb: Orb) {
        let state = orb.state(self.orb_type());
        self.orbs.insert(orb.entity_id, orb);
        self.broadcast(None, &Outbound::EntityAdded(state));
    }

    /// An orb from the entity store.
    pub(super) fn revive_orb(&mut self, entity_id: i32, nbt: &Nbt) {
        let Some(pos) = read_doubles(nbt, "Pos") else {
            return;
        };
        let short = |key| match nbt.get(key) {
            Some(Nbt::Short(v)) => i32::from(*v),
            _ => 0,
        };
        let velocity = read_doubles(nbt, "Motion").unwrap_or_default();
        self.add_orb(Orb {
            entity_id,
            uuid: uuid_from_nbt(nbt).unwrap_or_else(Uuid::new_v4),
            pos,
            velocity,
            on_ground: matches!(nbt.get("OnGround"), Some(Nbt::Byte(1))),
            value: short("Value").max(1),
            count: match nbt.get("Count") {
                Some(Nbt::Int(c)) if *c > 0 => *c,
                _ => 1,
            },
            age: short("Age"),
            tracker: Tracker::new(
                &Pose {
                    pos,
                    yaw: 0.0,
                    pitch: 0.0,
                    head_yaw: 0.0,
                    on_ground: false,
                },
                UPDATE_INTERVAL,
            ),
            sent_velocity: velocity,
            state: None,
        });
    }

    /// How the orbs look now, for a player who just joined.
    pub(super) fn orb_states(&mut self) -> Vec<Arc<EntityState>> {
        let orb_type = self.orb_type();
        self.orbs.values_mut().map(|o| o.state(orb_type)).collect()
    }

    pub(super) fn remove_orb(&mut self, entity_id: i32) {
        if self.orbs.remove(&entity_id).is_some() {
            self.broadcast(None, &Outbound::EntityRemoved(entity_id));
        }
    }

    /// Every orb for one tick: `ExperienceOrb.tick` and being taken up.
    pub(super) fn tick_orbs(&mut self) {
        let ids: Vec<i32> = self.orbs.keys().copied().collect();
        for entity_id in ids {
            self.tick_orb(entity_id);
        }
        for player in self.players.values_mut() {
            player.xp_delay = player.xp_delay.saturating_sub(1);
        }
    }

    fn tick_orb(&mut self, entity_id: i32) {
        let data = Arc::clone(&self.context.data);
        let world = Arc::clone(&self.context.world);
        let Some(orb) = self.orbs.get(&entity_id) else {
            return;
        };
        // `followNearbyPlayer`.
        let follow = self
            .players
            .values()
            .filter(|p| p.game_mode() != GameMode::Spectator && !p.dead)
            .map(|p| {
                // Half the eye height up.
                let target = [p.pos[0], (p.pos[1] + p.eyes()[1]) / 2.0, p.pos[2]];
                let d: f64 = (0..3).map(|i| (target[i] - orb.pos[i]).powi(2)).sum();
                (d, target)
            })
            .filter(|&(d, _)| d < FOLLOW * FOLLOW)
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let Some(orb) = self.orbs.get_mut(&entity_id) else {
            return;
        };
        if !physics::blocked(&world, &data, orb.bounds()) {
            orb.velocity[1] -= GRAVITY;
        }
        if let Some((distance_sq, target)) = follow {
            let delta = [
                target[0] - orb.pos[0],
                target[1] - orb.pos[1],
                target[2] - orb.pos[2],
            ];
            let length = distance_sq.sqrt();
            let power = 1.0 - length.sqrt() / 8.0;
            if length > 1.0e-6 {
                for (v, d) in orb.velocity.iter_mut().zip(delta) {
                    *v += d / length * power * power * 0.1;
                }
            }
        }
        let fall = orb.velocity[1];
        let moved = physics::collide(&world, &data, orb.bounds(), orb.velocity);
        for (axis, distance) in moved.movement.iter().enumerate() {
            orb.pos[axis] += distance;
            if *distance != orb.velocity[axis] {
                orb.velocity[axis] = 0.0;
            }
        }
        orb.on_ground = moved.on_ground;
        let mut friction = 0.98;
        if orb.on_ground {
            let below = BlockPos {
                x: orb.pos[0].floor() as i32,
                y: (orb.pos[1] - 0.999_999).floor() as i32,
                z: orb.pos[2].floor() as i32,
            };
            friction *= physics::friction(&data, world.block(below));
        }
        orb.velocity = orb.velocity.map(|v| v * friction);
        if moved.on_ground && fall < -GRAVITY {
            orb.velocity[1] = -fall * 0.4;
        }
        orb.age += 1;
        if orb.age >= LIFETIME {
            return self.remove_orb(entity_id);
        }
        self.take_orb(entity_id);
        self.sync_orb(entity_id);
    }

    /// `ExperienceOrb.playerTouch`: one of its count to a player near it,
    /// every other tick at most.
    fn take_orb(&mut self, entity_id: i32) {
        let Some(orb) = self.orbs.get(&entity_id) else {
            return;
        };
        let bounds = orb.bounds();
        let taker = self
            .players
            .values()
            .find(|p| {
                let b = p.bounds();
                p.game_mode() != GameMode::Spectator
                    && !p.dead
                    && p.xp_delay == 0
                    && overlaps(
                        &[
                            b[0] - 1.0,
                            b[1] - 0.5,
                            b[2] - 1.0,
                            b[3] + 1.0,
                            b[4] + 0.5,
                            b[5] + 1.0,
                        ],
                        &bounds,
                    )
            })
            .map(|p| p.entity_id);
        let Some(taker) = taker else {
            return;
        };
        let value = orb.value;
        if let Some(player) = self.players.get_mut(&taker) {
            player.xp_delay = 2;
        }
        let take = Encoded::new(&TakeItemEntity {
            item_id: entity_id,
            player_id: taker,
            amount: 1,
        });
        self.broadcast_orb(entity_id, vec![take]);
        self.give_experience(taker, value);
        if let Some(orb) = self.orbs.get_mut(&entity_id) {
            orb.count -= 1;
            if orb.count <= 0 {
                self.remove_orb(entity_id);
            }
        }
    }

    fn broadcast_orb(&mut self, entity_id: i32, packets: Vec<Encoded>) {
        let orb_type = self.orb_type();
        let Some(orb) = self.orbs.get_mut(&entity_id) else {
            return;
        };
        let message = Outbound::Entity {
            state: orb.state(orb_type),
            packets: packets.into(),
        };
        self.broadcast(None, &message);
    }

    fn sync_orb(&mut self, entity_id: i32) {
        let Some(orb) = self.orbs.get_mut(&entity_id) else {
            return;
        };
        let pose = orb.pose();
        let mut packets = orb.tracker.sync(entity_id, &pose, false);
        if !packets.is_empty() {
            orb.sent_velocity = orb.velocity;
            packets.push(Encoded::new(&SetEntityMotion {
                entity_id,
                velocity: LpVec3(orb.velocity),
            }));
            orb.state = None;
            self.broadcast_orb(entity_id, packets);
        }
    }

    /// `Player.giveExperiencePoints`: points towards the next level, a
    /// fanfare every five levels.
    pub(super) fn give_experience(&mut self, entity_id: i32, points: i32) {
        let tick = self.tick;
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        let before = player.experience.level;
        player.experience.score = player.experience.score.saturating_add(points);
        player.experience.progress += points as f32 / needed_for(player.experience.level) as f32;
        player.experience.total = player.experience.total.saturating_add(points).max(0);
        while player.experience.progress < 0.0 {
            let remaining = player.experience.progress * needed_for(player.experience.level) as f32;
            if player.experience.level > 0 {
                player.experience.level -= 1;
                player.experience.progress =
                    1.0 + remaining / needed_for(player.experience.level) as f32;
            } else {
                player.experience.progress = 0.0;
            }
        }
        while player.experience.progress >= 1.0 {
            player.experience.progress =
                (player.experience.progress - 1.0) * needed_for(player.experience.level) as f32;
            player.experience.level += 1;
            player.experience.progress /= needed_for(player.experience.level) as f32;
        }
        let level = player.experience.level;
        let fanfare =
            level > before && level % 5 == 0 && tick.saturating_sub(player.last_level_up) > 100;
        let pos = player.pos;
        if fanfare {
            player.last_level_up = tick;
        }
        self.send_experience(entity_id);
        if fanfare {
            let volume = if level > 30 { 1.0 } else { level as f32 / 30.0 };
            self.play_sound_from(
                None,
                "minecraft:entity.player.levelup",
                pos,
                SOUND_SOURCE_PLAYERS,
                volume * 0.75,
                1.0,
            );
        }
    }

    /// `Player.giveExperienceLevels`.
    pub(super) fn give_levels(&mut self, entity_id: i32, levels: i32) {
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        player.experience.level = player.experience.level.saturating_add(levels);
        if player.experience.level < 0 {
            player.experience.level = 0;
            player.experience.progress = 0.0;
            player.experience.total = 0;
        }
        self.send_experience(entity_id);
    }

    pub(super) fn send_experience(&self, entity_id: i32) {
        if let Some(player) = self.players.get(&entity_id) {
            player.send(Outbound::Packet(Arc::new(Encoded::new(&SetExperience {
                progress: player.experience.progress,
                level: player.experience.level,
                total: player.experience.total,
            }))));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orbs_and_levels_like_vanilla() {
        assert_eq!(orb_value(5), 3);
        assert_eq!(orb_value(2), 1);
        assert_eq!(orb_value(100), 73);
        assert_eq!(needed_for(0), 7);
        assert_eq!(needed_for(15), 37);
        assert_eq!(needed_for(30), 112);
    }
}
