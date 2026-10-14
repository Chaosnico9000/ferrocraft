//! Containers and block entities: screens, clicks, the cursor stack, signs,
//! and block entity data.

use super::super::packet;
use super::Slot;
use crate::codec::{BlockPos, Decode, Encode, Error, Result, VarInt, decode_count, decode_string};
use crate::nbt::Nbt;

packet! {
    /// `minecraft:open_screen`, clientbound: opens a container screen. `menu`
    /// is the `minecraft:menu` id, the title a text component.
    OpenScreen = 0x3c {
        container_id: i32 as varint,
        menu: i32 as varint,
        title: Nbt,
    }
}

packet! {
    /// `minecraft:container_close`, clientbound: the server closes a screen.
    ClientboundContainerClose = 0x11 {
        container_id: i32 as varint,
    }
}

packet! {
    /// `minecraft:container_close`, serverbound: the player closed a screen.
    ServerboundContainerClose = 0x13 {
        container_id: i32 as varint,
    }
}

packet! {
    /// `minecraft:set_cursor_item`, clientbound: the stack the mouse holds.
    SetCursorItem = 0x62 {
        stack: Slot,
    }
}

/// `ContainerInput`: what kind of click.
pub mod container_input {
    pub const PICKUP: i32 = 0;
    pub const QUICK_MOVE: i32 = 1;
    pub const SWAP: i32 = 2;
    pub const CLONE: i32 = 3;
    pub const THROW: i32 = 4;
    pub const QUICK_CRAFT: i32 = 5;
    pub const PICKUP_ALL: i32 = 6;
}

/// What the client believes a slot holds after its own prediction
/// (`HashedStack`): the item and count; the components only as hashes, which
/// the server does not check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HashedStack {
    pub item: i32,
    pub count: i32,
}

/// Most components a hashed patch may name.
const MAX_HASHED_COMPONENTS: usize = 256;

fn decode_hashed(buf: &mut &[u8]) -> Result<Option<HashedStack>> {
    if !bool::decode(buf)? {
        return Ok(None);
    }
    let item = VarInt::decode(buf)?.0;
    let count = VarInt::decode(buf)?.0;
    let added = decode_count(buf, MAX_HASHED_COMPONENTS)?;
    for _ in 0..added {
        VarInt::decode(buf)?;
        i32::decode(buf)?;
    }
    let removed = decode_count(buf, MAX_HASHED_COMPONENTS)?;
    for _ in 0..removed {
        VarInt::decode(buf)?;
    }
    Ok(Some(HashedStack { item, count }))
}

fn encode_hashed(stack: &Option<HashedStack>, out: &mut Vec<u8>) {
    stack.is_some().encode(out);
    if let Some(stack) = stack {
        VarInt(stack.item).encode(out);
        VarInt(stack.count).encode(out);
        VarInt(0).encode(out);
        VarInt(0).encode(out);
    }
}

/// `minecraft:container_click`, serverbound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerClick {
    pub container_id: i32,
    pub state_id: i32,
    /// -999 for a click outside the screen.
    pub slot: i16,
    pub button: i8,
    pub input: i32,
    /// The slots the client changed in its prediction.
    pub changed: Vec<(i16, Option<HashedStack>)>,
    pub carried: Option<HashedStack>,
}

impl super::Packet for ContainerClick {
    const ID: i32 = 0x12;
}

/// `ServerboundContainerClickPacket.MAX_SLOT_COUNT`.
const MAX_CHANGED_SLOTS: usize = 128;

impl Encode for ContainerClick {
    fn encode(&self, out: &mut Vec<u8>) {
        VarInt(self.container_id).encode(out);
        VarInt(self.state_id).encode(out);
        self.slot.encode(out);
        self.button.encode(out);
        VarInt(self.input).encode(out);
        VarInt(self.changed.len() as i32).encode(out);
        for (slot, stack) in &self.changed {
            slot.encode(out);
            encode_hashed(stack, out);
        }
        encode_hashed(&self.carried, out);
    }
}

impl Decode for ContainerClick {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let container_id = VarInt::decode(buf)?.0;
        let state_id = VarInt::decode(buf)?.0;
        let slot = i16::decode(buf)?;
        let button = i8::decode(buf)?;
        let input = VarInt::decode(buf)?.0;
        if !(0..=6).contains(&input) {
            return Err(Error::Invalid("unknown container input"));
        }
        let count = decode_count(buf, MAX_CHANGED_SLOTS)?;
        let changed = (0..count)
            .map(|_| Ok((i16::decode(buf)?, decode_hashed(buf)?)))
            .collect::<Result<_>>()?;
        Ok(Self {
            container_id,
            state_id,
            slot,
            button,
            input,
            changed,
            carried: decode_hashed(buf)?,
        })
    }
}

packet! {
    /// `minecraft:block_event`, clientbound: an animation of a block, e.g. a
    /// chest's lid (event 1, the number of viewers).
    BlockEvent = 0x07 {
        pos: BlockPos,
        event: u8,
        param: u8,
        /// Network id in `minecraft:block`.
        block: i32 as varint,
    }
}

packet! {
    /// `minecraft:block_entity_data`, clientbound: what the client should
    /// know of a block entity, such as a sign's text.
    BlockEntityData = 0x06 {
        pos: BlockPos,
        /// Network id in `minecraft:block_entity_type`.
        kind: i32 as varint,
        tag: Nbt,
    }
}

packet! {
    /// `minecraft:open_sign_editor`, clientbound. `side` 1 is the front.
    OpenSignEditor = 0x3d {
        pos: BlockPos,
        side: i32 as varint,
    }
}

/// `minecraft:sign_update`, serverbound: four lines of up to 384 characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignUpdate {
    pub pos: BlockPos,
    pub lines: [String; 4],
    /// 1 front, 0 back.
    pub side: i32,
}

impl super::Packet for SignUpdate {
    const ID: i32 = 0x3e;
}

impl Encode for SignUpdate {
    fn encode(&self, out: &mut Vec<u8>) {
        self.pos.encode(out);
        self.lines.iter().for_each(|line| line.encode(out));
        VarInt(self.side).encode(out);
    }
}

impl Decode for SignUpdate {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let pos = BlockPos::decode(buf)?;
        let mut lines: [String; 4] = Default::default();
        for line in &mut lines {
            *line = decode_string(buf, 384)?;
        }
        Ok(Self {
            pos,
            lines,
            side: VarInt::decode(buf)?.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_roundtrips() {
        let click = ContainerClick {
            container_id: 3,
            state_id: 7,
            slot: -999,
            button: 1,
            input: container_input::PICKUP,
            changed: vec![(4, Some(HashedStack { item: 1, count: 2 })), (5, None)],
            carried: None,
        };
        let mut out = Vec::new();
        click.encode(&mut out);
        assert_eq!(ContainerClick::decode(&mut out.as_slice()).unwrap(), click);
    }
}
