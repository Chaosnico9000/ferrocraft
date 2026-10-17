//! Brewing stands (`BrewingStandBlockEntity`): blaze powder gives twenty
//! brews; a reagent on top turns the three bottles below in twenty seconds,
//! by the brewing recipes of vanilla's data pack (`recipe/brewing`). The
//! block shows which bottle places are filled.

use std::sync::Arc;

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::component::Registries;
use ferrocraft_protocol::packets::play::ItemStack;

use super::Game;
use crate::block_entity::{BlockEntity, Brewing};
use crate::crafting::{self, PotionIngredient};
use crate::data::GameData;

/// `LevelEvent.SOUND_BREWING_STAND_BREW`.
const BREWED: i32 = 1035;

/// The four data slots of the screen: brew time, fuel, and their totals.
pub fn screen_data(brewing: &Brewing) -> [i32; 4] {
    [
        brewing.brew_time,
        brewing.fuel,
        brewing.total_brew_time,
        brewing.total_fuel,
    ]
}

/// The potion of a stack, if it holds one.
pub fn potion_of(data: &GameData, stack: &ItemStack) -> Option<String> {
    match crate::stack::get(data, stack, "minecraft:potion_contents")?.get("potion") {
        Some(Nbt::String(potion)) => Some(potion.clone()),
        _ => None,
    }
}

/// `PotionIngredient.test`.
fn matches(data: &GameData, ingredient: &PotionIngredient, stack: &ItemStack) -> bool {
    if !ingredient.items.contains(&stack.item) {
        return false;
    }
    let Some(potions) = &ingredient.potions else {
        return true;
    };
    let Some(potion) = potion_of(data, stack) else {
        return false;
    };
    potions.iter().any(|p| match p.strip_prefix('#') {
        Some(tag) => Registries::id(data, "minecraft:potion", &potion)
            .is_some_and(|id| data.tag_contains("minecraft:potion", tag, id)),
        None => *p == potion,
    })
}

/// The brewing recipe a bottle and a reagent make.
pub fn recipe<'a>(
    data: &'a GameData,
    input: &ItemStack,
    reagent: &ItemStack,
) -> Option<&'a crafting::Brewing> {
    data.recipes
        .brewing
        .iter()
        .find(|r| matches(data, &r.input, input) && matches(data, &r.reagent, reagent))
}

/// `RecipePropertySet.BREWING_REAGENTS`: what goes on top.
pub fn is_reagent(data: &GameData, stack: &ItemStack) -> bool {
    data.recipes
        .brewing
        .iter()
        .any(|r| r.reagent.items.contains(&stack.item))
}

/// `PotionIngredient.isPotionInput`: what goes in the bottle places.
pub fn is_potion_input(data: &GameData, stack: &ItemStack) -> bool {
    data.recipes
        .brewing
        .iter()
        .any(|r| r.input.items.contains(&stack.item))
        || data
            .item_tag("minecraft:brewing_potion_inputs")
            .is_some_and(|items| items.contains(&stack.item))
}

/// `FuelSlot.mayPlaceItem`.
pub fn is_fuel(data: &GameData, stack: &ItemStack) -> bool {
    data.item(stack.item)
        .is_some_and(|i| i.brewing_fuel.is_some())
}

/// `ItemStackTemplate.create` for a recipe's output.
fn output(data: &GameData, recipe: &crafting::Brewing) -> ItemStack {
    let mut stack = crate::stack::plain(recipe.output, 1);
    for (name, value) in &recipe.components {
        let value = crate::stack::json_to_nbt(value);
        crate::stack::set(data, &mut stack, name, &value);
    }
    stack
}

/// What one tick does (`serverTick`), apart from the world.
#[derive(Debug, Default)]
pub struct Outcome {
    pub changed: bool,
    /// The bottles to show, when they changed.
    pub bottles: Option<[bool; 3]>,
    pub brewed: bool,
    /// What was left of the reagent or fuel and did not fit.
    pub drops: Vec<ItemStack>,
}

/// `BrewingStandBlockEntity.serverTick`.
pub fn tick(entity: &mut BlockEntity, data: &GameData) -> Outcome {
    let mut outcome = Outcome::default();
    let mut brewing = entity.brewing.unwrap_or_default();
    // Fuel catches when the last is used up.
    if brewing.fuel <= 0
        && let Some(fuel) = entity.items[4].clone()
        && let Some((uses, speed)) = data.item(fuel.item).and_then(|i| i.brewing_fuel)
    {
        brewing.fuel = uses;
        brewing.total_fuel = uses;
        brewing.speed = speed;
        let left = fuel.count - 1;
        let remainder = data
            .item(fuel.item)
            .and_then(|i| crafting::remainder(&i.name))
            .and_then(|r| data.item_id(r));
        entity.items[4] = if left > 0 {
            if let Some(r) = remainder {
                outcome.drops.push(crate::stack::plain(r, 1));
            }
            Some(ItemStack {
                count: left,
                ..fuel
            })
        } else {
            remainder.map(|r| crate::stack::plain(r, 1))
        };
        outcome.changed = true;
    }
    let brewable = brewable(entity, data);
    let reagent = entity.items[3].as_ref().map(|s| s.item);
    if brewing.brew_time > 0 {
        brewing.brew_time -= 1;
        if brewing.brew_time == 0 && brewable {
            brew(entity, data, &mut outcome);
        } else if !brewable || reagent != brewing.ingredient {
            brewing.brew_time = 0;
        }
        outcome.changed = true;
    } else if brewable && brewing.fuel > 0 {
        let speed = if brewing.speed > 0.0 {
            brewing.speed
        } else {
            1.0
        };
        brewing.fuel -= 1;
        brewing.brew_time = (400.0 / speed).ceil() as i32;
        brewing.total_brew_time = brewing.brew_time;
        brewing.ingredient = reagent;
        outcome.changed = true;
    }
    let bottles = [0, 1, 2].map(|i| entity.items[i].is_some());
    if brewing.bottles != Some(bottles) {
        brewing.bottles = Some(bottles);
        outcome.bottles = Some(bottles);
    }
    if outcome.changed {
        entity.modified = true;
    }
    entity.brewing = Some(brewing);
    outcome
}

/// `isBrewable`: a reagent, and a bottle it changes.
fn brewable(entity: &BlockEntity, data: &GameData) -> bool {
    let Some(reagent) = &entity.items[3] else {
        return false;
    };
    is_reagent(data, reagent)
        && entity.items[..3]
            .iter()
            .flatten()
            .any(|bottle| recipe(data, bottle, reagent).is_some())
}

/// `doBrew`: every bottle the reagent changes, then one reagent less.
fn brew(entity: &mut BlockEntity, data: &GameData, outcome: &mut Outcome) {
    let Some(reagent) = entity.items[3].clone() else {
        return;
    };
    for slot in 0..3 {
        if let Some(bottle) = &entity.items[slot]
            && let Some(recipe) = recipe(data, bottle, &reagent)
        {
            entity.items[slot] = Some(output(data, recipe));
        }
    }
    let remainder = data
        .item(reagent.item)
        .and_then(|i| crafting::remainder(&i.name))
        .and_then(|r| data.item_id(r));
    let left = reagent.count - 1;
    entity.items[3] = if left > 0 {
        if let Some(r) = remainder {
            outcome.drops.push(crate::stack::plain(r, 1));
        }
        Some(ItemStack {
            count: left,
            ..reagent
        })
    } else {
        remainder.map(|r| crate::stack::plain(r, 1))
    };
    outcome.brewed = true;
}

impl Game {
    pub(super) fn tick_brewing(&mut self, pos: BlockPos) {
        let data = Arc::clone(&self.context.data);
        let world = Arc::clone(&self.context.world);
        let mut outcome = Outcome::default();
        world.change_block_entity(pos, |entity| {
            outcome = tick(entity, &data);
            outcome.changed
        });
        if let Some(bottles) = outcome.bottles {
            let state = world.block(pos);
            if let Some((_, block)) = data.block_of(state) {
                let mut changed = state;
                for (i, filled) in bottles.iter().enumerate() {
                    if let Some(s) = block.with(
                        changed,
                        &format!("has_bottle_{i}"),
                        if *filled { "true" } else { "false" },
                    ) {
                        changed = s;
                    }
                }
                if changed != state {
                    self.set_block_with(pos, changed, super::blocks::update::CLIENTS);
                }
            }
        }
        for stack in outcome.drops {
            self.drop_at_block(pos, stack);
        }
        if outcome.brewed {
            self.level_event(BREWED, pos, 0);
        }
        if outcome.changed {
            self.refresh_viewers(pos);
            self.container_changed(pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn potion(data: &GameData, item: &str, potion: &str) -> ItemStack {
        let mut stack = crate::stack::plain(data.item_id(item).unwrap(), 1);
        crate::stack::set(
            data,
            &mut stack,
            "minecraft:potion_contents",
            &Nbt::Compound(vec![("potion".into(), Nbt::String(potion.into()))]),
        );
        stack
    }

    #[test]
    fn water_and_wart_brew_awkward_potions() {
        let data = GameData::load().unwrap();
        let id = |name: &str| data.item_id(name).unwrap();
        let mut stand = BlockEntity::new("minecraft:brewing_stand");
        stand.items[0] = Some(potion(&data, "minecraft:potion", "minecraft:water"));
        stand.items[2] = Some(potion(&data, "minecraft:splash_potion", "minecraft:water"));
        stand.items[3] = Some(crate::stack::plain(id("minecraft:nether_wart"), 2));
        stand.items[4] = Some(crate::stack::plain(id("minecraft:blaze_powder"), 1));
        let first = tick(&mut stand, &data);
        assert_eq!(first.bottles, Some([true, false, true]));
        let state = stand.brewing.unwrap();
        assert_eq!((state.fuel, state.brew_time), (19, 400));
        assert!(stand.items[4].is_none(), "the powder burns");
        let mut brewed = false;
        for _ in 0..400 {
            brewed |= tick(&mut stand, &data).brewed;
        }
        assert!(brewed);
        let awkward = |slot: usize| potion_of(&data, stand.items[slot].as_ref().unwrap());
        assert_eq!(awkward(0).as_deref(), Some("minecraft:awkward"));
        assert_eq!(awkward(2).as_deref(), Some("minecraft:awkward"));
        assert_eq!(
            stand.items[2].as_ref().unwrap().item,
            id("minecraft:splash_potion")
        );
        assert_eq!(stand.items[3].as_ref().unwrap().count, 1);
        // Nothing left to do with wart: the brew does not start again.
        tick(&mut stand, &data);
        assert_eq!(stand.brewing.unwrap().brew_time, 0);
        assert!(is_reagent(
            &data,
            &crate::stack::plain(id("minecraft:gunpowder"), 1)
        ));
        assert!(is_potion_input(
            &data,
            &crate::stack::plain(id("minecraft:glass_bottle"), 1)
        ));
    }
}
