//! Wire format of chunk data: paletted containers, sections, heightmaps and
//! light. What is in the world is the server's business; this module only
//! knows how vanilla lays it out (`LevelChunkSection.write`,
//! `PalettedContainer.Data.write`, `ClientboundLightUpdatePacketData`).

use crate::codec::{Decode, Encode, Error, Result, VarInt, decode_byte_array, decode_count};

/// Blocks per section (16³) and biome cells per section (4³).
pub const BLOCKS_PER_SECTION: usize = 4096;
pub const BIOMES_PER_SECTION: usize = 64;
/// Bytes of one light layer: 4096 nibbles.
pub const LIGHT_LAYER_LEN: usize = 2048;

/// Which palette rules a container follows (`PalettedContainer.Strategy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    /// Indirect palettes of 4–8 bits, beyond that global ids.
    BlockStates { global_bits: u8 },
    /// Indirect palettes of 1–3 bits, beyond that global ids.
    Biomes { global_bits: u8 },
}

impl ContainerKind {
    fn len(self) -> usize {
        match self {
            ContainerKind::BlockStates { .. } => BLOCKS_PER_SECTION,
            ContainerKind::Biomes { .. } => BIOMES_PER_SECTION,
        }
    }

    /// Bits per entry for a palette of `distinct` values, `None` for global ids.
    fn indirect_bits(self, distinct: usize) -> Option<u8> {
        let needed = bits_for(distinct.saturating_sub(1) as u64);
        match self {
            ContainerKind::BlockStates { .. } if needed <= 8 => Some(needed.max(4)),
            ContainerKind::Biomes { .. } if needed <= 3 => Some(needed.max(1)),
            _ => None,
        }
    }

    fn global_bits(self) -> u8 {
        match self {
            ContainerKind::BlockStates { global_bits } | ContainerKind::Biomes { global_bits } => {
                global_bits
            }
        }
    }
}

/// Number of bits needed to store `max` (`Mth.ceillog2(max + 1)`).
pub fn bits_for(max: u64) -> u8 {
    (u64::BITS - max.leading_zeros()) as u8
}

/// Bits per global id for a registry of `len` entries.
pub fn global_bits(len: usize) -> u8 {
    bits_for(len.saturating_sub(1) as u64)
}

/// Packs values into longs the way `SimpleBitStorage` does: entries never span
/// two longs, so the top bits of each long may stay unused.
pub fn pack_bits(values: impl ExactSizeIterator<Item = u64>, bits: u8) -> Vec<i64> {
    let per_long = 64 / usize::from(bits);
    let mut longs = vec![0i64; values.len().div_ceil(per_long)];
    for (i, value) in values.enumerate() {
        let shift = (i % per_long) * usize::from(bits);
        longs[i / per_long] |= (value << shift) as i64;
    }
    longs
}

/// Encodes one paletted container with the smallest palette vanilla would pick.
/// Palette entries keep the order of first appearance.
pub fn encode_container(kind: ContainerKind, ids: &[u32], out: &mut Vec<u8>) {
    assert_eq!(ids.len(), kind.len(), "container has the wrong size");
    let mut palette: Vec<u32> = Vec::new();
    for &id in ids {
        if !palette.contains(&id) {
            palette.push(id);
        }
    }

    if let [only] = palette[..] {
        out.push(0);
        VarInt(only as i32).encode(out);
        return;
    }

    let longs = match kind.indirect_bits(palette.len()) {
        Some(bits) => {
            out.push(bits);
            VarInt(palette.len() as i32).encode(out);
            palette.iter().for_each(|&id| VarInt(id as i32).encode(out));
            let index = |id: &u32| palette.iter().position(|p| p == id).unwrap() as u64;
            pack_bits(ids.iter().map(index), bits)
        }
        None => {
            let bits = kind.global_bits();
            out.push(bits);
            pack_bits(ids.iter().map(|&id| u64::from(id)), bits)
        }
    };
    // Since 1.21.5 the long array has no length prefix: its size follows from
    // the bit count.
    longs.iter().for_each(|l| l.encode(out));
}

/// One 16×16×16 section as `LevelChunkSection.write` lays it out.
pub fn encode_section(
    non_empty_blocks: u16,
    fluids: u16,
    states: (ContainerKind, &[u32]),
    biomes: (ContainerKind, &[u32]),
    out: &mut Vec<u8>,
) {
    non_empty_blocks.encode(out);
    fluids.encode(out);
    encode_container(states.0, states.1, out);
    encode_container(biomes.0, biomes.1, out);
}

/// `java.util.BitSet` on the wire: `toByteArray()` with a `VarInt`
/// length, little-endian and without trailing zero bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BitSet(Vec<u8>);

impl BitSet {
    pub fn set(&mut self, index: usize) {
        let byte = index / 8;
        if self.0.len() <= byte {
            self.0.resize(byte + 1, 0);
        }
        self.0[byte] |= 1 << (index % 8);
    }

    pub fn get(&self, index: usize) -> bool {
        self.0
            .get(index / 8)
            .is_some_and(|b| b & (1 << (index % 8)) != 0)
    }
}

impl Encode for BitSet {
    fn encode(&self, out: &mut Vec<u8>) {
        let used = self.0.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
        crate::codec::encode_byte_array(&self.0[..used], out);
    }
}

impl Decode for BitSet {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        decode_byte_array(buf, 1 << 16).map(BitSet)
    }
}

/// Light of one chunk column, one entry per light section (the chunk's
/// sections plus one below and one above).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LightData {
    pub sky_mask: BitSet,
    pub block_mask: BitSet,
    pub empty_sky_mask: BitSet,
    pub empty_block_mask: BitSet,
    /// One 2048-byte layer per bit set in `sky_mask`, lowest section first.
    pub sky_layers: Vec<Vec<u8>>,
    pub block_layers: Vec<Vec<u8>>,
}

impl Encode for LightData {
    fn encode(&self, out: &mut Vec<u8>) {
        self.sky_mask.encode(out);
        self.block_mask.encode(out);
        self.empty_sky_mask.encode(out);
        self.empty_block_mask.encode(out);
        for layers in [&self.sky_layers, &self.block_layers] {
            VarInt(layers.len() as i32).encode(out);
            for layer in layers {
                crate::codec::encode_byte_array(layer, out);
            }
        }
    }
}

impl Decode for LightData {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let sky_mask = BitSet::decode(buf)?;
        let block_mask = BitSet::decode(buf)?;
        let empty_sky_mask = BitSet::decode(buf)?;
        let empty_block_mask = BitSet::decode(buf)?;
        let mut layers = || -> Result<Vec<Vec<u8>>> {
            let count = decode_count(buf, 4096)?;
            (0..count)
                .map(|_| decode_byte_array(buf, LIGHT_LAYER_LEN))
                .collect()
        };
        let sky_layers = layers()?;
        let block_layers = layers()?;
        Ok(Self {
            sky_mask,
            block_mask,
            empty_sky_mask,
            empty_block_mask,
            sky_layers,
            block_layers,
        })
    }
}

/// Heightmap kinds the client receives (`Heightmap.Types` with `sendToClient`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeightmapKind {
    WorldSurface = 1,
    MotionBlocking = 4,
    MotionBlockingNoLeaves = 5,
}

/// One heightmap: per column the height of the highest matching block above
/// the world's bottom plus one, packed like any bit storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heightmap {
    pub kind: i32,
    pub data: Vec<i64>,
}

impl Heightmap {
    /// `heights` are 256 values in `z * 16 + x` order; `world_height` decides
    /// the bit width.
    pub fn new(kind: HeightmapKind, heights: &[u32; 256], world_height: u32) -> Self {
        let bits = bits_for(u64::from(world_height));
        Self {
            kind: kind as i32,
            data: pack_bits(heights.iter().map(|&h| u64::from(h)), bits),
        }
    }
}

impl Encode for Heightmap {
    fn encode(&self, out: &mut Vec<u8>) {
        VarInt(self.kind).encode(out);
        self.data.encode(out);
    }
}

impl Decode for Heightmap {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let kind = VarInt::decode(buf)?.0;
        if !(0..=5).contains(&kind) {
            return Err(Error::Invalid("unknown heightmap type"));
        }
        let data = crate::codec::decode_list(buf, 1024)?;
        Ok(Self { kind, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCKS: ContainerKind = ContainerKind::BlockStates { global_bits: 15 };
    const BIOMES: ContainerKind = ContainerKind::Biomes { global_bits: 7 };

    #[test]
    fn single_value_container() {
        let mut out = Vec::new();
        encode_container(BIOMES, &[41; 64], &mut out);
        assert_eq!(out, [0, 41], "bits 0, the value, no data longs");
    }

    #[test]
    fn indirect_container_uses_at_least_four_bits_for_blocks() {
        let mut ids = [0u32; BLOCKS_PER_SECTION];
        ids[0] = 88;
        let mut out = Vec::new();
        encode_container(BLOCKS, &ids, &mut out);
        // bits, palette [88, 0], then 4096 / 16 longs.
        assert_eq!(&out[..4], [4, 2, 88, 0]);
        assert_eq!(out.len(), 4 + 256 * 8);
        // Block 0 is palette index 0 (bedrock) in the lowest nibble, the rest air.
        assert_eq!(&out[4..12], 0x1111_1111_1111_1110i64.to_be_bytes());
        assert_eq!(&out[12..20], 0x1111_1111_1111_1111i64.to_be_bytes());
    }

    #[test]
    fn entries_do_not_span_longs() {
        // 5 bits: 12 entries per long, the top 4 bits unused.
        let longs = pack_bits([31u64; 13].into_iter(), 5);
        assert_eq!(longs, [(1i64 << 60) - 1, 31]);
    }

    #[test]
    fn many_distinct_blocks_fall_back_to_global_ids() {
        let ids: Vec<u32> = (0..BLOCKS_PER_SECTION as u32).collect();
        let mut out = Vec::new();
        encode_container(BLOCKS, &ids, &mut out);
        assert_eq!(out[0], 15, "global bit width, no palette follows");
        assert_eq!(out.len(), 1 + BLOCKS_PER_SECTION.div_ceil(64 / 15) * 8);
    }

    #[test]
    fn bitset_matches_java_to_byte_array() {
        let mut bits = BitSet::default();
        bits.set(1);
        bits.set(2);
        let mut out = Vec::new();
        bits.encode(&mut out);
        assert_eq!(out, [1, 0x06]);

        out.clear();
        BitSet::default().encode(&mut out);
        assert_eq!(out, [0]);
        assert!(bits.get(2) && !bits.get(3) && !bits.get(100));
    }

    #[test]
    fn flat_heightmap_matches_vanilla() {
        // Superflat, top block at y = -61 in a world from -64 to 320: height 4
        // in every column, 9 bits each, 7 per long.
        let map = Heightmap::new(HeightmapKind::WorldSurface, &[4; 256], 384);
        assert_eq!(map.data.len(), 37);
        assert_eq!(map.data[0], 0x0100_8040_2010_0804);
    }
}
