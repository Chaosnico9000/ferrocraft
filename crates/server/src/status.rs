use std::sync::atomic::Ordering;

use anyhow::bail;
use ferrocraft_protocol::packets::Packet;
use ferrocraft_protocol::packets::status::{
    PingRequest, PongResponse, StatusRequest, StatusResponse,
};
use ferrocraft_protocol::{GAME_VERSION, PROTOCOL_VERSION};
use serde_json::json;

use crate::Server;
use crate::connection::Connection;

/// Server list ping. Like vanilla, the status request may come at most once and
/// a ping is answered whether or not it came first; the pong ends the exchange.
pub async fn handle(server: &Server, conn: &mut Connection) -> anyhow::Result<()> {
    let mut answered_status = false;
    // The client may close at any point, e.g. when it only wanted the MOTD.
    while let Some(raw) = conn.read_raw().await? {
        match raw.id {
            StatusRequest::ID if !answered_status => {
                StatusRequest::from_raw(&raw)?;
                answered_status = true;
                conn.send(&StatusResponse {
                    json: status_json(server),
                })
                .await?;
            }
            PingRequest::ID => {
                let ping = PingRequest::from_raw(&raw)?;
                return conn
                    .send(&PongResponse {
                        payload: ping.payload,
                    })
                    .await;
            }
            id => bail!("unexpected status packet 0x{id:02x}"),
        }
    }
    Ok(())
}

fn status_json(server: &Server) -> String {
    json!({
        "version": { "name": GAME_VERSION, "protocol": PROTOCOL_VERSION },
        "players": {
            "max": server.config.max_players,
            "online": server.online_players.load(Ordering::Relaxed),
            "sample": [],
        },
        "description": { "text": server.config.motd },
        "enforcesSecureChat": false,
    })
    .to_string()
}
