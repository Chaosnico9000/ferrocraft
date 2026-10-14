//! Bots: players that join a server and walk in circles around spawn, so that
//! there is someone to see and something to capture. Offline mode only.
//!
//!   bot <addr> [--count N] [--name Bot] [--radius 3] [--seconds S]
//!
//! With `--count` above 1 the bots are called `<name>1`, `<name>2`, ... and
//! walk the same circle, spread out. Without `--seconds` they stay until
//! killed or kicked.

use std::f64::consts::TAU;
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use ferrocraft_devtools::client::Client;
use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::packets::Packet;
use ferrocraft_protocol::packets::play::{
    AcceptTeleportation, ChunkBatchFinished, ChunkBatchReceived, ClientTickEnd,
    ClientboundKeepAlive, Disconnect, MovePlayer, PlayerLoaded, PlayerPosition,
    ServerboundKeepAlive,
};

/// Blocks per tick, about vanilla's walking speed.
const SPEED: f64 = 0.2;

struct Options {
    addr: SocketAddr,
    count: usize,
    name: String,
    radius: f64,
    seconds: Option<f64>,
}

fn parse() -> anyhow::Result<Options> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: bot <addr> [--count N] [--name Bot] [--radius 3] [--seconds S]";
    let addr = args.next().context(usage)?;
    let mut options = Options {
        addr: addr
            .to_socket_addrs()?
            .next()
            .with_context(|| format!("resolving {addr}"))?,
        count: 1,
        name: "Bot".into(),
        radius: 3.0,
        seconds: None,
    };
    while let Some(arg) = args.next() {
        let mut value = || args.next().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--count" => options.count = value()?.parse()?,
            "--name" => options.name = value()?,
            "--radius" => options.radius = value()?.parse()?,
            "--seconds" => options.seconds = Some(value()?.parse()?),
            _ => bail!("unknown argument {arg}\n{usage}"),
        }
    }
    Ok(options)
}

fn text(nbt: &Nbt) -> String {
    match nbt.get("text") {
        Some(Nbt::String(text)) => text.clone(),
        _ => format!("{nbt:?}"),
    }
}

async fn run(addr: SocketAddr, name: String, radius: f64, phase: f64, until: Option<Instant>) {
    let (mut client, finished) = Client::login(addr, &name).await;
    println!("{name}: logged in as {}", finished.profile.uuid);
    client.configure(4).await;
    // Keep-alives come every 15 s; anything longer means the server is gone.
    client.timeout = Duration::from_secs(40);

    let mut center = None;
    let mut angle = phase;
    let mut ticks = tokio::time::interval(Duration::from_millis(50));
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if until.is_some_and(|t| Instant::now() >= t) {
            println!("{name}: leaving");
            return;
        }
        tokio::select! {
            raw = client.read_raw() => {
                let Some(raw) = raw else {
                    println!("{name}: connection closed");
                    return;
                };
                match raw.id {
                    ClientboundKeepAlive::ID => {
                        let id = ClientboundKeepAlive::from_raw(&raw).unwrap().id;
                        client.send(&ServerboundKeepAlive { id }).await;
                    }
                    PlayerPosition::ID => {
                        let to = PlayerPosition::from_raw(&raw).unwrap();
                        client.send(&AcceptTeleportation {
                            teleport_id: to.teleport_id,
                            pos: to.pos,
                            yaw: to.yaw,
                            pitch: to.pitch,
                        }).await;
                        if center.is_none() {
                            client.send(&PlayerLoaded).await;
                        }
                        // Circle around wherever the server puts us.
                        let [x, y, z] = to.pos;
                        center = Some([x - radius * angle.cos(), y, z - radius * angle.sin()]);
                    }
                    ChunkBatchFinished::ID => {
                        client.send(&ChunkBatchReceived { desired_chunks_per_tick: 25.0 }).await;
                    }
                    Disconnect::ID => {
                        let bye = Disconnect::from_raw(&raw).unwrap();
                        println!("{name}: kicked: {}", text(&bye.reason));
                        return;
                    }
                    _ => {}
                }
            }
            _ = ticks.tick() => {
                let Some([cx, y, cz]) = center else { continue };
                angle = (angle + SPEED / radius) % TAU;
                let movement = MovePlayer {
                    pos: Some([cx + radius * angle.cos(), y, cz + radius * angle.sin()]),
                    rot: Some([angle.to_degrees() as f32, 0.0]),
                    flags: 1,
                };
                let mut body = Vec::new();
                let id = movement.encode_variant(&mut body);
                client.send_raw(id, &body).await;
                client.send(&ClientTickEnd).await;
            }
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let options = parse()?;
    let until = options
        .seconds
        .map(|s| Instant::now() + Duration::from_secs_f64(s));
    let mut bots = tokio::task::JoinSet::new();
    for i in 0..options.count {
        let name = if options.count == 1 {
            options.name.clone()
        } else {
            format!("{}{}", options.name, i + 1)
        };
        let phase = TAU * i as f64 / options.count as f64;
        bots.spawn(run(options.addr, name, options.radius, phase, until));
        // Vanilla throttles logins from one address.
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    while let Some(result) = bots.join_next().await {
        result?;
    }
    Ok(())
}
