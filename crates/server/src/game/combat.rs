//! Hitting things: `Player.attack` against animals and, in survival, other
//! players. Damage depends on the weapon and how charged the swing is;
//! falling hits are critical, sprinting hits knock back further.

use std::sync::Arc;

use ferrocraft_protocol::packets::play::{
    ANIMATE_CRITICAL_HIT, Animate, GameMode, LpVec3, SOUND_SOURCE_PLAYERS, SetEntityMotion,
    equipment_slot, text,
};

use super::{Encoded, Game, Outbound};
use crate::data::Aabb;

/// `Attributes.ATTACK_DAMAGE` and `ATTACK_SPEED` of a bare-handed player.
const BASE_DAMAGE: f32 = 1.0;
const BASE_SPEED: f32 = 4.0;
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
        let weapon = player
            .inventory
            .equipment(equipment_slot::MAIN_HAND)
            .and_then(|s| data.item(s.item));
        // `Player.getAttackStrengthScale`: a full swing takes 20 / speed ticks.
        let speed = BASE_SPEED + weapon.map_or(0.0, |w| w.attack_speed);
        let delay = 20.0 / speed.max(0.1);
        let strength = (((self.tick - player.last_swing) as f32 + 0.5) / delay).clamp(0.0, 1.0);
        let mut damage = (BASE_DAMAGE + weapon.map_or(0.0, |w| w.attack_damage))
            * (0.2 + strength * strength * 0.8);
        let full = strength > 0.9;
        let knockback = player.sprinting && full;
        let critical = full && player.fall_distance > 0.0 && !player.on_ground && !player.sprinting;
        if critical {
            damage *= 1.5;
        }
        let from = (entity_id, player.pos);
        let attacker_name = player.name().to_owned();
        let attacker_uuid = player.uuid();
        let yaw = f64::from(player.yaw).to_radians();
        let at = player.pos;
        let slot = player.inventory.selected_slot();
        let weapon_name = weapon.map(|w| w.name.clone());
        let wears = weapon.is_some_and(|w| w.max_damage.is_some());
        let mortal = player.game_mode() != GameMode::Creative;

        let hurt = if self.mobs.contains_key(&target) {
            let hurt = self.hurt_mob(target, damage, Some(from), "minecraft:player_attack");
            if hurt && knockback {
                self.push_mob(target, yaw, 0.5);
            }
            hurt
        } else {
            let Some(victim) = self.players.get(&target) else {
                return;
            };
            let message = text::translate(
                "death.attack.player",
                vec![
                    text::player_name(victim.name(), victim.uuid()),
                    text::player_name(&attacker_name, attacker_uuid),
                ],
                None,
            );
            let before = victim.health;
            self.hurt(target, damage, "minecraft:player_attack", message);
            let hurt = self.players.get(&target).is_some_and(|v| v.health < before);
            if hurt && let Some(victim) = self.players.get(&target) {
                // `LivingEntity.knockback(0.4)`, plus the sprint's extra, sent to
                // the victim, whose client moves itself.
                let (dx, dz) = (at[0] - victim.pos[0], at[2] - victim.pos[2]);
                let length = dx.hypot(dz).max(1.0e-4);
                let extra = if knockback { 0.5 * 0.5 } else { 0.0 };
                let velocity = [
                    -dx / length * 0.4 - yaw.sin() * extra,
                    0.4,
                    -dz / length * 0.4 + yaw.cos() * extra,
                ];
                victim.send(Outbound::Packet(Arc::new(Encoded::new(&SetEntityMotion {
                    entity_id: target,
                    velocity: LpVec3(velocity),
                }))));
            }
            hurt
        };
        if let Some(player) = self.players.get_mut(&entity_id) {
            player.last_swing = self.tick;
            if knockback && hurt {
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
            "crit"
        } else if knockback {
            "knockback"
        } else if full {
            "strong"
        } else {
            "weak"
        };
        self.play_sound_from(
            None,
            &format!("minecraft:entity.player.attack.{sound}"),
            at,
            SOUND_SOURCE_PLAYERS,
            1.0,
            1.0,
        );
        if critical {
            self.broadcast_packet(
                None,
                &Animate {
                    entity_id: target,
                    action: ANIMATE_CRITICAL_HIT,
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
