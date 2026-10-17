//! What enchantments do (`Enchantment`, `EnchantmentHelper`), read from
//! their definitions in vanilla's data pack: effect components such as
//! `minecraft:damage` or `minecraft:post_attack`, each with conditions on
//! the victim, the attacker and the damage, and values that grow with the
//! level. This interprets the parts Ferrocraft has a use for.

use std::sync::Arc;

use ferrocraft_protocol::packets::play::{ItemStack, equipment_slot};
use rand::Rng;
use serde_json::Value;

use super::Game;
use super::attributes::slot_in_group;
use super::effect::{EffectInstance, Living};
use crate::data::GameData;

/// `LevelBasedValue.calculate`; a plain number is a constant.
pub fn level_value(value: &Value, level: i32) -> f32 {
    match value {
        Value::Number(n) => n.as_f64().unwrap_or(0.0) as f32,
        Value::Object(o) => {
            let f = |key: &str| o.get(key).map_or(0.0, |v| level_value(v, level));
            match o.get("type").and_then(Value::as_str).unwrap_or("") {
                "minecraft:linear" => {
                    let base = o.get("base").and_then(Value::as_f64).unwrap_or(0.0) as f32;
                    let per = o
                        .get("per_level_above_first")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0) as f32;
                    base + per * (level - 1) as f32
                }
                "minecraft:levels_squared" => {
                    (level * level) as f32
                        + o.get("added").and_then(Value::as_f64).unwrap_or(0.0) as f32
                }
                "minecraft:clamped" => {
                    let min = o.get("min").and_then(Value::as_f64).unwrap_or(f64::MIN) as f32;
                    let max = o.get("max").and_then(Value::as_f64).unwrap_or(f64::MAX) as f32;
                    f("value").clamp(min, max)
                }
                "minecraft:fraction" => {
                    let denominator = f("denominator");
                    if denominator == 0.0 {
                        0.0
                    } else {
                        f("numerator") / denominator
                    }
                }
                "minecraft:exponent" => f("base").powf(f("power")),
                "minecraft:lookup" => {
                    let values = o.get("values").and_then(Value::as_array);
                    match values.and_then(|v| v.get(usize::try_from(level - 1).ok()?)) {
                        Some(v) => v.as_f64().unwrap_or(0.0) as f32,
                        None => f("fallback"),
                    }
                }
                _ => o.get("value").and_then(Value::as_f64).unwrap_or(0.0) as f32,
            }
        }
        _ => 0.0,
    }
}

/// `EnchantmentValueEffect.process`.
pub fn apply_value(effect: &Value, level: i32, value: f32, rng: &mut impl Rng) -> f32 {
    let v = |key: &str| level_value(&effect[key], level);
    match effect["type"].as_str().unwrap_or("") {
        "minecraft:add" => value + v("value"),
        "minecraft:set" => v("value"),
        "minecraft:multiply" => value * v("factor"),
        "minecraft:remove_binomial" => {
            let p = v("chance");
            let removed = (0..value.max(0.0) as i32)
                .filter(|_| rng.random::<f32>() < p)
                .count();
            value - removed as f32
        }
        "minecraft:all_of" => effect["effects"]
            .as_array()
            .into_iter()
            .flatten()
            .fold(value, |v, e| apply_value(e, level, v, rng)),
        _ => value,
    }
}

/// `Enchantment.matchingSlot`: one of its slot groups holds the slot.
pub fn applies_in(enchantment: &Value, slot: u8) -> bool {
    enchantment["slots"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|group| slot_in_group(slot, group))
}

/// Whether an item is in a holder set of items: a name, `#tag` or a list.
pub fn item_in(data: &GameData, set: &Value, item: i32) -> bool {
    match set {
        Value::String(s) => match s.strip_prefix('#') {
            Some(tag) => data.item_tag(tag).is_some_and(|ids| ids.contains(&item)),
            None => data.item_id(s) == Some(item),
        },
        Value::Array(list) => list.iter().any(|s| item_in(data, s, item)),
        _ => false,
    }
}

/// `Enchantment.isSupportedItem` (`canEnchant`).
pub fn supports(data: &GameData, enchantment: &Value, item: i32) -> bool {
    item_in(data, &enchantment["supported_items"], item)
}

/// `Enchantment.isPrimaryItem`: what an enchanting table offers it for.
pub fn primary_for(data: &GameData, enchantment: &Value, item: i32) -> bool {
    supports(data, enchantment, item)
        && match &enchantment["primary_items"] {
            Value::Null => true,
            set => item_in(data, set, item),
        }
}

/// Whether the enchantment is in an enchantment tag (`#minecraft:curse`).
pub fn in_tag(data: &GameData, name: &str, tag: &str) -> bool {
    data.registry_id("minecraft:enchantment", name)
        .is_ok_and(|id| data.tag_contains("minecraft:enchantment", tag, id))
}

/// `Enchantment.areCompatible`: different, and neither's exclusive set
/// holds the other.
pub fn compatible(data: &GameData, a: &str, b: &str) -> bool {
    let excludes = |x: &str, y: &str| {
        let Some((_, json)) = data.enchantment(x) else {
            return false;
        };
        match &json["exclusive_set"] {
            Value::String(set) => match set.strip_prefix('#') {
                Some(tag) => in_tag(data, y, tag),
                None => set == y,
            },
            Value::Array(list) => list.iter().any(|s| s == y),
            _ => false,
        }
    };
    a != b && !excludes(a, b) && !excludes(b, a)
}

/// Whether an enchantment on the item has an effect component
/// (`EnchantmentHelper.has`), such as `minecraft:prevent_armor_change`.
pub fn has_effect(data: &GameData, stack: &ItemStack, component: &str) -> bool {
    crate::stack::enchantments(data, stack)
        .into_iter()
        .any(|(name, _)| {
            data.enchantment(&name)
                .is_some_and(|(_, json)| !json["effects"][component].is_null())
        })
}

/// The cost range of a level (`Enchantment.Cost`).
pub fn cost(enchantment: &Value, key: &str, level: i32) -> i32 {
    let c = &enchantment[key];
    let base = c["base"].as_i64().unwrap_or(0) as i32;
    let per = c["per_level_above_first"].as_i64().unwrap_or(0) as i32;
    base + per * (level - 1)
}

pub fn max_level(enchantment: &Value) -> i32 {
    enchantment["max_level"].as_i64().unwrap_or(1) as i32
}

/// A `minecraft:post_attack` effect to run: the effect, the level, the
/// enchanted item, and who holds it in which slot.
type PostAttack = (Value, i32, ItemStack, Option<(Living, u8)>);

/// What the conditions of an effect may ask about (the parameters of
/// `LootContextParamSets.ENCHANTED_*`).
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Context<'a> {
    pub level: i32,
    /// The enchanted item (`TOOL`).
    pub tool: Option<&'a ItemStack>,
    /// `THIS_ENTITY`: the victim of a hit, or who wears the item.
    pub this: Option<i32>,
    pub attacker: Option<i32>,
    /// What hit: the arrow, or the attacker itself.
    pub direct: Option<i32>,
    /// The `minecraft:damage_type` of the hit.
    pub damage_type: Option<&'a str>,
}

impl Game {
    /// The entity type of anything in the game, by entity id.
    pub(super) fn entity_type_name(&self, entity_id: i32) -> Option<&'static str> {
        if self.players.contains_key(&entity_id) {
            return Some("minecraft:player");
        }
        if let Some(mob) = self.mobs.get(&entity_id) {
            return Some(mob.kind.name());
        }
        if self.arrows.contains_key(&entity_id) {
            return Some("minecraft:arrow");
        }
        if self.tnts.contains_key(&entity_id) {
            return Some("minecraft:tnt");
        }
        if self.items.contains_key(&entity_id) {
            return Some("minecraft:item");
        }
        None
    }

    /// `EntityPredicate`, for the parts enchantments use: the type and a
    /// few flags.
    fn entity_matches(&self, entity_id: i32, predicate: &Value) -> bool {
        let data = &self.context.data;
        let Some(type_name) = self.entity_type_name(entity_id) else {
            return false;
        };
        let Value::Object(checks) = predicate else {
            return true;
        };
        checks.iter().all(|(key, wanted)| match key.as_str() {
            "minecraft:entity_type" | "type" => match wanted {
                Value::String(s) => match s.strip_prefix('#') {
                    Some(tag) => data
                        .entity_type(type_name)
                        .is_ok_and(|id| data.tag_contains("minecraft:entity_type", tag, id)),
                    None => s == type_name,
                },
                Value::Array(list) => list.iter().any(|s| s == type_name),
                _ => false,
            },
            "flags" | "minecraft:flags" => wanted.as_object().is_some_and(|flags| {
                flags.iter().all(|(flag, value)| {
                    let actual = match flag.as_str() {
                        "is_on_fire" => self.on_fire(entity_id),
                        "is_sneaking" => self.players.get(&entity_id).is_some_and(|p| p.sneaking),
                        "is_sprinting" => self.players.get(&entity_id).is_some_and(|p| p.sprinting),
                        "is_baby" => self.mobs.get(&entity_id).is_some_and(|m| m.is_baby()),
                        _ => false,
                    };
                    value.as_bool() == Some(actual)
                })
            }),
            _ => false,
        })
    }

    pub(super) fn on_fire(&self, entity_id: i32) -> bool {
        self.players
            .get(&entity_id)
            .map(|p| p.fire > 0)
            .or_else(|| self.mobs.get(&entity_id).map(|m| m.fire > 0))
            .unwrap_or(false)
    }

    /// A loot condition in an enchantment's context.
    pub(super) fn enchant_condition(
        &self,
        condition: &Value,
        context: &Context,
        rng: &mut impl Rng,
    ) -> bool {
        let data = &self.context.data;
        match condition["type"].as_str().unwrap_or("") {
            "minecraft:all_of" => condition["terms"]
                .as_array()
                .into_iter()
                .flatten()
                .all(|t| self.enchant_condition(t, context, rng)),
            "minecraft:any_of" => condition["terms"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|t| self.enchant_condition(t, context, rng)),
            "minecraft:inverted" => !self.enchant_condition(&condition["term"], context, rng),
            "minecraft:random_chance" => {
                let chance = match &condition["chance"] {
                    Value::Object(o)
                        if o.get("type").and_then(Value::as_str)
                            == Some("minecraft:enchantment_level") =>
                    {
                        level_value(o.get("amount").unwrap_or(&Value::Null), context.level)
                    }
                    other => level_value(other, context.level),
                };
                rng.random::<f32>() < chance
            }
            "minecraft:entity_properties" => {
                let entity = match condition["entity"].as_str().unwrap_or("this") {
                    "this" => context.this,
                    "attacker" => context.attacker,
                    "direct_attacker" => context.direct,
                    _ => None,
                };
                entity.is_some_and(|e| self.entity_matches(e, &condition["predicate"]))
            }
            "minecraft:damage_source_properties" => {
                let Some(damage_type) = context.damage_type else {
                    return false;
                };
                let predicate = &condition["predicate"];
                let id = data
                    .registry_id("minecraft:damage_type", damage_type)
                    .unwrap_or(-1);
                let tags_hold = predicate["tags"].as_array().into_iter().flatten().all(|t| {
                    let tag = t["id"].as_str().unwrap_or("").trim_start_matches('#');
                    let expected = t["expected"].as_bool().unwrap_or(true);
                    data.tag_contains("minecraft:damage_type", tag, id) == expected
                });
                // `DamageSource.isDirect`: the attacker hit itself.
                let direct = context.attacker == context.direct;
                let is_direct = predicate["is_direct"]
                    .as_bool()
                    .is_none_or(|wanted| wanted == direct);
                let source = predicate
                    .get("source_entity")
                    .is_none_or(|p| context.attacker.is_some_and(|a| self.entity_matches(a, p)));
                let direct_entity = predicate
                    .get("direct_entity")
                    .is_none_or(|p| context.direct.is_some_and(|d| self.entity_matches(d, p)));
                tags_hold && is_direct && source && direct_entity
            }
            "minecraft:match_tool" => {
                let Some(tool) = context.tool else {
                    return false;
                };
                let predicate = &condition["predicate"];
                predicate
                    .get("items")
                    .is_none_or(|set| item_in(data, set, tool.item))
            }
            "minecraft:weather_check" => {
                let raining = self.weather.raining;
                let thundering = self.weather.thundering;
                condition["raining"].as_bool().is_none_or(|r| r == raining)
                    && condition["thundering"]
                        .as_bool()
                        .is_none_or(|t| t == thundering)
            }
            // Places, active location effects: nothing Ferrocraft asks.
            _ => false,
        }
    }

    /// Runs the value effects of one component of one enchantment.
    fn process_value(
        &self,
        effects: &Value,
        context: &Context,
        value: f32,
        rng: &mut impl Rng,
    ) -> f32 {
        let mut value = value;
        for entry in effects.as_array().into_iter().flatten() {
            let holds = entry
                .get("requirements")
                .is_none_or(|r| self.enchant_condition(r, context, rng));
            if holds {
                value = apply_value(&entry["effect"], context.level, value, rng);
            }
        }
        value
    }

    /// `runIterationOnItem` for a value component: every enchantment on the
    /// item, wherever it is.
    pub(super) fn item_value(
        &self,
        stack: &ItemStack,
        component: &str,
        context: Context,
        value: f32,
    ) -> f32 {
        let data = Arc::clone(&self.context.data);
        let mut rng = rand::rng();
        let mut value = value;
        for (name, level) in crate::stack::enchantments(&data, stack) {
            let Some((_, json)) = data.enchantment(&name) else {
                continue;
            };
            let effects = &json["effects"][component];
            if effects.is_null() {
                continue;
            }
            let context = Context {
                level,
                tool: Some(stack),
                ..context
            };
            value = self.process_value(effects, &context, value, &mut rng);
        }
        value
    }

    /// `runIterationOnEquipment` for a value component: the enchantments of
    /// everything the entity wears, where they count.
    pub(super) fn equipment_value(
        &self,
        target: Living,
        component: &str,
        context: Context,
        value: f32,
    ) -> f32 {
        let data = Arc::clone(&self.context.data);
        let mut rng = rand::rng();
        let mut value = value;
        for (slot, stack) in self.equipment_of(target) {
            for (name, level) in crate::stack::enchantments(&data, &stack) {
                let Some((_, json)) = data.enchantment(&name) else {
                    continue;
                };
                let effects = &json["effects"][component];
                if effects.is_null() || !applies_in(json, slot) {
                    continue;
                }
                let context = Context {
                    level,
                    tool: Some(&stack),
                    ..context
                };
                value = self.process_value(effects, &context, value, &mut rng);
            }
        }
        value
    }

    /// `EnchantmentHelper.getDamageProtection`: the protection of a
    /// victim's armor against a hit.
    pub(super) fn damage_protection(
        &self,
        victim: Living,
        damage_type: &str,
        attacker: Option<i32>,
        direct: Option<i32>,
    ) -> f32 {
        self.equipment_value(
            victim,
            "minecraft:damage_protection",
            Context {
                this: Some(victim.id()),
                attacker,
                direct,
                damage_type: Some(damage_type),
                ..Context::default()
            },
            0.0,
        )
    }

    /// `EnchantmentHelper.isImmuneToDamage` (frost walker on magma).
    pub(super) fn immune_by_enchantment(&self, victim: Living, damage_type: &str) -> bool {
        let data = Arc::clone(&self.context.data);
        let mut rng = rand::rng();
        self.equipment_of(victim).iter().any(|(slot, stack)| {
            crate::stack::enchantments(&data, stack)
                .into_iter()
                .any(|(name, level)| {
                    let Some((_, json)) = data.enchantment(&name) else {
                        return false;
                    };
                    let context = Context {
                        level,
                        tool: Some(stack),
                        this: Some(victim.id()),
                        damage_type: Some(damage_type),
                        ..Context::default()
                    };
                    applies_in(json, *slot)
                        && json["effects"]["minecraft:damage_immunity"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|e| {
                                e.get("requirements")
                                    .is_none_or(|r| self.enchant_condition(r, &context, &mut rng))
                            })
                })
        })
    }

    /// `EnchantmentHelper.modifyDamage`: sharpness, smite, power, ...
    pub(super) fn enchanted_damage(
        &self,
        weapon: &ItemStack,
        victim: i32,
        damage_type: &str,
        attacker: Option<i32>,
        direct: Option<i32>,
        damage: f32,
    ) -> f32 {
        self.item_value(
            weapon,
            "minecraft:damage",
            Context {
                this: Some(victim),
                attacker,
                direct,
                damage_type: Some(damage_type),
                ..Context::default()
            },
            damage,
        )
    }

    /// `EnchantmentHelper.modifyKnockback`: knockback, punch.
    pub(super) fn enchanted_knockback(
        &self,
        weapon: &ItemStack,
        victim: i32,
        damage_type: &str,
        attacker: Option<i32>,
        knockback: f32,
    ) -> f32 {
        self.item_value(
            weapon,
            "minecraft:knockback",
            Context {
                this: Some(victim),
                attacker,
                direct: attacker,
                damage_type: Some(damage_type),
                ..Context::default()
            },
            knockback,
        )
    }

    /// `EnchantmentHelper.processDurabilityChange`: unbreaking spares the
    /// item some of the wear.
    pub(super) fn durability_change(&self, stack: &ItemStack, amount: i32) -> i32 {
        if amount <= 0 {
            return amount;
        }
        self.item_value(
            stack,
            "minecraft:item_damage",
            Context::default(),
            amount as f32,
        ) as i32
    }

    /// `EnchantmentHelper.processAmmoUse`: infinity keeps the arrow.
    pub(super) fn ammo_use(&self, weapon: &ItemStack, ammo: &ItemStack, amount: i32) -> i32 {
        let data = Arc::clone(&self.context.data);
        let mut rng = rand::rng();
        let mut value = amount as f32;
        for (name, level) in crate::stack::enchantments(&data, weapon) {
            let Some((_, json)) = data.enchantment(&name) else {
                continue;
            };
            // The conditions ask about the ammunition (`itemContext`).
            let context = Context {
                level,
                tool: Some(ammo),
                ..Context::default()
            };
            value = self.process_value(
                &json["effects"]["minecraft:ammo_use"],
                &context,
                value,
                &mut rng,
            );
        }
        value as i32
    }

    /// `EnchantmentHelper.processBlockExperience`: silk touch gives none.
    pub(super) fn block_experience(&self, tool: &ItemStack, amount: i32) -> i32 {
        self.item_value(
            tool,
            "minecraft:block_experience",
            Context::default(),
            amount as f32,
        ) as i32
    }

    /// `EnchantmentHelper.modifyDurabilityToRepairFromXp`: mending.
    pub(super) fn repair_from_xp(&self, stack: &ItemStack, durability: i32) -> i32 {
        (self.item_value(
            stack,
            "minecraft:repair_with_xp",
            Context::default(),
            durability as f32,
        ) as i32)
            .max(0)
    }

    /// Whether any enchantment on the item has an effect component, such
    /// as `minecraft:prevent_equipment_drop` (curse of vanishing).
    pub(super) fn has_enchantment_effect(&self, stack: &ItemStack, component: &str) -> bool {
        has_effect(&self.context.data, stack, component)
    }

    /// `EnchantmentHelper.doPostAttackEffects`: what a hit sets off: the
    /// victim's thorns, the weapon's fire aspect or bane of arthropods.
    pub(super) fn post_attack(
        &mut self,
        victim: Living,
        attacker: Option<i32>,
        direct: Option<i32>,
        damage_type: &str,
    ) {
        let data = Arc::clone(&self.context.data);
        let mut rng = rand::rng();
        let mut runs: Vec<PostAttack> = Vec::new();
        // The victim's equipment first, as vanilla.
        for (slot, stack) in self.equipment_of(victim) {
            for (name, level) in crate::stack::enchantments(&data, &stack) {
                if let Some((_, json)) = data.enchantment(&name)
                    && applies_in(json, slot)
                {
                    for effect in json["effects"]["minecraft:post_attack"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|e| e["enchanted"] == "victim")
                    {
                        runs.push((effect.clone(), level, stack.clone(), Some((victim, slot))));
                    }
                }
            }
        }
        // The attacker's weapon, if it hit with its own hand or a shot.
        let attacker_living = attacker.and_then(|a| self.living(a));
        if let Some(attacker_living) = attacker_living
            && let Some((_, weapon)) = self
                .equipment_of(attacker_living)
                .into_iter()
                .find(|(slot, _)| *slot == equipment_slot::MAIN_HAND)
        {
            for (name, level) in crate::stack::enchantments(&data, &weapon) {
                if let Some((_, json)) = data.enchantment(&name)
                    && applies_in(json, equipment_slot::MAIN_HAND)
                {
                    for effect in json["effects"]["minecraft:post_attack"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|e| e["enchanted"] == "attacker")
                    {
                        runs.push((
                            effect.clone(),
                            level,
                            weapon.clone(),
                            Some((attacker_living, equipment_slot::MAIN_HAND)),
                        ));
                    }
                }
            }
        }
        for (effect, level, stack, owner) in runs {
            let context = Context {
                level,
                tool: Some(&stack),
                this: Some(victim.id()),
                attacker,
                direct,
                damage_type: Some(damage_type),
            };
            let holds = effect
                .get("requirements")
                .is_none_or(|r| self.enchant_condition(r, &context, &mut rng));
            if !holds {
                continue;
            }
            let affected = match effect["affected"].as_str().unwrap_or("victim") {
                "attacker" => attacker,
                "damaging_entity" => direct,
                _ => Some(victim.id()),
            };
            if let Some(affected) = affected {
                self.entity_effect(&effect["effect"], level, affected, owner);
            }
        }
    }

    /// The player or mob with this id.
    pub(super) fn living(&self, entity_id: i32) -> Option<Living> {
        if self.players.contains_key(&entity_id) {
            Some(Living::Player(entity_id))
        } else if self.mobs.contains_key(&entity_id) {
            Some(Living::Mob(entity_id))
        } else {
            None
        }
    }

    /// `EnchantmentEntityEffect.apply` for the effects Ferrocraft knows:
    /// setting alight, status effects, damage, wear on the enchanted item.
    /// `owner` is who holds the item and in which slot.
    pub(super) fn entity_effect(
        &mut self,
        effect: &Value,
        level: i32,
        target: i32,
        owner: Option<(Living, u8)>,
    ) {
        let mut rng = rand::rng();
        match effect["type"].as_str().unwrap_or("") {
            "minecraft:all_of" => {
                for e in effect["effects"].as_array().into_iter().flatten() {
                    self.entity_effect(e, level, target, owner);
                }
            }
            "minecraft:ignite" => {
                let seconds = level_value(&effect["duration"], level);
                self.set_alight(target, seconds);
            }
            "minecraft:apply_mob_effect" => {
                let Some(living) = self.living(target) else {
                    return;
                };
                let names: Vec<&str> = match &effect["to_apply"] {
                    Value::String(s) => vec![s.as_str()],
                    Value::Array(list) => list.iter().filter_map(Value::as_str).collect(),
                    _ => Vec::new(),
                };
                if names.is_empty() {
                    return;
                }
                let name = names[rng.random_range(0..names.len())];
                let between = |min: f32, max: f32, rng: &mut rand::rngs::ThreadRng| {
                    if max > min {
                        rng.random_range(min..max)
                    } else {
                        min
                    }
                };
                let (min_d, max_d) = (
                    level_value(&effect["min_duration"], level),
                    level_value(&effect["max_duration"], level),
                );
                let (min_a, max_a) = (
                    level_value(&effect["min_amplifier"], level),
                    level_value(&effect["max_amplifier"], level),
                );
                let ticks = (between(min_d, max_d, &mut rng) * 20.0).round() as i32;
                let amplifier = between(min_a, max_a, &mut rng).round().max(0.0) as i32;
                if let Some((id, _)) = self.context.data.effect(name) {
                    self.add_effect(living, EffectInstance::new(id, ticks, amplifier));
                }
            }
            "minecraft:damage_entity" => {
                let Some(living) = self.living(target) else {
                    return;
                };
                let (min, max) = (
                    level_value(&effect["min_damage"], level),
                    level_value(&effect["max_damage"], level),
                );
                let amount = if max > min {
                    rng.random_range(min..max)
                } else {
                    min
                };
                let damage_type = effect["damage_type"]
                    .as_str()
                    .unwrap_or("minecraft:generic");
                let blame = owner.map(|(l, _)| l.id());
                self.hurt_living_by(living, amount, damage_type, blame);
            }
            "minecraft:change_item_damage" => {
                let amount = level_value(&effect["amount"], level) as i32;
                if let Some((Living::Player(player), slot)) = owner {
                    let menu_slot = self
                        .players
                        .get(&player)
                        .and_then(|p| equipment_menu_slot(p.inventory.selected_slot(), slot));
                    if let Some(menu_slot) = menu_slot {
                        self.wear(player, menu_slot, amount);
                    }
                }
            }
            _ => {}
        }
    }

    /// Sets a player or mob alight for some seconds (`igniteForSeconds`).
    pub(super) fn set_alight(&mut self, entity_id: i32, seconds: f32) {
        let ticks = (seconds * 20.0) as i32;
        if self.mobs.contains_key(&entity_id) {
            self.ignite_mob(entity_id, (ticks + 19) / 20);
        } else if let Some(player) = self.players.get_mut(&entity_id)
            && player.fire < ticks
        {
            player.fire = ticks;
            player.data_dirty = true;
            player.state = None;
        }
    }
}

/// The inventory menu slot of a player's equipment slot.
fn equipment_menu_slot(selected: usize, slot: u8) -> Option<usize> {
    match slot {
        equipment_slot::MAIN_HAND => Some(selected),
        equipment_slot::OFF_HAND => Some(crate::inventory::OFF_HAND),
        equipment_slot::HEAD => Some(5),
        equipment_slot::CHEST => Some(6),
        equipment_slot::LEGS => Some(7),
        equipment_slot::FEET => Some(8),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_values_like_vanilla() {
        let linear = serde_json::json!({"type": "minecraft:linear", "base": 2.5, "per_level_above_first": 2.5});
        assert_eq!(level_value(&linear, 3), 7.5);
        let squared = serde_json::json!({"type": "minecraft:levels_squared", "added": 1.0});
        assert_eq!(level_value(&squared, 4), 17.0);
        let fraction = serde_json::json!({"type": "minecraft:fraction",
            "numerator": {"type": "minecraft:linear", "base": 1.0, "per_level_above_first": 1.0},
            "denominator": {"type": "minecraft:linear", "base": 2.0, "per_level_above_first": 1.0}});
        assert!((level_value(&fraction, 3) - 0.75).abs() < 1e-6);
        assert_eq!(level_value(&serde_json::json!(5.0), 2), 5.0);
    }

    #[test]
    fn enchantments_know_their_items_and_rivals() {
        let data = GameData::load().unwrap();
        let (_, sharpness) = data.enchantment("minecraft:sharpness").unwrap();
        let sword = data.item_id("minecraft:diamond_sword").unwrap();
        let axe = data.item_id("minecraft:diamond_axe").unwrap();
        let pickaxe = data.item_id("minecraft:diamond_pickaxe").unwrap();
        assert!(primary_for(&data, sharpness, sword));
        assert!(supports(&data, sharpness, axe));
        assert!(!primary_for(&data, sharpness, axe), "only by anvil");
        assert!(!supports(&data, sharpness, pickaxe));
        assert!(!compatible(&data, "minecraft:sharpness", "minecraft:smite"));
        assert!(compatible(
            &data,
            "minecraft:sharpness",
            "minecraft:unbreaking"
        ));
        assert!(!compatible(
            &data,
            "minecraft:silk_touch",
            "minecraft:fortune"
        ));
        assert!(in_tag(&data, "minecraft:binding_curse", "minecraft:curse"));
        assert_eq!(cost(sharpness, "min_cost", 5), 1 + 11 * 4);
        assert!(applies_in(sharpness, equipment_slot::MAIN_HAND));
        assert!(!applies_in(sharpness, equipment_slot::OFF_HAND));
    }
}
