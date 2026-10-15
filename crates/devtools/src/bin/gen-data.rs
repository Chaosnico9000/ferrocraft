//! Turns the output of Mojang's data generator (`--reports --server`) into the
//! files under `data/<version>/` that the server embeds:
//!
//! - `registries.json`: the synchronized registries in the order vanilla sends
//!   them, each with its entry names in network-id order,
//! - `tags.json`: every tag the client receives, resolved to network ids,
//! - `blocks.json`: every block with its range of state ids, its default state,
//!   its properties and, from `block_physics.json` (`scripts/BlockPhysics.java`),
//!   light, hardness, collision and block entity, in state id order,
//! - `items.json`: every item with the default components the server needs
//!   (stack size, tool, food), in network-id order,
//! - `loot_tables.json`: the block and entity loot tables and the predicates
//!   they use,
//! - `entities.json`: size, tracking and base attributes of every entity type,
//!   from `entity_info.json` (`scripts/EntityInfo.java`),
//! - `recipes.json`: the crafting and cooking recipes,
//! - `features.json`: what grows from saplings (the tree features of
//!   `TreeGrower`), the block state providers they use, and the block
//!   transformers of hoes, shovels and axes,
//! - `ids.json`: the names of further static registries in network-id order.
//!
//!   gen-data <generator output dir> <data/version dir>

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde_json::Value;

/// `RegistryDataLoader.SYNCHRONIZED_REGISTRIES` of 26.3, in order. Not part of
/// any report, so it is copied from the vanilla source.
const SYNCED_REGISTRIES: &[&str] = &[
    "worldgen/biome",
    "chat_type",
    "trim_pattern",
    "trim_material",
    "wolf_variant",
    "wolf_sound_variant",
    "pig_variant",
    "pig_sound_variant",
    "frog_variant",
    "cat_variant",
    "cat_sound_variant",
    "cow_sound_variant",
    "cow_variant",
    "chicken_sound_variant",
    "chicken_variant",
    "zombie_nautilus_variant",
    "painting_variant",
    "sulfur_cube_archetype",
    "dimension_type",
    "damage_type",
    "banner_pattern",
    "enchantment",
    "jukebox_song",
    "instrument",
    "test_environment",
    "test_instance",
    "dialog",
    "world_clock",
    "timeline",
    "decorated_pot_pattern",
    "block_transformer",
    "worldgen/block_state_provider",
];

fn main() -> anyhow::Result<()> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [generated, out] = &args[..] else {
        bail!("usage: gen-data <generator output dir> <data/version dir>");
    };
    let data = generated.join("data/minecraft");
    let static_registries = read_json(&generated.join("reports/registries.json"))?;

    // Network ids: synced registries number their entries in the order they are
    // sent, which is the sorted order vanilla loads data pack files in; static
    // registries use the `protocol_id` from the report.
    let mut ids: HashMap<String, HashMap<String, i32>> = HashMap::new();
    let mut synced = Vec::new();
    for path in SYNCED_REGISTRIES {
        let entries = list_entries(&data.join(path))?;
        ids.insert(
            format!("minecraft:{path}"),
            entries.iter().cloned().zip(0..).collect(),
        );
        synced.push((format!("minecraft:{path}"), entries));
    }
    for (registry, content) in static_registries.as_object().context("registries report")? {
        let entries = content["entries"].as_object().context("registry entries")?;
        let map = entries
            .iter()
            .map(|(name, e)| {
                Ok((
                    name.clone(),
                    e["protocol_id"].as_i64().context("id")? as i32,
                ))
            })
            .collect::<anyhow::Result<_>>()?;
        ids.insert(registry.clone(), map);
    }

    // The English texts as the server jar has them, for the console.
    std::fs::copy(generated.join("en_us.json"), out.join("en_us.json"))
        .context("en_us.json: extract it from the server jar first")?;

    let tags = read_tags(&data.join("tags"), &ids)?;
    let physics = read_json(&generated.join("block_physics.json"))
        .context("block_physics.json: run scripts/BlockPhysics.java first")?;
    let blocks = read_blocks(
        &generated.join("reports/blocks.json"),
        &ids["minecraft:block"],
        &physics,
    )?;
    let items = items_json(
        &ids["minecraft:item"],
        &generated.join("reports/minecraft/components/item"),
        &data,
    )?;
    let loot_tables = loot_tables_json(&data)?;
    let entity_info = read_json(&generated.join("entity_info.json"))
        .context("entity_info.json: run scripts/EntityInfo.java first")?;
    let entities = entities_json(&ids["minecraft:entity_type"], &entity_info)?;
    let recipes = recipes_json(&data.join("recipe"))?;
    let features = features_json(&data)?;
    let mut names = Vec::new();
    for registry in [
        "entity_type",
        "block_entity_type",
        "menu",
        "command_argument_type",
        "data_component_type",
        "sound_event",
        "particle_type",
    ] {
        names.push(format!(
            "  \"{registry}\": {}",
            names_json(&ids[&format!("minecraft:{registry}")])?
        ));
    }

    std::fs::create_dir_all(out)?;
    std::fs::write(out.join("registries.json"), registries_json(&synced))?;
    std::fs::write(out.join("tags.json"), tags_json(&tags))?;
    std::fs::write(out.join("blocks.json"), blocks)?;
    std::fs::write(out.join("items.json"), items)?;
    std::fs::write(out.join("loot_tables.json"), loot_tables)?;
    std::fs::write(out.join("entities.json"), entities)?;
    std::fs::write(out.join("recipes.json"), recipes)?;
    std::fs::write(out.join("features.json"), features)?;
    std::fs::write(
        out.join("ids.json"),
        format!("{{\n{}\n}}\n", names.join(",\n")),
    )?;
    let tag_count: usize = tags.values().map(BTreeMap::len).sum();
    println!(
        "{} synchronized registries, {tag_count} tags in {} registries",
        synced.len(),
        tags.len()
    );
    Ok(())
}

/// The tree features saplings grow into (`TreeGrower` of 26.3, copied from
/// the vanilla source like [`SYNCED_REGISTRIES`]).
const SAPLING_TREES: &[&str] = &[
    "oak",
    "fancy_oak",
    "oak_bees_005",
    "fancy_oak_bees_005",
    "spruce",
    "mega_spruce",
    "mega_pine",
    "birch",
    "birch_bees_005",
    "jungle_tree_no_vine",
    "mega_jungle_tree",
    "acacia",
    "cherry",
    "cherry_bees_005",
    "dark_oak",
    "pale_oak_bonemeal",
    "mangrove",
    "tall_mangrove",
    "azalea_tree",
];

/// `{"trees": {name: feature}, "block_state_providers": {name: provider},
/// "block_transformers": {name: transformer}}`, the features and
/// providers as the data pack has them.
fn features_json(data: &Path) -> anyhow::Result<String> {
    let mut trees = serde_json::Map::new();
    for name in SAPLING_TREES {
        let path = data.join(format!("worldgen/feature/{name}.json"));
        trees.insert(format!("minecraft:{name}"), read_json(&path)?);
    }
    let mut out = serde_json::json!({ "trees": trees });
    for (key, dir) in [
        ("block_state_providers", "worldgen/block_state_provider"),
        ("block_transformers", "block_transformer"),
    ] {
        let mut entries = serde_json::Map::new();
        for entry in list_entries(&data.join(dir))? {
            let path = data
                .join(dir)
                .join(format!("{}.json", entry.trim_start_matches("minecraft:")));
            entries.insert(entry, read_json(&path)?);
        }
        out[key] = entries.into();
    }
    Ok(serde_json::to_string(&out)? + "\n")
}

fn read_json(path: &Path) -> anyhow::Result<Value> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Entry names of a data pack registry: every `.json` below `dir`, sorted.
fn list_entries(dir: &Path) -> anyhow::Result<Vec<String>> {
    let mut entries = Vec::new();
    for file in walk(dir)? {
        let rel = file.strip_prefix(dir)?.with_extension("");
        entries.push(format!("minecraft:{}", slash_path(&rel)));
    }
    entries.sort();
    Ok(entries)
}

fn walk(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in
            std::fs::read_dir(&dir).with_context(|| format!("listing {}", dir.display()))?
        {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "json") {
                files.push(path);
            }
        }
    }
    Ok(files)
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

type Tags = BTreeMap<String, BTreeMap<String, Vec<i32>>>;

/// Reads `tags/<registry path>/<tag path>.json` for every registry the client
/// knows and resolves nested tags. Tags of other registries (e.g. structures)
/// never reach the client and are skipped.
fn read_tags(dir: &Path, ids: &HashMap<String, HashMap<String, i32>>) -> anyhow::Result<Tags> {
    let mut raw: HashMap<String, HashMap<String, Value>> = HashMap::new();
    for file in walk(dir)? {
        let rel = slash_path(&file.strip_prefix(dir)?.with_extension(""));
        let parts: Vec<&str> = rel.split('/').collect();
        // The longest prefix that names a registry, e.g. `worldgen/biome`.
        let Some(split) = (1..parts.len())
            .rev()
            .find(|&k| ids.contains_key(&format!("minecraft:{}", parts[..k].join("/"))))
        else {
            continue;
        };
        let registry = format!("minecraft:{}", parts[..split].join("/"));
        let tag = format!("minecraft:{}", parts[split..].join("/"));
        raw.entry(registry)
            .or_default()
            .insert(tag, read_json(&file)?);
    }

    let mut tags = Tags::new();
    for (registry, files) in &raw {
        let resolver = Resolver {
            registry,
            files,
            ids: &ids[registry],
        };
        let resolved = files
            .keys()
            .map(|tag| Ok((tag.clone(), resolver.resolve(tag, &mut Vec::new())?)))
            .collect::<anyhow::Result<_>>()?;
        tags.insert(registry.clone(), resolved);
    }
    Ok(tags)
}

struct Resolver<'a> {
    registry: &'a str,
    files: &'a HashMap<String, Value>,
    ids: &'a HashMap<String, i32>,
}

impl Resolver<'_> {
    /// Network ids of a tag's entries in the order they are listed, nested
    /// tags expanded, duplicates dropped. Optional entries that do not exist
    /// are skipped, like vanilla's `TagLoader` does.
    fn resolve(&self, tag: &str, stack: &mut Vec<String>) -> anyhow::Result<Vec<i32>> {
        if stack.iter().any(|t| t == tag) {
            bail!("tag cycle in {}: {stack:?} -> {tag}", self.registry);
        }
        stack.push(tag.to_owned());
        let mut out: Vec<i32> = Vec::new();
        let values = self.files[tag]["values"]
            .as_array()
            .with_context(|| format!("{tag}: no values"))?;
        for value in values {
            let (entry, required) = match value {
                Value::String(s) => (s.as_str(), true),
                Value::Object(o) => (
                    o["id"].as_str().context("tag entry id")?,
                    o.get("required").and_then(Value::as_bool).unwrap_or(true),
                ),
                _ => bail!("{tag}: unexpected tag entry {value}"),
            };
            let found = if let Some(nested) = entry.strip_prefix('#') {
                if self.files.contains_key(nested) {
                    Some(self.resolve(nested, stack)?)
                } else {
                    None
                }
            } else {
                self.ids.get(entry).map(|&id| vec![id])
            };
            match found {
                Some(found) => out.extend(
                    found
                        .into_iter()
                        .filter(|id| !out.contains(id))
                        .collect::<Vec<_>>(),
                ),
                None if required => bail!("{} tag {tag}: {entry} does not exist", self.registry),
                None => {}
            }
        }
        stack.pop();
        Ok(out)
    }
}

/// `{"states": <count>, "blocks": [...]}` with one block per line:
/// `{"name": "minecraft:oak_log", "first": 139, "default": 140,
/// "properties": [["axis", ["x", "y", "z"]]]}`.
///
/// A block's states are numbered like vanilla's `StateDefinition` builds them:
/// properties sorted by name, the last one changing fastest. Every state of
/// the report is checked against that rule, so the server can compute a
/// state's properties from its id alone.
fn read_blocks(
    report: &Path,
    block_ids: &HashMap<String, i32>,
    physics: &Value,
) -> anyhow::Result<String> {
    let report = read_json(report)?;
    let mut blocks = Vec::new();
    for (name, block) in report.as_object().context("blocks report")? {
        let properties: Vec<(String, Vec<String>)> = match block["properties"].as_object() {
            Some(props) => props
                .iter()
                .map(|(key, values)| {
                    let values = values.as_array().context("property values")?;
                    let values = values
                        .iter()
                        .map(|v| v.as_str().map(str::to_owned).context("value"))
                        .collect::<anyhow::Result<_>>()?;
                    Ok((key.clone(), values))
                })
                .collect::<anyhow::Result<_>>()?,
            None => Vec::new(),
        };
        let states = block["states"].as_array().context("block states")?;
        let first = states
            .iter()
            .filter_map(|s| s["id"].as_i64())
            .min()
            .context("block without states")?;
        let mut default = None;
        for state in states {
            let id = state["id"].as_i64().context("state id")?;
            if state["default"].as_bool() == Some(true) {
                default = Some(id);
            }
            // Mixed radix, last property fastest.
            let mut index = (id - first) as usize;
            for (key, values) in properties.iter().rev() {
                let value = &values[index % values.len()];
                index /= values.len();
                if state["properties"][key].as_str() != Some(value.as_str()) {
                    bail!("{name}: state {id} does not follow the numbering rule");
                }
            }
        }
        let count: usize = properties.iter().map(|(_, v)| v.len()).product();
        if count != states.len() {
            bail!("{name}: {} states, expected {count}", states.len());
        }
        blocks.push((
            first,
            name.clone(),
            default.context("no default state")?,
            properties,
        ));
    }
    blocks.sort();
    // State ids are handed out in registration order, so sorting by the first
    // state also sorts by block id; the server relies on that.
    for (index, (_, name, ..)) in blocks.iter().enumerate() {
        if block_ids.get(name) != Some(&(index as i32)) {
            bail!("{name} is not block number {index}");
        }
    }

    let states = blocks
        .last()
        .map(|(first, _, _, props)| {
            *first as usize + props.iter().map(|(_, v)| v.len()).product::<usize>()
        })
        .unwrap_or(0);
    let lines: Vec<String> = blocks
        .iter()
        .map(|(first, name, default, properties)| {
            let block = &physics["blocks"][name];
            let count: usize = properties.iter().map(|(_, v)| v.len()).product();
            let physics_states = block["states"].as_array().context("physics states")?;
            if physics_states.len() != 1 && physics_states.len() != count {
                bail!(
                    "{name}: physics for {} of {count} states",
                    physics_states.len()
                );
            }
            let mut line = serde_json::json!({
                "name": name,
                "first": first,
                "default": default,
                "properties": properties,
                "item": block["item"],
                "hardness": block["hardness"],
                "requires_tool": block["requires_tool"],
                "explosion_resistance": block["explosion_resistance"],
                "classes": block["classes"],
                "states": block["states"],
            });
            if let Some(entity) = block["block_entity"].as_str() {
                line["block_entity"] = entity.into();
            }
            if !block["fire"].is_null() {
                line["fire"] = block["fire"].clone();
            }
            Ok(format!("    {line}"))
        })
        .collect::<anyhow::Result<_>>()?;
    Ok(format!(
        "{{\n  \"states\": {states},\n  \"collision_shapes\": {},\n  \"face_shapes\": {},\n  \"blocks\": [\n{}\n  ]\n}}\n",
        physics["collision_shapes"],
        physics["face_shapes"],
        lines.join(",\n")
    ))
}

/// A registry's names, one per line, in network-id order.
fn names_json(ids: &HashMap<String, i32>) -> anyhow::Result<String> {
    let mut entries: Vec<(&i32, &String)> = ids.iter().map(|(n, id)| (id, n)).collect();
    entries.sort();
    for (index, (id, name)) in entries.iter().enumerate() {
        if **id != index as i32 {
            bail!("ids have a gap before {name}");
        }
    }
    let lines: Vec<String> = entries.iter().map(|(_, n)| format!("  \"{n}\"")).collect();
    Ok(format!("[\n{}\n]\n", lines.join(",\n")))
}

fn registries_json(synced: &[(String, Vec<String>)]) -> String {
    let mut out = String::from("[\n");
    for (i, (registry, entries)) in synced.iter().enumerate() {
        let entries: Vec<String> = entries.iter().map(|e| format!("\"{e}\"")).collect();
        let _ = write!(
            out,
            "  {{ \"registry\": \"{registry}\", \"entries\": [{}] }}",
            entries.join(", ")
        );
        out += if i + 1 < synced.len() { ",\n" } else { "\n" };
    }
    out + "]\n"
}

/// One line per tag so that version bumps give readable diffs.
fn tags_json(registries: &Tags) -> String {
    let mut out = String::from("{\n");
    for (r, (registry, tags)) in registries.iter().enumerate() {
        let _ = writeln!(out, "  \"{registry}\": {{");
        for (t, (tag, ids)) in tags.iter().enumerate() {
            let ids: Vec<String> = ids.iter().map(i32::to_string).collect();
            let comma = if t + 1 < tags.len() { "," } else { "" };
            let _ = writeln!(out, "    \"{tag}\": [{}]{comma}", ids.join(", "));
        }
        out += if r + 1 < registries.len() {
            "  },\n"
        } else {
            "  }\n"
        };
    }
    out + "}\n"
}

/// One item per line, in network-id order, with the default components the
/// server uses: `{"name", "stack", "max_damage"?, "tool"?, "food"?,
/// "consume_seconds"?, "attack_damage"?, "attack_speed"?, "armor"?,
/// "armor_toughness"?, "fuel"?}`.
fn items_json(
    item_ids: &HashMap<String, i32>,
    components: &Path,
    data: &Path,
) -> anyhow::Result<String> {
    let mut items: Vec<(&i32, &String)> = item_ids.iter().map(|(n, id)| (id, n)).collect();
    items.sort();
    let mut lines = Vec::new();
    for (index, (id, name)) in items.iter().enumerate() {
        if **id != index as i32 {
            bail!("item ids have a gap before {name}");
        }
        let path = components.join(format!("{}.json", name.trim_start_matches("minecraft:")));
        let report = if path.exists() {
            read_json(&path)?
        } else {
            Value::Null
        };
        let c = &report["components"];
        let mut line = serde_json::json!({
            "name": name,
            "stack": c["minecraft:max_stack_size"].as_i64().unwrap_or(64),
        });
        if let Some(damage) = c["minecraft:max_damage"].as_i64() {
            line["max_damage"] = damage.into();
        }
        if let Some(transformer) = c["minecraft:block_transformer"].as_str() {
            line["block_transformer"] = transformer.into();
        }
        if c["minecraft:tool"].is_object() {
            line["tool"] = c["minecraft:tool"].clone();
        }
        if c["minecraft:food"].is_object() {
            line["food"] = c["minecraft:food"].clone();
        }
        // `attribute_modifiers` of weapons and armor: what they add to the
        // wearer's attack damage and speed, armor and toughness.
        for (attribute, key) in [
            ("minecraft:attack_damage", "attack_damage"),
            ("minecraft:attack_speed", "attack_speed"),
            ("minecraft:armor", "armor"),
            ("minecraft:armor_toughness", "armor_toughness"),
        ] {
            let bonus: f64 = c["minecraft:attribute_modifiers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| m["type"] == attribute && m["operation"] == "add_value")
                .filter_map(|m| m["amount"].as_f64())
                .sum();
            if bonus != 0.0 {
                line[key] = ((bonus * 1e6).round() / 1e6).into();
            }
        }
        if c["minecraft:consumable"].is_object() {
            line["consume_seconds"] = c["minecraft:consumable"]["consume_seconds"]
                .as_f64()
                .unwrap_or(1.6)
                .into();
        }
        // `cooking_fuel`: its burn time and the speed it gives, resolved for a
        // furnace and for the fast smoker and blast furnace.
        let fuel = &c["minecraft:cooking_fuel"];
        if fuel.is_object() {
            let mut resolved = Vec::new();
            for fast in [false, true] {
                let ticks = resolve_number(data, "context_int_provider", &fuel["burn_time"], fast)
                    .with_context(|| format!("burn time of {name}"))?;
                let speed = match &fuel["speed_multiplier"] {
                    Value::Null => 1.0,
                    speed => resolve_number(data, "context_float_provider", speed, fast)
                        .with_context(|| format!("cooking speed of {name}"))?,
                };
                resolved.push(serde_json::json!([ticks.floor() as i64, speed]));
            }
            line["fuel"] = resolved.into();
        }
        lines.push(format!("  {line}"));
    }
    Ok(format!("[\n{}\n]\n", lines.join(",\n")))
}

/// The value of a context number provider (`cooking/time_coal`) in a
/// furnace, or with `fast` in a smoker or blast furnace: constants,
/// references, quotients and the `block/fast_cooking` condition, which is
/// all the cooking fuels use.
fn resolve_number(data: &Path, registry: &str, value: &Value, fast: bool) -> anyhow::Result<f64> {
    match value {
        Value::Number(n) => n.as_f64().context("number"),
        Value::String(name) => {
            let path = data
                .join(registry)
                .join(format!("{}.json", name.trim_start_matches("minecraft:")));
            resolve_number(data, registry, &read_json(&path)?, fast)
        }
        Value::Object(o) => match o.get("type").and_then(Value::as_str) {
            Some("minecraft:div") => {
                let left = resolve_number(data, registry, &o["left"], fast)?;
                let right = resolve_number(data, registry, &o["right"], fast)?;
                // `Quotient` of two ints divides as Java does.
                Ok((left / right).trunc())
            }
            Some("minecraft:conditional") => {
                let condition = o["condition"].as_str().unwrap_or("");
                if condition != "minecraft:block/fast_cooking" {
                    bail!("unknown condition {condition}");
                }
                let branch = if fast { &o["on_true"] } else { &o["on_false"] };
                resolve_number(data, registry, branch, fast)
            }
            Some("minecraft:constant") => o["value"].as_f64().context("constant"),
            other => bail!("unknown number provider {other:?}"),
        },
        other => bail!("unknown number {other}"),
    }
}

/// `{"tables": {"minecraft:blocks/stone": <table>, "minecraft:entities/cow":
/// ...}, "predicates": {...}}`, one table per line.
fn loot_tables_json(data: &Path) -> anyhow::Result<String> {
    let mut parts = Vec::new();
    for (key, dirs) in [
        ("tables", &["loot_table/blocks", "loot_table/entities"][..]),
        ("predicates", &["predicate"][..]),
    ] {
        let mut lines = Vec::new();
        for dir in dirs {
            let root = data.join(if key == "tables" { "loot_table" } else { dir });
            let dir = data.join(dir);
            for path in walk(&dir)? {
                let name = slash_path(path.strip_prefix(&root)?.with_extension("").as_path());
                lines.push(format!("    \"minecraft:{name}\": {}", read_json(&path)?));
            }
        }
        lines.sort();
        parts.push(format!("  \"{key}\": {{\n{}\n  }}", lines.join(",\n")));
    }
    Ok(format!("{{\n{}\n}}\n", parts.join(",\n")))
}

/// Attributes the server uses, from `entity_info.json`.
const ENTITY_ATTRIBUTES: &[&str] = &[
    "max_health",
    "movement_speed",
    "step_height",
    "gravity",
    "knockback_resistance",
    "safe_fall_distance",
    "attack_damage",
    "follow_range",
    "armor",
];

/// One entity type per line, in network-id order: `{"name", "width",
/// "height", "eye_height", "tracking_range", "update_interval", "category",
/// <attributes>?}`.
fn entities_json(entity_ids: &HashMap<String, i32>, info: &Value) -> anyhow::Result<String> {
    let mut types: Vec<(&i32, &String)> = entity_ids.iter().map(|(n, id)| (id, n)).collect();
    types.sort();
    let mut lines = Vec::new();
    for (_, name) in types {
        let entity = &info[name.as_str()];
        let mut line = serde_json::json!({ "name": name });
        for key in [
            "width",
            "height",
            "eye_height",
            "tracking_range",
            "update_interval",
            "category",
        ] {
            line[key] = entity[key].clone();
        }
        for &attribute in ENTITY_ATTRIBUTES {
            if let Some(value) = entity["attributes"][attribute].as_f64() {
                line[attribute] = ((value * 1e6).round() / 1e6).into();
            }
        }
        lines.push(format!("  {line}"));
    }
    Ok(format!("[\n{}\n]\n", lines.join(",\n")))
}

/// The shaped and shapeless crafting recipes and the cooking ones, one per
/// line.
fn recipes_json(dir: &Path) -> anyhow::Result<String> {
    let mut lines = Vec::new();
    for path in walk(dir)? {
        let recipe = read_json(&path)?;
        let kind = recipe["type"].as_str().unwrap_or("");
        if matches!(
            kind,
            "minecraft:crafting_shaped"
                | "minecraft:crafting_shapeless"
                | "minecraft:smelting"
                | "minecraft:blasting"
                | "minecraft:smoking"
                | "minecraft:campfire_cooking"
        ) {
            let name = slash_path(path.strip_prefix(dir)?.with_extension("").as_path());
            lines.push(format!("  \"minecraft:{name}\": {recipe}"));
        }
    }
    lines.sort();
    Ok(format!("{{\n{}\n}}\n", lines.join(",\n")))
}
