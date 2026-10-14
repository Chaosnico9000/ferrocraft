//! A creative player's inventory as the client sets it, by inventory menu
//! slot: 5–8 armor, 9–35 the main inventory, 36–44 the hotbar, 45 the off
//! hand.

use ferrocraft_protocol::packets::play::{ItemStack, equipment_slot};

pub const SLOTS: usize = 46;
const HOTBAR: usize = 36;
pub const OFF_HAND: usize = 45;
/// Menu slots and the equipment slot they show as.
const ARMOR: [(usize, u8); 4] = [
    (5, equipment_slot::HEAD),
    (6, equipment_slot::CHEST),
    (7, equipment_slot::LEGS),
    (8, equipment_slot::FEET),
];
/// Every equipment slot a player shows to others.
pub const EQUIPMENT: [u8; 6] = [
    equipment_slot::MAIN_HAND,
    equipment_slot::OFF_HAND,
    equipment_slot::FEET,
    equipment_slot::LEGS,
    equipment_slot::CHEST,
    equipment_slot::HEAD,
];

#[derive(Debug, Clone, PartialEq)]
pub struct Inventory {
    slots: Vec<Option<ItemStack>>,
    /// Hotbar slot in hand, 0 to 8.
    selected: usize,
}

impl Default for Inventory {
    fn default() -> Self {
        Self {
            slots: vec![None; SLOTS],
            selected: 0,
        }
    }
}

impl Inventory {
    pub fn new(stacks: &[(usize, ItemStack)], selected: usize) -> Self {
        let mut inventory = Self {
            selected: selected.min(8),
            ..Self::default()
        };
        for (slot, stack) in stacks {
            inventory.set(*slot, Some(stack.clone()));
        }
        inventory
    }

    /// Puts a stack into a menu slot; slots outside 1–45 are ignored, as by
    /// vanilla.
    pub fn set(&mut self, slot: usize, stack: Option<ItemStack>) {
        if (1..SLOTS).contains(&slot) {
            self.slots[slot] = stack;
        }
    }

    /// Selects a hotbar slot; anything but 0–8 is ignored.
    pub fn select(&mut self, slot: usize) {
        if slot <= 8 {
            self.selected = slot;
        }
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn slots(&self) -> &[Option<ItemStack>] {
        &self.slots
    }

    /// The stacks that are not empty, by menu slot.
    pub fn stacks(&self) -> Vec<(usize, ItemStack)> {
        (0..)
            .zip(&self.slots)
            .filter_map(|(slot, stack)| Some((slot, stack.clone()?)))
            .collect()
    }

    pub fn equipment(&self, slot: u8) -> Option<&ItemStack> {
        let index = match slot {
            equipment_slot::MAIN_HAND => HOTBAR + self.selected,
            equipment_slot::OFF_HAND => OFF_HAND,
            _ => ARMOR.iter().find(|(_, s)| *s == slot)?.0,
        };
        self.slots[index].as_ref()
    }

    /// Menu slots in the order vanilla fills them: the hotbar, then the rest
    /// of the inventory (`Inventory.items` from 0).
    fn fill_order() -> impl Iterator<Item = usize> {
        (HOTBAR..HOTBAR + 9).chain(9..HOTBAR)
    }

    /// `Inventory.add`: tops up a matching stack (the one in hand, the off
    /// hand, then in order) or takes a free slot, as often as needed. Returns
    /// what did not fit and the menu slots that changed. `max` is the item's
    /// stack size.
    pub fn add(&mut self, mut stack: ItemStack, max: i32) -> (Option<ItemStack>, Vec<usize>) {
        let mut changed = Vec::new();
        while stack.count > 0 {
            let fits = |slot: &Option<ItemStack>| {
                slot.as_ref().is_some_and(|s| {
                    s.item == stack.item && s.patch == stack.patch && s.count < max
                })
            };
            let target = [HOTBAR + self.selected, OFF_HAND]
                .into_iter()
                .chain(Self::fill_order())
                .find(|&slot| fits(&self.slots[slot]))
                .or_else(|| Self::fill_order().find(|&slot| self.slots[slot].is_none()));
            let Some(slot) = target else {
                return (Some(stack), changed);
            };
            let existing = self.slots[slot].get_or_insert_with(|| ItemStack {
                count: 0,
                ..stack.clone()
            });
            let moved = (max - existing.count).min(stack.count);
            existing.count += moved;
            stack.count -= moved;
            if !changed.contains(&slot) {
                changed.push(slot);
            }
        }
        (None, changed)
    }

    /// `Inventory.getSuitableHotbarSlot`: the selected slot if empty, else
    /// the first empty hotbar slot, else the selected one.
    pub fn suitable_hotbar_slot(&self) -> usize {
        if self.slots[HOTBAR + self.selected].is_none() {
            return self.selected;
        }
        (0..9)
            .find(|&i| self.slots[HOTBAR + i].is_none())
            .unwrap_or(self.selected)
    }

    /// The menu slot of the selected hotbar slot.
    pub fn selected_slot(&self) -> usize {
        HOTBAR + self.selected
    }

    /// Takes `count` items from a menu slot; returns what was taken.
    pub fn take(&mut self, slot: usize, count: i32) -> Option<ItemStack> {
        let stack = self.slots.get_mut(slot)?.as_mut()?;
        let taken = count.min(stack.count);
        stack.count -= taken;
        let out = ItemStack {
            count: taken,
            ..stack.clone()
        };
        if stack.count == 0 {
            self.slots[slot] = None;
        }
        Some(out)
    }

    pub fn get(&self, slot: usize) -> Option<&ItemStack> {
        self.slots.get(slot)?.as_ref()
    }

    /// `ServerGamePacketListenerImpl.tryPickItem`: selects a hotbar slot
    /// with the stack or swaps it into the hotbar; in creative, a missing
    /// stack is put into the hand. Returns the menu slots that changed.
    pub fn pick(&mut self, stack: ItemStack, creative: bool) -> Vec<usize> {
        let found = Self::fill_order().find(|&slot| {
            self.slots[slot]
                .as_ref()
                .is_some_and(|s| s.item == stack.item && s.patch == stack.patch)
        });
        match found {
            Some(slot) if slot >= HOTBAR => {
                self.selected = slot - HOTBAR;
                Vec::new()
            }
            // `Inventory.pickSlot`.
            Some(slot) => {
                self.selected = self.suitable_hotbar_slot();
                let hand = HOTBAR + self.selected;
                self.slots.swap(slot, hand);
                vec![slot, hand]
            }
            None if creative => {
                // `Inventory.addAndPickItem`.
                self.selected = self.suitable_hotbar_slot();
                let hand = HOTBAR + self.selected;
                let mut changed = vec![hand];
                if self.slots[hand].is_some()
                    && let Some(free) = Self::fill_order().find(|&slot| self.slots[slot].is_none())
                {
                    self.slots[free] = self.slots[hand].take();
                    changed.push(free);
                }
                self.slots[hand] = Some(stack);
                changed
            }
            None => Vec::new(),
        }
    }

    /// Menu slots of the main inventory, hotbar first.
    #[cfg(test)]
    pub fn storage_slots() -> impl Iterator<Item = usize> {
        Self::fill_order()
    }

    /// Everything the player shows to others, as `(equipment slot, stack)`.
    pub fn visible(&self) -> Vec<(u8, Option<ItemStack>)> {
        EQUIPMENT
            .iter()
            .map(|&slot| (slot, self.equipment(slot).cloned()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use ferrocraft_protocol::packets::play::ComponentPatch;

    use super::*;

    fn stack(item: i32) -> ItemStack {
        ItemStack {
            item,
            count: 1,
            patch: ComponentPatch::default(),
        }
    }

    #[test]
    fn adding_tops_up_then_fills_like_vanilla() {
        let mut inventory = Inventory::new(&[(40, stack(1)), (20, stack(1))], 4);
        let (left, changed) = inventory.add(
            ItemStack {
                count: 130,
                ..stack(1)
            },
            64,
        );
        assert_eq!(left, None);
        // The stack in the hotbar first (40), then the rest of the
        // inventory (20), then the first free hotbar slot (36).
        assert_eq!(changed, [40, 20, 36]);
        assert_eq!(inventory.get(40).unwrap().count, 64);
        assert_eq!(inventory.get(20).unwrap().count, 64);
        assert_eq!(inventory.get(36).unwrap().count, 4);
        // The selected slot (4) is taken, so the first empty one: 36 got
        // stone above, 37 is free.
        assert_eq!(inventory.suitable_hotbar_slot(), 1);

        let mut full = Inventory::new(&[], 0);
        for slot in Inventory::storage_slots() {
            full.set(
                slot,
                Some(ItemStack {
                    count: 64,
                    ..stack(2)
                }),
            );
        }
        let (left, _) = full.add(stack(1), 64);
        assert_eq!(left.unwrap().count, 1);
        assert_eq!(full.take(9, 10).unwrap().count, 10);
        assert_eq!(full.get(9).unwrap().count, 54);
    }

    #[test]
    fn picking_selects_swaps_or_conjures() {
        let mut inventory = Inventory::new(&[(38, stack(1)), (20, stack(2)), (36, stack(3))], 0);
        assert!(inventory.pick(stack(1), false).is_empty());
        assert_eq!(inventory.selected(), 2);
        // From the main inventory into the first empty hotbar slot (37).
        assert_eq!(inventory.pick(stack(2), false), [20, 37]);
        assert_eq!(inventory.selected(), 1);
        assert_eq!(inventory.get(37), Some(&stack(2)));
        assert!(inventory.pick(stack(4), false).is_empty());
        let changed = inventory.pick(stack(4), true);
        assert_eq!(
            inventory.equipment(equipment_slot::MAIN_HAND),
            Some(&stack(4))
        );
        assert_eq!(changed.len(), 1);
    }

    #[test]
    fn equipment_follows_the_selected_slot() {
        let mut inventory = Inventory::new(&[(36, stack(1)), (38, stack(2)), (5, stack(3))], 0);
        assert_eq!(
            inventory.equipment(equipment_slot::MAIN_HAND),
            Some(&stack(1))
        );
        assert_eq!(inventory.equipment(equipment_slot::HEAD), Some(&stack(3)));
        inventory.select(2);
        assert_eq!(
            inventory.equipment(equipment_slot::MAIN_HAND),
            Some(&stack(2))
        );
        inventory.select(9);
        assert_eq!(inventory.selected(), 2, "not a hotbar slot");
        inventory.set(46, Some(stack(4)));
        inventory.set(0, Some(stack(4)));
        assert_eq!(
            inventory.stacks().len(),
            3,
            "the crafting result and beyond stay empty"
        );
    }
}
