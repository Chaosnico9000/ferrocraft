//! The world: an endless superflat overworld. Chunks nobody has changed are
//! all alike, so they share one prebuilt packet; changed chunks keep their
//! blocks and light in memory.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::chunk::{
    BIOMES_PER_SECTION, BLOCKS_PER_SECTION, ContainerKind, Heightmap, HeightmapKind, LightData,
    encode_section, global_bits,
};
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{ChunkBlockEntity, LevelChunkWithLight};

use crate::block_entity::BlockEntity;

use crate::data::GameData;
use crate::light::{self, ChunkLight, Kind, LightWorld};
use crate::storage::{ChunkStates, SectionBiomes, Storage, StoredChunk, chunk_to_nbt, region_of};

pub const DIMENSION: &str = "minecraft:overworld";
/// Height range of `dimension_type/overworld.json`.
pub const MIN_Y: i32 = -64;
pub const HEIGHT: i32 = 384;
pub const MAX_Y: i32 = MIN_Y + HEIGHT - 1;
const SECTIONS: usize = (HEIGHT / 16) as usize;

/// Vanilla's default superflat preset without its village: layers from the
/// bottom up, in the plains biome.
pub const LAYERS: &[(&str, i32)] = &[
    ("minecraft:bedrock", 1),
    ("minecraft:dirt", 2),
    ("minecraft:grass_block", 1),
];
pub const BIOME: &str = "minecraft:plains";
/// What vanilla reports as sea level for this preset.
pub const SEA_LEVEL: i32 = -63;

/// Chunk coordinates.
pub type ChunkPos = (i32, i32);

pub fn chunk_of(pos: BlockPos) -> ChunkPos {
    (pos.x >> 4, pos.z >> 4)
}

/// One 16×16×16 section of block states.
#[derive(Debug, Clone)]
enum Section {
    Uniform(u32),
    /// Indexed `y * 256 + z * 16 + x`, like vanilla's containers.
    Mixed(Box<[u32; BLOCKS_PER_SECTION]>),
}

impl Section {
    fn get(&self, index: usize) -> u32 {
        match self {
            Section::Uniform(state) => *state,
            Section::Mixed(states) => states[index],
        }
    }

    fn set(&mut self, index: usize, state: u32) {
        if let Section::Uniform(old) = *self {
            if old == state {
                return;
            }
            *self = Section::Mixed(Box::new([old; BLOCKS_PER_SECTION]));
        }
        if let Section::Mixed(states) = self {
            states[index] = state;
        }
    }

    fn states(&self) -> Box<[u32; BLOCKS_PER_SECTION]> {
        match self {
            Section::Uniform(state) => Box::new([*state; BLOCKS_PER_SECTION]),
            Section::Mixed(states) => states.clone(),
        }
    }
}

/// A chunk column that differs from the superflat template, in its blocks
/// or only in its light.
#[derive(Debug, Clone)]
pub struct Chunk {
    sections: Vec<Section>,
    /// Per section; `None` is plains everywhere.
    biomes: Option<Vec<SectionBiomes>>,
    light: ChunkLight,
    /// Lowest sky source per column (`z * 16 + x`), `i32::MIN` until known.
    sky_sources: Vec<i32>,
    block_entities: HashMap<BlockPos, BlockEntity>,
    /// The chunk as loaded from disk, to keep what Ferrocraft does not model.
    stored: Option<Nbt>,
    /// The blocks differ from what was loaded or generated.
    modified: bool,
}

/// No sky source computed yet.
const UNKNOWN: i32 = i32::MIN;

#[derive(Debug)]
pub struct World {
    pub dimension_type: i32,
    /// Feet position of a player standing on the surface.
    pub surface_y: i32,
    air: u32,
    /// Block state per y of an untouched column, from `MIN_Y` up.
    column: Vec<u32>,
    blocks: ContainerKind,
    biomes: ContainerKind,
    biome: u32,
    template: LevelChunkWithLight,
    template_light: ChunkLight,
    changed: RwLock<HashMap<ChunkPos, Chunk>>,
    /// Changed since the last save.
    dirty: Mutex<HashSet<ChunkPos>>,
    /// Light sections changed since the last [`World::take_light_updates`]:
    /// sky and block, as bit sets.
    light_updates: Mutex<HashMap<ChunkPos, (u32, u32)>>,
    /// Loaded chunks that still need their light computed.
    unlit: Mutex<Vec<ChunkPos>>,
    /// Block entities that work by themselves every tick: furnaces.
    ticking: Mutex<HashSet<BlockPos>>,
    data: Arc<GameData>,
}

/// Block entities that tick.
fn ticks(kind: &str) -> bool {
    crate::crafting::Cooker::of(kind).is_some()
}

impl World {
    pub fn superflat(data: Arc<GameData>) -> anyhow::Result<Self> {
        let air = data.default_state("minecraft:air")?;
        let mut column = Vec::new();
        for &(block, thickness) in LAYERS {
            let id = data.default_state(block)?;
            column.extend(std::iter::repeat_n(id, thickness as usize));
        }
        let mut world = Self {
            dimension_type: data.registry_id("minecraft:dimension_type", DIMENSION)?,
            surface_y: MIN_Y + column.len() as i32,
            air,
            column,
            blocks: ContainerKind::BlockStates {
                global_bits: global_bits(data.block_states),
            },
            biomes: ContainerKind::Biomes {
                global_bits: global_bits(data.registry_len("minecraft:worldgen/biome")?),
            },
            biome: data.registry_id("minecraft:worldgen/biome", BIOME)? as u32,
            template: LevelChunkWithLight {
                x: 0,
                z: 0,
                heightmaps: Vec::new(),
                sections: Vec::new(),
                block_entities: Vec::new(),
                light: LightData::default(),
            },
            template_light: ChunkLight::flat(column_len(&data)?),
            changed: RwLock::new(HashMap::new()),
            dirty: Mutex::new(HashSet::new()),
            light_updates: Mutex::new(HashMap::new()),
            unlit: Mutex::new(Vec::new()),
            ticking: Mutex::new(HashSet::new()),
            data,
        };
        world.template = world.encode(0, 0, &world.generate());
        Ok(world)
    }

    /// An untouched chunk.
    fn generate(&self) -> Chunk {
        let sections = (0..SECTIONS)
            .map(|section| {
                let layers = &self.column[(section * 16).min(self.column.len())..];
                if layers.is_empty() {
                    return Section::Uniform(self.air);
                }
                let mut s = Section::Uniform(self.air);
                for (local_y, &state) in layers.iter().take(16).enumerate() {
                    for i in 0..256 {
                        s.set(local_y * 256 + i, state);
                    }
                }
                s
            })
            .collect();
        Chunk {
            sections,
            biomes: None,
            light: self.template_light.clone(),
            sky_sources: vec![self.surface_y; 256],
            block_entities: HashMap::new(),
            stored: None,
            modified: false,
        }
    }

    fn index(pos: BlockPos) -> Option<(usize, usize)> {
        if !(MIN_Y..=MAX_Y).contains(&pos.y) {
            return None;
        }
        let y = (pos.y - MIN_Y) as usize;
        let index = (y % 16) * 256 + (pos.z & 15) as usize * 16 + (pos.x & 15) as usize;
        Some((y / 16, index))
    }

    pub fn block(&self, pos: BlockPos) -> u32 {
        let Some((section, index)) = Self::index(pos) else {
            return self.air;
        };
        let changed = self.changed.read().expect("world lock");
        match changed.get(&chunk_of(pos)) {
            Some(chunk) => chunk.sections[section].get(index),
            None => self
                .column
                .get((pos.y - MIN_Y) as usize)
                .copied()
                .unwrap_or(self.air),
        }
    }

    /// Sets a block and updates the light; `false` if it is outside the
    /// world or already that state.
    pub fn set_block(&self, pos: BlockPos, state: u32) -> bool {
        let Some((section, index)) = Self::index(pos) else {
            return false;
        };
        if self.block(pos) == state {
            return false;
        }
        let mut changed = self.changed.write().expect("world lock");
        let mut access = Access {
            world: self,
            chunks: &mut changed,
            updates: HashMap::new(),
        };
        let old_source = access.sky_source(pos.x, pos.z);
        let kind = self
            .data
            .block_of(state)
            .and_then(|(_, block)| block.block_entity.clone());
        let chunk = access.chunk_mut(chunk_of(pos));
        chunk.sections[section].set(index, state);
        chunk.modified = true;
        // A block entity goes with its block; a chest that becomes half of a
        // double chest keeps its items.
        if chunk.block_entities.get(&pos).map(|e| &e.kind) != kind.as_ref() {
            chunk.block_entities.remove(&pos);
            let mut ticking = self.ticking.lock().expect("world lock");
            ticking.remove(&pos);
            if let Some(kind) = kind {
                if ticks(&kind) {
                    ticking.insert(pos);
                }
                chunk.block_entities.insert(pos, BlockEntity::new(&kind));
            }
        }
        chunk.sky_sources[column_index(pos.x, pos.z)] = UNKNOWN;
        light::block_changed(&mut access, pos, old_source);
        let updates = access.updates;
        drop(changed);
        self.dirty.lock().expect("world lock").insert(chunk_of(pos));
        let mut pending = self.light_updates.lock().expect("world lock");
        for (chunk, (sky, block)) in updates {
            let entry = pending.entry(chunk).or_default();
            entry.0 |= sky;
            entry.1 |= block;
        }
        true
    }

    /// A copy of the block entity at `pos`.
    pub fn block_entity(&self, pos: BlockPos) -> Option<BlockEntity> {
        self.changed
            .read()
            .expect("world lock")
            .get(&chunk_of(pos))?
            .block_entities
            .get(&pos)
            .cloned()
    }

    /// Changes the block entity at `pos`; `false` if there is none.
    pub fn update_block_entity(
        &self,
        pos: BlockPos,
        change: impl FnOnce(&mut BlockEntity),
    ) -> bool {
        self.change_block_entity(pos, |entity| {
            change(entity);
            true
        })
    }

    /// Changes the block entity at `pos`, which is to be saved again only
    /// if `change` says something worth saving changed; `false` if there
    /// is none.
    pub fn change_block_entity(
        &self,
        pos: BlockPos,
        change: impl FnOnce(&mut BlockEntity) -> bool,
    ) -> bool {
        let mut changed = self.changed.write().expect("world lock");
        let Some(chunk) = changed.get_mut(&chunk_of(pos)) else {
            return false;
        };
        let Some(entity) = chunk.block_entities.get_mut(&pos) else {
            return false;
        };
        if change(entity) {
            entity.modified = true;
            chunk.modified = true;
            drop(changed);
            self.dirty.lock().expect("world lock").insert(chunk_of(pos));
        }
        true
    }

    /// Where the block entities are that tick, such as furnaces.
    pub fn ticking_block_entities(&self) -> Vec<BlockPos> {
        let mut positions: Vec<BlockPos> = self
            .ticking
            .lock()
            .expect("world lock")
            .iter()
            .copied()
            .collect();
        positions.sort_unstable_by_key(|p| (p.x, p.y, p.z));
        positions
    }

    /// The highest block of a column that is not air, or the bottom of the
    /// world below it (`Heightmap.Types.WORLD_SURFACE` minus one).
    pub fn top_y(&self, x: i32, z: i32) -> i32 {
        let changed = self.changed.read().expect("world lock");
        let Some(chunk) = changed.get(&(x >> 4, z >> 4)) else {
            return self.surface_y - 1;
        };
        let column = column_index(x, z);
        for (section, blocks) in chunk.sections.iter().enumerate().rev() {
            if let Section::Uniform(state) = blocks
                && *state == self.air
            {
                continue;
            }
            for y in (0..16).rev() {
                if blocks.get(y * 256 + column) != self.air {
                    return MIN_Y + (section * 16 + y) as i32;
                }
            }
        }
        MIN_Y - 1
    }

    /// The light level at a position.
    pub fn light(&self, kind: Kind, pos: BlockPos) -> u8 {
        let mut changed = self.changed.write().expect("world lock");
        Access {
            world: self,
            chunks: &mut changed,
            updates: HashMap::new(),
        }
        .light(kind, pos)
    }

    /// The light that changed since the last call, per chunk, as packets
    /// with only the changed sections.
    pub fn take_light_updates(&self) -> Vec<(ChunkPos, LightData)> {
        let pending: Vec<_> = self
            .light_updates
            .lock()
            .expect("world lock")
            .drain()
            .collect();
        let changed = self.changed.read().expect("world lock");
        pending
            .into_iter()
            .filter_map(|(pos, masks)| {
                let chunk = changed.get(&pos)?;
                Some((pos, chunk.light.to_packet(0, Some(masks))))
            })
            .collect()
    }

    /// Puts a chunk loaded from disk in place. Chunks without stored light
    /// get theirs in [`World::light_loaded`].
    pub fn load_chunk(&self, stored: StoredChunk) {
        let sections = stored
            .states
            .into_iter()
            .map(|states| match states.iter().all(|&s| s == states[0]) {
                true => Section::Uniform(states[0]),
                false => Section::Mixed(states),
            })
            .collect();
        let lit = stored.light.is_some();
        self.ticking.lock().expect("world lock").extend(
            stored
                .block_entities
                .iter()
                .filter(|(_, e)| ticks(&e.kind))
                .map(|(pos, _)| *pos),
        );
        let chunk = Chunk {
            sections,
            biomes: stored.biomes,
            light: stored
                .light
                .as_ref()
                .map_or_else(ChunkLight::dark, ChunkLight::from_packet),
            sky_sources: vec![UNKNOWN; 256],
            block_entities: stored.block_entities.into_iter().collect(),
            stored: Some(stored.nbt),
            modified: false,
        };
        self.changed
            .write()
            .expect("world lock")
            .insert(stored.pos, chunk);
        if !lit {
            self.unlit.lock().expect("world lock").push(stored.pos);
        }
    }

    /// Lights the loaded chunks that came without light, such as those
    /// Ferrocraft saved.
    pub fn light_loaded(&self) {
        let unlit: Vec<ChunkPos> = self.unlit.lock().expect("world lock").drain(..).collect();
        let mut changed = self.changed.write().expect("world lock");
        let mut access = Access {
            world: self,
            chunks: &mut changed,
            updates: HashMap::new(),
        };
        for (x, z) in unlit {
            light::light_chunk(&mut access, x, z);
        }
    }

    /// Writes the region files of the chunks changed since the last save;
    /// returns how many chunks those regions hold.
    pub fn save(&self, storage: &Storage, data: &GameData) -> anyhow::Result<usize> {
        let dirty: Vec<ChunkPos> = self.dirty.lock().expect("world lock").drain().collect();
        let regions: HashSet<(i32, i32)> = dirty.iter().map(|&pos| region_of(pos)).collect();
        let mut written = 0;
        for region in regions {
            let chunks: Vec<(ChunkPos, Nbt)> = {
                let changed = self.changed.read().expect("world lock");
                changed
                    .iter()
                    // Chunks that only differ in light are generated again.
                    .filter(|(pos, chunk)| {
                        region_of(**pos) == region && (chunk.modified || chunk.stored.is_some())
                    })
                    .map(|(pos, chunk)| {
                        let nbt = match (&chunk.stored, chunk.modified) {
                            (Some(stored), false) => stored.clone(),
                            (stored, _) => {
                                let states: ChunkStates =
                                    chunk.sections.iter().map(Section::states).collect();
                                let block_entities = chunk
                                    .block_entities
                                    .iter()
                                    .map(|(at, entity)| entity.to_nbt(*at, data))
                                    .collect();
                                chunk_to_nbt(*pos, &states, block_entities, stored.as_ref(), data)
                            }
                        };
                        (*pos, nbt)
                    })
                    .collect()
            };
            written += chunks.len();
            if let Err(err) = storage.write_region(region, &chunks) {
                // Try again at the next save.
                let mut dirty = self.dirty.lock().expect("world lock");
                dirty.extend(chunks.iter().map(|(pos, _)| *pos));
                return Err(err);
            }
        }
        Ok(written)
    }

    /// The packet for chunk `(x, z)`.
    pub fn chunk(&self, x: i32, z: i32) -> LevelChunkWithLight {
        let changed = self.changed.read().expect("world lock");
        match changed.get(&(x, z)) {
            Some(chunk) => self.encode(x, z, chunk),
            None => LevelChunkWithLight {
                x,
                z,
                ..self.template.clone()
            },
        }
    }

    /// Chunks that differ from the template, for saving.
    pub fn changed_chunks(&self) -> Vec<ChunkPos> {
        self.changed
            .read()
            .expect("world lock")
            .keys()
            .copied()
            .collect()
    }

    fn encode(&self, x: i32, z: i32, chunk: &Chunk) -> LevelChunkWithLight {
        let mut sections = Vec::new();
        // Highest non-air block per column, as height above the bottom.
        let mut heights = [0u32; 256];
        // Light sections up to one above the highest with blocks carry sky
        // light; the client fills the rest.
        let mut top_light = 0;
        for (i, section) in chunk.sections.iter().enumerate() {
            let states = section.states();
            let non_air = states.iter().filter(|&&s| s != self.air).count() as u16;
            let fluids = states
                .iter()
                .filter(|&&s| self.data.physics(s).fluid != 0)
                .count() as u16;
            if non_air > 0 {
                top_light = i + 2;
                for (index, &state) in states.iter().enumerate() {
                    if state != self.air {
                        let height = (i * 16 + index / 256 + 1) as u32;
                        let column = &mut heights[index % 256];
                        *column = (*column).max(height);
                    }
                }
            }
            let biomes = chunk
                .biomes
                .as_ref()
                .map_or([self.biome; BIOMES_PER_SECTION], |b| b[i]);
            encode_section(
                non_air,
                fluids,
                (self.blocks, &states[..]),
                (self.biomes, &biomes),
                &mut sections,
            );
        }
        let heightmaps = [
            HeightmapKind::WorldSurface,
            HeightmapKind::MotionBlocking,
            HeightmapKind::MotionBlockingNoLeaves,
        ]
        .map(|kind| Heightmap::new(kind, &heights, HEIGHT as u32))
        .to_vec();
        let block_entities = chunk
            .block_entities
            .iter()
            .filter_map(|(pos, entity)| {
                Some(ChunkBlockEntity {
                    packed_xz: ((pos.x & 15) << 4 | (pos.z & 15)) as u8,
                    y: pos.y as i16,
                    kind: self.data.block_entity_type(&entity.kind).ok()?,
                    tag: entity.update_tag(),
                })
            })
            .collect();
        LevelChunkWithLight {
            x,
            z,
            heightmaps,
            sections,
            block_entities,
            light: chunk.light.to_packet(top_light, None),
        }
    }
}

fn column_index(x: i32, z: i32) -> usize {
    (z & 15) as usize * 16 + (x & 15) as usize
}

fn column_len(data: &GameData) -> anyhow::Result<usize> {
    let _ = data.default_state("minecraft:air")?;
    Ok(LAYERS.iter().map(|(_, n)| *n as usize).sum())
}

/// The world as the light engine sees it, while a write lock is held.
/// Chunks that light spreads into become changed chunks.
struct Access<'a> {
    world: &'a World,
    chunks: &'a mut HashMap<ChunkPos, Chunk>,
    updates: HashMap<ChunkPos, (u32, u32)>,
}

impl Access<'_> {
    fn chunk_mut(&mut self, pos: ChunkPos) -> &mut Chunk {
        let world = self.world;
        self.chunks.entry(pos).or_insert_with(|| world.generate())
    }

    fn light_index(pos: BlockPos) -> Option<(usize, usize)> {
        let section = usize::try_from(((pos.y - MIN_Y) >> 4) + 1).ok()?;
        if section >= light::LIGHT_SECTIONS {
            return None;
        }
        let index =
            (pos.y & 15) as usize * 256 + (pos.z & 15) as usize * 16 + (pos.x & 15) as usize;
        Some((section, index))
    }
}

impl LightWorld for Access<'_> {
    fn data(&self) -> &GameData {
        &self.world.data
    }

    fn state(&mut self, pos: BlockPos) -> u32 {
        let Some((section, index)) = World::index(pos) else {
            return self.world.air;
        };
        match self.chunks.get(&chunk_of(pos)) {
            Some(chunk) => chunk.sections[section].get(index),
            None => self
                .world
                .column
                .get((pos.y - MIN_Y) as usize)
                .copied()
                .unwrap_or(self.world.air),
        }
    }

    fn light(&mut self, kind: Kind, pos: BlockPos) -> u8 {
        let Some((section, index)) = Self::light_index(pos) else {
            // Above the light sections there is only sky.
            return if kind == Kind::Sky && pos.y > MAX_Y {
                15
            } else {
                0
            };
        };
        match self.chunks.get(&chunk_of(pos)) {
            Some(chunk) => chunk.light.get(kind, section, index),
            None => self.world.template_light.get(kind, section, index),
        }
    }

    fn set_light(&mut self, kind: Kind, pos: BlockPos, level: u8) {
        let Some((section, index)) = Self::light_index(pos) else {
            return;
        };
        let chunk_pos = chunk_of(pos);
        self.chunk_mut(chunk_pos)
            .light
            .set(kind, section, index, level);
        let masks = self.updates.entry(chunk_pos).or_default();
        match kind {
            Kind::Sky => masks.0 |= 1 << section,
            Kind::Block => masks.1 |= 1 << section,
        }
    }

    fn sky_source(&mut self, x: i32, z: i32) -> i32 {
        let chunk_pos = (x >> 4, z >> 4);
        let column = column_index(x, z);
        match self.chunks.get(&chunk_pos) {
            None => self.world.surface_y,
            Some(chunk) if chunk.sky_sources[column] != UNKNOWN => chunk.sky_sources[column],
            Some(chunk) => {
                let sections = &chunk.sections;
                let source = light::lowest_sky_source(&self.world.data, |y| {
                    let (section, index) =
                        World::index(BlockPos { x, y, z }).expect("y inside the world");
                    sections[section].get(index)
                });
                self.chunks
                    .get_mut(&chunk_pos)
                    .expect("looked up above")
                    .sky_sources[column] = source;
                source
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ferrocraft_protocol::Encode;

    use super::*;

    #[test]
    fn superflat_chunk_matches_vanilla() {
        let data = Arc::new(GameData::load().unwrap());
        let world = World::superflat(Arc::clone(&data)).unwrap();
        assert_eq!(world.surface_y, -60);
        let chunk = world.chunk(0, -1);

        // Facts from a vanilla 26.3 superflat chunk: 24 sections of which the
        // first holds 1024 blocks in a 4-bit palette, 2244 bytes in total.
        assert_eq!(chunk.sections.len(), 2244);
        assert_eq!(&chunk.sections[..8], [0x04, 0x00, 0x00, 0x00, 4, 4, 88, 10]);
        let heights = &chunk.heightmaps;
        assert_eq!(
            heights.iter().map(|h| h.kind).collect::<Vec<_>>(),
            [1, 4, 5]
        );
        assert!(heights.iter().all(|h| h.data[0] == 0x0100_8040_2010_0804));

        // Light: sections 1 and 2 have sky light, 0 is empty, no block light.
        let mut light = Vec::new();
        chunk.light.encode(&mut light);
        assert_eq!(&light[..8], [1, 0x06, 0, 1, 0x01, 1, 0x07, 2]);
        let lower = &chunk.light.sky_layers[0];
        assert!(lower[..512].iter().all(|&b| b == 0), "inside the ground");
        assert!(lower[512..].iter().all(|&b| b == 0xff), "above the grass");
        assert!(chunk.light.sky_layers[1].iter().all(|&b| b == 0xff));
    }

    #[test]
    fn changed_blocks_show_up_in_their_chunk_only() {
        let data = Arc::new(GameData::load().unwrap());
        let world = World::superflat(Arc::clone(&data)).unwrap();
        let stone = data.default_state("minecraft:stone").unwrap();
        let pos = BlockPos {
            x: -1,
            y: 100,
            z: 17,
        };
        assert_eq!(world.block(pos), world.air);
        assert!(world.set_block(pos, stone));
        assert!(!world.set_block(pos, stone), "no change the second time");
        assert_eq!(world.block(pos), stone);
        assert_eq!(world.changed_chunks(), [(-1, 1)]);

        let changed = world.chunk(-1, 1);
        assert_ne!(changed.sections, world.template.sections);
        // The column at x = 15, z = 1 of chunk (-1, 1) reaches y = 100.
        let map = Heightmap::new(
            HeightmapKind::WorldSurface,
            &{
                let mut h = [4u32; 256];
                h[16 + 15] = (100 - MIN_Y + 1) as u32;
                h
            },
            HEIGHT as u32,
        );
        assert_eq!(changed.heightmaps[0], map);
        assert_eq!(world.chunk(0, 0).sections, world.template.sections);

        // Breaking a layer of the ground works the same way.
        let ground = BlockPos { x: 3, y: -61, z: 3 };
        assert_eq!(
            world.block(ground),
            data.default_state("minecraft:grass_block").unwrap()
        );
        assert!(world.set_block(ground, world.air));
        assert_eq!(world.block(ground), world.air);
        assert!(!world.set_block(
            BlockPos {
                x: 0,
                y: MAX_Y + 1,
                z: 0
            },
            stone
        ));
    }

    #[test]
    fn light_follows_block_changes() {
        let data = Arc::new(GameData::load().unwrap());
        let world = World::superflat(Arc::clone(&data)).unwrap();
        let state = |name| data.default_state(name).unwrap();
        let at = |x, y, z| BlockPos { x, y, z };
        assert_eq!(world.light(Kind::Sky, at(0, -60, 0)), 15);
        assert_eq!(world.light(Kind::Sky, at(0, -61, 0)), 0, "inside the grass");

        // Under a single stone block the sky light comes in from the side.
        world.set_block(at(0, -59, 0), state("minecraft:stone"));
        assert_eq!(world.light(Kind::Sky, at(0, -60, 0)), 14);
        assert!(!world.take_light_updates().is_empty());
        assert!(world.take_light_updates().is_empty(), "taken");

        // A torch: 14 on its own block, one less per step.
        let torch = at(20, -60, 20);
        world.set_block(torch, state("minecraft:torch"));
        assert_eq!(world.light(Kind::Block, torch), 14);
        assert_eq!(world.light(Kind::Block, at(21, -60, 20)), 13);
        assert_eq!(world.light(Kind::Block, at(20, -60, 25)), 9);
        // Across a chunk border too (x = 32 is the next chunk).
        world.set_block(at(31, -60, 20), state("minecraft:glowstone"));
        assert_eq!(world.light(Kind::Block, at(33, -60, 20)), 13);
        world.set_block(torch, state("minecraft:air"));
        assert_eq!(
            world.light(Kind::Block, torch),
            4,
            "only the glowstone's light"
        );
        world.set_block(at(31, -60, 20), state("minecraft:air"));
        assert_eq!(world.light(Kind::Block, at(33, -60, 20)), 0);
        assert_eq!(world.light(Kind::Block, torch), 0);

        // A closed stone box is dark inside; opening the roof lets the sky in.
        let stone = state("minecraft:stone");
        for x in 49..=51 {
            for z in 49..=51 {
                for y in -60..=-58 {
                    if (x, y, z) != (50, -59, 50) {
                        world.set_block(at(x, y, z), stone);
                    }
                }
            }
        }
        assert_eq!(world.light(Kind::Sky, at(50, -59, 50)), 0);
        world.set_block(at(50, -58, 50), state("minecraft:air"));
        assert_eq!(
            world.light(Kind::Sky, at(50, -59, 50)),
            15,
            "a sky source again"
        );
        // A bottom slab as roof keeps the light out from above.
        let slab = data.block_named("minecraft:oak_slab").unwrap();
        let bottom = slab.with(slab.default, "type", "bottom").unwrap();
        let bottom = slab.with(bottom, "waterlogged", "false").unwrap();
        world.set_block(at(50, -58, 50), bottom);
        assert_eq!(world.light(Kind::Sky, at(50, -59, 50)), 0);
    }
}
