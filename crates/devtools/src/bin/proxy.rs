//! Logging proxy: sits between a client and a server and prints every packet
//! with its state, direction and name. Bytes are forwarded unchanged.
//!
//!   proxy <listen addr> <upstream addr> [--log packets.jsonl] [--quiet]
//!
//! `--log` also writes each packet body as hex, one JSON object per line.
//! `--quiet` keeps stdout to the state changes and disconnects.
//!
//! The upstream must run in offline mode: encrypted traffic is still forwarded
//! but no longer decoded. Compressed packets are logged unpacked.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, bail};
use ferrocraft_protocol::Decode;
use ferrocraft_protocol::frame::{FrameCodec, RawPacket};
use ferrocraft_protocol::packets::handshake::{Intent, Intention};
use ferrocraft_protocol::packets::login::LoginCompression;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};

const PACKETS_JSON: &str = include_str!("../../../../data/26.3/packets.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum State {
    Handshake,
    Status,
    Login,
    Configuration,
    Play,
    /// Encryption started; bytes are passed through blindly.
    Opaque,
}

impl State {
    fn key(self) -> &'static str {
        match self {
            State::Handshake => "handshake",
            State::Status => "status",
            State::Login => "login",
            State::Configuration => "configuration",
            State::Play => "play",
            State::Opaque => "opaque",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Dir {
    Serverbound,
    Clientbound,
}

impl Dir {
    fn key(self) -> &'static str {
        match self {
            Dir::Serverbound => "serverbound",
            Dir::Clientbound => "clientbound",
        }
    }

    fn arrow(self) -> &'static str {
        match self {
            Dir::Serverbound => "C→S",
            Dir::Clientbound => "S→C",
        }
    }
}

/// Packet names by (state, direction, id), from the data generator report.
struct Names(HashMap<(State, Dir, i32), String>);

impl Names {
    fn load() -> anyhow::Result<Self> {
        let report: serde_json::Value = serde_json::from_str(PACKETS_JSON)?;
        let mut names = HashMap::new();
        for state in [
            State::Handshake,
            State::Status,
            State::Login,
            State::Configuration,
            State::Play,
        ] {
            for dir in [Dir::Serverbound, Dir::Clientbound] {
                let Some(packets) = report[state.key()][dir.key()].as_object() else {
                    continue;
                };
                for (name, packet) in packets {
                    let id = packet["protocol_id"].as_i64().context("protocol_id")? as i32;
                    names.insert((state, dir, id), name.clone());
                }
            }
        }
        Ok(Self(names))
    }

    fn get(&self, state: State, dir: Dir, id: i32) -> &str {
        self.0
            .get(&(state, dir, id))
            .map_or("<unknown>", String::as_str)
    }
}

/// Each direction switches state on its own packets: the server's side moves
/// on when it sends `login_finished`, the client's when it acknowledges.
struct Session {
    states: HashMap<Dir, State>,
    /// Threshold from `login_compression`; applies to both directions.
    compression: Option<usize>,
    started: Instant,
    log: Option<std::fs::File>,
    quiet: bool,
    id: u64,
}

impl Session {
    fn state(&self, dir: Dir) -> State {
        self.states[&dir]
    }

    fn set(&mut self, dir: Dir, state: State) {
        if self.states.insert(dir, state) != Some(state) {
            println!(
                "[{}] {:>8.3}s {} → {}",
                self.id,
                self.started.elapsed().as_secs_f64(),
                dir.arrow(),
                state.key()
            );
        }
    }

    fn on_packet(&mut self, names: &Names, dir: Dir, packet: &RawPacket) -> anyhow::Result<()> {
        let state = self.state(dir);
        let name = names.get(state, dir, packet.id);
        let t = self.started.elapsed().as_secs_f64();
        let important = name.contains("disconnect") || !self.quiet;
        if important {
            println!(
                "[{}] {t:>8.3}s {} {:<13} {name} ({} B)",
                self.id,
                dir.arrow(),
                state.key(),
                packet.body.len()
            );
        }
        if let Some(log) = &mut self.log {
            use std::io::Write;
            let hex: String = packet.body.iter().map(|b| format!("{b:02x}")).collect();
            let line = serde_json::json!({
                "session": self.id, "t": (t * 1000.0).round() / 1000.0, "dir": dir.key(),
                "state": state.key(), "id": packet.id, "name": name,
                "len": packet.body.len(), "hex": hex,
            });
            // One write per line: sessions share the file, and a line split
            // over several writes would interleave with other sessions.
            log.write_all(format!("{line}\n").as_bytes())?;
        }

        match (state, dir, name) {
            (State::Handshake, Dir::Serverbound, "minecraft:intention") => {
                let intention = Intention::decode(&mut packet.body.as_slice())?;
                let next = match intention.intent {
                    Intent::Status => State::Status,
                    Intent::Login | Intent::Transfer => State::Login,
                };
                self.set(Dir::Serverbound, next);
                self.set(Dir::Clientbound, next);
            }
            (State::Login, Dir::Clientbound, "minecraft:hello") => {
                println!(
                    "[{}] encryption requested: upstream is in online mode",
                    self.id
                );
                self.set(Dir::Clientbound, State::Opaque);
                self.set(Dir::Serverbound, State::Opaque);
            }
            (State::Login, Dir::Clientbound, "minecraft:login_compression") => {
                let threshold = LoginCompression::decode(&mut packet.body.as_slice())?.threshold;
                println!("[{}] compression from {threshold} bytes", self.id);
                self.compression = usize::try_from(threshold).ok();
            }
            (State::Login, Dir::Clientbound, "minecraft:login_finished")
            | (State::Login, Dir::Serverbound, "minecraft:login_acknowledged")
            | (State::Play, Dir::Clientbound, "minecraft:start_configuration")
            | (State::Play, Dir::Serverbound, "minecraft:configuration_acknowledged") => {
                self.set(dir, State::Configuration);
            }
            (State::Configuration, _, "minecraft:finish_configuration") => {
                self.set(dir, State::Play);
            }
            _ => {}
        }
        Ok(())
    }
}

async fn pump(
    mut from: OwnedReadHalf,
    mut to: OwnedWriteHalf,
    dir: Dir,
    session: Arc<Mutex<Session>>,
    names: Arc<Names>,
) -> anyhow::Result<()> {
    let mut buf = Vec::with_capacity(64 * 1024);
    let mut codec = FrameCodec::default();
    loop {
        if from.read_buf(&mut buf).await? == 0 {
            return Ok(());
        }
        let mut used = 0;
        loop {
            let (state, compression) = {
                let session = session.lock().unwrap();
                (session.state(dir), session.compression)
            };
            if state == State::Opaque {
                used = buf.len();
                break;
            }
            if let Some(threshold) = compression
                && codec.compression_threshold().is_none()
            {
                codec.enable_compression(threshold);
            }
            match codec.decode(&buf[used..])? {
                Some((packet, len)) => {
                    session.lock().unwrap().on_packet(&names, dir, &packet)?;
                    used += len;
                }
                None => break,
            }
        }
        // Forward only whole frames, so the other side sees packets in order
        // with their state switches.
        to.write_all(&buf[..used]).await?;
        buf.drain(..used);
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(listen), Some(upstream)) = (args.next(), args.next()) else {
        bail!("usage: proxy <listen addr> <upstream addr> [--log packets.jsonl] [--quiet]");
    };
    let mut log_path = None;
    let mut quiet = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--log" => log_path = Some(args.next().context("--log needs a path")?),
            "--quiet" => quiet = true,
            _ => bail!("unknown argument {arg}"),
        }
    }

    let names = Arc::new(Names::load()?);
    let listener = TcpListener::bind(&listen).await?;
    println!("proxy {listen} → {upstream}");
    let mut next_id = 0;
    loop {
        let (client, peer) = listener.accept().await?;
        next_id += 1;
        let log = match &log_path {
            Some(path) => Some(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)?,
            ),
            None => None,
        };
        let session = Arc::new(Mutex::new(Session {
            states: HashMap::from([
                (Dir::Serverbound, State::Handshake),
                (Dir::Clientbound, State::Handshake),
            ]),
            compression: None,
            started: Instant::now(),
            log,
            quiet,
            id: next_id,
        }));
        let upstream = upstream.clone();
        let names = Arc::clone(&names);
        tokio::spawn(async move {
            println!("[{next_id}] connection from {peer}");
            let result = async {
                let server = TcpStream::connect(&upstream)
                    .await
                    .with_context(|| format!("connecting to {upstream}"))?;
                client.set_nodelay(true)?;
                server.set_nodelay(true)?;
                let (client_read, client_write) = client.into_split();
                let (server_read, server_write) = server.into_split();
                tokio::select! {
                    r = pump(client_read, server_write, Dir::Serverbound, Arc::clone(&session), Arc::clone(&names)) => r,
                    r = pump(server_read, client_write, Dir::Clientbound, session, names) => r,
                }
            }
            .await;
            match result {
                Ok(()) => println!("[{next_id}] closed"),
                Err(err) => println!("[{next_id}] closed: {err:#}"),
            }
        });
    }
}
