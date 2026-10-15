//! What the world does to those in it (`Entity.baseTick`,
//! `LivingEntity.baseTick`, `applyEffectsFromBlocks`): fire and lava set
//! players and mobs alight and hurt them, burning hurts a point a second,
//! water and rain put it out; cacti and magma hurt; under water the air
//! runs out and then drowning hurts. Lightning hurts and ignites what it
//! hits and charges creepers. Items burn up in fire and lava.

use std::sync::Arc;

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    EntityDataValues, EntityEvent, GameMode, SetEntityData, data_index, text,
};
use rand::Rng;

use super::blocks::Kind;
use super::fluid::Fluid;
use super::{Encoded, Game, Outbound};
use crate::data::Aabb;
use crate::storage::MAX_AIR;

/// `EntityEvent` 67: bubbles burst around a drowning entity.
const EVENT_DROWN: u8 = 67;
/// `Player.getFireImmuneTicks`; other entities have one.
const PLAYER_FIRE_IMMUNE: i32 = 20;

/// The blocks an entity is in this tick, and what they do
/// (`InsideBlockEffectType`).
#[derive(Debug, Clone, Copy, Default)]
struct Inside {
    /// The damage of the hottest fire: 1, soul fire 2.
    fire: f32,
    lava: bool,
    water: bool,
    cactus: bool,
}

impl Game {
    /// `checkInsideBlocks`: the blocks the box, a hair smaller, touches.
    fn inside(&self, bounds: Aabb) -> Inside {
        let world = &self.context.world;
        let mut inside = Inside::default();
        let shrunk = [
            bounds[0] + 1.0e-5,
            bounds[1] + 1.0e-5,
            bounds[2] + 1.0e-5,
            bounds[3] - 1.0e-5,
            bounds[4] - 1.0e-5,
            bounds[5] - 1.0e-5,
        ];
        for x in shrunk[0].floor() as i32..=shrunk[3].floor() as i32 {
            for y in shrunk[1].floor() as i32..=shrunk[4].floor() as i32 {
                for z in shrunk[2].floor() as i32..=shrunk[5].floor() as i32 {
                    let pos = BlockPos { x, y, z };
                    let state = world.block(pos);
                    if self.is_fire(state) {
                        let damage = if self.is(state, "minecraft:soul_fire") {
                            2.0
                        } else {
                            1.0
                        };
                        inside.fire = inside.fire.max(damage);
                    }
                    if self.kind(state) == Kind::Cactus {
                        inside.cactus = true;
                    }
                    let fluid = self.fluid_state(state);
                    if let Some(kind) = fluid.fluid {
                        // The fluid only as high as it stands.
                        let top = f64::from(y) + f64::from(self.fluid_height(fluid, pos));
                        if shrunk[1] < top {
                            match kind {
                                Fluid::Water => inside.water = true,
                                Fluid::Lava => inside.lava = true,
                            }
                        }
                    }
                }
            }
        }
        inside
    }

    /// `isEyeInFluid(WATER)`, from a little below the eyes like vanilla.
    fn eyes_in_water(&self, eyes: [f64; 3]) -> bool {
        let y = eyes[1] - 0.111_111_11;
        let pos = BlockPos {
            x: eyes[0].floor() as i32,
            y: y.floor() as i32,
            z: eyes[2].floor() as i32,
        };
        let fluid = self.fluid_state(self.context.world.block(pos));
        fluid.fluid == Some(Fluid::Water)
            && f64::from(pos.y) + f64::from(self.fluid_height(fluid, pos)) > y
            && !self.is(self.context.world.block(pos), "minecraft:bubble_column")
    }

    /// `Entity.isInRain`: at the feet or the top of the box.
    fn in_rain(&self, pos: [f64; 3], top: f64) -> bool {
        let feet = BlockPos {
            x: pos[0].floor() as i32,
            y: pos[1].floor() as i32,
            z: pos[2].floor() as i32,
        };
        self.is_raining_at(feet)
            || self.is_raining_at(BlockPos {
                y: top.floor() as i32,
                ..feet
            })
    }

    /// Whether a game rule spares players this damage
    /// (`ServerPlayer.isInvulnerableTo`).
    pub(super) fn damage_ruled_out(&self, damage_type: &str) -> bool {
        let data = &self.context.data;
        let Ok(id) = data.registry_id("minecraft:damage_type", damage_type) else {
            return false;
        };
        let is = |tag| data.tag_contains("minecraft:damage_type", tag, id);
        (is("minecraft:is_fire") && !self.rules.flag("fire_damage"))
            || (is("minecraft:is_drowning") && !self.rules.flag("drowning_damage"))
            || (is("minecraft:is_fall") && !self.rules.flag("fall_damage"))
            || (is("minecraft:is_freezing") && !self.rules.flag("freeze_damage"))
    }

    /// Damage from the world, with vanilla's death message
    /// (`death.attack.<id>`).
    fn hurt_by_world(&mut self, entity_id: i32, amount: f32, damage_type: &str, message: &str) {
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let name = text::player_name(player.name(), player.uuid());
        self.hurt(
            entity_id,
            amount,
            damage_type,
            text::translate(&format!("death.attack.{message}"), vec![name], None),
        );
    }

    /// The shared flags and air of a player to their own client, which
    /// draws flames and bubbles from them.
    pub(super) fn send_own_data(&self, entity_id: i32) {
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let values = player
            .changeable_data()
            .into_iter()
            .filter(|v| matches!(v.index, data_index::SHARED_FLAGS | data_index::AIR_SUPPLY))
            .collect();
        player.send(Outbound::Packet(Arc::new(Encoded::new(&SetEntityData {
            entity_id,
            values: EntityDataValues(values),
        }))));
    }

    /// Fire, lava, cacti, magma, rain and air for a player, once a tick.
    pub(super) fn tick_player_hazards(&mut self, entity_id: i32) {
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let mode = player.game_mode();
        if player.dead || mode == GameMode::Spectator {
            return;
        }
        let creative = mode == GameMode::Creative;
        let (bounds, pos, eyes, on_ground, sneaking) = (
            player.bounds(),
            player.pos,
            player.eyes(),
            player.on_ground,
            player.sneaking,
        );
        let before = (player.fire > 0, player.air);
        let inside = self.inside(bounds);
        let in_rain = self.in_rain(pos, bounds[4]);
        let below = self.context.world.block(BlockPos {
            x: pos[0].floor() as i32,
            y: (pos[1] - 0.2).floor() as i32,
            z: pos[2].floor() as i32,
        });
        let eyes_in_water = self.eyes_in_water(eyes);
        let mut rng = rand::rng();
        let mut hurts: Vec<(f32, &str, &str)> = Vec::new();
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        // `Entity.baseTick`: burning hurts a point a second.
        if player.fire > 0 {
            if player.fire % 20 == 0 && !inside.lava {
                hurts.push((1.0, "minecraft:on_fire", "onFire"));
            }
            player.fire -= 1;
        }
        // `LivingEntity.baseTick`: air under water.
        if eyes_in_water && !creative {
            player.air -= 1;
            if player.air <= -20 {
                player.air = 0;
                let event = Encoded::new(&EntityEvent {
                    entity_id,
                    event: EVENT_DROWN,
                });
                player.send(Outbound::Packet(Arc::new(event.clone())));
                hurts.push((2.0, "minecraft:drown", "drown"));
            }
        } else if player.air < MAX_AIR {
            player.air = (player.air + 4).min(MAX_AIR);
        }
        // `applyEffectsFromBlocks`.
        let fire_before = player.fire;
        if on_ground && self_magma(&below, &self.context.data) && !sneaking {
            hurts.push((1.0, "minecraft:hot_floor", "hotFloor"));
        }
        if inside.fire > 0.0 {
            // `BaseFireBlock.fireIgnite`.
            if player.fire < 0 {
                player.fire += 1;
            } else {
                player.fire += rng.random_range(1..3);
            }
            if player.fire >= 0 {
                player.fire = player.fire.max(160);
            }
            hurts.push((inside.fire, "minecraft:in_fire", "inFire"));
        }
        if inside.lava {
            player.fire = player.fire.max(300);
            hurts.push((4.0, "minecraft:lava", "lava"));
        }
        if inside.water || in_rain {
            player.fire = player.fire.min(0);
        }
        if inside.cactus {
            hurts.push((1.0, "minecraft:cactus", "cactus"));
        }
        let ignited = player.fire > fire_before;
        if player.fire <= 0 && !ignited {
            player.fire = -PLAYER_FIRE_IMMUNE;
        }
        let after = (player.fire > 0, player.air);
        if before != after {
            player.data_dirty = true;
            player.state = None;
        }
        let extinguished = before.0 && !after.0;
        if before != after {
            self.send_own_data(entity_id);
        }
        if extinguished {
            self.extinguish_sound(pos);
        }
        for (amount, damage_type, message) in hurts {
            if !self.damage_ruled_out(damage_type) {
                self.hurt_by_world(entity_id, amount, damage_type, message);
            }
        }
    }

    fn extinguish_sound(&self, pos: [f64; 3]) {
        let mut rng = rand::rng();
        let pitch = 1.6 + (rng.random::<f32>() - rng.random::<f32>()) * 0.4;
        self.play_sound(
            None,
            "minecraft:entity.generic.extinguish_fire",
            pos,
            0.7,
            pitch,
        );
    }

    /// Fire, lava, cacti, rain and air for a mob, once a tick.
    pub(super) fn tick_mob_hazards(&mut self, entity_id: i32) {
        let data = Arc::clone(&self.context.data);
        let Some(mob) = self.mobs.get(&entity_id) else {
            return;
        };
        if !mob.alive() {
            return;
        }
        let (bounds, pos, eyes, type_id) = (mob.bounds(), mob.pos, mob.eyes(), mob.type_id());
        let inside = self.inside(bounds);
        let in_rain = self.in_rain(pos, bounds[4]);
        let breathes_water = data.tag_contains(
            "minecraft:entity_type",
            "minecraft:can_breathe_under_water",
            type_id,
        );
        let eyes_in_water = !breathes_water && self.eyes_in_water(eyes);
        let mut hurts: Vec<(f32, &str)> = Vec::new();
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return;
        };
        let before = mob.fire > 0;
        if eyes_in_water {
            mob.air -= 1;
            if mob.air <= -20 {
                mob.air = 0;
                hurts.push((2.0, "minecraft:drown"));
            }
        } else if mob.air < MAX_AIR {
            mob.air = (mob.air + 4).min(MAX_AIR);
        }
        let fire_before = mob.fire;
        if inside.fire > 0.0 {
            if mob.fire < 0 {
                mob.fire += 1;
            }
            if mob.fire >= 0 {
                mob.fire = mob.fire.max(160);
            }
            hurts.push((inside.fire, "minecraft:in_fire"));
        }
        if inside.lava {
            mob.fire = mob.fire.max(300);
            hurts.push((4.0, "minecraft:lava"));
        }
        if inside.water || in_rain {
            mob.fire = mob.fire.min(0);
        }
        if inside.cactus {
            hurts.push((1.0, "minecraft:cactus"));
        }
        // One tick of immunity once out (`getFireImmuneTicks`).
        if mob.fire <= 0 && mob.fire <= fire_before {
            mob.fire = -1;
        }
        if before != (mob.fire > 0) {
            mob.mark_dirty();
        }
        let extinguished = before && mob.fire <= 0;
        if extinguished {
            self.extinguish_sound(pos);
        }
        for (amount, damage_type) in hurts {
            self.hurt_mob(entity_id, amount, None, damage_type);
        }
    }

    /// Items burn up in fire and lava and break on cacti, unless they
    /// resist fire like netherite.
    pub(super) fn tick_item_hazards(&mut self) {
        let data = Arc::clone(&self.context.data);
        let resistant = data.data_component_type("minecraft:damage_resistant").ok();
        let doomed: Vec<(i32, [f64; 3], bool)> = self
            .items
            .values()
            .filter_map(|item| {
                let inside = self.inside(item.bounds());
                let burns = inside.fire > 0.0 || inside.lava;
                if !burns && !inside.cactus {
                    return None;
                }
                let fireproof = resistant.is_some_and(|t| {
                    item.stack.patch.added.iter().any(|(c, _)| *c == t)
                        || data
                            .item(item.stack.item)
                            .is_some_and(|i| i.name.starts_with("minecraft:netherite"))
                });
                if burns && fireproof && !inside.cactus {
                    return None;
                }
                Some((item.entity_id, item.pos, inside.lava))
            })
            .collect();
        for (id, pos, lava) in doomed {
            if let Some(item) = self.items.get_mut(&id) {
                item.health -= if lava { 4.0 } else { 1.0 };
                if item.health > 0.0 {
                    continue;
                }
            }
            self.remove_item(id);
            if lava {
                let pitch = 2.0 + rand::random::<f32>() * 0.4;
                self.play_sound(None, "minecraft:entity.generic.burn", pos, 0.4, pitch);
            }
        }
    }

    /// How long something without a life of its own burns after this tick
    /// in `bounds`, as an arrow: fire and lava set it alight, water and
    /// rain put it out, otherwise it burns down.
    pub(super) fn burning_thing(&self, bounds: Aabb, pos: [f64; 3], fire: i32) -> i32 {
        let inside = self.inside(bounds);
        let mut fire = (fire - 1).max(0);
        if inside.fire > 0.0 {
            fire = fire.max(160);
        }
        if inside.lava {
            fire = fire.max(300);
        }
        if inside.water || self.in_rain(pos, bounds[4]) {
            fire = 0;
        }
        fire
    }

    /// `igniteForSeconds` for a player.
    pub(super) fn ignite_player(&mut self, entity_id: i32, seconds: i32) {
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        let ticks = seconds * 20;
        if player.fire < ticks {
            let was = player.fire > 0;
            player.fire = ticks;
            if !was {
                player.data_dirty = true;
                player.state = None;
                self.send_own_data(entity_id);
            }
        }
    }

    /// `Entity.thunderHit` for a player.
    pub(super) fn player_thunder_hit(&mut self, entity_id: i32) {
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        player.fire += 1;
        if player.fire == 0 {
            player.fire = 160;
        }
        player.data_dirty = true;
        player.state = None;
        self.send_own_data(entity_id);
        self.hurt_by_world(entity_id, 5.0, "minecraft:lightning_bolt", "lightningBolt");
    }

    /// `Entity.thunderHit` for a mob; a creeper becomes charged.
    pub(super) fn mob_thunder_hit(&mut self, entity_id: i32) {
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return;
        };
        mob.fire += 1;
        if mob.fire == 0 {
            mob.fire = 160;
        }
        mob.thunder_hit();
        self.hurt_mob(entity_id, 5.0, None, "minecraft:lightning_bolt");
    }
}

/// `MagmaBlock`: stepping on it burns.
fn self_magma(state: &u32, data: &crate::data::GameData) -> bool {
    data.state_is(*state, "minecraft:magma_block")
}
