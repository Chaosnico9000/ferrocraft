//! Sky and block light, computed like vanilla's `LightEngine`: sky light comes
//! from every block at or above the column's lowest sky source, block light
//! from blocks that glow; both lose at least one level per block
//! (`max(1, dampening)`), and a face that a slab or stairs closes stops them.
//! Changes are incremental: a decrease pass clears what a change took away,
//! an increase pass spreads what is left.

use std::collections::VecDeque;

use ferrocraft_protocol::chunk::{LIGHT_LAYER_LEN, LightData};
use ferrocraft_protocol::codec::BlockPos;

use crate::data::{Aabb, GameData};
use crate::world::{MAX_Y, MIN_Y};

/// Light sections per chunk: the world's 24 plus one below and one above.
pub const LIGHT_SECTIONS: usize = 26;

/// Sky or block light.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Sky,
    Block,
}

/// The 16×16×16 light levels of one section, one nibble each, low nibble
/// first (`DataLayer`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Nibbles {
    Uniform(u8),
    Mixed(Box<[u8; LIGHT_LAYER_LEN]>),
}

impl Nibbles {
    pub fn get(&self, index: usize) -> u8 {
        match self {
            Nibbles::Uniform(level) => *level,
            Nibbles::Mixed(bytes) => (bytes[index >> 1] >> ((index & 1) * 4)) & 15,
        }
    }

    pub fn set(&mut self, index: usize, level: u8) {
        if let Nibbles::Uniform(old) = *self {
            if old == level {
                return;
            }
            *self = Nibbles::Mixed(Box::new([old | old << 4; LIGHT_LAYER_LEN]));
        }
        if let Nibbles::Mixed(bytes) = self {
            let shift = (index & 1) * 4;
            let byte = &mut bytes[index >> 1];
            *byte = (*byte & !(15 << shift)) | (level << shift);
        }
    }

    fn bytes(&self) -> Vec<u8> {
        match self {
            Nibbles::Uniform(level) => vec![level | level << 4; LIGHT_LAYER_LEN],
            Nibbles::Mixed(bytes) => bytes.to_vec(),
        }
    }

    fn from_bytes(bytes: &[u8]) -> Self {
        match bytes.first() {
            Some(&first) if bytes.iter().all(|&b| b == first) && first >> 4 == first & 15 => {
                Nibbles::Uniform(first & 15)
            }
            _ => {
                let mut layer = Box::new([0u8; LIGHT_LAYER_LEN]);
                layer[..bytes.len().min(LIGHT_LAYER_LEN)]
                    .copy_from_slice(&bytes[..bytes.len().min(LIGHT_LAYER_LEN)]);
                Nibbles::Mixed(layer)
            }
        }
    }
}

/// The light of one chunk column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkLight {
    pub sky: Vec<Nibbles>,
    pub block: Vec<Nibbles>,
}

impl ChunkLight {
    pub fn dark() -> Self {
        Self {
            sky: vec![Nibbles::Uniform(0); LIGHT_SECTIONS],
            block: vec![Nibbles::Uniform(0); LIGHT_SECTIONS],
        }
    }

    /// Light of a column whose lowest `solid` blocks are opaque and the rest
    /// air: full sky light above them, none below, no block light.
    pub fn flat(solid: usize) -> Self {
        let mut light = Self::dark();
        for (index, section) in light.sky.iter_mut().enumerate() {
            // y relative to the world's bottom of this section's first block.
            let base = index as i64 * 16 - 16;
            *section = if base >= solid as i64 {
                Nibbles::Uniform(15)
            } else if base + 16 <= solid as i64 {
                Nibbles::Uniform(0)
            } else {
                let mut nibbles = Nibbles::Uniform(0);
                for local_y in 0..16 {
                    if base + local_y >= solid as i64 {
                        for i in 0..256 {
                            nibbles.set(local_y as usize * 256 + i, 15);
                        }
                    }
                }
                nibbles
            };
        }
        light
    }

    /// Light as vanilla stored or sent it; sections it leaves out are dark
    /// below the lowest one it has and fully sky-lit above the highest.
    pub fn from_packet(data: &LightData) -> Self {
        let mut light = Self::dark();
        let mut layers = data.sky_layers.iter();
        let mut highest = None;
        for index in 0..LIGHT_SECTIONS {
            if data.sky_mask.get(index) {
                if let Some(layer) = layers.next() {
                    light.sky[index] = Nibbles::from_bytes(layer);
                    highest = Some(index);
                }
            } else if data.empty_sky_mask.get(index) {
                highest = Some(index);
            }
        }
        for section in light.sky.iter_mut().skip(highest.map_or(0, |h| h + 1)) {
            *section = Nibbles::Uniform(15);
        }
        let mut layers = data.block_layers.iter();
        for index in 0..LIGHT_SECTIONS {
            if data.block_mask.get(index)
                && let Some(layer) = layers.next()
            {
                light.block[index] = Nibbles::from_bytes(layer);
            }
        }
        light
    }

    fn layers(&mut self, kind: Kind) -> &mut Vec<Nibbles> {
        match kind {
            Kind::Sky => &mut self.sky,
            Kind::Block => &mut self.block,
        }
    }

    /// Light level at a position within the chunk; `section` is the light
    /// section index (world section + 1).
    pub fn get(&self, kind: Kind, section: usize, index: usize) -> u8 {
        let layers = match kind {
            Kind::Sky => &self.sky,
            Kind::Block => &self.block,
        };
        layers[section].get(index)
    }

    pub fn set(&mut self, kind: Kind, section: usize, index: usize, level: u8) {
        self.layers(kind)[section].set(index, level);
    }

    /// The light for a packet. `top` is the highest light section that needs
    /// sky data, like vanilla: one above the highest section with blocks;
    /// the client fills everything above with full sky light. `only` limits
    /// the sections to those that changed (sky, block), as bit sets.
    pub fn to_packet(&self, top: usize, only: Option<(u32, u32)>) -> LightData {
        let mut data = LightData::default();
        for index in 0..LIGHT_SECTIONS {
            let wanted = |mask: Option<u32>| mask.is_none_or(|m| m & 1 << index != 0);
            if wanted(only.map(|o| o.0)) && (index <= top || only.is_some()) {
                match &self.sky[index] {
                    Nibbles::Uniform(0) => data.empty_sky_mask.set(index),
                    layer => {
                        data.sky_mask.set(index);
                        data.sky_layers.push(layer.bytes());
                    }
                }
            }
            if wanted(only.map(|o| o.1))
                && (index <= top || only.is_some() || self.block[index] != Nibbles::Uniform(0))
            {
                match &self.block[index] {
                    Nibbles::Uniform(0) => data.empty_block_mask.set(index),
                    layer => {
                        data.block_mask.set(index);
                        data.block_layers.push(layer.bytes());
                    }
                }
            }
        }
        data
    }
}

/// What the light engine needs from the world.
pub trait LightWorld {
    fn data(&self) -> &GameData;
    fn state(&mut self, pos: BlockPos) -> u32;
    fn light(&mut self, kind: Kind, pos: BlockPos) -> u8;
    /// Also records the section as changed.
    fn set_light(&mut self, kind: Kind, pos: BlockPos, level: u8);
    /// The column's lowest sky source, see [`lowest_sky_source`]; worlds
    /// cache it.
    fn sky_source(&mut self, x: i32, z: i32) -> i32;
}

/// The six neighbours, in 3D data value order (down, up, north, south,
/// west, east).
const DIRECTIONS: [(i32, i32, i32); 6] = [
    (0, -1, 0),
    (0, 1, 0),
    (0, 0, -1),
    (0, 0, 1),
    (-1, 0, 0),
    (1, 0, 0),
];

fn opposite(direction: usize) -> usize {
    direction ^ 1
}

fn offset(pos: BlockPos, (dx, dy, dz): (i32, i32, i32)) -> BlockPos {
    BlockPos {
        x: pos.x + dx,
        y: pos.y + dy,
        z: pos.z + dz,
    }
}

/// Light lives one section below and above the world too.
fn in_light_range(pos: BlockPos) -> bool {
    (MIN_Y - 16..=MAX_Y + 16).contains(&pos.y)
}

/// `Shapes.faceShapeOccludes`: whether two face shapes together cover the
/// whole face between two blocks. `axis` 0 is x, 1 y, 2 z.
fn face_covered(a: &[Aabb], b: &[Aabb], axis: usize) -> bool {
    if a.is_empty() && b.is_empty() {
        return false;
    }
    let (u, v) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    let rects: Vec<[f64; 4]> = a
        .iter()
        .chain(b)
        .map(|r| [r[u], r[v], r[u + 3], r[v + 3]])
        .collect();
    if rects
        .iter()
        .any(|r| r[0] <= 0.0 && r[1] <= 0.0 && r[2] >= 1.0 && r[3] >= 1.0)
    {
        return true;
    }
    // Check the middle of every cell the rectangles' edges cut the face into.
    let mut us: Vec<f64> = rects
        .iter()
        .flat_map(|r| [r[0], r[2]])
        .chain([0.0, 1.0])
        .collect();
    let mut vs: Vec<f64> = rects
        .iter()
        .flat_map(|r| [r[1], r[3]])
        .chain([0.0, 1.0])
        .collect();
    for edges in [&mut us, &mut vs] {
        edges.retain(|e| (0.0..=1.0).contains(e));
        edges.sort_by(f64::total_cmp);
        edges.dedup();
    }
    us.windows(2).all(|uw| {
        vs.windows(2).all(|vw| {
            let (cu, cv) = ((uw[0] + uw[1]) / 2.0, (vw[0] + vw[1]) / 2.0);
            rects
                .iter()
                .any(|r| r[0] <= cu && cu <= r[2] && r[1] <= cv && cv <= r[3])
        })
    })
}

/// `LightEngine.shapeOccludes`: light cannot pass from `from` to `to` going
/// in `direction`.
fn occludes(data: &GameData, from: u32, to: u32, direction: usize) -> bool {
    face_covered(
        data.face_shape(from, direction),
        data.face_shape(to, opposite(direction)),
        direction / 2,
    )
}

/// `ChunkSkyLightSources.isEdgeOccluded`: sky light cannot fall from `top`
/// into `bottom`.
fn edge_occluded(data: &GameData, top: u32, bottom: u32) -> bool {
    data.physics(bottom).dampening != 0 || occludes(data, top, bottom, 0)
}

/// The lowest y in a column that gets full sky light: everything above the
/// first edge, from the top, that stops falling light. `state` gives the
/// column's blocks.
pub fn lowest_sky_source(data: &GameData, mut state: impl FnMut(i32) -> u32) -> i32 {
    let mut above = 0; // air above the world
    for y in (MIN_Y..=MAX_Y).rev() {
        let current = state(y);
        if edge_occluded(data, above, current) {
            return y + 1;
        }
        above = current;
    }
    MIN_Y
}

/// The level a position keeps whatever happens around it: full sky light at
/// sky sources, its glow for block light.
fn source_level(world: &mut impl LightWorld, kind: Kind, pos: BlockPos) -> u8 {
    match kind {
        Kind::Sky => {
            if pos.y >= world.sky_source(pos.x, pos.z) {
                15
            } else {
                0
            }
        }
        Kind::Block => {
            let state = world.state(pos);
            world.data().physics(state).emission
        }
    }
}

/// Clears and spreads light, like vanilla's two queues.
pub struct Engine {
    kind: Kind,
    decrease: VecDeque<(BlockPos, u8)>,
    increase: VecDeque<BlockPos>,
}

impl Engine {
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            decrease: VecDeque::new(),
            increase: VecDeque::new(),
        }
    }

    /// Takes away the light at `pos` and, when run, everything that depended
    /// on it.
    pub fn remove(&mut self, world: &mut impl LightWorld, pos: BlockPos) {
        let level = world.light(self.kind, pos);
        if level > 0 {
            world.set_light(self.kind, pos, 0);
            self.decrease.push_back((pos, level));
        }
    }

    /// Raises `pos` to at least `level` and, when run, spreads it.
    pub fn add(&mut self, world: &mut impl LightWorld, pos: BlockPos, level: u8) {
        if level > world.light(self.kind, pos) {
            world.set_light(self.kind, pos, level);
        }
        self.increase.push_back(pos);
    }

    /// Spreads whatever light `pos` has, e.g. from a neighbouring chunk.
    pub fn spread(&mut self, pos: BlockPos) {
        self.increase.push_back(pos);
    }

    /// Lets the neighbours shine into `pos` again.
    pub fn refill(&mut self, world: &mut impl LightWorld, pos: BlockPos) {
        for direction in DIRECTIONS {
            let neighbour = offset(pos, direction);
            if in_light_range(neighbour) && world.light(self.kind, neighbour) > 1 {
                self.increase.push_back(neighbour);
            }
        }
    }

    /// Runs the decrease pass, then the increase pass.
    pub fn run(&mut self, world: &mut impl LightWorld) {
        while let Some((pos, old)) = self.decrease.pop_front() {
            for direction in DIRECTIONS {
                let neighbour = offset(pos, direction);
                if !in_light_range(neighbour) {
                    continue;
                }
                let level = world.light(self.kind, neighbour);
                if level == 0 {
                    continue;
                }
                if level < old {
                    world.set_light(self.kind, neighbour, 0);
                    self.decrease.push_back((neighbour, level));
                    let own = source_level(world, self.kind, neighbour);
                    if own > 0 {
                        world.set_light(self.kind, neighbour, own);
                        self.increase.push_back(neighbour);
                    }
                } else {
                    // Lit from elsewhere: it may light the cleared area again.
                    self.increase.push_back(neighbour);
                }
            }
        }
        while let Some(pos) = self.increase.pop_front() {
            let level = world.light(self.kind, pos);
            if level <= 1 {
                continue;
            }
            let from = world.state(pos);
            for (direction, step) in DIRECTIONS.iter().enumerate() {
                let neighbour = offset(pos, *step);
                if !in_light_range(neighbour) {
                    continue;
                }
                let to = world.state(neighbour);
                let dampening = world.data().physics(to).dampening.max(1);
                let new = level.saturating_sub(dampening);
                if new <= world.light(self.kind, neighbour)
                    || occludes(world.data(), from, to, direction)
                {
                    continue;
                }
                world.set_light(self.kind, neighbour, new);
                self.increase.push_back(neighbour);
            }
        }
    }
}

/// Brings the light up to date after the block at `pos` changed. For sky
/// light the world must already know the column's new lowest source;
/// `old_source` is the one before the change.
pub fn block_changed(world: &mut impl LightWorld, pos: BlockPos, old_source: i32) {
    let mut block = Engine::new(Kind::Block);
    block.remove(world, pos);
    let glow = source_level(world, Kind::Block, pos);
    if glow > 0 {
        block.add(world, pos, glow);
    }
    block.refill(world, pos);
    block.run(world);

    let mut sky = Engine::new(Kind::Sky);
    let new_source = world.sky_source(pos.x, pos.z);
    let column = |y| BlockPos {
        x: pos.x,
        y,
        z: pos.z,
    };
    // Blocks that stopped or started being sky sources.
    for y in old_source..new_source {
        sky.remove(world, column(y));
    }
    for y in new_source..old_source {
        sky.add(world, column(y), 15);
    }
    if pos.y < new_source {
        sky.remove(world, pos);
    }
    sky.refill(world, pos);
    sky.run(world);
}

/// Lights a chunk from scratch: its own sky sources and glowing blocks, and
/// the light its neighbours shine in. Expects the chunk to be dark.
pub fn light_chunk(world: &mut impl LightWorld, chunk_x: i32, chunk_z: i32) {
    let (x0, z0) = (chunk_x * 16, chunk_z * 16);
    let mut sky = Engine::new(Kind::Sky);
    let mut block = Engine::new(Kind::Block);
    for x in x0..x0 + 16 {
        for z in z0..z0 + 16 {
            let source = world.sky_source(x, z);
            // Sources only spread down and sideways as far as a neighbour
            // column reaches lower.
            let lowest_neighbour = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                .iter()
                .map(|(dx, dz)| world.sky_source(x + dx, z + dz))
                .min()
                .unwrap_or(source);
            for y in source..=MAX_Y + 16 {
                let pos = BlockPos { x, y, z };
                world.set_light(Kind::Sky, pos, 15);
                if y <= source.max(lowest_neighbour) + 1 {
                    sky.spread(pos);
                }
            }
            for y in MIN_Y..=MAX_Y {
                let pos = BlockPos { x, y, z };
                let glow = source_level(world, Kind::Block, pos);
                if glow > 0 {
                    block.add(world, pos, glow);
                }
            }
        }
    }
    // Light flowing in over the chunk's edges.
    for i in 0..16 {
        for (x, z) in [
            (x0 - 1, z0 + i),
            (x0 + 16, z0 + i),
            (x0 + i, z0 - 1),
            (x0 + i, z0 + 16),
        ] {
            for y in MIN_Y - 16..=MAX_Y + 16 {
                let pos = BlockPos { x, y, z };
                if world.light(Kind::Sky, pos) > 1 {
                    sky.spread(pos);
                }
                if world.light(Kind::Block, pos) > 1 {
                    block.spread(pos);
                }
            }
        }
    }
    sky.run(world);
    block.run(world);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nibbles_pack_low_first() {
        let mut layer = Nibbles::Uniform(0);
        layer.set(0, 15);
        layer.set(1, 3);
        assert_eq!(layer.get(0), 15);
        assert_eq!(layer.get(1), 3);
        assert_eq!(layer.bytes()[0], 0x3f);
        assert_eq!(
            Nibbles::from_bytes(&[0xff; LIGHT_LAYER_LEN]),
            Nibbles::Uniform(15)
        );
    }

    #[test]
    fn faces_cover_together() {
        let bottom_half = [[0.0, 0.0, 0.0, 1.0, 0.5, 1.0]];
        let top_half = [[0.0, 0.5, 0.0, 1.0, 1.0, 1.0]];
        let full = [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];
        // Seen along x, two halves of a face make a whole one.
        assert!(face_covered(&bottom_half, &top_half, 0));
        assert!(!face_covered(&bottom_half, &[], 0));
        assert!(face_covered(&full, &[], 1));
        assert!(!face_covered(&[], &[], 1));
    }
}
