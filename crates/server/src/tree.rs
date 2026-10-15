//! Trees as vanilla's `TreeFeature` grows them from saplings, read from
//! the data pack's tree features (`features.json`): a trunk placer puts the
//! logs, a foliage placer the leaves at the points the trunk leaves for it.
//! Straight, forking (acacia), dark oak, fancy oak and cherry trunks are
//! known, with blob, fancy, spruce, acacia, dark oak and cherry foliage;
//! other trees (giant spruces, mangroves) do not grow.
//!
//! [`grow`] only plans: it returns the blocks to set, which the game then
//! places like any other block change.

use std::collections::{HashMap, HashSet, VecDeque};

use ferrocraft_protocol::codec::BlockPos;
use rand::Rng;
use serde_json::Value;

use crate::data::GameData;
use crate::world::{MAX_Y, MIN_Y};

/// What a planned tree reads of the world.
pub trait Blocks {
    fn block(&self, pos: BlockPos) -> u32;
}

impl<F: Fn(BlockPos) -> u32> Blocks for F {
    fn block(&self, pos: BlockPos) -> u32 {
        self(pos)
    }
}

/// The blocks a tree sets, in order.
pub type Plan = Vec<(BlockPos, u32)>;

/// A sample of an `IntProvider`: a constant, `uniform`, or
/// `weighted_list`.
pub fn sample_int(value: &Value, rng: &mut impl Rng) -> i32 {
    match value {
        Value::Number(n) => n.as_i64().unwrap_or(0) as i32,
        Value::Object(o) => {
            let ty = o
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("minecraft:uniform");
            match ty {
                "minecraft:constant" => o.get("value").map_or(0, |v| sample_int(v, rng)),
                "minecraft:weighted_list" => {
                    let entries = o
                        .get("distribution")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let total: i64 = entries.iter().filter_map(|e| e["weight"].as_i64()).sum();
                    if total <= 0 {
                        return 0;
                    }
                    let mut pick = rng.random_range(0..total);
                    for entry in &entries {
                        let weight = entry["weight"].as_i64().unwrap_or(0);
                        if pick < weight {
                            return sample_int(&entry["data"], rng);
                        }
                        pick -= weight;
                    }
                    0
                }
                // `uniform` and the bare `{min_inclusive, max_inclusive}`.
                _ => {
                    let min = o.get("min_inclusive").and_then(Value::as_i64).unwrap_or(0) as i32;
                    let max = o.get("max_inclusive").and_then(Value::as_i64).unwrap_or(0) as i32;
                    if max <= min {
                        min
                    } else {
                        rng.random_range(min..=max)
                    }
                }
            }
        }
        _ => 0,
    }
}

/// A block state written as `{"id", "properties"}`.
pub fn parse_state(data: &GameData, value: &Value) -> Option<u32> {
    let id = value.get("id").or_else(|| value.get("Name"))?.as_str()?;
    let block = data.block_named(id)?;
    let mut state = block.default;
    if let Some(properties) = value
        .get("properties")
        .or_else(|| value.get("Properties"))
        .and_then(Value::as_object)
    {
        for (key, v) in properties {
            if let Some(v) = v.as_str() {
                state = block.with(state, key, v).unwrap_or(state);
            }
        }
    }
    Some(state)
}

/// A `BlockPredicate` at `pos`: block tags and names, with an offset, and
/// `not`, `all_of`, `any_of`, `true`.
pub fn test_predicate(
    data: &GameData,
    blocks: &dyn Blocks,
    predicate: &Value,
    pos: BlockPos,
) -> bool {
    let at = |p: &Value| {
        let offset = p.get("offset").and_then(Value::as_array);
        let o = |i: usize| {
            offset
                .and_then(|o| o.get(i))
                .and_then(Value::as_i64)
                .unwrap_or(0) as i32
        };
        BlockPos {
            x: pos.x + o(0),
            y: pos.y + o(1),
            z: pos.z + o(2),
        }
    };
    match predicate["type"].as_str().unwrap_or("") {
        "minecraft:matching_block_tag" => {
            let tag = predicate["tag"].as_str().unwrap_or("");
            data.state_in_tag(blocks.block(at(predicate)), tag)
        }
        "minecraft:matching_blocks" => {
            let state = blocks.block(at(predicate));
            let Some((_, block)) = data.block_of(state) else {
                return false;
            };
            match &predicate["blocks"] {
                Value::String(name) => match name.strip_prefix('#') {
                    Some(tag) => data.state_in_tag(state, tag),
                    None => *name == block.name,
                },
                Value::Array(names) => names.iter().any(|n| n.as_str() == Some(&block.name)),
                _ => false,
            }
        }
        "minecraft:not" => !test_predicate(data, blocks, &predicate["predicate"], pos),
        "minecraft:all_of" => predicate["predicates"]
            .as_array()
            .is_some_and(|ps| ps.iter().all(|p| test_predicate(data, blocks, p, pos))),
        "minecraft:any_of" => predicate["predicates"]
            .as_array()
            .is_some_and(|ps| ps.iter().any(|p| test_predicate(data, blocks, p, pos))),
        "minecraft:true" => true,
        _ => false,
    }
}

/// A `BlockStateProvider` at `pos`: a state, a reference to a named
/// provider, `rule_based` (the first rule that holds) or
/// `copy_properties` (the source with the properties of the block there).
/// `None` if no rule holds.
pub fn provide(
    data: &GameData,
    blocks: &dyn Blocks,
    provider: &Value,
    pos: BlockPos,
    rng: &mut impl Rng,
) -> Option<u32> {
    match provider {
        Value::String(name) => {
            let named = data.features.block_state_providers.get(name)?;
            provide(data, blocks, named, pos, rng)
        }
        Value::Object(o) if o.contains_key("id") => parse_state(data, provider),
        Value::Object(o) => match o.get("type").and_then(Value::as_str)? {
            "minecraft:simple_state_provider" => parse_state(data, &provider["state"]),
            "minecraft:rule_based" => {
                for rule in provider["rules"].as_array()? {
                    if test_predicate(data, blocks, &rule["if_true"], pos) {
                        return provide(data, blocks, &rule["then"], pos, rng);
                    }
                }
                match provider.get("fallback") {
                    Some(fallback) => provide(data, blocks, fallback, pos, rng),
                    None => None,
                }
            }
            "minecraft:copy_properties" => {
                let target = provide(data, blocks, &provider["source"], pos, rng)?;
                let current = blocks.block(pos);
                let (_, from) = data.block_of(current)?;
                let (_, to) = data.block_of(target)?;
                let mut state = target;
                for (key, value) in from.values(current) {
                    state = to.with(state, key, value).unwrap_or(state);
                }
                Some(state)
            }
            "minecraft:weighted_state_provider" => {
                let entries = provider["entries"].as_array()?;
                let total: i64 = entries.iter().filter_map(|e| e["weight"].as_i64()).sum();
                let mut pick = rng.random_range(0..total.max(1));
                for entry in entries {
                    let weight = entry["weight"].as_i64().unwrap_or(0);
                    if pick < weight {
                        return parse_state(data, &entry["data"]);
                    }
                    pick -= weight;
                }
                None
            }
            _ => None,
        },
        _ => None,
    }
}

/// `FoliagePlacer.FoliageAttachment`.
#[derive(Debug, Clone, Copy)]
struct Attachment {
    pos: BlockPos,
    radius_offset: i32,
    double_trunk: bool,
}

const HORIZONTAL: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];

fn add(pos: BlockPos, dx: i32, dy: i32, dz: i32) -> BlockPos {
    BlockPos {
        x: pos.x + dx,
        y: pos.y + dy,
        z: pos.z + dz,
    }
}

/// The world with what a planned tree set so far on top.
struct View<'a> {
    world: &'a dyn Blocks,
    placed: HashMap<BlockPos, u32>,
}

impl Blocks for View<'_> {
    fn block(&self, pos: BlockPos) -> u32 {
        self.placed
            .get(&pos)
            .copied()
            .unwrap_or_else(|| self.world.block(pos))
    }
}

/// A tree being planned.
struct Builder<'a, R: Rng> {
    data: &'a GameData,
    view: View<'a>,
    rng: &'a mut R,
    feature: &'a Value,
    order: Vec<BlockPos>,
    trunks: Vec<BlockPos>,
    foliage: HashSet<BlockPos>,
}

impl<R: Rng> Builder<'_, R> {
    fn set(&mut self, pos: BlockPos, state: u32) {
        if self.view.placed.insert(pos, state).is_none() {
            self.order.push(pos);
        }
    }

    fn in_tag(&self, pos: BlockPos, tag: &str) -> bool {
        self.data.state_in_tag(self.view.block(pos), tag)
    }

    /// `TreeFeature.validTreePos`.
    fn valid_tree_pos(&self, pos: BlockPos) -> bool {
        self.in_tag(pos, "minecraft:air") || self.in_tag(pos, "minecraft:replaceable_by_trees")
    }

    /// `TrunkPlacer.isFree`.
    fn is_free(&self, pos: BlockPos) -> bool {
        self.valid_tree_pos(pos) || self.in_tag(pos, "minecraft:logs")
    }

    fn place_below_trunk(&mut self, pos: BlockPos) {
        let provider = &self.feature["below_trunk_provider"];
        if let Some(state) = provide(self.data, &self.view, provider, pos, self.rng) {
            self.set(pos, state);
            self.trunks.push(pos);
        }
    }

    /// `TrunkPlacer.placeLog`, the log turned along `axis` if given.
    fn place_log(&mut self, pos: BlockPos, axis: Option<&str>) -> bool {
        if !self.valid_tree_pos(pos) {
            return false;
        }
        let provider = &self.feature["trunk_provider"];
        let Some(mut state) = provide(self.data, &self.view, provider, pos, self.rng) else {
            return false;
        };
        if let Some(axis) = axis
            && let Some((_, block)) = self.data.block_of(state)
        {
            state = block.with(state, "axis", axis).unwrap_or(state);
        }
        self.set(pos, state);
        self.trunks.push(pos);
        true
    }

    /// `FoliagePlacer.tryPlaceLeaf`.
    fn place_leaf(&mut self, pos: BlockPos) -> bool {
        let current = self.view.block(pos);
        let persistent = self
            .data
            .block_of(current)
            .is_some_and(|(_, b)| b.value(current, "persistent") == Some("true"));
        if persistent || !self.valid_tree_pos(pos) {
            return false;
        }
        let provider = &self.feature["foliage_provider"];
        let Some(mut state) = provide(self.data, &self.view, provider, pos, self.rng) else {
            return false;
        };
        if let Some((_, block)) = self.data.block_of(state)
            && block.has("waterlogged")
        {
            // A water source, or water held by a waterlogged block.
            let water = current == block_first(self.data, "minecraft:water")
                || self
                    .data
                    .block_of(current)
                    .is_some_and(|(_, b)| b.value(current, "waterlogged") == Some("true"));
            state = block
                .with(state, "waterlogged", if water { "true" } else { "false" })
                .unwrap_or(state);
        }
        self.set(pos, state);
        self.foliage.insert(pos);
        true
    }

    fn trunk_type(&self) -> &str {
        self.feature["trunk_placer"]["type"].as_str().unwrap_or("")
    }

    fn foliage_type(&self) -> &str {
        self.feature["foliage_placer"]["type"]
            .as_str()
            .unwrap_or("")
    }

    /// `TrunkPlacer.getTreeHeight`.
    fn tree_height(&mut self) -> i32 {
        let t = &self.feature["trunk_placer"];
        let base = t["base_height"].as_i64().unwrap_or(4) as i32;
        let a = t["height_rand_a"].as_i64().unwrap_or(0) as i32;
        let b = t["height_rand_b"].as_i64().unwrap_or(0) as i32;
        base + self.rng.random_range(0..=a) + self.rng.random_range(0..=b)
    }

    fn foliage_int(&mut self, key: &str) -> i32 {
        let value = self.feature["foliage_placer"][key].clone();
        sample_int(&value, self.rng)
    }

    /// `FoliagePlacer.foliageHeight`.
    fn foliage_height(&mut self, tree_height: i32) -> i32 {
        match self.foliage_type() {
            "minecraft:blob_foliage_placer" | "minecraft:fancy_foliage_placer" => {
                self.feature["foliage_placer"]["height"]
                    .as_i64()
                    .unwrap_or(3) as i32
            }
            "minecraft:spruce_foliage_placer" => {
                (tree_height - self.foliage_int("trunk_height")).max(4)
            }
            "minecraft:acacia_foliage_placer" => 0,
            "minecraft:dark_oak_foliage_placer" => 4,
            "minecraft:cherry_foliage_placer" => self.foliage_int("height"),
            _ => 0,
        }
    }

    /// `FeatureSize.getSizeAtHeight`.
    fn size_at(&self, tree_height: i32, y: i32) -> i32 {
        let size = &self.feature["minimum_size"];
        let int = |key: &str, default: i64| size[key].as_i64().unwrap_or(default) as i32;
        match size["type"].as_str().unwrap_or("") {
            "minecraft:three_layers_feature_size" => {
                if y < int("limit", 1) {
                    int("lower_size", 0)
                } else if y >= tree_height - int("upper_limit", 1) {
                    int("upper_size", 1)
                } else {
                    int("middle_size", 1)
                }
            }
            _ => {
                if y < int("limit", 1) {
                    int("lower_size", 0)
                } else {
                    int("upper_size", 1)
                }
            }
        }
    }

    /// `TreeFeature.getMaxFreeTreeHeight`.
    fn max_free_height(&self, tree_height: i32, origin: BlockPos) -> i32 {
        let ignore_vines = self.feature["ignore_vines"].as_bool().unwrap_or(false);
        for y in 0..=tree_height + 1 {
            let r = self.size_at(tree_height, y);
            for x in -r..=r {
                for z in -r..=r {
                    let pos = add(origin, x, y, z);
                    let vine =
                        !ignore_vines && self.data.state_is(self.view.block(pos), "minecraft:vine");
                    if !self.is_free(pos) || vine {
                        return y - 2;
                    }
                }
            }
        }
        tree_height
    }

    /// `TreeFeature.doPlace`; `false` if there is no room or a placer is
    /// unknown.
    fn place(&mut self, origin: BlockPos) -> bool {
        let known_trunk = matches!(
            self.trunk_type(),
            "minecraft:straight_trunk_placer"
                | "minecraft:forking_trunk_placer"
                | "minecraft:dark_oak_trunk_placer"
                | "minecraft:fancy_trunk_placer"
                | "minecraft:cherry_trunk_placer"
        );
        let known_foliage = matches!(
            self.foliage_type(),
            "minecraft:blob_foliage_placer"
                | "minecraft:fancy_foliage_placer"
                | "minecraft:spruce_foliage_placer"
                | "minecraft:acacia_foliage_placer"
                | "minecraft:dark_oak_foliage_placer"
                | "minecraft:cherry_foliage_placer"
        );
        if !known_trunk || !known_foliage || self.feature.get("root_placer").is_some() {
            return false;
        }
        let tree_height = self.tree_height();
        let foliage_height = self.foliage_height(tree_height);
        let trunk_height = tree_height - foliage_height;
        let _ = trunk_height;
        let leaf_radius = self.foliage_int("radius");
        if origin.y < MIN_Y + 1 || origin.y + tree_height + 1 > MAX_Y + 1 {
            return false;
        }
        let min_clipped = self.feature["minimum_size"]["min_clipped_height"].as_i64();
        let clipped = self.max_free_height(tree_height, origin);
        if clipped < tree_height && !min_clipped.is_some_and(|m| i64::from(clipped) >= m) {
            return false;
        }
        let attachments = self.place_trunk(clipped, origin);
        for attachment in attachments {
            let offset = self.foliage_int("offset");
            self.create_foliage(attachment, foliage_height, leaf_radius, offset);
        }
        !self.trunks.is_empty() || !self.foliage.is_empty()
    }

    fn random_direction(&mut self) -> (i32, i32) {
        HORIZONTAL[self.rng.random_range(0..4)]
    }

    fn place_trunk(&mut self, height: i32, origin: BlockPos) -> Vec<Attachment> {
        let single = |pos, radius_offset| Attachment {
            pos,
            radius_offset,
            double_trunk: false,
        };
        match self.trunk_type() {
            "minecraft:straight_trunk_placer" => {
                self.place_below_trunk(add(origin, 0, -1, 0));
                for y in 0..height {
                    self.place_log(add(origin, 0, y, 0), None);
                }
                vec![single(add(origin, 0, height, 0), 0)]
            }
            "minecraft:forking_trunk_placer" => self.forking_trunk(height, origin),
            "minecraft:dark_oak_trunk_placer" => self.dark_oak_trunk(height, origin),
            "minecraft:fancy_trunk_placer" => self.fancy_trunk(height, origin),
            "minecraft:cherry_trunk_placer" => self.cherry_trunk(height, origin),
            _ => Vec::new(),
        }
    }

    /// `ForkingTrunkPlacer`: acacias lean and fork.
    fn forking_trunk(&mut self, height: i32, origin: BlockPos) -> Vec<Attachment> {
        self.place_below_trunk(add(origin, 0, -1, 0));
        let mut attachments = Vec::new();
        let lean = self.random_direction();
        let lean_height = height - self.rng.random_range(0..4) - 1;
        let mut lean_steps = 3 - self.rng.random_range(0..3);
        let (mut tx, mut tz) = (origin.x, origin.z);
        let mut end = None;
        for yo in 0..height {
            let yy = origin.y + yo;
            if yo >= lean_height && lean_steps > 0 {
                tx += lean.0;
                tz += lean.1;
                lean_steps -= 1;
            }
            if self.place_log(
                BlockPos {
                    x: tx,
                    y: yy,
                    z: tz,
                },
                None,
            ) {
                end = Some(yy + 1);
            }
        }
        if let Some(y) = end {
            attachments.push(Attachment {
                pos: BlockPos { x: tx, y, z: tz },
                radius_offset: 1,
                double_trunk: false,
            });
        }
        let (mut tx, mut tz) = (origin.x, origin.z);
        let branch = self.random_direction();
        if branch != lean {
            let branch_pos = lean_height - self.rng.random_range(0..2) - 1;
            let mut steps = 1 + self.rng.random_range(0..3);
            let mut end = None;
            let mut yo = branch_pos;
            while yo < height && steps > 0 {
                if yo >= 1 {
                    let yy = origin.y + yo;
                    tx += branch.0;
                    tz += branch.1;
                    if self.place_log(
                        BlockPos {
                            x: tx,
                            y: yy,
                            z: tz,
                        },
                        None,
                    ) {
                        end = Some(yy + 1);
                    }
                }
                yo += 1;
                steps -= 1;
            }
            if let Some(y) = end {
                attachments.push(Attachment {
                    pos: BlockPos { x: tx, y, z: tz },
                    radius_offset: 0,
                    double_trunk: false,
                });
            }
        }
        attachments
    }

    /// `DarkOakTrunkPlacer`: two by two logs, with short branches around.
    fn dark_oak_trunk(&mut self, height: i32, origin: BlockPos) -> Vec<Attachment> {
        let mut attachments = Vec::new();
        let below = add(origin, 0, -1, 0);
        for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            self.place_below_trunk(add(below, dx, 0, dz));
        }
        let lean = self.random_direction();
        let lean_height = height - self.rng.random_range(0..4);
        let mut lean_steps = 2 - self.rng.random_range(0..3);
        let (x, y, z) = (origin.x, origin.y, origin.z);
        let (mut tx, mut tz) = (x, z);
        let ey = y + height - 1;
        for dy in 0..height {
            if dy >= lean_height && lean_steps > 0 {
                tx += lean.0;
                tz += lean.1;
                lean_steps -= 1;
            }
            let pos = BlockPos {
                x: tx,
                y: y + dy,
                z: tz,
            };
            if self.in_tag(pos, "minecraft:air") || self.in_tag(pos, "minecraft:leaves") {
                for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    self.place_log(add(pos, dx, 0, dz), None);
                }
            }
        }
        attachments.push(Attachment {
            pos: BlockPos {
                x: tx,
                y: ey,
                z: tz,
            },
            radius_offset: 0,
            double_trunk: true,
        });
        for ox in -1..=2 {
            for oz in -1..=2 {
                if (!(0..=1).contains(&ox) || !(0..=1).contains(&oz))
                    && self.rng.random_range(0..3) == 0
                {
                    let length = self.rng.random_range(0..3) + 2;
                    for branch_y in 0..length {
                        self.place_log(
                            BlockPos {
                                x: x + ox,
                                y: ey - branch_y - 1,
                                z: z + oz,
                            },
                            None,
                        );
                    }
                    attachments.push(Attachment {
                        pos: BlockPos {
                            x: x + ox,
                            y: ey,
                            z: z + oz,
                        },
                        radius_offset: 0,
                        double_trunk: false,
                    });
                }
            }
        }
        attachments
    }

    /// `FancyTrunkPlacer`: the big oak with branches.
    fn fancy_trunk(&mut self, tree_height: i32, origin: BlockPos) -> Vec<Attachment> {
        let height = tree_height + 2;
        let trunk_height = (f64::from(height) * 0.618).floor() as i32;
        self.place_below_trunk(add(origin, 0, -1, 0));
        let clusters_per_y = 1.min((1.382 + (f64::from(height) / 13.0).powi(2)).floor() as i32);
        let trunk_top = origin.y + trunk_height;
        let mut relative_y = height - 5;
        // (attachment position, branch base y)
        let mut coords = vec![(add(origin, 0, relative_y, 0), trunk_top)];
        while relative_y >= 0 {
            let shape = tree_shape(height, relative_y);
            if shape >= 0.0 {
                for _ in 0..clusters_per_y {
                    let radius = f64::from(shape) * (f64::from(self.rng.random::<f32>()) + 0.328);
                    let angle = f64::from(self.rng.random::<f32>() * 2.0) * std::f64::consts::PI;
                    let x = radius * angle.sin() + 0.5;
                    let z = radius * angle.cos() + 0.5;
                    let check_start =
                        add(origin, x.floor() as i32, relative_y - 1, z.floor() as i32);
                    let check_end = add(check_start, 0, 5, 0);
                    if self.make_limb(check_start, check_end, false) {
                        let dx = origin.x - check_start.x;
                        let dz = origin.z - check_start.z;
                        let branch_height =
                            f64::from(check_start.y) - f64::from(dx * dx + dz * dz).sqrt() * 0.381;
                        let branch_top = if branch_height > f64::from(trunk_top) {
                            trunk_top
                        } else {
                            branch_height as i32
                        };
                        let base = BlockPos {
                            x: origin.x,
                            y: branch_top,
                            z: origin.z,
                        };
                        if self.make_limb(base, check_start, false) {
                            coords.push((check_start, base.y));
                        }
                    }
                }
            }
            relative_y -= 1;
        }
        self.make_limb(origin, add(origin, 0, trunk_height, 0), true);
        // `makeBranches`.
        let trims = |base_y: i32| f64::from(base_y - origin.y) >= f64::from(height) * 0.2;
        for &(end, base_y) in &coords {
            let base = BlockPos {
                x: origin.x,
                y: base_y,
                z: origin.z,
            };
            if base != end && trims(base_y) {
                self.make_limb(base, end, true);
            }
        }
        coords
            .into_iter()
            .filter(|&(_, base_y)| trims(base_y))
            .map(|(pos, _)| Attachment {
                pos,
                radius_offset: 0,
                double_trunk: false,
            })
            .collect()
    }

    /// `FancyTrunkPlacer.makeLimb`: a line of logs, or only a check that
    /// it is free.
    fn make_limb(&mut self, start: BlockPos, end: BlockPos, place: bool) -> bool {
        if !place && start == end {
            return true;
        }
        let delta = [end.x - start.x, end.y - start.y, end.z - start.z];
        let steps = delta.iter().map(|d| d.abs()).max().unwrap_or(0).max(1);
        let d = delta.map(|v| v as f32 / steps as f32);
        for i in 0..=steps {
            let at = add(
                start,
                (0.5 + i as f32 * d[0]).floor() as i32,
                (0.5 + i as f32 * d[1]).floor() as i32,
                (0.5 + i as f32 * d[2]).floor() as i32,
            );
            if place {
                let (xd, zd) = ((at.x - start.x).abs(), (at.z - start.z).abs());
                let axis = match xd.max(zd) {
                    0 => "y",
                    m if xd == m => "x",
                    _ => "z",
                };
                self.place_log(at, Some(axis));
            } else if !self.is_free(at) {
                return false;
            }
        }
        true
    }

    /// `CherryTrunkPlacer`: a trunk that forks into one to three
    /// branches.
    fn cherry_trunk(&mut self, tree_height: i32, origin: BlockPos) -> Vec<Attachment> {
        let t = self.feature["trunk_placer"].clone();
        self.place_below_trunk(add(origin, 0, -1, 0));
        let start = &t["branch_start_offset_from_top"];
        let (min, max) = (
            start["min_inclusive"].as_i64().unwrap_or(-4) as i32,
            start["max_inclusive"].as_i64().unwrap_or(-3) as i32,
        );
        let first = (tree_height - 1 + self.rng.random_range(min..=max)).max(0);
        let mut second = (tree_height - 1 + self.rng.random_range(min..=(max - 1).max(min))).max(0);
        if second >= first {
            second += 1;
        }
        let count = sample_int(&t["branch_count"], self.rng);
        let middle = count == 3;
        let both = count >= 2;
        let trunk_height = if middle {
            tree_height
        } else if both {
            first.max(second) + 1
        } else {
            first + 1
        };
        for y in 0..trunk_height {
            self.place_log(add(origin, 0, y, 0), None);
        }
        let mut attachments = Vec::new();
        if middle {
            attachments.push(Attachment {
                pos: add(origin, 0, trunk_height, 0),
                radius_offset: 0,
                double_trunk: false,
            });
        }
        let direction = self.random_direction();
        let sideways = if direction.0 != 0 { "x" } else { "z" };
        attachments.push(self.cherry_branch(
            &t,
            tree_height,
            origin,
            sideways,
            direction,
            first,
            first < trunk_height - 1,
        ));
        if both {
            attachments.push(self.cherry_branch(
                &t,
                tree_height,
                origin,
                sideways,
                (-direction.0, -direction.1),
                second,
                second < trunk_height - 1,
            ));
        }
        attachments
    }

    #[allow(clippy::too_many_arguments)]
    fn cherry_branch(
        &mut self,
        t: &Value,
        tree_height: i32,
        origin: BlockPos,
        sideways: &str,
        direction: (i32, i32),
        offset: i32,
        middle_continues: bool,
    ) -> Attachment {
        let mut log = add(origin, 0, offset, 0);
        let end_y = tree_height - 1 + sample_int(&t["branch_end_offset_from_top"], self.rng);
        let away = middle_continues || end_y < offset;
        let distance = sample_int(&t["branch_horizontal_length"], self.rng) + i32::from(away);
        let end = add(
            origin,
            direction.0 * distance,
            end_y,
            direction.1 * distance,
        );
        for _ in 0..if away { 2 } else { 1 } {
            log = add(log, direction.0, 0, direction.1);
            self.place_log(log, Some(sideways));
        }
        let vertical = if end.y > log.y { 1 } else { -1 };
        loop {
            let distance = (log.x - end.x).abs() + (log.y - end.y).abs() + (log.z - end.z).abs();
            if distance == 0 {
                return Attachment {
                    pos: add(end, 0, 1, 0),
                    radius_offset: 0,
                    double_trunk: false,
                };
            }
            let chance = (end.y - log.y).abs() as f32 / distance as f32;
            let up = self.rng.random::<f32>() < chance;
            log = if up {
                add(log, 0, vertical, 0)
            } else {
                add(log, direction.0, 0, direction.1)
            };
            self.place_log(log, if up { None } else { Some(sideways) });
        }
    }

    // --- Foliage ---------------------------------------------------------------

    fn create_foliage(
        &mut self,
        a: Attachment,
        foliage_height: i32,
        leaf_radius: i32,
        offset: i32,
    ) {
        match self.foliage_type() {
            "minecraft:blob_foliage_placer" => {
                for yo in (offset - foliage_height..=offset).rev() {
                    let r = (leaf_radius + a.radius_offset - 1 - yo / 2).max(0);
                    self.leaves_row(a.pos, r, yo, a.double_trunk);
                }
            }
            "minecraft:fancy_foliage_placer" => {
                for yo in (offset - foliage_height..=offset).rev() {
                    let edge = yo == offset || yo == offset - foliage_height;
                    let r = leaf_radius + if edge { 0 } else { 1 };
                    self.leaves_row(a.pos, r, yo, a.double_trunk);
                }
            }
            "minecraft:spruce_foliage_placer" => {
                let mut current = self.rng.random_range(0..2);
                let (mut max, mut min) = (1, 0);
                for yo in (-foliage_height..=offset).rev() {
                    self.leaves_row(a.pos, current, yo, a.double_trunk);
                    if current >= max {
                        current = min;
                        min = 1;
                        max = (max + 1).min(leaf_radius + a.radius_offset);
                    } else {
                        current += 1;
                    }
                }
            }
            "minecraft:acacia_foliage_placer" => {
                let pos = add(a.pos, 0, offset, 0);
                let fh = foliage_height;
                self.leaves_row(pos, leaf_radius + a.radius_offset, -1 - fh, a.double_trunk);
                self.leaves_row(pos, leaf_radius - 1, -fh, a.double_trunk);
                self.leaves_row(pos, leaf_radius + a.radius_offset - 1, 0, a.double_trunk);
            }
            "minecraft:dark_oak_foliage_placer" => {
                let pos = add(a.pos, 0, offset, 0);
                if a.double_trunk {
                    self.leaves_row(pos, leaf_radius + 2, -1, true);
                    self.leaves_row(pos, leaf_radius + 3, 0, true);
                    self.leaves_row(pos, leaf_radius + 2, 1, true);
                    if self.rng.random::<bool>() {
                        self.leaves_row(pos, leaf_radius, 2, true);
                    }
                } else {
                    self.leaves_row(pos, leaf_radius + 2, -1, false);
                    self.leaves_row(pos, leaf_radius + 1, 0, false);
                }
            }
            "minecraft:cherry_foliage_placer" => {
                let pos = add(a.pos, 0, offset, 0);
                let r = leaf_radius + a.radius_offset - 1;
                let fh = foliage_height;
                self.leaves_row(pos, r - 2, fh - 3, a.double_trunk);
                self.leaves_row(pos, r - 1, fh - 4, a.double_trunk);
                for y in (0..=fh - 5).rev() {
                    self.leaves_row(pos, r, y, a.double_trunk);
                }
                self.leaves_row_hanging(pos, r, -1, a.double_trunk);
                self.leaves_row_hanging(pos, r - 1, -2, a.double_trunk);
            }
            _ => {}
        }
    }

    fn foliage_float(&self, key: &str) -> f32 {
        self.feature["foliage_placer"][key].as_f64().unwrap_or(0.0) as f32
    }

    /// `shouldSkipLocation` of the foliage placer.
    fn skip(&mut self, dx: i32, y: i32, dz: i32, r: i32, double: bool) -> bool {
        match self.foliage_type() {
            "minecraft:blob_foliage_placer" => {
                dx == r && dz == r && (self.rng.random_range(0..2) == 0 || y == 0)
            }
            "minecraft:fancy_foliage_placer" => {
                (dx as f32 + 0.5).powi(2) + (dz as f32 + 0.5).powi(2) > (r * r) as f32
            }
            "minecraft:spruce_foliage_placer" => dx == r && dz == r && r > 0,
            "minecraft:acacia_foliage_placer" => {
                if y == 0 {
                    (dx > 1 || dz > 1) && dx != 0 && dz != 0
                } else {
                    dx == r && dz == r && r > 0
                }
            }
            "minecraft:dark_oak_foliage_placer" => {
                if y == -1 && !double {
                    dx == r && dz == r
                } else {
                    y == 1 && dx + dz > r * 2 - 2
                }
            }
            "minecraft:cherry_foliage_placer" => {
                if y == -1
                    && (dx == r || dz == r)
                    && self.rng.random::<f32>()
                        < self.foliage_float("wide_bottom_layer_hole_chance")
                {
                    return true;
                }
                let corner = dx == r && dz == r;
                let chance = self.foliage_float("corner_hole_chance");
                if r > 2 {
                    corner || dx + dz > r * 2 - 2 && self.rng.random::<f32>() < chance
                } else {
                    corner && self.rng.random::<f32>() < chance
                }
            }
            _ => false,
        }
    }

    /// `shouldSkipLocationSigned`.
    fn skip_signed(&mut self, dx: i32, y: i32, dz: i32, r: i32, double: bool) -> bool {
        if self.foliage_type() == "minecraft:dark_oak_foliage_placer"
            && y == 0
            && double
            && (dx == -r || dx >= r)
            && (dz == -r || dz >= r)
        {
            return true;
        }
        let (mx, mz) = if double {
            (dx.abs().min((dx - 1).abs()), dz.abs().min((dz - 1).abs()))
        } else {
            (dx.abs(), dz.abs())
        };
        self.skip(mx, y, mz, r, double)
    }

    /// `placeLeavesRow`.
    fn leaves_row(&mut self, origin: BlockPos, r: i32, y: i32, double: bool) {
        let extra = i32::from(double);
        for dx in -r..=r + extra {
            for dz in -r..=r + extra {
                if !self.skip_signed(dx, y, dz, r, double) {
                    self.place_leaf(add(origin, dx, y, dz));
                }
            }
        }
    }

    /// `placeLeavesRowWithHangingLeavesBelow`.
    fn leaves_row_hanging(&mut self, origin: BlockPos, r: i32, y: i32, double: bool) {
        self.leaves_row(origin, r, y, double);
        let chance = self.foliage_float("hanging_leaves_chance");
        let extension = self.foliage_float("hanging_leaves_extension_chance");
        let extra = i32::from(double);
        let log = add(origin, 0, -1, 0);
        // Along each edge: (along, towards the edge).
        for (along, to_edge) in [
            ((0, -1), (1, 0)),
            ((1, 0), (0, 1)),
            ((0, 1), (-1, 0)),
            ((-1, 0), (0, -1)),
        ] {
            let positive = to_edge.0 + to_edge.1 > 0;
            let to = if positive { r + extra } else { r };
            let mut pos = add(
                origin,
                to_edge.0 * to - along.0 * r,
                y - 1,
                to_edge.1 * to - along.1 * r,
            );
            for _ in -r..r + extra {
                let above_set = self.foliage.contains(&add(pos, 0, 1, 0));
                if above_set && self.try_extension(chance, log, pos) {
                    self.try_extension(extension, log, add(pos, 0, -1, 0));
                }
                pos = add(pos, along.0, 0, along.1);
            }
        }
    }

    fn try_extension(&mut self, chance: f32, log: BlockPos, pos: BlockPos) -> bool {
        let manhattan = (pos.x - log.x).abs() + (pos.y - log.y).abs() + (pos.z - log.z).abs();
        if manhattan >= 7 || self.rng.random::<f32>() > chance {
            return false;
        }
        self.place_leaf(pos)
    }

    /// `TreeFeature.updateLeaves`: leaves get their distance to the
    /// nearest log.
    fn update_leaves(&mut self) {
        let mut distance: HashMap<BlockPos, i32> = HashMap::new();
        let mut queue: VecDeque<BlockPos> = VecDeque::new();
        for &log in &self.trunks {
            distance.insert(log, 0);
            queue.push_back(log);
        }
        while let Some(pos) = queue.pop_front() {
            let d = distance[&pos];
            if d + 1 >= 7 {
                continue;
            }
            for (dx, dy, dz) in [
                (0, -1, 0),
                (0, 1, 0),
                (0, 0, -1),
                (0, 0, 1),
                (-1, 0, 0),
                (1, 0, 0),
            ] {
                let next = add(pos, dx, dy, dz);
                if !self.foliage.contains(&next) || distance.contains_key(&next) {
                    continue;
                }
                distance.insert(next, d + 1);
                queue.push_back(next);
                let state = self.view.placed[&next];
                if let Some((_, block)) = self.data.block_of(state)
                    && let Some(set) = block.with(state, "distance", &(d + 1).to_string())
                {
                    self.view.placed.insert(next, set);
                }
            }
        }
    }
}

fn block_first(data: &GameData, name: &str) -> u32 {
    data.block_named(name).map_or(u32::MAX, |b| b.first)
}

/// `FancyTrunkPlacer.treeShape`.
fn tree_shape(height: i32, y: i32) -> f32 {
    if (y as f32) < height as f32 * 0.3 {
        return -1.0;
    }
    let radius = height as f32 / 2.0;
    let adjacent = radius - y as f32;
    let mut distance = (radius * radius - adjacent * adjacent).sqrt();
    if adjacent == 0.0 {
        distance = radius;
    } else if adjacent.abs() >= radius {
        return 0.0;
    }
    distance * 0.5
}

/// Plans the tree feature `name` growing at `origin`; `None` if it does not
/// fit or Ferrocraft does not know how it grows.
pub fn grow(
    data: &GameData,
    world: &dyn Blocks,
    name: &str,
    origin: BlockPos,
    rng: &mut impl Rng,
) -> Option<Plan> {
    let feature = data.features.trees.get(name)?;
    let mut builder = Builder {
        data,
        view: View {
            world,
            placed: HashMap::new(),
        },
        rng,
        feature,
        order: Vec::new(),
        trunks: Vec::new(),
        foliage: HashSet::new(),
    };
    if !builder.place(origin) {
        return None;
    }
    builder.update_leaves();
    Some(
        builder
            .order
            .iter()
            .map(|pos| (*pos, builder.view.placed[pos]))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(data: &GameData) -> impl Fn(BlockPos) -> u32 + '_ {
        let grass = data.default_state("minecraft:grass_block").unwrap();
        move |pos: BlockPos| if pos.y < 0 { grass } else { 0 }
    }

    fn count(data: &GameData, plan: &Plan, block: &str) -> usize {
        plan.iter()
            .filter(|(_, s)| data.state_is(*s, block))
            .count()
    }

    #[test]
    fn oaks_grow_like_vanilla() {
        let data = GameData::load().unwrap();
        let world = flat(&data);
        let mut rng = rand::rng();
        for _ in 0..50 {
            let plan = grow(
                &data,
                &world,
                "minecraft:oak",
                BlockPos { x: 0, y: 0, z: 0 },
                &mut rng,
            )
            .expect("room for an oak");
            // A trunk of 4 to 6 logs on dirt, a crown of leaves around its
            // top.
            let logs = count(&data, &plan, "minecraft:oak_log");
            assert!((4..=6).contains(&logs), "{logs} logs");
            assert_eq!(count(&data, &plan, "minecraft:dirt"), 1);
            let leaves = count(&data, &plan, "minecraft:oak_leaves");
            assert!((50..=70).contains(&leaves), "{leaves} leaves");
            // Every leaf is close enough to a log not to decay.
            for (_, state) in &plan {
                let (_, block) = data.block_of(*state).unwrap();
                if block.name == "minecraft:oak_leaves" {
                    let d: i32 = block.value(*state, "distance").unwrap().parse().unwrap();
                    assert!(d < 7);
                }
            }
        }
    }

    #[test]
    fn every_sapling_tree_but_the_odd_ones_grows() {
        let data = GameData::load().unwrap();
        let world = flat(&data);
        let mut rng = rand::rng();
        for name in [
            "minecraft:oak",
            "minecraft:fancy_oak",
            "minecraft:birch",
            "minecraft:spruce",
            "minecraft:jungle_tree_no_vine",
            "minecraft:acacia",
            "minecraft:cherry",
            "minecraft:dark_oak",
            "minecraft:pale_oak_bonemeal",
        ] {
            let plan = grow(&data, &world, name, BlockPos { x: 0, y: 0, z: 0 }, &mut rng);
            let plan = plan.unwrap_or_else(|| panic!("{name} did not grow"));
            assert!(plan.len() > 10, "{name}: {} blocks", plan.len());
        }
        assert!(
            grow(
                &data,
                &world,
                "minecraft:mega_spruce",
                BlockPos { x: 0, y: 0, z: 0 },
                &mut rng
            )
            .is_none()
        );
    }

    #[test]
    fn no_tree_under_a_roof() {
        let data = GameData::load().unwrap();
        let stone = data.default_state("minecraft:stone").unwrap();
        let grass = data.default_state("minecraft:grass_block").unwrap();
        let world = move |pos: BlockPos| match pos.y {
            ..0 => grass,
            3 => stone,
            _ => 0,
        };
        let plan = grow(
            &data,
            &world,
            "minecraft:oak",
            BlockPos { x: 0, y: 0, z: 0 },
            &mut rand::rng(),
        );
        assert!(plan.is_none());
    }

    #[test]
    fn transformers_till_and_strip() {
        let data = GameData::load().unwrap();
        let grass = data.default_state("minecraft:grass_block").unwrap();
        let world = move |pos: BlockPos| if pos.y == 0 { grass } else { 0 };
        let hoe = &data.features.block_transformers["minecraft:hoe"];
        let rule = &hoe[0]["block_state_provider"];
        let at = BlockPos { x: 0, y: 0, z: 0 };
        let farmland = provide(&data, &world, rule, at, &mut rand::rng()).unwrap();
        assert!(data.state_is(farmland, "minecraft:farmland"));
        // Not with something on top.
        let covered = move |pos: BlockPos| if pos.y <= 1 { grass } else { 0 };
        assert!(provide(&data, &covered, rule, at, &mut rand::rng()).is_none());

        let log = data.block_named("minecraft:oak_log").unwrap();
        let sideways = log.with(log.default, "axis", "x").unwrap();
        let world = move |_| sideways;
        let axe = &data.features.block_transformers["minecraft:axe"];
        let stripped = axe
            .as_array()
            .unwrap()
            .iter()
            .find_map(|t| {
                provide(
                    &data,
                    &world,
                    &t["block_state_provider"],
                    at,
                    &mut rand::rng(),
                )
            })
            .unwrap();
        let (_, block) = data.block_of(stripped).unwrap();
        assert_eq!(block.name, "minecraft:stripped_oak_log");
        assert_eq!(block.value(stripped, "axis"), Some("x"));
    }
}
