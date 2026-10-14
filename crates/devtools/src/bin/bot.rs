//! Bots: players that join a server and keep busy around spawn, so that
//! there is something to see and to capture. Offline mode only; building
//! and taking items needs creative mode.
//!
//!   bot <addr> [--count N] [--name Bot] [--radius 3] [--seconds S]
//!       [--roles walk,build,house,throw]
//!
//! With `--count` above 1 the bots are called `<name>1`, `<name>2`, ... and
//! take the roles in turn:
//!
//! - `walk`: circles the spawn, jumps, sneaks, swings and changes what it
//!   holds.
//! - `build`: builds a tower of changing blocks with a torch inside and tears
//!   it down again.
//! - `house`: sets up a chest, a door and a lever and keeps using them.
//! - `throw`: walks back and forth throwing items and picks them up again.
//!
//! Everyone says something in the chat now and then.

use std::collections::HashMap;
use std::f64::consts::{PI, TAU};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use ferrocraft_devtools::client::Client;
use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::Packet;
use ferrocraft_protocol::packets::play::{
    AcceptTeleportation, Chat, ChunkBatchFinished, ChunkBatchReceived, ClientTickEnd,
    ClientboundKeepAlive, ComponentPatch, Disconnect, ItemStack, MovePlayer, OpenScreen,
    PlayerAction, PlayerInput, PlayerLoaded, PlayerPosition, Punch, ServerboundContainerClose,
    ServerboundKeepAlive, SetCarriedItem, SetCreativeModeSlot, UntrustedSlot, UseItemOn,
    player_action,
};
use rand::rngs::StdRng;
use rand::seq::IndexedRandom;
use rand::{Rng, SeedableRng};

/// Blocks per tick, about vanilla's walking speed.
const SPEED: f64 = 0.2;
/// The hotbar slot every bot holds things in.
const HAND: i16 = 36;
/// How far from spawn the bots that do things set up.
const SITE_DISTANCE: f64 = 9.0;

/// Item ids by name, from the same data the server embeds.
static ITEMS: LazyLock<HashMap<String, i32>> = LazyLock::new(|| {
    let items: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../../../../data/26.3/items.json")).expect("items.json");
    items
        .iter()
        .zip(0..)
        .filter_map(|(item, id)| Some((item.get("name")?.as_str()?.to_owned(), id)))
        .collect()
});

fn item(name: &str) -> i32 {
    ITEMS[&format!("minecraft:{name}")]
}

const SHOWCASE: &[&str] = &[
    "diamond_sword",
    "golden_apple",
    "torch",
    "bow",
    "shield",
    "fishing_rod",
    "iron_pickaxe",
    "cake",
    "compass",
    "red_tulip",
];
const MATERIALS: &[&str] = &[
    "stone_bricks",
    "oak_planks",
    "glass",
    "bricks",
    "white_wool",
    "cherry_planks",
    "copper_block",
    "quartz_block",
];
const THROWN: &[&str] = &[
    "apple",
    "diamond",
    "emerald",
    "gold_ingot",
    "egg",
    "snowball",
];
const CHAT: &[&str] = &[
    "Moin!",
    "Schönes Wetter heute.",
    "Wer hat meine Kekse gesehen?",
    "Ferrocraft läuft rund.",
    "Ich mach mal Pause.",
    "Hat jemand Diamanten?",
    "Gleich wird es dunkel.",
    "Wer baut mit?",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Walk,
    Build,
    House,
    Throw,
}

impl Role {
    fn parse(name: &str) -> anyhow::Result<Self> {
        Ok(match name {
            "walk" => Self::Walk,
            "build" => Self::Build,
            "house" => Self::House,
            "throw" => Self::Throw,
            _ => bail!("unknown role {name}"),
        })
    }
}

struct Options {
    addr: SocketAddr,
    count: usize,
    name: String,
    radius: f64,
    seconds: Option<f64>,
    roles: Vec<Role>,
}

fn parse() -> anyhow::Result<Options> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: bot <addr> [--count N] [--name Bot] [--radius 3] [--seconds S] \
                 [--roles walk,build,house,throw]";
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
        roles: vec![Role::Walk, Role::Build, Role::House, Role::Throw],
    };
    while let Some(arg) = args.next() {
        let mut value = || args.next().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--count" => options.count = value()?.parse()?,
            "--name" => options.name = value()?,
            "--radius" => options.radius = value()?.parse()?,
            "--seconds" => options.seconds = Some(value()?.parse()?),
            "--roles" => {
                options.roles = value()?
                    .split(',')
                    .map(Role::parse)
                    .collect::<anyhow::Result<_>>()?;
                if options.roles.is_empty() {
                    bail!("--roles needs at least one role");
                }
            }
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

fn block_at(pos: [f64; 3]) -> BlockPos {
    BlockPos {
        x: pos[0].floor() as i32,
        y: pos[1].floor() as i32,
        z: pos[2].floor() as i32,
    }
}

fn center(pos: BlockPos) -> [f64; 3] {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ]
}

/// Yaw and pitch from `eyes` towards `target`, as vanilla counts them.
fn look_at(eyes: [f64; 3], target: [f64; 3]) -> [f32; 2] {
    let [dx, dy, dz] = [
        target[0] - eyes[0],
        target[1] - eyes[1],
        target[2] - eyes[2],
    ];
    let yaw = (-dx).atan2(dz).to_degrees();
    let pitch = -dy.atan2((dx * dx + dz * dz).sqrt()).to_degrees();
    [yaw as f32, pitch as f32]
}

/// What a builder or house keeper does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Clears the site of what an earlier run may have left.
    Clear(usize),
    Place(usize),
    Wait(u32),
    Tear(usize),
    /// The house keeper uses its things in turn.
    Use(usize),
}

struct Bot {
    client: Client,
    name: String,
    role: Role,
    rng: StdRng,
    /// Where the server put us; the walkers circle it.
    spawn: [f64; 3],
    /// Where builders, house keepers and throwers work.
    site: [f64; 3],
    pos: [f64; 3],
    rot: [f32; 2],
    on_ground: bool,
    angle: f64,
    radius: f64,
    tick: u64,
    sequence: i32,
    step: Step,
    /// Ticks until the next chat line.
    chat_in: u64,
    material: usize,
    /// An open screen to close again.
    screen: Option<(i32, u64)>,
    sneaking: bool,
    thrown: u32,
}

impl Bot {
    fn eyes(&self) -> [f64; 3] {
        [self.pos[0], self.pos[1] + 1.62, self.pos[2]]
    }

    fn ground(&self) -> i32 {
        self.spawn[1].floor() as i32
    }

    async fn hold(&mut self, name: &str, count: i32) {
        self.client.send(&SetCarriedItem { slot: 0 }).await;
        self.client
            .send(&SetCreativeModeSlot {
                slot: HAND,
                stack: UntrustedSlot(Some(ItemStack {
                    item: item(name),
                    count,
                    patch: ComponentPatch::default(),
                })),
            })
            .await;
    }

    async fn swing(&mut self) {
        self.client.send(&Punch).await;
    }

    /// A right click on top of `below`.
    async fn place_on(&mut self, below: BlockPos) {
        self.sequence += 1;
        self.client
            .send(&UseItemOn {
                hand: 0,
                pos: below,
                face: 1,
                cursor: [0.5, 1.0, 0.5],
                inside: false,
                world_border_hit: false,
                sequence: self.sequence,
            })
            .await;
        self.swing().await;
    }

    async fn use_block(&mut self, pos: BlockPos) {
        self.sequence += 1;
        self.client
            .send(&UseItemOn {
                hand: 0,
                pos,
                face: 2,
                cursor: [0.5, 0.5, 0.0],
                inside: false,
                world_border_hit: false,
                sequence: self.sequence,
            })
            .await;
        self.swing().await;
    }

    async fn break_block(&mut self, pos: BlockPos) {
        self.sequence += 1;
        self.client
            .send(&PlayerAction {
                action: player_action::START_DESTROY_BLOCK,
                pos,
                face: 1,
                sequence: self.sequence,
            })
            .await;
        self.swing().await;
    }

    async fn sneak(&mut self, sneaking: bool) {
        if self.sneaking != sneaking {
            self.sneaking = sneaking;
            self.client
                .send(&PlayerInput {
                    flags: if sneaking { 0x20 } else { 0 },
                })
                .await;
        }
    }

    async fn chat(&mut self) {
        let line = (*CHAT.choose(&mut self.rng).expect("lines")).to_owned();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        self.client
            .send(&Chat {
                message: line,
                timestamp,
                salt: self.rng.random(),
                signature: None,
                last_seen_offset: 0,
                acknowledged: [0; 3],
                // `LastSeenMessages.Update.IGNORE_CHECKSUM`: nothing seen to check.
                checksum: 0,
            })
            .await;
    }

    async fn send_movement(&mut self) {
        let mut body = Vec::new();
        let id = MovePlayer {
            pos: Some(self.pos),
            rot: Some(self.rot),
            flags: u8::from(self.on_ground),
        }
        .encode_variant(&mut body);
        self.client.send_raw(id, &body).await;
    }

    /// The blocks of a builder's tower: a ring of eight around the site,
    /// four high, the torch first.
    fn tower(&self) -> Vec<BlockPos> {
        let site = block_at(self.site);
        let ground = self.ground();
        let mut blocks = Vec::new();
        for y in 0..4 {
            for (dx, dz) in [
                (-1, -1),
                (0, -1),
                (1, -1),
                (1, 0),
                (1, 1),
                (0, 1),
                (-1, 1),
                (-1, 0),
            ] {
                blocks.push(BlockPos {
                    x: site.x + dx,
                    y: ground + y,
                    z: site.z + dz,
                });
            }
        }
        blocks
    }

    /// The house keeper's things: chest, door, lever in a row.
    fn things(&self) -> [(BlockPos, &'static str); 3] {
        let site = block_at(self.site);
        let ground = self.ground();
        [
            (
                BlockPos {
                    x: site.x - 2,
                    y: ground,
                    z: site.z,
                },
                "chest",
            ),
            (
                BlockPos {
                    x: site.x,
                    y: ground,
                    z: site.z,
                },
                "oak_door",
            ),
            (
                BlockPos {
                    x: site.x + 2,
                    y: ground,
                    z: site.z,
                },
                "lever",
            ),
        ]
    }

    async fn act(&mut self) {
        self.tick += 1;
        if self.chat_in == 0 {
            self.chat().await;
            self.chat_in = self.rng.random_range(900..2400);
        }
        self.chat_in -= 1;
        match self.role {
            Role::Walk => self.walk().await,
            Role::Build => self.build().await,
            Role::House => self.keep_house().await,
            Role::Throw => self.throw().await,
        }
        self.send_movement().await;
        self.client.send(&ClientTickEnd).await;
    }

    async fn walk(&mut self) {
        let phase = self.tick % 200;
        self.sneak((120..160).contains(&phase)).await;
        let speed = if self.sneaking { SPEED * 0.3 } else { SPEED };
        self.angle = (self.angle + speed / self.radius) % TAU;
        let [cx, y, cz] = self.spawn;
        // A jump every five seconds: up and down in ten ticks.
        let jump = self.tick % 100;
        let height = if jump < 10 {
            1.25 * (PI * jump as f64 / 10.0).sin()
        } else {
            0.0
        };
        self.on_ground = jump >= 10;
        self.pos = [
            cx + self.radius * self.angle.cos(),
            y + height,
            cz + self.radius * self.angle.sin(),
        ];
        // Facing the way it walks, along the circle.
        self.rot = [self.angle.to_degrees() as f32, 0.0];
        if self.tick.is_multiple_of(37) {
            self.swing().await;
        }
        if self.tick % 240 == 1 {
            let shown = *SHOWCASE.choose(&mut self.rng).expect("items");
            self.hold(shown, 1).await;
        }
    }

    /// Strolls around the site at `distance`, slowly.
    fn stroll(&mut self, distance: f64) {
        self.angle = (self.angle + 0.04 / distance) % TAU;
        self.pos = [
            self.site[0] + distance * self.angle.cos(),
            self.spawn[1],
            self.site[2] + distance * self.angle.sin(),
        ];
        self.on_ground = true;
    }

    async fn build(&mut self) {
        self.stroll(3.5);
        let tower = self.tower();
        let site = block_at(self.site);
        let torch = BlockPos {
            y: self.ground(),
            ..site
        };
        self.rot = look_at(self.eyes(), center(site));
        match self.step {
            Step::Clear(i) => {
                if i < tower.len() {
                    self.break_block(tower[i]).await;
                    self.step = Step::Clear(i + 1);
                } else {
                    self.break_block(torch).await;
                    self.hold("torch", 1).await;
                    self.place_on(BlockPos {
                        y: torch.y - 1,
                        ..torch
                    })
                    .await;
                    let material = MATERIALS[self.material % MATERIALS.len()];
                    self.hold(material, 64).await;
                    self.step = Step::Place(0);
                }
            }
            Step::Place(i) if self.tick.is_multiple_of(4) => {
                let Some(&pos) = tower.get(i) else {
                    self.step = Step::Wait(100);
                    return;
                };
                self.rot = look_at(self.eyes(), center(pos));
                self.place_on(BlockPos {
                    y: pos.y - 1,
                    ..pos
                })
                .await;
                self.step = Step::Place(i + 1);
            }
            Step::Wait(0) => self.step = Step::Tear(tower.len()),
            Step::Wait(n) => self.step = Step::Wait(n - 1),
            Step::Tear(i) if self.tick.is_multiple_of(3) => {
                if i == 0 {
                    self.material += 1;
                    let material = MATERIALS[self.material % MATERIALS.len()];
                    self.hold(material, 64).await;
                    self.step = Step::Place(0);
                    return;
                }
                let pos = tower[i - 1];
                self.rot = look_at(self.eyes(), center(pos));
                self.break_block(pos).await;
                self.step = Step::Tear(i - 1);
            }
            _ => {}
        }
    }

    async fn keep_house(&mut self) {
        // In front of the row, walking along it.
        let sway = (self.tick as f64 / 80.0).sin() * 1.5;
        self.pos = [self.site[0] + 0.5 + sway, self.spawn[1], self.site[2] - 2.0];
        self.on_ground = true;
        let things = self.things();
        if let Some((id, opened)) = self.screen
            && self.tick >= opened + 50
        {
            self.client
                .send(&ServerboundContainerClose { container_id: id })
                .await;
            self.screen = None;
        }
        match self.step {
            Step::Clear(i) if self.tick.is_multiple_of(2) => {
                if let Some(&(pos, _)) = things.get(i) {
                    self.rot = look_at(self.eyes(), center(pos));
                    self.break_block(pos).await;
                    self.break_block(BlockPos {
                        y: pos.y + 1,
                        ..pos
                    })
                    .await;
                    self.step = Step::Clear(i + 1);
                } else {
                    self.step = Step::Place(0);
                }
            }
            Step::Place(i) if self.tick.is_multiple_of(5) => {
                if let Some(&(pos, name)) = things.get(i) {
                    self.rot = look_at(self.eyes(), center(pos));
                    self.hold(name, 1).await;
                    self.place_on(BlockPos {
                        y: pos.y - 1,
                        ..pos
                    })
                    .await;
                    self.step = Step::Place(i + 1);
                } else {
                    self.hold("cookie", 16).await;
                    self.step = Step::Use(0);
                }
            }
            Step::Use(i) if self.tick.is_multiple_of(60) && self.screen.is_none() => {
                let (pos, _) = things[i % things.len()];
                self.rot = look_at(self.eyes(), center(pos));
                self.use_block(pos).await;
                self.step = Step::Use(i + 1);
            }
            _ => {}
        }
    }

    async fn throw(&mut self) {
        // Back and forth along eight blocks, throwing forward on the way out
        // and walking through the items on the way back.
        let period = 8.0 / (SPEED * 0.5);
        let t = (self.tick as f64 % (2.0 * period)) / period;
        let outward = t < 1.0;
        let along = if outward { t } else { 2.0 - t } * 8.0;
        self.pos = [self.site[0], self.spawn[1], self.site[2] - 4.0 + along];
        self.on_ground = true;
        self.rot = [if outward { 0.0 } else { 180.0 }, 10.0];
        if self.tick % 400 == 1 || self.thrown >= 48 {
            let thrown = *THROWN.choose(&mut self.rng).expect("items");
            let count = if thrown == "egg" || thrown == "snowball" {
                16
            } else {
                64
            };
            self.hold(thrown, count).await;
            self.thrown = 0;
        }
        if outward && self.tick.is_multiple_of(25) {
            self.client
                .send(&PlayerAction {
                    action: player_action::DROP_ITEM,
                    pos: BlockPos { x: 0, y: 0, z: 0 },
                    face: 0,
                    sequence: 0,
                })
                .await;
            self.swing().await;
            self.thrown += 1;
        }
    }
}

async fn run(
    addr: SocketAddr,
    name: String,
    role: Role,
    radius: f64,
    phase: f64,
    until: Option<Instant>,
) {
    let (mut client, finished) = Client::login(addr, &name).await;
    println!("{name}: logged in as {} ({role:?})", finished.profile.uuid);
    client.configure(4).await;
    // Keep-alives come every 15 s; anything longer means the server is gone.
    client.timeout = Duration::from_secs(40);

    let mut rng = StdRng::seed_from_u64(rand::random());
    let mut bot = Bot {
        client,
        name,
        role,
        chat_in: rng.random_range(200..1200),
        rng,
        spawn: [0.0; 3],
        site: [0.0; 3],
        pos: [0.0; 3],
        rot: [0.0; 2],
        on_ground: true,
        angle: phase,
        radius,
        tick: 0,
        sequence: 0,
        step: Step::Clear(0),
        material: 0,
        screen: None,
        sneaking: false,
        thrown: 0,
    };
    let mut placed = false;
    let mut ticks = tokio::time::interval(Duration::from_millis(50));
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if until.is_some_and(|t| Instant::now() >= t) {
            println!("{}: leaving", bot.name);
            return;
        }
        tokio::select! {
            raw = bot.client.read_raw() => {
                let Some(raw) = raw else {
                    println!("{}: connection closed", bot.name);
                    return;
                };
                match raw.id {
                    ClientboundKeepAlive::ID => {
                        let id = ClientboundKeepAlive::from_raw(&raw).unwrap().id;
                        bot.client.send(&ServerboundKeepAlive { id }).await;
                    }
                    PlayerPosition::ID => {
                        let to = PlayerPosition::from_raw(&raw).unwrap();
                        bot.client.send(&AcceptTeleportation {
                            teleport_id: to.teleport_id,
                            pos: to.pos,
                            yaw: to.yaw,
                            pitch: to.pitch,
                        }).await;
                        if !placed {
                            bot.client.send(&PlayerLoaded).await;
                            placed = true;
                            // The first teleport is the spawn: walkers circle
                            // it, the others work further out, spread by phase.
                            let [x, y, z] = to.pos;
                            bot.spawn = [x.floor() + 0.5, y, z.floor() + 0.5];
                            bot.site = [
                                (x + SITE_DISTANCE * phase.cos()).floor() + 0.5,
                                y,
                                (z + SITE_DISTANCE * phase.sin()).floor() + 0.5,
                            ];
                        }
                        bot.pos = to.pos;
                    }
                    OpenScreen::ID => {
                        let screen = OpenScreen::from_raw(&raw).unwrap();
                        bot.screen = Some((screen.container_id, bot.tick));
                    }
                    ChunkBatchFinished::ID => {
                        bot.client.send(&ChunkBatchReceived { desired_chunks_per_tick: 25.0 }).await;
                    }
                    Disconnect::ID => {
                        let bye = Disconnect::from_raw(&raw).unwrap();
                        println!("{}: kicked: {}", bot.name, text(&bye.reason));
                        return;
                    }
                    _ => {}
                }
            }
            _ = ticks.tick() => {
                if placed {
                    bot.act().await;
                }
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
        let role = options.roles[i % options.roles.len()];
        let phase = TAU * i as f64 / options.count as f64;
        bots.spawn(run(options.addr, name, role, options.radius, phase, until));
        // Vanilla throttles logins from one address.
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    while let Some(result) = bots.join_next().await {
        result?;
    }
    Ok(())
}
