//! Primed TNT (`PrimedTnt`): lit by flint and steel, a fire charge or
//! another explosion, it hops up, falls, and blows up after its fuse; the
//! blast breaks blocks without losing their drops, as `TNT` explosions do.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    EntityData, EntityDataValue, GameMode, LpVec3, SOUND_SOURCE_BLOCKS, SetEntityMotion, UseItemOn,
};
use rand::Rng;
use uuid::Uuid;

use ferrocraft_protocol::Nbt;

use super::explosion::Blast;
use super::mob::save::{doubles, read_doubles, uuid_from_ints, uuid_from_nbt, uuid_to_nbt};
use super::physics;
use super::tracker::{Pose, Tracker};
use super::{Encoded, EntityState, Game, Outbound};
use crate::data::Aabb;
use crate::inventory;

/// `PrimedTnt.DATA_FUSE_ID`, after `Entity`'s 0–7.
const FUSE: u8 = 8;
/// A fuse of four seconds (`PrimedTnt.DEFAULT_FUSE_TIME`).
pub const DEFAULT_FUSE: i32 = 80;
const POWER: f32 = 4.0;
const GRAVITY: f64 = 0.04;
const AIR_DRAG: f64 = 0.98;
const HALF: f64 = 0.49;
const HEIGHT: f64 = 0.98;
/// Seen up to 10 chunks away, updated every 10 ticks.
const RANGE: f64 = 10.0 * 16.0;
const UPDATE_INTERVAL: u32 = 10;

pub struct PrimedTnt {
    entity_id: i32,
    uuid: Uuid,
    pos: [f64; 3],
    velocity: [f64; 3],
    on_ground: bool,
    fuse: i32,
    /// Who lit it, to blame for the blast.
    owner: Option<i32>,
    tracker: Tracker,
    sent_velocity: [f64; 3],
    state: Option<Arc<EntityState>>,
}

impl PrimedTnt {
    fn bounds(&self) -> Aabb {
        let [x, y, z] = self.pos;
        [x - HALF, y, z - HALF, x + HALF, y + HEIGHT, z + HALF]
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

    fn state(&mut self, tnt_type: i32) -> Arc<EntityState> {
        if let Some(state) = &self.state {
            return Arc::clone(state);
        }
        let state = Arc::new(EntityState {
            entity_id: self.entity_id,
            uuid: self.uuid,
            entity_type: tnt_type,
            pos: self.tracker.sent_pos,
            yaw: 0,
            pitch: 0,
            head_yaw: 0,
            velocity: self.sent_velocity,
            object_data: 0,
            // The client counts the fuse down itself and flashes.
            data: vec![EntityDataValue {
                index: FUSE,
                value: EntityData::Int(self.fuse),
            }],
            equipment: Vec::new(),
            spectator: false,
            range: RANGE,
        });
        self.state = Some(Arc::clone(&state));
        state
    }
}

impl Game {
    fn tnt_type(&self) -> i32 {
        self.context.data.entity_type("minecraft:tnt").unwrap_or(0)
    }

    /// `TntBlock.prime`: lit TNT where the block was, hopping up in a
    /// random direction.
    pub(super) fn prime_tnt(&mut self, pos: BlockPos, owner: Option<i32>, fuse: i32) {
        let at = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        let angle = rand::rng().random::<f64>() * std::f64::consts::TAU;
        let velocity = [-angle.sin() * 0.02, 0.2, -angle.cos() * 0.02];
        let entity_id = self.context.entity_ids.fetch_add(1, Ordering::Relaxed);
        let pose = Pose {
            pos: at,
            yaw: 0.0,
            pitch: 0.0,
            head_yaw: 0.0,
            on_ground: false,
        };
        let mut tnt = PrimedTnt {
            entity_id,
            uuid: Uuid::new_v4(),
            pos: at,
            velocity,
            on_ground: false,
            fuse,
            owner,
            tracker: Tracker::new(&pose, UPDATE_INTERVAL),
            sent_velocity: velocity,
            state: None,
        };
        let state = tnt.state(self.tnt_type());
        self.tnts.insert(entity_id, tnt);
        self.broadcast(None, &Outbound::EntityAdded(state));
        self.play_sound_from(
            None,
            "minecraft:entity.tnt.primed",
            at,
            SOUND_SOURCE_BLOCKS,
            1.0,
            1.0,
        );
    }

    /// `TntBlock.useItemOn`: flint and steel or a fire charge on TNT lights
    /// it, unless the player sneaks. Returns whether it did.
    pub(super) fn light_tnt(&mut self, entity_id: i32, click: &UseItemOn) -> bool {
        let data = Arc::clone(&self.context.data);
        let Some(player) = self.players.get(&entity_id) else {
            return false;
        };
        let state = self.context.world.block(click.pos);
        let is_tnt = data
            .block_of(state)
            .is_some_and(|(_, b)| b.name == "minecraft:tnt");
        let slot = if click.hand == 1 {
            inventory::OFF_HAND
        } else {
            player.inventory.selected_slot()
        };
        let held = player
            .inventory
            .get(slot)
            .and_then(|s| data.item(s.item))
            .map(|i| i.name.as_str());
        let lighter = matches!(
            held,
            Some("minecraft:flint_and_steel" | "minecraft:fire_charge")
        );
        if !is_tnt || !lighter || player.sneaking {
            return false;
        }
        let creative = player.game_mode() == GameMode::Creative;
        let fire_charge = held == Some("minecraft:fire_charge");
        self.set_block(click.pos, self.air);
        self.prime_tnt(click.pos, Some(entity_id), DEFAULT_FUSE);
        self.swing(entity_id);
        if !creative {
            if fire_charge {
                self.update_inventory(entity_id, |inventory| {
                    inventory.take(slot, 1);
                });
                self.send_slots(entity_id, &[slot]);
            } else {
                self.wear(entity_id, slot, 1);
            }
        }
        true
    }

    /// Lit TNT, for explosions to push about.
    pub(super) fn tnt_boxes(&self) -> Vec<(i32, Aabb)> {
        self.tnts
            .values()
            .map(|t| (t.entity_id, t.bounds()))
            .collect()
    }

    pub(super) fn fling_tnt(&mut self, entity_id: i32, by: [f64; 3]) {
        if let Some(tnt) = self.tnts.get_mut(&entity_id) {
            for (v, d) in tnt.velocity.iter_mut().zip(by) {
                *v += d;
            }
        }
    }

    /// `PrimedTnt.addAdditionalSaveData` after the entity's own.
    pub(super) fn tnt_nbt(&self) -> Vec<([f64; 3], Nbt)> {
        self.tnts
            .values()
            .map(|t| {
                let mut entries = vec![
                    ("id".to_owned(), Nbt::String("minecraft:tnt".into())),
                    ("Pos".into(), doubles(&t.pos)),
                    ("Motion".into(), doubles(&t.velocity)),
                    (
                        "Rotation".into(),
                        Nbt::List(vec![Nbt::Float(0.0), Nbt::Float(0.0)]),
                    ),
                    ("OnGround".into(), Nbt::Byte(i8::from(t.on_ground))),
                    ("UUID".into(), uuid_to_nbt(t.uuid)),
                    ("fuse".into(), Nbt::Short(t.fuse.clamp(0, 32767) as i16)),
                    (
                        "block_state".into(),
                        Nbt::Compound(vec![("Name".into(), Nbt::String("minecraft:tnt".into()))]),
                    ),
                ];
                if let Some(owner) = t.owner.and_then(|o| self.uuid_of(o)) {
                    entries.push(("owner".into(), uuid_to_nbt(owner)));
                }
                (t.pos, Nbt::Compound(entries))
            })
            .collect()
    }

    /// Lit TNT from the entity store, burning on.
    pub(super) fn revive_tnt(&mut self, entity_id: i32, nbt: &Nbt) {
        let Some(pos) = read_doubles(nbt, "Pos") else {
            return;
        };
        let velocity = read_doubles(nbt, "Motion").unwrap_or_default();
        let fuse = match nbt.get("fuse") {
            Some(Nbt::Short(f)) => i32::from(*f),
            _ => DEFAULT_FUSE,
        };
        let owner = nbt
            .get("owner")
            .and_then(uuid_from_ints)
            .and_then(|u| self.entity_of(u));
        let pose = Pose {
            pos,
            yaw: 0.0,
            pitch: 0.0,
            head_yaw: 0.0,
            on_ground: false,
        };
        let mut tnt = PrimedTnt {
            entity_id,
            uuid: uuid_from_nbt(nbt).unwrap_or_else(Uuid::new_v4),
            pos,
            velocity,
            on_ground: matches!(nbt.get("OnGround"), Some(Nbt::Byte(1))),
            fuse,
            owner,
            tracker: Tracker::new(&pose, UPDATE_INTERVAL),
            sent_velocity: velocity,
            state: None,
        };
        let state = tnt.state(self.tnt_type());
        self.tnts.insert(entity_id, tnt);
        self.broadcast(None, &Outbound::EntityAdded(state));
    }

    /// Moves lit TNT, as a piston does.
    pub(super) fn shift_tnt(&mut self, entity_id: i32, by: [f64; 3]) {
        if let Some(tnt) = self.tnts.get_mut(&entity_id) {
            for (p, d) in tnt.pos.iter_mut().zip(by) {
                *p += d;
            }
        }
    }

    /// How the lit TNT looks now, for a player who just joined.
    pub(super) fn tnt_states(&mut self) -> Vec<Arc<EntityState>> {
        let tnt_type = self.tnt_type();
        self.tnts.values_mut().map(|t| t.state(tnt_type)).collect()
    }

    /// `PrimedTnt.tick` for all of them.
    pub(super) fn tick_tnt(&mut self) {
        let ids: Vec<i32> = self.tnts.keys().copied().collect();
        for entity_id in ids {
            self.tick_one_tnt(entity_id);
        }
    }

    fn tick_one_tnt(&mut self, entity_id: i32) {
        let data = Arc::clone(&self.context.data);
        let world = Arc::clone(&self.context.world);
        let Some(tnt) = self.tnts.get_mut(&entity_id) else {
            return;
        };
        tnt.velocity[1] -= GRAVITY;
        let moved = physics::collide(&world, &data, tnt.bounds(), tnt.velocity);
        for (axis, distance) in moved.movement.iter().enumerate() {
            tnt.pos[axis] += distance;
            if *distance != tnt.velocity[axis] {
                tnt.velocity[axis] = 0.0;
            }
        }
        tnt.on_ground = moved.on_ground;
        tnt.velocity = tnt.velocity.map(|v| v * AIR_DRAG);
        if tnt.on_ground {
            tnt.velocity = [
                tnt.velocity[0] * 0.7,
                tnt.velocity[1] * -0.5,
                tnt.velocity[2] * 0.7,
            ];
        }
        tnt.fuse -= 1;
        if tnt.fuse <= 0 || tnt.pos[1] < f64::from(crate::world::MIN_Y - 64) {
            let (pos, owner) = (tnt.pos, tnt.owner);
            self.tnts.remove(&entity_id);
            self.broadcast(None, &Outbound::EntityRemoved(entity_id));
            self.explode(Blast {
                // `getY(0.0625)`: a little above its bottom.
                center: [pos[0], pos[1] + HEIGHT * 0.0625, pos[2]],
                radius: POWER,
                source: Some(entity_id),
                attacker: owner,
                breaks_blocks: true,
                decay: false,
            });
            return;
        }
        let pose = tnt.pose();
        let mut packets = tnt.tracker.sync(entity_id, &pose, false);
        if !packets.is_empty() {
            tnt.sent_velocity = tnt.velocity;
            packets.push(Encoded::new(&SetEntityMotion {
                entity_id,
                velocity: LpVec3(tnt.velocity),
            }));
            tnt.state = None;
            let tnt_type = self.tnt_type();
            let Some(tnt) = self.tnts.get_mut(&entity_id) else {
                return;
            };
            let message = Outbound::Entity {
                state: tnt.state(tnt_type),
                packets: packets.into(),
            };
            self.broadcast(None, &message);
        }
    }
}
