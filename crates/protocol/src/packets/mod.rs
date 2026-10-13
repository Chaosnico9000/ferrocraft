//! Packets, grouped by connection state. Each type knows its id and how to
//! encode and decode itself, so both the server and test clients can use them.

pub mod handshake;
pub mod login;
pub mod status;

use crate::codec::{Decode, Encode, Error, Result};
use crate::frame::{RawPacket, encode_frame};

pub trait Packet: Encode + Decode {
    /// `protocol_id` from `packets.json` for this packet's state and direction.
    const ID: i32;

    /// Frames the packet, ready to write to the socket.
    fn to_frame(&self, out: &mut Vec<u8>) {
        let mut body = Vec::new();
        self.encode(&mut body);
        encode_frame(Self::ID, &body, out);
    }

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
