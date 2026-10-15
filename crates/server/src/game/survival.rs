//! Survival: digging takes time and drops loot, tools wear out, falling
//! hurts, hunger, eating, dying and respawning
//! (`ServerPlayerGameMode`, `LivingEntity`, `FoodData`).

use std::sync::Arc;

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::codec::{BlockPos, Decode, Encode, VarInt};
use ferrocraft_protocol::packets::play::{
    BlockDestruction, ClientboundContainerClose, DamageEvent, EntityEvent, GameMode, ItemStack,
    LEVEL_EVENT_DESTROY_BLOCK, LevelEvent, LpVec3, PlayerCombatKill, SetEntityMotion, SetHealth,
    SetHeldSlot, SystemChat, equipment_slot, text,
};
use tracing::info;

use super::player::{Digging, Food, Player, Using};
use super::{Encoded, Game, Outbound};
use crate::inventory;
use crate::loot;
use crate::storage::MAX_HEALTH;
use crate::world::chunk_of;

/// `Attributes.SAFE_FALL_DISTANCE`.
const SAFE_FALL_DISTANCE: f64 = 3.0;
/// Ticks a player cannot be hurt again after a hit (half of
/// `invulnerableTime`, which only blocks weaker hits after that).
const HURT_COOLDOWN: u32 = 10;
/// `EntityEvent.DEATH` and `EntityEvent.USE_ITEM_COMPLETE`.
const EVENT_DEATH: u8 = 3;
const EVENT_USE_ITEM_COMPLETE: u8 = 9;
/// What stays behind after eating.
fn use_remainder(item: &str) -> Option<&'static str> {
    match item {
        "minecraft:mushroom_stew"
        | "minecraft:rabbit_stew"
        | "minecraft:beetroot_soup"
        | "minecraft:suspicious_stew" => Some("minecraft:bowl"),
        "minecraft:honey_bottle" => Some("minecraft:glass_bottle"),
        _ => None,
    }
}

/// `BowItem.getUseDuration`: a bow can be held drawn for an hour.
const BOW_USE_TICKS: u32 = 72000;

/// `BowItem.getPowerForTime`: full power after a second of drawing.
pub(super) fn bow_power(ticks: u32) -> f32 {
    let f = ticks as f32 / 20.0;
    ((f * f + f * 2.0) / 3.0).min(1.0)
}

/// Modes in which the player has a body that can starve, fall and die.
fn mortal(mode: GameMode) -> bool {
    matches!(mode, GameMode::Survival | GameMode::Adventure)
}

/// The armor slots of the inventory menu: head, chest, legs, feet.
const ARMOR_SLOTS: [usize; 4] = [5, 6, 7, 8];

/// Damage to a player and where it came from.
pub(super) struct Hit<'a> {
    pub amount: f32,
    /// `minecraft:damage_type`.
    pub damage_type: &'a str,
    /// Who is to blame, and what hit (the arrow, or the same mob).
    pub attacker: Option<i32>,
    pub direct: Option<i32>,
    /// Knocked back away from here (`LivingEntity.knockback(0.4)`).
    pub knockback_from: Option<[f64; 3]>,
    pub message: Nbt,
}

impl Game {
    // --- Digging -----------------------------------------------------------

    /// How much of a block one tick of digging takes away
    /// (`BlockBehaviour.getDestroyProgress`); 1 or more breaks it at once.
    fn destroy_progress(&self, player: &Player, state: u32) -> f32 {
        let data = &self.context.data;
        let Some((id, block)) = data.block_of(state) else {
            return 0.0;
        };
        if block.hardness < 0.0 {
            return 0.0;
        }
        let tool = player
            .inventory
            .equipment(equipment_slot::MAIN_HAND)
            .and_then(|stack| data.item(stack.item))
            .and_then(|item| item.tool.as_ref());
        let mut speed = tool.map_or(1.0, |tool| {
            tool.rules
                .iter()
                .find(|r| r.speed.is_some() && data.in_block_set(&r.blocks, id))
                .and_then(|r| r.speed)
                .unwrap_or(tool.default_mining_speed)
        });
        let eyes = player.eyes();
        let eye_block = BlockPos {
            x: eyes[0].floor() as i32,
            y: eyes[1].floor() as i32,
            z: eyes[2].floor() as i32,
        };
        if data.physics(self.context.world.block(eye_block)).fluid == 1 {
            // `Attributes.SUBMERGED_MINING_SPEED`.
            speed *= 0.2;
        }
        if !player.on_ground {
            speed /= 5.0;
        }
        let modifier = if self.harvests(player, state) {
            30.0
        } else {
            100.0
        };
        speed / block.hardness / modifier
    }

    /// `Player.hasCorrectToolForDrops`.
    fn harvests(&self, player: &Player, state: u32) -> bool {
        let data = &self.context.data;
        let Some((id, block)) = data.block_of(state) else {
            return false;
        };
        !block.requires_tool
            || player
                .inventory
                .equipment(equipment_slot::MAIN_HAND)
                .and_then(|stack| data.item(stack.item))
                .and_then(|item| item.tool.as_ref())
                .and_then(|tool| {
                    tool.rules
                        .iter()
                        .find(|r| r.correct_for_drops.is_some() && data.in_block_set(&r.blocks, id))
                })
                .and_then(|rule| rule.correct_for_drops)
                .unwrap_or(false)
    }

    /// Cracks of a block someone digs, for everyone else.
    fn show_cracks(&self, entity_id: i32, pos: BlockPos, stage: i32) {
        let packet = Encoded::new(&BlockDestruction {
            id: entity_id,
            pos,
            progress: if (0..10).contains(&stage) {
                stage as u8
            } else {
                255
            },
        });
        self.broadcast(
            Some(entity_id),
            &Outbound::InChunk {
                chunk: chunk_of(pos),
                packet: Arc::new(packet),
            },
        );
    }

    /// `START_DESTROY_BLOCK` in survival: breaks at once what is soft
    /// enough, starts digging otherwise.
    pub(super) fn start_digging(&mut self, entity_id: i32, pos: BlockPos, sequence: i32) {
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let state = self.context.world.block(pos);
        let progress = self.destroy_progress(player, state);
        if let Some(previous) = player.digging
            && previous.pos != pos
        {
            self.show_cracks(entity_id, previous.pos, -1);
        }
        if state != self.air && progress >= 1.0 {
            if let Some(player) = self.players.get_mut(&entity_id) {
                player.digging = None;
            }
            let changed = self.destroy(entity_id, pos);
            return self.finish_block_action(entity_id, sequence, &changed);
        }
        let stage = (progress * 10.0) as i32;
        if let Some(player) = self.players.get_mut(&entity_id) {
            player.digging = Some(Digging {
                pos,
                started: self.tick,
                stage,
                delayed: false,
            });
        }
        self.show_cracks(entity_id, pos, stage);
        self.finish_block_action(entity_id, sequence, &[]);
    }

    /// `STOP_DESTROY_BLOCK`: the client thinks the block is gone. Vanilla
    /// believes it from 70 % on; earlier, the block breaks once the time
    /// is really up.
    pub(super) fn stop_digging(&mut self, entity_id: i32, pos: BlockPos, sequence: i32) {
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        if let Some(digging) = player.digging.filter(|d| d.pos == pos && !d.delayed) {
            let state = self.context.world.block(pos);
            let ticks = self.tick - digging.started;
            let progress = self.destroy_progress(player, state) * (ticks + 1) as f32;
            if state == self.air || progress >= 0.7 {
                if let Some(player) = self.players.get_mut(&entity_id) {
                    player.digging = None;
                }
                self.show_cracks(entity_id, pos, -1);
                let changed = if state == self.air {
                    vec![pos]
                } else {
                    self.destroy(entity_id, pos)
                };
                return self.finish_block_action(entity_id, sequence, &changed);
            }
            if let Some(player) = self.players.get_mut(&entity_id) {
                player.digging = Some(Digging {
                    delayed: true,
                    ..digging
                });
            }
        }
        self.finish_block_action(entity_id, sequence, &[pos]);
    }

    pub(super) fn abort_digging(&mut self, entity_id: i32, pos: BlockPos, sequence: i32) {
        if let Some(player) = self.players.get_mut(&entity_id)
            && player.digging.is_some_and(|d| !d.delayed)
        {
            player.digging = None;
            self.show_cracks(entity_id, pos, -1);
        }
        self.finish_block_action(entity_id, sequence, &[pos]);
    }

    /// Digging goes on: cracks for the others, a delayed break when its time
    /// comes (`ServerPlayerGameMode.tick`).
    fn tick_digging(&mut self, entity_id: i32) {
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let Some(digging) = player.digging else {
            return;
        };
        let state = self.context.world.block(digging.pos);
        if state == self.air {
            if let Some(player) = self.players.get_mut(&entity_id) {
                player.digging = None;
            }
            return self.show_cracks(entity_id, digging.pos, -1);
        }
        let progress =
            self.destroy_progress(player, state) * (self.tick - digging.started + 1) as f32;
        if digging.delayed {
            if progress >= 1.0 {
                if let Some(player) = self.players.get_mut(&entity_id) {
                    player.digging = None;
                }
                self.show_cracks(entity_id, digging.pos, -1);
                self.destroy(entity_id, digging.pos);
            }
            return;
        }
        let stage = (progress * 10.0) as i32;
        if stage != digging.stage {
            if let Some(player) = self.players.get_mut(&entity_id) {
                player.digging = Some(Digging { stage, ..digging });
            }
            self.show_cracks(entity_id, digging.pos, stage);
        }
    }

    /// `ServerPlayerGameMode.destroyBlock` in survival: the block goes,
    /// with its loot if the tool is right, and the tool wears.
    fn destroy(&mut self, entity_id: i32, pos: BlockPos) -> Vec<BlockPos> {
        let Some(player) = self.players.get(&entity_id) else {
            return Vec::new();
        };
        let state = self.context.world.block(pos);
        let harvests = self.harvests(player, state);
        let tool = player
            .inventory
            .equipment(equipment_slot::MAIN_HAND)
            .cloned();
        let removed = self.removal(pos);
        let mut drops = Vec::new();
        if harvests {
            let data = &self.context.data;
            let world = &self.context.world;
            let mut rng = rand::rng();
            for &(at, state) in &removed {
                let Some((_, block)) = data.block_of(state) else {
                    continue;
                };
                let context = loot::Context {
                    data,
                    table: loot::block_table(block),
                    state,
                    pos: at,
                    tool: tool.as_ref(),
                    block_at: &|p| world.block(p),
                    entity: None,
                    by_player: true,
                    explosion: None,
                };
                drops.extend(context.drops(&mut rng).into_iter().map(|d| (at, d)));
            }
        }
        let hardness = self
            .context
            .data
            .block_of(state)
            .map_or(0.0, |(_, b)| b.hardness);
        let changed = self.break_block(pos, Some(entity_id));
        for (at, stack) in drops {
            self.pop_resource(at, stack);
        }
        self.exhaust(entity_id, 0.005);
        let per_block = tool
            .as_ref()
            .and_then(|t| self.context.data.item(t.item))
            .and_then(|i| i.tool.as_ref())
            .map(|t| t.damage_per_block);
        if hardness != 0.0
            && let (Some(amount), Some(player)) = (per_block, self.players.get(&entity_id))
        {
            let slot = player.inventory.selected_slot();
            self.wear(entity_id, slot, amount);
        }
        changed
    }

    /// `ItemStack.hurtAndBreak`: `amount` more points of `minecraft:damage`
    /// for the item in a slot; at `max_damage` it breaks.
    pub(super) fn wear(&mut self, entity_id: i32, slot: usize, amount: i32) {
        let data = &self.context.data;
        let Ok(damage_type) = data.data_component_type("minecraft:damage") else {
            return;
        };
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        if !mortal(player.game_mode()) {
            return;
        }
        let Some(mut stack) = player.inventory.get(slot).cloned() else {
            return;
        };
        let Some(max_damage) = data.item(stack.item).and_then(|i| i.max_damage) else {
            return;
        };
        let current = stack
            .patch
            .added
            .iter()
            .find(|(t, _)| *t == damage_type)
            .and_then(|(_, bytes)| VarInt::decode(&mut bytes.as_slice()).ok())
            .map_or(0, |v| v.0);
        let damage = current + amount;
        let worn = if damage >= max_damage {
            // `EntityEvent` 47: the main hand's item breaks, with sound and
            // particles.
            let event = Encoded::new(&EntityEvent {
                entity_id,
                event: if slot == inventory::OFF_HAND { 48 } else { 47 },
            });
            player.send(Outbound::Packet(Arc::new(event.clone())));
            self.broadcast_entity(entity_id, vec![event]);
            None
        } else {
            let mut bytes = Vec::new();
            VarInt(damage).encode(&mut bytes);
            stack.patch.added.retain(|(t, _)| *t != damage_type);
            stack.patch.added.push((damage_type, bytes));
            Some(stack)
        };
        self.update_inventory(entity_id, |inventory| inventory.set(slot, worn));
        self.send_slots(entity_id, &[slot]);
    }

    /// The block at `pos` and the other half of a door or bed, with their
    /// states.
    pub(super) fn removal(&self, pos: BlockPos) -> Vec<(BlockPos, u32)> {
        let data = &self.context.data;
        let world = &self.context.world;
        let state = world.block(pos);
        let mut removed = vec![(pos, state)];
        if let Some((id, block)) = data.block_of(state)
            && let Some(other) = crate::placement::partner(block, state, pos)
        {
            let other_state = world.block(other);
            if data
                .block_of(other_state)
                .is_some_and(|(other_id, _)| other_id == id)
            {
                removed.push((other, other_state));
            }
        }
        removed
    }

    /// The particles and sound of a breaking block, for everyone but the
    /// one who broke it, whose client shows them already.
    pub(super) fn break_effect(&self, except: Option<i32>, pos: BlockPos, state: u32) {
        let packet = Encoded::new(&LevelEvent {
            event: LEVEL_EVENT_DESTROY_BLOCK,
            pos,
            data: state as i32,
            global: false,
        });
        self.broadcast(
            except,
            &Outbound::InChunk {
                chunk: chunk_of(pos),
                packet: Arc::new(packet),
            },
        );
    }

    // --- Health ------------------------------------------------------------

    pub(super) fn send_health(&self, entity_id: i32) {
        if let Some(player) = self.players.get(&entity_id) {
            player.send(Outbound::Packet(Arc::new(Encoded::new(&SetHealth {
                health: player.health,
                food: player.food.level,
                saturation: player.food.saturation,
            }))));
        }
    }

    /// `Player.causeFoodExhaustion`; not for players who cannot be hurt.
    pub(super) fn exhaust(&mut self, entity_id: i32, amount: f32) {
        if let Some(player) = self.players.get_mut(&entity_id)
            && mortal(player.game_mode())
        {
            player.food.exhaustion = (player.food.exhaustion + amount).min(40.0);
        }
    }

    /// A move the client reported: jumping and sprinting make hungry,
    /// landing after a fall hurts (`Entity.checkFallDamage`).
    pub(super) fn moved(
        &mut self,
        entity_id: i32,
        delta: [f64; 3],
        was_on_ground: bool,
        on_ground: bool,
    ) {
        let data = &self.context.data;
        let world = &self.context.world;
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        if player.dead {
            return;
        }
        let feet = BlockPos {
            x: player.pos[0].floor() as i32,
            y: player.pos[1].floor() as i32,
            z: player.pos[2].floor() as i32,
        };
        let feet_state = world.block(feet);
        let in_water = data.physics(feet_state).fluid == 1;
        let climbing = data
            .block_of(feet_state)
            .is_some_and(|(id, _)| data.block_in_tag("minecraft:climbable", id));
        let mut exhaustion = 0.0;
        if was_on_ground && !on_ground && delta[1] > 0.0 {
            exhaustion += if player.sprinting { 0.2 } else { 0.05 };
        }
        let horizontal = (delta[0] * delta[0] + delta[2] * delta[2]).sqrt() as f32;
        if in_water {
            exhaustion += 0.01 * horizontal;
        } else if player.sprinting && on_ground {
            exhaustion += 0.1 * horizontal;
        }

        let mut damage = 0.0;
        let mut landed = None;
        if !mortal(player.game_mode()) || player.flying || in_water || climbing {
            player.fall_distance = 0.0;
        } else {
            if delta[1] < 0.0 {
                player.fall_distance -= delta[1];
            }
            if on_ground {
                let below = BlockPos {
                    x: feet.x,
                    y: (player.pos[1] - 0.2).floor() as i32,
                    z: feet.z,
                };
                // `Block.fallOn`: soft blocks take some of the fall.
                let multiplier = match data
                    .block_of(world.block(below))
                    .map(|(_, b)| b.name.as_str())
                {
                    Some("minecraft:hay_block" | "minecraft:honey_block") => 0.2,
                    Some("minecraft:slime_block") if !player.sneaking => 0.0,
                    Some(name) if name.ends_with("_bed") => 0.5,
                    _ => 1.0,
                };
                damage =
                    ((player.fall_distance + 1.0e-6 - SAFE_FALL_DISTANCE) * multiplier).floor();
                if player.fall_distance > 0.0 {
                    landed = Some((below, player.fall_distance));
                }
                player.fall_distance = 0.0;
            }
        }
        self.exhaust(entity_id, exhaustion);
        if let Some((below, distance)) = landed {
            self.trample(below, distance);
        }
        if damage > 0.0 {
            let name = self.players[&entity_id].name().to_owned();
            let uuid = self.players[&entity_id].uuid();
            self.hurt(
                entity_id,
                damage as f32,
                "minecraft:fall",
                text::translate(
                    "death.attack.fall",
                    vec![text::player_name(&name, uuid)],
                    None,
                ),
            );
        }
    }

    /// Damage without an attacker, such as a fall.
    pub(super) fn hurt(
        &mut self,
        entity_id: i32,
        amount: f32,
        damage_type: &str,
        death_message: Nbt,
    ) {
        self.hurt_player(
            entity_id,
            Hit {
                amount,
                damage_type,
                attacker: None,
                direct: None,
                knockback_from: None,
                message: death_message,
            },
        );
    }

    /// `Player.hurtServer` and `LivingEntity.hurtServer` for players: the
    /// difficulty scales what mobs and explosions do, armor takes its part
    /// and wears, health goes down, the client flinches and is knocked
    /// back, everyone sees it; at zero the player dies. Returns whether it
    /// hurt.
    pub(super) fn hurt_player(&mut self, entity_id: i32, hit: Hit) -> bool {
        let data = Arc::clone(&self.context.data);
        let source_type = data
            .registry_id("minecraft:damage_type", hit.damage_type)
            .unwrap_or(0);
        let bypasses = hit.damage_type == "minecraft:generic_kill";
        let armored = !data.tag_contains(
            "minecraft:damage_type",
            "minecraft:bypasses_armor",
            source_type,
        );
        // `DamageScaling`: explosions always, the rest when a mob is to
        // blame.
        let by_mob = hit.attacker.is_some_and(|a| !self.players.contains_key(&a));
        let scales = matches!(
            hit.damage_type,
            "minecraft:explosion" | "minecraft:player_explosion"
        ) || by_mob;
        let difficulty = self.context.difficulty;
        let Some(player) = self.players.get(&entity_id) else {
            return false;
        };
        if player.dead || (!bypasses && (!mortal(player.game_mode()) || player.invulnerable > 0)) {
            return false;
        }
        let mut amount = hit.amount;
        if scales {
            amount = match difficulty {
                0 => 0.0,
                1 => (amount / 2.0 + 1.0).min(amount),
                3 => amount * 3.0 / 2.0,
                _ => amount,
            };
        }
        if amount <= 0.0 {
            return false;
        }
        let mut wear: Vec<(usize, i32)> = Vec::new();
        if armored {
            // `LivingEntity.hurtArmor`: a point per four of the damage on
            // each piece, then `CombatRules.getDamageAfterAbsorb`.
            let points = (amount / 4.0).max(1.0) as i32;
            let (mut armor, mut toughness) = (0.0, 0.0);
            for slot in ARMOR_SLOTS {
                if let Some(item) = player.inventory.get(slot).and_then(|s| data.item(s.item)) {
                    armor += item.armor;
                    toughness += item.armor_toughness;
                    if item.armor > 0.0 {
                        wear.push((slot, points));
                    }
                }
            }
            amount = super::mob::after_armor(amount, armor, toughness);
        }
        let Some(player) = self.players.get_mut(&entity_id) else {
            return false;
        };
        player.health = (player.health - amount).max(0.0);
        player.invulnerable = HURT_COOLDOWN;
        player.data_dirty = true;
        player.state = None;
        let packet = Encoded::new(&DamageEvent {
            entity_id,
            source_type,
            cause: hit.attacker,
            direct: hit.direct.or(hit.attacker),
            source_pos: None,
        });
        player.send(Outbound::Packet(Arc::new(packet.clone())));
        if let Some(at) = hit.knockback_from {
            // The client moves itself, so it is told directly.
            let (dx, dz) = (at[0] - player.pos[0], at[2] - player.pos[2]);
            let length = dx.hypot(dz).max(1.0e-4);
            player.send(Outbound::Packet(Arc::new(Encoded::new(&SetEntityMotion {
                entity_id,
                velocity: LpVec3([-dx / length * 0.4, 0.4, -dz / length * 0.4]),
            }))));
        }
        let dead = player.health <= 0.0;
        self.broadcast_entity(entity_id, vec![packet]);
        // `DamageType.exhaustion`, 0.1 for blows, nothing for falls and
        // hunger.
        if !matches!(
            hit.damage_type,
            "minecraft:fall" | "minecraft:starve" | "minecraft:on_fire" | "minecraft:generic"
        ) {
            self.exhaust(entity_id, 0.1);
        }
        for (slot, points) in wear {
            self.wear(entity_id, slot, points);
        }
        self.send_health(entity_id);
        if dead {
            self.die(entity_id, hit.message);
        }
        true
    }

    /// `ServerPlayer.die`: the death screen, a message for everyone, the
    /// inventory scattered on the ground.
    pub(super) fn die(&mut self, entity_id: i32, message: Nbt) {
        if let Some(player) = self.players.get(&entity_id)
            && let Some(menu) = &player.menu
        {
            player.send(Outbound::Packet(Arc::new(Encoded::new(
                &ClientboundContainerClose {
                    container_id: menu.id,
                },
            ))));
        }
        self.close_menu(entity_id, true);
        self.stop_using(entity_id);
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        player.dead = true;
        player.health = 0.0;
        player.digging = None;
        player.fall_distance = 0.0;
        player.data_dirty = true;
        player.state = None;
        info!(name = %player.name(), "died");
        player.send(Outbound::Packet(Arc::new(Encoded::new(
            &PlayerCombatKill {
                player_id: entity_id,
                message: message.clone(),
            },
        ))));
        let death = Encoded::new(&EntityEvent {
            entity_id,
            event: EVENT_DEATH,
        });
        player.send(Outbound::Packet(Arc::new(death.clone())));
        let stacks: Vec<ItemStack> = player
            .inventory
            .stacks()
            .into_iter()
            .map(|(_, s)| s)
            .collect();
        self.broadcast_entity(entity_id, vec![death]);
        self.broadcast_packet(
            None,
            &SystemChat {
                content: message,
                overlay: false,
            },
        );
        self.update_inventory(entity_id, |inventory| {
            *inventory = inventory::Inventory::new(&[], inventory.selected());
        });
        for stack in stacks {
            self.scatter_from(entity_id, stack);
        }
        self.send_menu(entity_id);
        // `Player.getBaseExperienceReward`: seven points a level, at most a
        // hundred; the rest is lost.
        if let Some(player) = self.players.get_mut(&entity_id) {
            let dropped = (player.experience.level * 7).min(100);
            let at = player.pos;
            player.experience = super::Experience {
                score: player.experience.score,
                ..Default::default()
            };
            if dropped > 0 && player.game_mode() != GameMode::Spectator {
                self.award_experience(at, dropped);
            }
        }
    }

    /// The respawn button: back to the world spawn with full health.
    pub(super) fn respawn(&mut self, entity_id: i32) {
        let spawn = self.context.level.spawn;
        let angle = self.context.level.spawn_angle;
        let player_type = self.player_type;
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        if !player.dead {
            return;
        }
        player.dead = false;
        player.health = MAX_HEALTH;
        player.food = Food::default();
        player.invulnerable = 0;
        player.flying = false;
        player.pos = [
            f64::from(spawn.x) + 0.5,
            f64::from(spawn.y),
            f64::from(spawn.z) + 0.5,
        ];
        [player.yaw, player.pitch] = angle;
        player.tracker =
            super::tracker::Tracker::new(&player.pose(), super::player::PLAYER_UPDATE_INTERVAL);
        player.state = None;
        player.data_dirty = false;
        player.send(Outbound::Respawn {
            pos: player.pos,
            rot: [player.yaw, player.pitch],
            game_mode: player.game_mode(),
        });
        let state = player.state(player_type);
        self.broadcast(Some(entity_id), &Outbound::Respawned(state));
        self.send_health(entity_id);
        self.send_experience(entity_id);
        self.send_menu(entity_id);
        if let Some(player) = self.players.get(&entity_id) {
            let slot = player.inventory.selected() as i32;
            player.send(Outbound::Packet(Arc::new(Encoded::new(&SetHeldSlot {
                slot,
            }))));
            player.send(Outbound::Packet(Arc::new(self.time_packet())));
        }
    }

    // --- Eating ------------------------------------------------------------

    /// `use_item`: starts eating what is in hand, or drawing a bow, if the
    /// player may.
    pub(super) fn use_item(&mut self, entity_id: i32, hand: i32, sequence: i32) {
        self.finish_block_action(entity_id, sequence, &[]);
        let data = Arc::clone(&self.context.data);
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        if player.dead || player.game_mode() == GameMode::Spectator {
            return;
        }
        let slot = if hand == 1 {
            inventory::OFF_HAND
        } else {
            player.inventory.selected_slot()
        };
        if matches!(player.game_mode(), GameMode::Survival | GameMode::Creative)
            && self.use_bucket(entity_id, slot)
        {
            return;
        }
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let Some(stack) = player.inventory.get(slot) else {
            return;
        };
        let Some(item) = data.item(stack.item) else {
            return;
        };
        let ticks = if item.name == "minecraft:bow" {
            // `BowItem.use`: only with arrows, unless in creative.
            if player.game_mode() != GameMode::Creative && self.arrow_slot(entity_id).is_none() {
                return;
            }
            BOW_USE_TICKS
        } else {
            let Some(food) = &item.food else {
                return;
            };
            // `Player.canEat`.
            let may_eat =
                !mortal(player.game_mode()) || food.can_always_eat || player.food.level < 20;
            if !may_eat {
                return;
            }
            (item.consume_seconds.unwrap_or(1.6) * 20.0)
                .round()
                .max(1.0) as u32
        };
        let item = stack.item;
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        player.using = Some(Using {
            hand,
            slot,
            item,
            ticks_left: ticks,
        });
        player.data_dirty = true;
        player.state = None;
    }

    /// The slot of the arrows a bow shoots (`Player.getProjectile`): in a
    /// hand first, then the inventory from the hotbar on.
    fn arrow_slot(&self, entity_id: i32) -> Option<usize> {
        let player = self.players.get(&entity_id)?;
        let data = &self.context.data;
        let is_arrow = |slot: usize| {
            player
                .inventory
                .get(slot)
                .and_then(|s| data.item(s.item))
                .is_some_and(|i| {
                    matches!(
                        i.name.as_str(),
                        "minecraft:arrow" | "minecraft:spectral_arrow" | "minecraft:tipped_arrow"
                    )
                })
        };
        [inventory::OFF_HAND, player.inventory.selected_slot()]
            .into_iter()
            .chain(36..45)
            .chain(9..36)
            .find(|&slot| is_arrow(slot))
    }

    /// Letting go of the use button: a drawn bow shoots
    /// (`BowItem.releaseUsing`), anything else just stops.
    pub(super) fn release_using(&mut self, entity_id: i32) {
        let data = Arc::clone(&self.context.data);
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let Some(using) = player.using else {
            return;
        };
        let bow = data
            .item(using.item)
            .is_some_and(|i| i.name == "minecraft:bow");
        let creative = player.game_mode() == GameMode::Creative;
        self.stop_using(entity_id);
        if !bow {
            return;
        }
        let held = BOW_USE_TICKS - using.ticks_left;
        let power = bow_power(held);
        let ammo = self.arrow_slot(entity_id);
        if power < 0.1 || (ammo.is_none() && !creative) {
            return;
        }
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let (yaw, pitch) = (
            f64::from(player.yaw).to_radians(),
            f64::from(player.pitch).to_radians(),
        );
        let eyes = player.eyes();
        let at = player.pos;
        self.shoot_arrow(super::projectile::Shot {
            owner: Some(entity_id),
            from: [eyes[0], eyes[1] - 0.1, eyes[2]],
            // `Projectile.shootFromRotation`.
            direction: [
                -yaw.sin() * pitch.cos(),
                -pitch.sin(),
                yaw.cos() * pitch.cos(),
            ],
            speed: f64::from(power) * 3.0,
            inaccuracy: 1.0,
            base_damage: 2.0,
            crit: power >= 1.0,
            pickup: if creative {
                super::projectile::Pickup::CreativeOnly
            } else {
                super::projectile::Pickup::Allowed
            },
        });
        let pitch = 1.0 / (rand::random::<f32>() * 0.4 + 1.2) + power * 0.5;
        self.play_sound_from(
            None,
            "minecraft:entity.arrow.shoot",
            at,
            ferrocraft_protocol::packets::play::SOUND_SOURCE_PLAYERS,
            1.0,
            pitch,
        );
        if !creative {
            if let Some(ammo) = ammo {
                self.update_inventory(entity_id, |inventory| {
                    inventory.take(ammo, 1);
                });
                self.send_slots(entity_id, &[ammo]);
            }
            self.wear(entity_id, using.slot, 1);
        }
    }

    pub(super) fn stop_using(&mut self, entity_id: i32) {
        if let Some(player) = self.players.get_mut(&entity_id)
            && player.using.take().is_some()
        {
            player.data_dirty = true;
            player.state = None;
        }
    }

    /// One tick of eating; at the end the food counts and is used up
    /// (`LivingEntity.completeUsingItem`).
    fn tick_using(&mut self, entity_id: i32) {
        let data = &self.context.data;
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        let Some(mut using) = player.using else {
            return;
        };
        if player.inventory.get(using.slot).map(|s| s.item) != Some(using.item) {
            return self.stop_using(entity_id);
        }
        using.ticks_left -= 1;
        if using.ticks_left > 0 {
            player.using = Some(using);
            return;
        }
        let Some(item) = data.item(using.item) else {
            return self.stop_using(entity_id);
        };
        if item.food.is_none() {
            // A bow held for the full hour simply stays drawn.
            return self.stop_using(entity_id);
        }
        if let Some(food) = &item.food {
            // `FoodData.add`.
            let level = (player.food.level + food.nutrition).clamp(0, 20);
            player.food.level = level;
            player.food.saturation =
                (player.food.saturation + food.saturation).clamp(0.0, level as f32);
        }
        let consume = mortal(player.game_mode());
        let remainder = use_remainder(&item.name).and_then(|name| data.item_id(name));
        player.send(Outbound::Packet(Arc::new(Encoded::new(&EntityEvent {
            entity_id,
            event: EVENT_USE_ITEM_COMPLETE,
        }))));
        self.stop_using(entity_id);
        if consume {
            let slot = using.slot;
            self.update_inventory(entity_id, |inventory| {
                inventory.take(slot, 1);
            });
            if let Some(remainder) = remainder {
                let bowl = ItemStack {
                    item: remainder,
                    count: 1,
                    patch: Default::default(),
                };
                let empty = self
                    .players
                    .get(&entity_id)
                    .is_some_and(|p| p.inventory.get(slot).is_none());
                if empty {
                    self.update_inventory(entity_id, |inventory| inventory.set(slot, Some(bowl)));
                } else {
                    self.give(entity_id, bowl);
                }
            }
            self.send_slots(entity_id, &[slot]);
        }
        self.send_health(entity_id);
    }

    /// `FoodData.tick`: exhaustion eats saturation, then food; a full
    /// stomach heals, an empty one hurts: on easy down to half the hearts,
    /// on normal to half a heart, on hard to death. In peaceful hunger and
    /// wounds heal by themselves (`Player.aiStep`).
    fn tick_food(&mut self, entity_id: i32) {
        let difficulty = self.context.difficulty;
        let tick = self.tick;
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        if difficulty == 0 && mortal(player.game_mode()) {
            let mut changed = false;
            if player.health < MAX_HEALTH && tick.is_multiple_of(20) {
                player.health = (player.health + 1.0).min(MAX_HEALTH);
                player.data_dirty = true;
                player.state = None;
                changed = true;
            }
            if player.food.level < 20 && tick.is_multiple_of(10) {
                player.food.level += 1;
                changed = true;
            }
            if changed {
                self.send_health(entity_id);
            }
            return;
        }
        let before = player.food;
        let food = &mut player.food;
        if food.exhaustion > 4.0 {
            food.exhaustion -= 4.0;
            if food.saturation > 0.0 {
                food.saturation = (food.saturation - 1.0).max(0.0);
            } else {
                food.level = (food.level - 1).max(0);
            }
        }
        let hurt = player.health < MAX_HEALTH && player.health > 0.0;
        let mut heal = 0.0;
        let mut starve = false;
        if food.saturation > 0.0 && hurt && food.level >= 20 {
            food.timer += 1;
            if food.timer >= 10 {
                let spent = food.saturation.min(6.0);
                heal = spent / 6.0;
                food.exhaustion = (food.exhaustion + spent).min(40.0);
                food.timer = 0;
            }
        } else if food.level >= 18 && hurt {
            food.timer += 1;
            if food.timer >= 80 {
                heal = 1.0;
                food.exhaustion = (food.exhaustion + 6.0).min(40.0);
                food.timer = 0;
            }
        } else if food.level <= 0 {
            food.timer += 1;
            if food.timer >= 80 {
                starve = match difficulty {
                    1 => player.health > 10.0,
                    2 => player.health > 1.0,
                    _ => true,
                };
                food.timer = 0;
            }
        } else {
            food.timer = 0;
        }
        if heal > 0.0 {
            player.health = (player.health + heal).min(MAX_HEALTH);
            player.data_dirty = true;
            player.state = None;
        }
        let changed = heal > 0.0
            || player.food.level != before.level
            || player.food.saturation != before.saturation;
        if starve {
            let name = player.name().to_owned();
            let uuid = player.uuid();
            self.hurt(
                entity_id,
                1.0,
                "minecraft:starve",
                text::translate(
                    "death.attack.starve",
                    vec![text::player_name(&name, uuid)],
                    None,
                ),
            );
        } else if changed {
            self.send_health(entity_id);
        }
    }

    /// Everything about players that goes on by itself.
    pub(super) fn tick_players(&mut self) {
        let ids: Vec<i32> = self.players.keys().copied().collect();
        for entity_id in ids {
            let Some(player) = self.players.get_mut(&entity_id) else {
                continue;
            };
            if player.dead {
                continue;
            }
            player.invulnerable = player.invulnerable.saturating_sub(1);
            self.tick_digging(entity_id);
            self.tick_using(entity_id);
            self.tick_food(entity_id);
            self.close_distant_menu(entity_id);
        }
    }

    // --- Picking blocks ----------------------------------------------------

    /// The middle click (`handlePickItemFromBlock`): selects the block's
    /// item if the inventory has it; in creative, conjures it up otherwise.
    pub(super) fn pick_block(&mut self, entity_id: i32, pos: BlockPos, _include_data: bool) {
        let data = &self.context.data;
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        if player.dead || !crate::placement::within_reach(player.eyes(), pos) {
            return;
        }
        let state = self.context.world.block(pos);
        let Some(item) = data
            .block_of(state)
            .and_then(|(_, block)| data.item_id(&block.item))
            .filter(|&item| item != 0)
        else {
            return;
        };
        let stack = ItemStack {
            item,
            count: 1,
            patch: Default::default(),
        };
        let creative = player.game_mode() == GameMode::Creative;
        let mut changed = Vec::new();
        self.update_inventory(entity_id, |inventory| {
            changed = inventory.pick(stack, creative);
        });
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let slot = player.inventory.selected() as i32;
        player.send(Outbound::Packet(Arc::new(Encoded::new(&SetHeldSlot {
            slot,
        }))));
        self.send_slots(entity_id, &changed);
    }
}
