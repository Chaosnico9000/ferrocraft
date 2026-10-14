//! Chat: messages from players and text from the server.

use super::super::packet;
use crate::codec::{Decode, Encode, Result, VarInt, decode_string};
use crate::nbt::Nbt;

packet! {
    /// `minecraft:system_chat`, clientbound: a text component, shown in the
    /// chat or, with `overlay`, above the hotbar.
    SystemChat = 0x7c {
        content: Nbt,
        overlay: bool,
    }
}

/// Longest chat message a client may send.
pub const MAX_CHAT_LEN: usize = 256;

/// `minecraft:chat`, serverbound: a chat message. Without secure chat the
/// signature is absent and the server only needs the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chat {
    pub message: String,
    /// Milliseconds since the Unix epoch.
    pub timestamp: i64,
    pub salt: i64,
    pub signature: Option<Box<[u8; 256]>>,
    /// `LastSeenMessages.Update`: offset, 20 acknowledgement bits, checksum.
    pub last_seen_offset: i32,
    pub acknowledged: [u8; 3],
    pub checksum: u8,
}

impl super::super::Packet for Chat {
    const ID: i32 = 0x09;
}

impl Encode for Chat {
    fn encode(&self, out: &mut Vec<u8>) {
        self.message.encode(out);
        self.timestamp.encode(out);
        self.salt.encode(out);
        self.signature.is_some().encode(out);
        if let Some(signature) = &self.signature {
            out.extend_from_slice(&signature[..]);
        }
        VarInt(self.last_seen_offset).encode(out);
        self.acknowledged.encode(out);
        self.checksum.encode(out);
    }
}

impl Decode for Chat {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            message: decode_string(buf, MAX_CHAT_LEN)?,
            timestamp: i64::decode(buf)?,
            salt: i64::decode(buf)?,
            signature: if bool::decode(buf)? {
                let mut signature = Box::new([0u8; 256]);
                for byte in signature.iter_mut() {
                    *byte = u8::decode(buf)?;
                }
                Some(signature)
            } else {
                None
            },
            last_seen_offset: VarInt::decode(buf)?.0,
            acknowledged: <[u8; 3]>::decode(buf)?,
            checksum: u8::decode(buf)?,
        })
    }
}

impl Chat {
    /// An unsigned message, as a client without secure chat sends it.
    pub fn unsigned(message: &str) -> Self {
        Self {
            message: message.into(),
            timestamp: 0,
            salt: 0,
            signature: None,
            last_seen_offset: 0,
            acknowledged: [0; 3],
            checksum: 0,
        }
    }
}

packet! {
    /// `minecraft:chat_command`, serverbound: a command typed into the chat,
    /// without the slash.
    ChatCommand = 0x07 {
        command: String,
    }
}

/// Builders for the text components the server sends.
pub mod text {
    use crate::nbt::Nbt;

    pub fn plain(text: &str) -> Nbt {
        Nbt::text(text)
    }

    /// A translated message, e.g. `multiplayer.player.joined` with the
    /// player's name; the client fills in its own language.
    pub fn translate(key: &str, with: Vec<Nbt>, color: Option<&str>) -> Nbt {
        let mut entries = vec![("with".into(), Nbt::List(with))];
        if let Some(color) = color {
            entries.push(("color".into(), Nbt::String(color.into())));
        }
        entries.push(("translate".into(), Nbt::String(key.into())));
        Nbt::Compound(entries)
    }

    /// A player's name as vanilla shows it in messages: clicking suggests a
    /// private message, hovering shows the player.
    pub fn player_name(name: &str, uuid: uuid::Uuid) -> Nbt {
        let id = uuid.as_u128();
        let words = [96, 64, 32, 0].map(|shift| (id >> shift) as u32 as i32);
        Nbt::Compound(vec![
            (
                "click_event".into(),
                Nbt::Compound(vec![
                    ("action".into(), Nbt::String("suggest_command".into())),
                    ("command".into(), Nbt::String(format!("/tell {name} "))),
                ]),
            ),
            ("insertion".into(), Nbt::String(name.into())),
            ("text".into(), Nbt::String(name.into())),
            (
                "hover_event".into(),
                Nbt::Compound(vec![
                    ("name".into(), Nbt::String(name.into())),
                    ("action".into(), Nbt::String("show_entity".into())),
                    ("id".into(), Nbt::String("minecraft:player".into())),
                    ("uuid".into(), Nbt::IntArray(words.to_vec())),
                ]),
            ),
        ])
    }
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

    #[test]
    fn join_message_matches_vanilla_capture() {
        // What a vanilla 26.3 server sent when "Beta" joined.
        let captured = hex(concat!(
            "0a090004776974680a000000010a000b636c69636b5f6576656e740800066163",
            "74696f6e000f737567676573745f636f6d6d616e64080007636f6d6d616e6400",
            "0b2f74656c6c20426574612000080009696e73657274696f6e00044265746108",
            "0004746578740004426574610a000b686f7665725f6576656e740800046e616d",
            "65000442657461080006616374696f6e000b73686f775f656e74697479080002",
            "696400106d696e6563726166743a706c617965720b0004757569640000000411",
            "10dc55a244318c9843101d405cbf340000080005636f6c6f72000679656c6c6f",
            "770800097472616e736c61746500196d756c7469706c617965722e706c617965",
            "722e6a6f696e65640000",
        ));
        let uuid: uuid::Uuid = "1110dc55-a244-318c-9843-101d405cbf34".parse().unwrap();
        let message = SystemChat {
            content: text::translate(
                "multiplayer.player.joined",
                vec![text::player_name("Beta", uuid)],
                Some("yellow"),
            ),
            overlay: false,
        };
        let mut out = Vec::new();
        message.encode(&mut out);
        assert_eq!(out, captured);
        assert_eq!(SystemChat::decode(&mut out.as_slice()).unwrap(), message);
    }

    #[test]
    fn chat_roundtrips_with_and_without_signature() {
        for chat in [
            Chat::unsigned("Hallo Welt"),
            Chat {
                signature: Some(Box::new([7; 256])),
                ..Chat::unsigned("signiert")
            },
        ] {
            let mut out = Vec::new();
            chat.encode(&mut out);
            assert_eq!(Chat::decode(&mut out.as_slice()).unwrap(), chat);
        }
        let mut long = Vec::new();
        "x".repeat(257).encode(&mut long);
        assert!(Chat::decode(&mut long.as_slice()).is_err());
    }
}
