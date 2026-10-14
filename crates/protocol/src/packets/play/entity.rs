//! Entities other than the client's own player: spawning, movement, entity
//! data, and the tab list entries of other players.

use uuid::Uuid;

use super::super::{empty_packet, packet};
use crate::codec::{Decode, Encode, Error, Result, VarInt};

empty_packet! {
    /// `minecraft:bundle_delimiter`, clientbound: the packets between two
    /// delimiters are applied in the same client tick.
    BundleDelimiter = 0x00
}

/// A velocity in vanilla's compact `LpVec3` form: three 15-bit values scaled
/// by the largest component, usually 6 bytes and a single 0 for no motion.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LpVec3(pub [f64; 3]);

/// Below this every component is sent as zero.
const LP_MIN: f64 = 3.051944088384301E-5;
const LP_MAX: f64 = 1.7179869183E10;

impl Encode for LpVec3 {
    fn encode(&self, out: &mut Vec<u8>) {
        let v = self.0.map(|x| {
            if x.is_nan() {
                0.0
            } else {
                x.clamp(-LP_MAX, LP_MAX)
            }
        });
        let largest = v.iter().fold(0.0f64, |m, x| m.max(x.abs()));
        if largest < LP_MIN {
            out.push(0);
            return;
        }
        let scale = largest.ceil() as i64;
        let partial = scale & 3 != scale;
        let markers = if partial { scale & 3 | 4 } else { scale };
        let pack = |x: f64| ((x / scale as f64 * 0.5 + 0.5) * 32766.0).round() as i64;
        let packed = markers | pack(v[0]) << 3 | pack(v[1]) << 18 | pack(v[2]) << 33;
        out.push(packed as u8);
        out.push((packed >> 8) as u8);
        ((packed >> 16) as i32).encode(out);
        if partial {
            VarInt((scale >> 2) as i32).encode(out);
        }
    }
}

impl Decode for LpVec3 {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let lowest = u8::decode(buf)?;
        if lowest == 0 {
            return Ok(Self::default());
        }
        let middle = u8::decode(buf)?;
        let highest = u32::from_be_bytes(i32::decode(buf)?.to_be_bytes());
        let packed = i64::from(highest) << 16 | i64::from(middle) << 8 | i64::from(lowest);
        let mut scale = i64::from(lowest & 3);
        if lowest & 4 != 0 {
            scale |= i64::from(VarInt::decode(buf)?.0 as u32) << 2;
        }
        let unpack = |v: i64| ((v & 32767).min(32766) as f64) * 2.0 / 32766.0 - 1.0;
        Ok(Self(
            [packed >> 3, packed >> 18, packed >> 33].map(|v| unpack(v) * scale as f64),
        ))
    }
}

/// An angle in 1/256 of a turn, as entity rotations are sent
/// (`Mth.packDegrees`).
pub fn pack_degrees(degrees: f32) -> u8 {
    (degrees * 256.0 / 360.0).floor() as i32 as u8
}

packet! {
    /// `minecraft:add_entity`, clientbound: spawns an entity. Players get one
    /// like every other entity, after their tab list entry.
    AddEntity = 0x01 {
        entity_id: i32 as varint,
        uuid: Uuid,
        /// Network id in the `minecraft:entity_type` registry.
        entity_type: i32 as varint,
        pos: [f64; 3],
        velocity: LpVec3,
        pitch: u8,
        yaw: u8,
        head_yaw: u8,
        data: i32 as varint,
    }
}

/// `Entity.Pose`, the ones a player takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pose {
    Standing = 0,
    FallFlying = 1,
    Sleeping = 2,
    Swimming = 3,
    SpinAttack = 4,
    Crouching = 5,
}

/// One synchronized value; the variant decides the serializer id.
#[derive(Debug, Clone, PartialEq)]
pub enum EntityData {
    Byte(u8),
    Int(i32),
    Float(f32),
    Boolean(bool),
    Pose(Pose),
    /// `HumanoidArm`: 0 left, 1 right.
    HumanoidArm(i32),
    /// An item stack, e.g. what an item entity shows.
    Item(super::Slot),
}

impl EntityData {
    /// Index in `EntityDataSerializers`' registration order.
    fn serializer(&self) -> i32 {
        match self {
            EntityData::Byte(_) => 0,
            EntityData::Int(_) => 1,
            EntityData::Float(_) => 3,
            EntityData::Boolean(_) => 8,
            EntityData::Pose(_) => 20,
            EntityData::HumanoidArm(_) => 42,
            EntityData::Item(_) => 7,
        }
    }
}

/// Indices of the synchronized data a player has (`Entity`, `LivingEntity`,
/// `Avatar`, `Player`, in that order of definition).
pub mod data_index {
    /// Bit set: 0x02 crouching, 0x08 sprinting, 0x20 invisible, ...
    pub const SHARED_FLAGS: u8 = 0;
    pub const POSE: u8 = 6;
    pub const HEALTH: u8 = 9;
    pub const MAIN_HAND: u8 = 15;
    /// Visible skin layers, as the client reports them.
    pub const SKIN_PARTS: u8 = 16;
    /// The stack an item entity shows (`ItemEntity.DATA_ITEM`).
    pub const ITEM: u8 = 8;
    /// `LivingEntity.DATA_LIVING_ENTITY_FLAGS`: 0x01 using an item, 0x02
    /// with the off hand.
    pub const LIVING_ENTITY_FLAGS: u8 = 8;
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntityDataValue {
    pub index: u8,
    pub value: EntityData,
}

packet! {
    /// `minecraft:set_entity_data`, clientbound: changed synchronized values,
    /// ended by index 255.
    SetEntityData = 0x65 {
        entity_id: i32 as varint,
        values: EntityDataValues,
    }
}

/// The value list of [`SetEntityData`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EntityDataValues(pub Vec<EntityDataValue>);

impl Encode for EntityDataValues {
    fn encode(&self, out: &mut Vec<u8>) {
        for value in &self.0 {
            value.index.encode(out);
            VarInt(value.value.serializer()).encode(out);
            match &value.value {
                EntityData::Byte(v) => v.encode(out),
                EntityData::Int(v) | EntityData::HumanoidArm(v) => VarInt(*v).encode(out),
                EntityData::Float(v) => v.encode(out),
                EntityData::Boolean(v) => v.encode(out),
                EntityData::Pose(v) => VarInt(*v as i32).encode(out),
                EntityData::Item(v) => v.encode(out),
            }
        }
        255u8.encode(out);
    }
}

impl Decode for EntityDataValues {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let mut values = Vec::new();
        loop {
            let index = u8::decode(buf)?;
            if index == 255 {
                return Ok(Self(values));
            }
            let value = match VarInt::decode(buf)?.0 {
                0 => EntityData::Byte(u8::decode(buf)?),
                1 => EntityData::Int(VarInt::decode(buf)?.0),
                3 => EntityData::Float(f32::decode(buf)?),
                8 => EntityData::Boolean(bool::decode(buf)?),
                20 => EntityData::Pose(match VarInt::decode(buf)?.0 {
                    0 => Pose::Standing,
                    1 => Pose::FallFlying,
                    2 => Pose::Sleeping,
                    3 => Pose::Swimming,
                    4 => Pose::SpinAttack,
                    5 => Pose::Crouching,
                    _ => return Err(Error::Invalid("unsupported pose")),
                }),
                42 => EntityData::HumanoidArm(VarInt::decode(buf)?.0),
                7 => EntityData::Item(super::Slot::decode(buf)?),
                _ => return Err(Error::Invalid("unsupported entity data serializer")),
            };
            values.push(EntityDataValue { index, value });
        }
    }
}

/// A position change in steps of 1/4096 block, relative to the last position
/// sent (`VecDelta`). `Stepped` spreads it over ticks of client
/// interpolation; vanilla sends one step over the update interval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VecDelta {
    Linear([i16; 3]),
    Stepped(Vec<(i32, [i16; 3])>),
}

impl VecDelta {
    /// Fixed-point position as `VecDeltaCodec.encode` computes it.
    pub fn quantize(v: f64) -> i64 {
        (v * 4096.0).round() as i64
    }

    /// The delta from `from` to `to`, `None` if a component does not fit in
    /// a short and the move needs an absolute [`EntityPositionSync`].
    pub fn between(from: [f64; 3], to: [f64; 3]) -> Option<[i16; 3]> {
        let mut delta = [0i16; 3];
        for i in 0..3 {
            let d = Self::quantize(to[i]) - Self::quantize(from[i]);
            delta[i] = i16::try_from(d).ok()?;
        }
        Some(delta)
    }

    fn step_count(&self) -> i32 {
        match self {
            VecDelta::Linear(_) => 0,
            VecDelta::Stepped(steps) => steps.len() as i32,
        }
    }

    /// The flag `VarInt` that precedes a delta: on ground and the step count.
    fn encode_with_flags(&self, on_ground: bool, out: &mut Vec<u8>) {
        VarInt(i32::from(on_ground) | self.step_count() << 1).encode(out);
        match self {
            VecDelta::Linear(d) => d.iter().for_each(|v| v.encode(out)),
            VecDelta::Stepped(steps) => {
                for (ticks, d) in steps {
                    VarInt(*ticks).encode(out);
                    d.iter().for_each(|v| v.encode(out));
                }
            }
        }
    }

    fn decode_with_flags(buf: &mut &[u8]) -> Result<(Self, bool)> {
        let flags = VarInt::decode(buf)?.0;
        let short3 = |buf: &mut &[u8]| -> Result<[i16; 3]> {
            Ok([i16::decode(buf)?, i16::decode(buf)?, i16::decode(buf)?])
        };
        let steps = flags >> 1;
        let delta = if steps <= 0 {
            VecDelta::Linear(short3(buf)?)
        } else {
            if steps as usize > buf.len() / 7 {
                return Err(Error::Invalid("more steps than the packet holds"));
            }
            VecDelta::Stepped(
                (0..steps)
                    .map(|_| Ok((VarInt::decode(buf)?.0, short3(buf)?)))
                    .collect::<Result<_>>()?,
            )
        };
        Ok((delta, flags & 1 != 0))
    }
}

/// `minecraft:move_entity_pos`, clientbound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveEntityPos {
    pub entity_id: i32,
    pub delta: VecDelta,
    pub on_ground: bool,
}

/// `minecraft:move_entity_pos_rot`, clientbound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveEntityPosRot {
    pub entity_id: i32,
    pub delta: VecDelta,
    pub yaw: u8,
    pub pitch: u8,
    pub on_ground: bool,
}

impl super::Packet for MoveEntityPos {
    const ID: i32 = 0x36;
}

impl Encode for MoveEntityPos {
    fn encode(&self, out: &mut Vec<u8>) {
        VarInt(self.entity_id).encode(out);
        self.delta.encode_with_flags(self.on_ground, out);
    }
}

impl Decode for MoveEntityPos {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let entity_id = VarInt::decode(buf)?.0;
        let (delta, on_ground) = VecDelta::decode_with_flags(buf)?;
        Ok(Self {
            entity_id,
            delta,
            on_ground,
        })
    }
}

impl super::Packet for MoveEntityPosRot {
    const ID: i32 = 0x37;
}

impl Encode for MoveEntityPosRot {
    fn encode(&self, out: &mut Vec<u8>) {
        VarInt(self.entity_id).encode(out);
        self.delta.encode_with_flags(self.on_ground, out);
        self.yaw.encode(out);
        self.pitch.encode(out);
    }
}

impl Decode for MoveEntityPosRot {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let entity_id = VarInt::decode(buf)?.0;
        let (delta, on_ground) = VecDelta::decode_with_flags(buf)?;
        Ok(Self {
            entity_id,
            delta,
            yaw: u8::decode(buf)?,
            pitch: u8::decode(buf)?,
            on_ground,
        })
    }
}

packet! {
    /// `minecraft:move_entity_rot`, clientbound. Unlike the other two the
    /// ground flag is a plain bool, before the angles.
    MoveEntityRot = 0x39 {
        entity_id: i32 as varint,
        on_ground: bool,
        yaw: u8,
        pitch: u8,
    }
}

/// `minecraft:entity_position_sync`, clientbound: an absolute position, for
/// moves too large for a delta and when the ground flag changes.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityPositionSync {
    pub entity_id: i32,
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

/// `PositionPath.Type.LINEAR`: a single end position.
const POSITION_PATH_LINEAR: i32 = 0;

impl super::Packet for EntityPositionSync {
    const ID: i32 = 0x23;
}

impl Encode for EntityPositionSync {
    fn encode(&self, out: &mut Vec<u8>) {
        VarInt(self.entity_id).encode(out);
        VarInt(POSITION_PATH_LINEAR).encode(out);
        self.pos.encode(out);
        self.yaw.encode(out);
        self.pitch.encode(out);
        self.on_ground.encode(out);
    }
}

impl Decode for EntityPositionSync {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let entity_id = VarInt::decode(buf)?.0;
        if VarInt::decode(buf)?.0 != POSITION_PATH_LINEAR {
            return Err(Error::Invalid("only linear position paths are supported"));
        }
        Ok(Self {
            entity_id,
            pos: <[f64; 3]>::decode(buf)?,
            yaw: f32::decode(buf)?,
            pitch: f32::decode(buf)?,
            on_ground: bool::decode(buf)?,
        })
    }
}

packet! {
    /// `minecraft:set_entity_motion`, clientbound: a new velocity, which the
    /// client keeps simulating.
    SetEntityMotion = 0x67 {
        entity_id: i32 as varint,
        velocity: LpVec3,
    }
}

packet! {
    /// `minecraft:take_item_entity`, clientbound: the pickup animation of an
    /// item flying to a player.
    TakeItemEntity = 0x7f {
        item_id: i32 as varint,
        player_id: i32 as varint,
        amount: i32 as varint,
    }
}

packet! {
    /// `minecraft:rotate_head`, clientbound.
    RotateHead = 0x55 {
        entity_id: i32 as varint,
        head_yaw: u8,
    }
}

/// A list of `VarInt` entity ids.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EntityIds(pub Vec<i32>);

impl Encode for EntityIds {
    fn encode(&self, out: &mut Vec<u8>) {
        VarInt(self.0.len() as i32).encode(out);
        self.0.iter().for_each(|&id| VarInt(id).encode(out));
    }
}

impl Decode for EntityIds {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let ids: Vec<VarInt> = Vec::decode(buf)?;
        Ok(Self(ids.into_iter().map(|v| v.0).collect()))
    }
}

packet! {
    /// `minecraft:remove_entities`, clientbound.
    RemoveEntities = 0x4e {
        entity_ids: EntityIds,
    }
}

packet! {
    /// `minecraft:player_info_remove`, clientbound: takes players off the tab
    /// list.
    PlayerInfoRemove = 0x46 {
        uuids: Vec<Uuid>,
    }
}

/// `UPDATE_LATENCY` alone, as a fixed bit set of the eight actions.
const PLAYER_INFO_LATENCY: u8 = 0x10;

/// `minecraft:player_info_update` with only the latency action: the ping
/// bars in the tab list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfoLatency {
    pub entries: Vec<(Uuid, i32)>,
}

impl super::Packet for PlayerInfoLatency {
    const ID: i32 = 0x47;
}

impl Encode for PlayerInfoLatency {
    fn encode(&self, out: &mut Vec<u8>) {
        PLAYER_INFO_LATENCY.encode(out);
        VarInt(self.entries.len() as i32).encode(out);
        for (uuid, latency) in &self.entries {
            uuid.encode(out);
            VarInt(*latency).encode(out);
        }
    }
}

impl Decode for PlayerInfoLatency {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        if u8::decode(buf)? != PLAYER_INFO_LATENCY {
            return Err(Error::Invalid("not a latency-only player_info_update"));
        }
        let count = crate::codec::decode_count(buf, buf.len())?;
        let entries = (0..count)
            .map(|_| Ok((Uuid::decode(buf)?, VarInt::decode(buf)?.0)))
            .collect::<Result<_>>()?;
        Ok(Self { entries })
    }
}

/// `UPDATE_GAME_MODE` alone.
const PLAYER_INFO_GAME_MODE: u8 = 0x04;

/// `minecraft:player_info_update` with only the game mode action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfoGameMode {
    pub entries: Vec<(Uuid, i32)>,
}

impl super::Packet for PlayerInfoGameMode {
    const ID: i32 = 0x47;
}

impl Encode for PlayerInfoGameMode {
    fn encode(&self, out: &mut Vec<u8>) {
        PLAYER_INFO_GAME_MODE.encode(out);
        VarInt(self.entries.len() as i32).encode(out);
        for (uuid, mode) in &self.entries {
            uuid.encode(out);
            VarInt(*mode).encode(out);
        }
    }
}

impl Decode for PlayerInfoGameMode {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        if u8::decode(buf)? != PLAYER_INFO_GAME_MODE {
            return Err(Error::Invalid("not a game-mode-only player_info_update"));
        }
        let count = crate::codec::decode_count(buf, buf.len())?;
        let entries = (0..count)
            .map(|_| Ok((Uuid::decode(buf)?, VarInt::decode(buf)?.0)))
            .collect::<Result<_>>()?;
        Ok(Self { entries })
    }
}

packet! {
    /// `minecraft:swing_animation`, clientbound: an arm swing of another
    /// entity. `animation` is `SwingAnimationType` (1 whack, 2 stab).
    SwingAnimation = 0x7b {
        entity_id: i32 as varint,
        /// 0 main hand, 1 off hand.
        hand: i32 as varint,
        animation: i32 as varint,
        duration: i32 as varint,
    }
}

packet! {
    /// `minecraft:player_input`, serverbound: the movement keys held. Bit
    /// 0x20 is sneak.
    PlayerInput = 0x2b {
        flags: u8,
    }
}

/// Actions of [`PlayerCommand`] (`ServerboundPlayerCommandPacket.Action`).
pub mod player_command {
    pub const START_SPRINTING: i32 = 1;
    pub const STOP_SPRINTING: i32 = 2;
}

packet! {
    /// `minecraft:player_command`, serverbound.
    PlayerCommand = 0x2a {
        entity_id: i32 as varint,
        action: i32 as varint,
        data: i32 as varint,
    }
}

packet! {
    /// `minecraft:player_abilities`, serverbound: whether the player flies
    /// (flag 0x02).
    ServerboundPlayerAbilities = 0x28 {
        flags: u8,
    }
}

empty_packet! {
    /// `minecraft:punch`, serverbound: a left click into the air or at an
    /// entity; the server swings the player's arm for everyone else.
    Punch = 0x2e
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

    fn roundtrip<T: Encode + Decode + PartialEq + std::fmt::Debug>(bytes: &[u8]) -> T {
        let value = T::decode(&mut &bytes[..]).unwrap();
        let mut out = Vec::new();
        value.encode(&mut out);
        assert_eq!(out, bytes, "{value:?}");
        value
    }

    // Packets a vanilla 26.3 server sent to one bot about another.

    #[test]
    fn add_entity_matches_vanilla_capture() {
        let spawn: AddEntity = roundtrip(&hex(concat!(
            "dd02",
            "1110dc55a244318c9843101d405cbf34",
            "9f01",
            "c009b1d7742d8a40c04e0000000000004018de444f66685a",
            "00",
            "00",
            "92",
            "92",
            "00"
        )));
        assert_eq!(spawn.entity_id, 349);
        assert_eq!(spawn.entity_type, 159, "minecraft:player");
        assert_eq!(spawn.pos[1], -60.0);
        assert_eq!(spawn.velocity, LpVec3::default());
    }

    #[test]
    fn entity_data_matches_vanilla_capture() {
        let data: SetEntityData = roundtrip(&hex("dd02090341a0000010007fff"));
        assert_eq!(
            data.values.0,
            [
                EntityDataValue {
                    index: data_index::HEALTH,
                    value: EntityData::Float(20.0)
                },
                EntityDataValue {
                    index: data_index::SKIN_PARTS,
                    value: EntityData::Byte(0x7f)
                },
            ]
        );
    }

    #[test]
    fn movement_matches_vanilla_capture() {
        let idle: MoveEntityPos = roundtrip(&hex("dd0201000000000000"));
        assert_eq!(idle.delta, VecDelta::Linear([0, 0, 0]));
        assert!(idle.on_ground);

        let step: MoveEntityPosRot = roundtrip(&hex("dd020302ff93000006620500"));
        assert_eq!(step.delta, VecDelta::Stepped(vec![(2, [-109, 0, 1634])]));
        assert_eq!((step.yaw, step.pitch), (5, 0));

        let head: RotateHead = roundtrip(&hex("dd0202"));
        assert_eq!(head.head_yaw, 2);
        let gone: RemoveEntities = roundtrip(&hex("01dd02"));
        assert_eq!(gone.entity_ids.0, [349]);
        let off: PlayerInfoRemove = roundtrip(&hex("011110dc55a244318c9843101d405cbf34"));
        assert_eq!(off.uuids.len(), 1);
    }

    #[test]
    fn deltas_and_angles() {
        assert_eq!(
            VecDelta::between([0.0, -60.0, 0.0], [0.25, -60.0, -1.0]),
            Some([1024, 0, -4096])
        );
        assert_eq!(VecDelta::between([0.0; 3], [8.0, 0.0, 0.0]), None);
        assert_eq!(pack_degrees(90.0), 64);
        assert_eq!(pack_degrees(-90.0), 192);
        assert_eq!(pack_degrees(359.0), 255);
    }

    #[test]
    fn lp_vec3_roundtrips_within_its_precision() {
        for v in [
            [0.0, 0.0, 0.0],
            [0.1, -0.08, 0.0],
            [3.5, 0.0, -2.0],
            [-40.0, 7.0, 1.0],
        ] {
            let mut out = Vec::new();
            LpVec3(v).encode(&mut out);
            let back = LpVec3::decode(&mut out.as_slice()).unwrap().0;
            let scale = v.iter().fold(0.0f64, |m, x| m.max(x.abs())).ceil().max(1.0);
            for i in 0..3 {
                assert!(
                    (back[i] - v[i]).abs() <= scale / 16000.0,
                    "{v:?} → {back:?}"
                );
            }
        }
    }
}
