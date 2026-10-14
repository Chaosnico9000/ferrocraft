//! A minimal client that speaks the protocol the way the vanilla client does,
//! for tests and the `bot` binary. Protocol errors panic: it is a probe, and a
//! panic with the offending packet is the most useful report.

use std::net::SocketAddr;
use std::time::Duration;

use ferrocraft_protocol::codec::ByteArray;
use ferrocraft_protocol::crypt::{Cipher, server_hash};
use ferrocraft_protocol::frame::{FrameCodec, RawPacket};
use ferrocraft_protocol::packets::Packet;
use ferrocraft_protocol::packets::configuration::{
    self, ClientInformation, ClientboundFinishConfiguration, ClientboundSelectKnownPacks,
    RegistryData, ServerboundFinishConfiguration, ServerboundSelectKnownPacks, UpdateTags,
};
use ferrocraft_protocol::packets::handshake::{Intent, Intention};
use ferrocraft_protocol::packets::login::{
    EncryptionRequest, Hello, Key, LoginAcknowledged, LoginCompression, LoginFinished,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

/// How long a read waits by default before the client gives up.
pub const READ_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Client {
    stream: TcpStream,
    buf: Vec<u8>,
    codec: FrameCodec,
    cipher: Option<Cipher>,
    /// The hash this client would report to Mojang in online mode.
    pub server_hash: Option<String>,
    /// Bytes read from the socket so far, as they came over the wire.
    pub received: usize,
    /// Bounds every read, so that a missing packet fails a test instead of
    /// hanging it.
    pub timeout: Duration,
}

/// What the server sent during configuration.
pub struct Configured {
    pub registries: Vec<RegistryData>,
    pub tags: UpdateTags,
}

impl Client {
    pub async fn connect(addr: SocketAddr, intent: Intent, protocol_version: i32) -> Self {
        let mut client = Self {
            stream: TcpStream::connect(addr).await.unwrap(),
            buf: Vec::new(),
            codec: FrameCodec::default(),
            cipher: None,
            server_hash: None,
            received: 0,
            timeout: READ_TIMEOUT,
        };
        client.stream.set_nodelay(true).unwrap();
        client
            .send(&Intention {
                protocol_version,
                host: "localhost".into(),
                port: addr.port(),
                intent,
            })
            .await;
        client
    }

    pub async fn send<P: Packet>(&mut self, packet: &P) {
        let mut body = Vec::new();
        packet.encode(&mut body);
        self.send_raw(P::ID, &body).await;
    }

    pub async fn send_raw(&mut self, id: i32, body: &[u8]) {
        let mut out = Vec::new();
        self.codec.encode(id, body, &mut out);
        self.send_bytes(&out).await;
    }

    /// Writes bytes bypassing the framing (but not the encryption).
    pub async fn send_bytes(&mut self, bytes: &[u8]) {
        let mut bytes = bytes.to_vec();
        if let Some(cipher) = &mut self.cipher {
            cipher.encrypt(&mut bytes);
        }
        self.stream.write_all(&bytes).await.unwrap();
    }

    pub fn compression_threshold(&self) -> Option<usize> {
        self.codec.compression_threshold()
    }

    /// The next frame, `None` once the server has closed the connection.
    /// Cancel safe: a frame is either returned whole or stays buffered.
    pub async fn read_raw(&mut self) -> Option<RawPacket> {
        tokio::time::timeout(self.timeout, async {
            loop {
                if let Some((raw, used)) = self.codec.decode(&self.buf).unwrap() {
                    self.buf.drain(..used);
                    return Some(raw);
                }
                let before = self.buf.len();
                let read = match self.stream.read_buf(&mut self.buf).await {
                    Ok(read) => read,
                    // A reset after a kick counts as closed, like a clean FIN.
                    Err(err) if err.kind() == std::io::ErrorKind::ConnectionReset => 0,
                    Err(err) => panic!("reading from the server: {err}"),
                };
                self.received += read;
                if let Some(cipher) = &mut self.cipher {
                    cipher.decrypt(&mut self.buf[before..]);
                }
                if read == 0 {
                    return None;
                }
            }
        })
        .await
        .expect("no packet from the server in time")
    }

    pub async fn read<P: Packet>(&mut self) -> P {
        let raw = self.read_raw().await.expect("server closed the connection");
        P::from_raw(&raw).unwrap_or_else(|e| {
            panic!(
                "expected {} (0x{:02x}), got 0x{:02x}: {e}",
                std::any::type_name::<P>(),
                P::ID,
                raw.id
            )
        })
    }

    /// Skips packets until one with `P`'s id arrives that decodes as `P`;
    /// some packets share an id (e.g. the forms of `player_info_update`).
    pub async fn read_until<P: Packet>(&mut self) -> P {
        loop {
            let raw = self.read_raw().await.expect("server closed the connection");
            if raw.id == P::ID
                && let Ok(packet) = P::from_raw(&raw)
            {
                return packet;
            }
        }
    }

    /// Handshake and login up to the switch to configuration, following
    /// `login_compression` and, in online mode, the key exchange (without
    /// asking Mojang: tests answer for it).
    pub async fn login(addr: SocketAddr, name: &str) -> (Self, LoginFinished) {
        let (client, finished) = Self::try_login(addr, name).await;
        match finished {
            Ok(finished) => (client, finished),
            Err(raw) => panic!("expected login_finished, got 0x{:02x}", raw.id),
        }
    }

    /// Like [`Client::login`], but hands back whatever the server sent
    /// instead of `login_finished`, such as a disconnect.
    pub async fn try_login(
        addr: SocketAddr,
        name: &str,
    ) -> (Self, Result<LoginFinished, RawPacket>) {
        let mut client =
            Self::connect(addr, Intent::Login, ferrocraft_protocol::PROTOCOL_VERSION).await;
        client
            .send(&Hello {
                name: name.into(),
                uuid: Uuid::new_v4(),
            })
            .await;
        let mut raw = client.read_raw().await.expect("closed during login");
        if raw.id == EncryptionRequest::ID {
            let request = EncryptionRequest::from_raw(&raw).unwrap();
            let public = crate::rsa::PublicKey::from_der(&request.public_key.0)
                .expect("the server's public key");
            let secret: [u8; 16] = rand::random();
            let encrypt = |data: &[u8]| ByteArray(public.encrypt(data));
            let key = Key {
                secret: encrypt(&secret),
                challenge: encrypt(&request.challenge.0),
            };
            client.send(&key).await;
            client.cipher = Some(Cipher::new(&secret));
            client.server_hash = Some(server_hash(
                &request.server_id,
                &secret,
                &request.public_key.0,
            ));
            raw = client
                .read_raw()
                .await
                .expect("closed after the key exchange");
        }
        if raw.id == LoginCompression::ID {
            let threshold = LoginCompression::from_raw(&raw).unwrap().threshold;
            client.codec.enable_compression(threshold as usize);
            raw = client.read_raw().await.expect("closed during login");
        }
        match LoginFinished::from_raw(&raw) {
            Ok(finished) => {
                client.send(&LoginAcknowledged).await;
                (client, Ok(finished))
            }
            Err(_) => (client, Err(raw)),
        }
    }

    /// Configuration as the vanilla client does it, with view distance
    /// `view`. Accepts whatever packs the server offers.
    pub async fn configure(&mut self, view: i8) -> Configured {
        self.send(&ClientInformation {
            language: "de_de".into(),
            view_distance: view,
            chat_visibility: 0,
            chat_colors: true,
            model_customisation: 0x7f,
            main_hand: 1,
            text_filtering: false,
            allows_listing: true,
            particle_status: 0,
        })
        .await;

        let mut registries = Vec::new();
        let mut tags = None;
        loop {
            let raw = self.read_raw().await.expect("closed during configuration");
            match raw.id {
                ClientboundSelectKnownPacks::ID => {
                    let offer = ClientboundSelectKnownPacks::from_raw(&raw).unwrap();
                    self.send(&ServerboundSelectKnownPacks { packs: offer.packs })
                        .await;
                }
                RegistryData::ID => registries.push(RegistryData::from_raw(&raw).unwrap()),
                UpdateTags::ID => tags = Some(UpdateTags::from_raw(&raw).unwrap()),
                configuration::ClientboundKeepAlive::ID => {
                    let id = configuration::ClientboundKeepAlive::from_raw(&raw)
                        .unwrap()
                        .id;
                    self.send(&configuration::ServerboundKeepAlive { id }).await;
                }
                configuration::Disconnect::ID => {
                    let bye = configuration::Disconnect::from_raw(&raw).unwrap();
                    panic!("disconnected during configuration: {:?}", bye.reason);
                }
                ClientboundFinishConfiguration::ID => break,
                _ => {}
            }
        }
        self.send(&ServerboundFinishConfiguration).await;
        Configured {
            registries,
            tags: tags.expect("no update_tags during configuration"),
        }
    }
}
