//! Wire types and packets of the Minecraft Java Edition protocol.
//!
//! Packet IDs come from Mojang's data generator (`data/<version>/packets.json`,
//! see `scripts/gen-reports.sh`), field layouts from the packet codecs in the
//! vanilla server jar.

pub mod codec;
pub mod frame;
pub mod packets;

pub use codec::{Decode, Encode, Error, VarInt};

/// The game version this crate speaks.
pub const GAME_VERSION: &str = "26.3";
/// `protocol_version` from the vanilla jar's `version.json` for [`GAME_VERSION`].
pub const PROTOCOL_VERSION: i32 = 777;
