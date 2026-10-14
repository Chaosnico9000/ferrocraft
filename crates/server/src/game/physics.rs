//! Moving boxes through the world without passing through blocks, like
//! vanilla's `Entity.collide`: the movement is cut short on each axis in
//! turn, y first.

use ferrocraft_protocol::codec::BlockPos;

use crate::data::{Aabb, GameData};
use crate::world::World;

/// The result of a move.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Moved {
    /// How far the box actually went.
    pub movement: [f64; 3],
    /// Stopped by something below.
    pub on_ground: bool,
    /// Stopped on x or z.
    pub horizontal_collision: bool,
}

fn offset(aabb: Aabb, by: [f64; 3]) -> Aabb {
    [
        aabb[0] + by[0],
        aabb[1] + by[1],
        aabb[2] + by[2],
        aabb[3] + by[0],
        aabb[4] + by[1],
        aabb[5] + by[2],
    ]
}

/// The collision boxes of all blocks the box could touch on its way.
fn obstacles(world: &World, data: &GameData, bounds: Aabb, movement: [f64; 3]) -> Vec<Aabb> {
    let moved = offset(bounds, movement);
    let lo = |axis: usize| (bounds[axis].min(moved[axis]) - 1.0).floor() as i32;
    let hi = |axis: usize| (bounds[axis + 3].max(moved[axis + 3]) + 1.0).floor() as i32;
    let mut boxes = Vec::new();
    for x in lo(0)..=hi(0) {
        for y in lo(1)..=hi(1) {
            for z in lo(2)..=hi(2) {
                let state = world.block(BlockPos { x, y, z });
                for shape in data.collision_shape(state) {
                    boxes.push(offset(*shape, [f64::from(x), f64::from(y), f64::from(z)]));
                }
            }
        }
    }
    boxes
}

/// `Shapes.collide` on one axis: how far `bounds` can go before it hits one
/// of `boxes`.
fn clip(boxes: &[Aabb], bounds: Aabb, axis: usize, mut distance: f64) -> f64 {
    const EPSILON: f64 = 1.0e-7;
    let (a, b) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    for block in boxes {
        let overlaps =
            |i: usize| block[i] < bounds[i + 3] - EPSILON && block[i + 3] > bounds[i] + EPSILON;
        if !(overlaps(a) && overlaps(b)) {
            continue;
        }
        if distance > 0.0 && block[axis] >= bounds[axis + 3] - EPSILON {
            distance = distance.min(block[axis] - bounds[axis + 3]);
        } else if distance < 0.0 && block[axis + 3] <= bounds[axis] + EPSILON {
            distance = distance.max(block[axis + 3] - bounds[axis]);
        }
    }
    distance
}

/// Moves a box as far as the blocks let it.
pub fn collide(world: &World, data: &GameData, bounds: Aabb, movement: [f64; 3]) -> Moved {
    let boxes = obstacles(world, data, bounds, movement);
    // y first, then the larger of x and z (`Direction.axisStepOrder`).
    let order = if movement[0].abs() < movement[2].abs() {
        [1, 2, 0]
    } else {
        [1, 0, 2]
    };
    let mut current = bounds;
    let mut actual = [0.0; 3];
    for axis in order {
        if movement[axis] == 0.0 {
            continue;
        }
        let distance = clip(&boxes, current, axis, movement[axis]);
        actual[axis] = distance;
        let mut step = [0.0; 3];
        step[axis] = distance;
        current = offset(current, step);
    }
    Moved {
        movement: actual,
        on_ground: movement[1] < 0.0 && actual[1] != movement[1],
        horizontal_collision: actual[0] != movement[0] || actual[2] != movement[2],
    }
}

/// How slippery a block is (`Block.getFriction`).
pub fn friction(data: &GameData, state: u32) -> f64 {
    match data.block_of(state).map(|(_, b)| b.name.as_str()) {
        Some("minecraft:ice" | "minecraft:packed_ice" | "minecraft:frosted_ice") => 0.98,
        Some("minecraft:blue_ice") => 0.989,
        Some("minecraft:slime_block") => 0.8,
        _ => 0.6,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn boxes_land_on_the_ground_and_stop_at_walls() {
        let data = Arc::new(GameData::load().unwrap());
        let world = World::superflat(Arc::clone(&data)).unwrap();
        // An item 0.25 wide, falling onto the grass whose top is y = -60.
        let item = [0.375, -59.5, 0.375, 0.625, -59.25, 0.625];
        let moved = collide(&world, &data, item, [0.0, -1.0, 0.0]);
        assert!(moved.on_ground);
        assert!((moved.movement[1] + 0.5).abs() < 1e-9);

        let stone = data.default_state("minecraft:stone").unwrap();
        world.set_block(BlockPos { x: 1, y: -60, z: 0 }, stone);
        let item = [0.375, -60.0, 0.375, 0.625, -59.75, 0.625];
        let moved = collide(&world, &data, item, [1.0, 0.0, 0.0]);
        assert!(moved.horizontal_collision);
        assert!((moved.movement[0] - 0.375).abs() < 1e-9);
        assert!(!moved.on_ground);
    }
}
