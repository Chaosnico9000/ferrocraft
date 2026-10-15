//! Finding a way through the blocks: A* over the places a mob can stand,
//! like vanilla's `PathFinder` with `WalkNodeEvaluator`. A mob walks, steps
//! up one block, drops down a few and goes diagonally where both sides are
//! free; it keeps out of lava, fire and cacti and avoids water.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use ferrocraft_protocol::codec::BlockPos;

use crate::data::GameData;
use crate::world::{MAX_Y, MIN_Y, World};

/// Who walks: their size and how far they dare to drop.
#[derive(Debug, Clone, Copy)]
pub struct Walker {
    pub width: f64,
    pub height: f64,
    /// `Mob.getMaxFallDistance`.
    pub max_drop: i32,
}

impl Walker {
    /// Blocks the mob covers across, from its node's corner.
    fn span(&self) -> i32 {
        (self.width.ceil() as i32).max(1)
    }

    fn tall(&self) -> i32 {
        (self.height.ceil() as i32).max(1)
    }
}

/// What a block means for someone walking.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Cell {
    /// Nothing to bump into.
    Open,
    /// Open, but costly (water).
    Wet,
    /// Never to be entered (lava, fire, cacti, berry bushes).
    Danger,
    Solid,
}

fn cell(world: &World, data: &GameData, pos: BlockPos) -> Cell {
    if !(MIN_Y..=MAX_Y).contains(&pos.y) {
        return Cell::Solid;
    }
    let state = world.block(pos);
    let physics = data.physics(state);
    if physics.fluid == 2 {
        return Cell::Danger;
    }
    if let Some((_, block)) = data.block_of(state) {
        let name = block.name.as_str();
        if matches!(
            name,
            "minecraft:fire"
                | "minecraft:soul_fire"
                | "minecraft:cactus"
                | "minecraft:sweet_berry_bush"
                | "minecraft:magma_block"
                | "minecraft:campfire"
                | "minecraft:soul_campfire"
                | "minecraft:powder_snow"
        ) {
            return Cell::Danger;
        }
    }
    if !data.collision_shape(state).is_empty() {
        return Cell::Solid;
    }
    if physics.fluid == 1 {
        Cell::Wet
    } else {
        Cell::Open
    }
}

/// The top of a block's collision shape, 0 if it has none.
fn floor_height(world: &World, data: &GameData, pos: BlockPos) -> f64 {
    data.collision_shape(world.block(pos))
        .iter()
        .map(|b| b[4])
        .fold(0.0, f64::max)
}

struct Grid<'a> {
    world: &'a World,
    data: &'a GameData,
    walker: Walker,
    cells: HashMap<BlockPos, Cell>,
}

impl Grid<'_> {
    fn cell(&mut self, pos: BlockPos) -> Cell {
        let (world, data) = (self.world, self.data);
        *self
            .cells
            .entry(pos)
            .or_insert_with(|| cell(world, data, pos))
    }

    /// The body fits at `pos`: the cost of being there, `None` if it does
    /// not fit or it is dangerous.
    fn body(&mut self, pos: BlockPos) -> Option<f64> {
        let (span, tall) = (self.walker.span(), self.walker.tall());
        let mut cost = 0.0;
        for dx in 0..span {
            for dz in 0..span {
                for dy in 0..tall {
                    match self.cell(BlockPos {
                        x: pos.x + dx,
                        y: pos.y + dy,
                        z: pos.z + dz,
                    }) {
                        Cell::Open => {}
                        Cell::Wet => cost = 8.0,
                        Cell::Danger | Cell::Solid => return None,
                    }
                }
            }
        }
        Some(cost)
    }

    /// Something to stand on under the body: a block with a top at most a
    /// full block high, and nothing dangerous.
    fn ground(&mut self, pos: BlockPos) -> bool {
        let span = self.walker.span();
        let mut any = false;
        for dx in 0..span {
            for dz in 0..span {
                let below = BlockPos {
                    x: pos.x + dx,
                    y: pos.y - 1,
                    z: pos.z + dz,
                };
                match self.cell(below) {
                    Cell::Danger => return false,
                    Cell::Solid => {
                        let top = floor_height(self.world, self.data, below);
                        if top > 1.0 {
                            return false;
                        }
                        any = true;
                    }
                    // Swimming at the surface counts as standing.
                    Cell::Wet => any = true,
                    Cell::Open => {}
                }
            }
        }
        any
    }

    /// Where the mob ends up stepping from `from` towards the column
    /// `(x, z)`: the same height, one up, or a few down. With its cost.
    fn step(&mut self, from: BlockPos, x: i32, z: i32) -> Option<(BlockPos, f64)> {
        let level = BlockPos { x, y: from.y, z };
        if let Some(cost) = self.body(level) {
            if self.ground(level) {
                return Some((level, cost));
            }
            // Down, as far as the mob dares.
            for drop in 1..=self.walker.max_drop {
                let below = BlockPos {
                    x,
                    y: from.y - drop,
                    z,
                };
                let cost = self.body(below)?;
                if self.ground(below) {
                    return Some((below, cost + f64::from(drop) * 0.5));
                }
            }
            return None;
        }
        // One up, if there is room to jump.
        let up = BlockPos {
            x,
            y: from.y + 1,
            z,
        };
        let above = BlockPos {
            y: from.y + 1,
            ..from
        };
        let headroom = self.body(above).is_some();
        let cost = self.body(up)?;
        (headroom && self.ground(up)).then_some((up, cost + 0.5))
    }
}

#[derive(PartialEq)]
struct Open {
    estimate: f64,
    cost: f64,
    pos: BlockPos,
}

impl Eq for Open {}

impl Ord for Open {
    fn cmp(&self, other: &Self) -> Ordering {
        // The smallest estimate first.
        other
            .estimate
            .total_cmp(&self.estimate)
            .then(other.cost.total_cmp(&self.cost))
    }
}

impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn distance(a: BlockPos, b: BlockPos) -> f64 {
    let (dx, dy, dz) = (
        f64::from(a.x - b.x),
        f64::from(a.y - b.y),
        f64::from(a.z - b.z),
    );
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// The node a mob at `pos` stands on: its feet's block, from the corner of
/// its box for mobs wider than a block.
pub fn node_at(pos: [f64; 3], walker: &Walker) -> BlockPos {
    let offset = if walker.span() > 1 {
        walker.width / 2.0
    } else {
        0.0
    };
    BlockPos {
        x: (pos[0] - offset).floor() as i32,
        y: (pos[1] + 0.001).floor() as i32,
        z: (pos[2] - offset).floor() as i32,
    }
}

/// Where a mob stands when it reaches a node: its middle.
pub fn node_center(node: BlockPos, walker: &Walker) -> [f64; 3] {
    let half = f64::from(walker.span()) / 2.0;
    [
        f64::from(node.x) + half,
        f64::from(node.y),
        f64::from(node.z) + half,
    ]
}

/// A way from `start` towards `goal`, without the start: the nodes to walk
/// through. If the goal cannot be reached within `max_nodes`, the way to
/// the node nearest to it, like vanilla's partial paths. `None` if the mob
/// cannot get any closer.
pub fn find(
    world: &World,
    data: &GameData,
    walker: Walker,
    start: BlockPos,
    goal: BlockPos,
    reach: f64,
    max_nodes: usize,
) -> Option<Vec<BlockPos>> {
    let mut grid = Grid {
        world,
        data,
        walker,
        cells: HashMap::new(),
    };
    let mut open = BinaryHeap::new();
    let mut came_from: HashMap<BlockPos, BlockPos> = HashMap::new();
    let mut best_cost: HashMap<BlockPos, f64> = HashMap::new();
    best_cost.insert(start, 0.0);
    open.push(Open {
        estimate: distance(start, goal),
        cost: 0.0,
        pos: start,
    });
    let mut closest = (distance(start, goal), start);
    let mut visited = 0;
    while let Some(Open { cost, pos, .. }) = open.pop() {
        if best_cost.get(&pos).is_some_and(|&c| c < cost) {
            continue;
        }
        let left = distance(pos, goal);
        if left < closest.0 {
            closest = (left, pos);
        }
        if left <= reach {
            closest = (left, pos);
            break;
        }
        visited += 1;
        if visited > max_nodes {
            break;
        }
        // The four sides first, then the corners where both sides are free.
        let mut free = [false; 4];
        let sides = [(0, -1), (0, 1), (-1, 0), (1, 0)];
        let mut moves = Vec::with_capacity(8);
        for (i, (dx, dz)) in sides.into_iter().enumerate() {
            if let Some(step) = grid.step(pos, pos.x + dx, pos.z + dz) {
                free[i] = step.0.y == pos.y;
                moves.push((step, 1.0));
            }
        }
        for (a, b, dx, dz) in [(0, 2, -1, -1), (0, 3, 1, -1), (1, 2, -1, 1), (1, 3, 1, 1)] {
            if free[a]
                && free[b]
                && let Some(step) = grid.step(pos, pos.x + dx, pos.z + dz)
                && step.0.y == pos.y
            {
                moves.push((step, std::f64::consts::SQRT_2));
            }
        }
        for ((next, extra), length) in moves {
            let total = cost + length + extra;
            if best_cost.get(&next).is_none_or(|&c| total < c) {
                best_cost.insert(next, total);
                came_from.insert(next, pos);
                open.push(Open {
                    estimate: total + distance(next, goal),
                    cost: total,
                    pos: next,
                });
            }
        }
    }
    let (_, end) = closest;
    if end == start {
        return None;
    }
    let mut path = vec![end];
    let mut at = end;
    while let Some(&previous) = came_from.get(&at) {
        if previous == start {
            break;
        }
        path.push(previous);
        at = previous;
    }
    path.reverse();
    Some(path)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn at(x: i32, y: i32, z: i32) -> BlockPos {
        BlockPos { x, y, z }
    }

    const ZOMBIE: Walker = Walker {
        width: 0.6,
        height: 1.95,
        max_drop: 3,
    };

    #[test]
    fn walks_around_walls_and_over_steps() {
        let data = Arc::new(GameData::load().unwrap());
        let world = World::superflat(Arc::clone(&data)).unwrap();
        let stone = data.default_state("minecraft:stone").unwrap();
        // A wall two high from z = -3 to 3 at x = 3.
        for z in -3..=3 {
            for y in -60..=-59 {
                world.set_block(at(3, y, z), stone);
            }
        }
        let path = find(
            &world,
            &data,
            ZOMBIE,
            at(0, -60, 0),
            at(6, -60, 0),
            0.5,
            1000,
        )
        .unwrap();
        assert_eq!(*path.last().unwrap(), at(6, -60, 0));
        assert!(path.iter().all(|p| p.x != 3 || p.z.abs() > 3), "{path:?}");
        assert!(path.len() > 8, "around the wall: {path:?}");

        // A single block is a step up and down.
        world.set_block(at(1, -60, 10), stone);
        let path = find(
            &world,
            &data,
            ZOMBIE,
            at(0, -60, 10),
            at(2, -60, 10),
            0.5,
            1000,
        )
        .unwrap();
        assert_eq!(path, [at(1, -59, 10), at(2, -60, 10)]);

        // Lava is never crossed; a moat of it around the goal is a dead end.
        let lava = data.default_state("minecraft:lava").unwrap();
        for x in 18..=22 {
            for z in 18..=22 {
                if (x, z) != (20, 20) {
                    world.set_block(at(x, -61, z), lava);
                }
            }
        }
        let path = find(
            &world,
            &data,
            ZOMBIE,
            at(14, -60, 20),
            at(20, -60, 20),
            0.5,
            2000,
        )
        .unwrap();
        assert_ne!(*path.last().unwrap(), at(20, -60, 20));
        assert!(
            path.iter()
                .all(|p| !(18..=22).contains(&p.x) || !(18..=22).contains(&p.z))
        );
    }
}
