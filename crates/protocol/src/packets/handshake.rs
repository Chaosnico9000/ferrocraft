use super::Packet;
use crate::codec::{Decode, Encode, Error, Result, VarInt, decode_string};

/// What the client wants after the handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Status,
    Login,
    /// Login after a `transfer` packet from another server.
    Transfer,
}

impl Intent {
    fn id(self) -> i32 {
        match self {
            Intent::Status => 1,
            Intent::Login => 2,
            Intent::Transfer => 3,
        }
    }
}

/// `minecraft:intention`, serverbound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intention {
    pub protocol_version: i32,
    pub host: String,
    pub port: u16,
    pub intent: Intent,
}

impl Packet for Intention {
    const ID: i32 = 0x00;
}

impl Encode for Intention {
    fn encode(&self, out: &mut Vec<u8>) {
        VarInt(self.protocol_version).encode(out);
        self.host.encode(out);
        self.port.encode(out);
        VarInt(self.intent.id()).encode(out);
    }
}

impl Decode for Intention {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            protocol_version: VarInt::decode(buf)?.0,
            host: decode_string(buf, 255)?,
            port: u16::decode(buf)?,
            intent: match VarInt::decode(buf)?.0 {
                1 => Intent::Status,
                2 => Intent::Login,
                3 => Intent::Transfer,
                _ => return Err(Error::Invalid("unknown handshake intent")),
            },
        })
    }
}
