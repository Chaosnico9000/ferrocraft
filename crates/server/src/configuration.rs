//! Configuration: tells the client which data packs, registries and tags the
//! server uses, in the order vanilla's `ServerConfigurationPacketListenerImpl`
//! does.

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::packets::Packet;
use ferrocraft_protocol::packets::configuration::{
    ClientInformation, ClientboundFinishConfiguration, ClientboundSelectKnownPacks, CustomPayload,
    Disconnect, KnownPack, ServerboundFinishConfiguration, ServerboundSelectKnownPacks,
    UpdateEnabledFeatures,
};
use tracing::debug;

use crate::Server;
use crate::connection::Connection;

/// What the client told us about itself during configuration.
#[derive(Debug, Clone, Default)]
pub struct ClientSettings {
    pub information: Option<ClientInformation>,
}

/// Runs the configuration phase. Returns `None` if the client was turned
/// away, otherwise its settings once it has switched to play.
pub async fn handle(
    server: &Server,
    conn: &mut Connection,
) -> anyhow::Result<Option<ClientSettings>> {
    let mut settings = ClientSettings::default();
    conn.send(&CustomPayload::brand("ferrocraft")).await?;
    conn.send(&UpdateEnabledFeatures {
        features: vec!["minecraft:vanilla".into()],
    })
    .await?;
    let core = KnownPack::vanilla_core();
    conn.send(&ClientboundSelectKnownPacks {
        packs: vec![core.clone()],
    })
    .await?;

    let accepted = loop {
        let raw = conn.read_raw().await?.ok_or_else(closed)?;
        match raw.id {
            ServerboundSelectKnownPacks::ID => {
                break ServerboundSelectKnownPacks::from_raw(&raw)?.packs;
            }
            ClientInformation::ID => {
                settings.information = Some(ClientInformation::from_raw(&raw)?)
            }
            id => debug!("ignoring configuration packet 0x{id:02x}"),
        }
    };
    // Registry entries go out without their data, so the client must have the
    // vanilla pack of exactly this version.
    if !accepted.contains(&core) {
        conn.send(&Disconnect {
            reason: Nbt::text(&format!(
                "Dein Client kennt das Vanilla-Datenpaket {} nicht.",
                core.version
            )),
        })
        .await?;
        return Ok(None);
    }

    for registry in &server.data.registries {
        conn.send(registry).await?;
    }
    conn.send(&server.data.tags).await?;
    conn.send(&ClientboundFinishConfiguration).await?;

    loop {
        let raw = conn.read_raw().await?.ok_or_else(closed)?;
        match raw.id {
            ServerboundFinishConfiguration::ID => {
                ServerboundFinishConfiguration::from_raw(&raw)?;
                return Ok(Some(settings));
            }
            ClientInformation::ID => {
                settings.information = Some(ClientInformation::from_raw(&raw)?)
            }
            id => debug!("ignoring configuration packet 0x{id:02x}"),
        }
    }
}

fn closed() -> anyhow::Error {
    anyhow::anyhow!("client closed the connection during configuration")
}
