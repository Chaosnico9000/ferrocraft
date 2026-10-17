//! Hoppers (`HopperBlockEntity`) and the containers they and droppers move
//! items between: every eight ticks a hopper passes one item on to the
//! container it points into and takes one from the container above, or
//! picks up items lying on it. Furnaces take ingredients from above and
//! fuel from the sides and give their results downwards; brewing stands
//! take reagents from above and bottles and fuel from the sides
//! (`WorldlyContainer`). A powered hopper rests.

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{Direction, ItemStack};

use super::Game;
use super::blocks::{Kind, above, update};
use crate::block_entity::container_size;
use crate::placement::opposite;

/// `HopperBlockEntity.MOVE_ITEM_SPEED`.
const COOLDOWN: i32 = 8;
/// What a container holds at most per slot (`Container.getMaxStackSize`).
const CONTAINER_MAX: i32 = 99;

/// A container as hoppers and droppers see it: the slots of one block
/// entity, or of both halves of a double chest.
#[derive(Debug, Clone)]
pub struct Container {
    slots: Vec<(BlockPos, usize)>,
    /// A furnace, smoker, blast furnace or brewing stand, which offers its
    /// slots by side.
    sided: Sided,
    /// A hopper: where it is, for its cooldown.
    hopper: Option<BlockPos>,
    /// Shulker boxes take no shulker boxes.
    shulker: bool,
    /// The block entities, to tell comparators of a change.
    positions: Vec<BlockPos>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sided {
    No,
    Furnace,
    Brewing,
}

fn same(a: &ItemStack, b: &ItemStack) -> bool {
    a.item == b.item && a.patch == b.patch
}

impl Game {
    fn item_max(&self, stack: &ItemStack) -> i32 {
        self.context
            .data
            .item(stack.item)
            .map_or(64, |i| i.stack.max(1))
    }

    /// `ChestBlock.getContainer`: both halves of a double chest, the right
    /// one first; nothing if a lid is blocked, unless `ignore_blocked`.
    pub(super) fn chest_halves(&self, pos: BlockPos, ignore_blocked: bool) -> Vec<BlockPos> {
        let data = &self.context.data;
        let world = &self.context.world;
        let state = world.block(pos);
        let Some((id, block)) = data.block_of(state) else {
            return Vec::new();
        };
        if !ignore_blocked && self.chest_blocked(pos) {
            return Vec::new();
        }
        let mut halves = vec![pos];
        if let Some(towards) = crate::placement::chest_partner(block, state) {
            let other = towards.offset(pos);
            let other_state = world.block(other);
            let joined = data.block_of(other_state).is_some_and(|(o, _)| o == id)
                && crate::placement::chest_partner(block, other_state).map(|d| d.offset(other))
                    == Some(pos);
            if joined {
                if !ignore_blocked && self.chest_blocked(other) {
                    return Vec::new();
                }
                if block.value(state, "type") == Some("right") {
                    halves.push(other);
                } else {
                    halves.insert(0, other);
                }
            }
        }
        halves
    }

    /// `ChestBlock.isChestBlockedAt`: a solid block on top keeps the lid
    /// shut.
    fn chest_blocked(&self, pos: BlockPos) -> bool {
        let above_state = self.context.world.block(above(pos));
        let data = &self.context.data;
        data.physics(above_state).dampening == 15
            && data.collision_shape(above_state) == [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]
    }

    /// `HopperBlockEntity.getContainerAt`: the container block at `pos`.
    pub(super) fn container_at(&self, pos: BlockPos) -> Option<Container> {
        let entity = self.context.world.block_entity(pos)?;
        let kind = entity.kind.as_str();
        if container_size(kind) == 0 {
            return None;
        }
        let positions = if kind.ends_with("chest") {
            self.chest_halves(pos, true)
        } else {
            vec![pos]
        };
        let size = container_size(kind);
        Some(Container {
            slots: positions
                .iter()
                .flat_map(|&p| (0..size).map(move |i| (p, i)))
                .collect(),
            sided: if entity.furnace.is_some() {
                Sided::Furnace
            } else if entity.brewing.is_some() {
                Sided::Brewing
            } else {
                Sided::No
            },
            hopper: (kind == "minecraft:hopper").then_some(pos),
            shulker: kind == "minecraft:shulker_box",
            positions,
        })
    }

    fn slot(&self, container: &Container, index: usize) -> Option<ItemStack> {
        let (pos, slot) = *container.slots.get(index)?;
        self.context
            .world
            .block_entity(pos)?
            .items
            .get(slot)?
            .clone()
    }

    fn set_slot(&mut self, container: &Container, index: usize, stack: Option<ItemStack>) {
        let Some(&(pos, slot)) = container.slots.get(index) else {
            return;
        };
        let data = std::sync::Arc::clone(&self.context.data);
        let stack = stack.filter(|s| s.count > 0);
        self.context.world.update_block_entity(pos, |entity| {
            entity.set_item(slot, stack, &data);
        });
    }

    /// `getSlots`: which slots a side offers (`getSlotsForFace` of a
    /// furnace), all of them for other containers.
    fn slots_for(&self, container: &Container, side: Option<Direction>) -> Vec<usize> {
        match (container.sided, side) {
            (Sided::Furnace, Some(Direction::Down)) => vec![2, 1],
            (Sided::Furnace, Some(Direction::Up)) => vec![0],
            (Sided::Furnace, Some(_)) => vec![1],
            (Sided::Brewing, Some(Direction::Up)) => vec![3],
            (Sided::Brewing, Some(Direction::Down)) => vec![0, 1, 2, 3],
            (Sided::Brewing, Some(_)) => vec![0, 1, 2, 4],
            _ => (0..container.slots.len()).collect(),
        }
    }

    /// `canPlaceItem` and `canPlaceItemThroughFace`.
    fn may_place(&self, container: &Container, index: usize, stack: &ItemStack) -> bool {
        let data = &self.context.data;
        let name = data.item(stack.item).map_or("", |i| i.name.as_str());
        if container.shulker && name.ends_with("shulker_box") {
            return false;
        }
        if container.sided == Sided::Brewing {
            // `BrewingStandBlockEntity.canPlaceItem`.
            return match index {
                4 => super::brewing::is_fuel(data, stack),
                3 => super::brewing::is_reagent(data, stack),
                _ => {
                    super::brewing::is_potion_input(data, stack)
                        && self.slot(container, index).is_none()
                }
            };
        }
        if container.sided != Sided::Furnace {
            return true;
        }
        match index {
            2 => false,
            1 => {
                let fuel = self.slot(container, 1);
                data.item(stack.item).is_some_and(|i| i.fuel.is_some())
                    || (name == "minecraft:bucket"
                        && fuel.is_none_or(|f| {
                            data.item(f.item).map(|i| i.name.as_str()) != Some("minecraft:bucket")
                        }))
            }
            _ => true,
        }
    }

    /// `canTakeItemThroughFace`: only empty buckets leave a furnace's fuel
    /// slot from below.
    fn may_take(
        &self,
        container: &Container,
        index: usize,
        stack: &ItemStack,
        side: Direction,
    ) -> bool {
        if container.sided == Sided::Brewing && index == 3 {
            // Only an empty bottle leaves by the reagent's place.
            return self.context.data.item(stack.item).map(|i| i.name.as_str())
                == Some("minecraft:glass_bottle");
        }
        if container.sided == Sided::Furnace && side == Direction::Down && index == 1 {
            return self
                .context
                .data
                .item_tag("minecraft:furnace_fuel_bottom_takeable")
                .is_some_and(|items| items.contains(&stack.item));
        }
        true
    }

    fn container_empty(&self, container: &Container) -> bool {
        (0..container.slots.len()).all(|i| self.slot(container, i).is_none())
    }

    /// `HopperBlockEntity.addItem`: as much of `stack` as fits, into the
    /// slots the side offers; what is left.
    pub(super) fn add_to_container(
        &mut self,
        from: Option<&Container>,
        into: &Container,
        mut stack: ItemStack,
        side: Option<Direction>,
    ) -> Option<ItemStack> {
        let slots = if into.sided != Sided::No && side.is_some() {
            self.slots_for(into, side)
        } else {
            (0..into.slots.len()).collect()
        };
        for index in slots {
            if stack.count <= 0 {
                break;
            }
            stack = self.move_into_slot(from, into, stack, index)?;
        }
        (stack.count > 0).then_some(stack)
    }

    /// `tryMoveInItem`.
    fn move_into_slot(
        &mut self,
        from: Option<&Container>,
        into: &Container,
        mut stack: ItemStack,
        index: usize,
    ) -> Option<ItemStack> {
        if !self.may_place(into, index, &stack) {
            return Some(stack);
        }
        let was_empty = self.container_empty(into);
        let current = self.slot(into, index);
        let moved = match current {
            None => {
                let max = self.item_max(&stack).min(CONTAINER_MAX);
                let put = stack.count.min(max);
                self.set_slot(
                    into,
                    index,
                    Some(ItemStack {
                        count: put,
                        ..stack.clone()
                    }),
                );
                stack.count -= put;
                put > 0
            }
            Some(current) if same(&current, &stack) && current.count <= self.item_max(&current) => {
                let room = self.item_max(&stack) - current.count;
                let count = stack.count.min(room).max(0);
                if count > 0 {
                    self.set_slot(
                        into,
                        index,
                        Some(ItemStack {
                            count: current.count + count,
                            ..current
                        }),
                    );
                    stack.count -= count;
                }
                count > 0
            }
            Some(_) => false,
        };
        if moved {
            // A hopper that was empty waits before passing it on; one tick
            // less if the giving hopper ran first this tick.
            if was_empty && let Some(hopper) = into.hopper {
                let from_ticked = from
                    .and_then(|f| f.hopper)
                    .and_then(|p| self.context.world.block_entity(p))
                    .and_then(|e| e.hopper)
                    .map(|h| h.ticked);
                self.context.world.change_block_entity(hopper, |entity| {
                    let Some(state) = &mut entity.hopper else {
                        return false;
                    };
                    if state.cooldown > COOLDOWN {
                        return false;
                    }
                    let skip = i32::from(from_ticked.is_some_and(|t| state.ticked >= t));
                    state.cooldown = COOLDOWN - skip;
                    true
                });
            }
            for &pos in &into.positions {
                self.container_changed(pos);
            }
        }
        (stack.count > 0).then_some(stack)
    }

    /// `isFullContainer`.
    fn container_full(&self, container: &Container, side: Direction) -> bool {
        self.slots_for(container, Some(side)).into_iter().all(|i| {
            self.slot(container, i)
                .is_some_and(|s| s.count >= self.item_max(&s))
        })
    }

    // --- Hoppers ---------------------------------------------------------------------

    /// `HopperBlock.checkPoweredState`: power switches it off.
    pub(super) fn hopper_check_powered(&mut self, pos: BlockPos, state: u32) {
        let enabled = !self.has_neighbor_signal(pos);
        if enabled != self.bool_value(state, "enabled") {
            let changed = self.with(state, "enabled", if enabled { "true" } else { "false" });
            self.set_block_with(pos, changed, update::CLIENTS);
        }
    }

    /// `HopperBlockEntity.pushItemsTick`.
    pub(super) fn tick_hopper(&mut self, pos: BlockPos) {
        let now = self.context.level.game_time;
        let mut ready = false;
        self.context.world.change_block_entity(pos, |entity| {
            let Some(hopper) = &mut entity.hopper else {
                return false;
            };
            hopper.cooldown -= 1;
            hopper.ticked = now;
            if hopper.cooldown <= 0 {
                hopper.cooldown = 0;
                ready = true;
            }
            false
        });
        if !ready {
            return;
        }
        let state = self.context.world.block(pos);
        if self.kind(state) != Kind::Hopper || !self.bool_value(state, "enabled") {
            return;
        }
        let Some(hopper) = self.container_at(pos) else {
            return;
        };
        let mut changed = false;
        if !self.container_empty(&hopper) {
            changed = self.eject(pos, state, &hopper);
        }
        let full = (0..hopper.slots.len()).all(|i| {
            self.slot(&hopper, i)
                .is_some_and(|s| s.count == self.item_max(&s))
        });
        if !full {
            changed |= self.suck_in(pos, &hopper);
        }
        if changed {
            self.context.world.change_block_entity(pos, |entity| {
                if let Some(hopper) = &mut entity.hopper {
                    hopper.cooldown = COOLDOWN;
                }
                true
            });
            self.container_changed(pos);
        }
    }

    /// `ejectItems`: one item into the container the hopper points at.
    fn eject(&mut self, pos: BlockPos, state: u32, hopper: &Container) -> bool {
        let Some(facing) = self.direction_value(state, "facing") else {
            return false;
        };
        let Some(target) = self.container_at(facing.offset(pos)) else {
            return false;
        };
        let side = opposite(facing);
        if self.container_full(&target, side) {
            return false;
        }
        for index in 0..hopper.slots.len() {
            let Some(stack) = self.slot(hopper, index) else {
                continue;
            };
            let one = ItemStack {
                count: 1,
                ..stack.clone()
            };
            if self
                .add_to_container(Some(hopper), &target, one, Some(side))
                .is_none()
            {
                self.set_slot(
                    hopper,
                    index,
                    Some(ItemStack {
                        count: stack.count - 1,
                        ..stack
                    }),
                );
                return true;
            }
        }
        false
    }

    /// `suckInItems`: one item from the container above, or items lying
    /// in or on the hopper.
    fn suck_in(&mut self, pos: BlockPos, hopper: &Container) -> bool {
        let up = above(pos);
        if let Some(source) = self.container_at(up) {
            for index in self.slots_for(&source, Some(Direction::Down)) {
                let Some(stack) = self.slot(&source, index) else {
                    continue;
                };
                if !self.may_take(&source, index, &stack, Direction::Down) {
                    continue;
                }
                let one = ItemStack {
                    count: 1,
                    ..stack.clone()
                };
                if self
                    .add_to_container(Some(&source), hopper, one, None)
                    .is_none()
                {
                    self.set_slot(
                        &source,
                        index,
                        Some(ItemStack {
                            count: stack.count - 1,
                            ..stack
                        }),
                    );
                    for &p in &source.positions {
                        self.container_changed(p);
                    }
                    return true;
                }
            }
            return false;
        }
        let data = &self.context.data;
        let above_state = self.context.world.block(up);
        let blocked = data.collision_shape(above_state) == [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]
            && !data.state_in_tag(above_state, "minecraft:does_not_block_hoppers");
        if blocked {
            return false;
        }
        // `Hopper.SUCK_AABB`: the bowl and the block above.
        let (x, y, z) = (f64::from(pos.x), f64::from(pos.y), f64::from(pos.z));
        let area = [x, y + 0.6875, z, x + 1.0, y + 2.0, z + 1.0];
        let mut lying: Vec<i32> = self
            .items
            .iter()
            .filter(|(_, item)| {
                let b = item.bounds();
                (0..3).all(|i| b[i] < area[i + 3] && b[i + 3] > area[i])
            })
            .map(|(&id, _)| id)
            .collect();
        lying.sort_unstable();
        for id in lying {
            let Some(stack) = self.items.get(&id).map(|i| i.stack.clone()) else {
                continue;
            };
            match self.add_to_container(None, hopper, stack, None) {
                None => {
                    self.remove_item(id);
                    return true;
                }
                Some(rest) => self.set_item_stack(id, rest),
            }
        }
        false
    }
}
