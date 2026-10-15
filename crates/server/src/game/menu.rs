//! Container screens (`AbstractContainerMenu`): the player's inventory, chests,
//! barrels, dispensers, hoppers, the ender chest, the crafting table and
//! furnaces.
//! Clicks follow vanilla's `doClick`; afterwards the client gets the true
//! content of the screen, whatever it predicted.

use std::sync::Arc;

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    BlockEvent, ClientboundContainerClose, ContainerClick, ContainerSetContent, GameMode,
    ItemStack, OpenScreen, Slot, Slots, container_input, text,
};

use super::{Encoded, Game, Outbound, Player};
use crate::crafting::{self, Cooker};
use crate::data::GameData;
use crate::inventory;
use crate::world::World;

/// Where the stack shown in a menu slot lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// An inventory menu slot of the player's [`Inventory`].
    Player(usize),
    /// A slot of the block entity at that position.
    Container(BlockPos, usize),
    Ender(usize),
    /// The menu's crafting grid.
    Grid(usize),
    /// The crafting result.
    Result,
}

/// An open screen other than the inventory, or the inventory's crafting
/// grid while none is open.
#[derive(Debug, Clone)]
pub struct Menu {
    /// Window id; 0 is the inventory.
    pub id: i32,
    pub slots: Vec<Source>,
    /// Crafting grid, 2×2 for the inventory, 3×3 for a crafting table.
    pub grid: Vec<Option<ItemStack>>,
    /// Block entities the screen shows, to tell their other viewers.
    pub containers: Vec<BlockPos>,
    /// Where it was opened and the block id there, to close it when the
    /// player walks away or the block goes.
    pub at: Option<BlockPos>,
    pub block: Option<usize>,
    /// What [`quick_move`](Game::quick_move) does for this kind of screen.
    kind: Kind,
    /// A furnace screen: where the furnace is, and the fire and progress
    /// the client was last told.
    pub furnace: Option<BlockPos>,
    pub sent_data: Vec<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Inventory,
    /// `n` container slots, then the player's inventory.
    Container(usize),
    Crafting,
    /// Ingredient, fuel, result, then the player's inventory.
    Furnace(Cooker),
}

/// A drag across slots (`QUICK_CRAFT`) in progress.
#[derive(Debug, Clone, Default)]
pub struct Drag {
    status: u8,
    kind: u8,
    slots: Vec<usize>,
}

/// The player's inventory after the container's slots: main, then hotbar.
fn player_slots() -> impl Iterator<Item = Source> {
    (9..45).map(Source::Player)
}

impl Menu {
    /// `InventoryMenu`: result, 2×2 grid, armor, main inventory, hotbar,
    /// off hand.
    pub fn inventory() -> Self {
        let mut slots = vec![Source::Result];
        slots.extend((0..4).map(Source::Grid));
        slots.extend((5..46).map(Source::Player));
        Self {
            id: 0,
            slots,
            grid: vec![None; 4],
            containers: Vec::new(),
            at: None,
            block: None,
            kind: Kind::Inventory,
            furnace: None,
            sent_data: Vec::new(),
        }
    }

    pub fn container(
        id: i32,
        sources: Vec<Source>,
        containers: Vec<BlockPos>,
        at: BlockPos,
    ) -> Self {
        let size = sources.len();
        let mut slots = sources;
        slots.extend(player_slots());
        Self {
            id,
            slots,
            grid: Vec::new(),
            containers,
            at: Some(at),
            block: None,
            kind: Kind::Container(size),
            furnace: None,
            sent_data: Vec::new(),
        }
    }

    /// `AbstractFurnaceMenu`.
    pub fn furnace(id: i32, at: BlockPos, cooker: Cooker) -> Self {
        let mut slots: Vec<Source> = (0..3).map(|i| Source::Container(at, i)).collect();
        slots.extend(player_slots());
        Self {
            id,
            slots,
            grid: Vec::new(),
            containers: vec![at],
            at: Some(at),
            block: None,
            kind: Kind::Furnace(cooker),
            furnace: Some(at),
            sent_data: Vec::new(),
        }
    }

    pub fn crafting(id: i32, at: BlockPos) -> Self {
        let mut slots = vec![Source::Result];
        slots.extend((0..9).map(Source::Grid));
        slots.extend(player_slots());
        Self {
            id,
            slots,
            grid: vec![None; 9],
            containers: Vec::new(),
            at: Some(at),
            block: None,
            kind: Kind::Crafting,
            furnace: None,
            sent_data: Vec::new(),
        }
    }

    fn grid_size(&self) -> usize {
        match self.kind {
            Kind::Inventory => 2,
            Kind::Crafting => 3,
            Kind::Container(_) | Kind::Furnace(_) => 0,
        }
    }
}

/// Reads and writes the slots of a player's current menu.
struct View<'a> {
    player: &'a mut Player,
    world: &'a World,
    data: &'a GameData,
}

impl View<'_> {
    fn menu(&self) -> &Menu {
        self.player
            .menu
            .as_ref()
            .unwrap_or(&self.player.inventory_menu)
    }

    fn menu_mut(&mut self) -> &mut Menu {
        match &mut self.player.menu {
            Some(menu) => menu,
            None => &mut self.player.inventory_menu,
        }
    }

    fn source(&self, slot: usize) -> Option<Source> {
        self.menu().slots.get(slot).copied()
    }

    fn get(&self, slot: usize) -> Option<ItemStack> {
        match self.source(slot)? {
            Source::Player(i) => self.player.inventory.get(i).cloned(),
            Source::Container(pos, i) => self.world.block_entity(pos)?.items.get(i)?.clone(),
            Source::Ender(i) => self.player.ender.get(i)?.clone(),
            Source::Grid(i) => self.menu().grid.get(i)?.clone(),
            Source::Result => self.result(),
        }
    }

    fn set(&mut self, slot: usize, stack: Option<ItemStack>) {
        let stack = stack.filter(|s| s.count > 0);
        match self.source(slot) {
            Some(Source::Player(i)) => self.player.inventory.set(i, stack),
            Some(Source::Container(pos, i)) => {
                let data = self.data;
                self.world.update_block_entity(pos, |entity| {
                    entity.set_item(i, stack, data);
                });
            }
            Some(Source::Ender(i)) => {
                if let Some(target) = self.player.ender.get_mut(i) {
                    *target = stack;
                }
            }
            Some(Source::Grid(i)) => {
                if let Some(target) = self.menu_mut().grid.get_mut(i) {
                    *target = stack;
                }
            }
            Some(Source::Result) | None => {}
        }
    }

    /// `Slot.mayPlace`: armor slots take only what is worn there, shulker
    /// boxes no shulker boxes, results nothing.
    fn allows(&self, slot: usize, stack: &ItemStack) -> bool {
        let name = self.data.item(stack.item).map_or("", |i| i.name.as_str());
        match self.source(slot) {
            None | Some(Source::Result) => false,
            Some(Source::Player(5)) => {
                name.ends_with("_helmet")
                    || name.ends_with("_head")
                    || name.ends_with("_skull")
                    || name == "minecraft:carved_pumpkin"
            }
            Some(Source::Player(6)) => name.ends_with("_chestplate") || name == "minecraft:elytra",
            Some(Source::Player(7)) => name.ends_with("_leggings"),
            Some(Source::Player(8)) => name.ends_with("_boots"),
            Some(Source::Container(pos, i)) => {
                let Some(entity) = self.world.block_entity(pos) else {
                    return false;
                };
                if entity.furnace.is_some() {
                    // `FurnaceResultSlot` takes nothing, `FurnaceFuelSlot`
                    // fuel and one empty bucket.
                    return match i {
                        2 => false,
                        1 => {
                            self.data.item(stack.item).is_some_and(|i| i.fuel.is_some())
                                || (name == "minecraft:bucket"
                                    && entity.items[1]
                                        .as_ref()
                                        .is_none_or(|s| s.item != stack.item))
                        }
                        _ => true,
                    };
                }
                !(name.ends_with("shulker_box") && entity.kind == "minecraft:shulker_box")
            }
            Some(_) => true,
        }
    }

    fn max(&self, stack: &ItemStack) -> i32 {
        self.data.item(stack.item).map_or(64, |i| i.stack.max(1))
    }

    /// What the grid crafts right now.
    fn result(&self) -> Option<ItemStack> {
        let menu = self.menu();
        let size = menu.grid_size();
        if size == 0 {
            return None;
        }
        let grid: Vec<Option<i32>> = menu
            .grid
            .iter()
            .map(|s| s.as_ref().map(|s| s.item))
            .collect();
        let output = self.data.recipes.craft(size, &grid)?;
        Some(ItemStack {
            item: output.item,
            count: output.count,
            patch: Default::default(),
        })
    }

    /// Takes the result: one of each ingredient is used up, containers like
    /// buckets stay behind (`ResultSlot.onTake`).
    fn craft(&mut self) -> Option<ItemStack> {
        let result = self.result()?;
        let data = self.data;
        let mut leftovers = Vec::new();
        let grid = &mut self.menu_mut().grid;
        for cell in grid.iter_mut() {
            let Some(stack) = cell else { continue };
            let remainder = data
                .item(stack.item)
                .and_then(|item| crafting::remainder(&item.name))
                .and_then(|name| data.item_id(name));
            stack.count -= 1;
            if stack.count == 0 {
                *cell = None;
            }
            if let Some(remainder) = remainder {
                let remainder = ItemStack {
                    item: remainder,
                    count: 1,
                    patch: Default::default(),
                };
                if cell.is_none() {
                    *cell = Some(remainder);
                } else {
                    leftovers.push(remainder);
                }
            }
        }
        for leftover in leftovers {
            let max = self.max(&leftover);
            self.player.inventory.add(leftover, max);
        }
        Some(result)
    }

    /// Takes up to `count` from a slot (`Slot.safeTake`); the result only
    /// whole.
    fn take(&mut self, slot: usize, count: i32) -> Option<ItemStack> {
        if self.source(slot)? == Source::Result {
            return self.craft();
        }
        let mut stack = self.get(slot)?;
        let taken = count.min(stack.count);
        stack.count -= taken;
        self.set(slot, Some(stack.clone()));
        Some(ItemStack {
            count: taken,
            ..stack
        })
    }

    /// `Slot.safeInsert`: puts up to `count` of `carried` into a slot;
    /// returns what is left in hand.
    fn insert(&mut self, slot: usize, mut carried: ItemStack, count: i32) -> Option<ItemStack> {
        if !self.allows(slot, &carried) {
            return Some(carried);
        }
        let max = self.max(&carried);
        let existing = self.get(slot);
        let room = match &existing {
            None => max,
            Some(s) if same(s, &carried) => max - s.count,
            Some(_) => 0,
        };
        let moved = count.min(carried.count).min(room).max(0);
        if moved > 0 {
            let placed = ItemStack {
                count: existing.as_ref().map_or(0, |s| s.count) + moved,
                ..carried.clone()
            };
            self.set(slot, Some(placed));
            carried.count -= moved;
        }
        (carried.count > 0).then_some(carried)
    }

    /// `moveItemStackTo`: first onto matching stacks, then into empty slots,
    /// in the slot order given.
    fn move_to(&mut self, mut stack: ItemStack, targets: &[usize]) -> Option<ItemStack> {
        let max = self.max(&stack);
        for &target in targets {
            if let Some(existing) = self.get(target)
                && same(&existing, &stack)
                && existing.count < max
            {
                let moved = (max - existing.count).min(stack.count);
                self.set(
                    target,
                    Some(ItemStack {
                        count: existing.count + moved,
                        ..existing
                    }),
                );
                stack.count -= moved;
                if stack.count == 0 {
                    return None;
                }
            }
        }
        for &target in targets {
            if self.get(target).is_none() && self.allows(target, &stack) {
                self.set(target, Some(stack));
                return None;
            }
        }
        Some(stack)
    }
}

/// The blocks whose openers a screen counts as.
fn watched(menu: &Menu) -> Vec<BlockPos> {
    if menu.containers.is_empty() {
        menu.at.into_iter().collect()
    } else {
        menu.containers.clone()
    }
}

/// `ItemStack.isSameItemSameComponents`.
fn same(a: &ItemStack, b: &ItemStack) -> bool {
    a.item == b.item && a.patch == b.patch
}

impl Game {
    fn view<'a>(&'a mut self, entity_id: i32) -> Option<View<'a>> {
        let player = self.players.get_mut(&entity_id)?;
        Some(View {
            player,
            world: &self.context.world,
            data: &self.context.data,
        })
    }

    /// `ServerGamePacketListenerImpl.handleContainerClick`, then the true
    /// content of the screen for the clicker and every other viewer.
    pub(super) fn container_click(&mut self, entity_id: i32, click: &ContainerClick) {
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let menu_id = player.menu.as_ref().map_or(0, |m| m.id);
        if click.container_id != menu_id {
            return;
        }
        let mode = player.game_mode();
        let size = player
            .menu
            .as_ref()
            .unwrap_or(&player.inventory_menu)
            .slots
            .len();
        let valid =
            click.slot == -999 || click.slot == -1 || (0..size as i16).contains(&click.slot);
        if mode != GameMode::Spectator && valid {
            self.click(entity_id, click.slot, click.button, click.input, mode);
        }
        let containers = self
            .players
            .get(&entity_id)
            .and_then(|p| p.menu.as_ref())
            .map(|m| m.containers.clone())
            .unwrap_or_default();
        self.send_menu(entity_id);
        self.update_viewers(entity_id, &containers);
        // What is in hand may have changed.
        self.update_inventory(entity_id, |_| {});
    }

    fn click(&mut self, entity_id: i32, slot: i16, button: i8, input: i32, mode: GameMode) {
        let mut drops = Vec::new();
        // `FurnaceResultSlot.onTake` pays out the experience.
        let furnace = self
            .players
            .get(&entity_id)
            .and_then(|p| p.menu.as_ref())
            .and_then(|m| m.furnace);
        let result_before = furnace.and_then(|pos| {
            self.context
                .world
                .block_entity(pos)
                .map(|e| e.items[2].as_ref().map_or(0, |s| s.count))
        });
        self.click_slots(entity_id, slot, button, input, mode, &mut drops);
        if let (Some(pos), Some(before)) = (furnace, result_before) {
            let after = self
                .context
                .world
                .block_entity(pos)
                .map_or(0, |e| e.items[2].as_ref().map_or(0, |s| s.count));
            if after < before
                && let Some(at) = self.players.get(&entity_id).map(|p| p.pos)
            {
                self.furnace_experience(pos, at);
            }
        }
        for stack in drops {
            self.drop_from(entity_id, stack);
        }
    }

    fn click_slots(
        &mut self,
        entity_id: i32,
        slot: i16,
        button: i8,
        input: i32,
        mode: GameMode,
        drops: &mut Vec<ItemStack>,
    ) {
        {
            let Some(mut s) = self.view(entity_id) else {
                return;
            };
            if input == container_input::QUICK_CRAFT {
                drag(&mut s, slot, button, mode);
                return;
            }
            s.player.drag = Drag::default();
            let index = usize::try_from(slot).ok();
            match (input, button) {
                (container_input::PICKUP, 0 | 1) if slot == -999 => {
                    if let Some(mut carried) = s.player.carried.take() {
                        if button == 0 {
                            drops.push(carried);
                        } else {
                            drops.push(ItemStack {
                                count: 1,
                                ..carried.clone()
                            });
                            carried.count -= 1;
                            s.player.carried = (carried.count > 0).then_some(carried);
                        }
                    }
                }
                (container_input::PICKUP, 0 | 1) => {
                    let Some(index) = index else { return };
                    pickup(&mut s, index, button == 0);
                }
                (container_input::QUICK_MOVE, 0 | 1) => {
                    let Some(index) = index else { return };
                    // Shift-clicking a result crafts as often as possible.
                    while let Some(moved) = quick_move(&mut s, index) {
                        if moved.count == 0 || s.source(index) != Some(Source::Result) {
                            break;
                        }
                    }
                }
                (container_input::SWAP, 0..=8 | 40) => {
                    let Some(index) = index else { return };
                    let hotbar = if button == 40 {
                        inventory::OFF_HAND
                    } else {
                        36 + button as usize
                    };
                    swap(&mut s, index, hotbar);
                }
                (container_input::CLONE, _)
                    if mode == GameMode::Creative && s.player.carried.is_none() =>
                {
                    if let Some(stack) = index.and_then(|i| s.get(i)) {
                        let max = s.max(&stack);
                        s.player.carried = Some(ItemStack {
                            count: max,
                            ..stack
                        });
                    }
                }
                (container_input::THROW, 0 | 1) if s.player.carried.is_none() => {
                    let Some(index) = index else { return };
                    let all = button == 1;
                    while let Some(stack) = s.take(index, if all { i32::MAX } else { 1 }) {
                        if stack.count == 0 {
                            break;
                        }
                        let again = all && s.source(index) == Some(Source::Result);
                        drops.push(stack);
                        if !again {
                            break;
                        }
                    }
                }
                (container_input::PICKUP_ALL, _) => {
                    if let Some(index) = index {
                        pickup_all(&mut s, index, button == 0);
                    }
                }
                _ => {}
            }
        }
    }

    /// The whole screen and the stack in hand, as the server has them.
    pub(super) fn send_menu(&mut self, entity_id: i32) {
        let Some(s) = self.view(entity_id) else {
            return;
        };
        let items = (0..s.menu().slots.len()).map(|i| Slot(s.get(i))).collect();
        let carried = Slot(s.player.carried.clone());
        let id = s.menu().id;
        let player = s.player;
        player.state_id = player.state_id.wrapping_add(1);
        let packet = Encoded::new(&ContainerSetContent {
            container_id: id,
            state_id: player.state_id,
            items: Slots(items),
            carried,
        });
        player.send(Outbound::Packet(Arc::new(packet)));
    }

    /// Others looking into the same containers see the change.
    fn update_viewers(&mut self, except: i32, containers: &[BlockPos]) {
        if containers.is_empty() {
            return;
        }
        let viewers: Vec<i32> = self
            .players
            .values()
            .filter(|p| {
                p.entity_id != except
                    && p.menu
                        .as_ref()
                        .is_some_and(|m| m.containers.iter().any(|c| containers.contains(c)))
            })
            .map(|p| p.entity_id)
            .collect();
        for viewer in viewers {
            self.send_menu(viewer);
        }
    }

    /// Opens a screen: the window, its content, and for chests the lid.
    pub(super) fn open_menu(
        &mut self,
        entity_id: i32,
        mut menu: Menu,
        menu_type: &str,
        title: &str,
    ) {
        self.close_menu(entity_id, false);
        let Ok(menu_type) = self.context.data.menu(menu_type) else {
            return;
        };
        let watched = watched(&menu);
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        player.window = player.window % 100 + 1;
        menu.id = player.window;
        let id = menu.id;
        player.menu = Some(menu);
        player.send(Outbound::Packet(Arc::new(Encoded::new(&OpenScreen {
            container_id: id,
            menu: menu_type,
            title: text::translate(title, Vec::new(), None),
        }))));
        self.send_menu(entity_id);
        for pos in watched {
            self.update_openers(pos);
        }
    }

    /// `AbstractContainerMenu.removed`: the grid goes back to the inventory,
    /// what is in hand too, whatever does not fit is dropped.
    pub(super) fn close_menu(&mut self, entity_id: i32, inventory_too: bool) {
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        let mut give_back = Vec::new();
        let mut containers = Vec::new();
        if let Some(menu) = player.menu.take() {
            containers = watched(&menu);
            give_back.extend(menu.grid.into_iter().flatten());
        } else if inventory_too {
            give_back.extend(
                player
                    .inventory_menu
                    .grid
                    .iter_mut()
                    .filter_map(Option::take),
            );
        }
        give_back.extend(player.carried.take());
        let mut drops = Vec::new();
        for stack in give_back {
            let max = self
                .context
                .data
                .item(stack.item)
                .map_or(64, |i| i.stack.max(1));
            if let Some(player) = self.players.get_mut(&entity_id)
                && let (Some(rest), _) = player.inventory.add(stack, max)
            {
                drops.push(rest);
            }
        }
        for stack in drops {
            self.drop_from(entity_id, stack);
        }
        for pos in containers {
            self.update_openers(pos);
        }
        self.send_menu(entity_id);
        self.update_inventory(entity_id, |_| {});
    }

    /// `ContainerOpenersCounter`: how many look into a chest, ender chest,
    /// barrel or shulker box decides whether it is open. Lids move with a
    /// block event, barrels change state, and opening and closing are heard.
    fn update_openers(&mut self, pos: BlockPos) {
        let data = &self.context.data;
        let state = self.context.world.block(pos);
        let Some((block_id, block)) = data.block_of(state) else {
            return;
        };
        let name = block.name.as_str();
        let (sound, lid) = match name {
            "minecraft:chest" | "minecraft:trapped_chest" => ("chest", true),
            "minecraft:ender_chest" => ("ender_chest", true),
            "minecraft:barrel" => ("barrel", false),
            _ if name.ends_with("shulker_box") => ("shulker_box", true),
            _ => return,
        };
        let viewers = self
            .players
            .values()
            .filter(|p| {
                p.menu.as_ref().is_some_and(|m| {
                    m.containers.contains(&pos) || (m.containers.is_empty() && m.at == Some(pos))
                })
            })
            .count();
        let before = self.openers.get(&pos).copied().unwrap_or(0);
        if viewers == before {
            return;
        }
        if viewers == 0 {
            self.openers.remove(&pos);
        } else {
            self.openers.insert(pos, viewers);
        }
        if lid {
            let packet = Encoded::new(&BlockEvent {
                pos,
                event: 1,
                param: viewers.min(255) as u8,
                block: block_id as i32,
            });
            self.broadcast(
                None,
                &Outbound::InChunk {
                    chunk: crate::world::chunk_of(pos),
                    packet: Arc::new(packet),
                },
            );
        }
        let opened = before == 0;
        if !opened && viewers != 0 {
            return;
        }
        if name == "minecraft:barrel"
            && let Some(changed) = block.with(state, "open", if opened { "true" } else { "false" })
        {
            self.set_block(pos, changed);
        }
        // A double chest is heard once, between its halves, from its right
        // half (`ChestBlockEntity.playSound`).
        let mut at = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        ];
        if lid && sound == "chest" {
            match block.value(state, "type") {
                Some("left") => return,
                Some("right") => {
                    if let Some(towards) = crate::placement::chest_partner(block, state) {
                        let other = towards.offset(pos);
                        at[0] += f64::from(other.x - pos.x) * 0.5;
                        at[2] += f64::from(other.z - pos.z) * 0.5;
                    }
                }
                _ => {}
            }
        }
        let event = format!(
            "minecraft:block.{sound}.{}",
            if opened { "open" } else { "close" }
        );
        let pitch = rand::random::<f32>() * 0.1 + 0.9;
        self.play_sound(None, &event, at, 0.5, pitch);
    }

    /// The screen goes away by the server's will: the block is gone or the
    /// player too far from it (`AbstractContainerMenu.stillValid`).
    pub(super) fn close_distant_menu(&mut self, entity_id: i32) {
        let data = &self.context.data;
        let world = &self.context.world;
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let Some(menu) = &player.menu else {
            return;
        };
        let (Some(at), Some(block)) = (menu.at, menu.block) else {
            return;
        };
        let still_there = data
            .block_of(world.block(at))
            .is_some_and(|(id, _)| id == block)
            && menu
                .containers
                .iter()
                .all(|&pos| world.block_entity(pos).is_some());
        let eyes = player.eyes();
        let distance_sq: f64 = [at.x, at.y, at.z]
            .iter()
            .zip(eyes)
            .map(|(&c, e)| {
                let d = f64::from(c) + 0.5 - e;
                d * d
            })
            .sum();
        // Interaction range plus vanilla's 4 blocks of slack.
        if still_there && distance_sq <= 10.5 * 10.5 {
            return;
        }
        let id = menu.id;
        player.send(Outbound::Packet(Arc::new(Encoded::new(
            &ClientboundContainerClose { container_id: id },
        ))));
        self.close_menu(entity_id, false);
    }
}

/// `PICKUP` on a slot.
fn pickup(s: &mut View, index: usize, primary: bool) {
    let clicked = s.get(index);
    let carried = s.player.carried.take();
    s.player.carried = match (clicked, carried) {
        (None, None) => None,
        (None, Some(carried)) => {
            let amount = if primary { carried.count } else { 1 };
            s.insert(index, carried, amount)
        }
        (Some(clicked), None) => {
            let amount = if primary {
                clicked.count
            } else {
                (clicked.count + 1) / 2
            };
            s.take(index, amount)
        }
        (Some(clicked), Some(mut carried)) => {
            if s.source(index) == Some(Source::Result) {
                // Only a whole result that fits onto what is in hand.
                let max = s.max(&carried);
                if same(&clicked, &carried)
                    && carried.count + clicked.count <= max
                    && let Some(made) = s.craft()
                {
                    carried.count += made.count;
                }
                Some(carried)
            } else if same(&clicked, &carried) {
                let amount = if primary { carried.count } else { 1 };
                s.insert(index, carried, amount)
            } else if carried.count <= s.max(&carried) && s.allows(index, &carried) {
                s.set(index, Some(carried));
                Some(clicked)
            } else {
                Some(carried)
            }
        }
    };
}

/// `QUICK_CRAFT`: start, add slots, then spread what is in hand over them.
fn drag(s: &mut View, slot: i16, button: i8, mode: GameMode) {
    let status = (button & 3) as u8;
    let kind = ((button >> 2) & 3) as u8;
    let expected = s.player.drag.status;
    if (expected != 1 || status != 2) && expected != status {
        s.player.drag = Drag::default();
        return;
    }
    let Some(carried) = s.player.carried.clone() else {
        s.player.drag = Drag::default();
        return;
    };
    match status {
        0 => {
            let allowed = kind < 2 || (kind == 2 && mode == GameMode::Creative);
            s.player.drag = if allowed {
                Drag {
                    status: 1,
                    kind,
                    slots: Vec::new(),
                }
            } else {
                Drag::default()
            };
        }
        1 => {
            let Ok(index) = usize::try_from(slot) else {
                return;
            };
            let fits = match s.get(index) {
                None => s.allows(index, &carried),
                Some(existing) => same(&existing, &carried) && existing.count < s.max(&carried),
            };
            let drag = &mut s.player.drag;
            if fits
                && (drag.kind == 2 || carried.count as usize > drag.slots.len())
                && !drag.slots.contains(&index)
            {
                drag.slots.push(index);
            }
        }
        2 => {
            let drag = std::mem::take(&mut s.player.drag);
            if drag.slots.len() == 1 {
                pickup(s, drag.slots[0], drag.kind == 0);
                return;
            }
            let max = s.max(&carried);
            let mut remaining = carried.count;
            let share = match drag.kind {
                0 => carried.count / drag.slots.len().max(1) as i32,
                1 => 1,
                _ => max,
            };
            for index in drag.slots {
                let had = s.get(index).map_or(0, |e| e.count);
                let count = (had + share).min(max);
                remaining -= count - had;
                s.set(
                    index,
                    Some(ItemStack {
                        count,
                        ..carried.clone()
                    }),
                );
            }
            if drag.kind == 2 {
                remaining = carried.count;
            }
            s.player.carried = (remaining > 0).then_some(ItemStack {
                count: remaining,
                ..carried
            });
        }
        _ => s.player.drag = Drag::default(),
    }
}

/// `SWAP`: the number keys and F swap a slot with a hotbar slot or the off
/// hand.
fn swap(s: &mut View, index: usize, hotbar: usize) {
    if s.source(index) == Some(Source::Player(hotbar)) {
        return;
    }
    let source = s.player.inventory.get(hotbar).cloned();
    if s.source(index) == Some(Source::Result) {
        // Only into an empty hotbar slot, which crafts.
        if source.is_none()
            && let Some(made) = s.craft()
        {
            s.player.inventory.set(hotbar, Some(made));
        }
        return;
    }
    let target = s.get(index);
    match source {
        None => {
            if target.is_some() {
                s.player.inventory.set(hotbar, target);
                s.set(index, None);
            }
        }
        Some(source) => {
            if s.allows(index, &source) {
                s.player.inventory.set(hotbar, target);
                s.set(index, Some(source));
            }
        }
    }
}

/// `PICKUP_ALL`: a double click gathers matching stacks into the hand,
/// partly filled ones first.
fn pickup_all(s: &mut View, index: usize, forward: bool) {
    let Some(mut carried) = s.player.carried.clone() else {
        return;
    };
    if s.get(index).is_some() && s.source(index) != Some(Source::Result) {
        return;
    }
    let max = s.max(&carried);
    let count = s.menu().slots.len();
    let order: Vec<usize> = if forward {
        (0..count).collect()
    } else {
        (0..count).rev().collect()
    };
    for pass in 0..2 {
        for &i in &order {
            if carried.count >= max || s.source(i) == Some(Source::Result) {
                continue;
            }
            if let Some(stack) = s.get(i)
                && same(&stack, &carried)
                && (pass == 1 || stack.count != max)
                && let Some(taken) = s.take(i, max - carried.count)
            {
                carried.count += taken.count;
            }
        }
    }
    s.player.carried = Some(carried);
}

/// `quickMoveStack` of each screen: shift-click moves a stack between the
/// container (or grid) and the inventory.
fn quick_move(s: &mut View, index: usize) -> Option<ItemStack> {
    let stack = s.get(index)?;
    let kind = s.menu().kind;
    let count = s.menu().slots.len();
    let range = |a: usize, b: usize, reverse: bool| -> Vec<usize> {
        if reverse {
            (a..b).rev().collect()
        } else {
            (a..b).collect()
        }
    };
    let targets = match kind {
        Kind::Inventory => match index {
            0 => range(9, 45, true),
            1..=8 | 45 => range(9, 45, false),
            9..=35 => range(36, 45, false),
            _ => range(9, 36, false),
        },
        Kind::Crafting => match index {
            0 => range(10, 46, true),
            1..=9 => range(10, 46, false),
            10..=36 => range(37, 46, false),
            _ => range(10, 37, false),
        },
        Kind::Container(size) => {
            if index < size {
                range(size, count, true)
            } else {
                range(0, size, false)
            }
        }
        // `AbstractFurnaceMenu.quickMoveStack`: what cooks goes in on top,
        // fuel below, the rest between inventory and hotbar.
        Kind::Furnace(cooker) => match index {
            2 => range(3, 39, true),
            0 | 1 => range(3, 39, false),
            _ if s.data.recipes.cook(cooker, stack.item).is_some() => range(0, 1, false),
            _ if s.data.item(stack.item).is_some_and(|i| i.fuel.is_some()) => range(1, 2, false),
            3..=29 => range(30, 39, false),
            _ => range(3, 30, false),
        },
    };
    if s.source(index) == Some(Source::Result) {
        // Only when the whole result fits.
        let fits = targets.iter().any(|&t| {
            s.get(t)
                .is_none_or(|e| same(&e, &stack) && e.count + stack.count <= s.max(&stack))
        });
        if !fits {
            return None;
        }
        let made = s.craft()?;
        let left = s.move_to(made.clone(), &targets);
        if let Some(left) = left {
            let max = s.max(&left);
            s.player.inventory.add(left, max);
        }
        return Some(made);
    }
    let before = stack.count;
    let left = s.move_to(stack.clone(), &targets);
    let moved = before - left.as_ref().map_or(0, |l| l.count);
    s.set(index, left);
    (moved > 0).then_some(ItemStack {
        count: moved,
        ..stack
    })
}
