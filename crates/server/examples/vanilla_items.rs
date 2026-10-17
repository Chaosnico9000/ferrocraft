//! Checks item components and the enchanting table against a vanilla
//! server in offline mode whose console reads a file:
//!
//!   cargo run --release --example vanilla_items -- 127.0.0.1:25569 \
//!       <vanilla dir>/console.in <vanilla dir>/world
//!
//! It joins as `Probe`, has the console give items with components and
//! reads them as the client gets them (Ferrocraft's component shapes must
//! read vanilla's wire form), compares vanilla's saved player file with
//! what Ferrocraft would save for the same items, and opens an enchanting
//! table to compare vanilla's offers with Ferrocraft's for the player's
//! enchantment seed.

use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context;
use ferrocraft::data::GameData;
use ferrocraft::storage::{item_from_nbt, item_to_nbt, read_nbt_file};
use ferrocraft_devtools::client::Client;
use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::component;
use ferrocraft_protocol::packets::Packet;
use ferrocraft_protocol::packets::play::{
    AcceptTeleportation, ClientboundKeepAlive, ContainerClick, ContainerSetContent,
    ContainerSetData, ContainerSetSlot, ItemStack, OpenScreen, PlayerPosition,
    ServerboundKeepAlive, UseItemOn, container_input,
};

/// Commands that give items, and what each item should hold.
const GIVES: &[&str] = &[
    "give Probe diamond_sword[enchantments={sharpness:5,unbreaking:3},damage=10,custom_name=\"Hi\"]",
    "give Probe potion[potion_contents={potion:\"minecraft:strong_swiftness\",custom_color:255,custom_effects:[{id:\"minecraft:poison\",amplifier:1,duration:200,show_icon:false}]}]",
    "give Probe enchanted_book[stored_enchantments={mending:1,protection:4}]",
    "give Probe leather_helmet[dyed_color=16711680,repair_cost=3,rarity=epic,lore=[\"a\",\"b\"],unbreakable={},enchantment_glint_override=true]",
    "give Probe suspicious_stew[suspicious_stew_effects=[{id:\"minecraft:night_vision\",duration:100},{id:\"minecraft:weakness\"}]]",
    "give Probe ominous_bottle[ominous_bottle_amplifier=2]",
    "give Probe tipped_arrow[potion_contents={potion:\"minecraft:long_poison\"}]",
    "give Probe stick[custom_data={a:1,b:\"x\"},max_stack_size=16,item_model=\"minecraft:blaze_rod\"]",
];

struct Probe {
    client: Client,
    console: PathBuf,
    data: GameData,
}

impl Probe {
    fn console(&self, line: &str) -> anyhow::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.console)?;
        writeln!(file, "{line}")?;
        Ok(())
    }

    /// Reads packets for a while, answering keep-alives and teleports;
    /// `each` sees every packet.
    async fn pump(
        &mut self,
        millis: u64,
        mut each: impl FnMut(&ferrocraft_protocol::frame::RawPacket),
    ) {
        let end = Instant::now() + Duration::from_millis(millis);
        while Instant::now() < end {
            let left = end - Instant::now();
            let Ok(Some(raw)) = tokio::time::timeout(left, self.client.read_raw()).await else {
                continue;
            };
            if raw.id == ClientboundKeepAlive::ID {
                let id = ClientboundKeepAlive::from_raw(&raw).unwrap().id;
                self.client.send(&ServerboundKeepAlive { id }).await;
            } else if raw.id == PlayerPosition::ID {
                let p = PlayerPosition::from_raw(&raw).unwrap();
                self.client
                    .send(&AcceptTeleportation {
                        teleport_id: p.teleport_id,
                        pos: p.pos,
                        yaw: p.yaw,
                        pitch: p.pitch,
                    })
                    .await;
            }
            each(&raw);
        }
    }
}

/// The saved form of every component of a stack, as Ferrocraft reads the
/// wire form.
fn saved_components(data: &GameData, stack: &ItemStack) -> Vec<(String, anyhow::Result<Nbt>)> {
    stack
        .patch
        .added
        .iter()
        .map(|(kind, bytes)| {
            let name = component::type_name(*kind).unwrap_or("?").to_owned();
            let value = component::component(&name)
                .context("no shape")
                .and_then(|c| c.from_wire(bytes, data).map_err(anyhow::Error::from));
            (name, value)
        })
        .collect()
}

/// A tag with every compound's entries sorted, to compare without order.
fn sorted(nbt: &Nbt) -> Nbt {
    match nbt {
        Nbt::Compound(entries) => {
            let mut entries: Vec<(String, Nbt)> = entries
                .iter()
                .map(|(k, v)| (k.clone(), sorted(v)))
                .collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Nbt::Compound(entries)
        }
        Nbt::List(items) => Nbt::List(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [addr, console, world] = &args[..] else {
        anyhow::bail!("usage: vanilla_items <addr> <console.in> <world>");
    };
    let addr: SocketAddr = addr.parse()?;
    let data = GameData::load()?;
    let (mut client, finished) = Client::login(addr, "Probe").await;
    let uuid = finished.profile.uuid;
    client.configure(4).await;
    let mut probe = Probe {
        client,
        console: PathBuf::from(console),
        data,
    };
    let mut position = None;
    probe
        .pump(2000, |raw| {
            if raw.id == PlayerPosition::ID {
                position = PlayerPosition::from_raw(raw).ok().map(|p| p.pos);
            }
        })
        .await;
    let pos = position.context("no spawn position")?;
    // Vanilla ignores block clicks until the client says it has loaded.
    probe
        .client
        .send(&ferrocraft_protocol::packets::play::PlayerLoaded {})
        .await;
    println!("joined at {pos:?} as {uuid}");
    probe.console("clear Probe")?;
    probe.console("experience set Probe 30 levels")?;

    // 1. Items with components, as the client gets them.
    let mut problems = 0;
    for give in GIVES {
        probe.console(give)?;
        let mut stacks = Vec::new();
        probe
            .pump(700, |raw| {
                if raw.id == ContainerSetSlot::ID {
                    match ContainerSetSlot::from_raw(raw) {
                        Ok(slot) => stacks.extend(slot.stack.0),
                        Err(err) => {
                            eprintln!("undecodable slot: {err}");
                            stacks.push(ItemStack {
                                item: -1,
                                count: 0,
                                patch: Default::default(),
                            });
                        }
                    }
                }
            })
            .await;
        println!("\n/{give}");
        for stack in &stacks {
            if stack.item < 0 {
                println!("  FAILED to read the slot");
                problems += 1;
                continue;
            }
            for (name, value) in saved_components(&probe.data, stack) {
                match value {
                    Ok(value) => println!("  {name} = {value:?}"),
                    Err(err) => {
                        println!("  {name}: {err}");
                        problems += 1;
                    }
                }
            }
        }
        if stacks.is_empty() {
            println!("  nothing arrived");
            problems += 1;
        }
    }

    // 2. Vanilla's saved inventory against Ferrocraft's form of it.
    probe.console("save-all flush")?;
    probe.pump(3000, |_| {}).await;
    let file = Path::new(world)
        .join("players/data")
        .join(format!("{uuid}.dat"));
    let saved = read_nbt_file(&file)?.context("no player file")?;
    let seed = match saved.get("XpSeed") {
        Some(Nbt::Int(seed)) => *seed,
        _ => anyhow::bail!("no XpSeed"),
    };
    println!("\nsaved inventory (XpSeed {seed}):");
    if let Some(Nbt::List(items)) = saved.get("Inventory") {
        for item in items {
            let Some(stack) = item_from_nbt(item, &probe.data) else {
                println!("  cannot read {item:?}");
                problems += 1;
                continue;
            };
            let ours = item_to_nbt(&stack, &probe.data).context("item")?;
            let theirs = item.get("components").cloned();
            let mine = ours.get("components").cloned();
            // Compounds compare without their order.
            let same = theirs.as_ref().map(sorted) == mine.as_ref().map(sorted);
            let id = item.get("id");
            if same && ours.get("ferrocraft:components").is_none() {
                println!("  {id:?}: same");
            } else {
                println!("  {id:?}: DIFFERENT\n    vanilla    {theirs:?}\n    ferrocraft {mine:?}");
                problems += 1;
            }
        }
    }

    // 3. The enchanting table: fifteen bookshelves around it, a sword.
    let (x, y, z) = (
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
    );
    let table = BlockPos { x, y, z: z + 4 };
    probe.console("clear Probe")?;
    probe.console(&format!(
        "fill {} {} {} {} {} {} bookshelf",
        x - 2,
        y,
        z + 2,
        x + 2,
        y + 1,
        z + 6
    ))?;
    probe.console(&format!(
        "fill {} {} {} {} {} {} air",
        x - 1,
        y,
        z + 3,
        x + 1,
        y + 1,
        z + 5
    ))?;
    probe.console(&format!("setblock {} {} {} enchanting_table", x, y, z + 4))?;
    // `TABLE_ITEM=book` checks books, which lose one enchantment at random.
    let item = std::env::var("TABLE_ITEM").unwrap_or_else(|_| "diamond_sword".into());
    probe.console(&format!("give Probe {item}"))?;
    probe.pump(1500, |_| {}).await;
    probe
        .client
        .send(&UseItemOn {
            hand: 0,
            pos: table,
            face: 1,
            cursor: [0.5, 1.0, 0.5],
            inside: false,
            world_border_hit: false,
            sequence: 1,
        })
        .await;
    let mut screen = None;
    probe
        .pump(2000, |raw| {
            if std::env::var("TRACE").is_ok() {
                eprintln!("packet {:#x}", raw.id);
            }
            if raw.id == OpenScreen::ID {
                screen = OpenScreen::from_raw(raw).ok().map(|s| s.container_id);
            }
        })
        .await;
    let screen = screen.context("the table did not open")?;
    // The sword is in the first hotbar slot, 29 on this screen.
    probe
        .client
        .send(&ContainerClick {
            container_id: screen,
            state_id: 0,
            slot: 29,
            button: 0,
            input: container_input::QUICK_MOVE,
            changed: Vec::new(),
            carried: None,
        })
        .await;
    let mut values = [0i16; 10];
    probe
        .pump(1500, |raw| {
            if std::env::var("TRACE").is_ok() {
                eprintln!("after click {:#x} {}", raw.id, raw.body.len());
            }
            if raw.id == ContainerSetData::ID
                && let Ok(d) = ContainerSetData::from_raw(raw)
                && let Some(v) = values.get_mut(d.id as usize)
            {
                *v = d.value;
            }
            if raw.id == ContainerSetContent::ID
                && std::env::var("TRACE").is_ok()
                && let Ok(c) = ContainerSetContent::from_raw(raw)
            {
                let filled: Vec<(usize, i32)> = c
                    .items
                    .0
                    .iter()
                    .enumerate()
                    .filter_map(|(i, s)| Some((i, s.0.as_ref()?.item)))
                    .collect();
                eprintln!("content {} {filled:?}", c.container_id);
            }
            if raw.id == ContainerSetSlot::ID
                && std::env::var("TRACE").is_ok()
                && let Ok(c) = ContainerSetSlot::from_raw(raw)
            {
                eprintln!(
                    "slot {} {} {:?}",
                    c.container_id,
                    c.slot,
                    c.stack.0.map(|s| s.item)
                );
            }
        })
        .await;
    let sword = ferrocraft::stack::plain(
        probe
            .data
            .item_id(&format!("minecraft:{item}"))
            .context("TABLE_ITEM")?,
        1,
    );
    let offers = ferrocraft::enchanting_offers(&probe.data, seed, &sword, 15);
    println!("\nenchanting table, vanilla: {values:?}");
    println!(
        "ferrocraft: costs {:?} clues {:?} levels {:?}",
        offers.0, offers.1, offers.2
    );
    let vanilla_costs = [values[0], values[1], values[2]].map(i32::from);
    let vanilla_clues = [values[4], values[5], values[6]].map(i32::from);
    let vanilla_levels = [values[7], values[8], values[9]].map(i32::from);
    if (vanilla_costs, vanilla_clues, vanilla_levels) != offers {
        println!("  DIFFERENT");
        problems += 1;
    } else {
        println!("  same");
    }
    println!("\n{problems} problems");
    Ok(())
}
