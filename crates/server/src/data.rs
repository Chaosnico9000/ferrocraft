//! Game data extracted from Mojang's data generator (`data/<version>/`, written
//! by `scripts/gen-reports.sh`), embedded into the binary.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Context;
use ferrocraft_protocol::packets::configuration::{
    RegistryData, RegistryEntry, RegistryTags, UpdateTags,
};
use serde::Deserialize;

use crate::crafting::Recipes;
use crate::loot::LootTables;

const REGISTRIES: &str = include_str!("../../../data/26.3/registries.json");
const TAGS: &str = include_str!("../../../data/26.3/tags.json");
const BLOCKS: &str = include_str!("../../../data/26.3/blocks.json");
const ITEMS: &str = include_str!("../../../data/26.3/items.json");
const IDS: &str = include_str!("../../../data/26.3/ids.json");
const RECIPES: &str = include_str!("../../../data/26.3/recipes.json");
const LOOT: &str = include_str!("../../../data/26.3/loot_tables.json");
const ENTITIES: &str = include_str!("../../../data/26.3/entities.json");
const FEATURES: &str = include_str!("../../../data/26.3/features.json");
/// The English texts of the server jar, for the console.
const LANGUAGE: &str = include_str!("../../../data/26.3/en_us.json");
const ALCHEMY: &str = include_str!("../../../data/26.3/alchemy.json");
const ENCHANTMENTS: &str = include_str!("../../../data/26.3/enchantments.json");

#[derive(Deserialize)]
struct SyncedRegistry {
    registry: String,
    entries: Vec<String>,
}

#[derive(Deserialize)]
struct Blocks {
    states: usize,
    collision_shapes: Vec<Vec<[f64; 6]>>,
    face_shapes: Vec<Vec<[f64; 6]>>,
    blocks: Vec<Block>,
}

/// A box from `(x, y, z)` to `(x, y, z)`, in block-relative coordinates.
pub type Aabb = [f64; 6];

/// What `scripts/BlockPhysics.java` read from vanilla for one state.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(from = "RawPhysics")]
pub struct StatePhysics {
    /// Block light the state gives off, 0–15.
    pub emission: u8,
    /// How much light it swallows on the way through (`getLightDampening`).
    pub dampening: u8,
    /// Sky light passes down through it unchanged.
    pub sky_passes: bool,
    /// Index into [`GameData::collision_shape`].
    pub collision: u16,
    /// Per direction (down, up, north, south, west, east), the part of that
    /// face that stops light (`LightEngine.getOcclusionShape`).
    pub faces: [u16; 6],
    /// 0 none, 1 water, 2 lava.
    pub fluid: u8,
    /// Monsters may spawn on top (`BlockState.isValidSpawn` for a zombie).
    pub spawnable: bool,
    /// The bits of [`flags`].
    pub flags: u32,
    /// Index into [`GameData::collision_shape`]'s table: the outline that
    /// aiming hits (`getShape`).
    pub outline: u16,
}

/// What [`StatePhysics::flags`] holds, as `scripts/BlockPhysics.java`
/// writes it.
pub mod flags {
    /// `BlockState.isSolid`.
    pub const SOLID: u32 = 1;
    /// `isRandomlyTicking`.
    pub const RANDOM_TICKS: u32 = 2;
    /// `isRedstoneConductor`.
    pub const CONDUCTOR: u32 = 4;
    /// `canBeReplaced`.
    pub const REPLACEABLE: u32 = 8;
    /// `isFaceSturdy` for the face in 3D data value order (down, up,
    /// north, south, west, east): `STURDY << direction`.
    pub const STURDY: u32 = 16;
    /// The top holds something in its centre (`SupportType.CENTER`).
    pub const CENTER_TOP: u32 = 1024;
    /// `isSignalSource`.
    pub const SIGNAL_SOURCE: u32 = 2048;
    /// `PushReaction` (see [`push`](super::push)) in three bits from here.
    pub const PUSH_SHIFT: u32 = 12;
    /// `hasAnalogOutputSignal`: a comparator reads it.
    pub const ANALOG: u32 = 1 << 15;
    /// `ignitedByLava`.
    pub const IGNITED_BY_LAVA: u32 = 1 << 16;
}

/// How a piston moves a block (`PushReaction`).
pub mod push {
    pub const PUSH_PULL: u32 = 0;
    pub const PUSH_ONLY: u32 = 1;
    /// It breaks.
    pub const POPPED: u32 = 2;
    pub const IMMOVABLE: u32 = 3;
    pub const IGNORE: u32 = 4;
}

type RawPhysics = (
    u8,
    u8,
    bool,
    u16,
    u16,
    u16,
    u16,
    u16,
    u16,
    u16,
    u8,
    bool,
    u32,
    u16,
);

impl From<RawPhysics> for StatePhysics {
    fn from(
        (
            emission,
            dampening,
            sky_passes,
            collision,
            d,
            u,
            n,
            s,
            w,
            e,
            fluid,
            spawnable,
            flags,
            outline,
        ): RawPhysics,
    ) -> Self {
        Self {
            emission,
            dampening,
            sky_passes,
            collision,
            faces: [d, u, n, s, w, e],
            fluid,
            spawnable,
            flags,
            outline,
        }
    }
}

impl StatePhysics {
    pub fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }

    /// How pistons move it, one of [`push`].
    pub fn push_reaction(&self) -> u32 {
        (self.flags >> flags::PUSH_SHIFT) & 7
    }

    /// Whether the face towards `direction` (3D data value) is sturdy.
    pub fn sturdy(&self, direction: usize) -> bool {
        self.flags & (flags::STURDY << direction) != 0
    }
}

/// A block and its states. States are numbered from `first`, one per
/// combination of property values, the last property changing fastest.
#[derive(Debug, Deserialize)]
pub struct Block {
    pub name: String,
    pub first: u32,
    pub default: u32,
    /// Sorted by name, each with its values in vanilla's order.
    pub properties: Vec<(String, Vec<String>)>,
    /// The item that stands for the block (`Block.asItem`), air if none.
    pub item: String,
    /// Seconds-ish of digging (`destroyTime`); -1 for unbreakable.
    pub hardness: f32,
    /// Drops only for the right tool (`requiresCorrectToolForDrops`).
    pub requires_tool: bool,
    /// How much of an explosion's power it swallows.
    pub explosion_resistance: f32,
    pub block_entity: Option<String>,
    /// Vanilla's class and its superclasses below `Block`, such as
    /// `["CarrotBlock", "CropBlock", "VegetationBlock"]`.
    #[serde(default)]
    pub classes: Vec<String>,
    /// How readily it catches fire and burns away (`FireBlock`'s ignite and
    /// burn odds).
    #[serde(default)]
    pub fire: Option<[i32; 2]>,
    /// Experience when mined with the right tool, at least and at most
    /// (`DropExperienceBlock.xpRange`).
    #[serde(default)]
    pub xp: Option<[i32; 2]>,
    /// One entry for all states, or one per state.
    states: Vec<StatePhysics>,
}

/// An item and its default components, as far as the server uses them.
#[derive(Debug, Deserialize)]
pub struct Item {
    pub name: String,
    /// `max_stack_size`.
    pub stack: i32,
    pub max_damage: Option<i32>,
    pub tool: Option<Tool>,
    pub food: Option<Food>,
    /// How long eating or drinking takes.
    pub consume_seconds: Option<f32>,
    /// What the item adds to the player's attack damage and speed in the
    /// main hand.
    #[serde(default)]
    pub attack_damage: f32,
    #[serde(default)]
    pub attack_speed: f32,
    /// What the item adds to its wearer's armor and toughness.
    #[serde(default)]
    pub armor: f32,
    #[serde(default)]
    pub armor_toughness: f32,
    /// As fuel: ticks it burns and how much faster it cooks, in a furnace
    /// and in a smoker or blast furnace.
    pub fuel: Option<[(i32, f32); 2]>,
    /// In a brewing stand (`brewing_fuel`): brews per item and their speed.
    pub brewing_fuel: Option<(i32, f32)>,
    /// What using it on a block turns the block into, such as
    /// `minecraft:hoe` (see [`Features::block_transformers`]).
    pub block_transformer: Option<String>,
    /// Further default components as vanilla's report has them, by name
    /// without `minecraft:` (`consumable`, `enchantable`, ...).
    #[serde(default)]
    pub components: HashMap<String, serde_json::Value>,
}

/// Data pack content that grows and changes blocks, as
/// `features.json` holds it: vanilla's own JSON.
#[derive(Debug, Deserialize)]
pub struct Features {
    /// Tree features by name, such as `minecraft:oak`.
    pub trees: HashMap<String, serde_json::Value>,
    pub block_state_providers: HashMap<String, serde_json::Value>,
    pub block_transformers: HashMap<String, serde_json::Value>,
}

/// Status effects, potions and attributes, as `scripts/Alchemy.java` read
/// them from vanilla, each in network-id order.
#[derive(Debug, Deserialize)]
pub struct Alchemy {
    pub effects: Vec<EffectInfo>,
    pub potions: Vec<PotionInfo>,
    pub attributes: Vec<AttributeInfo>,
}

/// A status effect (`MobEffect`).
#[derive(Debug, Deserialize)]
pub struct EffectInfo {
    pub name: String,
    /// `beneficial`, `harmful` or `neutral`.
    pub category: String,
    pub color: i32,
    /// Happens at once, like healing (`isInstantaneous`).
    pub instant: bool,
    /// Attribute modifiers at level I; each level adds the amount again.
    pub modifiers: Vec<EffectModifier>,
    /// The particle it shows, coloured with [`Self::color`] if
    /// `colored_particle`.
    pub particle: String,
    pub colored_particle: bool,
}

#[derive(Debug, Deserialize)]
pub struct EffectModifier {
    pub attribute: String,
    pub id: String,
    pub amount: f64,
    /// `add_value`, `add_multiplied_base` or `add_multiplied_total`.
    pub operation: String,
}

/// A potion (`Potion`): its effects, and the name its texts go by.
#[derive(Debug, Deserialize)]
pub struct PotionInfo {
    pub name: String,
    pub base: String,
    pub effects: Vec<PotionEffect>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PotionEffect {
    pub id: String,
    pub amplifier: i32,
    pub duration: i32,
}

/// An attribute (`RangedAttribute`).
#[derive(Debug, Deserialize)]
pub struct AttributeInfo {
    pub name: String,
    pub default: f64,
    #[serde(default = "f64_min")]
    pub min: f64,
    #[serde(default = "f64_max")]
    pub max: f64,
    /// Clients hear of it (`isClientSyncable`).
    pub sync: bool,
    /// A player's base value, if players have it.
    pub player: Option<f64>,
}

fn f64_min() -> f64 {
    f64::MIN
}

fn f64_max() -> f64 {
    f64::MAX
}

/// An entity type as `scripts/EntityInfo.java` read it from vanilla.
#[derive(Debug, Deserialize)]
pub struct EntityInfo {
    pub name: String,
    pub width: f64,
    pub height: f64,
    pub eye_height: f64,
    /// In chunks.
    pub tracking_range: u32,
    pub update_interval: u32,
    /// `MobCategory`: creature, monster, misc, ...
    pub category: String,
    pub max_health: Option<f32>,
    pub movement_speed: Option<f64>,
    pub step_height: Option<f64>,
    pub gravity: Option<f64>,
    pub knockback_resistance: Option<f64>,
    pub attack_damage: Option<f32>,
    /// How far a mob notices its target.
    pub follow_range: Option<f64>,
    pub armor: Option<f32>,
}

/// The `tool` component: how fast an item breaks which blocks.
#[derive(Debug, Deserialize)]
pub struct Tool {
    pub rules: Vec<ToolRule>,
    #[serde(default = "one")]
    pub default_mining_speed: f32,
    /// Wear per broken block.
    #[serde(default = "one_int")]
    pub damage_per_block: i32,
    /// Swords cannot break blocks in creative.
    #[serde(default = "yes")]
    pub can_destroy_blocks_in_creative: bool,
}

fn one_int() -> i32 {
    1
}

fn yes() -> bool {
    true
}

fn one() -> f32 {
    1.0
}

#[derive(Debug, Deserialize)]
pub struct ToolRule {
    /// A block, a list of blocks, or `#tag`.
    pub blocks: BlockSet,
    pub speed: Option<f32>,
    pub correct_for_drops: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum BlockSet {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Deserialize)]
pub struct Food {
    pub nutrition: i32,
    pub saturation: f32,
    #[serde(default)]
    pub can_always_eat: bool,
}

#[derive(Debug, Deserialize)]
struct Ids {
    entity_type: Vec<String>,
    block_entity_type: Vec<String>,
    menu: Vec<String>,
    command_argument_type: Vec<String>,
    data_component_type: Vec<String>,
    sound_event: Vec<String>,
    particle_type: Vec<String>,
    mob_effect: Vec<String>,
    potion: Vec<String>,
    attribute: Vec<String>,
}

impl Block {
    pub fn state_count(&self) -> u32 {
        self.properties
            .iter()
            .map(|(_, v)| v.len() as u32)
            .product()
    }

    /// The value of every property in `state`, which must belong to this block.
    pub fn values(&self, state: u32) -> Vec<(&str, &str)> {
        let mut index = (state - self.first) as usize;
        let mut values: Vec<_> = self
            .properties
            .iter()
            .rev()
            .map(|(key, values)| {
                let value = &values[index % values.len()];
                index /= values.len();
                (key.as_str(), value.as_str())
            })
            .collect();
        values.reverse();
        values
    }

    /// `state` with one property changed; `None` if the block has no such
    /// property or value.
    pub fn with(&self, state: u32, key: &str, value: &str) -> Option<u32> {
        let mut stride = 1;
        for (k, values) in self.properties.iter().rev() {
            if k == key {
                let new = values.iter().position(|v| v == value)? as u32;
                let index = state - self.first;
                let old = (index / stride) % values.len() as u32;
                return Some(self.first + index - old * stride + new * stride);
            }
            stride *= values.len() as u32;
        }
        None
    }

    pub fn has(&self, key: &str) -> bool {
        self.properties.iter().any(|(k, _)| k == key)
    }

    /// Whether vanilla's class for the block is or extends `class`, e.g.
    /// `FallingBlock`.
    pub fn is(&self, class: &str) -> bool {
        self.classes.iter().any(|c| c == class)
    }

    /// The value of one property in `state`.
    pub fn value(&self, state: u32, key: &str) -> Option<&str> {
        self.values(state)
            .into_iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v)
    }

    pub fn physics(&self, state: u32) -> &StatePhysics {
        let index = (state - self.first) as usize;
        self.states.get(index).unwrap_or(&self.states[0])
    }
}

#[derive(Debug)]
pub struct GameData {
    /// One packet per synchronized registry, entries without data: the
    /// client takes them from the vanilla pack it already has.
    pub registries: Vec<RegistryData>,
    pub tags: UpdateTags,
    /// Number of block states, which sets the bit width of global palettes.
    pub block_states: usize,
    /// In id order, which is also the order of their states.
    blocks: Vec<Block>,
    block_ids: HashMap<String, usize>,
    /// For each item, the block it places.
    item_blocks: Vec<Option<usize>>,
    pub items: Vec<Item>,
    item_ids: HashMap<String, i32>,
    ids: Ids,
    /// Blocks of `minecraft:replaceable`, which a placed block may replace.
    replaceable: HashSet<usize>,
    /// Physics of every state, by state id.
    physics: Vec<StatePhysics>,
    collision_shapes: Vec<Vec<Aabb>>,
    face_shapes: Vec<Vec<Aabb>>,
    pub recipes: Recipes,
    pub loot: LootTables,
    /// By network id.
    pub entities: Vec<EntityInfo>,
    pub features: Features,
    /// Per block tag, a bit per block id, for quick lookups.
    block_tags: HashMap<String, Vec<u64>>,
    /// Translation keys and their English text (`en_us.json`).
    pub language: HashMap<String, String>,
    pub alchemy: Alchemy,
    /// The enchantments as vanilla's data pack has them, in network-id
    /// order, each with its `name`.
    pub enchantments: Vec<serde_json::Value>,
}

impl GameData {
    pub fn load() -> anyhow::Result<Self> {
        let synced: Vec<SyncedRegistry> =
            serde_json::from_str(REGISTRIES).context("parsing registries.json")?;
        let tags: BTreeMap<String, BTreeMap<String, Vec<i32>>> =
            serde_json::from_str(TAGS).context("parsing tags.json")?;
        let blocks: Blocks = serde_json::from_str(BLOCKS).context("parsing blocks.json")?;
        let items: Vec<Item> = serde_json::from_str(ITEMS).context("parsing items.json")?;
        let ids: Ids = serde_json::from_str(IDS).context("parsing ids.json")?;

        let registries = synced
            .into_iter()
            .map(|r| RegistryData {
                registry: r.registry,
                entries: r
                    .entries
                    .into_iter()
                    .map(|id| RegistryEntry { id, data: None })
                    .collect(),
            })
            .collect();
        let tags = UpdateTags {
            registries: tags
                .into_iter()
                .map(|(registry, tags)| RegistryTags {
                    registry,
                    tags: tags.into_iter().collect(),
                })
                .collect(),
        };
        let block_ids: HashMap<String, usize> = blocks
            .blocks
            .iter()
            .enumerate()
            .map(|(id, b)| (b.name.clone(), id))
            .collect();
        let item_ids: HashMap<String, i32> = items
            .iter()
            .zip(0..)
            .map(|(item, id)| (item.name.clone(), id))
            .collect();
        // The block an item places: the one of the same name if it stands for
        // the item (torch, not wall_torch), else the first that does
        // (carrot → carrots). Air is an item only so that empty slots have
        // an id.
        let mut item_blocks: Vec<Option<usize>> = vec![None; items.len()];
        for (id, block) in blocks.blocks.iter().enumerate().skip(1) {
            if let Some(&item) = item_ids.get(&block.item)
                && item != 0
            {
                let slot = &mut item_blocks[item as usize];
                if slot.is_none() || block.name == block.item {
                    *slot = Some(id);
                }
            }
        }
        let mut physics = Vec::with_capacity(blocks.states);
        for block in &blocks.blocks {
            for state in block.first..block.first + block.state_count() {
                physics.push(*block.physics(state));
            }
        }
        let mut data = Self {
            registries,
            tags,
            block_states: blocks.states,
            blocks: blocks.blocks,
            block_ids,
            item_blocks,
            items,
            item_ids,
            ids,
            replaceable: HashSet::new(),
            physics,
            collision_shapes: blocks.collision_shapes,
            face_shapes: blocks.face_shapes,
            recipes: Recipes::default(),
            loot: LootTables::load(LOOT)?,
            entities: serde_json::from_str(ENTITIES).context("parsing entities.json")?,
            features: serde_json::from_str(FEATURES).context("parsing features.json")?,
            block_tags: HashMap::new(),
            language: serde_json::from_str(LANGUAGE).context("parsing en_us.json")?,
            alchemy: serde_json::from_str(ALCHEMY).context("parsing alchemy.json")?,
            enchantments: serde_json::from_str(ENCHANTMENTS)
                .context("parsing enchantments.json")?,
        };
        let words = data.blocks.len().div_ceil(64);
        if let Some(blocks) = data
            .tags
            .registries
            .iter()
            .find(|r| r.registry == "minecraft:block")
        {
            for (tag, ids) in &blocks.tags {
                let mut bits = vec![0u64; words];
                for &id in ids {
                    if let Some(word) = bits.get_mut(id as usize / 64) {
                        *word |= 1 << (id as usize % 64);
                    }
                }
                data.block_tags.insert(tag.clone(), bits);
            }
        }
        data.recipes = Recipes::load(
            RECIPES,
            |name| data.item_id(name),
            |tag| data.item_tag(tag).map(<[i32]>::to_vec),
        )?;
        data.replaceable = data
            .block_tag("minecraft:replaceable")?
            .iter()
            .map(|&id| id as usize)
            .collect();
        Ok(data)
    }

    /// Whether a placed block may take the place of this state, like air,
    /// water or grass.
    pub fn is_replaceable(&self, state: u32) -> bool {
        self.block_of(state)
            .is_some_and(|(id, _)| self.replaceable.contains(&id))
    }

    /// Network id of an entry in a synchronized registry: its position in the
    /// `registry_data` packet.
    pub fn registry_id(&self, registry: &str, entry: &str) -> anyhow::Result<i32> {
        let data = self.registry(registry)?;
        let index = data
            .entries
            .iter()
            .position(|e| e.id == entry)
            .with_context(|| format!("{entry} is not in {registry}"))?;
        Ok(index as i32)
    }

    pub fn registry_len(&self, registry: &str) -> anyhow::Result<usize> {
        Ok(self.registry(registry)?.entries.len())
    }

    fn registry(&self, registry: &str) -> anyhow::Result<&RegistryData> {
        self.registries
            .iter()
            .find(|r| r.registry == registry)
            .with_context(|| format!("{registry} is not a synchronized registry"))
    }

    /// Global id of a block's default state, e.g. `minecraft:stone` → 1.
    pub fn default_state(&self, block: &str) -> anyhow::Result<u32> {
        self.block_ids
            .get(block)
            .map(|&id| self.blocks[id].default)
            .with_context(|| format!("unknown block {block}"))
    }

    /// Network id of an entity type, e.g. `minecraft:player`.
    pub fn entity_type(&self, name: &str) -> anyhow::Result<i32> {
        position(&self.ids.entity_type, name)
    }

    pub fn block_entity_type(&self, name: &str) -> anyhow::Result<i32> {
        position(&self.ids.block_entity_type, name)
    }

    pub fn menu(&self, name: &str) -> anyhow::Result<i32> {
        position(&self.ids.menu, name)
    }

    /// An entity type and its network id.
    pub fn entity(&self, name: &str) -> anyhow::Result<(i32, &EntityInfo)> {
        let id = self.entity_type(name)?;
        let info = self
            .entities
            .get(id as usize)
            .context("entities.json is shorter than the registry")?;
        anyhow::ensure!(
            info.name == name,
            "entities.json does not match the registry"
        );
        Ok((id, info))
    }

    /// A status effect and its network id.
    pub fn effect(&self, name: &str) -> Option<(usize, &EffectInfo)> {
        self.alchemy
            .effects
            .iter()
            .enumerate()
            .find(|(_, e)| e.name == name)
    }

    pub fn potion(&self, name: &str) -> Option<&PotionInfo> {
        self.alchemy.potions.iter().find(|p| p.name == name)
    }

    pub fn attribute(&self, name: &str) -> Option<(usize, &AttributeInfo)> {
        self.alchemy
            .attributes
            .iter()
            .enumerate()
            .find(|(_, a)| a.name == name)
    }

    /// An enchantment as the data pack has it, with its network id.
    pub fn enchantment(&self, name: &str) -> Option<(usize, &serde_json::Value)> {
        self.enchantments
            .iter()
            .enumerate()
            .find(|(_, e)| e["name"] == name)
    }

    pub fn data_component_type(&self, name: &str) -> anyhow::Result<i32> {
        position(&self.ids.data_component_type, name)
    }

    /// The entries of a registry that is not synchronized but built into
    /// the game, in network-id order.
    pub fn static_registry(&self, registry: &str) -> Option<&[String]> {
        let ids = &self.ids;
        Some(match registry {
            "minecraft:entity_type" => &ids.entity_type,
            "minecraft:block_entity_type" => &ids.block_entity_type,
            "minecraft:menu" => &ids.menu,
            "minecraft:data_component_type" => &ids.data_component_type,
            "minecraft:sound_event" => &ids.sound_event,
            "minecraft:particle_type" => &ids.particle_type,
            "minecraft:mob_effect" => &ids.mob_effect,
            "minecraft:potion" => &ids.potion,
            "minecraft:attribute" => &ids.attribute,
            _ => return None,
        })
    }

    /// Whether a block is in a tool rule's or loot predicate's set: a
    /// name, `#tag` or a list of names.
    pub fn in_block_set(&self, set: &BlockSet, block: usize) -> bool {
        let name = &self.blocks[block].name;
        match set {
            BlockSet::One(one) => match one.strip_prefix('#') {
                Some(tag) => self.block_in_tag(tag, block),
                None => one == name,
            },
            BlockSet::Many(many) => many.contains(name),
        }
    }

    pub fn command_argument_type(&self, name: &str) -> anyhow::Result<i32> {
        position(&self.ids.command_argument_type, name)
    }

    pub fn sound_event(&self, name: &str) -> anyhow::Result<i32> {
        position(&self.ids.sound_event, name)
    }

    pub fn particle_type(&self, name: &str) -> anyhow::Result<i32> {
        position(&self.ids.particle_type, name)
    }

    pub fn item(&self, id: i32) -> Option<&Item> {
        self.items.get(usize::try_from(id).ok()?)
    }

    /// Network id of an item, e.g. `minecraft:stone` → 1.
    pub fn item_id(&self, name: &str) -> Option<i32> {
        self.item_ids.get(name).copied()
    }

    /// Physics of a state; air's for states that do not exist.
    pub fn physics(&self, state: u32) -> &StatePhysics {
        self.physics.get(state as usize).unwrap_or(&self.physics[0])
    }

    pub fn collision_shape(&self, state: u32) -> &[Aabb] {
        &self.collision_shapes[usize::from(self.physics(state).collision)]
    }

    /// What aiming at the state hits (`getShape`).
    pub fn outline_shape(&self, state: u32) -> &[Aabb] {
        &self.collision_shapes[usize::from(self.physics(state).outline)]
    }

    /// The part of a face that stops light, `direction` in 3D data value
    /// order (down, up, north, south, west, east).
    pub fn face_shape(&self, state: u32, direction: usize) -> &[Aabb] {
        &self.face_shapes[usize::from(self.physics(state).faces[direction])]
    }

    /// Whether `tag` (without `#`) of the block registry holds `block`.
    pub fn block_in_tag(&self, tag: &str, block: usize) -> bool {
        self.block_tags
            .get(tag)
            .and_then(|bits| bits.get(block / 64))
            .is_some_and(|word| word & (1 << (block % 64)) != 0)
    }

    /// Whether the block of `state` is in a block tag.
    pub fn state_in_tag(&self, state: u32, tag: &str) -> bool {
        self.block_of(state)
            .is_some_and(|(id, _)| self.block_in_tag(tag, id))
    }

    /// Whether `state` is a state of the block called `name`.
    pub fn state_is(&self, state: u32, name: &str) -> bool {
        self.block_named(name)
            .is_some_and(|b| (b.first..b.first + b.state_count()).contains(&state))
    }

    /// The block a state belongs to, with its id; `None` for an invalid state.
    pub fn block_of(&self, state: u32) -> Option<(usize, &Block)> {
        let id = self
            .blocks
            .partition_point(|b| b.first <= state)
            .checked_sub(1)?;
        let block = &self.blocks[id];
        (state < block.first + block.state_count()).then_some((id, block))
    }

    pub fn block_named(&self, name: &str) -> Option<&Block> {
        self.block_ids.get(name).map(|&id| &self.blocks[id])
    }

    pub fn block(&self, id: usize) -> &Block {
        &self.blocks[id]
    }

    /// Every block, in id order.
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// The block an item places, `None` for items that are not blocks.
    pub fn block_for_item(&self, item: i32) -> Option<&Block> {
        let id = (*self.item_blocks.get(usize::try_from(item).ok()?)?)?;
        Some(&self.blocks[id])
    }

    /// Network ids of the items in an item tag such as `minecraft:planks`.
    pub fn item_tag(&self, tag: &str) -> Option<&[i32]> {
        self.tags
            .registries
            .iter()
            .find(|r| r.registry == "minecraft:item")
            .and_then(|r| r.tags.iter().find(|(name, _)| name == tag))
            .map(|(_, ids)| ids.as_slice())
    }

    /// Whether a tag of a registry the client gets tags of holds `id`, e.g.
    /// `minecraft:bypasses_armor` of `minecraft:damage_type`.
    pub fn tag_contains(&self, registry: &str, tag: &str, id: i32) -> bool {
        self.tags
            .registries
            .iter()
            .find(|r| r.registry == registry)
            .and_then(|r| r.tags.iter().find(|(name, _)| name == tag))
            .is_some_and(|(_, ids)| ids.contains(&id))
    }

    /// Network ids of the blocks in a block tag such as `minecraft:replaceable`.
    pub fn block_tag(&self, tag: &str) -> anyhow::Result<&[i32]> {
        self.tags
            .registries
            .iter()
            .find(|r| r.registry == "minecraft:block")
            .and_then(|r| r.tags.iter().find(|(name, _)| name == tag))
            .map(|(_, ids)| ids.as_slice())
            .with_context(|| format!("unknown block tag {tag}"))
    }
}

fn position(names: &[String], name: &str) -> anyhow::Result<i32> {
    names
        .iter()
        .position(|e| e == name)
        .map(|id| id as i32)
        .with_context(|| format!("unknown {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_data_loads_and_matches_vanilla() {
        let data = GameData::load().unwrap();
        // Counts and ids as a vanilla 26.3 server sends them.
        assert_eq!(data.registries.len(), 32);
        assert_eq!(data.registries[0].registry, "minecraft:worldgen/biome");
        assert_eq!(data.registry_len("minecraft:worldgen/biome").unwrap(), 67);
        assert_eq!(
            data.registry_id("minecraft:worldgen/biome", "minecraft:plains")
                .unwrap(),
            41
        );
        assert_eq!(
            data.registry_id("minecraft:dimension_type", "minecraft:overworld")
                .unwrap(),
            0
        );
        assert_eq!(data.tags.registries.len(), 15);
        let tag_count: usize = data.tags.registries.iter().map(|r| r.tags.len()).sum();
        assert_eq!(tag_count, 773);
        assert_eq!(data.default_state("minecraft:air").unwrap(), 0);
        assert_eq!(data.default_state("minecraft:bedrock").unwrap(), 88);
        assert_eq!(data.default_state("minecraft:grass_block").unwrap(), 9);
        assert_eq!(data.items.len(), 1658);
        assert_eq!(data.items[1].name, "minecraft:stone");
        assert_eq!(data.entity_type("minecraft:player").unwrap(), 159);
    }

    #[test]
    fn block_states_decompose_like_the_report() {
        let data = GameData::load().unwrap();
        // From the 26.3 blocks report: oak_log axis=x is 139, the default
        // axis=y is 140; oak_stairs facing=east, half=top, shape=straight,
        // waterlogged=false is 5463 + 3 * 20 + 1.
        let (id, log) = data.block_of(140).unwrap();
        assert_eq!((id, log.name.as_str()), (51, "minecraft:oak_log"));
        assert_eq!(log.values(139), [("axis", "x")]);
        assert_eq!(log.with(140, "axis", "z"), Some(141));
        assert_eq!(log.with(140, "facing", "north"), None);

        let (_, stairs) = data.block_of(5474).unwrap();
        let state = stairs.with(5474, "facing", "east").unwrap();
        let state = stairs.with(state, "half", "top").unwrap();
        assert_eq!(state, 5463 + 3 * 20 + 1);
        assert_eq!(
            stairs.values(state),
            [
                ("facing", "east"),
                ("half", "top"),
                ("shape", "straight"),
                ("waterlogged", "false")
            ]
        );
        assert!(data.block_of(data.block_states as u32).is_none());

        let stone = data.item_id("minecraft:stone").unwrap();
        assert_eq!(data.block_for_item(stone).unwrap().default, 1);
        let carrot = data.item_id("minecraft:carrot").unwrap();
        assert_eq!(
            data.block_for_item(carrot).unwrap().name,
            "minecraft:carrots"
        );
        let stick = data.item_id("minecraft:stick").unwrap();
        assert!(data.block_for_item(stick).is_none());
        assert!(
            data.block_tag("minecraft:replaceable")
                .unwrap()
                .contains(&0)
        );
        assert!(data.is_replaceable(0));
        assert!(data.is_replaceable(data.default_state("minecraft:short_grass").unwrap()));
        assert!(!data.is_replaceable(data.default_state("minecraft:stone").unwrap()));
        let torch = data.item_id("minecraft:torch").unwrap();
        assert_eq!(data.block_for_item(torch).unwrap().name, "minecraft:torch");
    }

    #[test]
    fn physics_from_vanilla() {
        let data = GameData::load().unwrap();
        let state = |name| data.default_state(name).unwrap();
        let stone = data.physics(state("minecraft:stone"));
        assert_eq!(
            (stone.emission, stone.dampening, stone.sky_passes),
            (0, 15, false)
        );
        assert_eq!(
            data.collision_shape(state("minecraft:stone")),
            [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]
        );
        let torch = data.physics(state("minecraft:torch"));
        assert_eq!(
            (torch.emission, torch.dampening, torch.sky_passes),
            (14, 0, true)
        );
        assert!(data.collision_shape(state("minecraft:torch")).is_empty());
        assert_eq!(data.physics(state("minecraft:glowstone")).emission, 15);
        assert_eq!(data.physics(state("minecraft:oak_leaves")).dampening, 1);
        assert_eq!(data.physics(state("minecraft:water")).fluid, 1);
        // A bottom slab stops light through its bottom face, not its top.
        let slab = data.block_named("minecraft:oak_slab").unwrap();
        let bottom = slab.with(slab.default, "type", "bottom").unwrap();
        assert!(!data.face_shape(bottom, 0).is_empty());
        assert!(data.face_shape(bottom, 1).is_empty());

        let (_, chest) = data.block_of(state("minecraft:chest")).unwrap();
        assert_eq!(chest.block_entity.as_deref(), Some("minecraft:chest"));
        assert_eq!(
            data.block_named("minecraft:bedrock").unwrap().hardness,
            -1.0
        );
        assert!(
            data.block_named("minecraft:diamond_ore")
                .unwrap()
                .requires_tool
        );
        let pickaxe = data
            .item(data.item_id("minecraft:diamond_pickaxe").unwrap())
            .unwrap();
        assert_eq!(pickaxe.stack, 1);
        assert_eq!(pickaxe.tool.as_ref().unwrap().rules[1].speed, Some(8.0));
        let bread = data.item(data.item_id("minecraft:bread").unwrap()).unwrap();
        assert_eq!(bread.food.as_ref().unwrap().nutrition, 5);
        assert_eq!(
            data.item(data.item_id("minecraft:ender_pearl").unwrap())
                .unwrap()
                .stack,
            16
        );
    }
}
