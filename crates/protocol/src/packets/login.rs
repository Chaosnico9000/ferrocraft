use uuid::Uuid;

use super::Packet;
use crate::codec::{Decode, Encode, Result, decode_string};

/// `minecraft:hello`, serverbound: the player name and the UUID the client
/// believes it has (its account UUID, or a random one when offline).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub name: String,
    pub uuid: Uuid,
}

impl Packet for Hello {
    const ID: i32 = 0x00;
}

impl Encode for Hello {
    fn encode(&self, out: &mut Vec<u8>) {
        self.name.encode(out);
        self.uuid.encode(out);
    }
}

impl Decode for Hello {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            name: decode_string(buf, 16)?,
            uuid: Uuid::decode(buf)?,
        })
    }
}

/// `minecraft:login_disconnect`, clientbound. Unlike the disconnect packets of
/// later states, the reason is a JSON text component, not NBT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginDisconnect {
    pub reason_json: String,
}

impl Packet for LoginDisconnect {
    const ID: i32 = 0x00;
}

impl Encode for LoginDisconnect {
    fn encode(&self, out: &mut Vec<u8>) {
        self.reason_json.encode(out);
    }
}

impl Decode for LoginDisconnect {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            reason_json: decode_string(buf, 262_144)?,
        })
    }
}
