//! Status effects, attributes, and the screens that enchant and repair:
//! the enchanting table's buttons and the anvil's name field.

use super::super::packet;
use crate::codec::{Decode, Encode, Result, VarInt, decode_count, decode_string};

packet! {
    /// `minecraft:update_mob_effect`, clientbound: an effect on an entity
    /// started or changed. `flags`: 1 ambient, 2 particles, 4 icon, 8 fade
    /// in.
    UpdateMobEffect = 0x87 {
        entity_id: i32 as varint,
        /// Network id in `minecraft:mob_effect`.
        effect: i32 as varint,
        amplifier: i32 as varint,
        /// Ticks, -1 for ever.
        duration: i32 as varint,
        flags: u8,
    }
}

/// [`UpdateMobEffect::flags`].
pub mod effect_flags {
    pub const AMBIENT: u8 = 1;
    pub const VISIBLE: u8 = 2;
    pub const SHOW_ICON: u8 = 4;
    pub const BLEND: u8 = 8;
}

packet! {
    /// `minecraft:remove_mob_effect`, clientbound.
    RemoveMobEffect = 0x4f {
        entity_id: i32 as varint,
        effect: i32 as varint,
    }
}

packet! {
    /// `minecraft:update_attributes`, clientbound: base values and
    /// modifiers of attributes the client computes with.
    UpdateAttributes = 0x86 {
        entity_id: i32 as varint,
        attributes: AttributeSnapshots,
    }
}

/// An attribute modifier (`AttributeModifier`); `operation` 0 adds, 1
/// adds a multiple of the base, 2 multiplies the total.
#[derive(Debug, Clone, PartialEq)]
pub struct AttributeModifier {
    pub id: String,
    pub amount: f64,
    pub operation: i32,
}

/// One attribute of [`UpdateAttributes`].
#[derive(Debug, Clone, PartialEq)]
pub struct AttributeSnapshot {
    /// Network id in `minecraft:attribute`.
    pub attribute: i32,
    pub base: f64,
    pub modifiers: Vec<AttributeModifier>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AttributeSnapshots(pub Vec<AttributeSnapshot>);

impl Encode for AttributeSnapshots {
    fn encode(&self, out: &mut Vec<u8>) {
        VarInt(self.0.len() as i32).encode(out);
        for snapshot in &self.0 {
            VarInt(snapshot.attribute).encode(out);
            snapshot.base.encode(out);
            VarInt(snapshot.modifiers.len() as i32).encode(out);
            for modifier in &snapshot.modifiers {
                modifier.id.encode(out);
                modifier.amount.encode(out);
                VarInt(modifier.operation).encode(out);
            }
        }
    }
}

impl Decode for AttributeSnapshots {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let count = decode_count(buf, 128)?;
        let mut snapshots = Vec::with_capacity(count);
        for _ in 0..count {
            let attribute = VarInt::decode(buf)?.0;
            let base = f64::decode(buf)?;
            let modifiers = (0..decode_count(buf, buf.len())?)
                .map(|_| {
                    Ok(AttributeModifier {
                        id: decode_string(buf, 32767)?,
                        amount: f64::decode(buf)?,
                        operation: VarInt::decode(buf)?.0,
                    })
                })
                .collect::<Result<_>>()?;
            snapshots.push(AttributeSnapshot {
                attribute,
                base,
                modifiers,
            });
        }
        Ok(Self(snapshots))
    }
}

packet! {
    /// `minecraft:container_button_click`, serverbound: a button in a
    /// screen, such as one of the enchanting table's three offers.
    ContainerButtonClick = 0x11 {
        container_id: i32 as varint,
        button: i32 as varint,
    }
}

packet! {
    /// `minecraft:rename_item`, serverbound: the anvil's name field.
    RenameItem = 0x31 {
        name: String,
    }
}
