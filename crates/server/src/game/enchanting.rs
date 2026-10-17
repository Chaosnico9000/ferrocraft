//! The enchanting table, the anvil and the grindstone (`EnchantmentMenu`,
//! `AnvilMenu`, `GrindstoneMenu`): what they offer for the items put in,
//! and what taking it costs. The enchanting table draws with Java's random
//! numbers from the player's enchantment seed, so it offers what vanilla
//! would.

use std::sync::Arc;

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{ContainerSetData, GameMode, ItemStack};

use super::enchant::{compatible, cost, in_tag, max_level, primary_for, supports};
use super::menu::{Extra, Kind};
use super::{Encoded, Game, Outbound};
use crate::data::GameData;
use crate::java_random::JavaRandom;
use crate::stack;

/// `LevelEvent.SOUND_ANVIL_USED`, `SOUND_ANVIL_BROKEN`,
/// `SOUND_GRINDSTONE_USED`.
const ANVIL_USED: i32 = 1030;
const ANVIL_BROKEN: i32 = 1029;
const GRINDSTONE_USED: i32 = 1042;

/// The component that holds a stack's enchantments: stored ones for a
/// book.
fn enchantment_component(data: &GameData, item: &ItemStack) -> &'static str {
    if data.item(item.item).map(|i| i.name.as_str()) == Some("minecraft:enchanted_book") {
        "minecraft:stored_enchantments"
    } else {
        "minecraft:enchantments"
    }
}

/// `EnchantmentHelper.getEnchantmentsForCrafting`.
pub fn crafting_enchantments(data: &GameData, item: &ItemStack) -> Vec<(String, i32)> {
    stack::enchantments_in(data, item, enchantment_component(data, item))
}

fn set_crafting_enchantments(data: &GameData, item: &mut ItemStack, list: &[(String, i32)]) {
    let component = enchantment_component(data, item);
    stack::set_enchantments_in(data, item, component, list);
}

/// `ItemStack.isEnchantable`: it takes enchantments and has none yet.
fn enchantable(data: &GameData, item: &ItemStack) -> Option<i32> {
    let value = match stack::get(data, item, "minecraft:enchantable")?.get("value") {
        Some(Nbt::Int(v)) => *v,
        _ => return None,
    };
    stack::enchantments(data, item).is_empty().then_some(value)
}

fn is(data: &GameData, item: &ItemStack, name: &str) -> bool {
    data.item(item.item).is_some_and(|i| i.name == name)
}

/// `EnchantmentHelper.getEnchantmentCost`.
fn enchantment_cost(random: &mut JavaRandom, slot: i32, bookcases: i32, enchantable: bool) -> i32 {
    if !enchantable {
        return 0;
    }
    let bookcases = bookcases.min(15);
    let selected =
        random.next_int_bounded(8) + 1 + (bookcases >> 1) + random.next_int_bounded(bookcases + 1);
    match slot {
        0 => (selected / 3).max(1),
        1 => selected * 2 / 3 + 1,
        _ => selected.max(bookcases * 2),
    }
}

/// `WeightedRandom.getRandomItem` over `(name, level, weight)`.
fn weighted<'a>(
    random: &mut JavaRandom,
    list: &'a [(String, i32, i32)],
) -> Option<&'a (String, i32, i32)> {
    let total: i32 = list.iter().map(|e| e.2).sum();
    if total <= 0 {
        return None;
    }
    let mut index = random.next_int_bounded(total);
    list.iter().find(|e| {
        index -= e.2;
        index < 0
    })
}

/// `EnchantmentHelper.selectEnchantment` over `#in_enchanting_table`, in
/// the tag's order.
pub fn select_enchantments(
    data: &GameData,
    random: &mut JavaRandom,
    item: &ItemStack,
    cost_value: i32,
) -> Vec<(String, i32)> {
    let Some(value) =
        stack::get(data, item, "minecraft:enchantable").and_then(|e| match e.get("value") {
            Some(Nbt::Int(v)) => Some(*v),
            _ => None,
        })
    else {
        return Vec::new();
    };
    let mut cost_value = cost_value
        + 1
        + random.next_int_bounded(value / 4 + 1)
        + random.next_int_bounded(value / 4 + 1);
    let span = (random.next_float() + random.next_float() - 1.0) * 0.15;
    cost_value = ((cost_value as f32 + cost_value as f32 * span + 0.5).floor() as i32).max(1);
    let book = is(data, item, "minecraft:book");
    // `getAvailableEnchantmentResults`.
    let source = data
        .tags
        .registries
        .iter()
        .find(|r| r.registry == "minecraft:enchantment")
        .and_then(|r| {
            r.tags
                .iter()
                .find(|(t, _)| t == "minecraft:in_enchanting_table")
        })
        .map(|(_, ids)| ids.clone())
        .unwrap_or_default();
    let mut available: Vec<(String, i32, i32)> = Vec::new();
    for id in source {
        let Some(json) = data.enchantments.get(id as usize) else {
            continue;
        };
        let name = json["name"].as_str().unwrap_or("").to_owned();
        if !(book || primary_for(data, json, item.item)) {
            continue;
        }
        for level in (1..=max_level(json)).rev() {
            if cost_value >= cost(json, "min_cost", level)
                && cost_value <= cost(json, "max_cost", level)
            {
                let weight = json["weight"].as_i64().unwrap_or(1) as i32;
                available.push((name, level, weight));
                break;
            }
        }
    }
    let mut results: Vec<(String, i32)> = Vec::new();
    if available.is_empty() {
        return results;
    }
    if let Some((name, level, _)) = weighted(random, &available) {
        results.push((name.clone(), *level));
    }
    while random.next_int_bounded(50) <= cost_value {
        if let Some((last, _)) = results.last() {
            let last = last.clone();
            available.retain(|(name, _, _)| compatible(data, &last, name));
        }
        if available.is_empty() {
            break;
        }
        if let Some((name, level, _)) = weighted(random, &available) {
            results.push((name.clone(), *level));
        }
        cost_value /= 2;
    }
    results
}

/// What the enchanting table shows (`EnchantmentMenu`'s data slots).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offers {
    pub costs: [i32; 3],
    pub clues: [i32; 3],
    pub levels: [i32; 3],
}

impl Default for Offers {
    fn default() -> Self {
        Self {
            costs: [0; 3],
            clues: [-1; 3],
            levels: [-1; 3],
        }
    }
}

/// `EnchantmentMenu.getEnchantmentList`.
fn enchantment_list(
    data: &GameData,
    seed: i32,
    item: &ItemStack,
    slot: i32,
    cost_value: i32,
) -> (Vec<(String, i32)>, JavaRandom) {
    let mut random = JavaRandom::new(i64::from(seed) + i64::from(slot));
    let mut list = select_enchantments(data, &mut random, item, cost_value);
    if is(data, item, "minecraft:book") && list.len() > 1 {
        let index = random.next_int_bounded(list.len() as i32) as usize;
        list.remove(index);
    }
    (list, random)
}

/// `EnchantmentMenu.slotsChanged` for an item and a number of bookshelves.
pub fn offers(data: &GameData, seed: i32, item: Option<&ItemStack>, bookcases: i32) -> Offers {
    let mut offers = Offers::default();
    let Some(item) = item.filter(|i| enchantable(data, i).is_some()) else {
        return offers;
    };
    let mut random = JavaRandom::new(i64::from(seed));
    for slot in 0..3 {
        offers.costs[slot] = enchantment_cost(&mut random, slot as i32, bookcases, true);
        if offers.costs[slot] < slot as i32 + 1 {
            offers.costs[slot] = 0;
        }
    }
    for slot in 0..3 {
        if offers.costs[slot] > 0 {
            // The clue comes from the same random, after the list.
            let (list, mut random) =
                enchantment_list(data, seed, item, slot as i32, offers.costs[slot]);
            if !list.is_empty() {
                let (name, level) = &list[random.next_int_bounded(list.len() as i32) as usize];
                offers.clues[slot] = data.enchantment(name).map_or(-1, |(id, _)| id as i32);
                offers.levels[slot] = *level;
            }
        }
    }
    offers
}

/// The anvil's take on two items (`AnvilMenu.createResult`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnvilResult {
    pub result: Option<ItemStack>,
    pub cost: i32,
    /// Items of the right slot a repair uses.
    pub repair_count: i32,
    pub only_renaming: bool,
}

/// `Component.getString` of a stack's name: its custom name, or the
/// English name of the item.
pub fn hover_name(data: &GameData, item: &ItemStack) -> String {
    if let Some(name) = stack::get(data, item, "minecraft:custom_name") {
        return plain_text(&name);
    }
    let id = data.item(item.item).map_or("", |i| i.name.as_str());
    let path = id.trim_start_matches("minecraft:");
    data.language
        .get(&format!("item.minecraft.{path}"))
        .or_else(|| data.language.get(&format!("block.minecraft.{path}")))
        .cloned()
        .unwrap_or_else(|| path.to_owned())
}

/// The plain text of a text component.
fn plain_text(component: &Nbt) -> String {
    match component {
        Nbt::String(s) => s.clone(),
        Nbt::Compound(_) => {
            let mut out = match component.get("text") {
                Some(Nbt::String(s)) => s.clone(),
                _ => String::new(),
            };
            if let Some(Nbt::List(extra)) = component.get("extra") {
                for part in extra {
                    out.push_str(&plain_text(part));
                }
            }
            out
        }
        Nbt::List(parts) => parts.iter().map(plain_text).collect(),
        _ => String::new(),
    }
}

/// `AnvilMenu.calculateIncreasedRepairCost`.
fn increased_repair_cost(cost: i32) -> i32 {
    (i64::from(cost) * 2 + 1).min(i64::from(i32::MAX)) as i32
}

/// `AnvilMenu.createResult`.
pub fn anvil(
    data: &GameData,
    input: Option<&ItemStack>,
    addition: Option<&ItemStack>,
    name: Option<&str>,
    creative: bool,
) -> AnvilResult {
    let fail = AnvilResult::default();
    let Some(input) = input else {
        return fail;
    };
    let component = enchantment_component(data, input);
    if stack::get(data, input, component).is_none() {
        return fail;
    }
    let mut result = input.clone();
    let mut enchantments = crafting_enchantments(data, &result);
    let mut price: i32 = 0;
    let repair_cost = |s: &ItemStack| stack::int(data, s, "minecraft:repair_cost");
    let tax = i64::from(repair_cost(input)) + addition.map_or(0, |a| i64::from(repair_cost(a)));
    let mut repair_count = 0;
    let mut naming_cost = 0;
    if let Some(addition) = addition {
        let using_book = stack::get(data, addition, "minecraft:stored_enchantments").is_some();
        let repairable = stack::get(data, input, "minecraft:repairable")
            .and_then(|r| r.get("items").cloned())
            .is_some_and(|items| {
                let json = nbt_to_json(&items);
                super::enchant::item_in(data, &json, addition.item)
            });
        if stack::damageable(data, &result) && repairable {
            let mut repair = stack::damage(data, &result).min(stack::max_damage(data, &result) / 4);
            if repair <= 0 {
                return fail;
            }
            let mut count = 0;
            while repair > 0 && count < addition.count {
                let damage = stack::damage(data, &result) - repair;
                stack::set(data, &mut result, "minecraft:damage", &Nbt::Int(damage));
                price += 1;
                repair = stack::damage(data, &result).min(stack::max_damage(data, &result) / 4);
                count += 1;
            }
            repair_count = count;
        } else {
            if !using_book && (result.item != addition.item || !stack::damageable(data, &result)) {
                return fail;
            }
            if stack::damageable(data, &result) && !using_book {
                let remaining1 = stack::max_damage(data, input) - stack::damage(data, input);
                let remaining2 = stack::max_damage(data, addition) - stack::damage(data, addition);
                let additional = remaining2 + stack::max_damage(data, &result) * 12 / 100;
                let remaining = remaining1 + additional;
                let damage = (stack::max_damage(data, &result) - remaining).max(0);
                if damage < stack::damage(data, &result) {
                    stack::set(data, &mut result, "minecraft:damage", &Nbt::Int(damage));
                    price += 2;
                }
            }
            let mut any_compatible = false;
            let mut any_incompatible = false;
            for (name, level) in crafting_enchantments(data, addition) {
                let Some((_, json)) = data.enchantment(&name) else {
                    continue;
                };
                let current = enchantments
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map_or(0, |(_, l)| *l);
                let mut level = if current == level {
                    level + 1
                } else {
                    level.max(current)
                };
                let mut fits = supports(data, json, input.item);
                if creative || is(data, input, "minecraft:enchanted_book") {
                    fits = true;
                }
                for (other, _) in &enchantments {
                    if *other != name && !compatible(data, &name, other) {
                        fits = false;
                        price += 1;
                    }
                }
                if !fits {
                    any_incompatible = true;
                    continue;
                }
                any_compatible = true;
                level = level.min(max_level(json));
                match enchantments.iter_mut().find(|(n, _)| *n == name) {
                    Some((_, l)) => *l = level,
                    None => enchantments.push((name.clone(), level)),
                }
                let mut fee = json["anvil_cost"].as_i64().unwrap_or(1) as i32;
                if using_book {
                    fee = (fee / 2).max(1);
                }
                price += fee * level;
                if input.count > 1 {
                    price = 40;
                }
            }
            if any_incompatible && !any_compatible {
                return fail;
            }
        }
    }
    match name {
        Some(name) if !name.trim().is_empty() => {
            if name != hover_name(data, input) {
                naming_cost = 1;
                price += naming_cost;
                stack::set(
                    data,
                    &mut result,
                    "minecraft:custom_name",
                    &Nbt::String(name.to_owned()),
                );
            }
        }
        _ => {
            if stack::get(data, input, "minecraft:custom_name").is_some()
                && stack::default(data, input.item, "minecraft:custom_name").is_none()
            {
                naming_cost = 1;
                price += naming_cost;
                stack::reset(data, &mut result, "minecraft:custom_name");
            }
        }
    }
    let mut cost_value = if price <= 0 {
        0
    } else {
        (tax + i64::from(price)).clamp(0, i64::from(i32::MAX)) as i32
    };
    let mut result = (price > 0).then_some(result);
    let mut only_renaming = false;
    if naming_cost == price && naming_cost > 0 {
        if cost_value >= 40 {
            cost_value = 39;
        }
        only_renaming = true;
    }
    if cost_value >= 40 && !creative {
        result = None;
    }
    if let Some(result) = &mut result {
        let mut base = repair_cost(result);
        if let Some(addition) = addition {
            base = base.max(repair_cost(addition));
        }
        if naming_cost != price || naming_cost == 0 {
            base = increased_repair_cost(base);
        }
        stack::set(data, result, "minecraft:repair_cost", &Nbt::Int(base));
        set_crafting_enchantments(data, result, &enchantments);
    }
    AnvilResult {
        result,
        cost: cost_value,
        repair_count,
        only_renaming,
    }
}

/// NBT back into the JSON shape of a data pack, for holder sets.
fn nbt_to_json(nbt: &Nbt) -> serde_json::Value {
    match nbt {
        Nbt::String(s) => serde_json::Value::String(s.clone()),
        Nbt::List(items) => serde_json::Value::Array(items.iter().map(nbt_to_json).collect()),
        _ => serde_json::Value::Null,
    }
}

/// `GrindstoneMenu.computeResult`.
pub fn grindstone(
    data: &GameData,
    input: Option<&ItemStack>,
    additional: Option<&ItemStack>,
) -> Option<ItemStack> {
    match (input, additional) {
        (None, None) => None,
        (Some(i), Some(a)) if i.count <= 1 && a.count <= 1 => grind_merge(data, i, a),
        (Some(item), None) | (None, Some(item)) if item.count <= 1 => {
            let any = !stack::enchantments(data, item).is_empty()
                || !stack::enchantments_in(data, item, "minecraft:stored_enchantments").is_empty();
            any.then(|| remove_non_curses(data, item.clone()))
        }
        _ => None,
    }
}

fn grind_merge(data: &GameData, input: &ItemStack, additional: &ItemStack) -> Option<ItemStack> {
    if input.item != additional.item {
        return None;
    }
    let durability = stack::max_damage(data, input).max(stack::max_damage(data, additional));
    let remaining1 = stack::max_damage(data, input) - stack::damage(data, input);
    let remaining2 = stack::max_damage(data, additional) - stack::damage(data, additional);
    let remaining = remaining1 + remaining2 + durability * 5 / 100;
    let mut count = 1;
    if !stack::damageable(data, input) {
        if stack::max_stack_size(data, input) < 2 || input != additional {
            return None;
        }
        count = 2;
    }
    let mut item = ItemStack {
        count,
        ..input.clone()
    };
    if stack::damageable(data, &item) {
        stack::set(
            data,
            &mut item,
            "minecraft:max_damage",
            &Nbt::Int(durability),
        );
        stack::set(
            data,
            &mut item,
            "minecraft:damage",
            &Nbt::Int((durability - remaining).max(0)),
        );
    }
    // `mergeEnchantsFrom`: the other's enchantments, curses only if new.
    let mut enchantments = crafting_enchantments(data, &item);
    for (name, level) in crafting_enchantments(data, additional) {
        let current = enchantments
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, l)| *l);
        if !in_tag(data, &name, "minecraft:curse") || current.is_none() {
            match enchantments.iter_mut().find(|(n, _)| *n == name) {
                Some((_, l)) => *l = (*l).max(level),
                None => enchantments.push((name, level)),
            }
        }
    }
    set_crafting_enchantments(data, &mut item, &enchantments);
    Some(remove_non_curses(data, item))
}

/// `GrindstoneMenu.removeNonCursesFrom`: curses stay; a book without
/// enchantments is a plain book again.
fn remove_non_curses(data: &GameData, mut item: ItemStack) -> ItemStack {
    let left: Vec<(String, i32)> = crafting_enchantments(data, &item)
        .into_iter()
        .filter(|(name, _)| in_tag(data, name, "minecraft:curse"))
        .collect();
    set_crafting_enchantments(data, &mut item, &left);
    if is(data, &item, "minecraft:enchanted_book")
        && left.is_empty()
        && let Some(book) = data.item_id("minecraft:book")
    {
        let mut plain = item.clone();
        plain.item = book;
        stack::reset(data, &mut plain, "minecraft:stored_enchantments");
        item = plain;
    }
    let mut repair_cost = 0;
    for _ in 0..left.len() {
        repair_cost = increased_repair_cost(repair_cost);
    }
    stack::set(
        data,
        &mut item,
        "minecraft:repair_cost",
        &Nbt::Int(repair_cost),
    );
    item
}

/// `getExperienceFromItem`: the minimum cost of each enchantment but the
/// curses.
pub fn grind_experience(data: &GameData, item: Option<&ItemStack>) -> i32 {
    item.map_or(0, |item| {
        crafting_enchantments(data, item)
            .into_iter()
            .filter(|(name, _)| !in_tag(data, name, "minecraft:curse"))
            .filter_map(|(name, level)| Some(cost(data.enchantment(&name)?.1, "min_cost", level)))
            .sum()
    })
}

impl Game {
    /// `EnchantingTableBlock.isValidBookShelf` around a table.
    fn bookcases(&self, at: BlockPos) -> i32 {
        let world = &self.context.world;
        let data = &self.context.data;
        let mut count = 0;
        for y in 0..=1 {
            for x in -2..=2i32 {
                for z in -2..=2i32 {
                    if x.abs() != 2 && z.abs() != 2 {
                        continue;
                    }
                    let shelf = BlockPos {
                        x: at.x + x,
                        y: at.y + y,
                        z: at.z + z,
                    };
                    // Java's division rounds towards zero.
                    let between = BlockPos {
                        x: at.x + x / 2,
                        y: at.y + y,
                        z: at.z + z / 2,
                    };
                    if data.state_in_tag(world.block(shelf), "minecraft:enchantment_power_provider")
                        && data.state_in_tag(
                            world.block(between),
                            "minecraft:enchantment_power_transmitter",
                        )
                    {
                        count += 1;
                    }
                }
            }
        }
        count
    }

    /// `slotsChanged` and `createResult`: what the open table, anvil or
    /// grindstone offers now, and its data slots for the client.
    pub(super) fn refresh_workstation(&mut self, entity_id: i32) {
        let data = Arc::clone(&self.context.data);
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let Some(menu) = &player.menu else {
            return;
        };
        let creative = player.game_mode() == GameMode::Creative;
        let seed = player.enchantment_seed;
        let (kind, at) = (menu.kind(), menu.at);
        let grid = menu.grid.clone();
        let extra = match (kind, &menu.extra) {
            (Kind::Enchanting, _) => {
                let bookcases = at.map_or(0, |at| self.bookcases(at));
                Extra::Enchanting(offers(&data, seed, grid[0].as_ref(), bookcases))
            }
            (Kind::Anvil, Extra::Anvil { name, .. }) => {
                let name = name.clone();
                let result = anvil(
                    &data,
                    grid[0].as_ref(),
                    grid[1].as_ref(),
                    name.as_deref(),
                    creative,
                );
                Extra::Anvil { name, result }
            }
            (Kind::Grindstone, _) => {
                Extra::Grindstone(grindstone(&data, grid[0].as_ref(), grid[1].as_ref()))
            }
            _ => return,
        };
        let values: Vec<i32> = match &extra {
            Extra::Enchanting(o) => o
                .costs
                .iter()
                .copied()
                .chain(std::iter::once(seed))
                .chain(o.clues)
                .chain(o.levels)
                .collect(),
            Extra::Anvil { result, .. } => vec![result.cost],
            _ => Vec::new(),
        };
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        let Some(menu) = player.menu.as_mut() else {
            return;
        };
        menu.extra = extra;
        let id = menu.id;
        let mut packets = Vec::new();
        for (index, value) in values.iter().enumerate() {
            if menu.sent_data.get(index) == Some(value) {
                continue;
            }
            packets.push(Encoded::new(&ContainerSetData {
                container_id: id,
                id: index as i16,
                value: *value as i16,
            }));
        }
        menu.sent_data = values;
        for packet in packets {
            player.send(Outbound::Packet(Arc::new(packet)));
        }
    }

    /// `minecraft:container_button_click`: one of the enchanting table's
    /// offers (`EnchantmentMenu.clickMenuButton`).
    pub(super) fn menu_button(&mut self, entity_id: i32, container_id: i32, button: i32) {
        let data = Arc::clone(&self.context.data);
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let Some(menu) = &player.menu else {
            return;
        };
        if menu.id != container_id || menu.kind() != Kind::Enchanting || !(0..3).contains(&button) {
            return;
        }
        let Extra::Enchanting(offers) = menu.extra else {
            return;
        };
        let creative = player.game_mode() == GameMode::Creative;
        let slot = button as usize;
        let needed = button + 1;
        let (Some(item), lapis) = (menu.grid[0].clone(), menu.grid[1].clone()) else {
            return;
        };
        let lapis_count = lapis.as_ref().map_or(0, |l| l.count);
        let level = player.experience.level;
        if (lapis_count < needed && !creative)
            || offers.costs[slot] <= 0
            || ((level < needed || level < offers.costs[slot]) && !creative)
        {
            return;
        }
        let seed = player.enchantment_seed;
        let at = menu.at;
        let (list, _) = enchantment_list(&data, seed, &item, button, offers.costs[slot]);
        if list.is_empty() {
            return;
        }
        let mut enchanted = item;
        if is(&data, &enchanted, "minecraft:book")
            && let Some(book) = data.item_id("minecraft:enchanted_book")
        {
            enchanted = ItemStack {
                item: book,
                ..enchanted
            };
        }
        let mut current = crafting_enchantments(&data, &enchanted);
        for (name, level) in list {
            match current.iter_mut().find(|(n, _)| *n == name) {
                Some((_, l)) => *l = (*l).max(level),
                None => current.push((name, level)),
            }
        }
        set_crafting_enchantments(&data, &mut enchanted, &current);
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        // `onEnchantmentPerformed`.
        player.experience.level -= needed;
        if player.experience.level < 0 {
            player.experience = super::Experience {
                score: player.experience.score,
                ..Default::default()
            };
        }
        player.enchantment_seed = rand::random();
        if let Some(menu) = player.menu.as_mut() {
            menu.grid[0] = Some(enchanted);
            if !creative {
                menu.grid[1] = lapis.and_then(|l| {
                    let count = l.count - needed;
                    (count > 0).then_some(ItemStack { count, ..l })
                });
            }
        }
        self.send_experience(entity_id);
        self.refresh_workstation(entity_id);
        self.send_menu(entity_id);
        if let Some(at) = at {
            let pitch = rand::random::<f32>() * 0.1 + 0.9;
            let centre = [
                f64::from(at.x) + 0.5,
                f64::from(at.y) + 0.5,
                f64::from(at.z) + 0.5,
            ];
            self.play_sound_from(
                None,
                "minecraft:block.enchantment_table.use",
                centre,
                ferrocraft_protocol::packets::play::SOUND_SOURCE_BLOCKS,
                1.0,
                pitch,
            );
        }
    }

    /// `minecraft:rename_item`: the anvil's name field
    /// (`AnvilMenu.setItemName`).
    pub(super) fn rename_item(&mut self, entity_id: i32, name: &str) {
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        let Some(menu) = player.menu.as_mut() else {
            return;
        };
        // `StringUtil.filterText`, at most 50 characters.
        let name: String = name
            .chars()
            .filter(|&c| c != '§' && c >= ' ' && c != '\u{7f}')
            .collect();
        if name.chars().count() > 50 {
            return;
        }
        if let Extra::Anvil { name: current, .. } = &mut menu.extra {
            if current.as_deref() == Some(name.as_str()) {
                return;
            }
            *current = Some(name);
        } else {
            return;
        }
        self.refresh_workstation(entity_id);
        self.send_menu(entity_id);
    }

    /// After the result of an anvil or grindstone was taken: the price.
    pub(super) fn workstation_taken(&mut self, entity_id: i32, taken: super::menu::Taken) {
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let creative = player.game_mode() == GameMode::Creative;
        let Some(at) = player.menu.as_ref().and_then(|m| m.at) else {
            return;
        };
        match taken {
            super::menu::Taken::Anvil { cost } => {
                if !creative && let Some(player) = self.players.get_mut(&entity_id) {
                    player.experience.level = (player.experience.level - cost).max(0);
                    self.send_experience(entity_id);
                }
                // `AnvilBlock.damage`: one in eight uses chips it.
                let data = Arc::clone(&self.context.data);
                let state = self.context.world.block(at);
                let next = data
                    .block_of(state)
                    .and_then(|(_, b)| match b.name.as_str() {
                        "minecraft:anvil" => Some(Some("minecraft:chipped_anvil")),
                        "minecraft:chipped_anvil" => Some(Some("minecraft:damaged_anvil")),
                        "minecraft:damaged_anvil" => Some(None),
                        _ => None,
                    });
                if !creative
                    && rand::random::<f32>() < 0.12
                    && let Some(next) = next
                {
                    match next {
                        Some(name) => {
                            let facing = data
                                .block_of(state)
                                .and_then(|(_, b)| b.value(state, "facing").map(str::to_owned));
                            if let Some(block) = data.block_named(name) {
                                let mut changed = block.default;
                                if let Some(f) = facing
                                    && let Some(s) = block.with(changed, "facing", &f)
                                {
                                    changed = s;
                                }
                                self.set_block_with(at, changed, super::blocks::update::CLIENTS);
                                // Still an anvil: the screen stays open.
                                let id = data.block_of(changed).map(|(id, _)| id);
                                if let Some(menu) = self
                                    .players
                                    .get_mut(&entity_id)
                                    .and_then(|p| p.menu.as_mut())
                                {
                                    menu.block = id;
                                }
                            }
                            self.level_event(ANVIL_USED, at, 0);
                        }
                        None => {
                            self.remove_block_keeping_fluid(at);
                            self.level_event(ANVIL_BROKEN, at, 0);
                        }
                    }
                } else {
                    self.level_event(ANVIL_USED, at, 0);
                }
            }
            super::menu::Taken::Grindstone { experience } => {
                if experience > 0 {
                    let half = (experience + 1) / 2;
                    let amount = half + rand::random_range(0..half.max(1));
                    let centre = [
                        f64::from(at.x) + 0.5,
                        f64::from(at.y) + 0.5,
                        f64::from(at.z) + 0.5,
                    ];
                    self.award_experience(centre, amount);
                }
                self.level_event(GRINDSTONE_USED, at, 0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anvils_combine_and_repair_like_vanilla() {
        let data = GameData::load().unwrap();
        let id = |name: &str| data.item_id(name).unwrap();
        let sword = |enchantments: &[(&str, i32)], damage: i32| {
            let mut s = stack::plain(id("minecraft:diamond_sword"), 1);
            let list: Vec<(String, i32)> = enchantments
                .iter()
                .map(|(n, l)| (format!("minecraft:{n}"), *l))
                .collect();
            stack::set_enchantments_in(&data, &mut s, "minecraft:enchantments", &list);
            stack::set(&data, &mut s, "minecraft:damage", &Nbt::Int(damage));
            s
        };
        // Sharpness III and III make IV: four a level from a weapon, and the
        // repair of two (more than 12 % of the durability).
        let a = sword(&[("sharpness", 3)], 500);
        let b = sword(&[("sharpness", 3)], 0);
        let combined = anvil(&data, Some(&a), Some(&b), None, false);
        let result = combined.result.unwrap();
        assert_eq!(
            stack::enchantment_level(&data, &result, "minecraft:sharpness"),
            4
        );
        assert_eq!(stack::damage(&data, &result), 0);
        assert_eq!(combined.cost, 4 + 2);
        assert_eq!(stack::int(&data, &result, "minecraft:repair_cost"), 1);
        // Diamonds repair a quarter each: 390, 390, then the last 220.
        let diamonds = stack::plain(id("minecraft:diamond"), 5);
        let repaired = anvil(&data, Some(&sword(&[], 1000)), Some(&diamonds), None, false);
        assert_eq!(repaired.repair_count, 3);
        assert_eq!(stack::damage(&data, repaired.result.as_ref().unwrap()), 0);
        assert_eq!(repaired.cost, 3);
        // Smite does not join sharpness; nothing else to do, no result.
        let rival = sword(&[("smite", 1)], 0);
        assert_eq!(
            anvil(&data, Some(&b), Some(&rival), None, false).result,
            None
        );
        // A name alone costs one level.
        let renamed = anvil(&data, Some(&b), None, Some("Excalibur"), false);
        assert_eq!(renamed.cost, 1);
        assert!(renamed.only_renaming);
    }

    #[test]
    fn grindstones_keep_only_curses() {
        let data = GameData::load().unwrap();
        let mut book = stack::plain(data.item_id("minecraft:enchanted_book").unwrap(), 1);
        stack::set_enchantments_in(
            &data,
            &mut book,
            "minecraft:stored_enchantments",
            &[("minecraft:sharpness".into(), 5)],
        );
        let plain = grindstone(&data, Some(&book), None).unwrap();
        assert_eq!(plain.item, data.item_id("minecraft:book").unwrap());
        assert_eq!(grind_experience(&data, Some(&book)), 1 + 11 * 4);
    }

    #[test]
    fn tables_offer_by_bookshelves() {
        let data = GameData::load().unwrap();
        let sword = stack::plain(data.item_id("minecraft:diamond_sword").unwrap(), 1);
        let offers = offers(&data, 12345, Some(&sword), 15);
        assert!(offers.costs[2] >= 30, "{offers:?}");
        assert!(offers.costs[0] >= 1);
        assert!(offers.clues.iter().all(|&c| c >= 0));
        // Without anything enchantable nothing is offered.
        let stick = stack::plain(data.item_id("minecraft:stick").unwrap(), 1);
        assert_eq!(super::offers(&data, 1, Some(&stick), 15), Offers::default());
    }
}
