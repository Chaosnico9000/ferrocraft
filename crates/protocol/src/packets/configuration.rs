//! Configuration state: registries, tags and feature flags, sent between login
//! and play.

use super::{Packet, empty_packet};
use crate::codec::{Decode, Encode, Result, VarInt, decode_list, decode_string};
use crate::nbt::Nbt;

/// `minecraft:client_information`, serverbound: the client's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInformation {
    pub language: String,
    pub view_distance: i8,
    pub chat_visibility: i32,
    pub chat_colors: bool,
    pub model_customisation: u8,
    pub main_hand: i32,
    pub text_filtering: bool,
    pub allows_listing: bool,
    pub particle_status: i32,
}

impl Packet for ClientInformation {
    const ID: i32 = 0x00;
}

impl Encode for ClientInformation {
    fn encode(&self, out: &mut Vec<u8>) {
        self.language.encode(out);
        self.view_distance.encode(out);
        VarInt(self.chat_visibility).encode(out);
        self.chat_colors.encode(out);
        self.model_customisation.encode(out);
        VarInt(self.main_hand).encode(out);
        self.text_filtering.encode(out);
        self.allows_listing.encode(out);
        VarInt(self.particle_status).encode(out);
    }
}

impl Decode for ClientInformation {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            language: decode_string(buf, 16)?,
            view_distance: i8::decode(buf)?,
            chat_visibility: VarInt::decode(buf)?.0,
            chat_colors: bool::decode(buf)?,
            model_customisation: u8::decode(buf)?,
            main_hand: VarInt::decode(buf)?.0,
            text_filtering: bool::decode(buf)?,
            allows_listing: bool::decode(buf)?,
            particle_status: VarInt::decode(buf)?.0,
        })
    }
}

/// `minecraft:custom_payload`, clientbound: a plugin channel message. The data
/// is the rest of the packet; the `minecraft:brand` payload is one string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomPayload {
    pub channel: String,
    pub data: Vec<u8>,
}

impl CustomPayload {
    pub fn brand(brand: &str) -> Self {
        let mut data = Vec::new();
        brand.encode(&mut data);
        Self {
            channel: "minecraft:brand".into(),
            data,
        }
    }
}

impl Packet for CustomPayload {
    const ID: i32 = 0x01;
}

impl Encode for CustomPayload {
    fn encode(&self, out: &mut Vec<u8>) {
        self.channel.encode(out);
        out.extend_from_slice(&self.data);
    }
}

impl Decode for CustomPayload {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let channel = String::decode(buf)?;
        let data = std::mem::take(buf).to_vec();
        Ok(Self { channel, data })
    }
}

/// `minecraft:disconnect`, clientbound. From configuration on, the reason is
/// an NBT text component.
#[derive(Debug, Clone, PartialEq)]
pub struct Disconnect {
    pub reason: Nbt,
}

impl Packet for Disconnect {
    const ID: i32 = 0x02;
}

impl Encode for Disconnect {
    fn encode(&self, out: &mut Vec<u8>) {
        self.reason.encode(out);
    }
}

impl Decode for Disconnect {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            reason: Nbt::decode(buf)?,
        })
    }
}

empty_packet! {
    /// `minecraft:finish_configuration`, clientbound: the server is done.
    ClientboundFinishConfiguration = 0x03
}

empty_packet! {
    /// `minecraft:finish_configuration`, serverbound: the client switches to play.
    ServerboundFinishConfiguration = 0x03
}

/// `minecraft:keep_alive` in either direction: the client echoes the id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientboundKeepAlive {
    pub id: i64,
}

impl Packet for ClientboundKeepAlive {
    const ID: i32 = 0x04;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundKeepAlive {
    pub id: i64,
}

impl Packet for ServerboundKeepAlive {
    const ID: i32 = 0x04;
}

macro_rules! keep_alive_codec {
    ($($name:ty),*) => {$(
        impl Encode for $name {
            fn encode(&self, out: &mut Vec<u8>) {
                self.id.encode(out);
            }
        }

        impl Decode for $name {
            fn decode(buf: &mut &[u8]) -> Result<Self> {
                Ok(Self { id: i64::decode(buf)? })
            }
        }
    )*};
}
pub(crate) use keep_alive_codec;

keep_alive_codec!(ClientboundKeepAlive, ServerboundKeepAlive);

/// One entry of a synchronized registry. Without data the client takes the
/// entry from a data pack both sides know (see [`KnownPack`]).
#[derive(Debug, Clone, PartialEq)]
pub struct RegistryEntry {
    pub id: String,
    pub data: Option<Nbt>,
}

impl Encode for RegistryEntry {
    fn encode(&self, out: &mut Vec<u8>) {
        self.id.encode(out);
        self.data.encode(out);
    }
}

impl Decode for RegistryEntry {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            id: String::decode(buf)?,
            data: Option::decode(buf)?,
        })
    }
}

/// `minecraft:registry_data`, clientbound: all entries of one registry, in the
/// order that defines their network ids.
#[derive(Debug, Clone, PartialEq)]
pub struct RegistryData {
    pub registry: String,
    pub entries: Vec<RegistryEntry>,
}

impl Packet for RegistryData {
    const ID: i32 = 0x07;
}

impl Encode for RegistryData {
    fn encode(&self, out: &mut Vec<u8>) {
        self.registry.encode(out);
        self.entries.encode(out);
    }
}

impl Decode for RegistryData {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            registry: String::decode(buf)?,
            entries: decode_list(buf, usize::MAX)?,
        })
    }
}

/// `minecraft:update_enabled_features`, clientbound: feature flags such as
/// `minecraft:vanilla`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateEnabledFeatures {
    pub features: Vec<String>,
}

impl Packet for UpdateEnabledFeatures {
    const ID: i32 = 0x0d;
}

impl Encode for UpdateEnabledFeatures {
    fn encode(&self, out: &mut Vec<u8>) {
        self.features.encode(out);
    }
}

impl Decode for UpdateEnabledFeatures {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            features: decode_list(buf, usize::MAX)?,
        })
    }
}

/// The tags of one registry: tag name and the network ids of its entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryTags {
    pub registry: String,
    pub tags: Vec<(String, Vec<i32>)>,
}

impl Encode for RegistryTags {
    fn encode(&self, out: &mut Vec<u8>) {
        self.registry.encode(out);
        VarInt(self.tags.len() as i32).encode(out);
        for (name, ids) in &self.tags {
            name.encode(out);
            VarInt(ids.len() as i32).encode(out);
            ids.iter().for_each(|&id| VarInt(id).encode(out));
        }
    }
}

impl Decode for RegistryTags {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        let registry = String::decode(buf)?;
        let count = crate::codec::decode_count(buf, usize::MAX)?;
        let tags = (0..count)
            .map(|_| {
                let name = String::decode(buf)?;
                let ids = decode_list::<VarInt>(buf, usize::MAX)?;
                Ok((name, ids.into_iter().map(|VarInt(id)| id).collect()))
            })
            .collect::<Result<_>>()?;
        Ok(Self { registry, tags })
    }
}

/// `minecraft:update_tags`, clientbound (also sent in play after a reload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateTags {
    pub registries: Vec<RegistryTags>,
}

impl Packet for UpdateTags {
    const ID: i32 = 0x0e;
}

impl Encode for UpdateTags {
    fn encode(&self, out: &mut Vec<u8>) {
        self.registries.encode(out);
    }
}

impl Decode for UpdateTags {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            registries: decode_list(buf, usize::MAX)?,
        })
    }
}

/// A data pack identified across server and client, e.g. `minecraft:core` in
/// version `26.3`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownPack {
    pub namespace: String,
    pub id: String,
    pub version: String,
}

impl KnownPack {
    /// The built-in vanilla pack of [`crate::GAME_VERSION`].
    pub fn vanilla_core() -> Self {
        Self {
            namespace: "minecraft".into(),
            id: "core".into(),
            version: crate::GAME_VERSION.into(),
        }
    }
}

impl Encode for KnownPack {
    fn encode(&self, out: &mut Vec<u8>) {
        self.namespace.encode(out);
        self.id.encode(out);
        self.version.encode(out);
    }
}

impl Decode for KnownPack {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            namespace: String::decode(buf)?,
            id: String::decode(buf)?,
            version: String::decode(buf)?,
        })
    }
}

/// `minecraft:select_known_packs`, clientbound: the packs the server's
/// registries come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundSelectKnownPacks {
    pub packs: Vec<KnownPack>,
}

impl Packet for ClientboundSelectKnownPacks {
    const ID: i32 = 0x0f;
}

impl Encode for ClientboundSelectKnownPacks {
    fn encode(&self, out: &mut Vec<u8>) {
        self.packs.encode(out);
    }
}

impl Decode for ClientboundSelectKnownPacks {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            packs: decode_list(buf, usize::MAX)?,
        })
    }
}

/// `minecraft:select_known_packs`, serverbound: which of those packs the client
/// has too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundSelectKnownPacks {
    pub packs: Vec<KnownPack>,
}

impl Packet for ServerboundSelectKnownPacks {
    const ID: i32 = 0x07;
}

impl Encode for ServerboundSelectKnownPacks {
    fn encode(&self, out: &mut Vec<u8>) {
        self.packs.encode(out);
    }
}

impl Decode for ServerboundSelectKnownPacks {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            packs: decode_list(buf, 64)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_known_packs_matches_vanilla_capture() {
        // Captured from vanilla 26.3: one pack, minecraft:core 26.3.
        let bytes = b"\x01\x09minecraft\x04core\x0426.3";
        let packet = ClientboundSelectKnownPacks::decode(&mut &bytes[..]).unwrap();
        assert_eq!(packet.packs, [KnownPack::vanilla_core()]);
        let mut out = Vec::new();
        packet.encode(&mut out);
        assert_eq!(out, bytes);
    }

    #[test]
    fn brand_payload_is_a_string() {
        let mut out = Vec::new();
        CustomPayload::brand("vanilla").encode(&mut out);
        assert_eq!(out, b"\x0fminecraft:brand\x07vanilla");
    }
}
