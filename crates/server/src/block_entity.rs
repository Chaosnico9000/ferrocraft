//! Block entities: what blocks such as chests and signs hold beyond their
//! state. Containers keep their items, signs their text, furnaces their fire
//! and progress. What vanilla stored and Ferrocraft does not model stays as
//! it was loaded.

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{Direction, ItemStack};

use crate::crafting::Cooker;
use crate::data::GameData;
use crate::storage::{item_from_nbt, item_to_nbt, state_from_nbt, state_to_nbt};

/// Slots of the containers Ferrocraft knows, by block entity type.
pub fn container_size(kind: &str) -> usize {
    match kind {
        "minecraft:chest"
        | "minecraft:trapped_chest"
        | "minecraft:barrel"
        | "minecraft:shulker_box" => 27,
        "minecraft:dispenser" | "minecraft:dropper" => 9,
        "minecraft:hopper" | "minecraft:brewing_stand" => 5,
        "minecraft:furnace" | "minecraft:blast_furnace" | "minecraft:smoker" => 3,
        _ => 0,
    }
}

/// One side of a sign as vanilla saves it: `{messages, color,
/// has_glowing_text}`.
fn empty_side() -> Nbt {
    Nbt::Compound(vec![
        (
            "messages".into(),
            Nbt::List(vec![Nbt::String(String::new()); 4]),
        ),
        ("color".into(), Nbt::String("black".into())),
        ("has_glowing_text".into(), Nbt::Byte(0)),
    ])
}

fn is_sign(kind: &str) -> bool {
    matches!(kind, "minecraft:sign" | "minecraft:hanging_sign")
}

/// What a furnace, smoker or blast furnace keeps besides its three slots
/// (`AbstractFurnaceBlockEntity`): the fuel burning, the cooking, and the
/// recipes done since someone last took out the result, for experience.
#[derive(Debug, Clone, PartialEq)]
pub struct Furnace {
    pub lit_remaining: i32,
    pub lit_total: i32,
    pub cooking: i32,
    pub cooking_total: i32,
    pub speed: f32,
    pub recipes_used: Vec<(String, i32)>,
}

impl Default for Furnace {
    fn default() -> Self {
        Self {
            lit_remaining: 0,
            lit_total: 0,
            cooking: 0,
            cooking_total: 0,
            speed: 1.0,
            recipes_used: Vec::new(),
        }
    }
}

/// A block on its way, pushed or pulled by a piston
/// (`PistonMovingBlockEntity`): it sits in the moving piston block where it
/// will end up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Moving {
    /// The block that moves (`movedState`).
    pub state: u32,
    /// Which way the piston faces.
    pub facing: Direction,
    /// 0 to 1, half a block a tick.
    pub progress: f32,
    pub progress_old: f32,
    pub extending: bool,
    /// The piston's own head or base.
    pub source: bool,
    /// The game tick it last moved in.
    pub last_ticked: i64,
}

/// A hopper's wait before it moves the next item (`TransferCooldown`) and
/// the tick it last ran in.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Hopper {
    pub cooldown: i32,
    pub ticked: i64,
}

/// Keys of the block entities Ferrocraft models beyond items, which it
/// writes itself.
const MODELLED_KEYS: [&str; 7] = [
    "blockState",
    "facing",
    "progress",
    "extending",
    "source",
    "OutputSignal",
    "TransferCooldown",
];

/// Keys of a furnace's saved data.
const FURNACE_KEYS: [&str; 6] = [
    "cooking_time_spent",
    "cooking_total_time",
    "lit_time_remaining",
    "lit_total_time",
    "speed_multiplier",
    "RecipesUsed",
];

/// Keys every saved block entity has, which the client is not sent.
const POSITION_KEYS: [&str; 5] = ["id", "x", "y", "z", "keepPacked"];

#[derive(Debug, Clone, PartialEq)]
pub struct BlockEntity {
    /// Its type, e.g. `minecraft:chest`.
    pub kind: String,
    /// The container's slots; empty for other block entities.
    pub items: Vec<Option<ItemStack>>,
    /// Front and back of a sign.
    pub sign: Option<[Nbt; 2]>,
    pub waxed: bool,
    pub furnace: Option<Furnace>,
    /// A moving piston's block.
    pub moving: Option<Moving>,
    /// A comparator's output (`OutputSignal`).
    pub comparator: Option<i32>,
    pub hopper: Option<Hopper>,
    /// As loaded, for everything Ferrocraft does not model.
    stored: Option<Nbt>,
    /// Changed since it was loaded.
    pub modified: bool,
}

impl BlockEntity {
    pub fn new(kind: &str) -> Self {
        Self {
            kind: kind.into(),
            items: vec![None; container_size(kind)],
            sign: is_sign(kind).then(|| [empty_side(), empty_side()]),
            waxed: false,
            furnace: Cooker::of(kind).map(|_| Furnace::default()),
            moving: (kind == "minecraft:piston").then_some(Moving {
                state: 0,
                facing: Direction::Down,
                progress: 0.0,
                progress_old: 0.0,
                extending: false,
                source: false,
                last_ticked: 0,
            }),
            comparator: (kind == "minecraft:comparator").then_some(0),
            hopper: (kind == "minecraft:hopper").then_some(Hopper {
                cooldown: -1,
                ticked: 0,
            }),
            stored: None,
            modified: true,
        }
    }

    /// Reads one entry of a chunk's `block_entities`.
    pub fn from_nbt(nbt: Nbt, data: &GameData) -> Option<(BlockPos, Self)> {
        let int = |key| match nbt.get(key) {
            Some(Nbt::Int(v)) => Some(*v),
            _ => None,
        };
        let pos = BlockPos {
            x: int("x")?,
            y: int("y")?,
            z: int("z")?,
        };
        let Some(Nbt::String(kind)) = nbt.get("id") else {
            return None;
        };
        let mut entity = Self::new(kind);
        if let Some(Nbt::List(items)) = nbt.get("Items") {
            for item in items {
                if let (Some(Nbt::Byte(slot)), Some(stack)) =
                    (item.get("Slot"), item_from_nbt(item, data))
                    && let Some(target) = entity.items.get_mut(*slot as u8 as usize)
                {
                    *target = Some(stack);
                }
            }
        }
        if let Some(sides) = &mut entity.sign {
            for (side, key) in sides.iter_mut().zip(["front_text", "back_text"]) {
                if let Some(text) = nbt.get(key) {
                    *side = text.clone();
                }
            }
            entity.waxed = nbt.get("is_waxed") == Some(&Nbt::Byte(1));
        }
        if let Some(furnace) = &mut entity.furnace {
            let int = |key| match nbt.get(key) {
                Some(Nbt::Int(v)) => *v,
                _ => 0,
            };
            furnace.cooking = int("cooking_time_spent");
            furnace.cooking_total = int("cooking_total_time");
            furnace.lit_remaining = int("lit_time_remaining");
            furnace.lit_total = int("lit_total_time");
            if let Some(Nbt::Float(speed)) = nbt.get("speed_multiplier") {
                furnace.speed = *speed;
            }
            if let Some(Nbt::Compound(used)) = nbt.get("RecipesUsed") {
                furnace.recipes_used = used
                    .iter()
                    .filter_map(|(name, count)| match count {
                        Nbt::Int(count) => Some((name.clone(), *count)),
                        _ => None,
                    })
                    .collect();
            }
        }
        if let Some(moving) = &mut entity.moving {
            if let Some(state) = nbt.get("blockState").and_then(|s| state_from_nbt(s, data)) {
                moving.state = state;
            }
            if let Some(Nbt::Int(facing)) = nbt.get("facing") {
                moving.facing = Direction::from_id(*facing).unwrap_or(Direction::Down);
            }
            if let Some(Nbt::Float(progress)) = nbt.get("progress") {
                moving.progress = *progress;
                moving.progress_old = *progress;
            }
            moving.extending = nbt.get("extending") == Some(&Nbt::Byte(1));
            moving.source = nbt.get("source") == Some(&Nbt::Byte(1));
        }
        if let (Some(output), Some(Nbt::Int(saved))) =
            (&mut entity.comparator, nbt.get("OutputSignal"))
        {
            *output = *saved;
        }
        if let (Some(hopper), Some(Nbt::Int(saved))) =
            (&mut entity.hopper, nbt.get("TransferCooldown"))
        {
            hopper.cooldown = *saved;
        }
        entity.stored = Some(nbt);
        entity.modified = false;
        Some((pos, entity))
    }

    /// The entry for a chunk's `block_entities`.
    pub fn to_nbt(&self, pos: BlockPos, data: &GameData) -> Nbt {
        if let (false, Some(stored)) = (self.modified, &self.stored) {
            return stored.clone();
        }
        let mut entries: Vec<(String, Nbt)> = match &self.stored {
            Some(Nbt::Compound(stored)) => stored
                .iter()
                .filter(|(k, _)| {
                    !matches!(
                        k.as_str(),
                        "Items" | "front_text" | "back_text" | "is_waxed"
                    ) && !POSITION_KEYS.contains(&k.as_str())
                        && !FURNACE_KEYS.contains(&k.as_str())
                        && !MODELLED_KEYS.contains(&k.as_str())
                })
                .cloned()
                .collect(),
            _ => Vec::new(),
        };
        entries.splice(
            0..0,
            [
                ("id".to_owned(), Nbt::String(self.kind.clone())),
                ("x".into(), Nbt::Int(pos.x)),
                ("y".into(), Nbt::Int(pos.y)),
                ("z".into(), Nbt::Int(pos.z)),
                ("keepPacked".into(), Nbt::Byte(0)),
            ],
        );
        if container_size(&self.kind) > 0 {
            let items = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(slot, stack)| {
                    let mut item = item_to_nbt(stack.as_ref()?, data)?;
                    if let Nbt::Compound(entries) = &mut item {
                        entries.insert(0, ("Slot".into(), Nbt::Byte(slot as i8)));
                    }
                    Some(item)
                })
                .collect();
            entries.push(("Items".into(), Nbt::List(items)));
        }
        if let Some(furnace) = &self.furnace {
            entries.push(("cooking_time_spent".into(), Nbt::Int(furnace.cooking)));
            entries.push(("cooking_total_time".into(), Nbt::Int(furnace.cooking_total)));
            entries.push(("lit_time_remaining".into(), Nbt::Int(furnace.lit_remaining)));
            entries.push(("lit_total_time".into(), Nbt::Int(furnace.lit_total)));
            entries.push(("speed_multiplier".into(), Nbt::Float(furnace.speed)));
            entries.push((
                "RecipesUsed".into(),
                Nbt::Compound(
                    furnace
                        .recipes_used
                        .iter()
                        .map(|(name, count)| (name.clone(), Nbt::Int(*count)))
                        .collect(),
                ),
            ));
        }
        if let Some([front, back]) = &self.sign {
            entries.push(("front_text".into(), front.clone()));
            entries.push(("back_text".into(), back.clone()));
            entries.push(("is_waxed".into(), Nbt::Byte(i8::from(self.waxed))));
        }
        if let Some(moving) = &self.moving {
            entries.extend(moving_nbt(moving, data));
        }
        if let Some(output) = self.comparator {
            entries.push(("OutputSignal".into(), Nbt::Int(output)));
        }
        if let Some(hopper) = &self.hopper {
            entries.push(("TransferCooldown".into(), Nbt::Int(hopper.cooldown)));
        }
        Nbt::Compound(entries)
    }

    /// What the client is sent (`BlockEntity.getUpdateTag`): a sign's text,
    /// the saved data of block entities Ferrocraft only keeps, nothing of a
    /// container's items.
    pub fn update_tag(&self, data: &GameData) -> Option<Nbt> {
        if let Some(moving) = &self.moving {
            // `saveCustomOnly`: a chunk sent mid-move shows the block moving.
            return Some(Nbt::Compound(moving_nbt(moving, data)));
        }
        if self.comparator.is_some() || self.hopper.is_some() {
            return None;
        }
        if let Some([front, back]) = &self.sign {
            return Some(Nbt::Compound(vec![
                ("front_text".into(), front.clone()),
                ("back_text".into(), back.clone()),
                ("is_waxed".into(), Nbt::Byte(i8::from(self.waxed))),
            ]));
        }
        match &self.stored {
            Some(Nbt::Compound(stored)) if container_size(&self.kind) == 0 => Some(Nbt::Compound(
                stored
                    .iter()
                    .filter(|(k, _)| !POSITION_KEYS.contains(&k.as_str()))
                    .cloned()
                    .collect(),
            )),
            _ => None,
        }
    }

    /// Puts a stack into a slot (`Container.setItem`). A furnace starts
    /// cooking anew when a different item goes in (`setItem` of
    /// `AbstractFurnaceBlockEntity`).
    pub fn set_item(&mut self, slot: usize, stack: Option<ItemStack>, data: &GameData) {
        let Some(target) = self.items.get_mut(slot) else {
            return;
        };
        let same = match (&stack, &*target) {
            (Some(new), Some(old)) => new.item == old.item && new.patch == old.patch,
            _ => false,
        };
        *target = stack;
        self.modified = true;
        if slot != 0 || same {
            return;
        }
        let item = self.items[0].as_ref().map(|s| s.item);
        if let (Some(furnace), Some(cooker)) = (&mut self.furnace, Cooker::of(&self.kind)) {
            let time = item
                .and_then(|item| data.recipes.cook(cooker, item))
                .map_or(200, |r| r.time);
            furnace.cooking_total = if furnace.speed > 0.0 {
                (time as f32 / furnace.speed).ceil() as i32
            } else {
                time
            };
            furnace.cooking = 0;
        }
    }

    /// Sets a sign side's four lines as plain text.
    pub fn set_sign_text(&mut self, front: bool, lines: &[String; 4]) {
        let Some(sides) = &mut self.sign else {
            return;
        };
        let side = &mut sides[usize::from(!front)];
        if let Nbt::Compound(entries) = side {
            for (key, value) in entries.iter_mut() {
                if key == "messages" {
                    *value = Nbt::List(lines.iter().map(|l| Nbt::String(l.clone())).collect());
                }
            }
        }
        self.modified = true;
    }
}

/// What a moving piston's block entity saves (`saveAdditional`); the
/// progress is the one of the last tick.
fn moving_nbt(moving: &Moving, data: &GameData) -> Vec<(String, Nbt)> {
    vec![
        ("blockState".into(), state_to_nbt(moving.state, data)),
        ("facing".into(), Nbt::Int(moving.facing as i32)),
        ("progress".into(), Nbt::Float(moving.progress_old)),
        ("extending".into(), Nbt::Byte(i8::from(moving.extending))),
        ("source".into(), Nbt::Byte(i8::from(moving.source))),
    ]
}

#[cfg(test)]
mod tests {
    use ferrocraft_protocol::packets::play::ComponentPatch;

    use super::*;

    #[test]
    fn chests_and_signs_survive_nbt() {
        let data = GameData::load().unwrap();
        let pos = BlockPos {
            x: 1,
            y: -60,
            z: -3,
        };
        let mut chest = BlockEntity::new("minecraft:chest");
        assert_eq!(chest.items.len(), 27);
        chest.items[13] = Some(ItemStack {
            item: 1,
            count: 5,
            patch: ComponentPatch::default(),
        });
        let (at, loaded) = BlockEntity::from_nbt(chest.to_nbt(pos, &data), &data).unwrap();
        assert_eq!(at, pos);
        assert_eq!(loaded.items, chest.items);
        assert_eq!(
            loaded.update_tag(&data),
            None,
            "the client does not see the items"
        );

        let mut sign = BlockEntity::new("minecraft:sign");
        sign.set_sign_text(
            true,
            &["Hallo".into(), String::new(), String::new(), "Welt".into()],
        );
        let (_, loaded) = BlockEntity::from_nbt(sign.to_nbt(pos, &data), &data).unwrap();
        assert_eq!(loaded.sign, sign.sign);
        let tag = loaded.update_tag(&data).unwrap();
        assert!(tag.get("front_text").is_some());

        // A furnace keeps its fire and what it has cooked.
        let mut furnace = BlockEntity::new("minecraft:smoker");
        let state = Furnace {
            lit_remaining: 700,
            lit_total: 800,
            cooking: 12,
            cooking_total: 100,
            speed: 2.0,
            recipes_used: vec![("minecraft:cooked_beef_from_smoking".into(), 3)],
        };
        furnace.furnace = Some(state.clone());
        let (_, loaded) = BlockEntity::from_nbt(furnace.to_nbt(pos, &data), &data).unwrap();
        assert_eq!(loaded.furnace, Some(state));
        assert_eq!(loaded.items.len(), 3);

        // A block entity Ferrocraft does not know keeps everything.
        let banner = Nbt::Compound(vec![
            ("id".into(), Nbt::String("minecraft:banner".into())),
            ("x".into(), Nbt::Int(0)),
            ("y".into(), Nbt::Int(0)),
            ("z".into(), Nbt::Int(0)),
            ("patterns".into(), Nbt::List(Vec::new())),
        ]);
        let (_, loaded) = BlockEntity::from_nbt(banner.clone(), &data).unwrap();
        assert_eq!(loaded.to_nbt(BlockPos { x: 0, y: 0, z: 0 }, &data), banner);
        assert_eq!(
            loaded.update_tag(&data),
            Some(Nbt::Compound(vec![(
                "patterns".into(),
                Nbt::List(Vec::new())
            )]))
        );
    }
}
