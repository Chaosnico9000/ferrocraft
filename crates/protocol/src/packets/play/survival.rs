//! Survival: digging, health, death and respawning, picking blocks.

use super::super::packet;
use super::SpawnInfo;
use crate::codec::{BlockPos, Decode, Encode, Result, VarInt};
use crate::nbt::Nbt;

packet! {
    /// `minecraft:block_destruction`, clientbound: the cracks of a block
    /// another player digs, `progress` 0–9, anything else removes them. `id`
    /// tells diggers apart.
    BlockDestruction = 0x05 {
        id: i32 as varint,
        pos: BlockPos,
        progress: u8,
    }
}

packet! {
    /// `minecraft:level_event`, clientbound: a sound and particles at a
    /// place, e.g. 2001 for a block breaking (`data` its state).
    LevelEvent = 0x2f {
        event: i32,
        pos: BlockPos,
        data: i32,
        global: bool,
    }
}

/// `LevelEvent.PARTICLES_DESTROY_BLOCK`.
pub const LEVEL_EVENT_DESTROY_BLOCK: i32 = 2001;

packet! {
    /// `minecraft:set_health`, clientbound.
    SetHealth = 0x6a {
        health: f32,
        food: i32 as varint,
        saturation: f32,
    }
}

packet! {
    /// `minecraft:hurt_animation`, clientbound: an entity flinches; `yaw`
    /// is where the hit came from, relative to its view.
    HurtAnimation = 0x2b {
        entity_id: i32 as varint,
        yaw: f32,
    }
}

packet! {
    /// `minecraft:player_combat_kill`, clientbound: shows the death screen
    /// with a message.
    PlayerCombatKill = 0x45 {
        player_id: i32 as varint,
        message: Nbt,
    }
}

packet! {
    /// `minecraft:respawn`, clientbound: a new player entity, here or in
    /// another dimension. `keep` 1 keeps attribute modifiers, 2 entity data.
    Respawn = 0x54 {
        spawn: SpawnInfo,
        keep: u8,
    }
}

packet! {
    /// `minecraft:client_command`, serverbound: 0 respawn, 1 statistics,
    /// 2 game rules.
    ClientCommand = 0x0c {
        action: i32 as varint,
    }
}

/// `ServerboundClientCommandPacket.Action.PERFORM_RESPAWN`.
pub const CLIENT_COMMAND_RESPAWN: i32 = 0;

packet! {
    /// `minecraft:pick_item_from_block`, serverbound: the middle click on a
    /// block; `include_data` with Ctrl, to copy a block entity's content.
    PickItemFromBlock = 0x24 {
        pos: BlockPos,
        include_data: bool,
    }
}

packet! {
    /// `minecraft:use_item`, serverbound: a right click with an item, not on
    /// a block. `hand` 0 main, 1 off hand.
    UseItem = 0x43 {
        hand: i32 as varint,
        sequence: i32 as varint,
        yaw: f32,
        pitch: f32,
    }
}

packet! {
    /// `minecraft:sound`, clientbound. `sound` is the `minecraft:sound_event`
    /// id + 1 (0 would be an inline event), the position in eighths of a
    /// block.
    Sound = 0x77 {
        sound: i32 as varint,
        /// `SoundSource`: 4 blocks, 7 players.
        source: i32 as varint,
        x: i32,
        y: i32,
        z: i32,
        volume: f32,
        pitch: f32,
        seed: i64,
    }
}

/// `SoundSource.BLOCKS`.
pub const SOUND_SOURCE_BLOCKS: i32 = 4;

/// `minecraft:damage_event`, clientbound: an entity was hurt; the client
/// plays the hurt animation and sound.
#[derive(Debug, Clone, PartialEq)]
pub struct DamageEvent {
    pub entity_id: i32,
    /// Network id in `minecraft:damage_type`.
    pub source_type: i32,
    pub cause: Option<i32>,
    pub direct: Option<i32>,
    pub source_pos: Option<[f64; 3]>,
}

impl super::Packet for DamageEvent {
    const ID: i32 = 0x19;
}

impl Encode for DamageEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        VarInt(self.entity_id).encode(out);
        VarInt(self.source_type).encode(out);
        VarInt(self.cause.map_or(0, |id| id + 1)).encode(out);
        VarInt(self.direct.map_or(0, |id| id + 1)).encode(out);
        self.source_pos.is_some().encode(out);
        if let Some(pos) = self.source_pos {
            pos.iter().for_each(|v| v.encode(out));
        }
    }
}

impl Decode for DamageEvent {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let entity_id = VarInt::decode(buf)?.0;
        let source_type = VarInt::decode(buf)?.0;
        let optional = |buf: &mut &[u8]| -> Result<Option<i32>> {
            Ok(match VarInt::decode(buf)?.0 {
                0 => None,
                id => Some(id - 1),
            })
        };
        let cause = optional(buf)?;
        let direct = optional(buf)?;
        let source_pos = if bool::decode(buf)? {
            Some([f64::decode(buf)?, f64::decode(buf)?, f64::decode(buf)?])
        } else {
            None
        };
        Ok(Self {
            entity_id,
            source_type,
            cause,
            direct,
            source_pos,
        })
    }
}

packet! {
    /// `minecraft:attack`, serverbound: a left click on an entity.
    Attack = 0x01 {
        entity_id: i32 as varint,
    }
}

packet! {
    /// `minecraft:interact`, serverbound: a right click on an entity, e.g.
    /// shears on a sheep. `location` is where on it, relative to its feet.
    Interact = 0x1a {
        entity_id: i32 as varint,
        /// 0 main hand, 1 off hand.
        hand: i32 as varint,
        location: super::LpVec3,
        sneaking: bool,
    }
}

packet! {
    /// `minecraft:animate`, clientbound: 1 shows the stars of a critical
    /// hit on an entity.
    Animate = 0x02 {
        entity_id: i32 as varint,
        action: u8,
    }
}

/// `ClientboundAnimatePacket.CRITICAL_HIT`.
pub const ANIMATE_CRITICAL_HIT: u8 = 1;
/// `SoundSource.NEUTRAL` and `SoundSource.PLAYERS`.
pub const SOUND_SOURCE_NEUTRAL: i32 = 6;
pub const SOUND_SOURCE_PLAYERS: i32 = 7;
