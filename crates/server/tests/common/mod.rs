//! Starts a real server on a random port for the test client in
//! `ferrocraft_devtools::client`, and reads the join sequence.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::Arc;

use ferrocraft::{Config, Server, Timing};
pub use ferrocraft_devtools::client::Client;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::Packet;
use ferrocraft_protocol::packets::play::{
    AcceptTeleportation, ChunkBatchFinished, ChunkBatchStart, Commands, ComponentPatch,
    ContainerSetContent, EntityEvent, GameEvent, GameMode, ItemStack, LevelChunkWithLight, Login,
    MovePlayer, PlayerAbilities, PlayerInfoUpdate, PlayerPosition, SetCarriedItem,
    SetChunkCacheCenter, SetCreativeModeSlot, SetDefaultSpawnPosition, SetEntityData, SetHeldSlot,
    UntrustedSlot, UseItemOn,
};
use tokio::net::TcpListener;

pub async fn start(config: Config) -> SocketAddr {
    start_with_timing(config, Timing::default()).await
}

pub async fn start_with_timing(config: Config, timing: Timing) -> SocketAddr {
    start_server(config, timing).await.1
}

/// Starts a server and keeps a handle to it, e.g. to shut it down.
pub async fn start_server(config: Config, timing: Timing) -> (Arc<Server>, SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = Server::with_timing(config, timing).unwrap();
    tokio::spawn(Arc::clone(&server).serve(listener));
    (server, addr)
}

/// A server that keeps its world in memory.
pub fn test_config() -> Config {
    Config {
        motd: "Testwelt".into(),
        world: String::new(),
        online_mode: false,
        game_mode: "creative".into(),
        // Tests that want mobs summon them themselves.
        animals: false,
        monsters: false,
        difficulty: "normal".into(),
        ..Config::default()
    }
}

/// Vanilla's `ChunkTrackingView.contains`, which decides what is sent.
pub fn in_view(center: (i32, i32), r: i32, (x, z): (i32, i32)) -> bool {
    let near = |d: i32| i64::from((d.abs() - 2).max(0));
    near(x - center.0).pow(2) + near(z - center.1).pow(2) < i64::from(r * r)
}

/// Chunks vanilla sends for view distance `r`.
pub fn chunks_in_view(r: i32) -> usize {
    (-r - 2..=r + 2)
        .flat_map(|x| (-r - 2..=r + 2).map(move |z| (x, z)))
        .filter(|&chunk| in_view((0, 0), r, chunk))
        .count()
}

/// Reads the join sequence up to the end of the first chunk batch.
pub async fn enter_world(client: &mut Client) -> (Login, PlayerPosition, Vec<LevelChunkWithLight>) {
    let login: Login = client.read().await;
    client.read::<PlayerAbilities>().await;
    client.read::<EntityEvent>().await;
    let commands = client.read_raw().await.unwrap();
    assert_eq!(commands.id, Commands::ID);
    let info: PlayerInfoUpdate = client.read().await;
    assert_eq!(info.entries[0].game_mode, GameMode::Creative);
    let own: SetEntityData = client.read().await;
    assert_eq!(own.entity_id, login.entity_id, "the own skin layers");
    client.read::<SetDefaultSpawnPosition>().await;
    let position: PlayerPosition = client.read().await;
    let inventory: ContainerSetContent = client.read().await;
    assert_eq!(inventory.items.0.len(), 46, "the whole inventory menu");
    client.read::<SetHeldSlot>().await;
    let event: GameEvent = client.read().await;
    assert_eq!(event.event, 13, "level chunks load start");
    assert_eq!(
        client.read::<SetChunkCacheCenter>().await,
        SetChunkCacheCenter { x: 0, z: 0 }
    );
    client.read::<ChunkBatchStart>().await;
    let mut chunks = Vec::new();
    let finished = loop {
        let raw = client
            .read_raw()
            .await
            .expect("closed while sending chunks");
        match raw.id {
            LevelChunkWithLight::ID => chunks.push(LevelChunkWithLight::from_raw(&raw).unwrap()),
            ChunkBatchFinished::ID => break ChunkBatchFinished::from_raw(&raw).unwrap(),
            id => panic!("unexpected packet 0x{id:02x} in a chunk batch"),
        }
    };
    assert_eq!(finished.batch_size as usize, chunks.len());
    (login, position, chunks)
}

pub async fn accept_spawn(client: &mut Client, position: &PlayerPosition) {
    client
        .send(&AcceptTeleportation {
            teleport_id: position.teleport_id,
            pos: position.pos,
            yaw: position.yaw,
            pitch: position.pitch,
        })
        .await;
}

/// A player that has joined, accepted the spawn teleport and received its
/// first chunks.
pub async fn join(addr: SocketAddr, name: &str, view: i8) -> (Client, Login) {
    let (mut client, _) = Client::login(addr, name).await;
    client.configure(view).await;
    let (login, position, _) = enter_world(&mut client).await;
    accept_spawn(&mut client, &position).await;
    (client, login)
}

/// Sends a movement packet.
pub async fn walk(client: &mut Client, pos: [f64; 3], rot: Option<[f32; 2]>) {
    let mut body = Vec::new();
    let id = MovePlayer {
        pos: Some(pos),
        rot,
        flags: 1,
    }
    .encode_variant(&mut body);
    client.send_raw(id, &body).await;
}

/// Reads until a packet of type `P` matches `wanted`.
pub async fn read_matching<P: Packet>(client: &mut Client, wanted: impl Fn(&P) -> bool) -> P {
    loop {
        let packet: P = client.read_until().await;
        if wanted(&packet) {
            return packet;
        }
    }
}

/// Puts a stack of `item` into the first hotbar slot and selects it.
pub async fn hold(client: &mut Client, item: i32) {
    hold_some(client, item, 64).await;
}

pub async fn hold_some(client: &mut Client, item: i32, count: i32) {
    client.send(&SetCarriedItem { slot: 0 }).await;
    client
        .send(&SetCreativeModeSlot {
            slot: 36,
            stack: UntrustedSlot(Some(ItemStack {
                item,
                count,
                patch: ComponentPatch::default(),
            })),
        })
        .await;
}

pub fn click(pos: BlockPos, face: i32, sequence: i32) -> UseItemOn {
    UseItemOn {
        hand: 0,
        pos,
        face,
        cursor: [0.5, 1.0, 0.5],
        inside: false,
        world_border_hit: false,
        sequence,
    }
}
