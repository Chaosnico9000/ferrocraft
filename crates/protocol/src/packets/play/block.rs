//! Changing blocks and the items to do it with: clicks on blocks, block
//! updates and the creative inventory.

use super::super::packet;
use crate::codec::{BlockPos, Decode, Encode, Error, Result, VarInt, decode_count};

/// A block face (`Direction`, in its 3D data value order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Down = 0,
    Up = 1,
    North = 2,
    South = 3,
    West = 4,
    East = 5,
}

impl Direction {
    pub fn from_id(id: i32) -> Result<Self> {
        Ok(match id {
            0 => Direction::Down,
            1 => Direction::Up,
            2 => Direction::North,
            3 => Direction::South,
            4 => Direction::West,
            5 => Direction::East,
            _ => return Err(Error::Invalid("unknown direction")),
        })
    }

    /// The block next to `pos` on this side.
    pub fn offset(self, pos: BlockPos) -> BlockPos {
        let (dx, dy, dz) = match self {
            Direction::Down => (0, -1, 0),
            Direction::Up => (0, 1, 0),
            Direction::North => (0, 0, -1),
            Direction::South => (0, 0, 1),
            Direction::West => (-1, 0, 0),
            Direction::East => (1, 0, 0),
        };
        BlockPos {
            x: pos.x + dx,
            y: pos.y + dy,
            z: pos.z + dz,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Direction::Down => "down",
            Direction::Up => "up",
            Direction::North => "north",
            Direction::South => "south",
            Direction::West => "west",
            Direction::East => "east",
        }
    }
}

packet! {
    /// `minecraft:block_update`, clientbound: one block changed.
    BlockUpdate = 0x08 {
        pos: BlockPos,
        state: i32 as varint,
    }
}

packet! {
    /// `minecraft:block_changed_ack`, clientbound: the server has handled the
    /// client's block actions up to `sequence`; the client drops its
    /// predictions for them and keeps what the server sent.
    BlockChangedAck = 0x04 {
        sequence: i32 as varint,
    }
}

/// Actions of [`PlayerAction`] (`ServerboundPlayerActionPacket.Action`).
pub mod player_action {
    /// A left click on a block; in creative it breaks the block at once.
    pub const START_DESTROY_BLOCK: i32 = 0;
    pub const CHANGE_DESTROY_DIRECTION: i32 = 1;
    pub const ABORT_DESTROY_BLOCK: i32 = 2;
    pub const STOP_DESTROY_BLOCK: i32 = 3;
    /// Ctrl+Q.
    pub const DROP_ALL_ITEMS: i32 = 4;
    /// Q.
    pub const DROP_ITEM: i32 = 5;
    pub const RELEASE_USE_ITEM: i32 = 6;
    /// F.
    pub const SWAP_ITEM_WITH_OFFHAND: i32 = 7;
}

packet! {
    /// `minecraft:player_action`, serverbound: digging, dropping and swapping
    /// items. The face is a byte here, unlike in [`UseItemOn`].
    PlayerAction = 0x29 {
        action: i32 as varint,
        pos: BlockPos,
        face: u8,
        sequence: i32 as varint,
    }
}

packet! {
    /// `minecraft:use_item_on`, serverbound: a right click on a block.
    /// `cursor` is where on the block, from 0 to 1 on each axis.
    UseItemOn = 0x42 {
        /// 0 main hand, 1 off hand.
        hand: i32 as varint,
        pos: BlockPos,
        face: i32 as varint,
        cursor: [f32; 3],
        inside: bool,
        world_border_hit: bool,
        sequence: i32 as varint,
    }
}

packet! {
    /// `minecraft:set_carried_item`, serverbound: the selected hotbar slot,
    /// 0 to 8.
    SetCarriedItem = 0x36 {
        slot: i16,
    }
}

/// The components an item stack changes from its item's defaults. The
/// server does not interpret them; it keeps their encoded form to show the
/// stack to other players.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ComponentPatch {
    /// Component type id and its encoded value.
    pub added: Vec<(i32, Vec<u8>)>,
    pub removed: Vec<i32>,
}

/// Most components a patch may carry; vanilla caps the map at 65536.
const MAX_COMPONENTS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemStack {
    /// Network id in the `minecraft:item` registry.
    pub item: i32,
    pub count: i32,
    pub patch: ComponentPatch,
}

/// A slot as the client sends it (`ItemStack.OPTIONAL_UNTRUSTED_STREAM_CODEC`):
/// every component value has a length prefix, so it can be read without
/// knowing its type.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UntrustedSlot(pub Option<ItemStack>);

/// A slot as the server sends it (`ItemStack.OPTIONAL_STREAM_CODEC`): the
/// same, without the length prefixes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Slot(pub Option<ItemStack>);

impl Decode for UntrustedSlot {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let count = VarInt::decode(buf)?.0;
        if count <= 0 {
            return Ok(Self(None));
        }
        let item = VarInt::decode(buf)?.0;
        let added_count = decode_count(buf, MAX_COMPONENTS)?;
        let removed_count = decode_count(buf, MAX_COMPONENTS)?;
        let added = (0..added_count)
            .map(|_| {
                let kind = VarInt::decode(buf)?.0;
                let data = crate::codec::decode_byte_array(buf, buf.len())?;
                Ok((kind, data))
            })
            .collect::<Result<_>>()?;
        let removed = (0..removed_count)
            .map(|_| Ok(VarInt::decode(buf)?.0))
            .collect::<Result<_>>()?;
        Ok(Self(Some(ItemStack {
            item,
            count,
            patch: ComponentPatch { added, removed },
        })))
    }
}

impl Encode for UntrustedSlot {
    fn encode(&self, out: &mut Vec<u8>) {
        encode_slot(self.0.as_ref(), true, out);
    }
}

impl Encode for Slot {
    fn encode(&self, out: &mut Vec<u8>) {
        encode_slot(self.0.as_ref(), false, out);
    }
}

fn encode_slot(stack: Option<&ItemStack>, delimited: bool, out: &mut Vec<u8>) {
    let Some(stack) = stack else {
        VarInt(0).encode(out);
        return;
    };
    VarInt(stack.count).encode(out);
    VarInt(stack.item).encode(out);
    VarInt(stack.patch.added.len() as i32).encode(out);
    VarInt(stack.patch.removed.len() as i32).encode(out);
    for (kind, data) in &stack.patch.added {
        VarInt(*kind).encode(out);
        if delimited {
            crate::codec::encode_byte_array(data, out);
        } else {
            out.extend_from_slice(data);
        }
    }
    for kind in &stack.patch.removed {
        VarInt(*kind).encode(out);
    }
}

packet! {
    /// `minecraft:set_creative_mode_slot`, serverbound: a creative player put
    /// a stack into a slot of their inventory menu (36–44 are the hotbar,
    /// 45 the off hand).
    SetCreativeModeSlot = 0x39 {
        slot: i16,
        stack: UntrustedSlot,
    }
}

impl Decode for Slot {
    /// Component values carry no length here: only components with a shape
    /// in [`crate::component`] can be read, once the type names are known
    /// ([`crate::component::set_type_names`]).
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let count = VarInt::decode(buf)?.0;
        if count <= 0 {
            return Ok(Self(None));
        }
        let item = VarInt::decode(buf)?.0;
        let added_count = decode_count(buf, MAX_COMPONENTS)?;
        let removed_count = decode_count(buf, MAX_COMPONENTS)?;
        let mut added = Vec::with_capacity(added_count);
        for _ in 0..added_count {
            let kind = VarInt::decode(buf)?.0;
            let shape = crate::component::type_name(kind)
                .and_then(crate::component::component)
                .ok_or(Error::Invalid("cannot read this item component"))?;
            let start = *buf;
            shape.shape.skip(buf)?;
            let used = start.len() - buf.len();
            added.push((kind, start[..used].to_vec()));
        }
        let removed = (0..removed_count)
            .map(|_| Ok(VarInt::decode(buf)?.0))
            .collect::<Result<_>>()?;
        Ok(Self(Some(ItemStack {
            item,
            count,
            patch: ComponentPatch { added, removed },
        })))
    }
}

/// A list of slots with a `VarInt` count.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Slots(pub Vec<Slot>);

impl Encode for Slots {
    fn encode(&self, out: &mut Vec<u8>) {
        VarInt(self.0.len() as i32).encode(out);
        self.0.iter().for_each(|slot| slot.encode(out));
    }
}

impl Decode for Slots {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let count = decode_count(buf, buf.len())?;
        (0..count)
            .map(|_| Slot::decode(buf))
            .collect::<Result<_>>()
            .map(Self)
    }
}

packet! {
    /// `minecraft:container_set_content`, clientbound: every slot of a
    /// container; container 0 is the player's inventory menu (46 slots).
    ContainerSetContent = 0x12 {
        container_id: i32 as varint,
        state_id: i32 as varint,
        items: Slots,
        /// The stack on the cursor.
        carried: Slot,
    }
}

packet! {
    /// `minecraft:container_set_slot`, clientbound: one slot of a container
    /// changed.
    ContainerSetSlot = 0x14 {
        container_id: i32 as varint,
        state_id: i32 as varint,
        slot: i16,
        stack: Slot,
    }
}

packet! {
    /// `minecraft:set_held_slot`, clientbound: selects a hotbar slot.
    SetHeldSlot = 0x6b {
        slot: i32 as varint,
    }
}

/// `EquipmentSlot` ordinals.
pub mod equipment_slot {
    pub const MAIN_HAND: u8 = 0;
    pub const OFF_HAND: u8 = 1;
    pub const FEET: u8 = 2;
    pub const LEGS: u8 = 3;
    pub const CHEST: u8 = 4;
    pub const HEAD: u8 = 5;
}

/// `minecraft:set_equipment`, clientbound: what an entity holds and wears.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetEquipment {
    pub entity_id: i32,
    /// At least one entry.
    pub slots: Vec<(u8, Option<ItemStack>)>,
}

impl super::super::Packet for SetEquipment {
    const ID: i32 = 0x68;
}

impl Encode for SetEquipment {
    fn encode(&self, out: &mut Vec<u8>) {
        VarInt(self.entity_id).encode(out);
        for (i, (slot, stack)) in self.slots.iter().enumerate() {
            // The top bit says that another slot follows.
            let more = if i + 1 < self.slots.len() { 0x80 } else { 0 };
            (slot | more).encode(out);
            encode_slot(stack.as_ref(), false, out);
        }
    }
}

impl Decode for SetEquipment {
    /// Only for stacks without components: plain component values cannot be
    /// read without knowing their type.
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let entity_id = VarInt::decode(buf)?.0;
        let mut slots = Vec::new();
        loop {
            let slot = u8::decode(buf)?;
            let count = VarInt::decode(buf)?.0;
            let stack = if count > 0 {
                let item = VarInt::decode(buf)?.0;
                if (VarInt::decode(buf)?.0, VarInt::decode(buf)?.0) != (0, 0) {
                    return Err(Error::Invalid("cannot read item components"));
                }
                Some(ItemStack {
                    item,
                    count,
                    patch: ComponentPatch::default(),
                })
            } else {
                None
            };
            slots.push((slot & 0x7f, stack));
            if slot & 0x80 == 0 {
                return Ok(Self { entity_id, slots });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_update_matches_vanilla_capture() {
        // `setblock 3 -60 3 minecraft:stone` on a vanilla 26.3 server.
        let bytes = [0x00, 0x00, 0x00, 0xc0, 0x00, 0x00, 0x3f, 0xc4, 0x01];
        let update = BlockUpdate::decode(&mut &bytes[..]).unwrap();
        assert_eq!(update.pos, BlockPos { x: 3, y: -60, z: 3 });
        assert_eq!(update.state, 1);
    }

    #[test]
    fn untrusted_slot_keeps_components_for_the_trusted_form() {
        // 2 × item 7 with component 3 = [0xaa, 0xbb] and component 9 removed.
        let delimited = [2, 7, 1, 1, 3, 2, 0xaa, 0xbb, 9];
        let slot = UntrustedSlot::decode(&mut &delimited[..]).unwrap();
        let stack = slot.0.clone().unwrap();
        assert_eq!((stack.item, stack.count), (7, 2));
        assert_eq!(stack.patch.added, [(3, vec![0xaa, 0xbb])]);
        assert_eq!(stack.patch.removed, [9]);
        let mut out = Vec::new();
        slot.encode(&mut out);
        assert_eq!(out, delimited);
        out.clear();
        Slot(slot.0).encode(&mut out);
        assert_eq!(out, [2, 7, 1, 1, 3, 0xaa, 0xbb, 9]);

        assert_eq!(
            UntrustedSlot::decode(&mut &[0][..]).unwrap(),
            UntrustedSlot(None)
        );
    }

    #[test]
    fn equipment_marks_all_but_the_last_slot() {
        let packet = SetEquipment {
            entity_id: 5,
            slots: vec![
                (equipment_slot::MAIN_HAND, None),
                (
                    equipment_slot::HEAD,
                    Some(ItemStack {
                        item: 3,
                        count: 1,
                        patch: ComponentPatch::default(),
                    }),
                ),
            ],
        };
        let mut out = Vec::new();
        packet.encode(&mut out);
        assert_eq!(out, [5, 0x80, 0, 5, 1, 3, 0, 0]);
        assert_eq!(SetEquipment::decode(&mut out.as_slice()).unwrap(), packet);
    }
}
