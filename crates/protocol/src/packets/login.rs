use uuid::Uuid;

use super::{Packet, empty_packet, packet};
use crate::codec::{ByteArray, Decode, Encode, Result, VarInt, decode_list, decode_string};

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

/// A player's identity as the client sees it (`com.mojang.authlib.GameProfile`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameProfile {
    pub uuid: Uuid,
    pub name: String,
    /// Signed properties such as `textures`; empty in offline mode.
    pub properties: Vec<ProfileProperty>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileProperty {
    pub name: String,
    pub value: String,
    pub signature: Option<String>,
}

impl Encode for GameProfile {
    fn encode(&self, out: &mut Vec<u8>) {
        self.uuid.encode(out);
        self.name.encode(out);
        self.properties.encode(out);
    }
}

impl Decode for GameProfile {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            uuid: Uuid::decode(buf)?,
            name: decode_string(buf, 16)?,
            properties: decode_list(buf, 16)?,
        })
    }
}

impl Encode for ProfileProperty {
    fn encode(&self, out: &mut Vec<u8>) {
        self.name.encode(out);
        self.value.encode(out);
        self.signature.encode(out);
    }
}

impl Decode for ProfileProperty {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            name: decode_string(buf, 64)?,
            value: String::decode(buf)?,
            signature: if bool::decode(buf)? {
                Some(decode_string(buf, 1024)?)
            } else {
                None
            },
        })
    }
}

/// `minecraft:login_finished`, clientbound: the accepted profile, followed by
/// the server's session id (random per server start in vanilla).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginFinished {
    pub profile: GameProfile,
    pub session_id: Uuid,
}

impl Packet for LoginFinished {
    const ID: i32 = 0x02;
}

impl Encode for LoginFinished {
    fn encode(&self, out: &mut Vec<u8>) {
        self.profile.encode(out);
        self.session_id.encode(out);
    }
}

impl Decode for LoginFinished {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            profile: GameProfile::decode(buf)?,
            session_id: Uuid::decode(buf)?,
        })
    }
}

packet! {
    /// `minecraft:hello`, clientbound: online mode's key exchange. The
    /// client answers with [`Key`], encrypted with `public_key`.
    EncryptionRequest = 0x01 {
        /// Always empty since 1.7.
        server_id: String,
        /// RSA public key, DER-encoded `SubjectPublicKeyInfo`.
        public_key: ByteArray,
        /// Random bytes the client must send back encrypted.
        challenge: ByteArray,
        /// Whether the client must report the join to Mojang first.
        should_authenticate: bool,
    }
}

packet! {
    /// `minecraft:key`, serverbound: the shared secret and the challenge,
    /// each encrypted with the server's public key (PKCS #1 v1.5).
    Key = 0x01 {
        secret: ByteArray,
        challenge: ByteArray,
    }
}

/// `minecraft:login_compression`, clientbound: from the next packet on, both
/// sides use the compressed frame format with this threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginCompression {
    pub threshold: i32,
}

impl Packet for LoginCompression {
    const ID: i32 = 0x03;
}

impl Encode for LoginCompression {
    fn encode(&self, out: &mut Vec<u8>) {
        VarInt(self.threshold).encode(out);
    }
}

impl Decode for LoginCompression {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            threshold: VarInt::decode(buf)?.0,
        })
    }
}

empty_packet! {
    /// `minecraft:login_acknowledged`, serverbound: the client switches to
    /// configuration.
    LoginAcknowledged = 0x03
}
