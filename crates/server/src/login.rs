use ferrocraft_protocol::packets::login::{Hello, LoginDisconnect};
use ferrocraft_protocol::{GAME_VERSION, PROTOCOL_VERSION};
use md5::{Digest, Md5};
use serde_json::json;
use tracing::info;
use uuid::Uuid;

use crate::Server;
use crate::connection::Connection;

/// Login in offline mode. There is no world to join yet, so every login ends
/// with a disconnect that tells the player so.
pub async fn handle(
    _server: &Server,
    conn: &mut Connection,
    protocol_version: i32,
) -> anyhow::Result<()> {
    if protocol_version != PROTOCOL_VERSION {
        let side = if protocol_version < PROTOCOL_VERSION {
            "Dein Client ist zu alt"
        } else {
            "Dein Client ist zu neu"
        };
        return disconnect(
            conn,
            &format!("{side}: dieser Server spricht {GAME_VERSION}."),
        )
        .await;
    }

    let hello: Hello = conn.read().await?;
    let uuid = offline_uuid(&hello.name);
    info!(name = %hello.name, %uuid, "login");
    disconnect(
        conn,
        &format!(
            "Hallo {}! Ferrocraft kann noch keine Welt laden – Login klappt aber schon.",
            hello.name
        ),
    )
    .await
}

async fn disconnect(conn: &mut Connection, text: &str) -> anyhow::Result<()> {
    conn.send(&LoginDisconnect {
        reason_json: json!({ "text": text }).to_string(),
    })
    .await
}

/// The UUID vanilla assigns a player in offline mode: a version-3 UUID over the
/// MD5 of `OfflinePlayer:<name>`, without a namespace.
pub fn offline_uuid(name: &str) -> Uuid {
    let hash: [u8; 16] = Md5::digest(format!("OfflinePlayer:{name}")).into();
    uuid::Builder::from_md5_bytes(hash).into_uuid()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuid_matches_vanilla() {
        // Values as computed by vanilla's UUIDUtil.createOfflinePlayerUUID.
        assert_eq!(
            offline_uuid("Notch").to_string(),
            "b50ad385-829d-3141-a216-7e7d7539ba7f"
        );
        assert_eq!(offline_uuid("Notch").get_version_num(), 3);
    }
}
