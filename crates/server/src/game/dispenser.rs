//! Dispensers and droppers (`DispenserBlock`, `DropperBlock`): power
//! triggers them, four ticks later they use a random filled slot. A
//! dropper puts the item into the container in front or throws it out; a
//! dispenser also shoots arrows, pours and fills buckets, lights fire and
//! TNT, sets TNT off and spreads bone meal (`DispenseItemBehavior`).
//! Items Ferrocraft has no behaviour for are thrown out like a dropper's.

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{Direction, ItemStack};
use rand::Rng;

use super::Game;
use super::blocks::{Kind, above, priority, update};
use super::fluid::Fluid;
use super::projectile::{Pickup, Shot};
use crate::placement::opposite;

/// Level events: dispensed, failed, shot (`LevelEvent`), and the smoke of
/// dispensing.
const DISPENSED: i32 = 1000;
const FAILED: i32 = 1001;
const SHOT: i32 = 1002;
const SMOKE: i32 = 2000;

/// What a behaviour did with the stack.
enum Outcome {
    /// The slot holds this now; the sound of success.
    Done(Option<ItemStack>),
    /// Nothing happened; the sound of failure.
    Failed(ItemStack),
}

fn steps(direction: Direction) -> [f64; 3] {
    let one = direction.offset(BlockPos { x: 0, y: 0, z: 0 });
    [f64::from(one.x), f64::from(one.y), f64::from(one.z)]
}

impl Game {
    /// `DispenserBlock.neighborChanged`: power, at the block or the one
    /// above, triggers it once.
    pub(super) fn dispenser_neighbor_changed(&mut self, pos: BlockPos, state: u32) {
        let should = self.has_neighbor_signal(pos) || self.has_neighbor_signal(above(pos));
        let triggered = self.bool_value(state, "triggered");
        if should && !triggered {
            self.schedule_block_tick(pos, state, 4, priority::NORMAL);
            let on = self.with(state, "triggered", "true");
            self.set_block_with(pos, on, update::CLIENTS);
        } else if !should && triggered {
            let off = self.with(state, "triggered", "false");
            self.set_block_with(pos, off, update::CLIENTS);
        }
    }

    /// `DispenserBlockEntity.getRandomSlot`: any filled slot, all equally
    /// likely.
    fn random_slot(&self, pos: BlockPos) -> Option<usize> {
        let entity = self.context.world.block_entity(pos)?;
        let mut rng = rand::rng();
        let mut picked = None;
        let mut odds = 1;
        for (slot, stack) in entity.items.iter().enumerate() {
            if stack.is_some() {
                if rng.random_range(0..odds) == 0 {
                    picked = Some(slot);
                }
                odds += 1;
            }
        }
        picked
    }

    /// `dispenseFrom`, the tick after the trigger.
    pub(super) fn dispense(&mut self, pos: BlockPos, state: u32) {
        let Some(slot) = self.random_slot(pos) else {
            self.level_event(FAILED, pos, 0);
            return;
        };
        let Some(stack) = self
            .context
            .world
            .block_entity(pos)
            .and_then(|e| e.items.get(slot).cloned().flatten())
        else {
            return;
        };
        let Some(facing) = self.direction_value(state, "facing") else {
            return;
        };
        let left = if self.kind(state) == Kind::Dropper {
            self.drop_into(pos, facing, stack)
        } else {
            self.dispense_item(pos, facing, stack)
        };
        let data = std::sync::Arc::clone(&self.context.data);
        self.context.world.update_block_entity(pos, |entity| {
            entity.set_item(slot, left, &data);
        });
        self.container_changed(pos);
    }

    /// `DropperBlock.dispenseFrom`: into the container in front, else out.
    fn drop_into(
        &mut self,
        pos: BlockPos,
        facing: Direction,
        stack: ItemStack,
    ) -> Option<ItemStack> {
        let Some(target) = self.container_at(facing.offset(pos)) else {
            return self.throw_out(pos, facing, stack);
        };
        let dropper = self.container_at(pos);
        let one = ItemStack {
            count: 1,
            ..stack.clone()
        };
        let rest = self.add_to_container(dropper.as_ref(), &target, one, Some(opposite(facing)));
        if rest.is_none() {
            let count = stack.count - 1;
            (count > 0).then_some(ItemStack { count, ..stack })
        } else {
            Some(stack)
        }
    }

    /// `DefaultDispenseItemBehavior.dispense`: one item flies out.
    fn throw_out(
        &mut self,
        pos: BlockPos,
        facing: Direction,
        stack: ItemStack,
    ) -> Option<ItemStack> {
        let one = ItemStack {
            count: 1,
            ..stack.clone()
        };
        self.spawn_dispensed(pos, facing, one);
        self.level_event(DISPENSED, pos, 0);
        self.level_event(SMOKE, pos, facing as i32);
        let count = stack.count - 1;
        (count > 0).then_some(ItemStack { count, ..stack })
    }

    /// `DefaultDispenseItemBehavior.spawnItem` with accuracy 6.
    fn spawn_dispensed(&mut self, pos: BlockPos, facing: Direction, stack: ItemStack) {
        let [sx, sy, sz] = steps(facing);
        let mut at = [
            f64::from(pos.x) + 0.5 + 0.7 * sx,
            f64::from(pos.y) + 0.5 + 0.7 * sy,
            f64::from(pos.z) + 0.5 + 0.7 * sz,
        ];
        at[1] -= if sy != 0.0 { 0.125 } else { 0.15625 };
        let mut rng = rand::rng();
        let power = rng.random::<f64>() * 0.1 + 0.2;
        let spread = 0.017_227_5 * 6.0;
        let velocity = [
            super::mob::triangle(&mut rng, sx * power, spread),
            super::mob::triangle(&mut rng, 0.2, spread),
            super::mob::triangle(&mut rng, sz * power, spread),
        ];
        self.spawn_item(at, velocity, stack, 0);
    }

    /// `DispenserBlock.getDispenseMethod`: the behaviour for the item.
    fn dispense_item(
        &mut self,
        pos: BlockPos,
        facing: Direction,
        stack: ItemStack,
    ) -> Option<ItemStack> {
        let data = std::sync::Arc::clone(&self.context.data);
        let name = data.item(stack.item).map_or("", |i| i.name.as_str());
        let target = facing.offset(pos);
        let outcome = match name {
            "minecraft:arrow" | "minecraft:tipped_arrow" | "minecraft:spectral_arrow" => {
                return self.shoot_from(pos, facing, stack);
            }
            "minecraft:water_bucket" | "minecraft:lava_bucket" => {
                let fluid = if name == "minecraft:water_bucket" {
                    Fluid::Water
                } else {
                    Fluid::Lava
                };
                if self.empty_bucket(fluid, target, None, false) {
                    Outcome::Done(self.consume_with_remainder(
                        pos,
                        facing,
                        stack,
                        "minecraft:bucket",
                    ))
                } else {
                    return self.throw_out(pos, facing, stack);
                }
            }
            "minecraft:bucket" => {
                let state = self.context.world.block(target);
                match self.pick_up_fluid(target, state) {
                    Some(fluid) => Outcome::Done(self.consume_with_remainder(
                        pos,
                        facing,
                        stack,
                        fluid.bucket(),
                    )),
                    None => return self.throw_out(pos, facing, stack),
                }
            }
            "minecraft:bone_meal" => {
                if self.bone_meal(target) {
                    self.level_event(super::growth::LEVEL_EVENT_BONE_MEAL, target, 15);
                    let count = stack.count - 1;
                    Outcome::Done((count > 0).then_some(ItemStack { count, ..stack }))
                } else {
                    Outcome::Failed(stack)
                }
            }
            "minecraft:tnt" if !self.rules.flag("tnt_explodes") => {
                return self.throw_out(pos, facing, stack);
            }
            "minecraft:tnt" => {
                self.prime_tnt(target, None, super::tnt::DEFAULT_FUSE);
                let count = stack.count - 1;
                Outcome::Done((count > 0).then_some(ItemStack { count, ..stack }))
            }
            "minecraft:flint_and_steel" => self.dispense_fire(target, facing, stack),
            _ => return self.throw_out(pos, facing, stack),
        };
        match outcome {
            Outcome::Done(left) => {
                self.level_event(DISPENSED, pos, 0);
                self.level_event(SMOKE, pos, facing as i32);
                left
            }
            Outcome::Failed(stack) => {
                self.level_event(FAILED, pos, 0);
                self.level_event(SMOKE, pos, facing as i32);
                Some(stack)
            }
        }
    }

    /// `ProjectileDispenseBehavior` for arrows.
    fn shoot_from(
        &mut self,
        pos: BlockPos,
        facing: Direction,
        stack: ItemStack,
    ) -> Option<ItemStack> {
        let [sx, sy, sz] = steps(facing);
        let from = [
            f64::from(pos.x) + 0.5 + 0.7 * sx,
            f64::from(pos.y) + 0.5 + 0.7 * sy + 0.1,
            f64::from(pos.z) + 0.5 + 0.7 * sz,
        ];
        self.shoot_arrow(Shot {
            owner: None,
            from,
            direction: [sx, sy, sz],
            speed: 1.1,
            inaccuracy: 6.0,
            base_damage: 2.0,
            crit: false,
            pickup: Pickup::Allowed,
            item: ItemStack {
                count: 1,
                ..stack.clone()
            },
            weapon: None,
        });
        self.level_event(SHOT, pos, 0);
        self.level_event(SMOKE, pos, facing as i32);
        let count = stack.count - 1;
        (count > 0).then_some(ItemStack { count, ..stack })
    }

    /// `consumeWithRemainder`: one used up, the remainder (an empty or
    /// full bucket) in its place or wherever it fits.
    fn consume_with_remainder(
        &mut self,
        pos: BlockPos,
        facing: Direction,
        stack: ItemStack,
        remainder: &str,
    ) -> Option<ItemStack> {
        let item = self.context.data.item_id(remainder)?;
        let remainder = ItemStack {
            item,
            count: 1,
            patch: Default::default(),
        };
        let count = stack.count - 1;
        if count <= 0 {
            return Some(remainder);
        }
        // `insertItem`: into the dispenser's own slots, else out it goes.
        if let Some(dispenser) = self.container_at(pos)
            && let Some(rest) = self.add_to_container(None, &dispenser, remainder, None)
        {
            self.spawn_dispensed(pos, facing, rest);
            self.level_event(DISPENSED, pos, 0);
            self.level_event(SMOKE, pos, facing as i32);
        }
        Some(ItemStack { count, ..stack })
    }

    /// `FlintAndSteelDispenseItemBehavior`: fire in front, or the TNT there
    /// lit; the flint and steel wears.
    fn dispense_fire(&mut self, target: BlockPos, facing: Direction, stack: ItemStack) -> Outcome {
        let state = self.context.world.block(target);
        let lit = if self.can_place_fire(target, facing) {
            let fire = self.fire_state_at(target);
            self.set_block(target, fire);
            true
        } else if self.can_light(state) {
            let on = self.with(state, "lit", "true");
            self.set_block(target, on);
            true
        } else if self.kind(state) == Kind::Tnt && self.rules.flag("tnt_explodes") {
            self.remove_block_keeping_fluid(target);
            self.prime_tnt(target, None, super::tnt::DEFAULT_FUSE);
            true
        } else {
            false
        };
        if !lit {
            return Outcome::Failed(stack);
        }
        match self.worn(stack.clone(), 1) {
            Some(worn) => Outcome::Done(worn),
            None => Outcome::Done(Some(stack)),
        }
    }
}
