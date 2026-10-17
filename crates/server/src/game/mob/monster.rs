//! What monsters want: a player to hunt (`NearestAttackableTargetGoal`,
//! `HurtByTargetGoal`), reached along a path and hit (`MeleeAttackGoal`),
//! shot at from a distance (`RangedBowAttackGoal`) or blown up
//! (`SwellGoal`); spiders leap and keep the peace by day; zombies and
//! skeletons burn in the sun.

use std::sync::Arc;

use ferrocraft_protocol::packets::play::{GameMode, SOUND_SOURCE_HOSTILE, text};
use rand::Rng;

use super::animal::{face, look_around, random_spot, turn_head};
use super::{Goal, Kind, Look, Mob, block_at, can_see_sky};
use crate::game::Game;
use crate::game::explosion::Blast;
use crate::game::physics;
use crate::game::projectile::{Pickup, Shot};
use crate::game::survival::{Hit, bow_power};

/// `Mob.DEFAULT_ATTACK_REACH`: a melee hit lands when the attacker's box,
/// this much wider, touches the target's.
const ATTACK_REACH: f64 = 0.828_427_124_746_190_1;
/// `Creeper.maxSwell` and its blast.
const FUSE: i32 = 30;
const CREEPER_BLAST: f32 = 3.0;
/// `RangedBowAttackGoal`'s range, squared.
const BOW_RANGE_SQ: f64 = 15.0 * 15.0;

impl Game {
    /// A player a monster may go for: in survival or adventure, alive.
    fn attackable(&self, player: i32) -> Option<[f64; 3]> {
        self.players
            .get(&player)
            .filter(|p| matches!(p.game_mode(), GameMode::Survival | GameMode::Adventure))
            .filter(|p| !p.dead)
            .map(|p| p.pos)
    }

    /// Whether nothing solid is between two points (`hasLineOfSight`).
    fn sees(&self, from: [f64; 3], to: [f64; 3]) -> bool {
        physics::clip(&self.context.world, &self.context.data, from, to).is_none()
    }

    /// The target, if still worth chasing: alive, attackable, in range and
    /// not out of sight for three seconds.
    fn keep_target(&mut self, entity_id: i32) {
        let Some(mob) = self.mobs.get(&entity_id) else {
            return;
        };
        let Some(target) = mob.target else {
            return;
        };
        let light = self.light_value(mob);
        let keep = match (self.attackable(target), self.players.get(&target)) {
            (Some(pos), Some(player)) => {
                let distance_sq: f64 = (0..3).map(|i| (pos[i] - mob.pos[i]).powi(2)).sum();
                let seen = self.sees(mob.eyes(), player.eyes());
                let calm = mob.kind == Kind::Spider
                    && light >= 0.5
                    && rand::rng().random_range(0..100) == 0;
                Some((distance_sq <= mob.follow_range.powi(2) && !calm, seen))
            }
            _ => None,
        };
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return;
        };
        match keep {
            Some((true, seen)) => {
                mob.unseen = if seen { 0 } else { mob.unseen + 1 };
                if mob.unseen > 60 {
                    lose_target(mob);
                }
            }
            _ => lose_target(mob),
        }
    }

    /// `NearestAttackableTargetGoal`: now and then, the nearest player in
    /// range and in sight. Spiders only hunt in the dark.
    fn find_target(&mut self, entity_id: i32) {
        let Some(mob) = self.mobs.get(&entity_id) else {
            return;
        };
        if mob.target.is_some() || rand::rng().random_range(0..10) != 0 {
            return;
        }
        if mob.kind == Kind::Spider && self.light_value(mob) >= 0.5 {
            return;
        }
        let range = mob.follow_range;
        let eyes = mob.eyes();
        let found = self
            .players
            .values()
            .filter(|p| self.attackable(p.entity_id).is_some())
            .filter(|p| {
                (p.pos[0] - mob.pos[0]).abs() <= range
                    && (p.pos[2] - mob.pos[2]).abs() <= range
                    && (p.pos[1] - mob.pos[1]).abs() <= 4.0
            })
            .map(|p| {
                let d: f64 = (0..3).map(|i| (p.pos[i] - mob.pos[i]).powi(2)).sum();
                (p.entity_id, d, p.eyes())
            })
            .filter(|&(_, d, _)| d <= range * range)
            .filter(|&(_, _, at)| self.sees(eyes, at))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, ..)| id);
        if let (Some(player), Some(mob)) = (found, self.mobs.get_mut(&entity_id)) {
            mob.target = Some(player);
            mob.unseen = 0;
            mob.repath = 0;
            mob.data_dirty = true;
        }
    }

    /// The goals of a monster for one tick.
    pub(in crate::game) fn think_monster(&mut self, entity_id: i32) {
        let was_aggressive = self.mobs.get(&entity_id).is_some_and(Mob::aggressive);
        self.keep_target(entity_id);
        self.find_target(entity_id);
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return;
        };
        if mob.aggressive() != was_aggressive {
            mob.data_dirty = true;
        }
        let kind = mob.kind;
        match mob.target {
            Some(target) => match kind {
                Kind::Skeleton => self.bow_attack(entity_id, target),
                _ => self.melee(entity_id, target),
            },
            None => self.wander(entity_id),
        }
    }

    /// No one to hunt: stroll now and then, look at whoever is near.
    fn wander(&mut self, entity_id: i32) {
        let data = Arc::clone(&self.context.data);
        let world = Arc::clone(&self.context.world);
        let Some(mob) = self.mobs.get(&entity_id) else {
            return;
        };
        let mut rng = rand::rng();
        let watcher = self.watcher(mob, 8.0);
        let looked_at = match mob.look {
            Look::At { player, .. } => self.players.get(&player).map(|p| p.eyes()),
            _ => None,
        };
        let stroll = (mob.goal == Goal::Idle && rng.random_range(0..120) == 0)
            .then(|| random_spot(&world, &data, mob, 10, 7, &mut rng))
            .flatten();
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return;
        };
        if mob.drawing.take().is_some() {
            mob.data_dirty = true;
        }
        if mob.swell_dir > 0 && !mob.ignited {
            mob.swell_dir = -1;
            mob.data_dirty = true;
        }
        if let Some(spot) = stroll {
            mob.goal = Goal::Walk {
                target: spot,
                speed: mob.kind.stroll_speed(),
            };
        }
        look_around(mob, watcher, looked_at, &mut rng);
    }

    /// `MeleeAttackGoal` (and `SwellGoal` for creepers, `LeapAtTargetGoal`
    /// for spiders): along a path to the target, planned anew every few
    /// ticks, and a hit once in reach.
    fn melee(&mut self, entity_id: i32, target: i32) {
        let Some(player) = self.players.get(&target) else {
            return;
        };
        let (pos, eyes, bounds) = (player.pos, player.eyes(), player.bounds());
        let Some(mob) = self.mobs.get(&entity_id) else {
            return;
        };
        let distance_sq: f64 = (0..3).map(|i| (pos[i] - mob.pos[i]).powi(2)).sum();
        let seen = mob.unseen == 0;
        let mut rng = rand::rng();
        let kind = mob.kind;
        // `SwellGoal`: close enough, the creeper stands and hisses.
        if kind == Kind::Creeper {
            let swelling = mob.swell_dir > 0 || distance_sq < 9.0;
            let dir = if !swelling || distance_sq > 49.0 || !seen {
                -1
            } else {
                1
            };
            let Some(mob) = self.mobs.get_mut(&entity_id) else {
                return;
            };
            if dir != mob.swell_dir && !mob.ignited {
                mob.swell_dir = dir;
                mob.data_dirty = true;
            }
            if swelling {
                mob.stop();
                mob.goal = Goal::Idle;
                let (yaw, pitch) = face(mob.eyes(), eyes);
                turn_head(mob, yaw, pitch);
                return;
            }
        }
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return;
        };
        mob.repath = mob.repath.saturating_sub(1);
        mob.attack_cooldown = (mob.attack_cooldown - 1).max(-1);
        let replan = seen && mob.repath == 0;
        if replan {
            mob.repath = 4 + rng.random_range(0..7);
            if distance_sq > 1024.0 {
                mob.repath += 10;
            } else if distance_sq > 256.0 {
                mob.repath += 5;
            }
        }
        let (yaw, pitch) = face(mob.eyes(), eyes);
        turn_head(mob, yaw, pitch);
        let attack = {
            let b = mob.bounds();
            let reach = [
                b[0] - ATTACK_REACH,
                b[1],
                b[2] - ATTACK_REACH,
                b[3] + ATTACK_REACH,
                b[4],
                b[5] + ATTACK_REACH,
            ];
            super::overlaps(&reach, &bounds) && mob.attack_cooldown <= 0 && seen
        };
        // `LeapAtTargetGoal`.
        if kind == Kind::Spider
            && (4.0..=16.0).contains(&distance_sq)
            && mob.on_ground
            && rng.random_range(0..5) == 0
        {
            let (dx, dz) = (pos[0] - mob.pos[0], pos[2] - mob.pos[2]);
            let length = dx.hypot(dz);
            if length > 1.0e-4 {
                mob.velocity = [
                    dx / length * 0.4 + mob.velocity[0] * 0.2,
                    0.4,
                    dz / length * 0.4 + mob.velocity[2] * 0.2,
                ];
            }
        }
        if replan {
            let max = ((mob.follow_range * 16.0) as usize).max(200);
            mob.stop();
            mob.goal = Goal::Walk {
                target: pos,
                speed: 1.0,
            };
            if !self.navigate(entity_id, pos, 1.0, max)
                && let Some(mob) = self.mobs.get_mut(&entity_id)
            {
                mob.repath += 15;
            }
        } else if let Some(mob) = self.mobs.get_mut(&entity_id)
            && mob.has_path()
        {
            mob.goal = Goal::Walk {
                target: pos,
                speed: 1.0,
            };
        }
        if attack && kind != Kind::Creeper {
            if let Some(mob) = self.mobs.get_mut(&entity_id) {
                mob.attack_cooldown = 20;
            }
            self.mob_swing(entity_id);
            self.mob_attack(entity_id, target);
        }
    }

    /// `Mob.doHurtTarget`: the mob's attack damage, knocking the player
    /// back.
    fn mob_attack(&mut self, entity_id: i32, target: i32) {
        let damage =
            self.attribute(super::Living::Mob(entity_id), "minecraft:attack_damage") as f32;
        let Some(mob) = self.mobs.get(&entity_id) else {
            return;
        };
        let at = mob.pos;
        let name = self.mob_name(entity_id);
        let Some(player) = self.players.get(&target) else {
            return;
        };
        let message = text::translate(
            "death.attack.mob",
            vec![text::player_name(player.name(), player.uuid())]
                .into_iter()
                .chain(name)
                .collect(),
            None,
        );
        let hurt = self.hurt_player(
            target,
            Hit {
                amount: damage,
                damage_type: "minecraft:mob_attack",
                attacker: Some(entity_id),
                direct: Some(entity_id),
                knockback_from: Some(at),
                message,
            },
        );
        // Thorns on the player's armor.
        if hurt {
            self.post_attack(
                super::Living::Player(target),
                Some(entity_id),
                Some(entity_id),
                "minecraft:mob_attack",
            );
        }
    }

    /// `RangedBowAttackGoal`: closer until in range and seen for a second,
    /// then draw the bow for a second and shoot, every two seconds (one on
    /// hard).
    fn bow_attack(&mut self, entity_id: i32, target: i32) {
        let hard = self.context.difficulty == 3;
        let Some(player) = self.players.get(&target) else {
            return;
        };
        let (pos, eyes) = (player.pos, player.eyes());
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return;
        };
        let distance_sq: f64 = (0..3).map(|i| (pos[i] - mob.pos[i]).powi(2)).sum();
        let seen = mob.unseen == 0;
        if seen != (mob.seen > 0) {
            mob.seen = 0;
        }
        mob.seen += if seen { 1 } else { -1 };
        let (yaw, pitch) = face(mob.eyes(), eyes);
        turn_head(mob, yaw, pitch);
        let approach = !(distance_sq <= BOW_RANGE_SQ && mob.seen >= 20);
        let mut shoot = None;
        match mob.drawing {
            Some(drawn) => {
                if !seen && mob.seen < -60 {
                    mob.drawing = None;
                    mob.data_dirty = true;
                } else if seen {
                    let drawn = drawn + 1;
                    if drawn >= 20 {
                        mob.drawing = None;
                        mob.data_dirty = true;
                        mob.attack_time = if hard { 20 } else { 40 };
                        shoot = Some(bow_power(drawn));
                    } else {
                        mob.drawing = Some(drawn);
                    }
                }
            }
            None => {
                mob.attack_time -= 1;
                if mob.attack_time <= 0 && mob.seen >= -60 {
                    mob.drawing = Some(0);
                    mob.data_dirty = true;
                }
            }
        }
        if approach {
            let goal = block_at(pos);
            let stale = mob.navigation.goal != Some(goal) && mob.navigation.ticks % 10 == 0;
            mob.goal = Goal::Walk {
                target: pos,
                speed: 1.0,
            };
            if stale || !mob.has_path() && mob.navigation.goal != Some(goal) {
                let max = ((mob.follow_range * 16.0) as usize).max(200);
                mob.stop();
                self.navigate(entity_id, pos, 1.0, max);
            }
        } else {
            mob.stop();
            mob.goal = Goal::Idle;
        }
        if let Some(power) = shoot {
            self.skeleton_shoot(entity_id, target, power);
        }
    }

    /// `AbstractSkeleton.performRangedAttack`: an arrow aimed a little above
    /// the target, less precise on easier difficulties.
    fn skeleton_shoot(&mut self, entity_id: i32, target: i32, power: f32) {
        let difficulty = self.context.difficulty;
        let Some(player) = self.players.get(&target) else {
            return;
        };
        let aim = [player.pos[0], player.pos[1] + 1.8 / 3.0, player.pos[2]];
        let Some(mob) = self.mobs.get(&entity_id) else {
            return;
        };
        let from = [mob.pos[0], mob.pos[1] + mob.eye_height - 0.1, mob.pos[2]];
        let (dx, dy, dz) = (aim[0] - from[0], aim[1] - from[1], aim[2] - from[2]);
        let horizontal = dx.hypot(dz);
        let mut rng = rand::rng();
        // `AbstractArrow.setBaseDamageFromMob`.
        let base_damage =
            f64::from(power) * 2.0 + triangle(&mut rng, f64::from(difficulty) * 0.11, 0.574_25);
        let inaccuracy = f64::from(14 - i32::from(difficulty) * 4);
        let at = mob.pos;
        self.play_sound_from(
            None,
            "minecraft:entity.skeleton.shoot",
            at,
            SOUND_SOURCE_HOSTILE,
            1.0,
            1.0 / (rng.random::<f32>() * 0.4 + 0.8),
        );
        let Some(item) = self.context.data.item_id("minecraft:arrow") else {
            return;
        };
        let weapon = self
            .mobs
            .get(&entity_id)
            .and_then(|m| {
                m.equipment().iter().find(|(slot, _)| {
                    *slot == ferrocraft_protocol::packets::play::equipment_slot::MAIN_HAND
                })
            })
            .map(|(_, stack)| stack.clone());
        self.shoot_arrow(Shot {
            owner: Some(entity_id),
            from,
            direction: [dx, dy + horizontal * 0.2, dz],
            speed: 1.6,
            inaccuracy,
            base_damage,
            crit: false,
            pickup: Pickup::Disallowed,
            item: crate::stack::plain(item, 1),
            weapon,
        });
    }

    /// After moving: the sun burns the undead, creepers burn their fuse.
    pub(in crate::game) fn after_move_monster(&mut self, entity_id: i32) {
        let data = Arc::clone(&self.context.data);
        let world = Arc::clone(&self.context.world);
        let Some(mob) = self.mobs.get(&entity_id) else {
            return;
        };
        let mut rng = rand::rng();
        // `Mob.isSunBurnTick`.
        if mob.kind.burns_in_daylight() && self.monsters_burn {
            let value = self.light_value(mob);
            let eyes = block_at(mob.eyes());
            let wet = data.physics(world.block(block_at(mob.pos))).fluid == 1;
            if value > 0.5
                && rng.random::<f32>() * 30.0 < (value - 0.4) * 2.0
                && !wet
                && can_see_sky(&world, &data, eyes)
            {
                self.ignite(entity_id, 8);
            }
        }
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return;
        };
        if mob.kind != Kind::Creeper || !mob.alive() {
            return;
        }
        // `Creeper.tick`.
        if mob.ignited && mob.swell_dir != 1 {
            mob.swell_dir = 1;
            mob.data_dirty = true;
        }
        let primed = mob.swell_dir > 0 && mob.swell == 0;
        mob.swell = (mob.swell + mob.swell_dir).max(0);
        let explode = mob.swell >= FUSE;
        if primed {
            let at = mob.pos;
            self.play_sound_from(
                None,
                "minecraft:entity.creeper.primed",
                at,
                SOUND_SOURCE_HOSTILE,
                1.0,
                0.5,
            );
        }
        if explode {
            let Some(mob) = self.mobs.get(&entity_id) else {
                return;
            };
            let (at, charged) = (mob.pos, mob.charged);
            // `Creeper.explodeCreeper`: the blast takes the creeper with it,
            // after naming it in the death messages; a charged creeper's is
            // twice as big.
            self.explode(Blast {
                center: at,
                radius: CREEPER_BLAST * if charged { 2.0 } else { 1.0 },
                source: Some(entity_id),
                attacker: Some(entity_id),
                breaks_blocks: self.rules.flag("mob_griefing"),
                decay: self.rules.flag("mob_explosion_drop_decay"),
            });
            self.remove_mob(entity_id);
        }
    }

    /// Lights a creeper's fuse, as flint and steel does.
    pub(in crate::game) fn ignite_creeper(&mut self, entity_id: i32) -> bool {
        match self.mobs.get_mut(&entity_id) {
            Some(mob) if mob.kind == Kind::Creeper && mob.alive() && !mob.ignited => {
                mob.ignited = true;
                mob.data_dirty = true;
                true
            }
            _ => false,
        }
    }
}

fn lose_target(mob: &mut Mob) {
    mob.target = None;
    mob.unseen = 0;
    mob.stop();
    mob.goal = Goal::Idle;
    mob.data_dirty = true;
}

/// `RandomSource.triangle`: around `mean`, at most `spread` off.
pub(in crate::game) fn triangle(rng: &mut impl Rng, mean: f64, spread: f64) -> f64 {
    mean + spread * (rng.random::<f64>() - rng.random::<f64>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bow_draws_fully_in_a_second() {
        assert_eq!(bow_power(20), 1.0);
        assert!((bow_power(10) - 0.416_666_7).abs() < 1e-6);
    }
}
