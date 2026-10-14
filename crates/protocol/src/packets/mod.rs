//! Packets, grouped by connection state. Each type knows its id and how to
//! encode and decode itself, so both the server and test clients can use them.

pub mod configuration;
pub mod handshake;
pub mod login;
pub mod play;
pub mod status;

use crate::codec::{Decode, Encode, Error, Result};
use crate::frame::RawPacket;

pub trait Packet: Encode + Decode {
    /// `protocol_id` from `packets.json` for this packet's state and direction.
    const ID: i32;

    /// Decodes the packet from a frame, rejecting a wrong id or trailing bytes.
    fn from_raw(raw: &RawPacket) -> Result<Self> {
        if raw.id != Self::ID {
            return Err(Error::Invalid("unexpected packet id"));
        }
        let mut body = raw.body.as_slice();
        let packet = Self::decode(&mut body)?;
        if !body.is_empty() {
            return Err(Error::Invalid("trailing bytes after packet"));
        }
        Ok(packet)
    }
}

/// A packet without fields.
macro_rules! empty_packet {
    ($(#[$doc:meta])* $name:ident = $id:expr) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct $name;

        impl $crate::packets::Packet for $name {
            const ID: i32 = $id;
        }

        impl $crate::codec::Encode for $name {
            fn encode(&self, _: &mut Vec<u8>) {}
        }

        impl $crate::codec::Decode for $name {
            fn decode(_: &mut &[u8]) -> $crate::codec::Result<Self> {
                Ok(Self)
            }
        }
    };
}
pub(crate) use empty_packet;

/// A packet whose fields are written one after another. A field marked
/// `as varint` is an `i32` sent as a `VarInt`; every other field uses its own
/// [`Encode`] and [`Decode`].
macro_rules! packet {
    (
        $(#[$doc:meta])*
        $name:ident = $id:literal {
            $($(#[$field_doc:meta])* $field:ident: $ty:ty $(as $wire:ident)?),* $(,)?
        }
    ) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name {
            $($(#[$field_doc])* pub $field: $ty),*
        }

        impl $crate::packets::Packet for $name {
            const ID: i32 = $id;
        }

        impl $crate::codec::Encode for $name {
            #[allow(unused_variables)]
            fn encode(&self, out: &mut Vec<u8>) {
                $($crate::packets::packet!(@encode self.$field, out $(, $wire)?);)*
            }
        }

        impl $crate::codec::Decode for $name {
            #[allow(unused_variables)]
            fn decode(buf: &mut &[u8]) -> $crate::codec::Result<Self> {
                Ok(Self {
                    $($field: $crate::packets::packet!(@decode buf, $ty $(, $wire)?)),*
                })
            }
        }
    };
    (@encode $value:expr, $out:ident) => {
        $crate::codec::Encode::encode(&$value, $out)
    };
    (@encode $value:expr, $out:ident, varint) => {
        $crate::codec::Encode::encode(&$crate::codec::VarInt($value), $out)
    };
    (@decode $buf:ident, $ty:ty) => {
        <$ty as $crate::codec::Decode>::decode($buf)?
    };
    (@decode $buf:ident, $ty:ty, varint) => {
        <$crate::codec::VarInt as $crate::codec::Decode>::decode($buf)?.0
    };
}
pub(crate) use packet;
