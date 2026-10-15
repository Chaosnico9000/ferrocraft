//! The world on disk, in vanilla 26.3's formats and layout, so that vanilla
//! and its tools can read it: Anvil region files
//! (`dimensions/minecraft/overworld/region/r.<x>.<z>.mca`) for chunks and
//! (`.../entities/r.<x>.<z>.mca`) for the entities in them, gzipped NBT
//! (`players/data/<uuid>.dat`) for players. Only chunks that differ from the
//! superflat template are written; the rest is generated.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::chunk::{
    BIOMES_PER_SECTION, BLOCKS_PER_SECTION, LIGHT_LAYER_LEN, LightData, bits_for, pack_bits,
};
use ferrocraft_protocol::packets::play::{ComponentPatch, ItemStack};
use flate2::Compression;
use flate2::read::{GzDecoder, ZlibDecoder};
use flate2::write::{GzEncoder, ZlibEncoder};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::block_entity::BlockEntity;
use crate::data::GameData;
use crate::world::{ChunkPos, MIN_Y};
use ferrocraft_protocol::codec::BlockPos;

/// `world_version` from 26.3's `version.json`: tells vanilla which upgrades
/// a file needs.
pub const DATA_VERSION: i32 = 5023;
/// The lowest section's `Y`.
const MIN_SECTION: i32 = MIN_Y >> 4;
const SECTOR: usize = 4096;
/// Chunk compression types in a region file.
const GZIP: u8 = 1;
const ZLIB: u8 = 2;
const UNCOMPRESSED: u8 = 3;
/// Set on the compression type when the chunk lives in a `.mcc` file of its
/// own because it needs more than 255 sectors.
const EXTERNAL: u8 = 128;

/// The block states of one chunk, section by section from the bottom.
pub type ChunkStates = Vec<Box<[u32; BLOCKS_PER_SECTION]>>;
/// Biome ids of one section, 4×4×4 cells.
pub type SectionBiomes = [u32; BIOMES_PER_SECTION];

/// A chunk as read from a region file.
#[derive(Debug)]
pub struct StoredChunk {
    pub pos: ChunkPos,
    pub states: ChunkStates,
    /// One entry per section; `None` if the file names no biome we know.
    pub biomes: Option<Vec<SectionBiomes>>,
    /// Light as vanilla stored it, if it marked it as correct.
    pub light: Option<LightData>,
    pub block_entities: Vec<(BlockPos, BlockEntity)>,
    /// The whole chunk, so that saving keeps what Ferrocraft does not read:
    /// entities, structures, ticks.
    pub nbt: Nbt,
}

/// Where 26.3 keeps the overworld's chunks and the players
/// (`LevelResource`, `DimensionType.getStorageFolder`).
const REGION_DIR: &str = "dimensions/minecraft/overworld/region";
const ENTITY_DIR: &str = "dimensions/minecraft/overworld/entities";
const PLAYER_DIR: &str = "players/data";

#[derive(Debug)]
pub struct Storage {
    dir: PathBuf,
    regions: PathBuf,
    entities: PathBuf,
    players: PathBuf,
}

impl Storage {
    pub fn open(dir: &Path) -> anyhow::Result<Self> {
        let storage = Self {
            dir: dir.to_owned(),
            regions: dir.join(REGION_DIR),
            entities: dir.join(ENTITY_DIR),
            players: dir.join(PLAYER_DIR),
        };
        for sub in [&storage.regions, &storage.entities, &storage.players] {
            std::fs::create_dir_all(sub).with_context(|| format!("creating {}", sub.display()))?;
        }
        Ok(storage)
    }

    fn region_path(&self, (x, z): (i32, i32)) -> PathBuf {
        self.regions.join(format!("r.{x}.{z}.mca"))
    }

    /// Every finished chunk in the region files. Blocks this version does
    /// not know become air. Chunks vanilla only began to generate (a
    /// `Status` short of `minecraft:full`, which it keeps around the
    /// explored area) are left out: they hold no blocks yet and are
    /// generated like any other.
    pub fn load_chunks(&self, data: &GameData) -> anyhow::Result<Vec<StoredChunk>> {
        let mut chunks = Vec::new();
        let mut unknown = BTreeMap::new();
        let mut unfinished = 0;
        for entry in std::fs::read_dir(&self.regions)? {
            let path = entry?.path();
            let Some(region) = parse_region_name(&path) else {
                continue;
            };
            for raw in read_region(&path, region)? {
                let nbt = Nbt::read_named(&mut raw.as_slice())
                    .with_context(|| format!("chunk in {}", path.display()))?
                    .1;
                if let Some(Nbt::String(status)) = nbt.get("Status")
                    && !matches!(status.as_str(), "minecraft:full" | "full")
                {
                    unfinished += 1;
                    continue;
                }
                chunks.push(chunk_from_nbt(nbt, data, &mut unknown)?);
            }
        }
        if unfinished > 0 {
            debug!(unfinished, "left out chunks vanilla had not finished");
        }
        if !unknown.is_empty() {
            warn!(?unknown, "unknown blocks in the saved world became air");
        }
        Ok(chunks)
    }

    /// Writes a region file with exactly these chunks.
    pub fn write_region(
        &self,
        region: (i32, i32),
        chunks: &[(ChunkPos, Nbt)],
    ) -> anyhow::Result<()> {
        let mut encoded = Vec::new();
        for (pos, chunk) in chunks {
            let mut nbt = Vec::new();
            chunk.write_named("", &mut nbt);
            encoded.push((*pos, nbt));
        }
        let path = self.region_path(region);
        write_atomically(&path, &region_file(&encoded)?, false)
    }

    /// Every chunk of the entity regions, with its position: `{DataVersion,
    /// Position, Entities}` (`EntityStorage`).
    pub fn load_entity_chunks(&self) -> anyhow::Result<Vec<(ChunkPos, Nbt)>> {
        let mut chunks = Vec::new();
        for entry in std::fs::read_dir(&self.entities)? {
            let path = entry?.path();
            let Some(region) = parse_region_name(&path) else {
                continue;
            };
            for raw in read_region(&path, region)? {
                let nbt = Nbt::read_named(&mut raw.as_slice())
                    .with_context(|| format!("entities in {}", path.display()))?
                    .1;
                match nbt.get("Position") {
                    Some(Nbt::IntArray(pos)) if pos.len() == 2 => {
                        chunks.push(((pos[0], pos[1]), nbt));
                    }
                    _ => warn!("{}: an entity chunk without a position", path.display()),
                }
            }
        }
        Ok(chunks)
    }

    /// Writes an entity region with exactly these chunks; one without any
    /// goes away.
    pub fn write_entity_region(
        &self,
        region: (i32, i32),
        chunks: &[(ChunkPos, Nbt)],
    ) -> anyhow::Result<()> {
        let path = self
            .entities
            .join(format!("r.{}.{}.mca", region.0, region.1));
        if chunks.is_empty() {
            return match std::fs::remove_file(&path) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
                _ => Ok(()),
            };
        }
        let mut encoded = Vec::new();
        for (pos, chunk) in chunks {
            let mut nbt = Vec::new();
            chunk.write_named("", &mut nbt);
            encoded.push((*pos, nbt));
        }
        write_atomically(&path, &region_file(&encoded)?, false)
    }

    fn player_path(&self, uuid: Uuid) -> PathBuf {
        self.players.join(format!("{uuid}.dat"))
    }

    pub fn load_player(&self, uuid: Uuid, data: &GameData) -> anyhow::Result<Option<PlayerData>> {
        // `PlayerDataStorage.load`: a broken file is not the player's fault;
        // they start anew rather than being kept out.
        match read_nbt_file(&self.player_path(uuid)) {
            Ok(nbt) => Ok(nbt.map(|nbt| PlayerData::from_nbt(&nbt, data))),
            Err(err) => {
                warn!(%uuid, "failed to load player data, starting anew: {err:#}");
                Ok(None)
            }
        }
    }

    pub fn save_player(
        &self,
        uuid: Uuid,
        player: &PlayerData,
        data: &GameData,
    ) -> anyhow::Result<()> {
        write_nbt_file(&self.player_path(uuid), &player.to_nbt(data))
    }

    /// A file of the world, relative to its directory.
    pub fn path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }
}

/// A gzipped NBT file as vanilla writes them; `None` if it does not exist.
pub fn read_nbt_file(path: &Path) -> anyhow::Result<Option<Nbt>> {
    match read_nbt_only(path) {
        Ok(nbt @ Some(_)) => Ok(nbt),
        // `NbtIo.readCompressed` callers fall back to the copy from before
        // the last save, as vanilla's `_old` files are meant for.
        result => {
            let backup = backup_path(path);
            match read_nbt_only(&backup) {
                Ok(Some(nbt)) => {
                    if let Err(err) = &result {
                        warn!("{err:#}; using {}", backup.display());
                    }
                    Ok(Some(nbt))
                }
                _ => result,
            }
        }
    }
}

fn read_nbt_only(path: &Path) -> anyhow::Result<Option<Nbt>> {
    let compressed = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    let mut bytes = Vec::new();
    GzDecoder::new(compressed.as_slice())
        .read_to_end(&mut bytes)
        .with_context(|| format!("unpacking {}", path.display()))?;
    let (_, nbt) = Nbt::read_named(&mut bytes.as_slice())
        .with_context(|| format!("parsing {}", path.display()))?;
    Ok(Some(nbt))
}

/// `level.dat` → `level.dat_old`, as vanilla names its backups.
fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push("_old");
    path.with_file_name(name)
}

pub fn write_nbt_file(path: &Path, nbt: &Nbt) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut bytes = Vec::new();
    nbt.write_named("", &mut bytes);
    let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
    gzip.write_all(&bytes)?;
    write_atomically(path, &gzip.finish()?, true)
}

/// Region coordinates of a chunk.
pub fn region_of((x, z): ChunkPos) -> (i32, i32) {
    (x >> 5, z >> 5)
}

fn parse_region_name(path: &Path) -> Option<(i32, i32)> {
    let name = path.file_name()?.to_str()?;
    let mut parts = name.strip_prefix("r.")?.strip_suffix(".mca")?.split('.');
    let x = parts.next()?.parse().ok()?;
    let z = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((x, z))
}

/// Replaces a file without leaving a half-written one behind, even if the
/// machine goes down right after (`Util.safeReplaceFile`): the new content
/// is on disk before it takes the old one's name. With `keep_backup`, the
/// old one stays as `<name>_old`.
fn write_atomically(path: &Path, bytes: &[u8], keep_backup: bool) -> anyhow::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut file =
        std::fs::File::create(&tmp).with_context(|| format!("writing {}", tmp.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .with_context(|| format!("writing {}", tmp.display()))?;
    drop(file);
    if keep_backup && path.exists() {
        std::fs::rename(path, backup_path(path))
            .with_context(|| format!("backing up {}", path.display()))?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    // The renames themselves live in the directory.
    if let Some(dir) = path.parent()
        && let Ok(dir) = std::fs::File::open(dir)
    {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// The uncompressed NBT of every chunk in a region file
/// (`RegionFile`): a table of 1024 sector locations, then the chunks, each
/// with its length and compression type.
fn read_region(path: &Path, region: (i32, i32)) -> anyhow::Result<Vec<Vec<u8>>> {
    let file = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    if file.len() < 2 * SECTOR {
        // Vanilla leaves empty or truncated files behind after crashes.
        return Ok(Vec::new());
    }
    let mut chunks = Vec::new();
    for index in 0..1024 {
        let location = u32::from_be_bytes(file[index * 4..index * 4 + 4].try_into()?);
        let offset = (location >> 8) as usize * SECTOR;
        if offset == 0 {
            continue;
        }
        let Some(header) = file.get(offset..offset + 5) else {
            bail!("{}: chunk {index} lies past the end", path.display());
        };
        let len = u32::from_be_bytes(header[..4].try_into()?) as usize;
        let kind = header[4];
        let payload = if kind & EXTERNAL != 0 {
            let x = region.0 * 32 + (index % 32) as i32;
            let z = region.1 * 32 + (index / 32) as i32;
            let external = path.with_file_name(format!("c.{x}.{z}.mcc"));
            std::fs::read(&external).with_context(|| format!("reading {}", external.display()))?
        } else {
            match file.get(offset + 5..offset + 4 + len) {
                Some(payload) => payload.to_vec(),
                None => bail!("{}: chunk {index} is cut off", path.display()),
            }
        };
        let mut nbt = Vec::new();
        match kind & !EXTERNAL {
            GZIP => {
                GzDecoder::new(payload.as_slice()).read_to_end(&mut nbt)?;
            }
            ZLIB => {
                ZlibDecoder::new(payload.as_slice()).read_to_end(&mut nbt)?;
            }
            UNCOMPRESSED => nbt = payload,
            other => bail!("{}: chunk {index} uses compression {other}", path.display()),
        }
        chunks.push(nbt);
    }
    Ok(chunks)
}

/// A region file with these chunks, zlib-compressed like vanilla's default.
fn region_file(chunks: &[(ChunkPos, Vec<u8>)]) -> anyhow::Result<Vec<u8>> {
    let mut file = vec![0u8; 2 * SECTOR];
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as u32);
    for ((x, z), nbt) in chunks {
        let index = (x & 31) as usize + (z & 31) as usize * 32;
        let mut zlib = ZlibEncoder::new(Vec::new(), Compression::default());
        zlib.write_all(nbt)?;
        let compressed = zlib.finish()?;
        let start = file.len() / SECTOR;
        let sectors = (compressed.len() + 5).div_ceil(SECTOR);
        if sectors > 255 {
            bail!("chunk ({x}, {z}) is too large for a region file");
        }
        file.extend_from_slice(&(compressed.len() as u32 + 1).to_be_bytes());
        file.push(ZLIB);
        file.extend_from_slice(&compressed);
        file.resize((start + sectors) * SECTOR, 0);
        let location = (start as u32) << 8 | sectors as u32;
        file[index * 4..index * 4 + 4].copy_from_slice(&location.to_be_bytes());
        file[SECTOR + index * 4..SECTOR + index * 4 + 4].copy_from_slice(&now.to_be_bytes());
    }
    Ok(file)
}

/// Bits per entry of a stored block state container (`Strategy`): none for
/// a single value, at least 4, and as many as the palette needs beyond 8.
fn stored_bits(palette_len: usize, min: u8) -> u8 {
    match bits_for(palette_len.saturating_sub(1) as u64) {
        0 => 0,
        bits => bits.max(min),
    }
}

/// The `block_states` of a section (`PalettedContainer.pack`): the palette
/// in order of first appearance, then the indices.
fn block_states_nbt(states: &[u32; BLOCKS_PER_SECTION], data: &GameData) -> Nbt {
    let mut palette: Vec<u32> = Vec::new();
    let indices: Vec<u64> = states
        .iter()
        .map(|state| {
            let index = palette.iter().position(|p| p == state).unwrap_or_else(|| {
                palette.push(*state);
                palette.len() - 1
            });
            index as u64
        })
        .collect();
    let mut block_states = vec![(
        "palette".to_owned(),
        Nbt::List(palette.iter().map(|&s| state_to_nbt(s, data)).collect()),
    )];
    let bits = stored_bits(palette.len(), 4);
    if bits > 0 {
        block_states.push((
            "data".into(),
            Nbt::LongArray(pack_bits(indices.into_iter(), bits)),
        ));
    }
    Nbt::Compound(block_states)
}

/// `SerializableChunkData.write` for a chunk of blocks and block entities:
/// no light (vanilla lights it on load), no heightmaps (primed on load).
/// With `original`, the chunk as it was loaded, everything else stays as it
/// was.
pub fn chunk_to_nbt(
    (x, z): ChunkPos,
    states: &ChunkStates,
    block_entities: Vec<Nbt>,
    original: Option<&Nbt>,
    data: &GameData,
) -> Nbt {
    let Some(Nbt::Compound(original)) = original else {
        let sections = states
            .iter()
            .zip(MIN_SECTION..)
            .map(|(states, y)| {
                Nbt::Compound(vec![
                    ("Y".into(), Nbt::Byte(y as i8)),
                    ("block_states".into(), block_states_nbt(states, data)),
                    (
                        "biomes".into(),
                        Nbt::Compound(vec![(
                            "palette".into(),
                            Nbt::List(vec![Nbt::String(crate::world::BIOME.into())]),
                        )]),
                    ),
                ])
            })
            .collect();
        return Nbt::Compound(vec![
            ("DataVersion".into(), Nbt::Int(DATA_VERSION)),
            ("xPos".into(), Nbt::Int(x)),
            ("yPos".into(), Nbt::Int(MIN_SECTION)),
            ("zPos".into(), Nbt::Int(z)),
            ("LastUpdate".into(), Nbt::Long(0)),
            ("InhabitedTime".into(), Nbt::Long(0)),
            ("Status".into(), Nbt::String("minecraft:full".into())),
            ("sections".into(), Nbt::List(sections)),
            ("isLightOn".into(), Nbt::Byte(0)),
            ("block_entities".into(), Nbt::List(block_entities)),
        ]);
    };
    // The blocks changed, so light and heightmaps are stale: vanilla
    // recomputes both when they are missing.
    let mut chunk: Vec<(String, Nbt)> = original
        .iter()
        .filter(|(key, _)| key != "Heightmaps")
        .cloned()
        .collect();
    let mut sections: Vec<Nbt> = match chunk.iter().find(|(k, _)| k == "sections") {
        Some((_, Nbt::List(sections))) => sections.clone(),
        _ => Vec::new(),
    };
    for (states, y) in states.iter().zip(MIN_SECTION..) {
        let existing = sections
            .iter_mut()
            .find(|s| s.get("Y") == Some(&Nbt::Byte(y as i8)));
        let block_states = block_states_nbt(states, data);
        match existing {
            Some(Nbt::Compound(section)) => {
                section
                    .retain(|(k, _)| k != "block_states" && k != "SkyLight" && k != "BlockLight");
                section.push(("block_states".into(), block_states));
            }
            _ => sections.push(Nbt::Compound(vec![
                ("Y".into(), Nbt::Byte(y as i8)),
                ("block_states".into(), block_states),
            ])),
        }
    }
    if !chunk.iter().any(|(k, _)| k == "block_entities") {
        chunk.push(("block_entities".into(), Nbt::List(Vec::new())));
    }
    let mut block_entities = Some(block_entities);
    for (key, value) in &mut chunk {
        match key.as_str() {
            "sections" => *value = Nbt::List(std::mem::take(&mut sections)),
            "block_entities" => *value = Nbt::List(block_entities.take().unwrap_or_default()),
            "isLightOn" => *value = Nbt::Byte(0),
            "DataVersion" => *value = Nbt::Int(DATA_VERSION),
            _ => {}
        }
    }
    Nbt::Compound(chunk)
}

fn int(nbt: &Nbt, key: &str) -> anyhow::Result<i32> {
    match nbt.get(key) {
        Some(Nbt::Int(v)) => Ok(*v),
        other => bail!("{key} is {other:?}, not an int"),
    }
}

/// `SerializableChunkData.parse`, as far as blocks, biomes and light go.
/// Sections outside the world's height (vanilla keeps light-only ones above
/// and below) only count for light; missing ones are air.
pub fn chunk_from_nbt(
    nbt: Nbt,
    data: &GameData,
    unknown: &mut BTreeMap<String, usize>,
) -> anyhow::Result<StoredChunk> {
    let pos = (int(&nbt, "xPos")?, int(&nbt, "zPos")?);
    let air = data.default_state("minecraft:air")?;
    let count = (crate::world::HEIGHT / 16) as usize;
    let mut states: ChunkStates = (0..count)
        .map(|_| Box::new([air; BLOCKS_PER_SECTION]))
        .collect();
    let plains = data.registry_id("minecraft:worldgen/biome", crate::world::BIOME)? as u32;
    let mut biomes = vec![[plains; BIOMES_PER_SECTION]; count];
    let mut has_biomes = false;
    let light_correct = nbt.get("isLightOn") == Some(&Nbt::Byte(1));
    let mut light = LightData::default();
    if let Some(Nbt::List(sections)) = nbt.get("sections") {
        for section in sections {
            let Some(Nbt::Byte(y)) = section.get("Y") else {
                continue;
            };
            // Light sections start one below the world.
            if let Ok(light_index) = usize::try_from(i32::from(*y) - MIN_SECTION + 1) {
                for (key, mask, empty, layers) in [
                    (
                        "SkyLight",
                        &mut light.sky_mask,
                        &mut light.empty_sky_mask,
                        &mut light.sky_layers,
                    ),
                    (
                        "BlockLight",
                        &mut light.block_mask,
                        &mut light.empty_block_mask,
                        &mut light.block_layers,
                    ),
                ] {
                    if let Some(Nbt::ByteArray(layer)) = section.get(key)
                        && layer.len() == LIGHT_LAYER_LEN
                    {
                        if layer.iter().all(|&b| b == 0) {
                            empty.set(light_index);
                        } else {
                            mask.set(light_index);
                            layers.push(layer.iter().map(|&b| b as u8).collect());
                        }
                    }
                }
            }
            let Ok(index) = usize::try_from(i32::from(*y) - MIN_SECTION) else {
                continue;
            };
            if let (Some(target), Some(container)) =
                (states.get_mut(index), section.get("block_states"))
            {
                **target = read_container(container, data, unknown)
                    .with_context(|| format!("section {y} of chunk {pos:?}"))?;
            }
            if let (Some(target), Some(container)) = (biomes.get_mut(index), section.get("biomes"))
                && let Some(read) = read_biomes(container, data, plains)
            {
                *target = read;
                has_biomes = true;
            }
        }
    }
    let block_entities = match nbt.get("block_entities") {
        Some(Nbt::List(list)) => list
            .iter()
            .filter_map(|entity| BlockEntity::from_nbt(entity.clone(), data))
            .collect(),
        _ => Vec::new(),
    };
    Ok(StoredChunk {
        pos,
        states,
        biomes: has_biomes.then_some(biomes),
        light: light_correct.then_some(light),
        block_entities,
        nbt,
    })
}

/// A section's biomes; unknown names become plains.
fn read_biomes(container: &Nbt, data: &GameData, plains: u32) -> Option<SectionBiomes> {
    let Some(Nbt::List(palette)) = container.get("palette") else {
        return None;
    };
    let palette: Vec<u32> = palette
        .iter()
        .map(|name| match name {
            Nbt::String(name) => data
                .registry_id("minecraft:worldgen/biome", name)
                .map_or(plains, |id| id as u32),
            _ => plains,
        })
        .collect();
    let mut biomes = [*palette.first()?; BIOMES_PER_SECTION];
    let bits = stored_bits(palette.len(), 1);
    if bits == 0 {
        return Some(biomes);
    }
    let Some(Nbt::LongArray(longs)) = container.get("data") else {
        return None;
    };
    let per_long = 64 / usize::from(bits);
    if longs.len() != BIOMES_PER_SECTION.div_ceil(per_long) {
        return None;
    }
    let mask = (1u64 << bits) - 1;
    for (i, biome) in biomes.iter_mut().enumerate() {
        let index = (longs[i / per_long] as u64 >> ((i % per_long) * usize::from(bits))) & mask;
        *biome = *palette.get(index as usize)?;
    }
    Some(biomes)
}

fn read_container(
    container: &Nbt,
    data: &GameData,
    unknown: &mut BTreeMap<String, usize>,
) -> anyhow::Result<[u32; BLOCKS_PER_SECTION]> {
    let Some(Nbt::List(palette)) = container.get("palette") else {
        bail!("block states without a palette");
    };
    let air = data.default_state("minecraft:air")?;
    let palette: Vec<u32> = palette
        .iter()
        .map(|entry| {
            state_from_nbt(entry, data).unwrap_or_else(|| {
                let name = match (entry, entry.get("id"), entry.get("Name")) {
                    (Nbt::String(name), ..) | (_, Some(Nbt::String(name)), _) => name.clone(),
                    (_, _, Some(Nbt::String(name))) => name.clone(),
                    _ => format!("{entry:?}"),
                };
                *unknown.entry(name).or_default() += 1;
                air
            })
        })
        .collect();
    let mut states = [*palette.first().context("empty palette")?; BLOCKS_PER_SECTION];
    let bits = stored_bits(palette.len(), 4);
    if bits == 0 {
        return Ok(states);
    }
    let Some(Nbt::LongArray(longs)) = container.get("data") else {
        bail!("{} palette entries but no data", palette.len());
    };
    let per_long = 64 / usize::from(bits);
    if longs.len() != BLOCKS_PER_SECTION.div_ceil(per_long) {
        bail!("{} longs for {bits} bits per entry", longs.len());
    }
    let mask = (1u64 << bits) - 1;
    for (i, state) in states.iter_mut().enumerate() {
        let long = longs[i / per_long] as u64;
        let index = (long >> ((i % per_long) * usize::from(bits))) & mask;
        *state = *palette
            .get(index as usize)
            .context("data points past the palette")?;
    }
    Ok(states)
}

/// A block state as 26.3 writes it (`BlockState.CODEC`): the default state
/// of a block is just its name, any other state `{id: "minecraft:oak_log",
/// properties: {axis: "z"}}`.
pub fn state_to_nbt(state: u32, data: &GameData) -> Nbt {
    let Some((_, block)) = data.block_of(state) else {
        return Nbt::String("minecraft:air".into());
    };
    if state == block.default {
        return Nbt::String(block.name.clone());
    }
    Nbt::Compound(vec![
        ("id".to_owned(), Nbt::String(block.name.clone())),
        (
            "properties".into(),
            Nbt::Compound(
                block
                    .values(state)
                    .into_iter()
                    .map(|(k, v)| (k.to_owned(), Nbt::String(v.to_owned())))
                    .collect(),
            ),
        ),
    ])
}

/// The state an NBT block state names, in the 26.3 form or the older
/// `{Name, Properties}` one. Properties it does not mention keep their
/// default, like vanilla's lenient codec.
pub fn state_from_nbt(nbt: &Nbt, data: &GameData) -> Option<u32> {
    let (name, properties) = match nbt {
        Nbt::String(name) => (name, None),
        _ => match (nbt.get("id"), nbt.get("Name")) {
            (Some(Nbt::String(name)), _) => (name, nbt.get("properties")),
            (None, Some(Nbt::String(name))) => (name, nbt.get("Properties")),
            _ => return None,
        },
    };
    let block = data.block_named(name)?;
    let mut state = block.default;
    if let Some(Nbt::Compound(properties)) = properties {
        for (key, value) in properties {
            if let Nbt::String(value) = value {
                state = block.with(state, key, value).unwrap_or(state);
            }
        }
    }
    Some(state)
}

/// What is kept of a player between sessions.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerData {
    pub pos: [f64; 3],
    /// Yaw and pitch.
    pub rot: [f32; 2],
    /// Hotbar slot in hand, 0 to 8.
    pub selected: usize,
    /// Stacks by inventory menu slot (9–35 main, 36–44 hotbar, 5–8 armor, 45
    /// off hand).
    pub inventory: Vec<(usize, ItemStack)>,
    /// 0 survival, 1 creative, 2 adventure, 3 spectator.
    pub game_mode: i32,
    /// The ender chest, by slot 0–26.
    pub ender: Vec<(usize, ItemStack)>,
    pub health: f32,
    /// Experience: level, progress to the next, total and score.
    pub xp_level: i32,
    pub xp_progress: f32,
    pub xp_total: i32,
    pub xp_score: i32,
    /// Ticks left burning (`Fire`) and breath left (`Air`).
    pub fire: i32,
    pub air: i32,
}

/// A player's full health.
pub const MAX_HEALTH: f32 = 20.0;
/// A full breath (`Entity.TOTAL_AIR_SUPPLY`).
pub const MAX_AIR: i32 = 300;

/// Inventory menu slots that vanilla saves under `equipment` instead.
const EQUIPMENT_SLOTS: [(usize, &str); 5] = [
    (5, "head"),
    (6, "chest"),
    (7, "legs"),
    (8, "feet"),
    (45, "offhand"),
];

impl PlayerData {
    /// `Player.addAdditionalSaveData`, as far as there is anything to save.
    fn to_nbt(&self, data: &GameData) -> Nbt {
        let mut inventory = Vec::new();
        let mut equipment = Vec::new();
        for (slot, stack) in &self.inventory {
            let Some(mut item) = item_to_nbt(stack, data) else {
                continue;
            };
            if let Some((_, name)) = EQUIPMENT_SLOTS.iter().find(|(s, _)| s == slot) {
                equipment.push((name.to_string(), item));
            } else if let Some(saved) = menu_to_saved_slot(*slot)
                && let Nbt::Compound(entries) = &mut item
            {
                entries.insert(0, ("Slot".into(), Nbt::Byte(saved)));
                inventory.push(item);
            }
        }
        let ender = self
            .ender
            .iter()
            .filter_map(|(slot, stack)| {
                let mut item = item_to_nbt(stack, data)?;
                if let Nbt::Compound(entries) = &mut item {
                    entries.insert(0, ("Slot".into(), Nbt::Byte(*slot as i8)));
                }
                Some(item)
            })
            .collect();
        Nbt::Compound(vec![
            ("DataVersion".into(), Nbt::Int(DATA_VERSION)),
            (
                "Pos".into(),
                Nbt::List(self.pos.iter().map(|&v| Nbt::Double(v)).collect()),
            ),
            (
                "Rotation".into(),
                Nbt::List(self.rot.iter().map(|&v| Nbt::Float(v)).collect()),
            ),
            ("OnGround".into(), Nbt::Byte(1)),
            (
                "Dimension".into(),
                Nbt::String(crate::world::DIMENSION.into()),
            ),
            ("playerGameType".into(), Nbt::Int(self.game_mode)),
            ("SelectedItemSlot".into(), Nbt::Int(self.selected as i32)),
            ("Inventory".into(), Nbt::List(inventory)),
            ("equipment".into(), Nbt::Compound(equipment)),
            ("EnderItems".into(), Nbt::List(ender)),
            ("Health".into(), Nbt::Float(self.health)),
            ("XpLevel".into(), Nbt::Int(self.xp_level)),
            ("XpP".into(), Nbt::Float(self.xp_progress)),
            ("XpTotal".into(), Nbt::Int(self.xp_total)),
            ("Score".into(), Nbt::Int(self.xp_score)),
            (
                "Fire".into(),
                Nbt::Short(self.fire.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16),
            ),
            (
                "Air".into(),
                Nbt::Short(self.air.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16),
            ),
        ])
    }

    fn from_nbt(nbt: &Nbt, data: &GameData) -> Self {
        let doubles = |key| match nbt.get(key) {
            Some(Nbt::List(list)) => list
                .iter()
                .map(|v| match v {
                    Nbt::Double(v) => *v,
                    Nbt::Float(v) => f64::from(*v),
                    _ => 0.0,
                })
                .collect(),
            _ => Vec::new(),
        };
        let pos = doubles("Pos");
        let rot = doubles("Rotation");
        let mut inventory = Vec::new();
        if let Some(Nbt::List(items)) = nbt.get("Inventory") {
            for item in items {
                let Some(Nbt::Byte(saved)) = item.get("Slot") else {
                    continue;
                };
                if let (Some(slot), Some(stack)) =
                    (saved_slot_to_menu(*saved), item_from_nbt(item, data))
                {
                    inventory.push((slot, stack));
                }
            }
        }
        let mut ender = Vec::new();
        if let Some(Nbt::List(items)) = nbt.get("EnderItems") {
            for item in items {
                if let (Some(Nbt::Byte(slot @ 0..=26)), Some(stack)) =
                    (item.get("Slot"), item_from_nbt(item, data))
                {
                    ender.push((*slot as usize, stack));
                }
            }
        }
        for (slot, name) in EQUIPMENT_SLOTS {
            if let Some(stack) = nbt
                .get("equipment")
                .and_then(|e| e.get(name))
                .and_then(|item| item_from_nbt(item, data))
            {
                inventory.push((slot, stack));
            }
        }
        Self {
            pos: match pos[..] {
                [x, y, z] if pos.iter().all(|v| v.is_finite()) => [x, y, z],
                _ => [0.5, f64::from(MIN_Y + 4), 0.5],
            },
            rot: match rot[..] {
                [yaw, pitch] => [yaw as f32, pitch as f32],
                _ => [0.0, 0.0],
            },
            selected: match nbt.get("SelectedItemSlot") {
                Some(Nbt::Int(slot @ 0..=8)) => *slot as usize,
                _ => 0,
            },
            inventory,
            game_mode: match nbt.get("playerGameType") {
                Some(Nbt::Int(mode @ 0..=3)) => *mode,
                _ => 0,
            },
            ender,
            health: match nbt.get("Health") {
                Some(Nbt::Float(health)) if health.is_finite() => health.clamp(0.0, MAX_HEALTH),
                _ => MAX_HEALTH,
            },
            xp_level: int_or_zero(nbt, "XpLevel").max(0),
            xp_progress: match nbt.get("XpP") {
                Some(Nbt::Float(p)) if p.is_finite() => p.clamp(0.0, 1.0),
                _ => 0.0,
            },
            xp_total: int_or_zero(nbt, "XpTotal").max(0),
            xp_score: int_or_zero(nbt, "Score"),
            fire: match nbt.get("Fire") {
                Some(Nbt::Short(fire)) => i32::from(*fire),
                _ => 0,
            },
            air: match nbt.get("Air") {
                Some(Nbt::Short(air)) => i32::from(*air),
                _ => MAX_AIR,
            },
        }
    }
}

fn int_or_zero(nbt: &Nbt, key: &str) -> i32 {
    match nbt.get(key) {
        Some(Nbt::Int(v)) => *v,
        _ => 0,
    }
}

/// Inventory menu slot to the `Slot` vanilla saves: the hotbar is 0–8, the
/// rest of the inventory keeps its number.
fn menu_to_saved_slot(slot: usize) -> Option<i8> {
    match slot {
        36..=44 => Some((slot - 36) as i8),
        9..=35 => Some(slot as i8),
        _ => None,
    }
}

fn saved_slot_to_menu(slot: i8) -> Option<usize> {
    match slot {
        0..=8 => Some(slot as usize + 36),
        9..=35 => Some(slot as usize),
        _ => None,
    }
}

/// An item as vanilla saves it, `{id, count}`. Components are kept in their
/// network form under a key vanilla ignores: turning them into NBT would need
/// a codec for every component type.
pub(crate) fn item_to_nbt(stack: &ItemStack, data: &GameData) -> Option<Nbt> {
    let id = &data.item(stack.item)?.name;
    let mut entries = vec![
        ("id".to_owned(), Nbt::String(id.clone())),
        ("count".into(), Nbt::Int(stack.count)),
    ];
    let patch = &stack.patch;
    if !patch.added.is_empty() || !patch.removed.is_empty() {
        entries.push((
            "ferrocraft:components".into(),
            Nbt::List(
                patch
                    .added
                    .iter()
                    .map(|(kind, bytes)| {
                        Nbt::Compound(vec![
                            ("type".into(), Nbt::Int(*kind)),
                            (
                                "data".into(),
                                Nbt::ByteArray(bytes.iter().map(|&b| b as i8).collect()),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ));
        entries.push((
            "ferrocraft:removed".into(),
            Nbt::IntArray(patch.removed.clone()),
        ));
    }
    Some(Nbt::Compound(entries))
}

pub(crate) fn item_from_nbt(nbt: &Nbt, data: &GameData) -> Option<ItemStack> {
    let Some(Nbt::String(id)) = nbt.get("id") else {
        return None;
    };
    let item = data.item_id(id)?;
    let count = match nbt.get("count") {
        Some(Nbt::Int(count)) if *count > 0 => *count,
        _ => 1,
    };
    let added = match nbt.get("ferrocraft:components") {
        Some(Nbt::List(list)) => list
            .iter()
            .filter_map(|c| match (c.get("type"), c.get("data")) {
                (Some(Nbt::Int(kind)), Some(Nbt::ByteArray(bytes))) => {
                    Some((*kind, bytes.iter().map(|&b| b as u8).collect()))
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let removed = match nbt.get("ferrocraft:removed") {
        Some(Nbt::IntArray(removed)) => removed.clone(),
        _ => Vec::new(),
    };
    Some(ItemStack {
        item,
        count,
        patch: ComponentPatch { added, removed },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ferrocraft-storage-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn mixed_chunk(data: &GameData) -> ChunkStates {
        let air = data.default_state("minecraft:air").unwrap();
        let stone = data.default_state("minecraft:stone").unwrap();
        let log = data.default_state("minecraft:oak_log").unwrap() + 1;
        let mut states: ChunkStates = (0..24)
            .map(|_| Box::new([air; BLOCKS_PER_SECTION]))
            .collect();
        states[0][..256].fill(stone);
        states[4][100] = log;
        // A section with more than 256 distinct states takes 9 bits.
        for (i, state) in states[10].iter_mut().enumerate() {
            *state = (i % 300) as u32;
        }
        states
    }

    #[test]
    fn broken_files_fall_back_to_the_backup() {
        let dir = std::env::temp_dir().join(format!("ferrocraft-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("level.dat");
        let first = Nbt::Compound(vec![("a".into(), Nbt::Int(1))]);
        let second = Nbt::Compound(vec![("a".into(), Nbt::Int(2))]);
        write_nbt_file(&path, &first).unwrap();
        write_nbt_file(&path, &second).unwrap();
        assert_eq!(read_nbt_file(&path).unwrap(), Some(second));
        assert!(dir.join("level.dat_old").exists());
        // A crash left the newest file empty: the one before counts.
        std::fs::write(&path, b"").unwrap();
        assert_eq!(read_nbt_file(&path).unwrap(), Some(first));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn chunks_survive_a_region_file() {
        let data = GameData::load().unwrap();
        let dir = temp_dir("region");
        let storage = Storage::open(&dir).unwrap();
        let states = mixed_chunk(&data);
        let chunks: Vec<_> = [(-1, 3), (-32, 31)]
            .map(|pos| (pos, chunk_to_nbt(pos, &states, Vec::new(), None, &data)))
            .into();
        storage.write_region((-1, 0), &chunks).unwrap();
        let mut loaded = storage.load_chunks(&data).unwrap();
        loaded.sort_by_key(|c| c.pos);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].pos, (-32, 31));
        assert_eq!(loaded[1].pos, (-1, 3));
        assert!(loaded.iter().all(|c| c.states == states));
        assert!(loaded.iter().all(|c| c.light.is_none()), "isLightOn is 0");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn chunk_nbt_follows_vanilla() {
        let data = GameData::load().unwrap();
        let nbt = chunk_to_nbt((2, -3), &mixed_chunk(&data), Vec::new(), None, &data);
        assert_eq!(nbt.get("DataVersion"), Some(&Nbt::Int(5023)));
        assert_eq!(nbt.get("yPos"), Some(&Nbt::Int(-4)));
        let Some(Nbt::List(sections)) = nbt.get("sections") else {
            panic!()
        };
        // Section 4 (Y = 0): air and one log, 4 bits per entry, 256 longs.
        let section = &sections[4];
        assert_eq!(section.get("Y"), Some(&Nbt::Byte(0)));
        let states = section.get("block_states").unwrap();
        let Some(Nbt::List(palette)) = states.get("palette") else {
            panic!()
        };
        // Like a vanilla 26.3 chunk: names for default states, the rest
        // with their properties.
        assert_eq!(palette[0], Nbt::String("minecraft:air".into()));
        assert_eq!(
            palette[1],
            Nbt::Compound(vec![
                ("id".into(), Nbt::String("minecraft:oak_log".into())),
                (
                    "properties".into(),
                    Nbt::Compound(vec![("axis".into(), Nbt::String("z".into()))])
                ),
            ])
        );
        let Some(Nbt::LongArray(longs)) = states.get("data") else {
            panic!()
        };
        assert_eq!(longs.len(), 256);
        assert_eq!(longs[6], 1 << 16, "entry 100 is the 5th nibble of long 6");
        // Air only: no data at all.
        assert!(
            sections[5]
                .get("block_states")
                .unwrap()
                .get("data")
                .is_none()
        );
        // 300 distinct states: 9 bits, as the palette needs.
        let Some(Nbt::LongArray(longs)) = sections[10].get("block_states").unwrap().get("data")
        else {
            panic!()
        };
        assert_eq!(longs.len(), BLOCKS_PER_SECTION.div_ceil(64 / 9));
    }

    #[test]
    fn saving_a_loaded_chunk_keeps_what_ferrocraft_does_not_read() {
        let data = GameData::load().unwrap();
        let states = mixed_chunk(&data);
        let Nbt::Compound(mut original) = chunk_to_nbt((0, 0), &states, Vec::new(), None, &data)
        else {
            panic!()
        };
        original.push(("entities".into(), Nbt::List(vec![Nbt::Int(7)])));
        original.push(("Heightmaps".into(), Nbt::Compound(Vec::new())));
        let original = Nbt::Compound(original);

        let mut changed = states.clone();
        changed[4][0] = data.default_state("minecraft:stone").unwrap();
        let saved = chunk_to_nbt((0, 0), &changed, Vec::new(), Some(&original), &data);
        assert_eq!(saved.get("entities"), original.get("entities"));
        assert!(saved.get("Heightmaps").is_none(), "stale after the change");
        let mut unknown = BTreeMap::new();
        let loaded = chunk_from_nbt(saved, &data, &mut unknown).unwrap();
        assert_eq!(loaded.states, changed);
    }

    #[test]
    fn biomes_and_light_are_read() {
        let data = GameData::load().unwrap();
        let desert = data
            .registry_id("minecraft:worldgen/biome", "minecraft:desert")
            .unwrap();
        // Two biomes: 1 bit per cell, one long; the first cell is desert.
        let section = Nbt::Compound(vec![
            ("Y".into(), Nbt::Byte(-4)),
            (
                "biomes".into(),
                Nbt::Compound(vec![
                    (
                        "palette".into(),
                        Nbt::List(vec![
                            Nbt::String("minecraft:desert".into()),
                            Nbt::String("minecraft:plains".into()),
                        ]),
                    ),
                    ("data".into(), Nbt::LongArray(vec![!1])),
                ]),
            ),
            ("SkyLight".into(), Nbt::ByteArray(vec![-1; 2048])),
        ]);
        let below = Nbt::Compound(vec![
            ("Y".into(), Nbt::Byte(-5)),
            ("SkyLight".into(), Nbt::ByteArray(vec![0; 2048])),
        ]);
        let chunk = Nbt::Compound(vec![
            ("xPos".into(), Nbt::Int(0)),
            ("zPos".into(), Nbt::Int(0)),
            ("isLightOn".into(), Nbt::Byte(1)),
            ("sections".into(), Nbt::List(vec![below, section])),
        ]);
        let loaded = chunk_from_nbt(chunk, &data, &mut BTreeMap::new()).unwrap();
        let biomes = loaded.biomes.unwrap();
        assert_eq!(biomes[0][0], desert as u32);
        assert_ne!(biomes[0][1], desert as u32);
        let light = loaded.light.unwrap();
        assert!(light.empty_sky_mask.get(0) && light.sky_mask.get(1));
        assert_eq!(light.sky_layers.len(), 1);
    }

    #[test]
    fn unknown_blocks_become_air_and_properties_default() {
        let data = GameData::load().unwrap();
        // The pre-26 form, as older worlds have it.
        let container = Nbt::Compound(vec![(
            "palette".into(),
            Nbt::List(vec![Nbt::Compound(vec![(
                "Name".into(),
                Nbt::String("minecraft:oak_log".into()),
            )])]),
        )]);
        let mut unknown = BTreeMap::new();
        let states = read_container(&container, &data, &mut unknown).unwrap();
        assert!(states.iter().all(|&s| s == 140), "axis=y by default");
        let partial = Nbt::Compound(vec![
            ("id".into(), Nbt::String("minecraft:oak_stairs".into())),
            (
                "properties".into(),
                Nbt::Compound(vec![("half".into(), Nbt::String("top".into()))]),
            ),
        ]);
        let stairs = data.block_named("minecraft:oak_stairs").unwrap();
        assert_eq!(
            state_from_nbt(&partial, &data),
            stairs.with(stairs.default, "half", "top")
        );

        let strange = Nbt::Compound(vec![(
            "palette".into(),
            Nbt::List(vec![Nbt::String("mod:thing".into())]),
        )]);
        let states = read_container(&strange, &data, &mut unknown).unwrap();
        assert!(states.iter().all(|&s| s == 0));
        assert_eq!(unknown["mod:thing"], 1);
    }

    #[test]
    fn players_keep_position_and_inventory() {
        let data = GameData::load().unwrap();
        let dir = temp_dir("player");
        let storage = Storage::open(&dir).unwrap();
        let uuid = Uuid::from_u128(7);
        assert_eq!(storage.load_player(uuid, &data).unwrap(), None);
        let stack = |item, count| ItemStack {
            item,
            count,
            patch: ComponentPatch::default(),
        };
        let player = PlayerData {
            pos: [12.5, -40.0, -3.25],
            rot: [91.0, -12.5],
            selected: 4,
            game_mode: 3,
            ender: vec![(26, stack(2, 7))],
            health: 13.5,
            xp_level: 7,
            xp_progress: 0.25,
            xp_total: 91,
            xp_score: 91,
            fire: 37,
            air: 120,
            inventory: vec![
                (36, stack(1, 64)),
                (20, stack(2, 3)),
                (45, stack(1, 1)),
                (
                    5,
                    ItemStack {
                        item: 3,
                        count: 1,
                        patch: ComponentPatch {
                            added: vec![(17, vec![1, 2, 3])],
                            removed: vec![4],
                        },
                    },
                ),
            ],
        };
        storage.save_player(uuid, &player, &data).unwrap();
        let mut loaded = storage.load_player(uuid, &data).unwrap().unwrap();
        loaded.inventory.sort_by_key(|(slot, _)| *slot);
        let mut expected = player.clone();
        expected.inventory.sort_by_key(|(slot, _)| *slot);
        assert_eq!(loaded, expected);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unfinished_chunks_are_left_out() {
        let data = GameData::load().unwrap();
        let dir = temp_dir("unfinished");
        let storage = Storage::open(&dir).unwrap();
        let states = mixed_chunk(&data);
        let Nbt::Compound(mut proto) = chunk_to_nbt((1, 1), &states, Vec::new(), None, &data)
        else {
            panic!()
        };
        for (key, value) in &mut proto {
            if key == "Status" {
                *value = Nbt::String("minecraft:structure_starts".into());
            }
        }
        let chunks = vec![
            (
                (0, 0),
                chunk_to_nbt((0, 0), &states, Vec::new(), None, &data),
            ),
            ((1, 1), Nbt::Compound(proto)),
        ];
        storage.write_region((0, 0), &chunks).unwrap();
        let loaded = storage.load_chunks(&data).unwrap();
        assert_eq!(loaded.iter().map(|c| c.pos).collect::<Vec<_>>(), [(0, 0)]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn region_names() {
        assert_eq!(parse_region_name(Path::new("r.-1.2.mca")), Some((-1, 2)));
        assert_eq!(parse_region_name(Path::new("r.1.2.mcc")), None);
        assert_eq!(region_of((-1, 31)), (-1, 0));
        assert_eq!(region_of((32, -33)), (1, -2));
    }
}
