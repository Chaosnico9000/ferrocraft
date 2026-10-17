//! Furnaces, smokers and blast furnaces at work
//! (`AbstractFurnaceBlockEntity.serverTick`): fuel burns while there is
//! something to cook, the cooked item goes into the result slot, the block
//! glows while lit. Taking the result out pays the experience of what was
//! cooked; the screen shows fire and progress (`container_set_data`).

use std::sync::Arc;

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{ContainerSetData, ItemStack};
use rand::Rng;

use super::{Encoded, Game, Outbound};
use crate::block_entity::BlockEntity;
use crate::crafting::{self, Cooker};
use crate::data::GameData;

/// A container's stack limit (`Container.getMaxStackSize`).
const MAX_STACK: i32 = 99;

/// What a tick of a furnace changed.
#[derive(Debug, Default)]
struct Outcome {
    /// Its slots or progress changed.
    changed: bool,
    /// It went out or caught fire.
    lit: Option<bool>,
    /// A bucket left from the lava, with no room in the fuel slot.
    remainder: Option<ItemStack>,
}

fn same(a: &ItemStack, b: &ItemStack) -> bool {
    a.item == b.item && a.patch == b.patch
}

/// One tick of a furnace.
fn tick(entity: &mut BlockEntity, cooker: Cooker, data: &GameData) -> Outcome {
    let mut out = Outcome::default();
    let Some(furnace) = entity.furnace.as_mut() else {
        return out;
    };
    let was_lit = furnace.lit_remaining > 0;
    if was_lit {
        furnace.lit_remaining -= 1;
    }
    let mut lit = furnace.lit_remaining > 0;
    let ingredient = entity.items[0].clone();
    let fuel = entity.items[1].clone();
    if lit || (fuel.is_some() && ingredient.is_some()) {
        if let Some(ingredient) = &ingredient {
            if let Some(recipe) = data.recipes.cook(cooker, ingredient.item) {
                let result = ItemStack {
                    item: recipe.output.item,
                    count: recipe.output.count,
                    patch: Default::default(),
                };
                let max = data
                    .item(result.item)
                    .map_or(64, |i| i.stack)
                    .min(MAX_STACK);
                // `canBurn`: room in the result slot.
                let fits = match &entity.items[2] {
                    None => true,
                    Some(there) => same(there, &result) && there.count + result.count <= max,
                };
                if fits {
                    if !lit {
                        let fast = usize::from(cooker.fast());
                        let (ticks, speed) = fuel
                            .as_ref()
                            .and_then(|f| data.item(f.item))
                            .and_then(|i| i.fuel)
                            .map_or((0, 1.0), |f| f[fast]);
                        furnace.lit_remaining = ticks;
                        furnace.lit_total = ticks;
                        furnace.speed = speed;
                        if furnace.cooking_total > 0 && furnace.cooking < furnace.cooking_total {
                            let ratio = furnace.cooking as f32 / furnace.cooking_total as f32;
                            furnace.cooking_total = cook_time(recipe.time, speed);
                            furnace.cooking = (ratio * furnace.cooking_total as f32).ceil() as i32;
                        }
                        if ticks > 0 {
                            // `consumeFuel`: lava leaves its bucket.
                            if let Some(mut fuel) = fuel.clone() {
                                let remainder = data
                                    .item(fuel.item)
                                    .and_then(|i| crafting::remainder(&i.name))
                                    .and_then(|name| data.item_id(name));
                                fuel.count -= 1;
                                let left = (fuel.count > 0).then_some(fuel);
                                entity.items[1] = match (left, remainder) {
                                    (None, Some(bucket)) => Some(ItemStack {
                                        item: bucket,
                                        count: 1,
                                        patch: Default::default(),
                                    }),
                                    (left, Some(bucket)) => {
                                        out.remainder = Some(ItemStack {
                                            item: bucket,
                                            count: 1,
                                            patch: Default::default(),
                                        });
                                        left
                                    }
                                    (left, None) => left,
                                };
                            }
                            lit = true;
                            out.changed = true;
                        }
                    }
                    if lit {
                        furnace.cooking += 1;
                        if furnace.cooking >= furnace.cooking_total {
                            furnace.cooking = 0;
                            furnace.cooking_total = cook_time(recipe.time, furnace.speed);
                            // `burn`.
                            match &mut entity.items[2] {
                                Some(there) => there.count += result.count,
                                slot => *slot = Some(result),
                            }
                            if let Some(input) = &mut entity.items[0] {
                                input.count -= 1;
                                if input.count <= 0 {
                                    entity.items[0] = None;
                                }
                            }
                            let name = recipe.name.clone();
                            match furnace.recipes_used.iter_mut().find(|(n, _)| *n == name) {
                                Some((_, count)) => *count += 1,
                                None => furnace.recipes_used.push((name, 1)),
                            }
                            out.changed = true;
                        }
                    } else {
                        furnace.cooking = 0;
                    }
                } else {
                    furnace.cooking = 0;
                }
            }
        } else {
            furnace.cooking = 0;
        }
    } else if furnace.cooking > 0 {
        // Cooling down twice as fast as it cooked.
        furnace.cooking = (furnace.cooking - 2).clamp(0, furnace.cooking_total);
    }
    if was_lit != lit {
        out.changed = true;
        out.lit = Some(lit);
    }
    out
}

/// `getTotalCookTime`: a recipe's time at the fuel's speed.
fn cook_time(time: i32, speed: f32) -> i32 {
    if speed > 0.0 {
        (time as f32 / speed).ceil() as i32
    } else {
        time
    }
}

/// The four values a furnace screen shows.
pub fn screen_data(entity: &BlockEntity) -> [i32; 4] {
    entity.furnace.as_ref().map_or([0; 4], |f| {
        [f.lit_remaining, f.lit_total, f.cooking, f.cooking_total]
    })
}

impl Game {
    /// `Level.tickBlockEntities`: furnaces, moving pistons and hoppers.
    pub(super) fn tick_block_entities(&mut self) {
        let world = Arc::clone(&self.context.world);
        for pos in world.ticking_block_entities() {
            let Some(kind) = world.block_entity(pos).map(|e| e.kind) else {
                continue;
            };
            match kind.as_str() {
                "minecraft:piston" => self.tick_moving(pos),
                "minecraft:hopper" => self.tick_hopper(pos),
                "minecraft:brewing_stand" => self.tick_brewing(pos),
                _ => self.tick_furnace(pos),
            }
        }
        self.send_furnace_data();
    }

    /// `AbstractFurnaceBlockEntity.serverTick`.
    fn tick_furnace(&mut self, pos: BlockPos) {
        let data = Arc::clone(&self.context.data);
        let world = Arc::clone(&self.context.world);
        let Some(cooker) = world.block_entity(pos).and_then(|e| Cooker::of(&e.kind)) else {
            return;
        };
        let mut outcome = Outcome::default();
        world.change_block_entity(pos, |entity| {
            outcome = tick(entity, cooker, &data);
            outcome.changed
        });
        if let Some(lit) = outcome.lit {
            let state = world.block(pos);
            if let Some((_, block)) = data.block_of(state)
                && let Some(changed) = block.with(state, "lit", if lit { "true" } else { "false" })
            {
                self.set_block(pos, changed);
            }
        }
        if let Some(bucket) = outcome.remainder {
            self.drop_at_block(pos, bucket);
        }
        if outcome.changed {
            self.refresh_viewers(pos);
            self.container_changed(pos);
        }
    }

    /// The screens of everyone looking into the container at `pos`.
    pub(super) fn refresh_viewers(&mut self, pos: BlockPos) {
        let viewers: Vec<i32> = self
            .players
            .values()
            .filter(|p| p.menu.as_ref().is_some_and(|m| m.containers.contains(&pos)))
            .map(|p| p.entity_id)
            .collect();
        for viewer in viewers {
            self.send_menu(viewer);
        }
    }

    /// `broadcastChanges` for the data slots of open furnace screens.
    fn send_furnace_data(&mut self) {
        let world = Arc::clone(&self.context.world);
        for player in self.players.values_mut() {
            let Some(menu) = player.menu.as_mut() else {
                continue;
            };
            let Some(pos) = menu.furnace else {
                continue;
            };
            let Some(entity) = world.block_entity(pos) else {
                continue;
            };
            let now = match &entity.brewing {
                Some(b) => super::brewing::screen_data(b),
                None => screen_data(&entity),
            };
            for (index, value) in now.iter().enumerate() {
                if menu.sent_data.get(index) == Some(value) {
                    continue;
                }
                let packet = Encoded::new(&ContainerSetData {
                    container_id: menu.id,
                    id: index as i16,
                    value: *value as i16,
                });
                let _ = player.outbound.send(Outbound::Packet(Arc::new(packet)));
            }
            menu.sent_data = now.to_vec();
        }
    }

    /// `awardUsedRecipesAndPopExperience`: the experience of everything
    /// cooked since the last time, as orbs at `at`.
    pub(super) fn furnace_experience(&mut self, pos: BlockPos, at: [f64; 3]) {
        let data = Arc::clone(&self.context.data);
        let mut used = Vec::new();
        self.context.world.change_block_entity(pos, |entity| {
            if let Some(furnace) = &mut entity.furnace {
                used = std::mem::take(&mut furnace.recipes_used);
            }
            !used.is_empty()
        });
        let mut rng = rand::rng();
        let mut total = 0;
        for (name, count) in used {
            let Some(recipe) = data.recipes.cooking_named(&name) else {
                continue;
            };
            // `createExperience`: the fraction is a chance of one more.
            let value = count as f32 * recipe.experience;
            let mut xp = value.floor() as i32;
            if rng.random::<f32>() < value.fract() {
                xp += 1;
            }
            total += xp;
        }
        if total > 0 {
            self.award_experience(at, total);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coal_cooks_eight_items_and_lights_the_furnace() {
        let data = GameData::load().unwrap();
        let id = |name: &str| data.item_id(name).unwrap();
        let stack = |name, count| {
            Some(ItemStack {
                item: id(name),
                count,
                patch: Default::default(),
            })
        };
        let mut furnace = BlockEntity::new("minecraft:furnace");
        furnace.items[0] = stack("minecraft:raw_iron", 9);
        furnace.items[1] = stack("minecraft:coal", 1);
        let first = tick(&mut furnace, Cooker::Furnace, &data);
        assert_eq!(first.lit, Some(true));
        assert!(furnace.items[1].is_none(), "the coal is burning");
        let mut ticks = 1;
        while furnace.furnace.as_ref().unwrap().lit_remaining > 0 {
            tick(&mut furnace, Cooker::Furnace, &data);
            ticks += 1;
        }
        // Coal burns 1600 ticks after the one it caught fire in, enough for
        // 8 items of 200 ticks each.
        assert_eq!(ticks, 1601);
        assert_eq!(furnace.items[2], stack("minecraft:iron_ingot", 8));
        assert_eq!(furnace.items[0], stack("minecraft:raw_iron", 1));
        let state = furnace.furnace.as_ref().unwrap();
        assert_eq!(
            state.recipes_used,
            [("minecraft:iron_ingot_from_smelting_raw_iron".to_owned(), 8)]
        );
        // Out of fuel it goes out and cools down.
        let out = tick(&mut furnace, Cooker::Furnace, &data);
        assert_eq!(out.lit, None, "already out");
    }

    #[test]
    fn smokers_cook_twice_as_fast_on_half_the_fuel() {
        let data = GameData::load().unwrap();
        let id = |name: &str| data.item_id(name).unwrap();
        let mut smoker = BlockEntity::new("minecraft:smoker");
        smoker.set_item(
            0,
            Some(ItemStack {
                item: id("minecraft:beef"),
                count: 2,
                patch: Default::default(),
            }),
            &data,
        );
        smoker.items[1] = Some(ItemStack {
            item: id("minecraft:lava_bucket"),
            count: 1,
            patch: Default::default(),
        });
        tick(&mut smoker, Cooker::Smoker, &data);
        let state = smoker.furnace.clone().unwrap();
        assert_eq!((state.lit_total, state.speed), (10000, 2.0));
        assert_eq!(state.cooking_total, 100);
        assert_eq!(
            smoker.items[1].as_ref().map(|s| s.item),
            Some(id("minecraft:bucket")),
            "lava leaves its bucket"
        );
        for _ in 0..200 {
            tick(&mut smoker, Cooker::Smoker, &data);
        }
        assert_eq!(smoker.items[2].as_ref().map(|s| s.count), Some(2));
    }
}
