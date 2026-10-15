//! Falling blocks (`FallingBlock`, `FallingBlockEntity`): sand, gravel,
//! concrete powder and anvils with nothing below turn into an entity that
//! falls and becomes the block again where it lands, or drops as an item
//! where it cannot. Concrete powder hardens in water.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{ItemStack, LpVec3, SetEntityMotion};
use uuid::Uuid;

use super::blocks::{Kind, below};
use super::fluid::Fluid;
use super::tracker::{Pose, Tracker};
use super::{Encoded, EntityState, Game, Outbound, physics};
use crate::data::{Aabb, flags};
use crate::world::{MAX_Y, MIN_Y};

const GRAVITY: f64 = 0.04;
const DRAG: f64 = 0.98;
const HALF: f64 = 0.49;
const HEIGHT: f64 = 0.98;
/// Seen up to 10 chunks away, updated every 20 ticks.
const RANGE: f64 = 10.0 * 16.0;
const UPDATE_INTERVAL: u32 = 20;

pub struct FallingBlock {
    entity_id: i32,
    uuid: Uuid,
    /// The block it is.
    block: u32,
    pos: [f64; 3],
    velocity: [f64; 3],
    on_ground: bool,
    time: u32,
    tracker: Tracker,
    sent_velocity: [f64; 3],
    entity_state: Option<Arc<EntityState>>,
}

impl FallingBlock {
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

    fn block_pos(&self) -> BlockPos {
        BlockPos {
            x: self.pos[0].floor() as i32,
            y: self.pos[1].floor() as i32,
            z: self.pos[2].floor() as i32,
        }
    }

    fn state(&mut self, entity_type: i32) -> Arc<EntityState> {
        if let Some(state) = &self.entity_state {
            return Arc::clone(state);
        }
        let state = Arc::new(EntityState {
            entity_id: self.entity_id,
            uuid: self.uuid,
            entity_type,
            pos: self.tracker.sent_pos,
            yaw: 0,
            pitch: 0,
            head_yaw: 0,
            velocity: self.sent_velocity,
            // The client draws the block from the extra int.
            object_data: self.block as i32,
            data: Vec::new(),
            equipment: Vec::new(),
            spectator: false,
            range: RANGE,
        });
        self.entity_state = Some(Arc::clone(&state));
        state
    }
}

impl Game {
    fn falling_type(&self) -> i32 {
        self.context
            .data
            .entity_type("minecraft:falling_block")
            .unwrap_or(0)
    }

    /// `FallingBlock.isFree`: what a falling block falls through.
    pub(super) fn is_free(&self, state: u32) -> bool {
        self.is_air(state)
            || self.context.data.state_in_tag(state, "minecraft:fire")
            || self.kind(state) == Kind::Liquid
            || self.flag(state, flags::REPLACEABLE)
    }

    /// `FallingBlock.tick`: with room below, the block starts falling.
    pub(super) fn falling_tick(&mut self, pos: BlockPos, state: u32) {
        let below_state = self.context.world.block(below(pos));
        if !self.is_free(below_state) || pos.y < MIN_Y {
            return;
        }
        // `FallingBlockEntity.fall`: water it held stays behind.
        let left = self.fluid_state(state).legacy_block(self);
        let block = if self.value(state, "waterlogged").is_some() {
            self.with(state, "waterlogged", "false")
        } else {
            state
        };
        self.set_block(pos, left);
        let at = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        let entity_id = self.context.entity_ids.fetch_add(1, Ordering::Relaxed);
        let pose = Pose {
            pos: at,
            yaw: 0.0,
            pitch: 0.0,
            head_yaw: 0.0,
            on_ground: false,
        };
        let mut falling = FallingBlock {
            entity_id,
            uuid: Uuid::new_v4(),
            block,
            pos: at,
            velocity: [0.0; 3],
            on_ground: false,
            time: 0,
            tracker: Tracker::new(&pose, UPDATE_INTERVAL),
            sent_velocity: [0.0; 3],
            entity_state: None,
        };
        let entity_state = falling.state(self.falling_type());
        self.falling.insert(entity_id, falling);
        self.broadcast(None, &Outbound::EntityAdded(entity_state));
    }

    /// Falling blocks, for a player who just joined.
    pub(super) fn falling_states(&mut self) -> Vec<Arc<EntityState>> {
        let entity_type = self.falling_type();
        self.falling
            .values_mut()
            .map(|f| f.state(entity_type))
            .collect()
    }

    pub(super) fn tick_falling(&mut self) {
        let ids: Vec<i32> = self.falling.keys().copied().collect();
        for entity_id in ids {
            self.tick_one_falling(entity_id);
        }
    }

    /// `FallingBlockEntity.tick`.
    fn tick_one_falling(&mut self, entity_id: i32) {
        let data = Arc::clone(&self.context.data);
        let world = Arc::clone(&self.context.world);
        let Some(falling) = self.falling.get_mut(&entity_id) else {
            return;
        };
        falling.time += 1;
        falling.velocity[1] -= GRAVITY;
        let moved = physics::collide(&world, &data, falling.bounds(), falling.velocity);
        for (axis, distance) in moved.movement.iter().enumerate() {
            falling.pos[axis] += distance;
            if *distance != falling.velocity[axis] {
                falling.velocity[axis] = 0.0;
            }
        }
        falling.on_ground = moved.on_ground;
        let (block, pos, time, on_ground) = (
            falling.block,
            falling.block_pos(),
            falling.time,
            falling.on_ground,
        );
        let concrete = self.kind(block) == Kind::ConcretePowder;
        let in_water = concrete && self.fluid_state(world.block(pos)).fluid == Some(Fluid::Water);
        if !on_ground && !in_water {
            if time > 100 && (pos.y <= MIN_Y || pos.y > MAX_Y) || time > 600 {
                self.remove_falling(entity_id);
                self.drop_falling(block, entity_id, pos);
            }
        } else {
            let current = world.block(pos);
            let may_replace = self.flag(current, flags::REPLACEABLE) || self.is_air(current);
            let keeps_falling = self.is_free(world.block(below(pos))) && (!concrete || !in_water);
            let survives = self.can_survive(pos, block) && !keeps_falling;
            self.remove_falling(entity_id);
            if may_replace && survives {
                let mut landed = block;
                if self.value(block, "waterlogged").is_some()
                    && self.fluid_state(current).fluid == Some(Fluid::Water)
                    && self.fluid_state(current).source
                {
                    landed = self.with(block, "waterlogged", "true");
                }
                if self.set_block(pos, landed) {
                    self.on_land(pos, landed, current);
                } else {
                    self.drop_falling(block, entity_id, pos);
                }
            } else {
                self.drop_falling(block, entity_id, pos);
            }
            return;
        }
        let Some(falling) = self.falling.get_mut(&entity_id) else {
            return;
        };
        falling.velocity = falling.velocity.map(|v| v * DRAG);
        let pose = falling.pose();
        let mut packets = falling.tracker.sync(entity_id, &pose, false);
        if !packets.is_empty() {
            falling.sent_velocity = falling.velocity;
            packets.push(Encoded::new(&SetEntityMotion {
                entity_id,
                velocity: LpVec3(falling.velocity),
            }));
            falling.entity_state = None;
            let entity_type = self.falling_type();
            let Some(falling) = self.falling.get_mut(&entity_id) else {
                return;
            };
            let message = Outbound::Entity {
                state: falling.state(entity_type),
                packets: packets.into(),
            };
            self.broadcast(None, &message);
        }
    }

    fn remove_falling(&mut self, entity_id: i32) {
        if self.falling.remove(&entity_id).is_some() {
            self.broadcast(None, &Outbound::EntityRemoved(entity_id));
        }
    }

    /// `spawnAtLocation`: the block's item where it ended up.
    fn drop_falling(&mut self, block: u32, _entity_id: i32, pos: BlockPos) {
        let data = Arc::clone(&self.context.data);
        let Some(item) = data
            .block_of(block)
            .and_then(|(_, b)| data.item_id(&b.item))
            .filter(|&i| i != 0)
        else {
            return;
        };
        let at = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        let velocity = [
            rand::random::<f64>() * 0.2 - 0.1,
            0.2,
            rand::random::<f64>() * 0.2 - 0.1,
        ];
        let stack = ItemStack {
            item,
            count: 1,
            patch: Default::default(),
        };
        self.spawn_item(at, velocity, stack, 10);
    }

    /// `Fallable.onLand`: concrete powder in water hardens, anvils clang.
    fn on_land(&mut self, pos: BlockPos, block: u32, replaced: u32) {
        match self.kind(block) {
            Kind::ConcretePowder => {
                if self.fluid_state(replaced).fluid == Some(Fluid::Water) || self.touches_water(pos)
                {
                    let concrete = self.concrete_of(block);
                    self.set_block(pos, concrete);
                }
            }
            _ => {
                let name = self
                    .context
                    .data
                    .block_of(block)
                    .map(|(_, b)| b.name.clone())
                    .unwrap_or_default();
                if name.ends_with("anvil") {
                    self.level_event(1031, pos, 0);
                }
            }
        }
    }

    /// The concrete a concrete powder hardens to.
    pub(super) fn concrete_of(&self, powder: u32) -> u32 {
        let data = &self.context.data;
        data.block_of(powder)
            .and_then(|(_, b)| b.name.strip_suffix("_powder"))
            .and_then(|name| data.default_state(name).ok())
            .unwrap_or(powder)
    }
}
