//! Play state. This module has the packets for joining a world, chunks and
//! keep-alives; the submodules the rest, re-exported here.

mod block;
mod chat;
mod command;
mod container;
mod effect;
mod entity;
mod survival;

pub use block::*;
pub use chat::*;
pub use command::*;
pub use container::*;
pub use effect::*;
pub use entity::*;
pub use survival::*;

use super::{Packet, empty_packet};
use crate::chunk::{Heightmap, LightData};
use crate::codec::{
    BlockPos, Decode, Encode, Error, Result, VarInt, decode_byte_array, decode_count, decode_list,
};
use crate::nbt::Nbt;
use crate::packets::login::GameProfile;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameMode {
    Survival = 0,
    Creative = 1,
    Adventure = 2,
    Spectator = 3,
}

impl GameMode {
    pub fn from_id(id: i32) -> Result<Self> {
        Ok(match id {
            0 => GameMode::Survival,
            1 => GameMode::Creative,
            2 => GameMode::Adventure,
            3 => GameMode::Spectator,
            _ => return Err(Error::Invalid("unknown game mode")),
        })
    }
}

/// What a player needs to know about the dimension they spawn in
/// (`CommonPlayerSpawnInfo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnInfo {
    /// Network id in the `minecraft:dimension_type` registry.
    pub dimension_type: i32,
    pub dimension: String,
    /// First eight bytes of the SHA-256 of the world seed, for biome noise.
    pub hashed_seed: i64,
    pub game_mode: GameMode,
    /// Sent as `VarInt` id + 1, with 0 for none.
    pub previous_game_mode: Option<GameMode>,
    pub debug: bool,
    pub flat: bool,
    pub death_location: Option<(String, BlockPos)>,
    pub portal_cooldown: i32,
    pub sea_level: i32,
}

impl Encode for SpawnInfo {
    fn encode(&self, out: &mut Vec<u8>) {
        VarInt(self.dimension_type).encode(out);
        self.dimension.encode(out);
        self.hashed_seed.encode(out);
        VarInt(self.game_mode as i32).encode(out);
        VarInt(self.previous_game_mode.map_or(0, |m| m as i32 + 1)).encode(out);
        self.debug.encode(out);
        self.flat.encode(out);
        self.death_location.is_some().encode(out);
        if let Some((dimension, pos)) = &self.death_location {
            dimension.encode(out);
            pos.encode(out);
        }
        VarInt(self.portal_cooldown).encode(out);
        VarInt(self.sea_level).encode(out);
    }
}

impl Decode for SpawnInfo {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            dimension_type: VarInt::decode(buf)?.0,
            dimension: String::decode(buf)?,
            hashed_seed: i64::decode(buf)?,
            game_mode: GameMode::from_id(VarInt::decode(buf)?.0)?,
            previous_game_mode: match VarInt::decode(buf)?.0 {
                0 => None,
                id => Some(GameMode::from_id(id - 1)?),
            },
            debug: bool::decode(buf)?,
            flat: bool::decode(buf)?,
            death_location: if bool::decode(buf)? {
                Some((String::decode(buf)?, BlockPos::decode(buf)?))
            } else {
                None
            },
            portal_cooldown: VarInt::decode(buf)?.0,
            sea_level: VarInt::decode(buf)?.0,
        })
    }
}

/// `minecraft:login`, clientbound: the first play packet, puts the player into
/// a dimension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Login {
    pub entity_id: i32,
    pub hardcore: bool,
    pub dimensions: Vec<String>,
    pub max_players: i32,
    pub view_distance: i32,
    pub simulation_distance: i32,
    pub reduced_debug_info: bool,
    pub show_death_screen: bool,
    pub limited_crafting: bool,
    pub spawn: SpawnInfo,
    /// Whether the server authenticates players with Mojang.
    pub online_mode: bool,
    pub enforces_secure_chat: bool,
}

impl Packet for Login {
    const ID: i32 = 0x32;
}

impl Encode for Login {
    fn encode(&self, out: &mut Vec<u8>) {
        self.entity_id.encode(out);
        self.hardcore.encode(out);
        self.dimensions.encode(out);
        VarInt(self.max_players).encode(out);
        VarInt(self.view_distance).encode(out);
        VarInt(self.simulation_distance).encode(out);
        self.reduced_debug_info.encode(out);
        self.show_death_screen.encode(out);
        self.limited_crafting.encode(out);
        self.spawn.encode(out);
        self.online_mode.encode(out);
        self.enforces_secure_chat.encode(out);
    }
}

impl Decode for Login {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            entity_id: i32::decode(buf)?,
            hardcore: bool::decode(buf)?,
            dimensions: decode_list(buf, usize::MAX)?,
            max_players: VarInt::decode(buf)?.0,
            view_distance: VarInt::decode(buf)?.0,
            simulation_distance: VarInt::decode(buf)?.0,
            reduced_debug_info: bool::decode(buf)?,
            show_death_screen: bool::decode(buf)?,
            limited_crafting: bool::decode(buf)?,
            spawn: SpawnInfo::decode(buf)?,
            online_mode: bool::decode(buf)?,
            enforces_secure_chat: bool::decode(buf)?,
        })
    }
}

/// `minecraft:disconnect`, clientbound: an NBT text component, like in
/// configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Disconnect {
    pub reason: Nbt,
}

impl Packet for Disconnect {
    const ID: i32 = 0x20;
}

impl Encode for Disconnect {
    fn encode(&self, out: &mut Vec<u8>) {
        self.reason.encode(out);
    }
}

impl Decode for Disconnect {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            reason: Nbt::decode(buf)?,
        })
    }
}

/// `minecraft:keep_alive`: the client echoes the id, or is kicked after 15 s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientboundKeepAlive {
    pub id: i64,
}

impl Packet for ClientboundKeepAlive {
    const ID: i32 = 0x2d;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundKeepAlive {
    pub id: i64,
}

impl Packet for ServerboundKeepAlive {
    const ID: i32 = 0x1c;
}

super::configuration::keep_alive_codec!(ClientboundKeepAlive, ServerboundKeepAlive);

/// Events of `minecraft:game_event` this server sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameEventKind {
    StartRaining = 1,
    StopRaining = 2,
    ChangeGameMode = 3,
    /// How strong the rain is, 0 to 1.
    RainLevelChange = 7,
    ThunderLevelChange = 8,
    /// Ends the "waiting for server" stage of the client's loading screen;
    /// it then waits for the chunk the player stands in.
    LevelChunksLoadStart = 13,
}

/// `minecraft:game_event`, clientbound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GameEvent {
    pub event: u8,
    pub param: f32,
}

impl GameEvent {
    pub fn new(event: GameEventKind, param: f32) -> Self {
        Self {
            event: event as u8,
            param,
        }
    }
}

impl Packet for GameEvent {
    const ID: i32 = 0x27;
}

impl Encode for GameEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        self.event.encode(out);
        self.param.encode(out);
    }
}

impl Decode for GameEvent {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            event: u8::decode(buf)?,
            param: f32::decode(buf)?,
        })
    }
}

/// `minecraft:set_chunk_cache_center`, clientbound: the chunk the client's
/// view area is centred on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetChunkCacheCenter {
    pub x: i32,
    pub z: i32,
}

impl Packet for SetChunkCacheCenter {
    const ID: i32 = 0x60;
}

impl Encode for SetChunkCacheCenter {
    fn encode(&self, out: &mut Vec<u8>) {
        VarInt(self.x).encode(out);
        VarInt(self.z).encode(out);
    }
}

impl Decode for SetChunkCacheCenter {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            x: VarInt::decode(buf)?.0,
            z: VarInt::decode(buf)?.0,
        })
    }
}

/// `minecraft:set_default_spawn_position`, clientbound: where the compass
/// points and respawns happen.
#[derive(Debug, Clone, PartialEq)]
pub struct SetDefaultSpawnPosition {
    pub dimension: String,
    pub pos: BlockPos,
    pub yaw: f32,
    pub pitch: f32,
}

impl Packet for SetDefaultSpawnPosition {
    const ID: i32 = 0x63;
}

impl Encode for SetDefaultSpawnPosition {
    fn encode(&self, out: &mut Vec<u8>) {
        self.dimension.encode(out);
        self.pos.encode(out);
        self.yaw.encode(out);
        self.pitch.encode(out);
    }
}

impl Decode for SetDefaultSpawnPosition {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            dimension: String::decode(buf)?,
            pos: BlockPos::decode(buf)?,
            yaw: f32::decode(buf)?,
            pitch: f32::decode(buf)?,
        })
    }
}

/// `minecraft:player_abilities`, clientbound. Flags: 1 invulnerable, 2
/// flying, 4 may fly, 8 instant build.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerAbilities {
    pub flags: u8,
    pub flying_speed: f32,
    pub walking_speed: f32,
}

impl PlayerAbilities {
    /// What vanilla sends for a creative player.
    pub fn creative() -> Self {
        Self {
            flags: 1 | 4 | 8,
            flying_speed: 0.05,
            walking_speed: 0.1,
        }
    }
}

impl Packet for PlayerAbilities {
    const ID: i32 = 0x41;
}

impl Encode for PlayerAbilities {
    fn encode(&self, out: &mut Vec<u8>) {
        self.flags.encode(out);
        self.flying_speed.encode(out);
        self.walking_speed.encode(out);
    }
}

impl Decode for PlayerAbilities {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            flags: u8::decode(buf)?,
            flying_speed: f32::decode(buf)?,
            walking_speed: f32::decode(buf)?,
        })
    }
}

/// One player in `minecraft:player_info_update` with every action set: what
/// vanilla sends for a player who just joined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfoEntry {
    pub profile: GameProfile,
    pub game_mode: GameMode,
    /// Shown in the tab list.
    pub listed: bool,
    pub latency: i32,
    pub list_order: i32,
    pub show_hat: bool,
}

/// `minecraft:player_info_update`, clientbound, limited to adding players.
/// The client reads its own game mode from its entry, so every player needs
/// one for itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfoUpdate {
    pub entries: Vec<PlayerInfoEntry>,
}

/// All eight `Action`s, as a fixed-size bit set of one byte.
const ALL_PLAYER_INFO_ACTIONS: u8 = 0xff;

impl Packet for PlayerInfoUpdate {
    const ID: i32 = 0x47;
}

impl Encode for PlayerInfoUpdate {
    fn encode(&self, out: &mut Vec<u8>) {
        ALL_PLAYER_INFO_ACTIONS.encode(out);
        VarInt(self.entries.len() as i32).encode(out);
        for entry in &self.entries {
            // In `Action` order: add player, chat session, game mode, listed,
            // latency, display name, list order, hat.
            entry.profile.uuid.encode(out);
            entry.profile.name.encode(out);
            entry.profile.properties.encode(out);
            false.encode(out);
            VarInt(entry.game_mode as i32).encode(out);
            entry.listed.encode(out);
            VarInt(entry.latency).encode(out);
            false.encode(out);
            VarInt(entry.list_order).encode(out);
            entry.show_hat.encode(out);
        }
    }
}

impl Decode for PlayerInfoUpdate {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        if u8::decode(buf)? != ALL_PLAYER_INFO_ACTIONS {
            return Err(Error::Invalid(
                "only player_info_update with all actions is supported",
            ));
        }
        let count = decode_count(buf, usize::MAX)?;
        let entries = (0..count)
            .map(|_| {
                let uuid = uuid::Uuid::decode(buf)?;
                let name = crate::codec::decode_string(buf, 16)?;
                let properties = decode_list(buf, 16)?;
                if bool::decode(buf)? {
                    return Err(Error::Invalid("chat sessions are not supported"));
                }
                let game_mode = GameMode::from_id(VarInt::decode(buf)?.0)?;
                let listed = bool::decode(buf)?;
                let latency = VarInt::decode(buf)?.0;
                if bool::decode(buf)? {
                    return Err(Error::Invalid("display names are not supported"));
                }
                Ok(PlayerInfoEntry {
                    profile: GameProfile {
                        uuid,
                        name,
                        properties,
                    },
                    game_mode,
                    listed,
                    latency,
                    list_order: VarInt::decode(buf)?.0,
                    show_hat: bool::decode(buf)?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Self { entries })
    }
}

/// `minecraft:player_position`, clientbound: teleports the player. The client
/// confirms with [`AcceptTeleportation`] carrying the same id.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerPosition {
    pub teleport_id: i32,
    pub pos: [f64; 3],
    pub delta: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    /// Bit set of `Relative` flags; 0 means every value is absolute.
    pub relatives: i32,
}

impl Packet for PlayerPosition {
    const ID: i32 = 0x49;
}

impl Encode for PlayerPosition {
    fn encode(&self, out: &mut Vec<u8>) {
        VarInt(self.teleport_id).encode(out);
        self.pos.iter().for_each(|v| v.encode(out));
        self.delta.iter().for_each(|v| v.encode(out));
        self.yaw.encode(out);
        self.pitch.encode(out);
        self.relatives.encode(out);
    }
}

impl Decode for PlayerPosition {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            teleport_id: VarInt::decode(buf)?.0,
            pos: decode_vec3(buf)?,
            delta: decode_vec3(buf)?,
            yaw: f32::decode(buf)?,
            pitch: f32::decode(buf)?,
            relatives: i32::decode(buf)?,
        })
    }
}

fn decode_vec3(buf: &mut &[u8]) -> Result<[f64; 3]> {
    <[f64; 3]>::decode(buf)
}

/// Most chunk data vanilla accepts in one packet.
const MAX_CHUNK_DATA: usize = 2 * 1024 * 1024;

/// A block entity in a chunk packet: where it is within the chunk, its type
/// and what the client should know of it (`BlockEntityInfo`).
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkBlockEntity {
    /// `x << 4 | z`, both within the chunk.
    pub packed_xz: u8,
    pub y: i16,
    /// Network id in `minecraft:block_entity_type`.
    pub kind: i32,
    pub tag: Option<Nbt>,
}

impl Encode for ChunkBlockEntity {
    fn encode(&self, out: &mut Vec<u8>) {
        self.packed_xz.encode(out);
        self.y.encode(out);
        VarInt(self.kind).encode(out);
        match &self.tag {
            Some(tag) => tag.encode(out),
            // `FriendlyByteBuf.writeNbt(null)`: an End tag.
            None => out.push(0),
        }
    }
}

impl Decode for ChunkBlockEntity {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let packed_xz = u8::decode(buf)?;
        let y = i16::decode(buf)?;
        let kind = VarInt::decode(buf)?.0;
        let tag = if buf.first() == Some(&0) {
            *buf = &buf[1..];
            None
        } else {
            Some(Nbt::decode(buf)?)
        };
        Ok(Self {
            packed_xz,
            y,
            kind,
            tag,
        })
    }
}

/// `minecraft:level_chunk_with_light`, clientbound: one chunk column.
/// `sections` holds the encoded sections from bottom to top (see
/// [`crate::chunk::encode_section`]).
#[derive(Debug, Clone, PartialEq)]
pub struct LevelChunkWithLight {
    pub x: i32,
    pub z: i32,
    pub heightmaps: Vec<Heightmap>,
    pub sections: Vec<u8>,
    pub block_entities: Vec<ChunkBlockEntity>,
    pub light: LightData,
}

impl Packet for LevelChunkWithLight {
    const ID: i32 = 0x2e;
}

impl Encode for LevelChunkWithLight {
    fn encode(&self, out: &mut Vec<u8>) {
        self.x.encode(out);
        self.z.encode(out);
        self.heightmaps.encode(out);
        crate::codec::encode_byte_array(&self.sections, out);
        self.block_entities.encode(out);
        self.light.encode(out);
    }
}

impl Decode for LevelChunkWithLight {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let x = i32::decode(buf)?;
        let z = i32::decode(buf)?;
        let heightmaps = decode_list(buf, 16)?;
        let sections = decode_byte_array(buf, MAX_CHUNK_DATA)?;
        let max = buf.len();
        let block_entities = decode_list(buf, max)?;
        Ok(Self {
            x,
            z,
            heightmaps,
            sections,
            block_entities,
            light: LightData::decode(buf)?,
        })
    }
}

/// One world clock's state (`ClockNetworkState`): ticks so far and how fast
/// it runs; the client keeps it going between updates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClockState {
    pub total_ticks: i64,
    pub partial_tick: f32,
    pub rate: f32,
}

/// `minecraft:set_time`, clientbound: the game time and, for a full update,
/// the world clocks by `minecraft:world_clock` id. Vanilla sends the game
/// time alone every second.
#[derive(Debug, Clone, PartialEq)]
pub struct SetTime {
    pub game_time: i64,
    pub clocks: Vec<(i32, ClockState)>,
}

impl Packet for SetTime {
    const ID: i32 = 0x73;
}

impl Encode for SetTime {
    fn encode(&self, out: &mut Vec<u8>) {
        self.game_time.encode(out);
        VarInt(self.clocks.len() as i32).encode(out);
        for (clock, state) in &self.clocks {
            VarInt(*clock).encode(out);
            crate::codec::VarLong(state.total_ticks).encode(out);
            state.partial_tick.encode(out);
            state.rate.encode(out);
        }
    }
}

impl Decode for SetTime {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let game_time = i64::decode(buf)?;
        let count = decode_count(buf, 64)?;
        let clocks = (0..count)
            .map(|_| {
                Ok((
                    VarInt::decode(buf)?.0,
                    ClockState {
                        total_ticks: crate::codec::VarLong::decode(buf)?.0,
                        partial_tick: f32::decode(buf)?,
                        rate: f32::decode(buf)?,
                    },
                ))
            })
            .collect::<Result<_>>()?;
        Ok(Self { game_time, clocks })
    }
}

super::packet! {
    /// `minecraft:light_update`, clientbound: new light for some sections of
    /// a chunk the client has; sections in neither mask stay as they are.
    LightUpdate = 0x31 {
        x: i32 as varint,
        z: i32 as varint,
        light: LightData,
    }
}

/// `minecraft:forget_level_chunk`, clientbound: the client drops a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForgetLevelChunk {
    pub x: i32,
    pub z: i32,
}

impl Packet for ForgetLevelChunk {
    const ID: i32 = 0x26;
}

/// `ChunkPos.pack`: x in the low 32 bits, z in the high ones.
impl Encode for ForgetLevelChunk {
    fn encode(&self, out: &mut Vec<u8>) {
        let packed = i64::from(self.x as u32) | i64::from(self.z as u32) << 32;
        packed.encode(out);
    }
}

impl Decode for ForgetLevelChunk {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let packed = i64::decode(buf)?;
        Ok(Self {
            x: packed as i32,
            z: (packed >> 32) as i32,
        })
    }
}

empty_packet! {
    /// `minecraft:chunk_batch_start`, clientbound: chunks follow.
    ChunkBatchStart = 0x0c
}

/// `minecraft:chunk_batch_finished`, clientbound: how many chunks the batch
/// had. The client answers with [`ChunkBatchReceived`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkBatchFinished {
    pub batch_size: i32,
}

impl Packet for ChunkBatchFinished {
    const ID: i32 = 0x0b;
}

impl Encode for ChunkBatchFinished {
    fn encode(&self, out: &mut Vec<u8>) {
        VarInt(self.batch_size).encode(out);
    }
}

impl Decode for ChunkBatchFinished {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            batch_size: VarInt::decode(buf)?.0,
        })
    }
}

/// `minecraft:accept_teleportation`, serverbound. Besides the id, the client
/// reports where the teleport put it.
#[derive(Debug, Clone, PartialEq)]
pub struct AcceptTeleportation {
    pub teleport_id: i32,
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
}

impl Packet for AcceptTeleportation {
    const ID: i32 = 0x00;
}

impl Encode for AcceptTeleportation {
    fn encode(&self, out: &mut Vec<u8>) {
        VarInt(self.teleport_id).encode(out);
        self.pos.iter().for_each(|v| v.encode(out));
        self.yaw.encode(out);
        self.pitch.encode(out);
    }
}

impl Decode for AcceptTeleportation {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            teleport_id: VarInt::decode(buf)?.0,
            pos: decode_vec3(buf)?,
            yaw: f32::decode(buf)?,
            pitch: f32::decode(buf)?,
        })
    }
}

/// `minecraft:chunk_batch_received`, serverbound: how many chunks per tick the
/// client would like.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChunkBatchReceived {
    pub desired_chunks_per_tick: f32,
}

impl Packet for ChunkBatchReceived {
    const ID: i32 = 0x0b;
}

impl Encode for ChunkBatchReceived {
    fn encode(&self, out: &mut Vec<u8>) {
        self.desired_chunks_per_tick.encode(out);
    }
}

impl Decode for ChunkBatchReceived {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            desired_chunks_per_tick: f32::decode(buf)?,
        })
    }
}

/// `minecraft:client_information` in play: the client changed its settings,
/// e.g. the skin layers or the view distance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInformation(pub crate::packets::configuration::ClientInformation);

impl Packet for ClientInformation {
    const ID: i32 = 0x0e;
}

impl Encode for ClientInformation {
    fn encode(&self, out: &mut Vec<u8>) {
        self.0.encode(out);
    }
}

impl Decode for ClientInformation {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        crate::packets::configuration::ClientInformation::decode(buf).map(Self)
    }
}

empty_packet! {
    /// `minecraft:client_tick_end`, serverbound: sent after every client tick.
    ClientTickEnd = 0x0d
}

empty_packet! {
    /// `minecraft:player_loaded`, serverbound: the loading screen is gone.
    PlayerLoaded = 0x2c
}

/// The `minecraft:move_player_*` packets, serverbound. Each carries a flag byte
/// (bit 0 on ground, bit 1 horizontal collision) and some of position and
/// rotation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovePlayer {
    pub pos: Option<[f64; 3]>,
    pub rot: Option<[f32; 2]>,
    pub flags: u8,
}

/// Packet ids of the four movement variants.
pub const MOVE_PLAYER_POS: i32 = 0x1e;
pub const MOVE_PLAYER_POS_ROT: i32 = 0x1f;
pub const MOVE_PLAYER_ROT: i32 = 0x20;
pub const MOVE_PLAYER_STATUS_ONLY: i32 = 0x21;

impl MovePlayer {
    /// Decodes whichever variant `id` names, `None` if it is none of them.
    pub fn decode_variant(id: i32, mut buf: &[u8]) -> Option<Result<Self>> {
        let (has_pos, has_rot) = match id {
            MOVE_PLAYER_POS => (true, false),
            MOVE_PLAYER_POS_ROT => (true, true),
            MOVE_PLAYER_ROT => (false, true),
            MOVE_PLAYER_STATUS_ONLY => (false, false),
            _ => return None,
        };
        let mut decode = || -> Result<Self> {
            let pos = has_pos.then(|| decode_vec3(&mut buf)).transpose()?;
            let rot = has_rot
                .then(|| Ok::<_, Error>([f32::decode(&mut buf)?, f32::decode(&mut buf)?]))
                .transpose()?;
            let flags = u8::decode(&mut buf)?;
            if !buf.is_empty() {
                return Err(Error::Invalid("trailing bytes after packet"));
            }
            Ok(Self { pos, rot, flags })
        };
        Some(decode())
    }

    /// Encodes the variant that matches which fields are set, as a test client
    /// would send it. Returns the packet id.
    pub fn encode_variant(&self, out: &mut Vec<u8>) -> i32 {
        if let Some(pos) = self.pos {
            pos.iter().for_each(|v| v.encode(out));
        }
        if let Some(rot) = self.rot {
            rot.iter().for_each(|v| v.encode(out));
        }
        self.flags.encode(out);
        match (self.pos.is_some(), self.rot.is_some()) {
            (true, false) => MOVE_PLAYER_POS,
            (true, true) => MOVE_PLAYER_POS_ROT,
            (false, true) => MOVE_PLAYER_ROT,
            (false, false) => MOVE_PLAYER_STATUS_ONLY,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn login_decodes_vanilla_capture() {
        // `minecraft:login` from a vanilla 26.3 superflat world, offline mode.
        let bytes = hex(concat!(
            "00000001",
            "00",
            "03",
            "136d696e6563726166743a6f766572776f726c64",
            "116d696e6563726166743a7468655f656e64",
            "146d696e6563726166743a7468655f6e6574686572",
            "14",
            "0a",
            "0a",
            "00",
            "01",
            "00",
            "00",
            "136d696e6563726166743a6f766572776f726c64",
            "55f13fc7033cb93c",
            "00",
            "00",
            "00",
            "01",
            "00",
            "00",
            "c1ffffff0f",
            "00",
            "00",
        ));
        let login = Login::decode(&mut bytes.as_slice()).unwrap();
        assert_eq!(login.entity_id, 1);
        assert_eq!(login.dimensions.len(), 3);
        assert_eq!(login.view_distance, 10);
        assert_eq!(login.spawn.game_mode, GameMode::Survival);
        assert_eq!(login.spawn.previous_game_mode, None);
        assert!(login.spawn.flat);
        assert_eq!(login.spawn.sea_level, -63);
        assert!(!login.online_mode);

        let mut out = Vec::new();
        login.encode(&mut out);
        assert_eq!(out, bytes);
    }

    #[test]
    fn player_info_matches_vanilla_capture() {
        // Vanilla's entry for a freshly joined offline player "Probe".
        let bytes = hex(concat!(
            "ff01",
            "3f0ca0c89f163b8792fbb626b9a1bb99",
            "0550726f6265",
            "00",
            "00",
            "00",
            "01",
            "00",
            "00",
            "00",
            "01",
        ));
        let info = PlayerInfoUpdate::decode(&mut bytes.as_slice()).unwrap();
        assert_eq!(info.entries[0].profile.name, "Probe");
        assert_eq!(info.entries[0].game_mode, GameMode::Survival);
        assert!(info.entries[0].listed && info.entries[0].show_hat);
        let mut out = Vec::new();
        info.encode(&mut out);
        assert_eq!(out, bytes);
    }

    #[test]
    fn set_time_matches_vanilla_capture() {
        // The full update a vanilla 26.3 server sends on join: game time 4253,
        // both clocks at 4253 ticks, running at normal speed.
        let bytes = hex("000000000000109d02019d21000000003f800000009d21000000003f800000");
        let time = SetTime::decode(&mut bytes.as_slice()).unwrap();
        assert_eq!(time.game_time, 4253);
        assert_eq!(time.clocks[1].0, 0, "minecraft:overworld");
        assert_eq!(time.clocks[1].1.total_ticks, 4253);
        assert_eq!(time.clocks[1].1.rate, 1.0);
        let mut out = Vec::new();
        time.encode(&mut out);
        assert_eq!(out, bytes);
    }

    #[test]
    fn forget_chunk_packs_like_chunk_pos() {
        let mut out = Vec::new();
        ForgetLevelChunk { x: -1, z: 2 }.encode(&mut out);
        assert_eq!(out, [0, 0, 0, 2, 0xff, 0xff, 0xff, 0xff]);
        assert_eq!(
            ForgetLevelChunk::decode(&mut out.as_slice()).unwrap(),
            ForgetLevelChunk { x: -1, z: 2 }
        );
    }

    #[test]
    fn move_player_variants_roundtrip() {
        for packet in [
            MovePlayer {
                pos: Some([0.5, -60.0, 0.5]),
                rot: None,
                flags: 1,
            },
            MovePlayer {
                pos: Some([1.0, 2.0, 3.0]),
                rot: Some([90.0, -10.0]),
                flags: 0,
            },
            MovePlayer {
                pos: None,
                rot: Some([0.0, 0.0]),
                flags: 3,
            },
            MovePlayer {
                pos: None,
                rot: None,
                flags: 1,
            },
        ] {
            let mut out = Vec::new();
            let id = packet.encode_variant(&mut out);
            assert_eq!(
                MovePlayer::decode_variant(id, &out).unwrap().unwrap(),
                packet
            );
        }
        assert!(MovePlayer::decode_variant(0x00, &[]).is_none());
    }
}
