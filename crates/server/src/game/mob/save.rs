//! Entities on disk and in chunks no player is near: vanilla's entity
//! storage (`EntityStorage`, `dimensions/<dim>/entities/r.<x>.<z>.mca`, one
//! `{DataVersion, Position, Entities}` per chunk). Mobs, items and
//! experience orbs far from every player are kept as their NBT and come
//! back when someone comes near; entities Ferrocraft does not know stay as
//! they were loaded.

use std::collections::{HashMap, HashSet};

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::packets::play::{ItemStack, equipment_slot};
use tracing::{debug, warn};
use uuid::Uuid;

use super::{Kind, Mob};
use crate::data::GameData;
use crate::game::Game;
use crate::storage::{DATA_VERSION, Storage, item_from_nbt, item_to_nbt, region_of};
use crate::world::ChunkPos;

/// Players bring the entities of chunks this close back into the game
/// and leave those farther than [`UNLOAD_RADIUS`] to the disk.
const LOAD_RADIUS: i32 = 8;
const UNLOAD_RADIUS: i32 = 10;

/// Entities of chunks no player is near, as their NBT.
#[derive(Debug, Default)]
pub struct StoredEntities {
    chunks: HashMap<ChunkPos, Vec<Nbt>>,
    /// Entity regions on disk, to empty them when their entities are gone.
    regions: HashSet<(i32, i32)>,
}

impl StoredEntities {
    /// Reads every entity region of the world.
    pub fn load(storage: &Storage) -> anyhow::Result<Self> {
        let mut stored = Self::default();
        for (pos, chunk) in storage.load_entity_chunks()? {
            stored.regions.insert(region_of(pos));
            if let Some(Nbt::List(entities)) = chunk.get("Entities")
                && !entities.is_empty()
            {
                stored.chunks.insert(pos, entities.clone());
            }
        }
        Ok(stored)
    }

    pub fn len(&self) -> usize {
        self.chunks.values().map(Vec::len).sum()
    }
}

/// `[x, y, z]` as a list of doubles.
pub(in crate::game) fn doubles(values: &[f64]) -> Nbt {
    Nbt::List(values.iter().map(|&v| Nbt::Double(v)).collect())
}

pub(in crate::game) fn read_doubles(nbt: &Nbt, key: &str) -> Option<[f64; 3]> {
    let Some(Nbt::List(list)) = nbt.get(key) else {
        return None;
    };
    let values: Vec<f64> = list
        .iter()
        .filter_map(|v| match v {
            Nbt::Double(v) => Some(*v),
            Nbt::Float(v) => Some(f64::from(*v)),
            _ => None,
        })
        .collect();
    match values[..] {
        [x, y, z] if values.iter().all(|v| v.is_finite()) => Some([x, y, z]),
        _ => None,
    }
}

/// `UUIDUtil.CODEC`: four ints, most significant first.
pub(in crate::game) fn uuid_to_nbt(uuid: Uuid) -> Nbt {
    let bits = uuid.as_u128();
    Nbt::IntArray(
        (0..4)
            .rev()
            .map(|i| (bits >> (i * 32)) as u32 as i32)
            .collect(),
    )
}

pub(in crate::game) fn uuid_from_nbt(nbt: &Nbt) -> Option<Uuid> {
    let Some(Nbt::IntArray(ints)) = nbt.get("UUID") else {
        return None;
    };
    let [a, b, c, d] = ints[..] else {
        return None;
    };
    let bits = [a, b, c, d]
        .iter()
        .fold(0u128, |acc, &i| acc << 32 | u128::from(i as u32));
    Some(Uuid::from_u128(bits))
}

/// The entries of `fresh`, then whatever `stored` had besides.
pub(in crate::game) fn merge(stored: Option<&Nbt>, fresh: Vec<(String, Nbt)>) -> Nbt {
    let mut entries = fresh;
    if let Some(Nbt::Compound(old)) = stored {
        for (key, value) in old {
            if !entries.iter().any(|(k, _)| k == key) {
                entries.push((key.clone(), value.clone()));
            }
        }
    }
    Nbt::Compound(entries)
}

fn byte(nbt: &Nbt, key: &str) -> bool {
    matches!(nbt.get(key), Some(Nbt::Byte(1..)))
}

impl Mob {
    /// `Entity.saveWithoutId` with what the mob's classes add.
    fn to_nbt(&self, data: &GameData) -> Nbt {
        let s = |v: &str| Nbt::String(v.into());
        let mut entries = vec![
            ("id".to_owned(), s(self.kind.name())),
            ("Pos".into(), doubles(&self.pos)),
            ("Motion".into(), doubles(&self.velocity)),
            (
                "Rotation".into(),
                Nbt::List(vec![Nbt::Float(self.yaw), Nbt::Float(self.pitch)]),
            ),
            ("fall_distance".into(), Nbt::Double(self.fall_distance)),
            ("Fire".into(), Nbt::Short(self.fire.clamp(-1, 32767) as i16)),
            ("Air".into(), Nbt::Short(300)),
            ("OnGround".into(), Nbt::Byte(i8::from(self.on_ground))),
            ("Invulnerable".into(), Nbt::Byte(0)),
            ("PortalCooldown".into(), Nbt::Int(0)),
            ("UUID".into(), uuid_to_nbt(self.uuid)),
            ("Health".into(), Nbt::Float(self.health)),
            ("HurtTime".into(), Nbt::Short(0)),
            ("DeathTime".into(), Nbt::Short(0)),
            ("AbsorptionAmount".into(), Nbt::Float(0.0)),
            ("FallFlying".into(), Nbt::Byte(0)),
            ("CanPickUpLoot".into(), Nbt::Byte(0)),
            (
                "PersistenceRequired".into(),
                Nbt::Byte(i8::from(self.persistent)),
            ),
            ("LeftHanded".into(), Nbt::Byte(0)),
        ];
        let equipment: Vec<(String, Nbt)> = self
            .equipment
            .iter()
            .filter_map(|(slot, stack)| {
                let name = match *slot {
                    equipment_slot::MAIN_HAND => "mainhand",
                    equipment_slot::OFF_HAND => "offhand",
                    equipment_slot::FEET => "feet",
                    equipment_slot::LEGS => "legs",
                    equipment_slot::CHEST => "chest",
                    _ => "head",
                };
                Some((name.to_owned(), item_to_nbt(stack, data)?))
            })
            .collect();
        if !equipment.is_empty() {
            entries.push(("equipment".into(), Nbt::Compound(equipment)));
        }
        if !self.kind.hostile() {
            // `AgeableMob` and `Animal`.
            entries.push(("Age".into(), Nbt::Int(self.age)));
            entries.push(("ForcedAge".into(), Nbt::Int(0)));
            entries.push(("AgeLocked".into(), Nbt::Byte(0)));
            entries.push(("InLove".into(), Nbt::Int(self.in_love)));
        }
        match self.kind {
            Kind::Sheep => {
                entries.push(("Sheared".into(), Nbt::Byte(i8::from(self.sheared))));
                entries.push(("Color".into(), Nbt::Byte(self.wool as i8)));
            }
            Kind::Chicken => {
                entries.push(("IsChickenJockey".into(), Nbt::Byte(0)));
                entries.push(("EggLayTime".into(), Nbt::Int(self.egg_in as i32)));
            }
            Kind::Zombie => {
                entries.push(("IsBaby".into(), Nbt::Byte(i8::from(self.baby))));
                entries.push(("CanBreakDoors".into(), Nbt::Byte(0)));
            }
            Kind::Creeper => {
                entries.push(("powered".into(), Nbt::Byte(0)));
                entries.push(("Fuse".into(), Nbt::Short(30)));
                entries.push(("ExplosionRadius".into(), Nbt::Byte(3)));
                entries.push(("ignited".into(), Nbt::Byte(i8::from(self.ignited))));
            }
            _ => {}
        }
        merge(self.stored.as_ref(), entries)
    }

    /// A mob from its saved NBT; `None` for kinds Ferrocraft does not have.
    fn from_nbt(entity_id: i32, nbt: &Nbt, data: &GameData) -> Option<Self> {
        let Some(Nbt::String(id)) = nbt.get("id") else {
            return None;
        };
        let kind = Kind::from_name(id)?;
        let pos = read_doubles(nbt, "Pos")?;
        let mut mob = Mob::new(entity_id, kind, pos, data, &mut rand::rng())?;
        if let Some(motion) = read_doubles(nbt, "Motion") {
            mob.velocity = motion.map(|v| if v.abs() > 10.0 { 0.0 } else { v });
            mob.sent_velocity = mob.velocity;
        }
        if let Some(Nbt::List(rotation)) = nbt.get("Rotation")
            && let [Nbt::Float(yaw), Nbt::Float(pitch)] = rotation[..]
            && yaw.is_finite()
            && pitch.is_finite()
        {
            mob.yaw = yaw;
            mob.head_yaw = yaw;
            mob.pitch = pitch;
        }
        if let Some(uuid) = uuid_from_nbt(nbt) {
            mob.uuid = uuid;
        }
        if let Some(Nbt::Float(health)) = nbt.get("Health")
            && health.is_finite()
        {
            mob.health = health.clamp(0.0, mob.max_health);
        }
        if let Some(Nbt::Short(fire)) = nbt.get("Fire") {
            mob.fire = i32::from(*fire).max(0);
        }
        if let Some(Nbt::Double(fall)) = nbt.get("fall_distance") {
            mob.fall_distance = *fall;
        }
        mob.on_ground = byte(nbt, "OnGround");
        mob.persistent = byte(nbt, "PersistenceRequired");
        if let Some(equipment) = nbt.get("equipment") {
            mob.equipment = [
                ("mainhand", equipment_slot::MAIN_HAND),
                ("offhand", equipment_slot::OFF_HAND),
                ("feet", equipment_slot::FEET),
                ("legs", equipment_slot::LEGS),
                ("chest", equipment_slot::CHEST),
                ("head", equipment_slot::HEAD),
            ]
            .into_iter()
            .filter_map(|(name, slot)| Some((slot, item_from_nbt(equipment.get(name)?, data)?)))
            .collect::<Vec<(u8, ItemStack)>>();
        }
        if !kind.hostile() {
            if let Some(Nbt::Int(age)) = nbt.get("Age") {
                mob.age = *age;
                if *age < 0 {
                    mob.set_baby(true);
                }
            }
            if let Some(Nbt::Int(love)) = nbt.get("InLove") {
                mob.in_love = (*love).max(0);
            }
        }
        match kind {
            Kind::Sheep => {
                mob.sheared = byte(nbt, "Sheared");
                if let Some(Nbt::Byte(color)) = nbt.get("Color") {
                    mob.wool = (*color as u8) & 15;
                }
            }
            Kind::Chicken => {
                if let Some(Nbt::Int(time)) = nbt.get("EggLayTime") {
                    mob.egg_in = (*time).max(1) as u32;
                }
            }
            Kind::Zombie if byte(nbt, "IsBaby") => mob.make_baby(),
            Kind::Creeper => mob.ignited = byte(nbt, "ignited"),
            _ => {}
        }
        mob.tracker = crate::game::tracker::Tracker::new(&mob.pose(), mob.tracker.interval());
        mob.stored = Some(nbt.clone());
        Some(mob)
    }
}

fn chunk_of(pos: [f64; 3]) -> ChunkPos {
    ((pos[0].floor() as i32) >> 4, (pos[2].floor() as i32) >> 4)
}

impl Game {
    /// Whether a player, spectators too, is within `radius` chunks.
    fn near_player(&self, chunk: ChunkPos, radius: i32) -> bool {
        self.players.values().any(|p| {
            let at = chunk_of(p.pos);
            (at.0 - chunk.0).abs().max((at.1 - chunk.1).abs()) <= radius
        })
    }

    /// Once a second: entities of chunks a player came near come back,
    /// those of chunks everyone left go to the store.
    pub(in crate::game) fn load_and_unload_entities(&mut self) {
        if !self.tick.is_multiple_of(20) {
            return;
        }
        let data = std::sync::Arc::clone(&self.context.data);
        // Leaving.
        let far: Vec<(i32, ChunkPos)> = self
            .mobs
            .values()
            .filter(|m| m.alive())
            .map(|m| (m.entity_id, chunk_of(m.pos)))
            .filter(|&(_, chunk)| !self.near_player(chunk, UNLOAD_RADIUS))
            .collect();
        for (entity_id, chunk) in far {
            if let Some(mob) = self.mobs.get(&entity_id) {
                let nbt = mob.to_nbt(&data);
                self.stored.chunks.entry(chunk).or_default().push(nbt);
                self.remove_mob(entity_id);
            }
        }
        let far_items: Vec<(i32, ChunkPos)> = self
            .items
            .values()
            .map(|i| (i.entity_id, chunk_of(i.pos)))
            .filter(|&(_, chunk)| !self.near_player(chunk, UNLOAD_RADIUS))
            .collect();
        for (entity_id, chunk) in far_items {
            if let Some(nbt) = self.items.get(&entity_id).map(|i| i.to_nbt(&data)) {
                self.stored.chunks.entry(chunk).or_default().push(nbt);
                self.remove_item(entity_id);
            }
        }
        let far_orbs: Vec<(i32, ChunkPos)> = self
            .orbs
            .values()
            .map(|o| (o.entity_id, chunk_of(o.pos)))
            .filter(|&(_, chunk)| !self.near_player(chunk, UNLOAD_RADIUS))
            .collect();
        for (entity_id, chunk) in far_orbs {
            if let Some(nbt) = self.orbs.get(&entity_id).map(|o| o.to_nbt()) {
                self.stored.chunks.entry(chunk).or_default().push(nbt);
                self.remove_orb(entity_id);
            }
        }
        // Coming back.
        let near: Vec<ChunkPos> = self
            .stored
            .chunks
            .keys()
            .copied()
            .filter(|&chunk| self.near_player(chunk, LOAD_RADIUS))
            .collect();
        for chunk in near {
            let Some(entities) = self.stored.chunks.remove(&chunk) else {
                continue;
            };
            let mut foreign = Vec::new();
            for nbt in entities {
                if !self.revive(&nbt) {
                    foreign.push(nbt);
                }
            }
            if !foreign.is_empty() {
                // Kept for saving, but no longer looked at.
                self.foreign.entry(chunk).or_default().extend(foreign);
            }
        }
    }

    /// Brings a saved entity into the game; `false` for one Ferrocraft
    /// does not have.
    fn revive(&mut self, nbt: &Nbt) -> bool {
        let data = std::sync::Arc::clone(&self.context.data);
        let entity_id = self
            .context
            .entity_ids
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match nbt.get("id") {
            Some(Nbt::String(id)) if id == "minecraft:item" => {
                self.revive_item(entity_id, nbt);
                true
            }
            Some(Nbt::String(id)) if id == "minecraft:experience_orb" => {
                self.revive_orb(entity_id, nbt);
                true
            }
            _ => match Mob::from_nbt(entity_id, nbt, &data) {
                Some(mob) => {
                    self.add_mob(mob);
                    true
                }
                None => false,
            },
        }
    }

    /// Writes every entity, in the game or stored, to the entity regions.
    pub(in crate::game) fn save_entities(&mut self, storage: &Storage) {
        let data = std::sync::Arc::clone(&self.context.data);
        let mut chunks: HashMap<ChunkPos, Vec<Nbt>> = HashMap::new();
        for (chunk, entities) in self.stored.chunks.iter().chain(self.foreign.iter()) {
            chunks
                .entry(*chunk)
                .or_default()
                .extend(entities.iter().cloned());
        }
        for mob in self.mobs.values().filter(|m| m.alive()) {
            chunks
                .entry(chunk_of(mob.pos))
                .or_default()
                .push(mob.to_nbt(&data));
        }
        for item in self.items.values() {
            chunks
                .entry(chunk_of(item.pos))
                .or_default()
                .push(item.to_nbt(&data));
        }
        for orb in self.orbs.values() {
            chunks
                .entry(chunk_of(orb.pos))
                .or_default()
                .push(orb.to_nbt());
        }
        let mut regions: HashMap<(i32, i32), Vec<(ChunkPos, Nbt)>> = HashMap::new();
        for region in &self.stored.regions {
            regions.entry(*region).or_default();
        }
        let mut count = 0;
        for ((x, z), entities) in chunks {
            count += entities.len();
            let chunk = Nbt::Compound(vec![
                ("DataVersion".into(), Nbt::Int(DATA_VERSION)),
                ("Position".into(), Nbt::IntArray(vec![x, z])),
                ("Entities".into(), Nbt::List(entities)),
            ]);
            regions
                .entry(region_of((x, z)))
                .or_default()
                .push(((x, z), chunk));
        }
        for (region, chunks) in regions {
            match storage.write_entity_region(region, &chunks) {
                Ok(()) => {
                    if chunks.is_empty() {
                        self.stored.regions.remove(&region);
                    } else {
                        self.stored.regions.insert(region);
                    }
                }
                Err(err) => warn!("saving the entities failed: {err:#}"),
            }
        }
        debug!(entities = count, "saved the entities");
    }
}
