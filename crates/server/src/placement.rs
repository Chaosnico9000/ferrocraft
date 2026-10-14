//! Where a click puts a block and which state it gets: a small part of
//! vanilla's `BlockItem.getPlacementState`, enough for the common building
//! blocks. Everything else is placed in its default state.

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::Direction;

use crate::data::Block;

/// Blocks only operators may place (`GameMasterBlockItem`); players here
/// are not operators.
pub const GAME_MASTER_BLOCKS: &[&str] = &[
    "minecraft:command_block",
    "minecraft:chain_command_block",
    "minecraft:repeating_command_block",
    "minecraft:structure_block",
    "minecraft:jigsaw",
    "minecraft:test_block",
    "minecraft:test_instance_block",
];

/// How far a creative player reaches (`block_interaction_range` 4.5 plus
/// creative's 0.5) and the slack vanilla allows on top.
const REACH: f64 = 5.0 + 1.0;

/// `Player.isWithinBlockInteractionRange`: the distance from the eyes to the
/// nearest point of the block.
pub fn within_reach(eyes: [f64; 3], pos: BlockPos) -> bool {
    let distance_sq: f64 = [pos.x, pos.y, pos.z]
        .iter()
        .zip(eyes)
        .map(|(&lo, eye)| {
            let lo = f64::from(lo);
            let nearest = eye.clamp(lo, lo + 1.0);
            (eye - nearest) * (eye - nearest)
        })
        .sum();
    distance_sq < REACH * REACH
}

/// The horizontal direction a player with this yaw looks in
/// (`Direction.fromYRot`).
pub fn looking(yaw: f32) -> Direction {
    match ((f64::from(yaw) / 90.0 + 0.5).floor() as i64).rem_euclid(4) {
        0 => Direction::South,
        1 => Direction::West,
        2 => Direction::North,
        _ => Direction::East,
    }
}

pub fn opposite(direction: Direction) -> Direction {
    match direction {
        Direction::Down => Direction::Up,
        Direction::Up => Direction::Down,
        Direction::North => Direction::South,
        Direction::South => Direction::North,
        Direction::West => Direction::East,
        Direction::East => Direction::West,
    }
}

/// `Direction.getClockWise` around the y axis.
pub fn clockwise(direction: Direction) -> Direction {
    match direction {
        Direction::North => Direction::East,
        Direction::East => Direction::South,
        Direction::South => Direction::West,
        Direction::West => Direction::North,
        vertical => vertical,
    }
}

pub fn counter_clockwise(direction: Direction) -> Direction {
    opposite(clockwise(direction))
}

fn horizontal(direction: Direction) -> bool {
    !matches!(direction, Direction::Up | Direction::Down)
}

/// `ChestBlock.getConnectedDirection`: where the other half of a double
/// chest is; `None` for a single chest.
pub fn chest_partner(block: &Block, state: u32) -> Option<Direction> {
    let facing = direction(value(block, state, "facing")?)?;
    match value(block, state, "type")? {
        "left" => Some(clockwise(facing)),
        "right" => Some(counter_clockwise(facing)),
        _ => None,
    }
}

/// `ChestBlock.getStateForPlacement`: a chest joins a single chest of the
/// same kind and facing next to it, or with sneaking the clicked one. The
/// new state and the partner's changed state.
pub fn connect_chest(
    block: &Block,
    state: u32,
    pos: BlockPos,
    face: Direction,
    sneaking: bool,
    block_at: impl Fn(BlockPos) -> u32,
) -> (u32, Option<(BlockPos, u32)>) {
    // `candidatePartnerFacing`: the facing of a single chest of this kind.
    let candidate = |towards: Direction| {
        let neighbour = block_at(towards.offset(pos));
        (neighbour >= block.first
            && neighbour < block.first + block.state_count()
            && value(block, neighbour, "type") == Some("single"))
        .then(|| value(block, neighbour, "facing").and_then(direction))
        .flatten()
    };
    let Some(mut facing) = value(block, state, "facing").and_then(direction) else {
        return (state, None);
    };
    let mut kind = "single";
    if horizontal(face) && sneaking {
        let clicked = opposite(face);
        if let Some(neighbour) = candidate(clicked)
            && horizontal(neighbour)
            && neighbour != face
            && neighbour != clicked
        {
            facing = neighbour;
            kind = if counter_clockwise(neighbour) == clicked {
                "right"
            } else {
                "left"
            };
        }
    }
    if kind == "single" && !sneaking {
        if candidate(clockwise(facing)) == Some(facing) {
            kind = "left";
        } else if candidate(counter_clockwise(facing)) == Some(facing) {
            kind = "right";
        }
    }
    let state = block
        .with(state, "facing", facing.name())
        .and_then(|s| block.with(s, "type", kind))
        .unwrap_or(state);
    // `updateShape` of the partner: it takes the opposite side.
    let partner = chest_partner(block, state).and_then(|towards| {
        let at = towards.offset(pos);
        let other = if kind == "left" { "right" } else { "left" };
        Some((at, block.with(block_at(at), "type", other)?))
    });
    (state, partner)
}

/// Blocks whose `facing` points the way the player looks; other blocks with
/// a horizontal `facing`, like furnaces and chests, face the player.
const FACING_AWAY: &[&str] = &[
    "_stairs",
    "_bed",
    "_door",
    "_fence_gate",
    "campfire",
    "bell",
    "decorated_pot",
];

pub fn direction(name: &str) -> Option<Direction> {
    Some(match name {
        "down" => Direction::Down,
        "up" => Direction::Up,
        "north" => Direction::North,
        "south" => Direction::South,
        "west" => Direction::West,
        "east" => Direction::East,
        _ => return None,
    })
}

fn value<'a>(block: &'a Block, state: u32, key: &str) -> Option<&'a str> {
    block
        .values(state)
        .into_iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

/// Whether `half` means the two blocks of a door or tall plant.
fn is_tall(block: &Block) -> bool {
    block
        .properties
        .iter()
        .any(|(k, v)| k == "half" && v.iter().any(|v| v == "upper"))
}

/// The state a block gets when placed against `face` of another block, with
/// the cursor at `cursor_y` (0 to 1) on that face, by a player with `yaw`.
pub fn state_for(block: &Block, face: Direction, cursor_y: f32, yaw: f32) -> u32 {
    let mut state = block.default;
    let mut set = |key: &str, value: &str| {
        if let Some(changed) = block.with(state, key, value) {
            state = changed;
        }
    };
    // Logs and pillars point away from the clicked face.
    if block.has("axis") {
        set(
            "axis",
            match face {
                Direction::Down | Direction::Up => "y",
                Direction::North | Direction::South => "z",
                Direction::West | Direction::East => "x",
            },
        );
    }
    let looking = looking(yaw);
    let horizontal = block
        .properties
        .iter()
        .any(|(k, v)| k == "facing" && v.len() == 4);
    if block.has("face") {
        // Buttons and levers: on the floor, the ceiling or a wall.
        let (attached, facing) = match face {
            Direction::Up => ("floor", looking),
            Direction::Down => ("ceiling", looking),
            side => ("wall", side),
        };
        set("face", attached);
        set("facing", facing.name());
    } else if horizontal {
        let away = FACING_AWAY.iter().any(|s| block.name.ends_with(s));
        set(
            "facing",
            if away { looking } else { opposite(looking) }.name(),
        );
    }
    // Stairs, slabs and trapdoors go to the top half when clicked from below
    // or on the upper half of a side.
    let top = face == Direction::Down || (face != Direction::Up && cursor_y > 0.5);
    let half = if top { "top" } else { "bottom" };
    set("half", half);
    if block.properties.iter().any(|(k, _)| k == "type") {
        set("type", half);
    }
    state
}

/// The blocks a placement sets: the block itself and, for doors, tall
/// plants and beds, its other half (`setPlacedBy`).
pub fn placements(
    block: &Block,
    target: BlockPos,
    face: Direction,
    cursor_y: f32,
    yaw: f32,
) -> Vec<(BlockPos, u32)> {
    let state = state_for(block, face, cursor_y, yaw);
    let mut blocks = vec![(target, state)];
    if is_tall(block)
        && let Some(upper) = block.with(state, "half", "upper")
    {
        blocks.push((Direction::Up.offset(target), upper));
    }
    if let (Some(head), Some(facing)) = (
        block.with(state, "part", "head"),
        value(block, state, "facing").and_then(direction),
    ) {
        blocks.push((facing.offset(target), head));
    }
    blocks
}

/// The other half of a two-block block at `pos`, which goes with it.
pub fn partner(block: &Block, state: u32, pos: BlockPos) -> Option<BlockPos> {
    if is_tall(block) {
        return Some(match value(block, state, "half")? {
            "upper" => Direction::Down.offset(pos),
            _ => Direction::Up.offset(pos),
        });
    }
    let facing = direction(value(block, state, "facing")?)?;
    match value(block, state, "part")? {
        "foot" => Some(facing.offset(pos)),
        _ => Some(opposite(facing).offset(pos)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::GameData;

    fn block<'a>(data: &'a GameData, name: &str) -> &'a Block {
        data.block_of(data.default_state(name).unwrap()).unwrap().1
    }

    #[test]
    fn reach_is_measured_to_the_nearest_point() {
        let eyes = [0.5, -58.38, 0.5];
        assert!(within_reach(eyes, BlockPos { x: 5, y: -59, z: 0 }));
        assert!(!within_reach(eyes, BlockPos { x: 7, y: -59, z: 0 }));
        assert!(within_reach(eyes, BlockPos { x: 0, y: -61, z: 0 }));
    }

    #[test]
    fn yaw_to_direction_like_vanilla() {
        assert_eq!(looking(0.0), Direction::South);
        assert_eq!(looking(90.0), Direction::West);
        assert_eq!(looking(-90.0), Direction::East);
        assert_eq!(looking(180.0), Direction::North);
        assert_eq!(looking(-135.0 - 1.0), Direction::North);
        assert_eq!(looking(44.0), Direction::South);
    }

    #[test]
    fn placement_states() {
        let data = GameData::load().unwrap();
        let log = block(&data, "minecraft:oak_log");
        let east = state_for(log, Direction::East, 0.5, 0.0);
        assert_eq!(log.values(east), [("axis", "x")]);

        // Stairs rise the way the player looks, furnaces face the player.
        let stairs = block(&data, "minecraft:oak_stairs");
        let state = state_for(stairs, Direction::Up, 1.0, 180.0);
        assert_eq!(log_value(stairs, state, "facing"), "north");
        assert_eq!(log_value(stairs, state, "half"), "bottom");
        let state = state_for(stairs, Direction::North, 0.75, 180.0);
        assert_eq!(log_value(stairs, state, "half"), "top");
        let furnace = block(&data, "minecraft:furnace");
        let state = state_for(furnace, Direction::Up, 1.0, 180.0);
        assert_eq!(log_value(furnace, state, "facing"), "south");

        let slab = block(&data, "minecraft:oak_slab");
        let state = state_for(slab, Direction::Down, 0.0, 0.0);
        assert_eq!(log_value(slab, state, "type"), "top");

        let stone = block(&data, "minecraft:stone");
        assert_eq!(state_for(stone, Direction::Up, 1.0, 33.0), stone.default);

        let button = block(&data, "minecraft:stone_button");
        let state = state_for(button, Direction::West, 0.5, 0.0);
        assert_eq!(log_value(button, state, "face"), "wall");
        assert_eq!(log_value(button, state, "facing"), "west");
    }

    #[test]
    fn chests_join_their_neighbours() {
        let data = GameData::load().unwrap();
        let chest = block(&data, "minecraft:chest");
        let at = BlockPos { x: 0, y: -60, z: 0 };
        // A single chest facing north at x = 1; a chest placed west of it,
        // also facing north, becomes its left half... seen from the front,
        // the right one.
        let north = state_for(chest, Direction::Up, 1.0, 0.0);
        assert_eq!(log_value(chest, north, "facing"), "north");
        let east = BlockPos { x: 1, ..at };
        let world = |pos: BlockPos| if pos == east { north } else { 0 };
        let (state, partner) = connect_chest(chest, north, at, Direction::Up, false, world);
        assert_eq!(log_value(chest, state, "type"), "left");
        let (partner_pos, partner_state) = partner.unwrap();
        assert_eq!(partner_pos, east);
        assert_eq!(log_value(chest, partner_state, "type"), "right");
        assert_eq!(chest_partner(chest, state), Some(Direction::East));
        assert_eq!(chest_partner(chest, partner_state), Some(Direction::West));

        // Sneaking places it alone.
        let (state, partner) = connect_chest(chest, north, at, Direction::Up, true, world);
        assert_eq!((log_value(chest, state, "type"), partner), ("single", None));
    }

    #[test]
    fn two_block_blocks_get_both_halves() {
        let data = GameData::load().unwrap();
        let at = BlockPos { x: 0, y: -60, z: 0 };

        let door = block(&data, "minecraft:oak_door");
        let placed = placements(door, at, Direction::Up, 1.0, 0.0);
        assert_eq!(placed.len(), 2);
        assert_eq!(log_value(door, placed[0].1, "half"), "lower");
        assert_eq!(placed[1].0, BlockPos { x: 0, y: -59, z: 0 });
        assert_eq!(log_value(door, placed[1].1, "half"), "upper");
        assert_eq!(partner(door, placed[1].1, placed[1].0), Some(at));
        assert_eq!(partner(door, placed[0].1, at), Some(placed[1].0));

        // Looking south (yaw 0): the head lies one block further south.
        let bed = block(&data, "minecraft:red_bed");
        let placed = placements(bed, at, Direction::Up, 1.0, 0.0);
        assert_eq!(placed[1].0, BlockPos { x: 0, y: -60, z: 1 });
        assert_eq!(log_value(bed, placed[1].1, "part"), "head");
        assert_eq!(partner(bed, placed[1].1, placed[1].0), Some(at));

        let stone = block(&data, "minecraft:stone");
        assert_eq!(placements(stone, at, Direction::Up, 1.0, 0.0).len(), 1);
        assert_eq!(partner(stone, stone.default, at), None);
    }

    fn log_value<'a>(block: &'a Block, state: u32, key: &str) -> &'a str {
        block
            .values(state)
            .into_iter()
            .find(|(k, _)| *k == key)
            .unwrap()
            .1
    }
}
