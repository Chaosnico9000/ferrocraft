//! Recipes from vanilla's data pack: shaped crafting recipes match their
//! pattern anywhere in the grid, also mirrored; shapeless ones any
//! arrangement of their ingredients. Cooking recipes turn one item into
//! another in a furnace, smoker or blast furnace.

use std::collections::HashMap;

use anyhow::Context;
use serde::Deserialize;

/// What can go into one place of a recipe: any of these items.
type Ingredient = Vec<i32>;

#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub item: i32,
    pub count: i32,
}

#[derive(Debug)]
enum Recipe {
    Shaped {
        width: usize,
        height: usize,
        /// Row by row; `None` is an empty place.
        cells: Vec<Option<Ingredient>>,
        output: Output,
    },
    Shapeless {
        ingredients: Vec<Ingredient>,
        output: Output,
    },
}

/// Where a cooking recipe works (`RecipeType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cooker {
    Furnace,
    Smoker,
    BlastFurnace,
    Campfire,
}

impl Cooker {
    /// The cooker a furnace-like block entity type is.
    pub fn of(block_entity: &str) -> Option<Self> {
        match block_entity {
            "minecraft:furnace" => Some(Self::Furnace),
            "minecraft:smoker" => Some(Self::Smoker),
            "minecraft:blast_furnace" => Some(Self::BlastFurnace),
            _ => None,
        }
    }

    /// Smokers and blast furnaces burn fuel twice as fast
    /// (`minecraft:block/fast_cooking`).
    pub fn fast(self) -> bool {
        matches!(self, Self::Smoker | Self::BlastFurnace)
    }
}

/// A smelting, smoking, blasting or campfire recipe.
#[derive(Debug, Clone)]
pub struct Cooking {
    /// `minecraft:iron_ingot_from_smelting_iron_ore`.
    pub name: String,
    pub cooker: Cooker,
    pub ingredient: Ingredient,
    pub output: Output,
    pub experience: f32,
    /// Ticks at normal speed.
    pub time: i32,
}

#[derive(Debug, Default)]
pub struct Recipes {
    recipes: Vec<Recipe>,
    cooking: Vec<Cooking>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawIngredient {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
struct RawResult {
    id: String,
    #[serde(default = "one")]
    count: i32,
}

fn one() -> i32 {
    1
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum RawRecipe {
    #[serde(rename = "minecraft:crafting_shaped")]
    Shaped {
        key: HashMap<char, RawIngredient>,
        pattern: Vec<String>,
        result: RawResult,
    },
    #[serde(rename = "minecraft:crafting_shapeless")]
    Shapeless {
        ingredients: Vec<RawIngredient>,
        result: RawResult,
    },
    #[serde(rename = "minecraft:smelting")]
    Smelting(RawCooking),
    #[serde(rename = "minecraft:smoking")]
    Smoking(RawCooking),
    #[serde(rename = "minecraft:blasting")]
    Blasting(RawCooking),
    #[serde(rename = "minecraft:campfire_cooking")]
    Campfire(RawCooking),
}

#[derive(Deserialize)]
struct RawCooking {
    ingredient: RawIngredient,
    result: RawResult,
    #[serde(default)]
    experience: f32,
    /// `AbstractCookingRecipe.Serializer`'s default.
    #[serde(default = "two_hundred")]
    cookingtime: i32,
}

fn two_hundred() -> i32 {
    200
}

impl Recipes {
    /// Parses `recipes.json`. `item` resolves an item name, `tag` an item tag
    /// (without `#`) to item ids. Recipes naming unknown items are skipped.
    pub fn load(
        json: &str,
        item: impl Fn(&str) -> Option<i32>,
        tag: impl Fn(&str) -> Option<Vec<i32>>,
    ) -> anyhow::Result<Self> {
        let raw: HashMap<String, RawRecipe> =
            serde_json::from_str(json).context("parsing recipes.json")?;
        let ingredient = |raw: &RawIngredient| -> Option<Ingredient> {
            let names = match raw {
                RawIngredient::One(name) => vec![name.as_str()],
                RawIngredient::Many(names) => names.iter().map(String::as_str).collect(),
            };
            let mut items = Vec::new();
            for name in names {
                match name.strip_prefix('#') {
                    Some(tag_name) => items.extend(tag(tag_name)?),
                    None => items.push(item(name)?),
                }
            }
            Some(items)
        };
        let output = |result: &RawResult| {
            Some(Output {
                item: item(&result.id)?,
                count: result.count,
            })
        };
        let mut recipes = Vec::new();
        let mut cooking = Vec::new();
        let mut names: Vec<_> = raw.into_iter().collect();
        // A stable order, so that the same grid always gives the same result.
        names.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, recipe) in names {
            let cook = |cooker, raw: RawCooking| {
                Some(Cooking {
                    name: name.clone(),
                    cooker,
                    ingredient: ingredient(&raw.ingredient)?,
                    output: output(&raw.result)?,
                    experience: raw.experience,
                    time: raw.cookingtime,
                })
            };
            let parsed = match recipe {
                RawRecipe::Smelting(raw) => {
                    cooking.extend(cook(Cooker::Furnace, raw));
                    continue;
                }
                RawRecipe::Smoking(raw) => {
                    cooking.extend(cook(Cooker::Smoker, raw));
                    continue;
                }
                RawRecipe::Blasting(raw) => {
                    cooking.extend(cook(Cooker::BlastFurnace, raw));
                    continue;
                }
                RawRecipe::Campfire(raw) => {
                    cooking.extend(cook(Cooker::Campfire, raw));
                    continue;
                }
                RawRecipe::Shaped {
                    key,
                    pattern,
                    result,
                } => (|| {
                    let height = pattern.len();
                    let width = pattern.iter().map(|row| row.chars().count()).max()?;
                    let mut cells = Vec::new();
                    for row in &pattern {
                        let chars: Vec<char> = row.chars().collect();
                        for x in 0..width {
                            cells.push(match chars.get(x) {
                                None | Some(' ') => None,
                                Some(c) => Some(ingredient(key.get(c)?)?),
                            });
                        }
                    }
                    Some(Recipe::Shaped {
                        width,
                        height,
                        cells,
                        output: output(&result)?,
                    })
                })(),
                RawRecipe::Shapeless {
                    ingredients,
                    result,
                } => (|| {
                    Some(Recipe::Shapeless {
                        ingredients: ingredients.iter().map(ingredient).collect::<Option<_>>()?,
                        output: output(&result)?,
                    })
                })(),
            };
            recipes.extend(parsed);
        }
        Ok(Self { recipes, cooking })
    }

    /// The cooking recipe for an item in a cooker, the first by name.
    pub fn cook(&self, cooker: Cooker, item: i32) -> Option<&Cooking> {
        self.cooking
            .iter()
            .find(|r| r.cooker == cooker && r.ingredient.contains(&item))
    }

    /// A cooking recipe by name, for the experience a furnace remembers.
    pub fn cooking_named(&self, name: &str) -> Option<&Cooking> {
        self.cooking.iter().find(|r| r.name == name)
    }

    /// What a square grid of `size`×`size` items (row by row) crafts.
    pub fn craft(&self, size: usize, grid: &[Option<i32>]) -> Option<Output> {
        let filled: Vec<(usize, usize)> = (0..grid.len())
            .filter(|&i| grid[i].is_some())
            .map(|i| (i % size, i / size))
            .collect();
        if filled.is_empty() {
            return None;
        }
        let (min_x, max_x) = (
            filled.iter().map(|p| p.0).min()?,
            filled.iter().map(|p| p.0).max()?,
        );
        let (min_y, max_y) = (
            filled.iter().map(|p| p.1).min()?,
            filled.iter().map(|p| p.1).max()?,
        );
        let (w, h) = (max_x - min_x + 1, max_y - min_y + 1);
        let at = |x: usize, y: usize| grid[(min_y + y) * size + min_x + x];
        let items: Vec<i32> = grid.iter().flatten().copied().collect();
        self.recipes.iter().find_map(|recipe| match recipe {
            Recipe::Shaped {
                width,
                height,
                cells,
                output,
            } => {
                if (*width, *height) != (w, h) {
                    return None;
                }
                let fits = |mirrored: bool| {
                    (0..h).all(|y| {
                        (0..w).all(|x| {
                            let cell = &cells[y * w + if mirrored { w - 1 - x } else { x }];
                            match (cell, at(x, y)) {
                                (None, None) => true,
                                (Some(options), Some(item)) => options.contains(&item),
                                _ => false,
                            }
                        })
                    })
                };
                (fits(false) || fits(true)).then(|| output.clone())
            }
            Recipe::Shapeless {
                ingredients,
                output,
            } => (ingredients.len() == items.len()
                && assign(ingredients, &items, &mut vec![false; items.len()]))
            .then(|| output.clone()),
        })
    }
}

/// Whether every ingredient gets an item of its own.
fn assign(ingredients: &[Ingredient], items: &[i32], used: &mut Vec<bool>) -> bool {
    let Some((first, rest)) = ingredients.split_first() else {
        return true;
    };
    for (i, item) in items.iter().enumerate() {
        if !used[i] && first.contains(item) {
            used[i] = true;
            if assign(rest, items, used) {
                return true;
            }
            used[i] = false;
        }
    }
    false
}

/// What stays in the grid after crafting with an item (`Item.getCraftingRemainder`).
pub fn remainder(item: &str) -> Option<&'static str> {
    match item {
        "minecraft:water_bucket"
        | "minecraft:lava_bucket"
        | "minecraft:milk_bucket"
        | "minecraft:powder_snow_bucket" => Some("minecraft:bucket"),
        "minecraft:honey_bottle" | "minecraft:dragon_breath" => Some("minecraft:glass_bottle"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::data::GameData;

    #[test]
    fn vanilla_recipes_craft() {
        let data = GameData::load().unwrap();
        let id = |name: &str| data.item_id(name).unwrap();
        let recipes = &data.recipes;

        // A log anywhere in the 2×2 grid gives four planks.
        let mut grid = [None; 4];
        grid[3] = Some(id("minecraft:oak_log"));
        let planks = recipes.craft(2, &grid).unwrap();
        assert_eq!((planks.item, planks.count), (id("minecraft:oak_planks"), 4));

        // Four planks of any wood make a crafting table.
        let grid = [
            Some(id("minecraft:oak_planks")),
            Some(id("minecraft:birch_planks")),
            Some(id("minecraft:oak_planks")),
            Some(id("minecraft:spruce_planks")),
        ];
        assert_eq!(
            recipes.craft(2, &grid).unwrap().item,
            id("minecraft:crafting_table")
        );

        // A torch, in the right column of a 3×3 grid.
        let mut grid = [None; 9];
        grid[2] = Some(id("minecraft:coal"));
        grid[5] = Some(id("minecraft:stick"));
        let torches = recipes.craft(3, &grid).unwrap();
        assert_eq!((torches.item, torches.count), (id("minecraft:torch"), 4));

        // A wooden pickaxe; its mirror image is the same.
        let p = Some(id("minecraft:oak_planks"));
        let s = Some(id("minecraft:stick"));
        let pickaxe = [p, p, p, None, s, None, None, s, None];
        assert_eq!(
            recipes.craft(3, &pickaxe).unwrap().item,
            id("minecraft:wooden_pickaxe")
        );
        let hoe = [p, p, None, None, s, None, None, s, None];
        let mirrored = [None, p, p, None, s, None, None, s, None];
        let hoe_item = id("minecraft:wooden_hoe");
        assert_eq!(recipes.craft(3, &hoe).unwrap().item, hoe_item);
        assert_eq!(recipes.craft(3, &mirrored).map(|o| o.item), Some(hoe_item));

        assert_eq!(recipes.craft(3, &[None; 9]), None);
        assert_eq!(recipes.craft(2, &[s, None, None, s]), None);
    }

    #[test]
    fn cooking_recipes_by_cooker() {
        use super::Cooker;
        let data = GameData::load().unwrap();
        let id = |name: &str| data.item_id(name).unwrap();
        let recipes = &data.recipes;
        let iron = recipes
            .cook(Cooker::Furnace, id("minecraft:raw_iron"))
            .unwrap();
        assert_eq!(iron.output.item, id("minecraft:iron_ingot"));
        assert_eq!((iron.time, iron.experience), (200, 0.7));
        // Smokers only cook food, blast furnaces only ores and metal.
        assert!(
            recipes
                .cook(Cooker::Smoker, id("minecraft:raw_iron"))
                .is_none()
        );
        assert!(
            recipes
                .cook(Cooker::BlastFurnace, id("minecraft:raw_iron"))
                .is_some()
        );
        assert!(recipes.cook(Cooker::Smoker, id("minecraft:beef")).is_some());
        assert!(
            recipes
                .cook(Cooker::BlastFurnace, id("minecraft:beef"))
                .is_none()
        );
        // Logs of any kind burn to charcoal, sand melts to glass.
        assert_eq!(
            recipes
                .cook(Cooker::Furnace, id("minecraft:birch_log"))
                .unwrap()
                .output
                .item,
            id("minecraft:charcoal")
        );
        assert!(
            recipes
                .cook(Cooker::Furnace, id("minecraft:sand"))
                .is_some()
        );
        let coal = data.item(id("minecraft:coal")).unwrap();
        assert_eq!(coal.fuel, Some([(1600, 1.0), (800, 2.0)]));
        assert!(data.item(id("minecraft:stone")).unwrap().fuel.is_none());
    }
}
