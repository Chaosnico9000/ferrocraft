//! What an item stack is beyond its item and count: its components
//! (`ItemStack.getComponents`). A stack keeps the components that differ
//! from its item's defaults in their wire form; this module reads and
//! changes them in the form vanilla saves (see
//! [`ferrocraft_protocol::component`]) and falls back to the item's
//! defaults from `items.json`.

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::component::{self, Registries};
use ferrocraft_protocol::packets::play::{ComponentPatch, ItemStack};
use serde_json::Value;

use crate::data::GameData;

impl Registries for GameData {
    fn name(&self, registry: &str, id: i32) -> Option<String> {
        if registry == "minecraft:item" {
            return self.item(id).map(|i| i.name.clone());
        }
        if let Some(names) = self.static_registry(registry) {
            return names.get(usize::try_from(id).ok()?).cloned();
        }
        let entry = self
            .registries
            .iter()
            .find(|r| r.registry == registry)?
            .entries
            .get(usize::try_from(id).ok()?)?;
        Some(entry.id.clone())
    }

    fn id(&self, registry: &str, name: &str) -> Option<i32> {
        if registry == "minecraft:item" {
            return self.item_id(name);
        }
        if let Some(names) = self.static_registry(registry) {
            return names.iter().position(|n| n == name).map(|i| i as i32);
        }
        self.registry_id(registry, name).ok()
    }
}

/// Vanilla's JSON (a data pack, a report) as NBT: whole numbers become
/// ints, the rest doubles, booleans bytes.
pub fn json_to_nbt(value: &Value) -> Nbt {
    match value {
        Value::Null => Nbt::Compound(Vec::new()),
        Value::Bool(b) => Nbt::Byte(i8::from(*b)),
        Value::Number(n) => match n.as_i64().and_then(|i| i32::try_from(i).ok()) {
            Some(i) => Nbt::Int(i),
            None => Nbt::Double(n.as_f64().unwrap_or(0.0)),
        },
        Value::String(s) => Nbt::String(s.clone()),
        Value::Array(items) => Nbt::List(items.iter().map(json_to_nbt).collect()),
        Value::Object(entries) => Nbt::Compound(
            entries
                .iter()
                .map(|(k, v)| (k.clone(), json_to_nbt(v)))
                .collect(),
        ),
    }
}

/// A stack of `count` of an item, without components.
pub fn plain(item: i32, count: i32) -> ItemStack {
    ItemStack {
        item,
        count,
        patch: ComponentPatch::default(),
    }
}

/// The value of a component as it is saved: the stack's own, else the
/// item's default.
pub fn get(data: &GameData, stack: &ItemStack, name: &str) -> Option<Nbt> {
    let kind = data.data_component_type(name).ok()?;
    if let Some((_, bytes)) = stack.patch.added.iter().find(|(t, _)| *t == kind) {
        return component::component(name)?.from_wire(bytes, data).ok();
    }
    if stack.patch.removed.contains(&kind) {
        return None;
    }
    default(data, stack.item, name)
}

/// The item's default value of a component.
pub fn default(data: &GameData, item: i32, name: &str) -> Option<Nbt> {
    let item = data.item(item)?;
    let short = name.trim_start_matches("minecraft:");
    match short {
        "max_damage" => item.max_damage.map(Nbt::Int),
        "damage" => item.max_damage.map(|_| Nbt::Int(0)),
        "max_stack_size" => Some(Nbt::Int(item.stack)),
        // Every item has these.
        "enchantments" => Some(Nbt::Compound(Vec::new())),
        "repair_cost" => Some(Nbt::Int(0)),
        "lore" => Some(Nbt::List(Vec::new())),
        _ => item.components.get(short).map(json_to_nbt),
    }
}

/// Sets a component; a value equal to the default takes the stack's own
/// one away.
pub fn set(data: &GameData, stack: &mut ItemStack, name: &str, value: &Nbt) {
    let Ok(kind) = data.data_component_type(name) else {
        return;
    };
    let Some(component) = component::component(name) else {
        return;
    };
    let Ok(bytes) = component.to_wire(value, data) else {
        tracing::warn!(name, ?value, "a component value that does not fit");
        return;
    };
    stack.patch.added.retain(|(t, _)| *t != kind);
    stack.patch.removed.retain(|t| *t != kind);
    let same_as_default = default(data, stack.item, name)
        .and_then(|d| component.to_wire(&d, data).ok())
        .is_some_and(|d| d == bytes);
    if !same_as_default {
        stack.patch.added.push((kind, bytes));
        stack.patch.added.sort_by_key(|(t, _)| *t);
    }
}

/// Takes a component away: the stack's own value, and the default too.
pub fn remove(data: &GameData, stack: &mut ItemStack, name: &str) {
    let Ok(kind) = data.data_component_type(name) else {
        return;
    };
    stack.patch.added.retain(|(t, _)| *t != kind);
    if default(data, stack.item, name).is_some() && !stack.patch.removed.contains(&kind) {
        stack.patch.removed.push(kind);
        stack.patch.removed.sort_unstable();
    }
}

/// Back to the item's default value.
pub fn reset(data: &GameData, stack: &mut ItemStack, name: &str) {
    if let Ok(kind) = data.data_component_type(name) {
        stack.patch.added.retain(|(t, _)| *t != kind);
        stack.patch.removed.retain(|t| *t != kind);
    }
}

/// Components in one order, so that equal stacks compare equal.
pub fn normalize(stack: &mut ItemStack) {
    stack.patch.added.sort_by_key(|(t, _)| *t);
    stack.patch.removed.sort_unstable();
}

/// An int component, 0 if missing.
pub fn int(data: &GameData, stack: &ItemStack, name: &str) -> i32 {
    match get(data, stack, name) {
        Some(Nbt::Int(v)) => v,
        _ => 0,
    }
}

/// A number component as a float (`potion_duration_scale`), whatever
/// number type it was saved as.
pub fn float(data: &GameData, stack: &ItemStack, name: &str) -> Option<f32> {
    match get(data, stack, name)? {
        Nbt::Float(v) => Some(v),
        Nbt::Double(v) => Some(v as f32),
        Nbt::Int(v) => Some(v as f32),
        _ => None,
    }
}

/// Wear on the item (`minecraft:damage`).
pub fn damage(data: &GameData, stack: &ItemStack) -> i32 {
    int(data, stack, "minecraft:damage")
}

/// `ItemStack.isDamageableItem`: it wears out and is not unbreakable.
pub fn damageable(data: &GameData, stack: &ItemStack) -> bool {
    get(data, stack, "minecraft:max_damage").is_some()
        && get(data, stack, "minecraft:unbreakable").is_none()
        && get(data, stack, "minecraft:damage").is_some()
}

/// The highest wear before it breaks.
pub fn max_damage(data: &GameData, stack: &ItemStack) -> i32 {
    int(data, stack, "minecraft:max_damage")
}

/// The stack size the stack allows.
pub fn max_stack_size(data: &GameData, stack: &ItemStack) -> i32 {
    match get(data, stack, "minecraft:max_stack_size") {
        Some(Nbt::Int(v)) => v.max(1),
        _ => 64,
    }
}

/// The enchantments of a map component (`enchantments`,
/// `stored_enchantments`) by name, with their level.
pub fn enchantments_in(data: &GameData, stack: &ItemStack, name: &str) -> Vec<(String, i32)> {
    match get(data, stack, name) {
        Some(Nbt::Compound(entries)) => entries
            .into_iter()
            .filter_map(|(k, v)| match v {
                Nbt::Int(level) if level > 0 => Some((k, level)),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The enchantments on the stack (`minecraft:enchantments`).
pub fn enchantments(data: &GameData, stack: &ItemStack) -> Vec<(String, i32)> {
    enchantments_in(data, stack, "minecraft:enchantments")
}

/// The level of one enchantment on the stack, 0 without it.
pub fn enchantment_level(data: &GameData, stack: &ItemStack, enchantment: &str) -> i32 {
    enchantments(data, stack)
        .into_iter()
        .find(|(name, _)| name == enchantment)
        .map_or(0, |(_, level)| level)
}

/// Writes a map component of enchantments; an empty map takes the stack's
/// own value away.
pub fn set_enchantments_in(
    data: &GameData,
    stack: &mut ItemStack,
    name: &str,
    enchantments: &[(String, i32)],
) {
    let value = Nbt::Compound(
        enchantments
            .iter()
            .filter(|(_, level)| *level > 0)
            .map(|(k, level)| (k.clone(), Nbt::Int((*level).min(255))))
            .collect(),
    );
    set(data, stack, name, &value);
}

/// Whether two stacks are of the same item with the same components
/// (`ItemStack.isSameItemSameComponents`).
pub fn same_item_same_components(a: &ItemStack, b: &ItemStack) -> bool {
    a.item == b.item && a.patch == b.patch
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_fall_back_to_the_item() {
        let data = GameData::load().unwrap();
        let sword = data.item_id("minecraft:diamond_sword").unwrap();
        let mut stack = plain(sword, 1);
        assert_eq!(max_damage(&data, &stack), 1561);
        assert_eq!(damage(&data, &stack), 0);
        assert!(damageable(&data, &stack));
        assert_eq!(
            get(&data, &stack, "minecraft:enchantable"),
            Some(Nbt::Compound(vec![("value".into(), Nbt::Int(10))]))
        );

        set(&data, &mut stack, "minecraft:damage", &Nbt::Int(12));
        assert_eq!(damage(&data, &stack), 12);
        set_enchantments_in(
            &data,
            &mut stack,
            "minecraft:enchantments",
            &[("minecraft:sharpness".into(), 3)],
        );
        assert_eq!(enchantment_level(&data, &stack, "minecraft:sharpness"), 3);
        // Back to the default: no own value left.
        set(&data, &mut stack, "minecraft:damage", &Nbt::Int(0));
        assert_eq!(stack.patch.added.len(), 1);
        remove(&data, &mut stack, "minecraft:damage");
        assert!(!damageable(&data, &stack));
        set(
            &data,
            &mut stack,
            "minecraft:unbreakable",
            &Nbt::Compound(vec![]),
        );
        reset(&data, &mut stack, "minecraft:damage");
        assert!(!damageable(&data, &stack), "unbreakable");
    }
}
