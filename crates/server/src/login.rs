use std::sync::atomic::Ordering;

use anyhow::{Context, bail};
use ferrocraft_protocol::codec::ByteArray;
use ferrocraft_protocol::crypt::server_hash;
use ferrocraft_protocol::packets::login::{
    EncryptionRequest, GameProfile, Hello, Key, LoginAcknowledged, LoginCompression,
    LoginDisconnect, LoginFinished,
};
use ferrocraft_protocol::{GAME_VERSION, PROTOCOL_VERSION};
use md5::{Digest, Md5};
use serde_json::json;
use tracing::{info, warn};
use uuid::Uuid;

use crate::Server;
use crate::auth::{self, Keys};
use crate::connection::Connection;

/// The login. Returns the player's profile once the client has acknowledged
/// it and switched to configuration, `None` if it was turned away.
pub async fn handle(
    server: &Server,
    conn: &mut Connection,
    protocol_version: i32,
) -> anyhow::Result<Option<GameProfile>> {
    if protocol_version != PROTOCOL_VERSION {
        let side = if protocol_version < PROTOCOL_VERSION {
            "Dein Client ist zu alt"
        } else {
            "Dein Client ist zu neu"
        };
        disconnect(
            conn,
            &format!("{side}: dieser Server spricht {GAME_VERSION}."),
        )
        .await?;
        return Ok(None);
    }

    let hello: Hello = conn.read().await?;
    // `StringUtil.isValidPlayerName`; the decoder already capped the length.
    if hello.name.chars().any(|c| c <= ' ' || c >= '\u{7f}') {
        disconnect(conn, "Ungültige Zeichen im Namen.").await?;
        return Ok(None);
    }
    if server.online_players.load(Ordering::Relaxed) >= server.config.max_players {
        disconnect(conn, "Der Server ist voll.").await?;
        return Ok(None);
    }

    let profile = match &server.keys {
        Some(keys) => match authenticate(server, conn, keys, &hello.name).await? {
            Some(profile) => profile,
            None => return Ok(None),
        },
        None => GameProfile {
            uuid: offline_uuid(&hello.name),
            name: hello.name,
            properties: Vec::new(),
        },
    };
    info!(name = %profile.name, uuid = %profile.uuid, "login");
    if let Ok(threshold) = usize::try_from(server.config.compression_threshold) {
        conn.send(&LoginCompression {
            threshold: threshold as i32,
        })
        .await?;
        conn.enable_compression(threshold);
    }
    conn.send(&LoginFinished {
        profile: profile.clone(),
        session_id: server.session_id,
    })
    .await?;
    conn.read::<LoginAcknowledged>().await?;
    Ok(Some(profile))
}

/// Online mode's key exchange, then the check with Mojang. `None` if the
/// player was turned away.
async fn authenticate(
    server: &Server,
    conn: &mut Connection,
    keys: &Keys,
    name: &str,
) -> anyhow::Result<Option<GameProfile>> {
    let challenge: [u8; 4] = rand::random();
    conn.send(&EncryptionRequest {
        server_id: String::new(),
        public_key: ByteArray(keys.public_der.clone()),
        challenge: ByteArray(challenge.to_vec()),
        should_authenticate: true,
    })
    .await?;
    let key: Key = conn.read().await?;
    if keys.decrypt(&key.challenge.0)? != challenge {
        bail!("the client sent back the wrong challenge");
    }
    let secret: [u8; 16] = keys
        .decrypt(&key.secret.0)?
        .try_into()
        .ok()
        .context("the shared secret is not 16 bytes")?;
    conn.enable_encryption(&secret);

    let hash = server_hash("", &secret, &keys.public_der);
    let url = &server.config.session_server;
    match auth::has_joined(&server.http, url, name, &hash).await {
        Ok(Some(profile)) => Ok(Some(profile)),
        Ok(None) => {
            warn!(%name, "tried to join with an invalid session");
            disconnect_translated(conn, "multiplayer.disconnect.unverified_username").await?;
            Ok(None)
        }
        Err(err) => {
            warn!(%name, "could not verify the player: {err:#}");
            disconnect_translated(conn, "multiplayer.disconnect.authservers_down").await?;
            Ok(None)
        }
    }
}

async fn disconnect(conn: &mut Connection, text: &str) -> anyhow::Result<()> {
    conn.send(&LoginDisconnect {
        reason_json: json!({ "text": text }).to_string(),
    })
    .await
}

/// A message the client shows in its own language.
async fn disconnect_translated(conn: &mut Connection, key: &str) -> anyhow::Result<()> {
    conn.send(&LoginDisconnect {
        reason_json: json!({ "translate": key }).to_string(),
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
