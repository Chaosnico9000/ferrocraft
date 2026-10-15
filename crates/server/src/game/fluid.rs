//! Water and lava (`FlowingFluid`, `WaterFluid`, `LavaFluid`,
//! `LiquidBlock`): sources feed flowing fluid that loses a level per block
//! (lava two), falls down, looks up to four blocks ahead (lava two) for the
//! nearest way down, and turns into a source between two water sources.
//! Lava meeting water becomes obsidian, cobblestone or stone. And the
//! buckets that pick them up and pour them out.

use std::collections::HashMap;

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{Direction, GameMode, ItemStack};
use rand::Rng;

use super::blocks::{DIRECTIONS, HORIZONTAL, Kind, below, center};
use super::{Game, physics};
use crate::data::{Aabb, flags};
use crate::inventory;
use crate::placement::opposite;

/// Lava's `fizz`.
const LEVEL_EVENT_LAVA_FIZZ: i32 = 1501;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fluid {
    Water,
    Lava,
}

impl Fluid {
    /// Ticks between steps of spreading (`getTickDelay`, the overworld's).
    pub fn tick_delay(self) -> u64 {
        match self {
            Fluid::Water => 5,
            Fluid::Lava => 30,
        }
    }

    /// Levels lost per block sideways.
    fn drop_off(self) -> i32 {
        match self {
            Fluid::Water => 1,
            Fluid::Lava => 2,
        }
    }

    /// How far it looks for a way down.
    fn slope_find_distance(self) -> i32 {
        match self {
            Fluid::Water => 4,
            Fluid::Lava => 2,
        }
    }

    fn block(self) -> &'static str {
        match self {
            Fluid::Water => "minecraft:water",
            Fluid::Lava => "minecraft:lava",
        }
    }

    fn bucket(self) -> &'static str {
        match self {
            Fluid::Water => "minecraft:water_bucket",
            Fluid::Lava => "minecraft:lava_bucket",
        }
    }
}

/// The fluid in a block (`FluidState`): none, or water or lava with an
/// amount from 1 to 8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FluidState {
    pub fluid: Option<Fluid>,
    pub amount: i32,
    pub falling: bool,
    pub source: bool,
}

impl FluidState {
    pub const EMPTY: FluidState = FluidState {
        fluid: None,
        amount: 0,
        falling: false,
        source: false,
    };

    fn source_of(fluid: Fluid) -> Self {
        Self {
            fluid: Some(fluid),
            amount: 8,
            falling: false,
            source: true,
        }
    }

    fn flowing(fluid: Fluid, amount: i32, falling: bool) -> Self {
        Self {
            fluid: Some(fluid),
            amount,
            falling,
            source: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.fluid.is_none()
    }

    fn own_height(&self) -> f32 {
        self.amount as f32 / 9.0
    }

    /// `createLegacyBlock`: the water or lava block for it, air if empty.
    pub fn legacy_block(&self, game: &Game) -> u32 {
        let Some(fluid) = self.fluid else {
            return game.air;
        };
        let level = if self.source {
            0
        } else {
            8 - self.amount.min(8) + if self.falling { 8 } else { 0 }
        };
        let data = &game.context.data;
        data.block_named(fluid.block())
            .map_or(game.air, |b| b.first + level as u32)
    }
}

/// `FlowingFluid.SpreadContext`: block states and holes looked up during
/// one search for the way down.
#[derive(Default)]
struct SpreadContext {
    states: HashMap<BlockPos, u32>,
    holes: HashMap<BlockPos, bool>,
}

impl Game {
    /// `BlockState.getFluidState`.
    pub(super) fn fluid_state(&self, state: u32) -> FluidState {
        let data = &self.context.data;
        let fluid = match data.physics(state).fluid {
            1 => Fluid::Water,
            2 => Fluid::Lava,
            _ => return FluidState::EMPTY,
        };
        let Some((id, block)) = data.block_of(state) else {
            return FluidState::EMPTY;
        };
        if self.kinds[id] != Kind::Liquid {
            // Waterlogged blocks, kelp and seagrass hold a source.
            return FluidState::source_of(fluid);
        }
        // `LiquidBlock`'s only property is `level`.
        match (state - block.first) as i32 {
            0 => FluidState::source_of(fluid),
            level @ 1..8 => FluidState::flowing(fluid, 8 - level, false),
            _ => FluidState::flowing(fluid, 8, true),
        }
    }

    fn fluid_at(&self, pos: BlockPos) -> FluidState {
        self.fluid_state(self.context.world.block(pos))
    }

    /// `FluidState.getHeight`: full if the same fluid is above.
    fn fluid_height(&self, fluid: FluidState, pos: BlockPos) -> f32 {
        if fluid.fluid.is_some() && self.fluid_at(super::blocks::above(pos)).fluid == fluid.fluid {
            1.0
        } else {
            fluid.own_height()
        }
    }

    /// `LiquidBlockContainer`: blocks that can hold water besides water
    /// itself.
    fn is_container(&self, state: u32) -> bool {
        self.context.data.block_of(state).is_some_and(|(_, b)| {
            b.has("waterlogged")
                || [
                    "KelpBlock",
                    "KelpPlantBlock",
                    "SeagrassBlock",
                    "TallSeagrassBlock",
                ]
                .iter()
                .any(|c| b.is(c))
        })
    }

    /// `canHoldAnyFluid`.
    fn can_hold_any_fluid(&self, state: u32) -> bool {
        self.is_container(state)
            || self
                .context
                .data
                .state_in_tag(state, "minecraft:washed_away_by_fluids")
    }

    /// `canHoldSpecificFluid`: containers only take water sources
    /// (`SimpleWaterloggedBlock.canPlaceLiquid`).
    fn can_hold_specific_fluid(&self, state: u32, new: FluidState) -> bool {
        if !self.is_container(state) {
            return true;
        }
        self.context
            .data
            .block_of(state)
            .is_some_and(|(_, b)| b.has("waterlogged"))
            && new.fluid == Some(Fluid::Water)
            && new.source
    }

    /// `FluidState.canBeReplacedWith`: the fluid at `pos` gives way to
    /// `new` flowing in from `direction`.
    fn fluid_replaceable_by(
        &self,
        existing: FluidState,
        pos: BlockPos,
        new: FluidState,
        direction: Direction,
    ) -> bool {
        match existing.fluid {
            None => true,
            Some(Fluid::Water) => direction == Direction::Down && new.fluid != Some(Fluid::Water),
            Some(Fluid::Lava) => {
                self.fluid_height(existing, pos) >= 0.444_444_45 && new.fluid == Some(Fluid::Water)
            }
        }
    }

    /// `canPassThroughWall`: whether fluid gets from one block into the
    /// next past their collision shapes.
    fn can_pass_through_wall(
        &mut self,
        direction: Direction,
        source_state: u32,
        target_state: u32,
    ) -> bool {
        let data = &self.context.data;
        let (source, target) = (
            data.physics(source_state).collision,
            data.physics(target_state).collision,
        );
        let full = |shape: &[Aabb]| shape == [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];
        let (source_shape, target_shape) = (
            data.collision_shape(source_state),
            data.collision_shape(target_state),
        );
        if full(target_shape) || full(source_shape) {
            return false;
        }
        if source_shape.is_empty() && target_shape.is_empty() {
            return true;
        }
        let key = (source, target, direction as u8);
        if let Some(&passes) = self.wall_cache.get(&key) {
            return passes;
        }
        let passes = !faces_close(source_shape, target_shape, direction);
        self.wall_cache.insert(key, passes);
        passes
    }

    /// `canMaybePassThrough`.
    fn can_maybe_pass_through(
        &mut self,
        fluid: Fluid,
        source_state: u32,
        direction: Direction,
        test_state: u32,
    ) -> bool {
        let test = self.fluid_state(test_state);
        !(test.fluid == Some(fluid) && test.source)
            && self.can_hold_any_fluid(test_state)
            && self.can_pass_through_wall(direction, source_state, test_state)
    }

    /// `getNewLiquid`: what the fluid at `pos` should be, from its
    /// neighbours.
    fn new_liquid(&mut self, fluid: Fluid, pos: BlockPos, state: u32) -> FluidState {
        let world = std::sync::Arc::clone(&self.context.world);
        let (mut highest, mut sources) = (0, 0);
        for direction in HORIZONTAL {
            let neighbour = world.block(direction.offset(pos));
            let neighbour_fluid = self.fluid_state(neighbour);
            if neighbour_fluid.fluid == Some(fluid)
                && self.can_pass_through_wall(direction, state, neighbour)
            {
                if neighbour_fluid.source {
                    sources += 1;
                }
                highest = highest.max(neighbour_fluid.amount);
            }
        }
        let converts = match fluid {
            Fluid::Water => self.rules.water_source_conversion,
            Fluid::Lava => self.rules.lava_source_conversion,
        };
        if sources >= 2 && converts {
            let below_state = world.block(below(pos));
            let below_fluid = self.fluid_state(below_state);
            if self.flag(below_state, flags::SOLID)
                || below_fluid.fluid == Some(fluid) && below_fluid.source
            {
                return FluidState::source_of(fluid);
            }
        }
        let above = world.block(super::blocks::above(pos));
        if self.fluid_state(above).fluid == Some(fluid)
            && self.can_pass_through_wall(Direction::Up, state, above)
        {
            return FluidState::flowing(fluid, 8, true);
        }
        let amount = highest - fluid.drop_off();
        if amount <= 0 {
            FluidState::EMPTY
        } else {
            FluidState::flowing(fluid, amount, false)
        }
    }

    /// `FlowingFluid.tick`: flowing fluid follows its sources or dries up,
    /// then everything spreads.
    pub(super) fn fluid_tick(&mut self, pos: BlockPos, state: u32) {
        let mut fluid_state = self.fluid_state(state);
        let Some(fluid) = fluid_state.fluid else {
            return;
        };
        let mut state = state;
        if !fluid_state.source {
            let new = self.new_liquid(fluid, pos, state);
            let delay = self.spread_delay(fluid, pos, fluid_state, new);
            if new.is_empty() {
                fluid_state = new;
                state = self.air;
                self.set_block(pos, state);
            } else if new != fluid_state {
                fluid_state = new;
                state = new.legacy_block(self);
                self.set_block(pos, state);
                self.schedule_fluid_tick_in(pos, fluid, delay);
            }
        }
        self.spread(fluid, pos, state, fluid_state);
    }

    /// `getSpreadDelay`: lava that rises slows down three times in four.
    fn spread_delay(&self, fluid: Fluid, pos: BlockPos, old: FluidState, new: FluidState) -> u64 {
        let delay = fluid.tick_delay();
        if fluid == Fluid::Lava
            && !old.is_empty()
            && !new.is_empty()
            && !old.falling
            && !new.falling
            && self.fluid_height(new, pos) > self.fluid_height(old, pos)
            && rand::rng().random_range(0..4) != 0
        {
            return delay * 4;
        }
        delay
    }

    /// `FlowingFluid.spread`: down if it can, else to the sides.
    fn spread(&mut self, fluid: Fluid, pos: BlockPos, state: u32, fluid_state: FluidState) {
        if fluid_state.is_empty() {
            return;
        }
        let below_pos = below(pos);
        let below_state = self.context.world.block(below_pos);
        let below_fluid = self.fluid_state(below_state);
        if self.can_maybe_pass_through(fluid, state, Direction::Down, below_state) {
            let new = self.new_liquid(fluid, below_pos, below_state);
            if self.fluid_replaceable_by(below_fluid, below_pos, new, Direction::Down)
                && self.can_hold_specific_fluid(below_state, new)
            {
                self.spread_to(fluid, below_pos, below_state, Direction::Down, new);
                if self.source_neighbours(fluid, pos) >= 3 {
                    self.spread_to_sides(fluid, pos, fluid_state, state);
                }
                return;
            }
        }
        if fluid_state.source || !self.is_hole(fluid, state, below_state) {
            self.spread_to_sides(fluid, pos, fluid_state, state);
        }
    }

    fn source_neighbours(&self, fluid: Fluid, pos: BlockPos) -> usize {
        HORIZONTAL
            .iter()
            .filter(|d| {
                let f = self.fluid_at(d.offset(pos));
                f.fluid == Some(fluid) && f.source
            })
            .count()
    }

    fn spread_to_sides(
        &mut self,
        fluid: Fluid,
        pos: BlockPos,
        fluid_state: FluidState,
        state: u32,
    ) {
        let neighbour = if fluid_state.falling {
            7
        } else {
            fluid_state.amount - fluid.drop_off()
        };
        if neighbour <= 0 {
            return;
        }
        for (direction, new) in self.get_spread(fluid, pos, state) {
            let at = direction.offset(pos);
            let at_state = self.context.world.block(at);
            self.spread_to(fluid, at, at_state, direction, new);
        }
    }

    /// `isWaterHole`: the fluid can fall through `bottom`.
    fn is_hole(&mut self, fluid: Fluid, top_state: u32, bottom_state: u32) -> bool {
        if !self.can_pass_through_wall(Direction::Down, top_state, bottom_state) {
            return false;
        }
        if self.fluid_state(bottom_state).fluid == Some(fluid) {
            return true;
        }
        self.can_hold_any_fluid(bottom_state)
            && self.can_hold_specific_fluid(bottom_state, FluidState::flowing(fluid, 1, false))
    }

    /// `getSpread`: the sides with the shortest way down get the fluid;
    /// in `Direction` order.
    fn get_spread(
        &mut self,
        fluid: Fluid,
        pos: BlockPos,
        state: u32,
    ) -> Vec<(Direction, FluidState)> {
        let mut lowest = 1000;
        let mut result: Vec<(Direction, FluidState)> = Vec::new();
        let mut context = SpreadContext::default();
        for direction in HORIZONTAL {
            let test = direction.offset(pos);
            let test_state = self.context.world.block(test);
            if !self.can_maybe_pass_through(fluid, state, direction, test_state) {
                continue;
            }
            let new = self.new_liquid(fluid, test, test_state);
            if !self.can_hold_specific_fluid(test_state, new) {
                continue;
            }
            let distance = if self.context_hole(fluid, &mut context, test) {
                0
            } else {
                self.slope_distance(
                    fluid,
                    &mut context,
                    test,
                    1,
                    opposite(direction),
                    test_state,
                )
            };
            if distance < lowest {
                result.clear();
            }
            if distance <= lowest {
                let test_fluid = self.fluid_state(test_state);
                if self.fluid_replaceable_by(test_fluid, test, new, direction) {
                    result.push((direction, new));
                }
                lowest = distance;
            }
        }
        result.sort_by_key(|(d, _)| *d as u8);
        result
    }

    fn context_state(&self, context: &mut SpreadContext, pos: BlockPos) -> u32 {
        *context
            .states
            .entry(pos)
            .or_insert_with(|| self.context.world.block(pos))
    }

    fn context_hole(&mut self, fluid: Fluid, context: &mut SpreadContext, pos: BlockPos) -> bool {
        if let Some(&hole) = context.holes.get(&pos) {
            return hole;
        }
        let state = self.context_state(context, pos);
        let below_state = self.context.world.block(below(pos));
        let hole = self.is_hole(fluid, state, below_state);
        context.holes.insert(pos, hole);
        hole
    }

    /// `getSlopeDistance`: how many blocks to the nearest way down.
    fn slope_distance(
        &mut self,
        fluid: Fluid,
        context: &mut SpreadContext,
        pos: BlockPos,
        pass: i32,
        from: Direction,
        state: u32,
    ) -> i32 {
        let mut lowest = 1000;
        for direction in HORIZONTAL {
            if direction == from {
                continue;
            }
            let test = direction.offset(pos);
            let test_state = self.context_state(context, test);
            if self.can_maybe_pass_through(fluid, state, direction, test_state)
                && self.can_hold_specific_fluid(test_state, FluidState::flowing(fluid, 1, false))
            {
                if self.context_hole(fluid, context, test) {
                    return pass;
                }
                if pass < fluid.slope_find_distance() {
                    let distance = self.slope_distance(
                        fluid,
                        context,
                        test,
                        pass + 1,
                        opposite(direction),
                        test_state,
                    );
                    lowest = lowest.min(distance);
                }
            }
        }
        lowest
    }

    /// `spreadTo`: the fluid takes the block over; lava falling into water
    /// turns it to stone.
    fn spread_to(
        &mut self,
        fluid: Fluid,
        pos: BlockPos,
        state: u32,
        direction: Direction,
        target: FluidState,
    ) {
        if fluid == Fluid::Lava
            && direction == Direction::Down
            && self.fluid_state(state).fluid == Some(Fluid::Water)
        {
            if self.kind(state) == Kind::Liquid {
                let stone = self.default_of("minecraft:stone");
                self.set_block(pos, stone);
            }
            self.level_event(LEVEL_EVENT_LAVA_FIZZ, pos, 0);
            return;
        }
        if self.is_container(state) {
            self.place_liquid(pos, state, target);
            return;
        }
        if !self.is_air(state) {
            match fluid {
                Fluid::Water => {
                    if self.kind(state) != Kind::Liquid {
                        self.drop_resources(pos, state);
                    }
                }
                Fluid::Lava => self.level_event(LEVEL_EVENT_LAVA_FIZZ, pos, 0),
            }
        }
        let block = target.legacy_block(self);
        self.set_block(pos, block);
    }

    /// `SimpleWaterloggedBlock.placeLiquid`.
    fn place_liquid(&mut self, pos: BlockPos, state: u32, fluid: FluidState) -> bool {
        if fluid.fluid == Some(Fluid::Water)
            && fluid.source
            && self.value(state, "waterlogged") == Some("false")
        {
            let wet = self.with(state, "waterlogged", "true");
            self.set_block(pos, wet);
            self.schedule_fluid_tick(pos, Fluid::Water);
            return true;
        }
        false
    }

    /// `LiquidBlock.onPlace` and `neighborChanged`.
    pub(super) fn liquid_on_place(&mut self, pos: BlockPos, state: u32) {
        if self.should_spread_liquid(pos, state)
            && let Some(fluid) = self.fluid_state(state).fluid
        {
            self.schedule_fluid_tick(pos, fluid);
        }
    }

    pub(super) fn liquid_neighbor_changed(&mut self, pos: BlockPos, state: u32) {
        self.liquid_on_place(pos, state);
    }

    /// `shouldSpreadLiquid`: lava touching water hardens instead.
    fn should_spread_liquid(&mut self, pos: BlockPos, state: u32) -> bool {
        let fluid = self.fluid_state(state);
        if fluid.fluid != Some(Fluid::Lava) {
            return true;
        }
        let on_soul_soil = self.is(self.context.world.block(below(pos)), "minecraft:soul_soil");
        // `POSSIBLE_FLOW_DIRECTIONS`.
        for direction in [
            Direction::Down,
            Direction::South,
            Direction::North,
            Direction::East,
            Direction::West,
        ] {
            let neighbour = opposite(direction).offset(pos);
            let neighbour_state = self.context.world.block(neighbour);
            if self.fluid_state(neighbour_state).fluid == Some(Fluid::Water) {
                let hard = if fluid.source {
                    "minecraft:obsidian"
                } else {
                    "minecraft:cobblestone"
                };
                let hard = self.default_of(hard);
                self.set_block(pos, hard);
                self.level_event(LEVEL_EVENT_LAVA_FIZZ, pos, 0);
                return false;
            }
            if on_soul_soil && self.is(neighbour_state, "minecraft:blue_ice") {
                let basalt = self.default_of("minecraft:basalt");
                self.set_block(pos, basalt);
                self.level_event(LEVEL_EVENT_LAVA_FIZZ, pos, 0);
                return false;
            }
        }
        true
    }

    /// Whether some neighbour of `pos` holds water (`touchesLiquid` of
    /// concrete powder, without the sturdy-face exception for walls).
    pub(super) fn touches_water(&self, pos: BlockPos) -> bool {
        DIRECTIONS.iter().any(|d| {
            *d != Direction::Down
                && self.fluid_at(d.offset(pos)).fluid == Some(Fluid::Water)
                && !self.sturdy(d.offset(pos), opposite(*d))
        })
    }

    // --- Buckets ---------------------------------------------------------------

    /// `BucketItem.use`: an empty bucket scoops up a source, a full one
    /// pours it out where the player looks. Returns whether it did.
    pub(super) fn use_bucket(&mut self, entity_id: i32, slot: usize) -> bool {
        let data = std::sync::Arc::clone(&self.context.data);
        let Some(player) = self.players.get(&entity_id) else {
            return false;
        };
        let Some(stack) = player.inventory.get(slot).cloned() else {
            return false;
        };
        let Some(name) = data.item(stack.item).map(|i| i.name.as_str()) else {
            return false;
        };
        let content = match name {
            "minecraft:bucket" => None,
            "minecraft:water_bucket" => Some(Fluid::Water),
            "minecraft:lava_bucket" => Some(Fluid::Lava),
            _ => return false,
        };
        let creative = player.game_mode() == GameMode::Creative;
        let sneaking = player.sneaking;
        let reach = if creative { 5.0 } else { 4.5 };
        let eyes = player.eyes();
        let (yaw, pitch) = (
            f64::from(player.yaw).to_radians(),
            f64::from(player.pitch).to_radians(),
        );
        let look = [
            -yaw.sin() * pitch.cos(),
            -pitch.sin(),
            yaw.cos() * pitch.cos(),
        ];
        let to = std::array::from_fn(|i| eyes[i] + look[i] * reach);
        let Some(hit) = self.clip_outline(eyes, to, content.is_none()) else {
            return false;
        };
        let face = hit_face(&hit, look);
        let result = match content {
            None => {
                let state = self.context.world.block(hit.block);
                let Some(fluid) = self.pick_up_fluid(hit.block, state) else {
                    return false;
                };
                let sound = match fluid {
                    Fluid::Water => "minecraft:item.bucket.fill",
                    Fluid::Lava => "minecraft:item.bucket.fill_lava",
                };
                self.play_sound(None, sound, center(hit.block), 1.0, 1.0);
                fluid.bucket()
            }
            Some(fluid) => {
                let clicked = self.context.world.block(hit.block);
                let at = if self.is_container(clicked) && fluid == Fluid::Water {
                    hit.block
                } else {
                    face.offset(hit.block)
                };
                if !self.empty_bucket(fluid, at, Some(face.offset(hit.block)), sneaking) {
                    return false;
                }
                "minecraft:bucket"
            }
        };
        let Some(result) = data.item_id(result) else {
            return true;
        };
        self.swing(entity_id);
        self.fill_result(entity_id, slot, creative, result);
        true
    }

    /// `ItemUtils.createFilledResult`: the bucket in hand becomes the new
    /// one; in creative the old one stays and the new one is added once.
    fn fill_result(&mut self, entity_id: i32, slot: usize, creative: bool, result: i32) {
        let new = ItemStack {
            item: result,
            count: 1,
            patch: Default::default(),
        };
        if creative {
            let has = self
                .players
                .get(&entity_id)
                .is_some_and(|p| p.inventory.contains(result));
            if !has {
                self.give(entity_id, new);
            }
            return;
        }
        let mut emptied = false;
        self.update_inventory(entity_id, |inventory| {
            inventory.take(slot, 1);
            emptied = inventory.get(slot).is_none();
            if emptied {
                inventory.set(slot, Some(new.clone()));
            }
        });
        self.send_slots(entity_id, &[slot]);
        if !emptied {
            self.give(entity_id, new);
        }
    }

    /// `BucketPickup.pickupBlock`: a source becomes air, a waterlogged
    /// block dry.
    fn pick_up_fluid(&mut self, pos: BlockPos, state: u32) -> Option<Fluid> {
        let fluid = self.fluid_state(state);
        if !fluid.source {
            return None;
        }
        if self.kind(state) == Kind::Liquid {
            self.set_block(pos, self.air);
            return fluid.fluid;
        }
        if self.bool_value(state, "waterlogged") {
            let dry = self.with(state, "waterlogged", "false");
            self.set_block(pos, dry);
            return Some(Fluid::Water);
        }
        None
    }

    /// `BucketItem.emptyContents`: pours a source out at `pos`, or at
    /// `fallback` if `pos` does not take it.
    fn empty_bucket(
        &mut self,
        fluid: Fluid,
        pos: BlockPos,
        fallback: Option<BlockPos>,
        sneaking: bool,
    ) -> bool {
        let state = self.context.world.block(pos);
        let physics = *self.context.data.physics(state);
        let may_replace = physics.has(flags::REPLACEABLE) || !physics.has(flags::SOLID);
        let container = self.is_container(state) && fluid == Fluid::Water;
        let place = may_replace || container;
        let inside = self.is_air(state) || place && (!sneaking || fallback.is_none());
        if !inside {
            return match fallback {
                Some(other) if other != pos => self.empty_bucket(fluid, other, None, sneaking),
                _ => false,
            };
        }
        let sound = match fluid {
            Fluid::Water => "minecraft:item.bucket.empty",
            Fluid::Lava => "minecraft:item.bucket.empty_lava",
        };
        if container && self.value(state, "waterlogged").is_some() {
            if self.place_liquid(pos, state, FluidState::source_of(fluid)) {
                self.play_sound(None, sound, center(pos), 1.0, 1.0);
                return true;
            }
            return false;
        }
        if may_replace && self.kind(state) != Kind::Liquid {
            self.destroy_block(pos, true);
        }
        let source = FluidState::source_of(fluid).legacy_block(self);
        if !self.set_block(pos, source) && !self.fluid_state(state).source {
            return false;
        }
        self.play_sound(None, sound, center(pos), 1.0, 1.0);
        true
    }

    /// `Level.clip` with `Block.OUTLINE`: the first block outline the line
    /// meets, or a fluid source if `sources`.
    pub(super) fn clip_outline(
        &self,
        from: [f64; 3],
        to: [f64; 3],
        sources: bool,
    ) -> Option<physics::Hit> {
        let data = &self.context.data;
        physics::clip_shapes(&self.context.world, from, to, &|pos, state| {
            let mut shapes = data.outline_shape(state).to_vec();
            let fluid = self.fluid_state(state);
            if sources && fluid.source {
                shapes.push([
                    0.0,
                    0.0,
                    0.0,
                    1.0,
                    f64::from(self.fluid_height(fluid, pos)),
                    1.0,
                ]);
            }
            shapes
        })
    }
}

impl inventory::Inventory {
    /// Whether some slot holds the item.
    pub fn contains(&self, item: i32) -> bool {
        (0..inventory::SLOTS).any(|slot| self.get(slot).is_some_and(|s| s.item == item))
    }
}

/// The face a line looking along `look` hit.
pub fn hit_face(hit: &physics::Hit, look: [f64; 3]) -> Direction {
    match (hit.axis, look[hit.axis] > 0.0) {
        (0, true) => Direction::West,
        (0, false) => Direction::East,
        (1, true) => Direction::Down,
        (1, false) => Direction::Up,
        (_, true) => Direction::North,
        (_, false) => Direction::South,
    }
}

/// `Shapes.mergedFaceOccludes`: whether the faces of two shapes where
/// they meet, towards `direction` from the first, together close the
/// whole side.
fn faces_close(first: &[Aabb], second: &[Aabb], direction: Direction) -> bool {
    let axis = match direction {
        Direction::West | Direction::East => 0,
        Direction::Down | Direction::Up => 1,
        _ => 2,
    };
    let positive = matches!(
        direction,
        Direction::Up | Direction::South | Direction::East
    );
    // The shape on the low side of the border touches it with its maximum,
    // the one on the high side with its minimum.
    let (low, high) = if positive {
        (first, second)
    } else {
        (second, first)
    };
    let touching = |shape: &[Aabb], at_max: bool| -> Vec<Aabb> {
        shape
            .iter()
            .filter(|b| {
                if at_max {
                    (b[axis + 3] - 1.0).abs() < 1e-7
                } else {
                    b[axis].abs() < 1e-7
                }
            })
            .copied()
            .collect()
    };
    let faces: Vec<Aabb> = touching(low, true)
        .into_iter()
        .chain(touching(high, false))
        .collect();
    let (u, v) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    // Shapes are made of sixteenths; the centre of every sixteenth square
    // must be covered.
    (0..16).all(|i| {
        (0..16).all(|j| {
            let (a, b) = ((f64::from(i) + 0.5) / 16.0, (f64::from(j) + 0.5) / 16.0);
            faces
                .iter()
                .any(|f| f[u] <= a && a <= f[u + 3] && f[v] <= b && b <= f[v + 3])
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_of_slabs_close_where_they_meet() {
        let bottom = [[0.0, 0.0, 0.0, 1.0, 0.5, 1.0]];
        let top = [[0.0, 0.5, 0.0, 1.0, 1.0, 1.0]];
        // Side by side, two bottom slabs leave the upper half open.
        assert!(!faces_close(&bottom, &bottom, Direction::East));
        // A bottom slab below a top slab: neither touches the border.
        assert!(!faces_close(&bottom, &top, Direction::Up));
        // A top slab below a bottom slab closes the border fully.
        assert!(faces_close(&top, &bottom, Direction::Up));
        assert!(!faces_close(&[], &[], Direction::North));
    }
}
