//! Block updates the way vanilla's `Level.setBlock` makes them: a changed
//! block tells its neighbours, which may change their shape (a redstone
//! wire connects, a flower without ground pops off) or react (water flows
//! on, a lamp lights up). On top come scheduled ticks (`LevelTicks`), for
//! blocks that act a little later, and random ticks, which make plants
//! grow.
//!
//! The behaviour of single blocks lives in `fluid`, `falling`, `growth`
//! and `redstone`; this module dispatches to them by vanilla's class of
//! the block ([`Kind`]).

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{BlockEvent, Direction, LevelEvent};
use rand::Rng;

use super::fluid::Fluid;
use super::{Encoded, Game, Outbound};
use crate::data::{Block, GameData, flags};
use crate::loot;
use crate::placement::opposite;
use crate::world::chunk_of;

/// `Block.UPDATE_*`: what a block change sets off.
pub mod update {
    /// Neighbours hear of it (`neighborChanged`).
    pub const NEIGHBORS: u32 = 1;
    /// Clients see it. Without it they are not told, as of the blocks a
    /// piston moves, which clients move themselves.
    pub const CLIENTS: u32 = 2;
    /// Clients do not redraw it (client side only).
    pub const INVISIBLE: u32 = 4;
    /// The neighbours' shapes are known not to change (`updateShape` is
    /// skipped).
    pub const KNOWN_SHAPE: u32 = 16;
    /// Blocks a shape update removes drop nothing.
    pub const SUPPRESS_DROPS: u32 = 32;
    /// A piston moves it: the old block's neighbours hear of its removal
    /// even without [`NEIGHBORS`].
    pub const MOVE_BY_PISTON: u32 = 64;
    /// The old block entity goes without its side effects.
    pub const SKIP_BLOCK_ENTITY_SIDEEFFECTS: u32 = 256;
    pub const ALL: u32 = NEIGHBORS | CLIENTS;
}

/// How deep shape updates may chain (`Block.UPDATE_LIMIT`).
const UPDATE_LIMIT: u32 = 512;
/// `maxChainedNeighborUpdates` of `server.properties`.
const MAX_CHAINED_NEIGHBOR_UPDATES: usize = 1_000_000;
/// `LevelTicks`: at most this many scheduled ticks run per game tick.
const MAX_TICKS_PER_TICK: usize = 65536;
/// Chunks this far from a player tick (`simulation-distance`).
pub const SIMULATION_DISTANCE: i32 = 10;

/// `Direction.values()`.
pub const DIRECTIONS: [Direction; 6] = [
    Direction::Down,
    Direction::Up,
    Direction::North,
    Direction::South,
    Direction::West,
    Direction::East,
];
/// `Direction.Plane.HORIZONTAL`.
pub const HORIZONTAL: [Direction; 4] = [
    Direction::North,
    Direction::East,
    Direction::South,
    Direction::West,
];
/// `NeighborUpdater.UPDATE_ORDER`.
const NEIGHBOR_ORDER: [Direction; 6] = [
    Direction::West,
    Direction::East,
    Direction::Down,
    Direction::Up,
    Direction::North,
    Direction::South,
];
/// `BlockBehaviour.UPDATE_SHAPE_ORDER`.
const SHAPE_ORDER: [Direction; 6] = [
    Direction::West,
    Direction::East,
    Direction::North,
    Direction::South,
    Direction::Down,
    Direction::Up,
];

pub fn offset(pos: BlockPos, dx: i32, dy: i32, dz: i32) -> BlockPos {
    BlockPos {
        x: pos.x + dx,
        y: pos.y + dy,
        z: pos.z + dz,
    }
}

pub fn above(pos: BlockPos) -> BlockPos {
    offset(pos, 0, 1, 0)
}

pub fn below(pos: BlockPos) -> BlockPos {
    offset(pos, 0, -1, 0)
}

pub fn is_horizontal(direction: Direction) -> bool {
    !matches!(direction, Direction::Up | Direction::Down)
}

/// The centre of a block.
pub fn center(pos: BlockPos) -> [f64; 3] {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ]
}

/// What a block does when things change around it, by vanilla's class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    None,
    /// `LiquidBlock`: water and lava.
    Liquid,
    /// `FallingBlock`: sand, gravel, anvils, ...
    Falling,
    ConcretePowder,
    /// `CropBlock`: wheat, carrots, potatoes, beetroots.
    Crop,
    /// `StemBlock`: pumpkin and melon stems.
    Stem,
    /// `AttachedStemBlock`.
    AttachedStem,
    /// Flowers, grass, ferns, bushes: they need ground.
    Plant,
    /// Dead bushes and dry grass, on sand and terracotta too.
    DryPlant,
    Sapling,
    Farmland,
    /// `SpreadingSnowyBlock`: grass blocks and mycelium.
    Spreading,
    /// `SnowyBlock`: podzol and the like, white under snow.
    Snowy,
    Leaves,
    SugarCane,
    Cactus,
    Torch,
    WallTorch,
    RedstoneTorch,
    RedstoneWallTorch,
    Wire,
    Repeater,
    Lever,
    Button,
    PressurePlate,
    WeightedPressurePlate,
    Lamp,
    Door,
    Trapdoor,
    FenceGate,
    Tnt,
    /// `PoweredBlock`: the redstone block.
    RedstoneBlock,
    /// `PistonBaseBlock`: pistons and sticky pistons.
    Piston,
    PistonHead,
    /// `MovingPistonBlock`: a block on its way.
    MovingPiston,
    Observer,
    Comparator,
    Hopper,
    Dispenser,
    Dropper,
    TrappedChest,
    /// `BaseFireBlock`: fire and soul fire.
    Fire,
}

impl Kind {
    fn of(block: &Block) -> Kind {
        let order: &[(&str, Kind)] = &[
            ("LiquidBlock", Kind::Liquid),
            ("ConcretePowderBlock", Kind::ConcretePowder),
            ("FallingBlock", Kind::Falling),
            ("TorchflowerCropBlock", Kind::None),
            ("CropBlock", Kind::Crop),
            ("AttachedStemBlock", Kind::AttachedStem),
            ("StemBlock", Kind::Stem),
            ("SaplingBlock", Kind::Sapling),
            ("DryVegetationBlock", Kind::DryPlant),
            ("FlowerBlock", Kind::Plant),
            ("TallGrassBlock", Kind::Plant),
            ("BushBlock", Kind::Plant),
            ("FarmlandBlock", Kind::Farmland),
            ("SpreadingSnowyBlock", Kind::Spreading),
            ("SnowyBlock", Kind::Snowy),
            ("LeavesBlock", Kind::Leaves),
            ("SugarCaneBlock", Kind::SugarCane),
            ("CactusBlock", Kind::Cactus),
            ("RedstoneWallTorchBlock", Kind::RedstoneWallTorch),
            ("RedstoneTorchBlock", Kind::RedstoneTorch),
            ("WallTorchBlock", Kind::WallTorch),
            ("TorchBlock", Kind::Torch),
            ("RedstoneWireBlock", Kind::Wire),
            ("RepeaterBlock", Kind::Repeater),
            ("LeverBlock", Kind::Lever),
            ("ButtonBlock", Kind::Button),
            ("WeightedPressurePlateBlock", Kind::WeightedPressurePlate),
            ("PressurePlateBlock", Kind::PressurePlate),
            ("RedstoneLampBlock", Kind::Lamp),
            ("DoorBlock", Kind::Door),
            ("TrapDoorBlock", Kind::Trapdoor),
            ("FenceGateBlock", Kind::FenceGate),
            ("TntBlock", Kind::Tnt),
            ("PoweredBlock", Kind::RedstoneBlock),
            ("PistonBaseBlock", Kind::Piston),
            ("PistonHeadBlock", Kind::PistonHead),
            ("MovingPistonBlock", Kind::MovingPiston),
            ("ObserverBlock", Kind::Observer),
            ("ComparatorBlock", Kind::Comparator),
            ("HopperBlock", Kind::Hopper),
            ("DropperBlock", Kind::Dropper),
            ("DispenserBlock", Kind::Dispenser),
            ("TrappedChestBlock", Kind::TrappedChest),
            ("BaseFireBlock", Kind::Fire),
        ];
        // The most specific class comes first in `classes`.
        for class in &block.classes {
            if let Some((_, kind)) = order.iter().find(|(name, _)| name == class) {
                return *kind;
            }
        }
        Kind::None
    }

    /// One per block id.
    pub fn all(data: &GameData) -> Vec<Kind> {
        data.blocks().iter().map(Kind::of).collect()
    }
}

/// What a scheduled tick is for: a block, or a fluid (which has its own
/// queue, run after the blocks').
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Block(usize),
    Fluid(Fluid),
}

/// `TickPriority`: lower runs first within a tick.
pub mod priority {
    pub const EXTREMELY_HIGH: i8 = -3;
    pub const VERY_HIGH: i8 = -2;
    pub const HIGH: i8 = -1;
    pub const NORMAL: i8 = 0;
}

/// Ticks waiting for their time (`LevelTicks`).
#[derive(Debug, Default)]
pub struct Ticks {
    /// By (game tick, priority, order of scheduling).
    queue: BTreeMap<(u64, i8, u64), (BlockPos, Target)>,
    pending: HashSet<(BlockPos, Target)>,
    /// What runs in the current game tick.
    running: HashSet<(BlockPos, Target)>,
    order: u64,
}

impl Ticks {
    /// `scheduleTick`: nothing happens if the same tick is already waiting.
    pub fn schedule(&mut self, now: u64, pos: BlockPos, target: Target, delay: u64, priority: i8) {
        if self.pending.insert((pos, target)) {
            self.order += 1;
            self.queue
                .insert((now + delay, priority, self.order), (pos, target));
        }
    }

    /// `willTickThisTick`.
    pub fn runs_now(&self, pos: BlockPos, target: Target) -> bool {
        self.running.contains(&(pos, target))
    }

    /// `hasScheduledTick`.
    pub fn is_scheduled(&self, pos: BlockPos, target: Target) -> bool {
        self.pending.contains(&(pos, target))
    }

    fn take_due(&mut self, now: u64) -> Vec<(BlockPos, Target)> {
        let mut due = Vec::new();
        while due.len() < MAX_TICKS_PER_TICK {
            let Some(entry) = self.queue.first_entry() else {
                break;
            };
            if entry.key().0 > now {
                break;
            }
            let (pos, target) = entry.remove();
            self.pending.remove(&(pos, target));
            due.push((pos, target));
        }
        self.running = due.iter().copied().collect();
        due
    }

    /// Ticks still waiting in a chunk, with how long they have to go and
    /// their priority, for saving.
    pub fn in_chunk(&self, now: u64, chunk: (i32, i32)) -> Vec<(BlockPos, Target, i64, i8)> {
        self.queue
            .iter()
            .filter(|(_, (pos, _))| chunk_of(*pos) == chunk)
            .map(|(&(at, priority, _), &(pos, target))| {
                (pos, target, at as i64 - now as i64, priority)
            })
            .collect()
    }

    /// Every tick still waiting: position and target.
    pub fn chunks(&self) -> HashSet<(i32, i32)> {
        self.queue.values().map(|(pos, _)| chunk_of(*pos)).collect()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }
}

/// A block event waiting for the end of the tick (`BlockEventData`), such
/// as a piston about to move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockEventData {
    pos: BlockPos,
    block: usize,
    b0: u8,
    b1: u8,
}

/// A pending `neighborChanged`: one block, or the six around a position.
#[derive(Debug, Clone, Copy)]
enum NeighborUpdate {
    One {
        pos: BlockPos,
        source: usize,
    },
    Around {
        center: BlockPos,
        source: usize,
        next: usize,
        skip: Option<Direction>,
    },
}

/// `CollectingNeighborUpdater`: updates caused by updates wait until the
/// current one is done, depth first, so long chains do not overflow the
/// stack.
#[derive(Debug, Default)]
pub struct Updater {
    stack: Vec<NeighborUpdate>,
    added: Vec<NeighborUpdate>,
    count: usize,
}

impl Game {
    pub(super) fn kind(&self, state: u32) -> Kind {
        self.context
            .data
            .block_of(state)
            .map_or(Kind::None, |(id, _)| self.kinds[id])
    }

    pub(super) fn block_id(&self, state: u32) -> usize {
        self.context.data.block_of(state).map_or(0, |(id, _)| id)
    }

    /// The block and its name, for checks like `state.is(Blocks.X)`.
    pub(super) fn is(&self, state: u32, name: &str) -> bool {
        self.context.data.state_is(state, name)
    }

    pub(super) fn flag(&self, state: u32, flag: u32) -> bool {
        self.context.data.physics(state).has(flag)
    }

    /// `isFaceSturdy` of the state at `pos` for the face towards
    /// `direction`.
    pub(super) fn sturdy(&self, pos: BlockPos, direction: Direction) -> bool {
        let state = self.context.world.block(pos);
        self.context.data.physics(state).sturdy(direction as usize)
    }

    pub(super) fn conductor(&self, pos: BlockPos) -> bool {
        self.flag(self.context.world.block(pos), flags::CONDUCTOR)
    }

    pub(super) fn is_air(&self, state: u32) -> bool {
        self.context.data.state_in_tag(state, "minecraft:air")
    }

    /// A property of a state, if its block has it.
    pub(super) fn value(&self, state: u32, key: &str) -> Option<&str> {
        self.context.data.block_of(state)?.1.value(state, key)
    }

    /// A state with one property changed; the state itself if its block
    /// has no such property.
    pub(super) fn with(&self, state: u32, key: &str, value: &str) -> u32 {
        self.context
            .data
            .block_of(state)
            .and_then(|(_, b)| b.with(state, key, value))
            .unwrap_or(state)
    }

    pub(super) fn int_value(&self, state: u32, key: &str) -> Option<i32> {
        self.value(state, key)?.parse().ok()
    }

    pub(super) fn bool_value(&self, state: u32, key: &str) -> bool {
        self.value(state, key) == Some("true")
    }

    pub(super) fn direction_value(&self, state: u32, key: &str) -> Option<Direction> {
        crate::placement::direction(self.value(state, key)?)
    }

    /// The default state of a block.
    pub(super) fn default_of(&self, name: &str) -> u32 {
        self.context.data.default_state(name).unwrap_or(self.air)
    }

    // --- Setting blocks ----------------------------------------------------

    /// Sets a block with neighbour and shape updates, like
    /// `setBlockAndUpdate`.
    pub(super) fn set_block(&mut self, pos: BlockPos, state: u32) -> bool {
        self.set_block_with(pos, state, update::ALL)
    }

    pub(super) fn set_block_with(&mut self, pos: BlockPos, state: u32, flags: u32) -> bool {
        self.set_block_limited(pos, state, flags, UPDATE_LIMIT)
    }

    fn set_block_limited(&mut self, pos: BlockPos, state: u32, flags: u32, limit: u32) -> bool {
        let old = self.context.world.block(pos);
        if !self.context.world.set_block(pos, state) {
            return false;
        }
        if flags & update::CLIENTS != 0 {
            self.broadcast(None, &Outbound::BlockChanged { pos, state });
        }
        let moved = flags & update::MOVE_BY_PISTON != 0;
        let (old_id, new_id) = (self.block_id(old), self.block_id(state));
        if old_id != new_id && (flags & update::NEIGHBORS != 0 || moved) {
            self.after_removal_moved(pos, old, moved);
        }
        self.on_place(pos, state, old, moved);
        if self.context.world.block(pos) != state {
            // `onPlace` replaced it, as burning TNT does.
            return true;
        }
        if flags & update::NEIGHBORS != 0 {
            self.update_neighbors_at(pos, old_id);
            if self.flag(state, flags::ANALOG) {
                self.update_neighbour_for_output_signal(pos, new_id);
            }
        }
        if flags & update::KNOWN_SHAPE == 0 && limit > 0 {
            let flags = flags & !(update::NEIGHBORS | update::SUPPRESS_DROPS);
            self.update_indirect_shapes(pos, old, flags, limit - 1);
            self.update_neighbour_shapes(pos, state, flags, limit - 1);
            self.update_indirect_shapes(pos, state, flags, limit - 1);
        }
        true
    }

    /// `Level.destroyBlock`: the block goes with the particles and sound of
    /// breaking, and drops its loot if `drop`; water in it stays.
    pub(super) fn destroy_block(&mut self, pos: BlockPos, drop: bool) -> bool {
        self.destroy_block_limited(pos, drop, UPDATE_LIMIT)
    }

    fn destroy_block_limited(&mut self, pos: BlockPos, drop: bool, limit: u32) -> bool {
        let state = self.context.world.block(pos);
        if self.is_air(state) {
            return false;
        }
        if !self.context.data.state_in_tag(state, "minecraft:fire") {
            self.break_effect(None, pos, state);
        }
        if drop {
            self.drop_resources(pos, state);
        }
        let left = self.fluid_state(state).legacy_block(self);
        self.set_block_limited(pos, left, update::ALL, limit)
    }

    /// `Block.dropResources` without a tool: the block's loot pops out.
    pub(super) fn drop_resources(&mut self, pos: BlockPos, state: u32) {
        let data = Arc::clone(&self.context.data);
        let Some((_, block)) = data.block_of(state) else {
            return;
        };
        let world = Arc::clone(&self.context.world);
        let context = loot::Context {
            data: &data,
            table: loot::block_table(block),
            state,
            pos,
            tool: None,
            block_at: &|p| world.block(p),
            entity: None,
            by_player: false,
            explosion: None,
        };
        for stack in context.drops(&mut rand::rng()) {
            self.pop_resource(pos, stack);
        }
    }

    /// A level event at a block, such as 1501 (lava fizzing) or 1505 (bone
    /// meal), for the players that have the chunk.
    pub(super) fn level_event(&self, event: i32, pos: BlockPos, data: i32) {
        let packet = Encoded::new(&LevelEvent {
            event,
            pos,
            data,
            global: false,
        });
        self.broadcast(
            None,
            &Outbound::InChunk {
                chunk: chunk_of(pos),
                packet: Arc::new(packet),
            },
        );
    }

    // --- Shape updates -----------------------------------------------------

    /// `BlockState.updateNeighbourShapes`.
    fn update_neighbour_shapes(&mut self, pos: BlockPos, state: u32, flags: u32, limit: u32) {
        for direction in SHAPE_ORDER {
            let neighbour = direction.offset(pos);
            self.neighbour_shape_changed(opposite(direction), neighbour, pos, state, flags, limit);
        }
    }

    /// `LevelAccessor.neighborShapeChanged`: the block at `pos` hears that
    /// the one towards `direction` is now `source_state`.
    fn neighbour_shape_changed(
        &mut self,
        direction: Direction,
        pos: BlockPos,
        source: BlockPos,
        source_state: u32,
        flags: u32,
        limit: u32,
    ) {
        let state = self.context.world.block(pos);
        let new = self.update_shape(pos, state, direction, source, source_state);
        self.update_or_destroy(pos, state, new, flags, limit);
    }

    /// `BlockState.updateNeighbourShapes` with the full depth, for blocks
    /// set without shape updates.
    pub(super) fn update_neighbour_shapes_of(&mut self, pos: BlockPos, state: u32, flags: u32) {
        self.update_neighbour_shapes(pos, state, flags, UPDATE_LIMIT);
    }

    /// `BlockState.updateIndirectNeighbourShapes` with the full depth.
    pub(super) fn update_indirect_shapes_of(&mut self, pos: BlockPos, state: u32, flags: u32) {
        self.update_indirect_shapes(pos, state, flags, UPDATE_LIMIT);
    }

    /// `Block.updateOrDestroy` with the full depth.
    pub(super) fn update_or_destroy_at(&mut self, pos: BlockPos, old: u32, new: u32, flags: u32) {
        self.update_or_destroy(pos, old, new, flags, UPDATE_LIMIT);
    }

    /// `Block.updateOrDestroy`.
    fn update_or_destroy(&mut self, pos: BlockPos, old: u32, new: u32, flags: u32, limit: u32) {
        if new == old {
            return;
        }
        if self.is_air(new) {
            self.destroy_block_limited(pos, flags & update::SUPPRESS_DROPS == 0, limit);
        } else {
            self.set_block_limited(pos, new, flags & !update::SUPPRESS_DROPS, limit);
        }
    }

    /// `Block.updateFromNeighbourShapes`: the state as its neighbours would
    /// shape it.
    pub(super) fn shaped_by_neighbours(&mut self, pos: BlockPos, state: u32) -> u32 {
        let mut state = state;
        for direction in SHAPE_ORDER {
            let neighbour = direction.offset(pos);
            let neighbour_state = self.context.world.block(neighbour);
            state = self.update_shape(pos, state, direction, neighbour, neighbour_state);
        }
        state
    }

    /// `ServerLevel.updateNeighboursOnBlockSet`, after a command set a
    /// block without updates.
    pub(super) fn updates_after_set(&mut self, pos: BlockPos, old: u32) {
        let state = self.context.world.block(pos);
        if self.block_id(old) != self.block_id(state) {
            self.after_removal(pos, old);
        }
        let id = self.block_id(state);
        self.update_neighbors_at(pos, id);
    }

    /// `updateIndirectNeighbourShapes`: only wires look at diagonal
    /// neighbours.
    fn update_indirect_shapes(&mut self, pos: BlockPos, state: u32, flags: u32, limit: u32) {
        if self.kind(state) == Kind::Wire {
            self.wire_indirect_shapes(pos, state, flags, limit);
        }
    }

    /// `BlockBehaviour.updateShape`: the state at `pos` after the block
    /// towards `direction` became `neighbour_state`.
    fn update_shape(
        &mut self,
        pos: BlockPos,
        state: u32,
        direction: Direction,
        neighbour: BlockPos,
        neighbour_state: u32,
    ) -> u32 {
        // Waterlogged blocks let their water flow on.
        if self.bool_value(state, "waterlogged") {
            self.schedule_fluid_tick(pos, Fluid::Water);
        }
        match self.kind(state) {
            Kind::Liquid => {
                if (self.fluid_state(state).source || self.fluid_state(neighbour_state).source)
                    && let Some(fluid) = self.fluid_state(state).fluid
                {
                    self.schedule_fluid_tick(pos, fluid);
                }
                state
            }
            Kind::Falling => {
                self.schedule_block_tick(pos, state, 2, priority::NORMAL);
                state
            }
            Kind::ConcretePowder => {
                if self.touches_water(pos) {
                    return self.concrete_of(state);
                }
                self.schedule_block_tick(pos, state, 2, priority::NORMAL);
                state
            }
            Kind::Crop
            | Kind::Stem
            | Kind::AttachedStem
            | Kind::Plant
            | Kind::DryPlant
            | Kind::Sapling => {
                if self.can_survive(pos, state) {
                    if self.kind(state) == Kind::AttachedStem {
                        return self.attached_stem_shape(state, direction, neighbour_state);
                    }
                    state
                } else {
                    self.air
                }
            }
            Kind::Farmland => {
                if direction == Direction::Up && !self.can_survive(pos, state) {
                    self.schedule_block_tick(pos, state, 1, priority::NORMAL);
                }
                state
            }
            Kind::Spreading | Kind::Snowy => {
                if direction == Direction::Up {
                    let snowy = self
                        .context
                        .data
                        .state_in_tag(neighbour_state, "minecraft:snow");
                    return self.with(state, "snowy", if snowy { "true" } else { "false" });
                }
                state
            }
            Kind::Leaves => {
                let distance = self.leaf_distance_of(neighbour_state) + 1;
                if distance != 1 || self.int_value(state, "distance") != Some(distance) {
                    self.schedule_block_tick(pos, state, 1, priority::NORMAL);
                }
                state
            }
            Kind::SugarCane | Kind::Cactus => {
                if !self.can_survive(pos, state) {
                    self.schedule_block_tick(pos, state, 1, priority::NORMAL);
                }
                state
            }
            Kind::Torch | Kind::RedstoneTorch => {
                if direction == Direction::Down && !self.can_survive(pos, state) {
                    self.air
                } else {
                    state
                }
            }
            Kind::WallTorch | Kind::RedstoneWallTorch => {
                if self.direction_value(state, "facing") == Some(opposite(direction))
                    && !self.can_survive(pos, state)
                {
                    self.air
                } else {
                    state
                }
            }
            Kind::Lever | Kind::Button => {
                if self.attached_towards(state) == Some(direction) && !self.can_survive(pos, state)
                {
                    self.air
                } else {
                    state
                }
            }
            Kind::PressurePlate | Kind::WeightedPressurePlate => {
                if direction == Direction::Down && !self.can_survive(pos, state) {
                    self.air
                } else {
                    state
                }
            }
            Kind::Wire => self.wire_shape(pos, state, direction, neighbour, neighbour_state),
            Kind::Repeater | Kind::Comparator => {
                self.diode_shape(pos, state, direction, neighbour_state)
            }
            Kind::PistonHead => {
                if self.direction_value(state, "facing") == Some(opposite(direction))
                    && !self.head_survives(pos, state)
                {
                    self.air
                } else {
                    state
                }
            }
            Kind::Observer => {
                self.observer_shape(pos, state, direction);
                state
            }
            Kind::Fire => self.fire_shape(pos, state),
            _ => state,
        }
    }

    /// `canSurvive` of the blocks Ferrocraft knows to need support.
    pub(super) fn can_survive(&self, pos: BlockPos, state: u32) -> bool {
        let data = &self.context.data;
        let below_pos = below(pos);
        let below_state = self.context.world.block(below_pos);
        let on = |tag: &str| data.state_in_tag(below_state, tag);
        match self.kind(state) {
            Kind::Crop => on("minecraft:supports_crops") && self.raw_brightness(pos) >= 8,
            Kind::Stem | Kind::AttachedStem => {
                // `StemBlock.stemSupportBlocks`: one tag per fruit.
                let fruit = if self
                    .context
                    .data
                    .block_of(state)
                    .is_some_and(|(_, b)| b.name.contains("pumpkin"))
                {
                    "pumpkin"
                } else {
                    "melon"
                };
                on(&format!("minecraft:supports_{fruit}_stem"))
            }
            Kind::Plant | Kind::Sapling => on("minecraft:supports_vegetation"),
            Kind::DryPlant => on("minecraft:supports_dry_vegetation"),
            Kind::Farmland => {
                let above = self.context.world.block(above(pos));
                !self.flag(above, flags::SOLID)
                    || data.state_in_tag(above, "minecraft:maintains_farmland")
            }
            Kind::SugarCane => self.sugar_cane_survives(pos),
            Kind::Cactus => self.cactus_survives(pos),
            Kind::Torch | Kind::RedstoneTorch => data.physics(below_state).has(flags::CENTER_TOP),
            Kind::WallTorch | Kind::RedstoneWallTorch => {
                self.direction_value(state, "facing").is_some_and(|facing| {
                    let support = opposite(facing).offset(pos);
                    self.sturdy(support, facing)
                })
            }
            Kind::Lever | Kind::Button => self.attached_towards(state).is_some_and(|towards| {
                let support = towards.offset(pos);
                self.sturdy(support, opposite(towards))
            }),
            Kind::PressurePlate | Kind::WeightedPressurePlate => {
                data.physics(below_state).sturdy(Direction::Up as usize)
                    || data.physics(below_state).has(flags::CENTER_TOP)
            }
            Kind::Wire => self.wire_survives_on(below_state),
            Kind::Repeater | Kind::Comparator => {
                data.physics(below_state).sturdy(Direction::Up as usize)
            }
            Kind::PistonHead => self.head_survives(pos, state),
            Kind::Fire => self.fire_survives(pos, state),
            _ => true,
        }
    }

    /// The side a lever or button hangs on (`getConnectedDirection`,
    /// reversed).
    pub(super) fn attached_towards(&self, state: u32) -> Option<Direction> {
        Some(match self.value(state, "face")? {
            "floor" => Direction::Down,
            "ceiling" => Direction::Up,
            _ => opposite(self.direction_value(state, "facing")?),
        })
    }

    // --- Neighbour updates ---------------------------------------------------

    /// `Level.updateNeighborsAt`: the six blocks around `pos` hear that
    /// `source` (a block id) changed.
    pub(super) fn update_neighbors_at(&mut self, pos: BlockPos, source: usize) {
        self.add_update(NeighborUpdate::Around {
            center: pos,
            source,
            next: 0,
            skip: None,
        });
    }

    /// `updateNeighborsAtExceptFromFacing`.
    pub(super) fn update_neighbors_except(
        &mut self,
        pos: BlockPos,
        source: usize,
        skip: Direction,
    ) {
        let next = usize::from(NEIGHBOR_ORDER[0] == skip);
        self.add_update(NeighborUpdate::Around {
            center: pos,
            source,
            next,
            skip: Some(skip),
        });
    }

    /// `Level.neighborChanged` for one block.
    pub(super) fn neighbor_changed_at(&mut self, pos: BlockPos, source: usize) {
        self.add_update(NeighborUpdate::One { pos, source });
    }

    fn add_update(&mut self, update: NeighborUpdate) {
        let running = self.updater.count > 0;
        let too_many = self.updater.count >= MAX_CHAINED_NEIGHBOR_UPDATES;
        self.updater.count += 1;
        if !too_many {
            if running {
                self.updater.added.push(update);
            } else {
                self.updater.stack.push(update);
            }
        } else if self.updater.count - 1 == MAX_CHAINED_NEIGHBOR_UPDATES {
            tracing::error!("Too many chained neighbor updates. Skipping the rest.");
        }
        if !running {
            self.run_updates();
        }
    }

    fn run_updates(&mut self) {
        loop {
            let added = std::mem::take(&mut self.updater.added);
            self.updater.stack.extend(added.into_iter().rev());
            let Some(mut next) = self.updater.stack.pop() else {
                break;
            };
            loop {
                let more = self.run_next(&mut next);
                if !more {
                    break;
                }
                if !self.updater.added.is_empty() {
                    self.updater.stack.push(next);
                    break;
                }
            }
        }
        self.updater.count = 0;
    }

    /// Runs one update of `update`; whether more remain.
    fn run_next(&mut self, update: &mut NeighborUpdate) -> bool {
        match update {
            NeighborUpdate::One { pos, source } => {
                self.neighbor_changed(*pos, *source);
                false
            }
            NeighborUpdate::Around {
                center,
                source,
                next,
                skip,
            } => {
                let target = NEIGHBOR_ORDER[*next].offset(*center);
                *next += 1;
                if *next < NEIGHBOR_ORDER.len() && Some(NEIGHBOR_ORDER[*next]) == *skip {
                    *next += 1;
                }
                let (source, more) = (*source, *next < NEIGHBOR_ORDER.len());
                self.neighbor_changed(target, source);
                more
            }
        }
    }

    /// `BlockBehaviour.neighborChanged`.
    fn neighbor_changed(&mut self, pos: BlockPos, source: usize) {
        let state = self.context.world.block(pos);
        match self.kind(state) {
            Kind::Liquid => self.liquid_neighbor_changed(pos, state),
            Kind::Wire => self.wire_neighbor_changed(pos, state),
            Kind::RedstoneTorch | Kind::RedstoneWallTorch => {
                self.torch_neighbor_changed(pos, state)
            }
            Kind::Repeater | Kind::Comparator => self.diode_neighbor_changed(pos, state),
            Kind::Lamp => self.lamp_neighbor_changed(pos, state),
            Kind::Piston => self.piston_check(pos, state),
            Kind::PistonHead => self.head_neighbor_changed(pos, state, source),
            Kind::Hopper => self.hopper_check_powered(pos, state),
            Kind::Dispenser | Kind::Dropper => self.dispenser_neighbor_changed(pos, state),
            Kind::Door => self.door_neighbor_changed(pos, state, source),
            Kind::Trapdoor | Kind::FenceGate => self.opening_neighbor_changed(pos, state),
            Kind::Tnt if self.has_neighbor_signal(pos) && self.rules.flag("tnt_explodes") => {
                self.set_block(pos, self.air);
                self.prime_tnt(pos, None, super::tnt::DEFAULT_FUSE);
            }
            _ => {}
        }
    }

    /// `onPlace`: the block at `pos` became `state`, which was `old`.
    fn on_place(&mut self, pos: BlockPos, state: u32, old: u32, _moved: bool) {
        let same_block = self.block_id(old) == self.block_id(state);
        match self.kind(state) {
            Kind::Liquid => self.liquid_on_place(pos, state),
            Kind::Falling | Kind::ConcretePowder => {
                self.schedule_block_tick(pos, state, 2, priority::NORMAL)
            }
            Kind::Wire if !same_block => self.wire_on_place(pos, state),
            Kind::RedstoneTorch | Kind::RedstoneWallTorch => self.torch_notify(pos, state),
            Kind::Repeater | Kind::Comparator => self.diode_update_front(pos, state),
            Kind::Piston if !same_block && self.moving_at(pos).is_none() => {
                self.piston_check(pos, state)
            }
            Kind::Observer if !same_block => self.observer_on_place(pos, state),
            Kind::Hopper if !same_block => self.hopper_check_powered(pos, state),
            Kind::Fire => self.fire_on_place(pos, state, !same_block),
            Kind::Tnt
                if !same_block
                    && self.has_neighbor_signal(pos)
                    && self.rules.flag("tnt_explodes") =>
            {
                self.set_block(pos, self.air);
                self.prime_tnt(pos, None, super::tnt::DEFAULT_FUSE);
            }
            _ => {}
        }
        // A waterlogged block's water starts flowing.
        if self.bool_value(state, "waterlogged") && !self.bool_value(old, "waterlogged") {
            self.schedule_fluid_tick(pos, Fluid::Water);
        }
    }

    /// `affectNeighborsAfterRemoval`: `old` at `pos` was replaced by another
    /// block.
    pub(super) fn after_removal(&mut self, pos: BlockPos, old: u32) {
        self.after_removal_moved(pos, old, false);
    }

    /// `affectNeighborsAfterRemoval`, with whether a piston moved the block.
    pub(super) fn after_removal_moved(&mut self, pos: BlockPos, old: u32, moved: bool) {
        match self.kind(old) {
            Kind::Wire => self.wire_after_removal(pos, old),
            Kind::RedstoneTorch | Kind::RedstoneWallTorch => self.torch_notify(pos, old),
            Kind::Repeater | Kind::Comparator if !moved => self.diode_update_front(pos, old),
            Kind::PistonHead => self.head_removed(pos, old),
            Kind::Observer => self.observer_removed(pos, old),
            Kind::Hopper | Kind::Dispenser | Kind::Dropper => {
                let id = self.block_id(old);
                self.update_neighbour_for_output_signal(pos, id);
            }
            Kind::TrappedChest => {
                let id = self.block_id(old);
                self.update_neighbour_for_output_signal(pos, id);
                if self.openers.contains_key(&pos) {
                    self.update_neighbors_at(pos, id);
                    self.update_neighbors_at(below(pos), id);
                }
            }
            Kind::Lever | Kind::Button => {
                if self.bool_value(old, "powered") {
                    self.attached_update(pos, old);
                }
            }
            Kind::PressurePlate | Kind::WeightedPressurePlate if self.plate_signal(old) > 0 => {
                self.plate_update(pos, old);
            }
            _ => {}
        }
    }

    // --- Scheduled ticks ---------------------------------------------------

    pub(super) fn schedule_block_tick(
        &mut self,
        pos: BlockPos,
        state: u32,
        delay: u64,
        priority: i8,
    ) {
        let target = Target::Block(self.block_id(state));
        self.block_ticks
            .schedule(self.tick, pos, target, delay.max(1), priority);
    }

    pub(super) fn schedule_fluid_tick(&mut self, pos: BlockPos, fluid: Fluid) {
        let delay = fluid.tick_delay();
        self.schedule_fluid_tick_in(pos, fluid, delay);
    }

    pub(super) fn schedule_fluid_tick_in(&mut self, pos: BlockPos, fluid: Fluid, delay: u64) {
        self.fluid_ticks.schedule(
            self.tick,
            pos,
            Target::Fluid(fluid),
            delay.max(1),
            priority::NORMAL,
        );
    }

    /// Ticks loaded with the world's chunks join the queues.
    pub(super) fn restore_ticks(&mut self) {
        for saved in self.context.world.take_saved_ticks() {
            let delay = saved.delay.max(0) as u64;
            if saved.fluid {
                let fluid = match saved.id.as_str() {
                    "minecraft:water" | "minecraft:flowing_water" => Fluid::Water,
                    "minecraft:lava" | "minecraft:flowing_lava" => Fluid::Lava,
                    _ => continue,
                };
                self.schedule_fluid_tick_in(saved.pos, fluid, delay);
            } else if let Some(block) = self.context.data.block_named(&saved.id) {
                let state = block.default;
                let priority = saved.priority.clamp(-3, 3) as i8;
                self.schedule_block_tick(saved.pos, state, delay, priority);
            }
        }
    }

    /// The scheduled ticks of a chunk, as vanilla saves them.
    pub(super) fn saved_ticks(&self, chunk: (i32, i32)) -> Vec<crate::world::SavedTick> {
        let data = &self.context.data;
        let mut saved = Vec::new();
        for (ticks, fluid) in [(&self.block_ticks, false), (&self.fluid_ticks, true)] {
            for (pos, target, delay, priority) in ticks.in_chunk(self.tick, chunk) {
                let id = match target {
                    Target::Block(id) => data.block(id).name.clone(),
                    Target::Fluid(f) => {
                        let source = self.fluid_state(self.context.world.block(pos)).source;
                        match (f, source) {
                            (Fluid::Water, true) => "minecraft:water",
                            (Fluid::Water, false) => "minecraft:flowing_water",
                            (Fluid::Lava, true) => "minecraft:lava",
                            (Fluid::Lava, false) => "minecraft:flowing_lava",
                        }
                        .to_owned()
                    }
                };
                saved.push(crate::world::SavedTick {
                    id,
                    pos,
                    delay: delay.max(0) as i32,
                    priority: i32::from(priority),
                    fluid,
                });
            }
        }
        saved
    }

    pub(super) fn block_tick_runs_now(&self, pos: BlockPos, state: u32) -> bool {
        self.block_ticks
            .runs_now(pos, Target::Block(self.block_id(state)))
    }

    pub(super) fn block_tick_scheduled(&self, pos: BlockPos, state: u32) -> bool {
        self.block_ticks
            .is_scheduled(pos, Target::Block(self.block_id(state)))
    }

    // --- Block events --------------------------------------------------------------

    /// `Level.blockEvent`: queued for the end of the tick, once.
    pub(super) fn block_event(&mut self, pos: BlockPos, block: usize, b0: u8, b1: u8) {
        let event = BlockEventData { pos, block, b0, b1 };
        if !self.block_events.contains(&event) {
            self.block_events.push_back(event);
        }
    }

    /// `ServerLevel.runBlockEvents`: each event whose block is still there;
    /// clients hear of those that happened and play them too.
    fn run_block_events(&mut self) {
        while let Some(event) = self.block_events.pop_front() {
            let state = self.context.world.block(event.pos);
            if self.block_id(state) != event.block {
                continue;
            }
            let happened = match self.kind(state) {
                Kind::Piston => self.piston_event(event.pos, state, event.b0, event.b1),
                _ => false,
            };
            if happened {
                let packet = Encoded::new(&BlockEvent {
                    pos: event.pos,
                    event: event.b0,
                    param: event.b1,
                    block: event.block as i32,
                });
                self.broadcast(
                    None,
                    &Outbound::InChunk {
                        chunk: chunk_of(event.pos),
                        packet: Arc::new(packet),
                    },
                );
            }
        }
    }

    /// Scheduled block ticks, then fluid ticks, then random ticks, as
    /// `ServerLevel.tick` runs them.
    pub(super) fn tick_blocks(&mut self) {
        self.handling_tick = true;
        let now = self.tick;
        for (pos, target) in self.block_ticks.take_due(now) {
            let state = self.context.world.block(pos);
            if Target::Block(self.block_id(state)) == target {
                self.block_tick(pos, state);
            }
        }
        self.block_ticks.running.clear();
        for (pos, target) in self.fluid_ticks.take_due(now) {
            let state = self.context.world.block(pos);
            if let Target::Fluid(fluid) = target
                && self.fluid_state(state).fluid == Some(fluid)
            {
                self.fluid_tick(pos, state);
            }
        }
        self.fluid_ticks.running.clear();
        self.random_ticks();
        self.tick_precipitation();
        self.run_block_events();
        self.handling_tick = false;
    }

    /// `BlockBehaviour.tick`.
    fn block_tick(&mut self, pos: BlockPos, state: u32) {
        match self.kind(state) {
            Kind::Falling | Kind::ConcretePowder => self.falling_tick(pos, state),
            Kind::Farmland => {
                if !self.can_survive(pos, state) {
                    self.farmland_to_dirt(pos);
                }
            }
            Kind::SugarCane | Kind::Cactus => {
                if !self.can_survive(pos, state) {
                    self.destroy_block(pos, true);
                }
            }
            Kind::Leaves => {
                let updated = self.leaf_distance_update(pos, state);
                self.set_block(pos, updated);
            }
            Kind::RedstoneTorch | Kind::RedstoneWallTorch => self.torch_tick(pos, state),
            Kind::Repeater => self.repeater_tick(pos, state),
            Kind::Comparator => self.comparator_refresh(pos, state),
            Kind::Observer => self.observer_tick(pos, state),
            Kind::Dispenser | Kind::Dropper => self.dispense(pos, state),
            Kind::Fire => self.fire_tick(pos, state),
            Kind::Lamp => {
                if self.bool_value(state, "lit") && !self.has_neighbor_signal(pos) {
                    let off = self.with(state, "lit", "false");
                    self.set_block_with(pos, off, update::CLIENTS);
                }
            }
            Kind::Button => self.button_tick(pos, state),
            Kind::PressurePlate | Kind::WeightedPressurePlate => self.plate_tick(pos, state),
            _ => {}
        }
    }

    /// `ServerLevel.tickChunk`: a few random blocks in every section of the
    /// chunks near players get a random tick.
    fn random_ticks(&mut self) {
        if self.players.is_empty() {
            return;
        }
        let centers: Vec<(i32, i32)> = self
            .players
            .values()
            .map(|p| {
                (
                    (p.pos[0].floor() as i32) >> 4,
                    (p.pos[2].floor() as i32) >> 4,
                )
            })
            .collect();
        let picked = self.context.world.random_ticks(
            &centers,
            SIMULATION_DISTANCE,
            self.rules.int("random_tick_speed").max(0) as u32,
            &self.context.data,
            &mut rand::rng(),
        );
        for (pos, state) in picked {
            // An earlier tick may have changed it.
            if self.context.world.block(pos) == state {
                self.random_tick(pos, state);
            }
        }
    }

    fn random_tick(&mut self, pos: BlockPos, state: u32) {
        match self.kind(state) {
            Kind::Crop => self.crop_random_tick(pos, state),
            Kind::Stem => self.stem_random_tick(pos, state),
            Kind::Sapling => self.sapling_random_tick(pos, state),
            Kind::Farmland => self.farmland_random_tick(pos, state),
            Kind::Spreading => self.grass_random_tick(pos, state),
            Kind::Leaves => self.leaves_random_tick(pos, state),
            Kind::SugarCane | Kind::Cactus => self.column_plant_random_tick(pos, state),
            Kind::Liquid if self.fluid_state(state).fluid == Some(Fluid::Lava) => {
                self.lava_random_tick(pos)
            }
            _ => {}
        }
    }

    // --- Light ----------------------------------------------------------------

    /// `getRawBrightness(pos, 0)`: sky or block light, whichever is
    /// brighter, regardless of the time of day.
    pub(super) fn raw_brightness(&self, pos: BlockPos) -> u8 {
        let world = &self.context.world;
        world
            .light(crate::light::Kind::Sky, pos)
            .max(world.light(crate::light::Kind::Block, pos))
    }

    /// `getMaxLocalRawBrightness`: sky light darkened by the time of day,
    /// or block light.
    pub(super) fn local_brightness(&self, pos: BlockPos) -> u8 {
        let world = &self.context.world;
        let sky = i32::from(world.light(crate::light::Kind::Sky, pos)) - self.sky_darken;
        (sky.max(0) as u8).max(world.light(crate::light::Kind::Block, pos))
    }

    pub(super) fn random_horizontal(&self) -> Direction {
        HORIZONTAL[rand::rng().random_range(0..4)]
    }
}
