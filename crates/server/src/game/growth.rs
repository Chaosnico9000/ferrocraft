//! What grows and decays by itself: crops and stems on farmland, saplings
//! into trees, grass spreading over dirt, leaves without a log, sugar cane
//! and cactus. Farmland dries out without water nearby and is trampled.
//! Bone meal speeds it up; hoes, shovels and axes turn blocks into others
//! by the data pack's block transformers.

use std::sync::Arc;

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{Direction, GameMode, UseItemOn};
use rand::Rng;

use super::Game;
use super::blocks::{DIRECTIONS, HORIZONTAL, Kind, above, below, center, offset, update};
use super::fluid::Fluid;
use crate::data::flags;
use crate::inventory;
use crate::tree;

/// `LevelEvent.PARTICLES_AND_SOUND_PLANT_GROWTH`.
pub(super) const LEVEL_EVENT_BONE_MEAL: i32 = 1505;

/// The flowers bone meal brings up on grass in the plains
/// (`flower_plains`).
const PLAINS_FLOWERS: &[&str] = &[
    "minecraft:dandelion",
    "minecraft:poppy",
    "minecraft:azure_bluet",
    "minecraft:oxeye_daisy",
    "minecraft:cornflower",
    "minecraft:red_tulip",
    "minecraft:orange_tulip",
    "minecraft:white_tulip",
    "minecraft:pink_tulip",
];

/// `TreeGrower`s by sapling: normal trees, trees near flowers (the
/// variants with bees), two-by-two trees; each weighted.
fn grower(sapling: &str) -> Option<(Weighted, Weighted, Weighted)> {
    Some(match sapling {
        "minecraft:oak_sapling" => (
            &[("minecraft:oak", 9), ("minecraft:fancy_oak", 1)],
            &[
                ("minecraft:oak_bees_005", 9),
                ("minecraft:fancy_oak_bees_005", 1),
            ],
            &[],
        ),
        "minecraft:spruce_sapling" => (
            &[("minecraft:spruce", 1)],
            &[],
            &[("minecraft:mega_spruce", 1), ("minecraft:mega_pine", 1)],
        ),
        "minecraft:birch_sapling" => (
            &[("minecraft:birch", 1)],
            &[("minecraft:birch_bees_005", 1)],
            &[],
        ),
        "minecraft:jungle_sapling" => (
            &[("minecraft:jungle_tree_no_vine", 1)],
            &[],
            &[("minecraft:mega_jungle_tree", 1)],
        ),
        "minecraft:acacia_sapling" => (&[("minecraft:acacia", 1)], &[], &[]),
        "minecraft:cherry_sapling" => (
            &[("minecraft:cherry", 1)],
            &[("minecraft:cherry_bees_005", 1)],
            &[],
        ),
        "minecraft:dark_oak_sapling" => (&[], &[], &[("minecraft:dark_oak", 1)]),
        "minecraft:pale_oak_sapling" => (&[], &[], &[("minecraft:pale_oak_bonemeal", 1)]),
        "minecraft:mangrove_propagule" => (
            &[("minecraft:mangrove", 15), ("minecraft:tall_mangrove", 85)],
            &[],
            &[],
        ),
        _ => return None,
    })
}

type Weighted = &'static [(&'static str, u32)];

fn pick(list: Weighted, rng: &mut impl Rng) -> Option<&'static str> {
    let total: u32 = list.iter().map(|(_, w)| w).sum();
    if total == 0 {
        return None;
    }
    let mut roll = rng.random_range(0..total);
    for (name, weight) in list {
        if roll < *weight {
            return Some(name);
        }
        roll -= weight;
    }
    None
}

impl Game {
    // --- Crops -----------------------------------------------------------------

    fn max_age(&self, state: u32) -> i32 {
        self.context
            .data
            .block_of(state)
            .and_then(|(_, b)| b.properties.iter().find(|(k, _)| k == "age"))
            .and_then(|(_, values)| values.last()?.parse().ok())
            .unwrap_or(7)
    }

    fn set_age(&self, state: u32, age: i32) -> u32 {
        self.with(state, "age", &age.to_string())
    }

    /// `CropBlock.getGrowthSpeed`: wet farmland around speeds it up, the
    /// same crop in a row on both sides slows it down.
    fn growth_speed(&self, pos: BlockPos, state: u32) -> f32 {
        let world = &self.context.world;
        let data = &self.context.data;
        let mut speed = 1.0;
        let under = below(pos);
        for dx in -1..=1 {
            for dz in -1..=1 {
                let soil = world.block(offset(under, dx, 0, dz));
                let mut block_speed = 0.0;
                if data.state_in_tag(soil, "minecraft:grows_crops") {
                    block_speed = 1.0;
                    if self.int_value(soil, "moisture").unwrap_or(0) > 0 {
                        block_speed = 3.0;
                    }
                }
                if dx != 0 || dz != 0 {
                    block_speed /= 4.0;
                }
                speed += block_speed;
            }
        }
        let id = self.block_id(state);
        let same = |dx, dz| self.block_id(world.block(offset(pos, dx, 0, dz))) == id;
        let horizontal = same(-1, 0) || same(1, 0);
        let vertical = same(0, -1) || same(0, 1);
        if horizontal && vertical || same(-1, -1) || same(1, -1) || same(1, 1) || same(-1, 1) {
            speed /= 2.0;
        }
        speed
    }

    /// `CropBlock.randomTick`.
    pub(super) fn crop_random_tick(&mut self, pos: BlockPos, state: u32) {
        let beetroot = self.is(state, "minecraft:beetroots");
        let mut rng = rand::rng();
        if beetroot && rng.random_range(0..3) == 0 {
            return;
        }
        if self.raw_brightness(pos) < 9 {
            return;
        }
        let age = self.int_value(state, "age").unwrap_or(0);
        if age >= self.max_age(state) {
            return;
        }
        let speed = self.growth_speed(pos, state);
        if rng.random_range(0..(25.0 / speed) as i32 + 1) == 0 {
            let grown = self.set_age(state, age + 1);
            self.set_block_with(pos, grown, update::CLIENTS);
        }
    }

    /// `StemBlock.randomTick`: a grown stem puts a pumpkin or melon next to
    /// it.
    pub(super) fn stem_random_tick(&mut self, pos: BlockPos, state: u32) {
        if self.raw_brightness(pos) < 9 {
            return;
        }
        let speed = self.growth_speed(pos, state);
        let mut rng = rand::rng();
        if rng.random_range(0..(25.0 / speed) as i32 + 1) != 0 {
            return;
        }
        let age = self.int_value(state, "age").unwrap_or(0);
        if age < 7 {
            let grown = self.set_age(state, age + 1);
            self.set_block_with(pos, grown, update::CLIENTS);
            return;
        }
        self.grow_fruit(pos, state);
    }

    fn grow_fruit(&mut self, pos: BlockPos, state: u32) {
        let Some(fruit) = self.stem_fruit(state) else {
            return;
        };
        let direction = self.random_horizontal();
        let at = direction.offset(pos);
        let soil = self.context.world.block(below(at));
        let support = format!(
            "minecraft:supports_{}_stem_fruit",
            fruit.trim_start_matches("minecraft:")
        );
        if self.is_air(self.context.world.block(at))
            && self.context.data.state_in_tag(soil, &support)
        {
            let fruit_state = self.default_of(fruit);
            let attached = format!(
                "minecraft:attached_{}_stem",
                fruit.trim_start_matches("minecraft:")
            );
            let attached = self.default_of(&attached);
            let attached = self.with(attached, "facing", direction.name());
            self.set_block(at, fruit_state);
            self.set_block(pos, attached);
        }
    }

    /// The fruit of a stem or attached stem.
    fn stem_fruit(&self, state: u32) -> Option<&'static str> {
        let name = &self.context.data.block_of(state)?.1.name;
        if name.contains("pumpkin") {
            Some("minecraft:pumpkin")
        } else if name.contains("melon") {
            Some("minecraft:melon")
        } else {
            None
        }
    }

    /// `AttachedStemBlock.updateShape`: without its fruit it is a grown
    /// stem again.
    pub(super) fn attached_stem_shape(
        &self,
        state: u32,
        direction: Direction,
        neighbour: u32,
    ) -> u32 {
        let fruit = self.stem_fruit(state);
        if self.direction_value(state, "facing") == Some(direction)
            && !fruit.is_some_and(|f| self.is(neighbour, f))
        {
            let stem = match fruit {
                Some("minecraft:pumpkin") => "minecraft:pumpkin_stem",
                _ => "minecraft:melon_stem",
            };
            return self.set_age(self.default_of(stem), 7);
        }
        state
    }

    // --- Farmland ----------------------------------------------------------------

    /// `FarmlandBlock.isNearWater`.
    fn near_water(&self, pos: BlockPos) -> bool {
        for dx in -4..=4 {
            for dy in 0..=1 {
                for dz in -4..=4 {
                    let state = self.context.world.block(offset(pos, dx, dy, dz));
                    if self.fluid_state(state).fluid == Some(Fluid::Water) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// `FarmlandBlock.randomTick`: wet near water, dries out and turns to
    /// dirt elsewhere unless a crop holds it.
    pub(super) fn farmland_random_tick(&mut self, pos: BlockPos, state: u32) {
        let moisture = self.int_value(state, "moisture").unwrap_or(0);
        if self.near_water(pos) || self.is_raining_at(above(pos)) {
            if moisture < 7 {
                let wet = self.with(state, "moisture", "7");
                self.set_block_with(pos, wet, update::CLIENTS);
            }
            return;
        }
        if moisture > 0 {
            let drier = self.with(state, "moisture", &(moisture - 1).to_string());
            self.set_block_with(pos, drier, update::CLIENTS);
        } else {
            let above_state = self.context.world.block(above(pos));
            if !self
                .context
                .data
                .state_in_tag(above_state, "minecraft:maintains_farmland")
            {
                self.farmland_to_dirt(pos);
            }
        }
    }

    /// `FarmlandBlock.turnToBaseBlock`.
    pub(super) fn farmland_to_dirt(&mut self, pos: BlockPos) {
        let dirt = self.default_of("minecraft:dirt");
        self.set_block(pos, dirt);
    }

    /// `FarmlandBlock.fallOn`: landing on farmland may trample it.
    pub(super) fn trample(&mut self, pos: BlockPos, fall_distance: f64) {
        let state = self.context.world.block(pos);
        if self.kind(state) == Kind::Farmland && rand::random::<f64>() < fall_distance - 0.5 {
            self.farmland_to_dirt(pos);
        }
    }

    // --- Saplings and trees --------------------------------------------------

    /// `SaplingBlock.randomTick`.
    pub(super) fn sapling_random_tick(&mut self, pos: BlockPos, state: u32) {
        if self.local_brightness(above(pos)) >= 9 && rand::rng().random_range(0..7) == 0 {
            self.advance_tree(pos, state);
        }
    }

    /// `SaplingBlock.advanceTree`: the first time it only gets ready.
    fn advance_tree(&mut self, pos: BlockPos, state: u32) {
        if self.int_value(state, "stage") == Some(0) {
            let ready = self.with(state, "stage", "1");
            self.set_block_with(pos, ready, update::CLIENTS | update::KNOWN_SHAPE);
        } else {
            self.grow_tree(pos, state);
        }
    }

    /// `TreeGrower.growTree`: a two-by-two tree if four saplings stand
    /// together and the sapling has one, else a normal one.
    pub(super) fn grow_tree(&mut self, pos: BlockPos, state: u32) -> bool {
        let data = Arc::clone(&self.context.data);
        let Some((_, block)) = data.block_of(state) else {
            return false;
        };
        let Some((trees, flowering, mega)) = grower(&block.name) else {
            return false;
        };
        let mut rng = rand::rng();
        if let Some(feature) = pick(mega, &mut rng)
            && let Some((dx, dz)) = self.two_by_two(pos, state)
        {
            let saplings: Vec<BlockPos> = [(0, 0), (1, 0), (0, 1), (1, 1)]
                .iter()
                .map(|(x, z)| offset(pos, dx + x, 0, dz + z))
                .collect();
            if self.place_tree(feature, offset(pos, dx, 0, dz), &saplings) {
                return true;
            }
            if trees.is_empty() {
                return false;
            }
        }
        let list = if !flowering.is_empty() && self.flowers_near(pos) {
            flowering
        } else {
            trees
        };
        let Some(feature) = pick(list, &mut rng) else {
            return false;
        };
        self.place_tree(feature, pos, &[pos])
    }

    /// `findTwoByTwoSaplingPos`.
    fn two_by_two(&self, pos: BlockPos, state: u32) -> Option<(i32, i32)> {
        let id = self.block_id(state);
        let world = &self.context.world;
        for dx in [0, -1] {
            for dz in [0, -1] {
                let all = [(0, 0), (1, 0), (0, 1), (1, 1)]
                    .iter()
                    .all(|(x, z)| self.block_id(world.block(offset(pos, dx + x, 0, dz + z))) == id);
                if all {
                    return Some((dx, dz));
                }
            }
        }
        None
    }

    fn flowers_near(&self, pos: BlockPos) -> bool {
        let data = &self.context.data;
        for dx in -2..=2 {
            for dy in -1..=1 {
                for dz in -2..=2 {
                    let state = self.context.world.block(offset(pos, dx, dy, dz));
                    if data.state_in_tag(state, "minecraft:flowers") {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Plans the tree with the saplings gone; places it if it fits.
    fn place_tree(&mut self, feature: &str, origin: BlockPos, saplings: &[BlockPos]) -> bool {
        let data = Arc::clone(&self.context.data);
        let world = Arc::clone(&self.context.world);
        let air = self.air;
        let blocks = |p: BlockPos| {
            if saplings.contains(&p) {
                air
            } else {
                world.block(p)
            }
        };
        let Some(plan) = tree::grow(&data, &blocks, feature, origin, &mut rand::rng()) else {
            return false;
        };
        for &sapling in saplings {
            self.set_block_with(sapling, air, update::CLIENTS | update::KNOWN_SHAPE);
        }
        for (pos, state) in plan {
            self.set_block_with(pos, state, update::ALL | update::KNOWN_SHAPE);
        }
        true
    }

    // --- Grass ---------------------------------------------------------------------

    /// `SpreadingSnowyBlock.canStayAlive`: light must get through to it.
    fn grass_can_stay(&self, pos: BlockPos) -> bool {
        let above_state = self.context.world.block(above(pos));
        if self.is(above_state, "minecraft:snow")
            && self.int_value(above_state, "layers") == Some(1)
        {
            return true;
        }
        let fluid = self.fluid_state(above_state);
        if fluid.amount == 8 {
            return false;
        }
        let data = &self.context.data;
        let closed = data.face_shape(above_state, Direction::Down as usize)
            == [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]
            || data
                .face_shape(above_state, Direction::Down as usize)
                .iter()
                .any(|b| b[0] <= 0.0 && b[2] <= 0.0 && b[3] >= 1.0 && b[5] >= 1.0 && b[1] <= 0.0);
        let dampening = if closed {
            15
        } else {
            data.physics(above_state).dampening
        };
        dampening < 15
    }

    /// `SpreadingSnowyBlock.randomTick`: grass in the dark dies back to
    /// dirt; in the light it spreads to dirt nearby.
    pub(super) fn grass_random_tick(&mut self, pos: BlockPos, state: u32) {
        let base = self.default_of("minecraft:dirt");
        if !self.grass_can_stay(pos) {
            self.set_block(pos, base);
            return;
        }
        if self.local_brightness(above(pos)) < 9 {
            return;
        }
        let grass = self
            .context
            .data
            .block_of(state)
            .map_or(state, |(_, b)| b.default);
        let mut rng = rand::rng();
        for _ in 0..4 {
            let test = offset(
                pos,
                rng.random_range(0..3) - 1,
                rng.random_range(0..5) - 3,
                rng.random_range(0..3) - 1,
            );
            if self.context.world.block(test) == base
                && self.grass_can_stay(test)
                && self
                    .fluid_state(self.context.world.block(above(test)))
                    .fluid
                    != Some(Fluid::Water)
            {
                let above_state = self.context.world.block(above(test));
                let snowy = self
                    .context
                    .data
                    .state_in_tag(above_state, "minecraft:snow");
                let spread = self.with(grass, "snowy", if snowy { "true" } else { "false" });
                self.set_block(test, spread);
            }
        }
    }

    // --- Leaves ----------------------------------------------------------------------

    /// `LeavesBlock.getDistanceAt`: logs count as 0.
    pub(super) fn leaf_distance_of(&self, state: u32) -> i32 {
        if self
            .context
            .data
            .state_in_tag(state, "minecraft:prevents_nearby_leaf_decay")
        {
            return 0;
        }
        self.int_value(state, "distance").unwrap_or(7)
    }

    /// `LeavesBlock.updateDistance`.
    pub(super) fn leaf_distance_update(&self, pos: BlockPos, state: u32) -> u32 {
        let mut distance = 7;
        for direction in DIRECTIONS {
            let neighbour = self.context.world.block(direction.offset(pos));
            distance = distance.min(self.leaf_distance_of(neighbour) + 1);
            if distance == 1 {
                break;
            }
        }
        self.with(state, "distance", &distance.to_string())
    }

    /// `LeavesBlock.randomTick`: leaves seven blocks from any log decay.
    pub(super) fn leaves_random_tick(&mut self, pos: BlockPos, state: u32) {
        if !self.bool_value(state, "persistent") && self.int_value(state, "distance") == Some(7) {
            self.drop_resources(pos, state);
            let left = self.fluid_state(state).legacy_block(self);
            self.set_block(pos, left);
        }
    }

    // --- Sugar cane and cactus -------------------------------------------------

    pub(super) fn sugar_cane_survives(&self, pos: BlockPos) -> bool {
        let data = &self.context.data;
        let below_pos = below(pos);
        let below_state = self.context.world.block(below_pos);
        if self.is(below_state, "minecraft:sugar_cane") {
            return true;
        }
        if !data.state_in_tag(below_state, "minecraft:supports_sugar_cane") {
            return false;
        }
        HORIZONTAL.iter().any(|d| {
            let side = self.context.world.block(d.offset(below_pos));
            self.fluid_state(side).fluid == Some(Fluid::Water)
                || data.state_in_tag(side, "minecraft:supports_sugar_cane_adjacently")
        })
    }

    pub(super) fn cactus_survives(&self, pos: BlockPos) -> bool {
        let world = &self.context.world;
        for direction in HORIZONTAL {
            let side = world.block(direction.offset(pos));
            if self.flag(side, flags::SOLID) || self.fluid_state(side).fluid == Some(Fluid::Lava) {
                return false;
            }
        }
        let below_state = world.block(below(pos));
        (self.is(below_state, "minecraft:cactus")
            || self
                .context
                .data
                .state_in_tag(below_state, "minecraft:supports_cactus"))
            && self.kind(world.block(above(pos))) != Kind::Liquid
    }

    /// `SugarCaneBlock` and `CactusBlock.randomTick`: up to three blocks
    /// high, a block each time the age comes round; a cactus may flower.
    pub(super) fn column_plant_random_tick(&mut self, pos: BlockPos, state: u32) {
        let up = above(pos);
        if !self.is_air(self.context.world.block(up)) {
            return;
        }
        let cactus = self.kind(state) == Kind::Cactus;
        let id = self.block_id(state);
        let age = self.int_value(state, "age").unwrap_or(0);
        let mut height = 1;
        while self.block_id(self.context.world.block(offset(pos, 0, -height, 0))) == id {
            height += 1;
            if cactus && height == 3 && age == 15 {
                return;
            }
        }
        let default = self
            .context
            .data
            .block_of(state)
            .map_or(state, |(_, b)| b.default);
        if !cactus {
            // `SugarCaneBlock`: only a column below three blocks ages.
            if height < 3 {
                if age == 15 {
                    self.set_block(up, default);
                    let reset = self.set_age(state, 0);
                    self.set_block_with(pos, reset, update::KNOWN_SHAPE);
                } else {
                    let older = self.set_age(state, age + 1);
                    self.set_block_with(pos, older, update::KNOWN_SHAPE);
                }
            }
            return;
        }
        if age == 8 && self.cactus_survives(up) {
            let chance = if height >= 3 { 0.25 } else { 0.1 };
            if rand::random::<f64>() <= chance {
                let flower = self.default_of("minecraft:cactus_flower");
                self.set_block(up, flower);
            }
        } else if age == 15 && height < 3 {
            self.set_block(up, default);
            let reset = self.set_age(state, 0);
            self.set_block_with(pos, reset, update::KNOWN_SHAPE);
            self.neighbor_changed_at(up, self.block_id(state));
        }
        if age < 15 {
            let older = self.set_age(state, age + 1);
            self.set_block_with(pos, older, update::KNOWN_SHAPE);
        }
    }

    // --- Using items on blocks ----------------------------------------------------

    /// `Item.useOn` of the items that change the clicked block: bone meal,
    /// and tools with a block transformer. Returns whether the click was
    /// used.
    pub(super) fn use_item_on_block(&mut self, entity_id: i32, click: &UseItemOn) -> bool {
        let data = Arc::clone(&self.context.data);
        let Some(player) = self.players.get(&entity_id) else {
            return false;
        };
        let slot = if click.hand == 1 {
            inventory::OFF_HAND
        } else {
            player.inventory.selected_slot()
        };
        let Some(item) = player.inventory.get(slot).and_then(|s| data.item(s.item)) else {
            return false;
        };
        let survival = player.game_mode() == GameMode::Survival;
        if item.name == "minecraft:bone_meal" {
            if !self.bone_meal(click.pos) {
                return false;
            }
            self.level_event(LEVEL_EVENT_BONE_MEAL, click.pos, 15);
            if survival {
                self.update_inventory(entity_id, |i| {
                    i.take(slot, 1);
                });
                self.send_slots(entity_id, &[slot]);
            }
            self.swing(entity_id);
            return true;
        }
        let Some(name) = &item.block_transformer else {
            return false;
        };
        let Some(transformer) = data.features.block_transformers.get(name) else {
            return false;
        };
        let Ok(face) = Direction::from_id(click.face) else {
            return false;
        };
        let world = Arc::clone(&self.context.world);
        let blocks = |p: BlockPos| world.block(p);
        for entry in transformer.as_array().into_iter().flatten() {
            let disallowed = entry["disallowed_faces"]
                .as_array()
                .is_some_and(|faces| faces.iter().any(|f| f.as_str() == Some(face.name())));
            if disallowed {
                continue;
            }
            let Some(new) = tree::provide(
                &data,
                &blocks,
                &entry["block_state_provider"],
                click.pos,
                &mut rand::rng(),
            ) else {
                continue;
            };
            self.set_block(click.pos, new);
            if let Some(sound) = entry["sound"].as_str() {
                self.play_sound(None, sound, center(click.pos), 1.0, 1.0);
            }
            if survival {
                let wear = entry["item_damage_per_use"].as_i64().unwrap_or(0) as i32;
                if wear > 0 {
                    self.wear(entity_id, slot, wear);
                }
            }
            self.swing(entity_id);
            return true;
        }
        false
    }

    /// `BoneMealItem.growCrop`: whether the block takes bone meal.
    pub(super) fn bone_meal(&mut self, pos: BlockPos) -> bool {
        let state = self.context.world.block(pos);
        let mut rng = rand::rng();
        match self.kind(state) {
            Kind::Crop => {
                let age = self.int_value(state, "age").unwrap_or(0);
                let max = self.max_age(state);
                if age >= max {
                    return false;
                }
                let mut more = rng.random_range(2..=5);
                if self.is(state, "minecraft:beetroots") {
                    more /= 3;
                }
                let grown = self.set_age(state, (age + more).min(max));
                self.set_block_with(pos, grown, update::CLIENTS);
                true
            }
            Kind::Stem => {
                let age = self.int_value(state, "age").unwrap_or(0);
                if age >= 7 {
                    return false;
                }
                let new_age = (age + rng.random_range(2..=5)).min(7);
                let grown = self.set_age(state, new_age);
                self.set_block_with(pos, grown, update::CLIENTS);
                if new_age == 7 {
                    self.grow_fruit(pos, grown);
                }
                true
            }
            Kind::Sapling => {
                if rng.random::<f32>() < 0.45 {
                    self.advance_tree(pos, state);
                }
                true
            }
            Kind::Spreading if self.is(state, "minecraft:grass_block") => {
                if !self.is_air(self.context.world.block(above(pos))) {
                    return false;
                }
                self.grass_bone_meal(pos, state);
                true
            }
            Kind::Plant if self.tall_variant(state).is_some() => self.grow_tall(pos, state),
            _ => false,
        }
    }

    /// The double plant short grass and ferns grow into.
    fn tall_variant(&self, state: u32) -> Option<&'static str> {
        if self.is(state, "minecraft:short_grass") {
            Some("minecraft:tall_grass")
        } else if self.is(state, "minecraft:fern") {
            Some("minecraft:large_fern")
        } else {
            None
        }
    }

    /// `TallGrassBlock.performBonemeal`: a tall plant if there is room
    /// above.
    fn grow_tall(&mut self, pos: BlockPos, state: u32) -> bool {
        let Some(tall) = self.tall_variant(state) else {
            return false;
        };
        let up = above(pos);
        if !self.is_air(self.context.world.block(up)) {
            return false;
        }
        let lower = self.with(self.default_of(tall), "half", "lower");
        let upper = self.with(lower, "half", "upper");
        self.set_block_with(pos, lower, update::CLIENTS);
        self.set_block_with(up, upper, update::ALL);
        true
    }

    /// `GrassBlock.performBonemeal`: grass and now and then a flower spring
    /// up around.
    fn grass_bone_meal(&mut self, pos: BlockPos, grass_block: u32) {
        let grass_id = self.block_id(grass_block);
        let short_grass = self.default_of("minecraft:short_grass");
        let mut rng = rand::rng();
        'attempts: for attempt in 0..128 {
            let mut test = above(pos);
            for _ in 0..attempt / 16 {
                test = offset(
                    test,
                    rng.random_range(-1..=1),
                    rng.random_range(-1..=1) * rng.random_range(0..3) / 2,
                    rng.random_range(-1..=1),
                );
                let under = self.context.world.block(below(test));
                let here = self.context.world.block(test);
                let full =
                    self.context.data.collision_shape(here) == [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];
                if self.block_id(under) != grass_id || full {
                    continue 'attempts;
                }
            }
            let here = self.context.world.block(test);
            if here == short_grass && rng.random::<f32>() < 0.1 {
                self.grow_tall(test, here);
            }
            if self.is_air(here) && (crate::world::MIN_Y..=crate::world::MAX_Y).contains(&test.y) {
                let plant = if rng.random::<f32>() < 0.125 {
                    let flower = PLAINS_FLOWERS[rng.random_range(0..PLAINS_FLOWERS.len())];
                    self.default_of(flower)
                } else {
                    short_grass
                };
                if self.can_survive(test, plant) {
                    self.set_block_with(test, plant, update::CLIENTS);
                }
            }
        }
    }
}
