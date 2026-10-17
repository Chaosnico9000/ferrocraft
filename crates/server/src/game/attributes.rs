//! Attributes (`AttributeInstance`): a base value per entity, changed by
//! modifiers from what it holds and wears (the `attribute_modifiers`
//! component), its enchantments (`minecraft:attributes` effects) and its
//! status effects. Clients compute movement and digging themselves, so a
//! player's client hears of the attributes it may sync, and of every
//! change to them.

use std::collections::BTreeMap;
use std::sync::Arc;

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::packets::play::{
    AttributeModifier, AttributeSnapshot, AttributeSnapshots, ItemStack, UpdateAttributes,
    equipment_slot,
};

use super::effect::Living;
use super::enchant;
use super::{Encoded, Game, Outbound};
use crate::data::{AttributeInfo, GameData};

/// `AttributeModifier.Operation` ids.
pub const ADD_VALUE: i32 = 0;
pub const ADD_MULTIPLIED_BASE: i32 = 1;
pub const ADD_MULTIPLIED_TOTAL: i32 = 2;

pub fn operation(name: &str) -> i32 {
    match name {
        "add_multiplied_base" => ADD_MULTIPLIED_BASE,
        "add_multiplied_total" => ADD_MULTIPLIED_TOTAL,
        _ => ADD_VALUE,
    }
}

/// `AttributeInstance.calculateValue`.
pub fn value(base: f64, modifiers: &[AttributeModifier], info: &AttributeInfo) -> f64 {
    let mut base = base;
    for m in modifiers.iter().filter(|m| m.operation == ADD_VALUE) {
        base += m.amount;
    }
    let mut result = base;
    for m in modifiers
        .iter()
        .filter(|m| m.operation == ADD_MULTIPLIED_BASE)
    {
        result += base * m.amount;
    }
    for m in modifiers
        .iter()
        .filter(|m| m.operation == ADD_MULTIPLIED_TOTAL)
    {
        result *= 1.0 + m.amount;
    }
    // `RangedAttribute.sanitizeValue`.
    if result.is_nan() {
        info.min
    } else {
        result.clamp(info.min, info.max)
    }
}

/// The name of an equipment slot as slot groups and enchantments name
/// it.
pub fn slot_name(slot: u8) -> &'static str {
    match slot {
        equipment_slot::MAIN_HAND => "mainhand",
        equipment_slot::OFF_HAND => "offhand",
        equipment_slot::FEET => "feet",
        equipment_slot::LEGS => "legs",
        equipment_slot::CHEST => "chest",
        equipment_slot::HEAD => "head",
        _ => "body",
    }
}

/// `EquipmentSlotGroup.test`.
pub fn slot_in_group(slot: u8, group: &str) -> bool {
    let name = slot_name(slot);
    match group {
        "any" => true,
        "hand" => matches!(slot, equipment_slot::MAIN_HAND | equipment_slot::OFF_HAND),
        "armor" => matches!(
            slot,
            equipment_slot::FEET
                | equipment_slot::LEGS
                | equipment_slot::CHEST
                | equipment_slot::HEAD
        ),
        group => group == name,
    }
}

/// The modifiers an item gives in a slot (`ItemStack.forEachModifier`):
/// its `attribute_modifiers`, then its enchantments' attributes.
pub fn item_modifiers(
    data: &GameData,
    stack: &ItemStack,
    slot: u8,
    out: &mut BTreeMap<String, Vec<AttributeModifier>>,
) {
    if let Some(Nbt::List(modifiers)) =
        crate::stack::get(data, stack, "minecraft:attribute_modifiers")
    {
        for m in modifiers {
            let group = match m.get("slot") {
                Some(Nbt::String(s)) => s.as_str(),
                _ => "any",
            };
            if !slot_in_group(slot, group) {
                continue;
            }
            let (Some(Nbt::String(attribute)), Some(Nbt::String(id))) =
                (m.get("type"), m.get("id"))
            else {
                continue;
            };
            let amount = match m.get("amount") {
                Some(Nbt::Double(v)) => *v,
                Some(Nbt::Float(v)) => f64::from(*v),
                Some(Nbt::Int(v)) => f64::from(*v),
                _ => 0.0,
            };
            let operation = match m.get("operation") {
                Some(Nbt::String(o)) => operation(o),
                _ => ADD_VALUE,
            };
            out.entry(attribute.clone())
                .or_default()
                .push(AttributeModifier {
                    id: id.clone(),
                    amount,
                    operation,
                });
        }
    }
    for (enchantment, level) in crate::stack::enchantments(data, stack) {
        let Some((_, json)) = data.enchantment(&enchantment) else {
            continue;
        };
        if !enchant::applies_in(json, slot) {
            continue;
        }
        for effect in json["effects"]["minecraft:attributes"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let (Some(attribute), Some(id)) = (effect["attribute"].as_str(), effect["id"].as_str())
            else {
                continue;
            };
            out.entry(attribute.to_owned())
                .or_default()
                .push(AttributeModifier {
                    // `EnchantmentAttributeEffect.idForSlot`.
                    id: format!("{id}/{}", slot_name(slot)),
                    amount: f64::from(enchant::level_value(&effect["amount"], level)),
                    operation: operation(effect["operation"].as_str().unwrap_or("")),
                });
        }
    }
}

impl Game {
    /// What the entity holds and wears, by equipment slot.
    pub(super) fn equipment_of(&self, target: Living) -> Vec<(u8, ItemStack)> {
        match target {
            Living::Player(id) => self.players.get(&id).map_or_else(Vec::new, |p| {
                crate::inventory::EQUIPMENT
                    .iter()
                    .filter_map(|&slot| Some((slot, p.inventory.equipment(slot)?.clone())))
                    .collect()
            }),
            Living::Mob(id) => self
                .mobs
                .get(&id)
                .map_or_else(Vec::new, |m| m.equipment().to_vec()),
        }
    }

    /// Every modifier on the entity, by attribute name.
    pub(super) fn modifiers(&self, target: Living) -> BTreeMap<String, Vec<AttributeModifier>> {
        let data = &self.context.data;
        let mut out = BTreeMap::new();
        for (slot, stack) in self.equipment_of(target) {
            // Broken gear gives nothing (`ItemStack.canBeHurtBy`... and
            // `isBroken`): only gear at its last point counts as broken.
            if crate::stack::damageable(data, &stack)
                && crate::stack::damage(data, &stack) >= crate::stack::max_damage(data, &stack)
            {
                continue;
            }
            item_modifiers(data, &stack, slot, &mut out);
        }
        if let Some(effects) = self.effects(target) {
            for effect in effects.active.values() {
                let Some(info) = data.alchemy.effects.get(effect.effect) else {
                    continue;
                };
                for m in &info.modifiers {
                    out.entry(m.attribute.clone())
                        .or_default()
                        .push(AttributeModifier {
                            id: m.id.clone(),
                            // `AttributeTemplate.create`.
                            amount: m.amount * f64::from(effect.amplifier + 1),
                            operation: operation(&m.operation),
                        });
                }
            }
        }
        out
    }

    /// The entity's base value of an attribute.
    pub(super) fn base_attribute(&self, target: Living, info: &AttributeInfo) -> f64 {
        match target {
            Living::Player(_) => info.player.unwrap_or(info.default),
            Living::Mob(id) => self
                .mobs
                .get(&id)
                .and_then(|m| m.base_attribute(&info.name))
                .unwrap_or(info.default),
        }
    }

    /// `LivingEntity.getAttributeValue`.
    pub(super) fn attribute(&self, target: Living, name: &str) -> f64 {
        let data = &self.context.data;
        let Some((_, info)) = data.attribute(name) else {
            return 0.0;
        };
        let base = self.base_attribute(target, info);
        let modifiers = self.modifiers(target);
        value(
            base,
            modifiers.get(name).map_or(&[][..], Vec::as_slice),
            info,
        )
    }

    /// The attributes the client may compute with, as they are now.
    fn syncable(&self, target: Living) -> Vec<AttributeSnapshot> {
        let data = &self.context.data;
        let modifiers = self.modifiers(target);
        data.alchemy
            .attributes
            .iter()
            .enumerate()
            .filter(|(_, info)| info.sync)
            .filter_map(|(id, info)| {
                let base = self.base_attribute(target, info);
                let mods = modifiers.get(&info.name).cloned().unwrap_or_default();
                // Only what differs from what the client assumes.
                let default = match target {
                    Living::Player(_) => info.player?,
                    Living::Mob(_) => base,
                };
                if mods.is_empty() && base == default {
                    return None;
                }
                Some(AttributeSnapshot {
                    attribute: id as i32,
                    base,
                    modifiers: mods,
                })
            })
            .collect()
    }

    /// Tells clients what changed of an entity's attributes
    /// (`ServerEntity.sendDirtyEntityData`): the player itself and those
    /// who see it.
    pub(super) fn sync_attributes(&mut self, target: Living) {
        let data = Arc::clone(&self.context.data);
        let now = self.syncable(target);
        let sent = match target {
            Living::Player(id) => self.players.get(&id).map(|p| &p.sent_attributes),
            Living::Mob(id) => self.mobs.get(&id).map(|m| &m.sent_attributes),
        };
        let Some(sent) = sent else {
            return;
        };
        let mut changed: Vec<AttributeSnapshot> =
            now.iter().filter(|s| !sent.contains(s)).cloned().collect();
        // An attribute that lost its modifiers goes back to plain.
        for old in sent {
            if !now.iter().any(|s| s.attribute == old.attribute) {
                let info = &data.alchemy.attributes[old.attribute as usize];
                changed.push(AttributeSnapshot {
                    attribute: old.attribute,
                    base: self.base_attribute(target, info),
                    modifiers: Vec::new(),
                });
            }
        }
        match target {
            Living::Player(id) => {
                if let Some(player) = self.players.get_mut(&id) {
                    player.sent_attributes = now;
                }
            }
            Living::Mob(id) => {
                if let Some(mob) = self.mobs.get_mut(&id) {
                    mob.sent_attributes = now;
                }
            }
        }
        if changed.is_empty() {
            return;
        }
        let packet = Encoded::new(&UpdateAttributes {
            entity_id: target.id(),
            attributes: AttributeSnapshots(changed),
        });
        match target {
            Living::Player(id) => {
                if let Some(player) = self.players.get(&id) {
                    player.send(Outbound::Packet(Arc::new(packet.clone())));
                }
                self.broadcast_entity(id, vec![packet]);
            }
            Living::Mob(id) => self.broadcast_mob(id, vec![packet]),
        }
    }

    /// A new view of a player for its own client (join, respawn): every
    /// attribute that differs from the client's defaults.
    pub(super) fn send_attributes(&mut self, entity_id: i32) {
        let now = self.syncable(Living::Player(entity_id));
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        player.sent_attributes = now.clone();
        if !now.is_empty() {
            player.send(Outbound::Packet(Arc::new(Encoded::new(
                &UpdateAttributes {
                    entity_id,
                    attributes: AttributeSnapshots(now),
                },
            ))));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_like_vanilla() {
        let info = AttributeInfo {
            name: "minecraft:movement_speed".into(),
            default: 0.7,
            min: 0.0,
            max: 1024.0,
            sync: true,
            player: Some(0.1),
        };
        let m = |amount, operation| AttributeModifier {
            id: "x".into(),
            amount,
            operation,
        };
        // Speed II: 0.1 * (1 + 0.4).
        let speed = value(0.1, &[m(0.4, ADD_MULTIPLIED_TOTAL)], &info);
        assert!((speed - 0.14).abs() < 1e-9);
        // Adding, then on the base, then on the total.
        let v = value(
            1.0,
            &[
                m(1.0, ADD_VALUE),
                m(0.5, ADD_MULTIPLIED_BASE),
                m(1.0, ADD_MULTIPLIED_TOTAL),
            ],
            &info,
        );
        assert!((v - 6.0).abs() < 1e-9);
        assert_eq!(value(-5.0, &[], &info), 0.0, "clamped");
    }

    #[test]
    fn swords_and_enchantments_give_modifiers() {
        let data = GameData::load().unwrap();
        let mut sword = crate::stack::plain(data.item_id("minecraft:diamond_sword").unwrap(), 1);
        let mut out = BTreeMap::new();
        item_modifiers(&data, &sword, equipment_slot::MAIN_HAND, &mut out);
        assert_eq!(out["minecraft:attack_damage"][0].amount, 6.0);
        out.clear();
        item_modifiers(&data, &sword, equipment_slot::OFF_HAND, &mut out);
        assert!(out.is_empty(), "a sword only counts in the main hand");
        crate::stack::set_enchantments_in(
            &data,
            &mut sword,
            "minecraft:enchantments",
            &[("minecraft:sweeping_edge".into(), 3)],
        );
        item_modifiers(&data, &sword, equipment_slot::MAIN_HAND, &mut out);
        let sweeping = &out["minecraft:sweeping_damage_ratio"][0];
        assert_eq!(sweeping.id, "minecraft:enchantment.sweeping_edge/mainhand");
        assert!((sweeping.amount - 0.75).abs() < 1e-6);
    }
}
