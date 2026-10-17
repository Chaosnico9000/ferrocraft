//! Eating and drinking up (`Consumable.onConsume`): food fills the stomach,
//! potions give their effects, milk takes them all away, and the item's
//! `on_consume_effects` do the rest (golden apples, rotten flesh, chorus
//! fruit). What stays behind is the item's `use_remainder`.

use std::sync::Arc;

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{GameMode, ItemStack, SOUND_SOURCE_PLAYERS};
use rand::Rng;
use serde_json::Value;

use super::effect::{EffectInstance, Living};
use super::{Game, Outbound};
use crate::data::GameData;

/// The effects a `potion_contents` value gives: those of its potion, then
/// its own (`PotionContents.forEachEffect`), each with its time scaled.
pub fn potion_effects(data: &GameData, contents: &Nbt, scale: f32) -> Vec<EffectInstance> {
    let mut effects = Vec::new();
    if let Some(Nbt::String(potion)) = contents.get("potion")
        && let Some(info) = data.potion(potion)
    {
        for e in &info.effects {
            if let Some((id, _)) = data.effect(&e.id) {
                effects.push(EffectInstance::new(id, e.duration, e.amplifier).scaled(scale));
            }
        }
    }
    if let Some(Nbt::List(custom)) = contents.get("custom_effects") {
        for e in custom {
            if let Some(effect) = EffectInstance::from_nbt(e, data) {
                effects.push(effect.scaled(scale));
            }
        }
    }
    effects
}

impl Game {
    /// `PotionContents.applyToLivingEntity`: instant effects happen, the
    /// others start. `source` threw or drank it.
    pub(super) fn apply_potion(
        &mut self,
        target: Living,
        contents: &Nbt,
        scale: f32,
        source: Option<i32>,
    ) {
        let data = Arc::clone(&self.context.data);
        for effect in potion_effects(&data, contents, scale) {
            let info = &data.alchemy.effects[effect.effect];
            if info.instant {
                let name = info.name.clone();
                self.apply_instant(target, &name, effect.amplifier, 1.0, source);
            } else {
                self.add_effect(target, effect);
            }
        }
    }

    /// The end of eating or drinking what is in `slot`.
    pub(super) fn consume(&mut self, entity_id: i32, slot: usize) {
        let data = Arc::clone(&self.context.data);
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let Some(stack) = player.inventory.get(slot).cloned() else {
            return;
        };
        let Some(item) = data.item(stack.item) else {
            return;
        };
        let creative = player.game_mode() == GameMode::Creative;
        let target = Living::Player(entity_id);
        // The listeners: food, potion contents, suspicious stew.
        if let Some(food) = &item.food
            && let Some(player) = self.players.get_mut(&entity_id)
        {
            player.food.add(food.nutrition, food.saturation);
        }
        if let Some(contents) = crate::stack::get(&data, &stack, "minecraft:potion_contents") {
            let scale = crate::stack::float(&data, &stack, "minecraft:potion_duration_scale")
                .unwrap_or(1.0);
            self.apply_potion(target, &contents, scale, Some(entity_id));
        }
        if let Some(Nbt::List(stew)) =
            crate::stack::get(&data, &stack, "minecraft:suspicious_stew_effects")
        {
            for entry in stew {
                if let Some(Nbt::String(name)) = entry.get("id")
                    && let Some((id, _)) = data.effect(name)
                {
                    let duration = match entry.get("duration") {
                        Some(Nbt::Int(d)) => *d,
                        _ => 160,
                    };
                    self.add_effect(target, EffectInstance::new(id, duration, 0));
                }
            }
        }
        if let Some(effects) = item
            .components
            .get("consumable")
            .and_then(|c| c["on_consume_effects"].as_array())
        {
            for effect in effects {
                self.consume_effect(entity_id, effect);
            }
        }
        self.send_health(entity_id);
        if creative {
            return;
        }
        self.update_inventory(entity_id, |inventory| {
            inventory.take(slot, 1);
        });
        // `UseRemainder.convertIntoRemainder`: into the emptied slot, or
        // anywhere else.
        if let Some(Value::String(remainder)) = item
            .components
            .get("use_remainder")
            .map(|r| r["id"].clone())
            && let Some(remainder) = data.item_id(&remainder)
        {
            let leftover = crate::stack::plain(remainder, 1);
            let empty = self
                .players
                .get(&entity_id)
                .is_some_and(|p| p.inventory.get(slot).is_none());
            if empty {
                self.update_inventory(entity_id, |inventory| inventory.set(slot, Some(leftover)));
            } else {
                self.give(entity_id, leftover);
            }
        }
        self.send_slots(entity_id, &[slot]);
    }

    /// One of an item's `on_consume_effects` (`ConsumeEffect.apply`).
    fn consume_effect(&mut self, entity_id: i32, effect: &Value) {
        let data = Arc::clone(&self.context.data);
        let target = Living::Player(entity_id);
        let mut rng = rand::rng();
        match effect["type"].as_str().unwrap_or("") {
            "minecraft:apply_effects" => {
                let probability = effect["probability"].as_f64().unwrap_or(1.0) as f32;
                if rng.random::<f32>() >= probability {
                    return;
                }
                for e in effect["effects"].as_array().into_iter().flatten() {
                    let nbt = crate::stack::json_to_nbt(e);
                    if let Some(instance) = EffectInstance::from_nbt(&nbt, &data) {
                        self.add_effect(target, instance);
                    }
                }
            }
            "minecraft:remove_effects" => {
                let names: Vec<String> = match &effect["effects"] {
                    Value::String(s) => vec![s.clone()],
                    Value::Array(list) => list
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect(),
                    _ => Vec::new(),
                };
                for name in names {
                    if let Some((id, _)) = data.effect(&name) {
                        self.remove_effect(target, id);
                    }
                }
            }
            "minecraft:clear_all_effects" => {
                self.clear_effects(target);
            }
            "minecraft:teleport_randomly" => {
                let diameter = effect["diameter"].as_f64().unwrap_or(16.0);
                self.teleport_randomly(entity_id, diameter);
            }
            "minecraft:play_sound" => {
                if let (Some(sound), Some(player)) =
                    (effect["sound"].as_str(), self.players.get(&entity_id))
                {
                    let at = player.pos;
                    self.play_sound_from(None, sound, at, SOUND_SOURCE_PLAYERS, 1.0, 1.0);
                }
            }
            _ => {}
        }
    }

    /// `LivingEntity.randomTeleport` from chorus fruit: up to sixteen
    /// tries at a random spot nearby, dropped to the ground, where the
    /// player fits.
    fn teleport_randomly(&mut self, entity_id: i32, diameter: f64) {
        let data = Arc::clone(&self.context.data);
        let world = Arc::clone(&self.context.world);
        let mut rng = rand::rng();
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let from = player.pos;
        let rot = [player.yaw, player.pitch];
        for _ in 0..16 {
            let x = from[0] + (rng.random::<f64>() - 0.5) * diameter;
            let y = (from[1] + (rng.random::<f64>() - 0.5) * diameter).clamp(
                f64::from(crate::world::MIN_Y),
                f64::from(crate::world::MIN_Y + crate::world::HEIGHT - 1),
            );
            let z = from[2] + (rng.random::<f64>() - 0.5) * diameter;
            let mut pos = BlockPos {
                x: x.floor() as i32,
                y: y.floor() as i32,
                z: z.floor() as i32,
            };
            // Down to something that blocks movement.
            let blocks = |p: BlockPos| !data.collision_shape(world.block(p)).is_empty();
            let mut found = false;
            while pos.y > crate::world::MIN_Y {
                let below = BlockPos {
                    y: pos.y - 1,
                    ..pos
                };
                if blocks(below) {
                    found = true;
                    break;
                }
                pos = below;
            }
            if !found {
                continue;
            }
            let below = world.block(BlockPos {
                y: pos.y - 1,
                ..pos
            });
            if data.state_in_tag(below, "minecraft:consumable_does_not_teleport_to") {
                continue;
            }
            let target = [x, f64::from(pos.y), z];
            let fits = (0..2).all(|dy| {
                let p = BlockPos {
                    y: pos.y + dy,
                    ..pos
                };
                let state = world.block(p);
                data.collision_shape(state).is_empty() && data.physics(state).fluid == 0
            });
            if !fits {
                continue;
            }
            if let Some(player) = self.players.get_mut(&entity_id) {
                player.pos = target;
                player.first_good = target;
                player.fall_distance = 0.0;
                player.send(Outbound::Teleport { pos: target, rot });
            }
            self.play_sound_from(
                None,
                "minecraft:item.chorus_fruit.teleport",
                target,
                SOUND_SOURCE_PLAYERS,
                1.0,
                1.0,
            );
            return;
        }
    }

    /// Whether the item can be eaten or drunk now (`Consumable.canConsume`):
    /// food only when hungry, unless it may always be eaten.
    pub(super) fn may_consume(&self, entity_id: i32, stack: &ItemStack) -> Option<u32> {
        let data = &self.context.data;
        let item = data.item(stack.item)?;
        let consumable = item.components.get("consumable")?;
        let player = self.players.get(&entity_id)?;
        if let Some(food) = &item.food {
            let mortal = matches!(player.game_mode(), GameMode::Survival | GameMode::Adventure);
            if mortal && !food.can_always_eat && player.food.level >= 20 {
                return None;
            }
        }
        let seconds = consumable["consume_seconds"].as_f64().unwrap_or(1.6);
        Some(((seconds * 20.0).round() as u32).max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn potions_give_their_effects() {
        let data = GameData::load().unwrap();
        let contents = Nbt::Compound(vec![(
            "potion".into(),
            Nbt::String("minecraft:strong_swiftness".into()),
        )]);
        let effects = potion_effects(&data, &contents, 1.0);
        let (speed, _) = data.effect("minecraft:speed").unwrap();
        assert_eq!(effects, vec![EffectInstance::new(speed, 1800, 1)]);
        // A lingering cloud gives a quarter of the time.
        assert_eq!(potion_effects(&data, &contents, 0.25)[0].duration, 450);
        let water = Nbt::Compound(vec![(
            "potion".into(),
            Nbt::String("minecraft:water".into()),
        )]);
        assert!(potion_effects(&data, &water, 1.0).is_empty());
    }
}
