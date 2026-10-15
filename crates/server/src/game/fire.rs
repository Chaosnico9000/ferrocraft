//! Fire (`FireBlock`, `SoulFireBlock`, `BaseFireBlock`): it needs a
//! sturdy floor or something flammable next to it, ages, burns flammable
//! blocks away and jumps to air near flammable blocks, the more readily the
//! younger it is. Rain puts it out. Lava sets fire to flammable blocks
//! around it (`LavaFluid.randomTick`). Flint and steel and fire charges
//! light fires, campfires and candles, and TNT.

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{Direction, ItemStack};
use rand::Rng;

use super::Game;
use super::blocks::{DIRECTIONS, Kind, above, below, center, offset, priority, update};
use crate::data::flags;

impl Game {
    /// The fire's ignite and burn odds of a block; nothing burns while it
    /// holds water.
    pub(super) fn fire_odds(&self, state: u32) -> (i32, i32) {
        if self.bool_value(state, "waterlogged") {
            return (0, 0);
        }
        self.context
            .data
            .block_of(state)
            .and_then(|(_, b)| b.fire)
            .map_or((0, 0), |[ignite, burn]| (ignite, burn))
    }

    /// `FireBlock.canBurn`.
    pub(super) fn can_burn(&self, state: u32) -> bool {
        self.fire_odds(state).0 > 0
    }

    pub(super) fn is_fire(&self, state: u32) -> bool {
        self.context.data.state_in_tag(state, "minecraft:fire")
    }

    /// `BaseFireBlock.getState`: soul fire on soul sand and soil, else fire
    /// shaped to the flammable blocks around it.
    pub(super) fn fire_state_at(&self, pos: BlockPos) -> u32 {
        let below_state = self.context.world.block(below(pos));
        if self
            .context
            .data
            .state_in_tag(below_state, "minecraft:soul_fire_base_blocks")
        {
            return self.default_of("minecraft:soul_fire");
        }
        let fire = self.default_of("minecraft:fire");
        if self.can_burn(below_state) || self.sturdy(below(pos), Direction::Up) {
            return fire;
        }
        let mut state = fire;
        for direction in DIRECTIONS {
            if direction == Direction::Down {
                continue;
            }
            let burns = self.can_burn(self.context.world.block(direction.offset(pos)));
            state = self.with(
                state,
                direction.name(),
                if burns { "true" } else { "false" },
            );
        }
        state
    }

    /// `isValidFireLocation`: something flammable next to it.
    fn fire_location_valid(&self, pos: BlockPos) -> bool {
        DIRECTIONS
            .iter()
            .any(|d| self.can_burn(self.context.world.block(d.offset(pos))))
    }

    /// `canSurvive` of fire and soul fire.
    pub(super) fn fire_survives(&self, pos: BlockPos, state: u32) -> bool {
        if self.is(state, "minecraft:soul_fire") {
            let below_state = self.context.world.block(below(pos));
            return self
                .context
                .data
                .state_in_tag(below_state, "minecraft:soul_fire_base_blocks");
        }
        self.sturdy(below(pos), Direction::Up) || self.fire_location_valid(pos)
    }

    /// `BaseFireBlock.canBePlacedAt`.
    pub(super) fn can_place_fire(&self, pos: BlockPos, _facing: Direction) -> bool {
        let state = self.context.world.block(pos);
        self.is_air(state) && self.fire_survives(pos, self.fire_state_at(pos))
    }

    /// `getStateWithAge`.
    fn fire_with_age(&self, pos: BlockPos, age: i32) -> u32 {
        let state = self.fire_state_at(pos);
        if self.is(state, "minecraft:fire") {
            self.with(state, "age", &age.to_string())
        } else {
            state
        }
    }

    /// `FireBlock.updateShape`.
    pub(super) fn fire_shape(&self, pos: BlockPos, state: u32) -> u32 {
        if !self.fire_survives(pos, state) {
            return self.air;
        }
        if self.is(state, "minecraft:soul_fire") {
            return self.default_of("minecraft:soul_fire");
        }
        let age = self.int_value(state, "age").unwrap_or(0);
        self.fire_with_age(pos, age)
    }

    /// `BaseFireBlock.onPlace` and `FireBlock.onPlace`.
    pub(super) fn fire_on_place(&mut self, pos: BlockPos, state: u32, new_block: bool) {
        if new_block && !self.fire_survives(pos, state) {
            self.remove_block_keeping_fluid(pos);
            return;
        }
        if self.is(state, "minecraft:fire") {
            let delay = 30 + rand::rng().random_range(0..10);
            self.schedule_block_tick(pos, state, delay, priority::NORMAL);
        }
    }

    /// `Level.removeBlock`: whatever fluid was there stays.
    pub(super) fn remove_block_keeping_fluid(&mut self, pos: BlockPos) {
        let left = self
            .fluid_state(self.context.world.block(pos))
            .legacy_block(self);
        self.set_block(pos, left);
    }

    /// `canSpreadFireAround`: a player within the spread radius of the
    /// game rule, anywhere if it is -1, nowhere if 0.
    pub(super) fn fire_may_spread(&self, pos: BlockPos) -> bool {
        let radius = self.rules.int("fire_spread_radius_around_player");
        if radius == -1 {
            return true;
        }
        let c = center(pos);
        let radius = f64::from(radius);
        self.players.values().any(|p| {
            let d = [p.pos[0] - c[0], p.pos[2] - c[2]];
            // `ChunkMap.anyPlayerCloseEnoughTo` measures horizontally.
            d[0] * d[0] + d[1] * d[1] < radius * radius
        })
    }

    /// `isNearRain`.
    fn near_rain(&self, pos: BlockPos) -> bool {
        self.is_raining_at(pos)
            || [
                Direction::West,
                Direction::East,
                Direction::North,
                Direction::South,
            ]
            .iter()
            .any(|d| self.is_raining_at(d.offset(pos)))
    }

    /// `FireBlock.tick`.
    pub(super) fn fire_tick(&mut self, pos: BlockPos, state: u32) {
        let mut rng = rand::rng();
        let delay = 30 + rng.random_range(0..10);
        self.schedule_block_tick(pos, state, delay, priority::NORMAL);
        if !self.fire_may_spread(pos) {
            return;
        }
        if !self.fire_survives(pos, state) {
            self.remove_block_keeping_fluid(pos);
        }
        let below_state = self.context.world.block(below(pos));
        let infiniburn = self
            .context
            .data
            .state_in_tag(below_state, "minecraft:infiniburn_overworld");
        let age = self.int_value(state, "age").unwrap_or(0);
        if !infiniburn
            && self.raining()
            && self.near_rain(pos)
            && rng.random::<f32>() < 0.2 + age as f32 * 0.03
        {
            self.remove_block_keeping_fluid(pos);
            return;
        }
        let new_age = (age + rng.random_range(0..3) / 2).min(15);
        if new_age != age {
            let older = self.with(state, "age", &new_age.to_string());
            self.set_block_with(
                pos,
                older,
                update::INVISIBLE | update::SKIP_BLOCK_ENTITY_SIDEEFFECTS,
            );
        }
        if !infiniburn {
            if !self.fire_location_valid(pos) {
                if !self.sturdy(below(pos), Direction::Up) || age > 3 {
                    self.remove_block_keeping_fluid(pos);
                }
                return;
            }
            if age == 15 && rng.random_range(0..4) == 0 && !self.can_burn(below_state) {
                self.remove_block_keeping_fluid(pos);
                return;
            }
        }
        for (neighbour, chance) in [
            (Direction::East, 300),
            (Direction::West, 300),
            (Direction::Down, 250),
            (Direction::Up, 250),
            (Direction::North, 300),
            (Direction::South, 300),
        ] {
            self.check_burn_out(neighbour.offset(pos), chance, age);
        }
        let difficulty = i32::from(self.context.difficulty);
        for dx in -1..=1 {
            for dz in -1..=1 {
                for dy in -1..=4 {
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    let rate = if dy > 1 { 100 + (dy - 1) * 100 } else { 100 };
                    let test = offset(pos, dx, dy, dz);
                    let ignite = self.ignite_odds_at(test);
                    if ignite <= 0 {
                        continue;
                    }
                    let odds = (ignite + 40 + difficulty * 7) / (age + 30);
                    if odds > 0
                        && rng.random_range(0..rate) <= odds
                        && (!self.raining() || !self.near_rain(test))
                    {
                        let spread_age = (age + rng.random_range(0..5) / 4).min(15);
                        let fire = self.fire_with_age(test, spread_age);
                        self.set_block(test, fire);
                    }
                }
            }
        }
    }

    /// `checkBurnOut`: a flammable block burns away, maybe leaving fire;
    /// TNT goes off.
    fn check_burn_out(&mut self, pos: BlockPos, chance: i32, age: i32) {
        let mut rng = rand::rng();
        let state = self.context.world.block(pos);
        let (_, burn) = self.fire_odds(state);
        if rng.random_range(0..chance) >= burn {
            return;
        }
        if rng.random_range(0..age + 10) < 5 && !self.is_raining_at(pos) {
            let new_age = (age + rng.random_range(0..5) / 4).min(15);
            let fire = self.fire_with_age(pos, new_age);
            self.set_block(pos, fire);
        } else {
            self.remove_block_keeping_fluid(pos);
        }
        if self.kind(state) == Kind::Tnt {
            self.prime_tnt(pos, None, super::tnt::DEFAULT_FUSE);
        }
    }

    /// `getIgniteOdds(level, pos)`: for air, the most flammable neighbour.
    fn ignite_odds_at(&self, pos: BlockPos) -> i32 {
        if !self.is_air(self.context.world.block(pos)) {
            return 0;
        }
        DIRECTIONS
            .iter()
            .map(|d| self.fire_odds(self.context.world.block(d.offset(pos))).0)
            .max()
            .unwrap_or(0)
    }

    /// `LavaFluid.randomTick`: lava sets fire above and around itself.
    pub(super) fn lava_random_tick(&mut self, pos: BlockPos) {
        if !self.fire_may_spread(pos) {
            return;
        }
        let mut rng = rand::rng();
        let passes = rng.random_range(0..3);
        if passes > 0 {
            let mut test = pos;
            for _ in 0..passes {
                test = offset(test, rng.random_range(-1..=1), 1, rng.random_range(-1..=1));
                let state = self.context.world.block(test);
                if self.is_air(state) {
                    if DIRECTIONS.iter().any(|d| self.lava_ignites(d.offset(test))) {
                        let fire = self.fire_state_at(test);
                        self.set_block(test, fire);
                        return;
                    }
                } else if self
                    .context
                    .data
                    .state_in_tag(state, "minecraft:blocks_lava_fire_spread")
                {
                    return;
                }
            }
        } else {
            for _ in 0..3 {
                let test = offset(pos, rng.random_range(-1..=1), 0, rng.random_range(-1..=1));
                if self.is_air(self.context.world.block(above(test))) && self.lava_ignites(test) {
                    let fire = self.fire_state_at(test);
                    self.set_block(above(test), fire);
                }
            }
        }
    }

    /// `LavaFluid.isFlammable`: `ignitedByLava`.
    fn lava_ignites(&self, pos: BlockPos) -> bool {
        self.flag(self.context.world.block(pos), flags::IGNITED_BY_LAVA)
    }

    /// `CampfireBlock`, `CandleBlock` and `CandleCakeBlock.canLight`.
    pub(super) fn can_light(&self, state: u32) -> bool {
        let data = &self.context.data;
        let unlit = self.value(state, "lit") == Some("false");
        let dry = !self.bool_value(state, "waterlogged");
        unlit
            && ((data.state_in_tag(state, "minecraft:campfires") && dry)
                || (data.state_in_tag(state, "minecraft:candles") && dry)
                || data.state_in_tag(state, "minecraft:candle_cakes"))
    }

    /// `FlintAndSteelItem.useOn` and `FireChargeItem.useOn`: lights the
    /// clicked campfire or candle, or fire on the clicked face. Whether it
    /// did.
    pub(super) fn strike_fire(&mut self, clicked: BlockPos, face: Direction, charge: bool) -> bool {
        let state = self.context.world.block(clicked);
        let mut rng = rand::rng();
        let (sound, pitch) = if charge {
            (
                "minecraft:item.firecharge.use",
                (rng.random::<f32>() - rng.random::<f32>()) * 0.2 + 1.0,
            )
        } else {
            (
                "minecraft:item.flintandsteel.use",
                rng.random::<f32>() * 0.4 + 0.8,
            )
        };
        if self.can_light(state) {
            self.play_sound(None, sound, center(clicked), 1.0, pitch);
            let lit = self.with(state, "lit", "true");
            self.set_block(clicked, lit);
            return true;
        }
        let at = face.offset(clicked);
        if !self.can_place_fire(at, face) {
            return false;
        }
        self.play_sound(None, sound, center(at), 1.0, pitch);
        let fire = self.fire_state_at(at);
        self.set_block(at, fire);
        true
    }

    /// What a stack of fire charges or flint and steel becomes after one
    /// use.
    pub(super) fn after_strike(&self, stack: ItemStack, charge: bool) -> Option<ItemStack> {
        if charge {
            let count = stack.count - 1;
            return (count > 0).then_some(ItemStack { count, ..stack });
        }
        self.worn(stack.clone(), 1).unwrap_or(Some(stack))
    }
}
