//! The command tree the client completes and checks commands against, and
//! the packets around permissions and game modes.

use super::super::packet;
use crate::codec::{Decode, Encode, Error, Result, VarInt};

/// One node of the tree (`ClientboundCommandsPacket.Entry`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandNode {
    pub kind: NodeKind,
    /// The command can be run when it ends here.
    pub executable: bool,
    pub children: Vec<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    Root,
    Literal(String),
    /// `parser` is the `minecraft:command_argument_type` id, `properties`
    /// its encoded settings (e.g. the bounds of an integer).
    Argument {
        name: String,
        parser: i32,
        properties: Vec<u8>,
        /// Ask the server for suggestions, e.g. `minecraft:ask_server`.
        suggestions: Option<String>,
    },
}

/// `minecraft:commands`, clientbound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commands {
    pub nodes: Vec<CommandNode>,
    pub root: i32,
}

impl super::Packet for Commands {
    const ID: i32 = 0x10;
}

impl Encode for Commands {
    fn encode(&self, out: &mut Vec<u8>) {
        VarInt(self.nodes.len() as i32).encode(out);
        for node in &self.nodes {
            let mut flags = match node.kind {
                NodeKind::Root => 0u8,
                NodeKind::Literal(_) => 1,
                NodeKind::Argument { .. } => 2,
            };
            if node.executable {
                flags |= 0x04;
            }
            if matches!(
                node.kind,
                NodeKind::Argument {
                    suggestions: Some(_),
                    ..
                }
            ) {
                flags |= 0x10;
            }
            flags.encode(out);
            VarInt(node.children.len() as i32).encode(out);
            node.children.iter().for_each(|&c| VarInt(c).encode(out));
            match &node.kind {
                NodeKind::Root => {}
                NodeKind::Literal(name) => name.encode(out),
                NodeKind::Argument {
                    name,
                    parser,
                    properties,
                    suggestions,
                } => {
                    name.encode(out);
                    VarInt(*parser).encode(out);
                    out.extend_from_slice(properties);
                    if let Some(suggestions) = suggestions {
                        suggestions.encode(out);
                    }
                }
            }
        }
        VarInt(self.root).encode(out);
    }
}

impl Decode for Commands {
    /// Argument settings have a layout per parser, which this crate does not
    /// know; only the server sends this packet.
    fn decode(_: &mut &[u8]) -> Result<Self> {
        Err(Error::Invalid("commands packets are not decoded"))
    }
}

packet! {
    /// `minecraft:entity_event`, clientbound: a one-byte event of an entity;
    /// for the player itself 24 to 28 set its permission level 0 to 4.
    EntityEvent = 0x22 {
        entity_id: i32,
        event: u8,
    }
}

/// [`EntityEvent`] for permission level 0 (24) to 4 (28).
pub const PERMISSION_LEVEL_EVENT: u8 = 24;

packet! {
    /// `minecraft:change_game_mode`, serverbound: the F3+F4 switcher.
    ChangeGameMode = 0x05 {
        mode: i32 as varint,
    }
}
