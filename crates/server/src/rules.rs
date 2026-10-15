//! Game rules (`GameRules`) as vanilla 26.3 has them: every rule with its
//! default, saved as `data/minecraft/game_rules.dat` like vanilla's
//! `GameRuleMap`. Rules Ferrocraft does not follow are still kept and
//! saved, so vanilla finds them as they were set.

use ferrocraft_protocol::Nbt;

use crate::storage::{DATA_VERSION, Storage, read_nbt_file, write_nbt_file};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    Int(i32),
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(n) => write!(f, "{n}"),
        }
    }
}

/// A rule: its name without the namespace, its default and, for numbers,
/// the bounds.
pub struct Rule {
    pub name: &'static str,
    pub default: Value,
    pub min: i32,
    pub max: i32,
}

const fn flag(name: &'static str, default: bool) -> Rule {
    Rule {
        name,
        default: Value::Bool(default),
        min: 0,
        max: 0,
    }
}

const fn int(name: &'static str, default: i32, min: i32, max: i32) -> Rule {
    Rule {
        name,
        default: Value::Int(default),
        min,
        max,
    }
}

/// `GameRules`, in vanilla's order; `max_minecart_speed` needs an
/// experiment and is left out like vanilla does without it.
pub const RULES: &[Rule] = &[
    flag("advance_time", true),
    flag("advance_weather", true),
    flag("allow_entering_nether_using_portals", true),
    flag("block_drops", true),
    flag("block_explosion_drop_decay", true),
    flag("command_blocks_work", true),
    flag("command_block_output", true),
    flag("drowning_damage", true),
    flag("elytra_movement_check", true),
    flag("ender_pearls_vanish_on_death", true),
    flag("entity_drops", true),
    flag("fall_damage", true),
    flag("fire_damage", true),
    int("fire_spread_radius_around_player", 128, -1, i32::MAX),
    flag("forgive_dead_players", true),
    flag("freeze_damage", true),
    flag("global_sound_events", true),
    flag("immediate_respawn", false),
    flag("keep_inventory", false),
    flag("lava_source_conversion", false),
    flag("limited_crafting", false),
    flag("locator_bar", true),
    flag("log_admin_commands", true),
    int("max_block_modifications", 32768, 1, i32::MAX),
    int("max_command_forks", 65536, 0, i32::MAX),
    int("max_command_sequence_length", 65536, 0, i32::MAX),
    int("max_entity_cramming", 24, 0, i32::MAX),
    int("max_snow_accumulation_height", 1, 0, 8),
    flag("mob_drops", true),
    flag("mob_explosion_drop_decay", true),
    flag("mob_griefing", true),
    flag("natural_health_regeneration", true),
    flag("player_movement_check", true),
    int("players_nether_portal_creative_delay", 0, 0, i32::MAX),
    int("players_nether_portal_default_delay", 80, 0, i32::MAX),
    int("players_sleeping_percentage", 100, 0, i32::MAX),
    flag("projectiles_can_break_blocks", true),
    flag("pvp", true),
    flag("raids", true),
    int("random_tick_speed", 3, 0, i32::MAX),
    flag("reduced_debug_info", false),
    int("respawn_radius", 10, 0, i32::MAX),
    flag("send_command_feedback", true),
    flag("show_advancement_messages", true),
    flag("show_death_messages", true),
    flag("spawner_blocks_work", true),
    flag("spawn_mobs", true),
    flag("spawn_monsters", true),
    flag("spawn_patrols", true),
    flag("spawn_phantoms", true),
    flag("spawn_wandering_traders", true),
    flag("spawn_wardens", true),
    flag("spectators_generate_chunks", true),
    flag("spread_vines", true),
    flag("tnt_explodes", true),
    flag("tnt_explosion_drop_decay", false),
    flag("universal_anger", false),
    flag("water_source_conversion", true),
];

const FILE: &str = "data/minecraft/game_rules.dat";

/// Why a value was refused, as vanilla's argument parsers say it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    Unknown,
    NotABool,
    NotANumber,
    TooLow(i32),
    TooHigh(i32),
}

/// The rules' values, one per entry of [`RULES`].
#[derive(Debug, Clone, PartialEq)]
pub struct GameRules {
    values: Vec<Value>,
    /// Rules of the file Ferrocraft does not know, kept for saving.
    unknown: Vec<(String, Nbt)>,
}

impl Default for GameRules {
    fn default() -> Self {
        Self {
            values: RULES.iter().map(|r| r.default).collect(),
            unknown: Vec::new(),
        }
    }
}

/// The rule's index, by its name with or without `minecraft:`.
fn index(name: &str) -> Option<usize> {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    RULES.iter().position(|r| r.name == name)
}

impl GameRules {
    pub fn get(&self, name: &str) -> Option<Value> {
        index(name).map(|i| self.values[i])
    }

    /// A true/false rule; panics in tests if it is a number or unknown.
    pub fn flag(&self, name: &str) -> bool {
        match self.get(name) {
            Some(Value::Bool(b)) => b,
            other => {
                debug_assert!(false, "{name} is not a flag: {other:?}");
                false
            }
        }
    }

    pub fn int(&self, name: &str) -> i32 {
        match self.get(name) {
            Some(Value::Int(n)) => n,
            other => {
                debug_assert!(false, "{name} is not a number: {other:?}");
                0
            }
        }
    }

    /// Sets a rule from the text of a command; the new value, or why not.
    pub fn set(&mut self, name: &str, text: &str) -> Result<Value, Refused> {
        let i = index(name).ok_or(Refused::Unknown)?;
        let rule = &RULES[i];
        let value = match rule.default {
            Value::Bool(_) => match text {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => return Err(Refused::NotABool),
            },
            Value::Int(_) => {
                let n: i32 = text.parse().map_err(|_| Refused::NotANumber)?;
                if n < rule.min {
                    return Err(Refused::TooLow(rule.min));
                }
                if n > rule.max {
                    return Err(Refused::TooHigh(rule.max));
                }
                Value::Int(n)
            }
        };
        self.values[i] = value;
        Ok(value)
    }

    pub fn names() -> impl Iterator<Item = &'static str> {
        RULES.iter().map(|r| r.name)
    }

    /// Reads `game_rules.dat`; defaults for what it does not say.
    pub fn load(storage: &Storage) -> anyhow::Result<Self> {
        let mut rules = Self::default();
        let Some(root) = read_nbt_file(&storage.path(FILE))? else {
            return Ok(rules);
        };
        let Some(Nbt::Compound(entries)) = root.get("data") else {
            return Ok(rules);
        };
        for (key, value) in entries {
            let Some(i) = index(key) else {
                rules.unknown.push((key.clone(), value.clone()));
                continue;
            };
            rules.values[i] = match (RULES[i].default, value) {
                (Value::Bool(_), Nbt::Byte(b)) => Value::Bool(*b != 0),
                (Value::Int(_), Nbt::Int(n)) => Value::Int(*n),
                _ => continue,
            };
        }
        Ok(rules)
    }

    pub fn save(&self, storage: &Storage) -> anyhow::Result<()> {
        let mut entries: Vec<(String, Nbt)> = RULES
            .iter()
            .zip(&self.values)
            .map(|(rule, value)| {
                let nbt = match value {
                    Value::Bool(b) => Nbt::Byte(i8::from(*b)),
                    Value::Int(n) => Nbt::Int(*n),
                };
                (format!("minecraft:{}", rule.name), nbt)
            })
            .collect();
        entries.extend(self.unknown.iter().cloned());
        let nbt = Nbt::Compound(vec![
            ("data".into(), Nbt::Compound(entries)),
            ("DataVersion".into(), Nbt::Int(DATA_VERSION)),
        ]);
        write_nbt_file(&storage.path(FILE), &nbt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_parse_and_keep_their_bounds() {
        let mut rules = GameRules::default();
        assert_eq!(rules.int("random_tick_speed"), 3);
        assert!(rules.flag("minecraft:advance_weather"));
        assert_eq!(rules.set("keep_inventory", "true"), Ok(Value::Bool(true)));
        assert!(rules.set("max_snow_accumulation_height", "9").is_err());
        assert!(rules.set("random_tick_speed", "drei").is_err());
        assert!(rules.set("does_not_exist", "1").is_err());
    }

    #[test]
    fn rules_survive_a_save() {
        let dir = std::env::temp_dir().join(format!("ferrocraft-rules-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let storage = Storage::open(&dir).unwrap();
        let mut rules = GameRules::default();
        rules.set("random_tick_speed", "100").unwrap();
        rules.set("advance_weather", "false").unwrap();
        rules.save(&storage).unwrap();
        assert_eq!(GameRules::load(&storage).unwrap(), rules);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
