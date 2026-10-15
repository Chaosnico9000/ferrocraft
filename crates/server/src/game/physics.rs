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
fn clip_axis(boxes: &[Aabb], bounds: Aabb, axis: usize, mut distance: f64) -> f64 {
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
        let distance = clip_axis(&boxes, current, axis, movement[axis]);
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

/// Whether any block's collision shape reaches into the box.
pub fn blocked(world: &World, data: &GameData, bounds: Aabb) -> bool {
    obstacles(world, data, bounds, [0.0; 3])
        .iter()
        .any(|b| (0..3).all(|i| b[i] < bounds[i + 3] - 1.0e-7 && b[i + 3] > bounds[i] + 1.0e-7))
}

/// Where a line first meets a block's collision shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    /// How far along the line, 0 at the start, 1 at the end.
    pub t: f64,
    pub at: [f64; 3],
    pub block: BlockPos,
    /// The axis of the face it came through (0 x, 1 y, 2 z).
    pub axis: usize,
}

/// Where a line from `from` enters a box, as a fraction of `dir`, and
/// through which axis; 0 if it starts inside.
fn ray_box(from: [f64; 3], dir: [f64; 3], b: &Aabb) -> Option<(f64, usize)> {
    let (mut enter, mut exit, mut axis) = (0.0f64, 1.0f64, 0);
    for i in 0..3 {
        if dir[i].abs() < 1.0e-12 {
            if from[i] < b[i] || from[i] > b[i + 3] {
                return None;
            }
            continue;
        }
        let (t1, t2) = ((b[i] - from[i]) / dir[i], (b[i + 3] - from[i]) / dir[i]);
        let (near, far) = if t1 < t2 { (t1, t2) } else { (t2, t1) };
        if near > enter {
            enter = near;
            axis = i;
        }
        exit = exit.min(far);
        if enter > exit {
            return None;
        }
    }
    Some((enter, axis))
}

/// The first block whose collision shape the line from `from` to `to`
/// meets (`Level.clip` with `ClipContext.Block.COLLIDER`), walking the
/// blocks along the line in order.
pub fn clip(world: &World, data: &GameData, from: [f64; 3], to: [f64; 3]) -> Option<Hit> {
    clip_shapes(world, from, to, &|_, state| {
        data.collision_shape(state).to_vec()
    })
}

/// [`clip`] against the shapes `shapes` gives for a block and its state,
/// such as outlines.
pub fn clip_shapes(
    world: &World,
    from: [f64; 3],
    to: [f64; 3],
    shapes: &dyn Fn(BlockPos, u32) -> Vec<Aabb>,
) -> Option<Hit> {
    let dir = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let mut cell = from.map(|v| v.floor() as i32);
    let end = to.map(|v| v.floor() as i32);
    let step = dir.map(|d| if d > 0.0 { 1 } else { -1 });
    let mut next = [0.0; 3];
    let mut delta = [f64::INFINITY; 3];
    for i in 0..3 {
        if dir[i].abs() > 1.0e-12 {
            delta[i] = (1.0 / dir[i]).abs();
            let boundary = if dir[i] > 0.0 {
                f64::from(cell[i]) + 1.0
            } else {
                f64::from(cell[i])
            };
            next[i] = (boundary - from[i]) / dir[i];
        } else {
            next[i] = f64::INFINITY;
        }
    }
    // Long lines are cut off; nothing asks for more than a few chunks.
    for _ in 0..1024 {
        let block = BlockPos {
            x: cell[0],
            y: cell[1],
            z: cell[2],
        };
        let state = world.block(block);
        let origin = [f64::from(cell[0]), f64::from(cell[1]), f64::from(cell[2])];
        let best = shapes(block, state)
            .iter()
            .filter_map(|shape| ray_box(from, dir, &offset(*shape, origin)))
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((t, axis)) = best {
            return Some(Hit {
                t,
                at: [
                    from[0] + dir[0] * t,
                    from[1] + dir[1] * t,
                    from[2] + dir[2] * t,
                ],
                block,
                axis,
            });
        }
        if cell == end {
            return None;
        }
        let axis = if next[0] < next[1] && next[0] < next[2] {
            0
        } else if next[1] < next[2] {
            1
        } else {
            2
        };
        if next[axis] > 1.0 {
            return None;
        }
        cell[axis] += step[axis];
        next[axis] += delta[axis];
    }
    None
}

/// Where a line meets a box, as [`clip`] reports blocks.
pub fn clip_box(from: [f64; 3], to: [f64; 3], b: &Aabb) -> Option<f64> {
    let dir = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    ray_box(from, dir, b).map(|(t, _)| t)
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

    #[test]
    fn lines_stop_at_the_first_block() {
        let data = Arc::new(GameData::load().unwrap());
        let world = World::superflat(Arc::clone(&data)).unwrap();
        // Straight down onto the grass, whose top is y = -60.
        let hit = clip(&world, &data, [0.5, -50.0, 0.5], [0.5, -70.0, 0.5]).unwrap();
        assert_eq!(hit.block, BlockPos { x: 0, y: -61, z: 0 });
        assert!((hit.at[1] + 60.0).abs() < 1e-9 && hit.axis == 1);
        // Over the ground nothing is in the way, until a wall is.
        let from = [0.5, -59.0, 0.5];
        assert!(clip(&world, &data, from, [9.5, -59.0, 3.5]).is_none());
        let stone = data.default_state("minecraft:stone").unwrap();
        world.set_block(BlockPos { x: 5, y: -59, z: 2 }, stone);
        world.set_block(BlockPos { x: 3, y: -59, z: 1 }, stone);
        let hit = clip(&world, &data, from, [9.5, -59.0, 3.5]).unwrap();
        assert_eq!(hit.block, BlockPos { x: 3, y: -59, z: 1 });
        assert!((hit.at[0] - 3.0).abs() < 1e-9 && hit.axis == 0);
        // Backwards too.
        let hit = clip(&world, &data, [9.5, -59.0, 3.5], from).unwrap();
        assert_eq!(hit.block, BlockPos { x: 5, y: -59, z: 2 });
    }
}
