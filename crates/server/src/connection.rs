use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use ferrocraft_protocol::crypt::Cipher;
use ferrocraft_protocol::frame::{FrameCodec, RawPacket};
use ferrocraft_protocol::packets::Packet;
use ferrocraft_protocol::packets::handshake::{Intent, Intention};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::{Server, configuration, login, play, status};

/// Time a client gets for the handshake plus the status exchange. Also ends
/// legacy (pre-1.7) pings, whose first byte reads as an unfinished frame
/// length.
const PRE_PLAY_TIMEOUT: Duration = Duration::from_secs(10);
/// Time for the login, which in online mode includes asking Mojang; vanilla
/// allows 600 ticks.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(30);
/// Time for the configuration phase. The client parses every registry and
/// tag in it, which can take a while on a slow machine; vanilla's limit is its
/// 30 s read timeout.
const CONFIGURATION_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Connection {
    stream: TcpStream,
    read_buf: Vec<u8>,
    write_buf: Vec<u8>,
    body_buf: Vec<u8>,
    codec: FrameCodec,
    cipher: Option<Cipher>,
}

impl Connection {
    fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            read_buf: Vec::with_capacity(512),
            write_buf: Vec::with_capacity(512),
            body_buf: Vec::with_capacity(512),
            codec: FrameCodec::default(),
            cipher: None,
        }
    }

    /// Encrypts everything from here on, both ways; call right after reading
    /// the client's `key` packet.
    pub fn enable_encryption(&mut self, secret: &[u8; 16]) {
        let mut cipher = Cipher::new(secret);
        // Whatever arrived after the key packet is already encrypted.
        cipher.decrypt(&mut self.read_buf);
        self.cipher = Some(cipher);
    }

    /// Switches both directions to the compressed format; call right after
    /// sending `login_compression`.
    pub fn enable_compression(&mut self, threshold: usize) {
        self.codec.enable_compression(threshold);
    }

    /// Reads the next frame. `Ok(None)` means the client closed the connection
    /// between packets.
    pub async fn read_raw(&mut self) -> anyhow::Result<Option<RawPacket>> {
        loop {
            if let Some((packet, used)) = self.codec.decode(&self.read_buf)? {
                self.read_buf.drain(..used);
                return Ok(Some(packet));
            }
            let before = self.read_buf.len();
            if self.stream.read_buf(&mut self.read_buf).await? == 0 {
                if self.read_buf.is_empty() {
                    return Ok(None);
                }
                bail!("connection closed mid-packet");
            }
            if let Some(cipher) = &mut self.cipher {
                cipher.decrypt(&mut self.read_buf[before..]);
            }
        }
    }

    pub async fn read<P: Packet>(&mut self) -> anyhow::Result<P> {
        let raw = self.read_raw().await?.context("connection closed")?;
        P::from_raw(&raw).with_context(|| format!("decoding packet 0x{:02x}", raw.id))
    }

    pub async fn send<P: Packet>(&mut self, packet: &P) -> anyhow::Result<()> {
        let mut body = std::mem::take(&mut self.body_buf);
        body.clear();
        packet.encode(&mut body);
        let result = self.send_raw(P::ID, &body).await;
        self.body_buf = body;
        result
    }

    /// Sends an already encoded packet body.
    pub async fn send_raw(&mut self, id: i32, body: &[u8]) -> anyhow::Result<()> {
        self.write_buf.clear();
        self.codec.encode(id, body, &mut self.write_buf);
        if let Some(cipher) = &mut self.cipher {
            cipher.encrypt(&mut self.write_buf);
        }
        self.stream.write_all(&self.write_buf).await?;
        Ok(())
    }
}

pub async fn handle(server: Arc<Server>, stream: TcpStream) -> anyhow::Result<()> {
    stream.set_nodelay(true)?;
    let mut conn = Connection::new(stream);
    let intention: Intention = tokio::time::timeout(PRE_PLAY_TIMEOUT, conn.read())
        .await
        .context("client took too long")??;
    let profile = match intention.intent {
        Intent::Status => {
            return tokio::time::timeout(PRE_PLAY_TIMEOUT, status::handle(&server, &mut conn))
                .await
                .context("client took too long")?;
        }
        Intent::Login | Intent::Transfer => tokio::time::timeout(
            LOGIN_TIMEOUT,
            login::handle(&server, &mut conn, intention.protocol_version),
        )
        .await
        .context("client took too long to log in")??,
    };
    let Some(profile) = profile else {
        return Ok(());
    };

    let settings = tokio::time::timeout(
        CONFIGURATION_TIMEOUT,
        configuration::handle(&server, &mut conn),
    )
    .await
    .context("client took too long to configure")??;
    let Some(settings) = settings else {
        return Ok(());
    };

    // From here on keep-alives decide whether the client is still there.
    play::handle(&server, &mut conn, profile, settings).await
}
