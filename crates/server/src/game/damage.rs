//! What stands between a hit and a body (`LivingEntity.hurtServer` and
//! `actuallyHurt`): immunities, armor and its wear, the resistance effect,
//! protection enchantments and the golden hearts of absorption.

use std::sync::Arc;

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::packets::play::equipment_slot;

use super::Game;
use super::effect::Living;

/// The armor slots of the inventory menu and their equipment slots.
const ARMOR: [(usize, u8); 4] = [
    (5, equipment_slot::HEAD),
    (6, equipment_slot::CHEST),
    (7, equipment_slot::LEGS),
    (8, equipment_slot::FEET),
];

/// `CombatRules.getDamageAfterAbsorb`.
pub fn after_armor(damage: f32, armor: f32, toughness: f32) -> f32 {
    let toughness = 2.0 + toughness / 4.0;
    let real = (armor - damage / toughness).clamp(armor * 0.2, 20.0);
    damage * (1.0 - real / 25.0)
}

/// `CombatRules.getDamageAfterMagicAbsorb`.
pub fn after_protection(damage: f32, protection: f32) -> f32 {
    damage * (1.0 - protection.clamp(0.0, 20.0) / 25.0)
}

impl Game {
    /// Whether a damage type is in a damage type tag, such as
    /// `minecraft:is_fire`.
    pub(super) fn damage_is(&self, damage_type: &str, tag: &str) -> bool {
        let data = &self.context.data;
        data.registry_id("minecraft:damage_type", damage_type)
            .is_ok_and(|id| data.tag_contains("minecraft:damage_type", tag, id))
    }

    /// What makes an entity shrug a hit off before anything else: fire
    /// resistance against fire, frost walker against magma.
    pub(super) fn immune_to(&self, target: Living, damage_type: &str) -> bool {
        (self.damage_is(damage_type, "minecraft:is_fire")
            && self.has_effect(target, "minecraft:fire_resistance"))
            || self.immune_by_enchantment(target, damage_type)
    }

    /// `getDamageAfterArmorAbsorb`, `getDamageAfterMagicAbsorb` and the
    /// absorption of `actuallyHurt`: what is left for the health. Armor
    /// wears as it takes the hit.
    pub(super) fn absorb_damage(
        &mut self,
        target: Living,
        amount: f32,
        damage_type: &str,
        attacker: Option<i32>,
        direct: Option<i32>,
    ) -> f32 {
        let mut damage = amount;
        if !self.damage_is(damage_type, "minecraft:bypasses_armor") {
            self.hurt_armor(target, damage_type, damage);
            let armor = self.attribute(target, "minecraft:armor") as f32;
            let toughness = self.attribute(target, "minecraft:armor_toughness") as f32;
            damage = after_armor(damage, armor, toughness);
        }
        if !self.damage_is(damage_type, "minecraft:bypasses_effects") {
            if let Some(level) = self.effect_level(target, "minecraft:resistance")
                && !self.damage_is(damage_type, "minecraft:bypasses_resistance")
            {
                let absorb = (25 - (level + 1) * 5) as f32;
                damage = (damage * absorb / 25.0).max(0.0);
            }
            if damage > 0.0 && !self.damage_is(damage_type, "minecraft:bypasses_enchantments") {
                let protection = self.damage_protection(target, damage_type, attacker, direct);
                if protection > 0.0 {
                    damage = after_protection(damage, protection);
                }
            }
        }
        let absorption = self.absorption(target);
        let left = (damage - absorption).max(0.0);
        if absorption > 0.0 {
            self.set_absorption(target, absorption - (damage - left));
        }
        left
    }

    /// `LivingEntity.doHurtEquipment` for the armor slots: a point of wear
    /// per four of the damage on every piece that wears out from hits,
    /// unless it resists this kind (netherite against fire).
    fn hurt_armor(&mut self, target: Living, damage_type: &str, damage: f32) {
        let Living::Player(entity_id) = target else {
            return;
        };
        if damage <= 0.0 {
            return;
        }
        let data = Arc::clone(&self.context.data);
        let points = (damage / 4.0).max(1.0) as i32;
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let worn: Vec<usize> = ARMOR
            .iter()
            .filter(|(slot, _)| {
                player.inventory.get(*slot).is_some_and(|stack| {
                    let equippable = crate::stack::get(&data, stack, "minecraft:equippable");
                    let hurt_on_hit = equippable
                        .as_ref()
                        .is_some_and(|e| !matches!(e.get("damage_on_hurt"), Some(Nbt::Byte(0))));
                    let resists =
                        match crate::stack::get(&data, stack, "minecraft:damage_resistant") {
                            Some(r) => match r.get("types") {
                                Some(Nbt::String(tag)) => {
                                    self.damage_is(damage_type, tag.trim_start_matches('#'))
                                }
                                _ => false,
                            },
                            None => false,
                        };
                    hurt_on_hit && crate::stack::damageable(&data, stack) && !resists
                })
            })
            .map(|(slot, _)| *slot)
            .collect();
        for slot in worn {
            self.wear(entity_id, slot, points);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armor_and_protection_like_vanilla() {
        // Full diamond (20 armor, 8 toughness) against 10 damage: 17.5
        // points count, 70 % less.
        let after = after_armor(10.0, 20.0, 8.0);
        assert!((after - 3.0).abs() < 1e-5, "{after}");
        // Protection IV on all four pieces caps at 20 points: 80 % less.
        assert!((after_protection(10.0, 64.0) - 2.0).abs() < 1e-5);
    }
}
