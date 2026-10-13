use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use ferrocraft_protocol::frame::{RawPacket, decode_frame};
use ferrocraft_protocol::packets::Packet;
use ferrocraft_protocol::packets::handshake::{Intent, Intention};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::{Server, login, status};

/// Time a client gets for the handshake plus the status or login exchange.
/// Also ends legacy (pre-1.7) pings, whose first byte reads as an unfinished
/// frame length.
const PRE_PLAY_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Connection {
    stream: TcpStream,
    read_buf: Vec<u8>,
    write_buf: Vec<u8>,
}

impl Connection {
    fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            read_buf: Vec::with_capacity(512),
            write_buf: Vec::with_capacity(512),
        }
    }

    /// Reads the next frame. `Ok(None)` means the client closed the connection
    /// between packets.
    pub async fn read_raw(&mut self) -> anyhow::Result<Option<RawPacket>> {
        loop {
            if let Some((packet, used)) = decode_frame(&self.read_buf)? {
                self.read_buf.drain(..used);
                return Ok(Some(packet));
            }
            if self.stream.read_buf(&mut self.read_buf).await? == 0 {
                if self.read_buf.is_empty() {
                    return Ok(None);
                }
                bail!("connection closed mid-packet");
            }
        }
    }

    pub async fn read<P: Packet>(&mut self) -> anyhow::Result<P> {
        let raw = self.read_raw().await?.context("connection closed")?;
        P::from_raw(&raw).with_context(|| format!("decoding packet 0x{:02x}", raw.id))
    }

    pub async fn send<P: Packet>(&mut self, packet: &P) -> anyhow::Result<()> {
        self.write_buf.clear();
        packet.to_frame(&mut self.write_buf);
        self.stream.write_all(&self.write_buf).await?;
        Ok(())
    }
}

pub async fn handle(server: Arc<Server>, stream: TcpStream) -> anyhow::Result<()> {
    stream.set_nodelay(true)?;
    let mut conn = Connection::new(stream);
    tokio::time::timeout(PRE_PLAY_TIMEOUT, async {
        let intention: Intention = conn.read().await?;
        match intention.intent {
            Intent::Status => status::handle(&server, &mut conn).await,
            Intent::Login | Intent::Transfer => {
                login::handle(&server, &mut conn, intention.protocol_version).await
            }
        }
    })
    .await
    .context("client took too long")?
}
