//! What players do to the world and their inventory: using, placing and
//! breaking blocks, the creative inventory, signs, game modes, teleports,
//! `/give`.

use std::sync::Arc;

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    BlockEntityData, ContainerSetSlot, Direction, GameEvent, GameEventKind, GameMode, ItemStack,
    OpenSignEditor, PlayerAction, PlayerInfoGameMode, SOUND_SOURCE_BLOCKS, SignUpdate, Slot, Sound,
    UseItemOn, equipment_slot, player_action, text,
};

use super::menu::{Menu, Source};
use super::player::ENDER_SLOTS;
use super::{Encoded, Game, Outbound};
use crate::block_entity::container_size;
use crate::data::Block;
use crate::inventory::{self, Inventory};
use crate::placement;
use crate::world::{MAX_Y, MIN_Y, chunk_of};

fn in_world(pos: BlockPos) -> bool {
    (MIN_Y..=MAX_Y).contains(&pos.y)
}

/// The material part of door, trapdoor and fence gate sounds
/// (`BlockSetType`, `WoodType`).
fn wood_sound(name: &str) -> &'static str {
    if name.contains("bamboo") {
        "bamboo_wood_"
    } else if name.contains("cherry") {
        "cherry_wood_"
    } else if name.contains("crimson") || name.contains("warped") {
        "nether_wood_"
    } else if name.contains("copper") {
        "copper_"
    } else {
        "wooden_"
    }
}

/// `ChatFormatting.stripFormatting`, which vanilla applies to sign text.
fn strip_formatting(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '§' {
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

impl Game {
    /// Sends the player the content of inventory menu slots after a change
    /// the client did not make itself.
    pub(super) fn send_slots(&mut self, entity_id: i32, slots: &[usize]) {
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        for &slot in slots {
            player.state_id = player.state_id.wrapping_add(1);
            let packet = Encoded::new(&ContainerSetSlot {
                container_id: 0,
                state_id: player.state_id,
                slot: slot as i16,
                stack: Slot(player.inventory.get(slot).cloned()),
            });
            player.send(Outbound::Packet(Arc::new(packet)));
        }
    }

    /// A block sound at a place, for everyone near it but `except`.
    pub(super) fn play_sound(
        &self,
        except: Option<i32>,
        name: &str,
        at: [f64; 3],
        volume: f32,
        pitch: f32,
    ) {
        self.play_sound_from(except, name, at, SOUND_SOURCE_BLOCKS, volume, pitch);
    }

    /// A sound of a `SoundSource` at a place.
    pub(super) fn play_sound_from(
        &self,
        except: Option<i32>,
        name: &str,
        at: [f64; 3],
        source: i32,
        volume: f32,
        pitch: f32,
    ) {
        let Ok(sound) = self.context.data.sound_event(name) else {
            return;
        };
        let packet = Encoded::new(&Sound {
            sound: sound + 1,
            source,
            x: (at[0] * 8.0) as i32,
            y: (at[1] * 8.0) as i32,
            z: (at[2] * 8.0) as i32,
            volume,
            pitch,
            seed: rand::random(),
        });
        let pos = BlockPos {
            x: at[0].floor() as i32,
            y: at[1].floor() as i32,
            z: at[2].floor() as i32,
        };
        self.broadcast(
            except,
            &Outbound::InChunk {
                chunk: chunk_of(pos),
                packet: Arc::new(packet),
            },
        );
    }

    /// `ServerGamePacketListenerImpl.handleSetCreativeModeSlot`.
    pub(super) fn creative_slot(&mut self, entity_id: i32, slot: i16, stack: Option<ItemStack>) {
        let creative = self
            .players
            .get(&entity_id)
            .is_some_and(|p| p.game_mode() == GameMode::Creative);
        if !creative {
            return;
        }
        // Outside the inventory screen: thrown into the world.
        if slot < 0 {
            if let Some(stack) = stack {
                self.drop_from(entity_id, stack);
            }
            return;
        }
        let max = stack
            .as_ref()
            .and_then(|s| self.context.data.item(s.item))
            .map_or(99, |i| i.stack.max(1));
        if stack.as_ref().is_some_and(|s| s.count > max) {
            return;
        }
        match usize::try_from(slot) {
            // The inventory's crafting grid.
            Ok(slot @ 1..=4) => {
                if let Some(player) = self.players.get_mut(&entity_id) {
                    player.inventory_menu.grid[slot - 1] = stack;
                }
            }
            Ok(slot @ 5..inventory::SLOTS) => {
                self.update_inventory(entity_id, |i| i.set(slot, stack));
            }
            _ => {}
        }
    }

    /// A right click on a block (`ServerPlayerGameMode.useItemOn`): the
    /// block's own use (opening a chest, a door), unless the player sneaks
    /// with something in hand; else place the block in hand next to it, or
    /// in its place if it is replaceable like grass.
    pub(super) fn use_item_on(&mut self, entity_id: i32, click: &UseItemOn) {
        let Ok(face) = Direction::from_id(click.face) else {
            return;
        };
        let adjacent = face.offset(click.pos);
        let refresh = [click.pos, adjacent];
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let mode = player.game_mode();
        let valid = !player.dead
            && placement::within_reach(player.eyes(), click.pos)
            && click.cursor.iter().all(|c| (c - 0.5).abs() < 1.000_000_1)
            && in_world(click.pos);
        if !valid {
            return self.finish_block_action(entity_id, click.sequence, &refresh);
        }
        let holds_something = player
            .inventory
            .equipment(equipment_slot::MAIN_HAND)
            .is_some()
            || player
                .inventory
                .equipment(equipment_slot::OFF_HAND)
                .is_some();
        // Only the main hand opens things; the off hand comes second.
        if click.hand == 0
            && (mode == GameMode::Spectator || !(player.sneaking && holds_something))
            && self.use_block(entity_id, click.pos)
        {
            return self.finish_block_action(entity_id, click.sequence, &refresh);
        }
        if !matches!(mode, GameMode::Creative | GameMode::Survival) {
            return self.finish_block_action(entity_id, click.sequence, &refresh);
        }
        let changed = self.place(entity_id, click, face);
        let mut refresh = refresh.to_vec();
        refresh.extend(changed);
        self.finish_block_action(entity_id, click.sequence, &refresh);
    }

    /// Places the block in hand; returns the positions it set.
    fn place(&mut self, entity_id: i32, click: &UseItemOn, face: Direction) -> Vec<BlockPos> {
        let Some(player) = self.players.get(&entity_id) else {
            return Vec::new();
        };
        let slot = if click.hand == 1 {
            inventory::OFF_HAND
        } else {
            player.inventory.selected_slot()
        };
        let data = &self.context.data;
        let world = &self.context.world;
        let Some(block) = player
            .inventory
            .get(slot)
            .and_then(|stack| data.block_for_item(stack.item))
            .filter(|block| !placement::GAME_MASTER_BLOCKS.contains(&block.name.as_str()))
        else {
            return Vec::new();
        };
        let replaceable = data.is_replaceable(world.block(click.pos));
        let target = if replaceable {
            click.pos
        } else {
            face.offset(click.pos)
        };
        let mut blocks = placement::placements(block, target, face, click.cursor[1], player.yaw);
        let mut partner = None;
        if block.name.ends_with("chest") && block.has("type") {
            let (state, joined) =
                placement::connect_chest(block, blocks[0].1, target, face, player.sneaking, |p| {
                    world.block(p)
                });
            blocks[0].1 = state;
            partner = joined;
        }
        let free = blocks.iter().all(|&(pos, state)| {
            in_world(pos)
                && data.is_replaceable(world.block(pos))
                && !self
                    .players
                    .values()
                    .any(|p| p.obstructs(pos, data.collision_shape(state)))
                && !self.mob_obstructs(pos, data.collision_shape(state))
        });
        if !free {
            return blocks.iter().map(|&(pos, _)| pos).collect();
        }
        let survival = player.game_mode() == GameMode::Survival;
        let sign = block
            .block_entity
            .as_deref()
            .is_some_and(|kind| kind.ends_with("sign"));
        for &(pos, state) in &blocks {
            self.set_block(pos, state);
        }
        if let Some((pos, state)) = partner {
            self.set_block(pos, state);
        }
        self.swing(entity_id);
        if survival {
            self.update_inventory(entity_id, |inventory| {
                inventory.take(slot, 1);
            });
            self.send_slots(entity_id, &[slot]);
        }
        if sign && let Some(player) = self.players.get(&entity_id) {
            // `SignItem`: write on it right away, on the front.
            player.send(Outbound::Packet(Arc::new(Encoded::new(&OpenSignEditor {
                pos: blocks[0].0,
                side: 1,
            }))));
        }
        blocks.iter().map(|&(pos, _)| pos).collect()
    }

    /// `BlockBehaviour.useWithoutItem` of the blocks that do something:
    /// screens, doors, trapdoors, fence gates, levers, buttons, signs.
    /// Returns whether the click was used up.
    fn use_block(&mut self, entity_id: i32, pos: BlockPos) -> bool {
        let data = Arc::clone(&self.context.data);
        let state = self.context.world.block(pos);
        let Some((block_id, block)) = data.block_of(state) else {
            return false;
        };
        let spectator = self
            .players
            .get(&entity_id)
            .is_some_and(|p| p.game_mode() == GameMode::Spectator);
        let name = block.name.as_str();
        if name == "minecraft:crafting_table" {
            if spectator {
                return false;
            }
            let mut menu = Menu::crafting(0, pos);
            menu.block = Some(block_id);
            self.open_menu(entity_id, menu, "minecraft:crafting", "container.crafting");
            return true;
        }
        if name == "minecraft:ender_chest" {
            if self.blocked_above(pos) {
                return true;
            }
            let mut menu = Menu::container(
                0,
                (0..ENDER_SLOTS).map(Source::Ender).collect(),
                Vec::new(),
                pos,
            );
            menu.block = Some(block_id);
            self.open_menu(
                entity_id,
                menu,
                "minecraft:generic_9x3",
                "container.enderchest",
            );
            return true;
        }
        if let Some(kind) = block.block_entity.as_deref() {
            return self.open_container(entity_id, pos, block_id, kind);
        }
        if spectator {
            return false;
        }
        if name.ends_with("_door") || name.ends_with("_trapdoor") || name.ends_with("_fence_gate") {
            if name.contains("iron") {
                return false;
            }
            self.toggle_opening(entity_id, pos, block, state);
            return true;
        }
        if name == "minecraft:lever" {
            let powered = block.value(state, "powered") == Some("true");
            if let Some(changed) =
                block.with(state, "powered", if powered { "false" } else { "true" })
            {
                self.set_block(pos, changed);
                let center = [
                    f64::from(pos.x) + 0.5,
                    f64::from(pos.y) + 0.5,
                    f64::from(pos.z) + 0.5,
                ];
                self.play_sound(
                    None,
                    "minecraft:block.lever.click",
                    center,
                    0.3,
                    if powered { 0.5 } else { 0.6 },
                );
            }
            return true;
        }
        if name.ends_with("_button") {
            if block.value(state, "powered") == Some("true") {
                return true;
            }
            let Some(pressed) = block.with(state, "powered", "true") else {
                return false;
            };
            self.set_block(pos, pressed);
            // `ButtonBlock`: wood stays pressed for 30 ticks, stone for 20.
            let stone = name.contains("stone") || name.contains("blackstone");
            let delay = if stone { 20 } else { 30 };
            self.schedule(delay, pos, pressed, state);
            let sound = if stone {
                "stone_button"
            } else {
                "wooden_button"
            };
            let center = [
                f64::from(pos.x) + 0.5,
                f64::from(pos.y) + 0.5,
                f64::from(pos.z) + 0.5,
            ];
            self.play_sound(
                None,
                &format!("minecraft:block.{sound}.click_on"),
                center,
                1.0,
                1.0,
            );
            return true;
        }
        false
    }

    /// `ChestBlock.isChestBlockedAt`: a solid block on top keeps the lid
    /// shut.
    fn blocked_above(&self, pos: BlockPos) -> bool {
        let above = self.context.world.block(Direction::Up.offset(pos));
        let data = &self.context.data;
        data.physics(above).dampening == 15
            && data.collision_shape(above) == [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]
    }

    /// Opens the screen of a container block entity; a double chest shows
    /// both halves, the right one first.
    fn open_container(
        &mut self,
        entity_id: i32,
        pos: BlockPos,
        block_id: usize,
        kind: &str,
    ) -> bool {
        let size = container_size(kind);
        let (menu_type, title) = match kind {
            "minecraft:chest" | "minecraft:trapped_chest" => {
                ("minecraft:generic_9x3", "container.chest")
            }
            "minecraft:barrel" => ("minecraft:generic_9x3", "container.barrel"),
            "minecraft:shulker_box" => ("minecraft:shulker_box", "container.shulkerBox"),
            "minecraft:dispenser" => ("minecraft:generic_3x3", "container.dispenser"),
            "minecraft:dropper" => ("minecraft:generic_3x3", "container.dropper"),
            "minecraft:hopper" => ("minecraft:hopper", "container.hopper"),
            // Furnaces, brewing stands and the rest have screens of their own.
            _ => return false,
        };
        if size == 0 || self.context.world.block_entity(pos).is_none() {
            return false;
        }
        let mut containers = vec![pos];
        let (mut menu_type, mut title) = (menu_type, title);
        if kind.ends_with("chest") {
            if self.blocked_above(pos) {
                return true;
            }
            let data = &self.context.data;
            let state = self.context.world.block(pos);
            let block = data.block(block_id);
            if let Some(towards) = placement::chest_partner(block, state) {
                let other = towards.offset(pos);
                let other_state = self.context.world.block(other);
                let joined = data
                    .block_of(other_state)
                    .is_some_and(|(id, _)| id == block_id)
                    && placement::chest_partner(block, other_state).map(|d| d.offset(other))
                        == Some(pos);
                if joined {
                    if self.blocked_above(other) {
                        return true;
                    }
                    // `DoubleBlockCombiner`: the right half is the first.
                    if block.value(state, "type") == Some("right") {
                        containers.push(other);
                    } else {
                        containers.insert(0, other);
                    }
                    (menu_type, title) = ("minecraft:generic_9x6", "container.chestDouble");
                }
            }
        }
        let sources = containers
            .iter()
            .flat_map(|&c| (0..size).map(move |i| Source::Container(c, i)))
            .collect();
        let mut menu = Menu::container(0, sources, containers, pos);
        menu.block = Some(block_id);
        self.open_menu(entity_id, menu, menu_type, title);
        true
    }

    /// Doors, trapdoors and fence gates open and close by hand.
    fn toggle_opening(&mut self, entity_id: i32, pos: BlockPos, block: &Block, state: u32) {
        let open = block.value(state, "open") == Some("true");
        let Some(mut changed) = block.with(state, "open", if open { "false" } else { "true" })
        else {
            return;
        };
        let name = block.name.clone();
        // `FenceGateBlock`: it swings away from the player.
        if name.ends_with("_fence_gate")
            && !open
            && let Some(player) = self.players.get(&entity_id)
        {
            let looking = placement::looking(player.yaw);
            if block
                .value(changed, "facing")
                .and_then(placement::direction)
                == Some(placement::opposite(looking))
                && let Some(turned) = block.with(changed, "facing", looking.name())
            {
                changed = turned;
            }
        }
        let partner = placement::partner(block, state, pos).filter(|_| name.ends_with("_door"));
        self.set_block(pos, changed);
        if let Some(other) = partner {
            let other_state = self.context.world.block(other);
            if let Some((_, other_block)) = self.context.data.block_of(other_state)
                && other_block.name == name
                && let Some(other_changed) =
                    other_block.with(other_state, "open", if open { "false" } else { "true" })
            {
                self.set_block(other, other_changed);
            }
        }
        let kind = if name.ends_with("_door") {
            "door"
        } else if name.ends_with("_trapdoor") {
            "trapdoor"
        } else {
            "fence_gate"
        };
        let material = if kind == "fence_gate" && wood_sound(&name) == "wooden_" {
            String::new()
        } else {
            wood_sound(&name).to_owned()
        };
        let event = format!(
            "minecraft:block.{material}{kind}.{}",
            if open { "close" } else { "open" }
        );
        let center = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        ];
        let pitch = rand::random::<f32>() * 0.1 + 0.9;
        // The clicking client plays it itself.
        self.play_sound(Some(entity_id), &event, center, 1.0, pitch);
    }

    /// `ServerGamePacketListenerImpl.handleSignUpdate`: new text for a sign
    /// side, for everyone to see.
    pub(super) fn sign_update(&mut self, entity_id: i32, update: &SignUpdate) {
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        if player.dead
            || player.game_mode() == GameMode::Spectator
            || !placement::within_reach(player.eyes(), update.pos)
        {
            return;
        }
        let lines = update.lines.clone().map(|line| strip_formatting(&line));
        let world = &self.context.world;
        let mut tag = None;
        let mut kind = None;
        world.update_block_entity(update.pos, |entity| {
            if entity.sign.is_none() || entity.waxed {
                return;
            }
            entity.set_sign_text(update.side == 1, &lines);
            tag = entity.update_tag();
            kind = Some(entity.kind.clone());
        });
        let (Some(tag), Some(kind)) = (tag, kind) else {
            return;
        };
        let Ok(kind) = self.context.data.block_entity_type(&kind) else {
            return;
        };
        let packet = Encoded::new(&BlockEntityData {
            pos: update.pos,
            kind,
            tag,
        });
        self.broadcast(
            None,
            &Outbound::InChunk {
                chunk: chunk_of(update.pos),
                packet: Arc::new(packet),
            },
        );
    }

    /// A left click on a block and the other `player_action`s.
    pub(super) fn player_action(&mut self, entity_id: i32, action: &PlayerAction) {
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let mode = player.game_mode();
        match action.action {
            player_action::START_DESTROY_BLOCK => {
                let allowed = !player.dead
                    && placement::within_reach(player.eyes(), action.pos)
                    && in_world(action.pos);
                let sword = player
                    .inventory
                    .equipment(equipment_slot::MAIN_HAND)
                    .and_then(|s| self.context.data.item(s.item))
                    .and_then(|i| i.tool.as_ref())
                    .is_some_and(|t| !t.can_destroy_blocks_in_creative);
                match mode {
                    GameMode::Creative if allowed && !sword => {
                        let changed = self.break_block(action.pos, Some(entity_id));
                        self.finish_block_action(entity_id, action.sequence, &changed);
                    }
                    GameMode::Survival if allowed => {
                        self.start_digging(entity_id, action.pos, action.sequence);
                    }
                    // Adventure and spectators do not break blocks.
                    _ => self.finish_block_action(entity_id, action.sequence, &[action.pos]),
                }
            }
            player_action::STOP_DESTROY_BLOCK => {
                if mode == GameMode::Survival {
                    self.stop_digging(entity_id, action.pos, action.sequence);
                } else {
                    self.finish_block_action(entity_id, action.sequence, &[]);
                }
            }
            player_action::ABORT_DESTROY_BLOCK => {
                self.abort_digging(entity_id, action.pos, action.sequence);
            }
            player_action::RELEASE_USE_ITEM => self.stop_using(entity_id),
            player_action::DROP_ITEM | player_action::DROP_ALL_ITEMS => {
                if mode == GameMode::Spectator || player.dead {
                    return;
                }
                let all = action.action == player_action::DROP_ALL_ITEMS;
                let slot = player.inventory.selected_slot();
                let mut taken = None;
                self.update_inventory(entity_id, |inventory| {
                    taken = inventory.take(slot, if all { i32::MAX } else { 1 });
                });
                if let Some(stack) = taken {
                    self.send_slots(entity_id, &[slot]);
                    self.drop_from(entity_id, stack);
                }
            }
            player_action::SWAP_ITEM_WITH_OFFHAND => {
                if mode == GameMode::Spectator || player.dead {
                    return;
                }
                let hand = player.inventory.selected_slot();
                self.stop_using(entity_id);
                self.update_inventory(entity_id, |inventory| {
                    let main = inventory.get(hand).cloned();
                    let off = inventory.get(inventory::OFF_HAND).cloned();
                    inventory.set(hand, off);
                    inventory.set(inventory::OFF_HAND, main);
                });
                self.send_slots(entity_id, &[hand, inventory::OFF_HAND]);
            }
            _ => {}
        }
    }

    /// Breaks a block, and the other half of a door or bed with it; a
    /// container's content falls out, the other half of a double chest
    /// becomes single. Returns the positions that changed.
    pub(super) fn break_block(&mut self, pos: BlockPos, breaker: Option<i32>) -> Vec<BlockPos> {
        let removed = self.removal(pos);
        if removed[0].1 == self.air {
            return vec![pos];
        }
        let mut contents = Vec::new();
        let mut chest = None;
        for &(at, state) in &removed {
            if let Some(entity) = self.context.world.block_entity(at) {
                contents.extend(entity.items.into_iter().flatten().map(|s| (at, s)));
            }
            if let Some((_, block)) = self.context.data.block_of(state)
                && let Some(towards) = placement::chest_partner(block, state)
            {
                chest = Some((towards.offset(at), block.first, block.state_count()));
            }
        }
        for &(at, _) in removed.iter().rev() {
            self.set_block(at, self.air);
        }
        if let Some((other, first, count)) = chest {
            let state = self.context.world.block(other);
            if (first..first + count).contains(&state)
                && let Some((_, block)) = self.context.data.block_of(state)
                && let Some(single) = block.with(state, "type", "single")
            {
                self.set_block(other, single);
            }
        }
        self.break_effect(breaker, pos, removed[0].1);
        for (at, stack) in contents {
            self.pop_resource(at, stack);
        }
        let mut changed: Vec<BlockPos> = removed.iter().map(|&(at, _)| at).collect();
        changed.extend(chest.map(|(other, _, _)| other));
        changed
    }

    /// `ServerPlayer.setGameMode`: the client, its tab list entry for
    /// everyone and what others see of it.
    pub(super) fn set_game_mode(&mut self, entity_id: i32, mode: GameMode) {
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        if player.game_mode() == mode {
            return;
        }
        player.info.game_mode = mode;
        if !matches!(mode, GameMode::Creative | GameMode::Spectator) {
            player.flying = false;
        }
        player.fall_distance = 0.0;
        player.digging = None;
        player.state = None;
        player.data_dirty = true;
        player.send(Outbound::Packet(Arc::new(Encoded::new(&GameEvent::new(
            GameEventKind::ChangeGameMode,
            mode as i32 as f32,
        )))));
        player.send(Outbound::Packet(Arc::new(Encoded::new(
            &player.abilities(),
        ))));
        player.send(Outbound::GameMode(mode));
        let uuid = player.uuid();
        self.broadcast_packet(
            None,
            &PlayerInfoGameMode {
                entries: vec![(uuid, mode as i32)],
            },
        );
        // Others re-check whether they see the player.
        self.broadcast_entity(entity_id, Vec::new());
    }

    /// Moves a player, e.g. by `/tp`.
    pub(super) fn teleport(&mut self, entity_id: i32, pos: [f64; 3]) {
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        player.pos = pos;
        player.fall_distance = 0.0;
        player.send(Outbound::Teleport {
            pos,
            rot: [player.yaw, player.pitch],
        });
    }

    /// Puts items into a player's inventory; what does not fit is dropped
    /// at their feet.
    pub(super) fn give(&mut self, entity_id: i32, mut stack: ItemStack) {
        let max = self
            .context
            .data
            .item(stack.item)
            .map_or(64, |i| i.stack.max(1));
        let mut changed = Vec::new();
        let mut left_over = Vec::new();
        while stack.count > 0 {
            let part = ItemStack {
                count: stack.count.min(max),
                ..stack.clone()
            };
            stack.count -= part.count;
            let mut added = Vec::new();
            let mut rest = None;
            self.update_inventory(entity_id, |inventory: &mut Inventory| {
                (rest, added) = inventory.add(part, max);
            });
            changed.extend(added);
            left_over.extend(rest);
        }
        changed.sort_unstable();
        changed.dedup();
        self.send_slots(entity_id, &changed);
        for stack in left_over {
            self.drop_from(entity_id, stack);
        }
    }

    /// `/kill`.
    pub(super) fn kill(&mut self, entity_id: i32) {
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let message = text::translate(
            "death.attack.genericKill",
            vec![text::player_name(player.name(), player.uuid())],
            None,
        );
        self.hurt(entity_id, f32::MAX, "minecraft:generic_kill", message);
    }
}
