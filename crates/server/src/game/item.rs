//! Item entities: stacks lying in the world (`ItemEntity`). They fall and
//! slide, merge with their neighbours, wait a moment before they can be
//! picked up and vanish after five minutes.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    EntityData, EntityDataValue, EntityDataValues, ItemStack, SetEntityData, SetEntityMotion, Slot,
    TakeItemEntity, data_index,
};
use uuid::Uuid;

use super::physics;
use super::tracker::{Pose, Tracker};
use super::{Encoded, EntityState, Game, Outbound};
use crate::data::Aabb;

/// Items are seen up to 6 chunks away and updated once a second
/// (`EntityTypes.ITEM`).
const ITEM_RANGE: f64 = 6.0 * 16.0;
const ITEM_UPDATE_INTERVAL: u32 = 20;
/// `ItemEntity.LIFETIME`.
const LIFETIME: i32 = 6000;
/// Half the width and the height of an item entity.
const HALF_WIDTH: f64 = 0.125;
const ITEM_HEIGHT: f64 = 0.25;
const GRAVITY: f64 = 0.04;
const AIR_DRAG: f64 = 0.98;

pub struct ItemEntity {
    pub entity_id: i32,
    uuid: Uuid,
    pub stack: ItemStack,
    pub pos: [f64; 3],
    pub velocity: [f64; 3],
    on_ground: bool,
    /// Ticks until it can be picked up.
    pub pickup_delay: i32,
    age: i32,
    tick_count: u32,
    tracker: Tracker,
    sent_velocity: [f64; 3],
    /// Movement changed enough that clients should hear now.
    needs_sync: bool,
    stack_dirty: bool,
    state: Option<Arc<EntityState>>,
}

impl ItemEntity {
    fn bounds(&self) -> Aabb {
        let [x, y, z] = self.pos;
        [
            x - HALF_WIDTH,
            y,
            z - HALF_WIDTH,
            x + HALF_WIDTH,
            y + ITEM_HEIGHT,
            z + HALF_WIDTH,
        ]
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

    pub fn state(&mut self, item_type: i32) -> Arc<EntityState> {
        if let Some(state) = &self.state {
            return Arc::clone(state);
        }
        let state = Arc::new(EntityState {
            entity_id: self.entity_id,
            uuid: self.uuid,
            entity_type: item_type,
            pos: self.tracker.sent_pos,
            yaw: 0,
            pitch: 0,
            head_yaw: 0,
            velocity: self.sent_velocity,
            object_data: 0,
            data: vec![EntityDataValue {
                index: data_index::ITEM,
                value: EntityData::Item(Slot(Some(self.stack.clone()))),
            }],
            equipment: Vec::new(),
            spectator: false,
            range: ITEM_RANGE,
        });
        self.state = Some(Arc::clone(&state));
        state
    }

    fn mergeable(&self, max: i32) -> bool {
        self.pickup_delay != 32767 && self.age < LIFETIME && self.stack.count < max
    }
}

fn overlaps(a: &Aabb, b: &Aabb) -> bool {
    (0..3).all(|i| a[i] < b[i + 3] && a[i + 3] > b[i])
}

impl Game {
    fn max_stack(&self, stack: &ItemStack) -> i32 {
        self.context
            .data
            .item(stack.item)
            .map_or(64, |i| i.stack.max(1))
    }

    /// Puts a stack into the world as an item entity.
    pub(super) fn spawn_item(
        &mut self,
        pos: [f64; 3],
        velocity: [f64; 3],
        stack: ItemStack,
        pickup_delay: i32,
    ) {
        if stack.count <= 0 {
            return;
        }
        let entity_id = self.context.entity_ids.fetch_add(1, Ordering::Relaxed);
        let pose = Pose {
            pos,
            yaw: 0.0,
            pitch: 0.0,
            head_yaw: 0.0,
            on_ground: false,
        };
        let mut item = ItemEntity {
            entity_id,
            uuid: Uuid::new_v4(),
            stack,
            pos,
            velocity,
            on_ground: false,
            pickup_delay,
            age: 0,
            tick_count: 0,
            tracker: Tracker::new(&pose, ITEM_UPDATE_INTERVAL),
            sent_velocity: velocity,
            needs_sync: false,
            stack_dirty: false,
            state: None,
        };
        let state = item.state(self.item_type);
        self.items.insert(entity_id, item);
        self.broadcast(None, &Outbound::EntityAdded(state));
    }

    /// `Player.drop`: thrown from the eyes in the direction the player
    /// looks, with some scatter; it cannot be picked up for two seconds.
    pub(super) fn drop_from(&mut self, entity_id: i32, stack: ItemStack) {
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let [x, y, z] = player.eyes();
        let (yaw, pitch) = (
            f64::from(player.yaw).to_radians(),
            f64::from(player.pitch).to_radians(),
        );
        let angle = rand::random::<f64>() * std::f64::consts::TAU;
        let power = 0.02 * rand::random::<f64>();
        let velocity = [
            -yaw.sin() * pitch.cos() * 0.3 + angle.cos() * power,
            -pitch.sin() * 0.3 + 0.1 + (rand::random::<f64>() - rand::random::<f64>()) * 0.1,
            yaw.cos() * pitch.cos() * 0.3 + angle.sin() * power,
        ];
        self.spawn_item([x, y - 0.3, z], velocity, stack, 40);
    }

    /// `Player.drop` with `throwRandomly`, as when dying: in any direction
    /// from the eyes.
    pub(super) fn scatter_from(&mut self, entity_id: i32, stack: ItemStack) {
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let [x, y, z] = player.eyes();
        let power = rand::random::<f64>() * 0.5;
        let angle = rand::random::<f64>() * std::f64::consts::TAU;
        let velocity = [-angle.sin() * power, 0.2, angle.cos() * power];
        self.spawn_item([x, y - 0.3, z], velocity, stack, 40);
    }

    /// `Block.popResource`: a drop at a random spot in the block, flung up a
    /// little.
    pub(super) fn pop_resource(&mut self, pos: BlockPos, stack: ItemStack) {
        let jitter = || rand::random::<f64>() * 0.5 - 0.25;
        let at = [
            f64::from(pos.x) + 0.5 + jitter(),
            f64::from(pos.y) + 0.5 + jitter() - ITEM_HEIGHT / 2.0,
            f64::from(pos.z) + 0.5 + jitter(),
        ];
        let velocity = [
            rand::random::<f64>() * 0.2 - 0.1,
            0.2,
            rand::random::<f64>() * 0.2 - 0.1,
        ];
        self.spawn_item(at, velocity, stack, 10);
    }

    fn remove_item(&mut self, entity_id: i32) {
        if self.items.remove(&entity_id).is_some() {
            self.broadcast(None, &Outbound::EntityRemoved(entity_id));
        }
    }

    /// One tick of every item entity: `ItemEntity.tick`, pickups and what
    /// clients hear about it.
    pub(super) fn tick_items(&mut self) {
        let ids: Vec<i32> = self.items.keys().copied().collect();
        for entity_id in ids {
            self.tick_item(entity_id);
        }
    }

    fn tick_item(&mut self, entity_id: i32) {
        let data = Arc::clone(&self.context.data);
        let world = Arc::clone(&self.context.world);
        let Some(item) = self.items.get_mut(&entity_id) else {
            return;
        };
        if item.pickup_delay > 0 && item.pickup_delay != 32767 {
            item.pickup_delay -= 1;
        }
        let old_block = item.pos.map(|v| v.floor());
        let old_velocity = item.velocity;
        item.velocity[1] -= GRAVITY;
        let resting = item.on_ground
            && item.velocity[0].powi(2) + item.velocity[2].powi(2) <= 1.0e-5
            && (item.tick_count as i32 + entity_id) % 4 != 0;
        if !resting {
            let moved = physics::collide(&world, &data, item.bounds(), item.velocity);
            for (axis, distance) in moved.movement.iter().enumerate() {
                item.pos[axis] += distance;
                if *distance != item.velocity[axis] {
                    item.velocity[axis] = 0.0;
                }
            }
            item.on_ground = moved.on_ground;
            let mut horizontal = AIR_DRAG;
            if item.on_ground {
                let below = BlockPos {
                    x: item.pos[0].floor() as i32,
                    y: (item.pos[1] - 0.5).floor() as i32,
                    z: item.pos[2].floor() as i32,
                };
                horizontal *= physics::friction(&data, world.block(below));
            }
            item.velocity = [
                item.velocity[0] * horizontal,
                item.velocity[1] * AIR_DRAG,
                item.velocity[2] * horizontal,
            ];
            if item.on_ground && item.velocity[1] < 0.0 {
                item.velocity[1] *= -0.5;
            }
        } else {
            // Resting on the ground: gravity does not pile up.
            item.velocity[1] = 0.0;
        }
        let changed: f64 = item
            .velocity
            .iter()
            .zip(old_velocity)
            .map(|(a, b)| (a - b) * (a - b))
            .sum();
        if changed > 0.01 {
            item.needs_sync = true;
        }
        let block_changed = item.pos.map(|v| v.floor()) != old_block;
        item.age += 1;
        item.tick_count += 1;
        let merge_now = item
            .tick_count
            .is_multiple_of(if block_changed { 2 } else { 40 });
        if item.age >= LIFETIME {
            return self.remove_item(entity_id);
        }
        if merge_now {
            self.merge_item(entity_id);
        }
        self.pick_up(entity_id);
        self.sync_item(entity_id);
    }

    /// `ItemEntity.mergeWithNeighbours`: the larger stack takes the smaller.
    fn merge_item(&mut self, entity_id: i32) {
        let Some(item) = self.items.get(&entity_id) else {
            return;
        };
        let max = self.max_stack(&item.stack);
        if !item.mergeable(max) {
            return;
        }
        let area = {
            let b = item.bounds();
            [b[0] - 0.5, b[1], b[2] - 0.5, b[3] + 0.5, b[4], b[5] + 0.5]
        };
        let neighbours: Vec<i32> = self
            .items
            .values()
            .filter(|other| {
                other.entity_id != entity_id
                    && other.stack.item == item.stack.item
                    && other.stack.patch == item.stack.patch
                    && other.mergeable(max)
                    && overlaps(&area, &other.bounds())
            })
            .map(|other| other.entity_id)
            .collect();
        for other in neighbours {
            let (Some(this), Some(that)) = (self.items.get(&entity_id), self.items.get(&other))
            else {
                return;
            };
            if this.stack.count + that.stack.count > max {
                continue;
            }
            let (into, from) = if this.stack.count >= that.stack.count {
                (entity_id, other)
            } else {
                (other, entity_id)
            };
            let Some(from_item) = self.items.get(&from) else {
                return;
            };
            let (count, delay, age) =
                (from_item.stack.count, from_item.pickup_delay, from_item.age);
            if let Some(target) = self.items.get_mut(&into) {
                target.stack.count += count;
                target.pickup_delay = target.pickup_delay.max(delay);
                target.age = target.age.min(age);
                target.stack_dirty = true;
            }
            self.remove_item(from);
            if from == entity_id {
                return;
            }
        }
    }

    /// `ItemEntity.playerTouch` for every player close enough.
    fn pick_up(&mut self, entity_id: i32) {
        let Some(item) = self.items.get(&entity_id) else {
            return;
        };
        if item.pickup_delay > 0 {
            return;
        }
        let bounds = item.bounds();
        let collector = self.players.values().find(|p| {
            let b = p.bounds();
            p.game_mode() != ferrocraft_protocol::packets::play::GameMode::Spectator
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
        });
        let Some(collector) = collector.map(|p| p.entity_id) else {
            return;
        };
        let stack = item.stack.clone();
        let max = self.max_stack(&stack);
        let mut leftover = None;
        let mut changed = Vec::new();
        self.update_inventory(collector, |inventory| {
            (leftover, changed) = inventory.add(stack.clone(), max);
        });
        let taken = stack.count - leftover.as_ref().map_or(0, |s| s.count);
        if taken == 0 {
            return;
        }
        self.send_slots(collector, &changed);
        let take = Encoded::new(&TakeItemEntity {
            item_id: entity_id,
            player_id: collector,
            amount: taken,
        });
        self.broadcast_item(entity_id, vec![take]);
        match leftover {
            None => self.remove_item(entity_id),
            Some(rest) => {
                if let Some(item) = self.items.get_mut(&entity_id) {
                    item.stack = rest;
                    item.stack_dirty = true;
                }
            }
        }
    }

    fn broadcast_item(&mut self, entity_id: i32, packets: Vec<Encoded>) {
        let item_type = self.item_type;
        let Some(item) = self.items.get_mut(&entity_id) else {
            return;
        };
        let message = Outbound::Entity {
            state: item.state(item_type),
            packets: packets.into(),
        };
        self.broadcast(None, &message);
    }

    /// `ServerEntity.sendChanges` for an item: position, velocity, stack.
    fn sync_item(&mut self, entity_id: i32) {
        let Some(item) = self.items.get_mut(&entity_id) else {
            return;
        };
        let force = std::mem::take(&mut item.needs_sync);
        let dirty = std::mem::take(&mut item.stack_dirty);
        let pose = item.pose();
        let mut packets = item.tracker.sync(entity_id, &pose, force || dirty);
        let velocity_changed: f64 = item
            .velocity
            .iter()
            .zip(item.sent_velocity)
            .map(|(a, b)| (a - b) * (a - b))
            .sum();
        if !packets.is_empty() && velocity_changed > 1.0e-7 {
            item.sent_velocity = item.velocity;
            packets.push(Encoded::new(&SetEntityMotion {
                entity_id,
                velocity: ferrocraft_protocol::packets::play::LpVec3(item.velocity),
            }));
        }
        if dirty {
            packets.push(Encoded::new(&SetEntityData {
                entity_id,
                values: EntityDataValues(vec![EntityDataValue {
                    index: data_index::ITEM,
                    value: EntityData::Item(Slot(Some(item.stack.clone()))),
                }]),
            }));
        }
        if !packets.is_empty() {
            item.state = None;
            self.broadcast_item(entity_id, packets);
        }
    }
}
