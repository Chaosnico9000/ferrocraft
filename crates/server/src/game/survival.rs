//! Survival: digging takes time and drops loot, tools wear out, falling
//! hurts, hunger, eating, dying and respawning
//! (`ServerPlayerGameMode`, `LivingEntity`, `FoodData`).

use std::sync::Arc;

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::codec::{BlockPos, Decode, Encode, VarInt};
use ferrocraft_protocol::packets::play::{
    BlockDestruction, ClientboundContainerClose, DamageEvent, EntityEvent, GameMode, ItemStack,
    LEVEL_EVENT_DESTROY_BLOCK, LevelEvent, PlayerCombatKill, SetHealth, SetHeldSlot, SystemChat,
    equipment_slot, text,
};
use tracing::info;

use super::player::{Digging, Food, Player, Using};
use super::{Encoded, Game, Outbound, Scheduled};
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

/// Modes in which the player has a body that can starve, fall and die.
fn mortal(mode: GameMode) -> bool {
    matches!(mode, GameMode::Survival | GameMode::Adventure)
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
                    state,
                    block,
                    pos: at,
                    tool: tool.as_ref(),
                    block_at: &|p| world.block(p),
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
        if hardness != 0.0 {
            self.wear_tool(entity_id);
        }
        changed
    }

    /// `ItemStack.hurtAndBreak` for a tool that broke a block: one more
    /// point of `minecraft:damage`, gone at `max_damage`.
    fn wear_tool(&mut self, entity_id: i32) {
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
        let slot = player.inventory.selected_slot();
        let Some(mut stack) = player.inventory.get(slot).cloned() else {
            return;
        };
        let Some(item) = data.item(stack.item) else {
            return;
        };
        let (Some(max_damage), Some(_)) = (item.max_damage, &item.tool) else {
            return;
        };
        let current = stack
            .patch
            .added
            .iter()
            .find(|(t, _)| *t == damage_type)
            .and_then(|(_, bytes)| VarInt::decode(&mut bytes.as_slice()).ok())
            .map_or(0, |v| v.0);
        let damage = current + 1;
        let worn = if damage >= max_damage {
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
                player.fall_distance = 0.0;
            }
        }
        self.exhaust(entity_id, exhaustion);
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

    /// `LivingEntity.hurtServer` for players: health goes down, the client
    /// flinches, everyone sees it; at zero the player dies.
    pub(super) fn hurt(
        &mut self,
        entity_id: i32,
        amount: f32,
        damage_type: &str,
        death_message: Nbt,
    ) {
        let source_type = self
            .context
            .data
            .registry_id("minecraft:damage_type", damage_type)
            .unwrap_or(0);
        let bypasses = damage_type == "minecraft:generic_kill";
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        if player.dead || (!bypasses && (!mortal(player.game_mode()) || player.invulnerable > 0)) {
            return;
        }
        player.health = (player.health - amount).max(0.0);
        player.invulnerable = HURT_COOLDOWN;
        player.data_dirty = true;
        player.state = None;
        let packet = Encoded::new(&DamageEvent {
            entity_id,
            source_type,
            cause: None,
            direct: None,
            source_pos: None,
        });
        player.send(Outbound::Packet(Arc::new(packet.clone())));
        let dead = player.health <= 0.0;
        self.broadcast_entity(entity_id, vec![packet]);
        self.send_health(entity_id);
        if dead {
            self.die(entity_id, death_message);
        }
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

    /// `use_item`: starts eating what is in hand, if the player may.
    pub(super) fn use_item(&mut self, entity_id: i32, hand: i32, sequence: i32) {
        self.finish_block_action(entity_id, sequence, &[]);
        let data = &self.context.data;
        let Some(player) = self.players.get_mut(&entity_id) else {
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
        let Some(stack) = player.inventory.get(slot) else {
            return;
        };
        let Some(item) = data.item(stack.item) else {
            return;
        };
        let Some(food) = &item.food else {
            return;
        };
        // `Player.canEat`.
        let may_eat = !mortal(player.game_mode()) || food.can_always_eat || player.food.level < 20;
        if !may_eat {
            return;
        }
        let ticks = (item.consume_seconds.unwrap_or(1.6) * 20.0)
            .round()
            .max(1.0) as u32;
        player.using = Some(Using {
            hand,
            slot,
            item: stack.item,
            ticks_left: ticks,
        });
        player.data_dirty = true;
        player.state = None;
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
    /// stomach heals, an empty one hurts (difficulty normal).
    fn tick_food(&mut self, entity_id: i32) {
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
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
                starve = player.health > 1.0;
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

    /// Block changes whose time has come, if nothing changed the block in
    /// between.
    pub(super) fn tick_scheduled(&mut self) {
        let now = self.tick;
        let (due, later): (Vec<Scheduled>, Vec<Scheduled>) =
            self.scheduled.drain(..).partition(|s| s.tick <= now);
        self.scheduled = later;
        for change in due {
            if self.context.world.block(change.pos) == change.from {
                self.set_block(change.pos, change.to);
            }
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

    /// Schedules a block change, like a button popping back out.
    pub(super) fn schedule(&mut self, delay: u64, pos: BlockPos, from: u32, to: u32) {
        self.scheduled.push(Scheduled {
            tick: self.tick + delay,
            pos,
            from,
            to,
        });
    }
}
