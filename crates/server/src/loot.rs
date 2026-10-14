//! What a broken block or a killed mob drops: vanilla's loot tables
//! (`data/minecraft/loot_table/blocks` and `entities`), evaluated like
//! `LootTable.getRandomItems`.
//! Enchantments and explosions do not exist here, so silk touch never
//! applies, fortune is level 0 and everything survives.

use std::collections::HashMap;

use anyhow::Context as _;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{ComponentPatch, ItemStack};
use rand::Rng;
use serde::Deserialize;
use serde_json::Value;

use crate::data::{Block, BlockSet, GameData};

/// One or several, as vanilla's codecs accept both.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    fn iter(&self) -> std::slice::Iter<'_, T> {
        match self {
            Self::One(one) => std::slice::from_ref(one).iter(),
            Self::Many(many) => many.iter(),
        }
    }
}

impl<T> Default for OneOrMany<T> {
    fn default() -> Self {
        Self::Many(Vec::new())
    }
}

/// A predicate, inline or by name (`minecraft:tool/can_silk_touch`).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Condition {
    Named(String),
    Inline(Box<Predicate>),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum Predicate {
    #[serde(rename = "minecraft:survives_explosion")]
    SurvivesExplosion,
    #[serde(rename = "minecraft:match_block")]
    MatchBlock(BlockPredicate),
    #[serde(rename = "minecraft:any_of")]
    AnyOf { terms: Vec<Condition> },
    #[serde(rename = "minecraft:all_of")]
    AllOf { terms: Vec<Condition> },
    #[serde(rename = "minecraft:inverted")]
    Inverted { term: Condition },
    /// The chance by enchantment level, here always level 0.
    #[serde(rename = "minecraft:table_bonus")]
    TableBonus { chances: Vec<f32> },
    #[serde(rename = "minecraft:random_chance")]
    RandomChance { chance: f32 },
    #[serde(rename = "minecraft:match_tool")]
    MatchTool { predicate: ToolPredicate },
    /// A block near the broken one, e.g. the upper half of a tall plant.
    #[serde(rename = "minecraft:location_check")]
    LocationCheck {
        #[serde(default, rename = "offsetX")]
        offset_x: i32,
        #[serde(default, rename = "offsetY")]
        offset_y: i32,
        #[serde(default, rename = "offsetZ")]
        offset_z: i32,
        predicate: LocationPredicate,
    },
    /// What an entity of the context is like: whether there is one at all,
    /// a sheep's colour, flags such as burning.
    #[serde(rename = "minecraft:entity_properties")]
    EntityProperties {
        #[serde(default)]
        entity: String,
        #[serde(default)]
        predicate: Value,
    },
    #[serde(rename = "minecraft:killed_by_player")]
    KilledByPlayer,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct BlockPredicate {
    blocks: Option<BlockSet>,
    /// Property values the state must have; ranges count as not matching.
    #[serde(default)]
    state: HashMap<String, Value>,
}

#[derive(Debug, Deserialize)]
struct LocationPredicate {
    block: Option<BlockPredicate>,
}

#[derive(Debug, Deserialize)]
struct ToolPredicate {
    /// An item, items or `#tag`.
    items: Option<OneOrMany<String>>,
    /// Component predicates such as enchantments, which no item has here.
    predicates: Option<Value>,
}

/// A number provider.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Number {
    Constant(f32),
    Provider(Provider),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum Provider {
    #[serde(rename = "minecraft:constant")]
    Constant { value: f32 },
    #[serde(rename = "minecraft:uniform")]
    Uniform { min: f32, max: f32 },
    #[serde(rename = "minecraft:binomial")]
    Binomial { n: i32, p: f32 },
}

impl Number {
    fn float(&self, rng: &mut impl Rng) -> f32 {
        match self {
            Self::Constant(v) | Self::Provider(Provider::Constant { value: v }) => *v,
            Self::Provider(Provider::Uniform { min, max }) => {
                if max > min {
                    rng.random_range(*min..*max)
                } else {
                    *min
                }
            }
            Self::Provider(Provider::Binomial { n, p }) => binomial(*n, *p, rng) as f32,
        }
    }

    /// `NumberProvider.getInt`: uniform rounds its bounds, constants round.
    fn int(&self, rng: &mut impl Rng) -> i32 {
        match self {
            Self::Provider(Provider::Uniform { min, max }) => {
                let (min, max) = (min.round() as i32, max.round() as i32);
                if max > min {
                    rng.random_range(min..=max)
                } else {
                    min
                }
            }
            other => other.float(rng).round() as i32,
        }
    }
}

fn binomial(n: i32, p: f32, rng: &mut impl Rng) -> i32 {
    (0..n).filter(|_| rng.random::<f32>() < p).count() as i32
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum Function {
    #[serde(rename = "minecraft:set_count")]
    SetCount {
        count: Number,
        #[serde(default)]
        add: bool,
        condition: Option<Condition>,
    },
    #[serde(rename = "minecraft:limit_count")]
    LimitCount { limit: Limit },
    /// Fortune's formulas; only the one with a base chance gives anything
    /// at level 0.
    #[serde(rename = "minecraft:apply_bonus")]
    ApplyBonus {
        formula: String,
        parameters: Option<BonusParameters>,
        condition: Option<Condition>,
    },
    /// Explosion decay, copying components or state into the item: nothing
    /// that changes the drop here.
    #[serde(other)]
    Ignored,
}

#[derive(Debug, Deserialize)]
struct Limit {
    min: Option<i32>,
    max: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct BonusParameters {
    #[serde(default)]
    extra: i32,
    #[serde(default)]
    probability: f32,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum Entry {
    #[serde(rename = "minecraft:item")]
    Item {
        name: String,
        condition: Option<Condition>,
        #[serde(default)]
        modifier: OneOrMany<Function>,
        #[serde(default = "one")]
        weight: u32,
    },
    /// The first child whose condition holds.
    #[serde(rename = "minecraft:alternatives")]
    Alternatives {
        children: Vec<Entry>,
        condition: Option<Condition>,
    },
    /// The drops of another table, e.g. a sheep's wool by colour.
    #[serde(rename = "minecraft:loot_table")]
    Reference {
        value: String,
        condition: Option<Condition>,
        #[serde(default = "one")]
        weight: u32,
    },
    /// Contents of a block entity, such as a decorated pot's sherds.
    #[serde(other)]
    Unsupported,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Deserialize)]
struct Pool {
    rolls: Number,
    entries: Vec<Entry>,
    condition: Option<Condition>,
    #[serde(default)]
    modifier: OneOrMany<Function>,
}

#[derive(Debug, Deserialize)]
struct Table {
    #[serde(default)]
    pools: Vec<Pool>,
    #[serde(default)]
    modifier: OneOrMany<Function>,
}

/// The tables by name (`minecraft:blocks/stone`, `minecraft:entities/cow`).
#[derive(Debug, Default)]
pub struct LootTables {
    tables: HashMap<String, Table>,
    predicates: HashMap<String, Condition>,
}

#[derive(Deserialize)]
struct Raw {
    tables: HashMap<String, Table>,
    predicates: HashMap<String, Condition>,
}

/// What an entity is like, for `entity_properties`.
#[derive(Debug, Clone, Copy, Default)]
pub struct EntityFacts<'a> {
    pub sheep_color: Option<&'a str>,
    pub sheared: bool,
    pub baby: bool,
}

/// What an entry stands for once its conditions hold.
enum Candidate<'e> {
    Item(&'e str, &'e OneOrMany<Function>),
    Table(&'e str),
}

/// Tables that name each other stop after this many steps.
const MAX_DEPTH: u32 = 8;

impl LootTables {
    /// Parses `loot_tables.json`.
    pub fn load(json: &str) -> anyhow::Result<Self> {
        let raw: Raw = serde_json::from_str(json).context("parsing loot_tables.json")?;
        Ok(Self {
            tables: raw.tables,
            predicates: raw.predicates,
        })
    }
}

/// The table a block drops from (`BlockBehaviour.getLootTable`).
pub fn block_table(block: &Block) -> String {
    format!(
        "minecraft:blocks/{}",
        block.name.trim_start_matches("minecraft:")
    )
}

/// The table an entity type drops from.
pub fn entity_table(entity_type: &str) -> String {
    format!(
        "minecraft:entities/{}",
        entity_type.trim_start_matches("minecraft:")
    )
}

/// What the drops depend on.
pub struct Context<'a> {
    pub data: &'a GameData,
    /// The table to roll, e.g. from [`block_table`].
    pub table: String,
    /// The broken block's state, air for an entity.
    pub state: u32,
    pub pos: BlockPos,
    /// What the breaker or killer holds.
    pub tool: Option<&'a ItemStack>,
    /// The world, for `location_check`.
    pub block_at: &'a dyn Fn(BlockPos) -> u32,
    /// The killed entity.
    pub entity: Option<EntityFacts<'a>>,
    /// A player broke the block or killed the entity.
    pub by_player: bool,
}

impl Context<'_> {
    fn test(&self, condition: Option<&Condition>, rng: &mut impl Rng) -> bool {
        condition.is_none_or(|c| self.holds(c, rng))
    }

    fn holds(&self, condition: &Condition, rng: &mut impl Rng) -> bool {
        let predicate = match condition {
            Condition::Named(name) => {
                return self
                    .data
                    .loot
                    .predicates
                    .get(name)
                    .is_some_and(|c| self.holds(c, rng));
            }
            Condition::Inline(predicate) => predicate,
        };
        match &**predicate {
            Predicate::SurvivesExplosion => true,
            Predicate::KilledByPlayer => self.by_player,
            Predicate::EntityProperties { entity, predicate } => {
                self.entity_matches(entity, predicate)
            }
            Predicate::MatchBlock(block) => self.matches(block, self.state),
            Predicate::AnyOf { terms } => terms.iter().any(|t| self.holds(t, rng)),
            Predicate::AllOf { terms } => terms.iter().all(|t| self.holds(t, rng)),
            Predicate::Inverted { term } => !self.holds(term, rng),
            Predicate::TableBonus { chances } => chances
                .first()
                .is_some_and(|&chance| rng.random::<f32>() < chance),
            Predicate::RandomChance { chance } => rng.random::<f32>() < *chance,
            Predicate::MatchTool { predicate } => self.tool_matches(predicate),
            Predicate::LocationCheck {
                offset_x,
                offset_y,
                offset_z,
                predicate,
            } => {
                let pos = BlockPos {
                    x: self.pos.x + offset_x,
                    y: self.pos.y + offset_y,
                    z: self.pos.z + offset_z,
                };
                predicate
                    .block
                    .as_ref()
                    .is_none_or(|b| self.matches(b, (self.block_at)(pos)))
            }
            Predicate::Unknown => false,
        }
    }

    fn matches(&self, predicate: &BlockPredicate, state: u32) -> bool {
        let Some((id, block)) = self.data.block_of(state) else {
            return false;
        };
        let named = predicate
            .blocks
            .as_ref()
            .is_none_or(|set| self.data.in_block_set(set, id));
        named
            && predicate.state.iter().all(|(key, wanted)| {
                let value = block.value(state, key);
                match wanted {
                    Value::String(wanted) => value == Some(wanted.as_str()),
                    Value::Bool(wanted) => value == Some(if *wanted { "true" } else { "false" }),
                    Value::Number(wanted) => value == Some(wanted.to_string().as_str()),
                    _ => false,
                }
            })
    }

    /// `EntityPredicate` for the parts loot tables use. Other checks, such as
    /// enchantments on the attacker's weapon, never hold here.
    fn entity_matches(&self, entity: &str, predicate: &Value) -> bool {
        let facts = match entity {
            "this" => self.entity,
            // The player behind it: nothing about it is ever asked but its
            // existence and its gear.
            _ if self.by_player => Some(EntityFacts::default()),
            _ => None,
        };
        let Some(facts) = facts else {
            return false;
        };
        let Value::Object(checks) = predicate else {
            return predicate.is_null();
        };
        checks.iter().all(|(key, wanted)| match key.as_str() {
            "minecraft:flags" => wanted.as_object().is_some_and(|flags| {
                flags.iter().all(|(flag, value)| {
                    let actual = match flag.as_str() {
                        "is_baby" => facts.baby,
                        // Nothing burns, sneaks, sprints, swims or flies here.
                        _ => false,
                    };
                    value.as_bool() == Some(actual)
                })
            }),
            "minecraft:components" => wanted.as_object().is_some_and(|components| {
                components
                    .iter()
                    .all(|(component, value)| match component.as_str() {
                        "minecraft:sheep/color" => value.as_str() == facts.sheep_color,
                        _ => false,
                    })
            }),
            "minecraft:type_specific/sheep" => {
                facts.sheep_color.is_some()
                    && wanted["sheared"]
                        .as_bool()
                        .is_none_or(|s| s == facts.sheared)
            }
            _ => false,
        })
    }

    fn tool_matches(&self, predicate: &ToolPredicate) -> bool {
        let Some(tool) = self.tool.filter(|t| t.count > 0) else {
            return false;
        };
        if predicate.predicates.is_some() {
            return false;
        }
        predicate.items.as_ref().is_none_or(|items| {
            items.iter().any(|name| match name.strip_prefix('#') {
                Some(tag) => self
                    .data
                    .item_tag(tag)
                    .is_some_and(|ids| ids.contains(&tool.item)),
                None => self.data.item_id(name) == Some(tool.item),
            })
        })
    }

    fn apply(&self, functions: &OneOrMany<Function>, count: &mut i32, rng: &mut impl Rng) {
        for function in functions.iter() {
            match function {
                Function::SetCount {
                    count: n,
                    add,
                    condition,
                } => {
                    if self.test(condition.as_ref(), rng) {
                        let n = n.int(rng);
                        *count = if *add { *count + n } else { n };
                    }
                }
                Function::LimitCount { limit } => {
                    if let Some(min) = limit.min {
                        *count = (*count).max(min);
                    }
                    if let Some(max) = limit.max {
                        *count = (*count).min(max);
                    }
                }
                Function::ApplyBonus {
                    formula,
                    parameters,
                    condition,
                } => {
                    if formula == "minecraft:binomial_with_bonus_count"
                        && let Some(p) = parameters
                        && self.test(condition.as_ref(), rng)
                    {
                        *count += binomial(p.extra, p.probability, rng);
                    }
                }
                Function::Ignored => {}
            }
        }
    }

    /// `LootPoolEntryContainer.expand`: the items an entry stands for, if its
    /// conditions hold.
    fn expand<'e>(&self, entry: &'e Entry, rng: &mut impl Rng) -> Option<(Candidate<'e>, u32)> {
        match entry {
            Entry::Item {
                name,
                condition,
                modifier,
                weight,
            } => self
                .test(condition.as_ref(), rng)
                .then_some((Candidate::Item(name, modifier), *weight)),
            Entry::Reference {
                value,
                condition,
                weight,
            } => self
                .test(condition.as_ref(), rng)
                .then_some((Candidate::Table(value), *weight)),
            Entry::Alternatives {
                children,
                condition,
            } => {
                if !self.test(condition.as_ref(), rng) {
                    return None;
                }
                children.iter().find_map(|child| self.expand(child, rng))
            }
            Entry::Unsupported => None,
        }
    }

    /// The drops, at most a stack each.
    pub fn drops(&self, rng: &mut impl Rng) -> Vec<ItemStack> {
        let mut drops = Vec::new();
        self.roll(&self.table, rng, MAX_DEPTH, &mut drops);
        drops
    }

    fn roll(&self, name: &str, rng: &mut impl Rng, depth: u32, drops: &mut Vec<ItemStack>) {
        let Some(table) = self.data.loot.tables.get(name).filter(|_| depth > 0) else {
            return;
        };
        for pool in &table.pools {
            if !self.test(pool.condition.as_ref(), rng) {
                continue;
            }
            for _ in 0..pool.rolls.int(rng) {
                let candidates: Vec<_> = pool
                    .entries
                    .iter()
                    .filter_map(|e| self.expand(e, rng))
                    .collect();
                let total: u32 = candidates.iter().map(|c| c.1).sum();
                if total == 0 {
                    continue;
                }
                let mut pick = rng.random_range(0..total);
                let Some((candidate, _)) = candidates.iter().find(|c| {
                    if pick < c.1 {
                        true
                    } else {
                        pick -= c.1;
                        false
                    }
                }) else {
                    continue;
                };
                let (item, modifier) = match candidate {
                    Candidate::Table(other) => {
                        self.roll(other, rng, depth - 1, drops);
                        continue;
                    }
                    Candidate::Item(item, modifier) => (item, modifier),
                };
                let mut count = 1;
                self.apply(modifier, &mut count, rng);
                self.apply(&pool.modifier, &mut count, rng);
                self.apply(&table.modifier, &mut count, rng);
                let Some(item) = self.data.item_id(item) else {
                    continue;
                };
                let max = self.data.item(item).map_or(64, |i| i.stack.max(1));
                while count > 0 {
                    let part = count.min(max);
                    count -= part;
                    drops.push(ItemStack {
                        item,
                        count: part,
                        patch: ComponentPatch::default(),
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    use super::*;

    fn drops(data: &GameData, state: u32, tool: Option<&str>) -> Vec<(String, i32)> {
        let tool = tool.map(|name| ItemStack {
            item: data.item_id(name).unwrap(),
            count: 1,
            patch: ComponentPatch::default(),
        });
        let air = data.default_state("minecraft:air").unwrap();
        let context = Context {
            data,
            table: block_table(data.block_of(state).unwrap().1),
            state,
            pos: BlockPos { x: 0, y: 0, z: 0 },
            tool: tool.as_ref(),
            block_at: &|_| air,
            entity: None,
            by_player: true,
        };
        let mut rng = StdRng::seed_from_u64(7);
        context
            .drops(&mut rng)
            .into_iter()
            .map(|s| (data.item(s.item).unwrap().name.clone(), s.count))
            .collect()
    }

    #[test]
    fn blocks_drop_like_vanilla() {
        let data = GameData::load().unwrap();
        let state = |name: &str| data.default_state(name).unwrap();
        let named = |name: &str, count| vec![(name.to_owned(), count)];

        assert_eq!(
            drops(&data, state("minecraft:stone"), None),
            named("minecraft:cobblestone", 1)
        );
        assert_eq!(
            drops(&data, state("minecraft:grass_block"), None),
            named("minecraft:dirt", 1)
        );
        assert_eq!(
            drops(&data, state("minecraft:diamond_ore"), None),
            named("minecraft:diamond", 1)
        );
        assert_eq!(drops(&data, state("minecraft:air"), None), []);

        // A double slab drops two, a door only from its lower half.
        let slab = data.block_named("minecraft:oak_slab").unwrap();
        let double = slab.with(slab.default, "type", "double").unwrap();
        assert_eq!(drops(&data, double, None), named("minecraft:oak_slab", 2));
        let door = data.block_named("minecraft:oak_door").unwrap();
        let upper = door.with(door.default, "half", "upper").unwrap();
        assert_eq!(drops(&data, upper, None), []);
        assert_eq!(
            drops(&data, door.default, None),
            named("minecraft:oak_door", 1)
        );

        // Grass needs shears; glass needs silk touch, which nothing has.
        assert_eq!(
            drops(
                &data,
                state("minecraft:short_grass"),
                Some("minecraft:shears")
            ),
            named("minecraft:short_grass", 1)
        );
        assert_eq!(drops(&data, state("minecraft:glass"), None), []);

        // Glowstone drops two to four dust.
        let dust = drops(&data, state("minecraft:glowstone"), None);
        assert_eq!(dust[0].0, "minecraft:glowstone_dust");
        assert!((2..=4).contains(&dust[0].1));
    }

    #[test]
    fn animals_drop_like_vanilla() {
        let data = GameData::load().unwrap();
        let air = data.default_state("minecraft:air").unwrap();
        let roll = |entity: &str, facts: EntityFacts, seed| {
            let context = Context {
                data: &data,
                table: entity_table(entity),
                state: air,
                pos: BlockPos { x: 0, y: 0, z: 0 },
                tool: None,
                block_at: &|_| air,
                entity: Some(facts),
                by_player: true,
            };
            let mut rng = StdRng::seed_from_u64(seed);
            let mut names: Vec<(String, i32)> = context
                .drops(&mut rng)
                .into_iter()
                .map(|s| (data.item(s.item).unwrap().name.clone(), s.count))
                .collect();
            names.sort();
            names
        };
        for seed in 0..20 {
            // A cow: one to three raw beef, never cooked; up to two leather.
            let cow = roll("minecraft:cow", EntityFacts::default(), seed);
            let beef = cow.iter().find(|(n, _)| n == "minecraft:beef").unwrap();
            assert!((1..=3).contains(&beef.1));
            assert!(
                cow.iter()
                    .all(|(n, _)| n == "minecraft:beef" || n == "minecraft:leather")
            );
        }
        // A white sheep with wool drops white wool, a sheared one none.
        let woolly = EntityFacts {
            sheep_color: Some("white"),
            ..EntityFacts::default()
        };
        assert!(roll("minecraft:sheep", woolly, 1).contains(&("minecraft:white_wool".into(), 1)));
        let sheared = EntityFacts {
            sheared: true,
            ..woolly
        };
        assert!(
            roll("minecraft:sheep", sheared, 1)
                .iter()
                .all(|(n, _)| n == "minecraft:mutton")
        );
    }
}
