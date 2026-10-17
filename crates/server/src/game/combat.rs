//! Hitting things: `Player.attack` against animals and, in survival, other
//! players. Damage depends on the weapon and how charged the swing is;
//! falling hits are critical, sprinting hits knock back further.

use std::sync::Arc;

use ferrocraft_protocol::packets::play::{
    ANIMATE_CRITICAL_HIT, ANIMATE_MAGIC_CRITICAL_HIT, Animate, GameMode, LpVec3,
    SOUND_SOURCE_PLAYERS, SetEntityMotion, equipment_slot, text,
};

use super::effect::Living;
use super::{Encoded, Game, Outbound};
use crate::data::Aabb;

/// How far from the eyes a hit may land: `entity_interaction_range` plus
/// vanilla's tolerance.
const ATTACK_REACH: f64 = 3.0 + 3.0;

/// The distance from `eyes` to the nearest point of a box.
fn distance_to(eyes: [f64; 3], bounds: &Aabb) -> f64 {
    (0..3)
        .map(|axis| {
            let nearest = eyes[axis].clamp(bounds[axis], bounds[axis + 3]);
            (eyes[axis] - nearest).powi(2)
        })
        .sum::<f64>()
        .sqrt()
}

impl Game {
    /// `minecraft:attack`: a left click on an entity.
    pub(super) fn attack(&mut self, entity_id: i32, target: i32) {
        let data = Arc::clone(&self.context.data);
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        if player.dead || player.game_mode() == GameMode::Spectator || target == entity_id {
            return;
        }
        let victim = self.players.get(&target);
        let bounds = match (self.mob_bounds(target), victim) {
            (Some(bounds), _) => bounds,
            (None, Some(victim)) if !victim.dead => victim.bounds(),
            _ => return,
        };
        if distance_to(player.eyes(), &bounds) > ATTACK_REACH {
            return;
        }
        let weapon_stack = player
            .inventory
            .equipment(equipment_slot::MAIN_HAND)
            .cloned();
        let weapon = weapon_stack.as_ref().and_then(|s| data.item(s.item));
        let living = Living::Player(entity_id);
        // `Player.getAttackStrengthScale`: a full swing takes 20 / speed
        // ticks.
        let speed = self.attribute(living, "minecraft:attack_speed") as f32;
        let delay = 20.0 / speed.max(0.1);
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let strength = (((self.tick - player.last_swing) as f32 + 0.5) / delay).clamp(0.0, 1.0);
        let mut damage = self.attribute(living, "minecraft:attack_damage") as f32;
        let damage_type = "minecraft:player_attack";
        // `getEnchantedDamage`: sharpness, smite, bane of arthropods.
        let magic = weapon_stack.as_ref().map_or(0.0, |w| {
            self.enchanted_damage(
                w,
                target,
                damage_type,
                Some(entity_id),
                Some(entity_id),
                damage,
            ) - damage
        }) * strength;
        damage *= 0.2 + strength * strength * 0.8;
        let full = strength > 0.9;
        let knockback_attack = player.sprinting && full;
        let critical = full
            && player.fall_distance > 0.0
            && !player.on_ground
            && !player.sprinting
            && self.living(target).is_some();
        if critical {
            damage *= 1.5;
        }
        let total = damage + magic;
        // `isSweepAttack`: a full swing with a sword, standing.
        let sweep = full
            && !critical
            && !knockback_attack
            && player.on_ground
            && weapon_stack.as_ref().is_some_and(|w| {
                data.item_tag("minecraft:swords")
                    .is_some_and(|swords| swords.contains(&w.item))
            });
        let from = (entity_id, player.pos);
        let attacker_name = player.name().to_owned();
        let attacker_uuid = player.uuid();
        let yaw = f64::from(player.yaw).to_radians();
        let at = player.pos;
        let slot = player.inventory.selected_slot();
        let weapon_name = weapon.map(|w| w.name.clone());
        let wears = weapon_stack
            .as_ref()
            .is_some_and(|w| crate::stack::damageable(&data, w));
        let mortal = player.game_mode() != GameMode::Creative;
        // `getKnockback`: the attribute and the knockback enchantment, half
        // of it; a sprint adds half a block.
        let knockback = {
            let base = self.attribute(living, "minecraft:attack_knockback") as f32;
            let enchanted = weapon_stack.as_ref().map_or(base, |w| {
                self.enchanted_knockback(w, target, damage_type, Some(entity_id), base)
            });
            enchanted / 2.0 + if knockback_attack { 0.5 } else { 0.0 }
        };

        let hurt = self.hit(
            entity_id,
            target,
            total,
            from,
            damage_type,
            (&attacker_name, attacker_uuid),
        );
        if hurt {
            // A player is always pushed (`dealDefaultKnockback`), a mob was
            // already.
            if knockback > 0.0 || self.players.contains_key(&target) {
                self.knock(target, yaw, f64::from(knockback), at);
            }
            if sweep {
                self.sweep(entity_id, target, damage, strength, yaw);
            }
            // Fire aspect, thorns and the like.
            self.post_attack(
                self.living(target).unwrap_or(Living::Mob(target)),
                Some(entity_id),
                Some(entity_id),
                damage_type,
            );
        }
        if let Some(player) = self.players.get_mut(&entity_id) {
            player.last_swing = self.tick;
            if knockback_attack && hurt {
                player.sprinting = false;
                player.data_dirty = true;
            }
        }
        if !hurt {
            self.play_sound_from(
                None,
                "minecraft:entity.player.attack.nodamage",
                at,
                SOUND_SOURCE_PLAYERS,
                1.0,
                1.0,
            );
            return;
        }
        // `Player.attackVisualEffects`.
        let sound = if critical {
            Some("crit")
        } else if sweep {
            None
        } else if knockback_attack {
            Some("knockback")
        } else if full {
            Some("strong")
        } else {
            Some("weak")
        };
        if let Some(sound) = sound {
            self.play_sound_from(
                None,
                &format!("minecraft:entity.player.attack.{sound}"),
                at,
                SOUND_SOURCE_PLAYERS,
                1.0,
                1.0,
            );
        }
        if critical {
            self.broadcast_packet(
                None,
                &Animate {
                    entity_id: target,
                    action: ANIMATE_CRITICAL_HIT,
                },
            );
        }
        if magic > 0.0 {
            self.broadcast_packet(
                None,
                &Animate {
                    entity_id: target,
                    action: ANIMATE_MAGIC_CRITICAL_HIT,
                },
            );
        }
        self.exhaust(entity_id, 0.1);
        // `Item.hurtEnemy`: swords wear by one, other tools by two.
        if wears && mortal {
            let amount = if weapon_name.is_some_and(|n| n.ends_with("_sword")) {
                1
            } else {
                2
            };
            self.wear(entity_id, slot, amount);
        }
    }

    /// Damage from a player's hand to a mob or another player.
    fn hit(
        &mut self,
        entity_id: i32,
        target: i32,
        damage: f32,
        from: (i32, [f64; 3]),
        damage_type: &str,
        (attacker_name, attacker_uuid): (&str, uuid::Uuid),
    ) -> bool {
        if self.mobs.contains_key(&target) {
            return self.hurt_mob(target, damage, Some(from), damage_type);
        }
        let Some(victim) = self.players.get(&target) else {
            return false;
        };
        let message = text::translate(
            "death.attack.player",
            vec![
                text::player_name(victim.name(), victim.uuid()),
                text::player_name(attacker_name, attacker_uuid),
            ],
            None,
        );
        let before = victim.health;
        self.hurt_player(
            target,
            super::survival::Hit {
                amount: damage,
                damage_type,
                attacker: Some(entity_id),
                direct: Some(entity_id),
                // The push comes with the extra knockback, from `knock`.
                knockback_from: None,
                message,
            },
        );
        self.players.get(&target).is_some_and(|v| v.health < before)
    }

    /// `LivingEntity.knockback` in the direction the attacker looks, on
    /// top of the push every hit gives (from `attacker`).
    fn knock(&mut self, target: i32, yaw: f64, strength: f64, attacker: [f64; 3]) {
        if self.mobs.contains_key(&target) {
            self.push_mob(target, yaw, strength);
            return;
        }
        let resistance = self.attribute(Living::Player(target), "minecraft:knockback_resistance");
        let Some(victim) = self.players.get(&target) else {
            return;
        };
        let s = (strength * (1.0 - resistance)).max(0.0);
        // The client moves itself, so it is told directly: the push of
        // every hit (`dealDefaultKnockback`, 0.4 away from the attacker),
        // then this one.
        let (dx, dz) = (attacker[0] - victim.pos[0], attacker[2] - victim.pos[2]);
        let length = dx.hypot(dz).max(1.0e-4);
        let push = 0.4 * (1.0 - resistance);
        let base = [-dx / length * push, push, -dz / length * push];
        let velocity = if s > 0.0 {
            [
                base[0] / 2.0 - yaw.sin() * s,
                (base[1] / 2.0 + s).min(0.4),
                base[2] / 2.0 + yaw.cos() * s,
            ]
        } else {
            base
        };
        victim.send(Outbound::Packet(Arc::new(Encoded::new(&SetEntityMotion {
            entity_id: target,
            velocity: LpVec3(velocity),
        }))));
    }

    /// `Player.doSweepAttack`: the swing hits those next to the target too.
    fn sweep(&mut self, entity_id: i32, target: i32, damage: f32, strength: f32, yaw: f64) {
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let at = player.pos;
        let attacker_name = player.name().to_owned();
        let attacker_uuid = player.uuid();
        let weapon = player
            .inventory
            .equipment(equipment_slot::MAIN_HAND)
            .cloned();
        let ratio =
            self.attribute(Living::Player(entity_id), "minecraft:sweeping_damage_ratio") as f32;
        let swept = 1.0 + ratio * damage;
        let Some(bounds) = self
            .mob_bounds(target)
            .or_else(|| self.players.get(&target).map(super::player::Player::bounds))
        else {
            return;
        };
        let area = [
            bounds[0] - 1.0,
            bounds[1] - 0.25,
            bounds[2] - 1.0,
            bounds[3] + 1.0,
            bounds[4] + 0.25,
            bounds[5] + 1.0,
        ];
        let nearby: Vec<i32> = self
            .mob_boxes()
            .into_iter()
            .chain(
                self.players
                    .values()
                    .filter(|p| !p.dead && p.game_mode() != GameMode::Spectator)
                    .map(|p| (p.entity_id, p.bounds())),
            )
            .filter(|&(id, b)| {
                id != entity_id
                    && id != target
                    && (0..3).all(|i| b[i] < area[i + 3] && b[i + 3] > area[i])
            })
            .filter(|(_, b)| {
                let centre = [(b[0] + b[3]) / 2.0, b[1], (b[2] + b[5]) / 2.0];
                (0..3).map(|i| (centre[i] - at[i]).powi(2)).sum::<f64>() < 9.0
            })
            .map(|(id, _)| id)
            .collect();
        self.play_sound_from(
            None,
            "minecraft:entity.player.attack.sweep",
            at,
            SOUND_SOURCE_PLAYERS,
            1.0,
            1.0,
        );
        let damage_type = "minecraft:player_attack";
        for other in nearby {
            let amount = weapon.as_ref().map_or(swept, |w| {
                self.enchanted_damage(
                    w,
                    other,
                    damage_type,
                    Some(entity_id),
                    Some(entity_id),
                    swept,
                )
            }) * strength;
            if self.hit(
                entity_id,
                other,
                amount,
                (entity_id, at),
                damage_type,
                (&attacker_name, attacker_uuid),
            ) {
                self.knock(other, yaw, 0.4, at);
                self.post_attack(
                    self.living(other).unwrap_or(Living::Mob(other)),
                    Some(entity_id),
                    Some(entity_id),
                    damage_type,
                );
            }
        }
    }

    /// `minecraft:interact` on an entity.
    pub(super) fn interact(&mut self, entity_id: i32, target: i32, hand: i32) {
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        if player.dead || player.game_mode() == GameMode::Spectator {
            return;
        }
        let Some(bounds) = self.mob_bounds(target) else {
            return;
        };
        if distance_to(player.eyes(), &bounds) > ATTACK_REACH {
            return;
        }
        if self.interact_mob(target, entity_id, hand) {
            self.swing(entity_id);
        }
    }
}
