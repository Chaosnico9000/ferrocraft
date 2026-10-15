//! `NaturalSpawner`: every tick a try per chunk near the players for
//! monsters, every 400 ticks for animals, until each player has their share
//! around them (`LocalMobCapCalculator`). Monsters need the dark, animals
//! grass and light. What spawns comes from the plains biome's
//! `natural_mob_spawns`.

use std::collections::{HashMap, HashSet};

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::GameMode;
use rand::Rng;

use super::Kind;
use crate::game::Game;
use crate::world::{ChunkPos, MIN_Y};

/// Chunks within this many of a player get spawn tries.
const SPAWN_RADIUS: i32 = 8;
/// `MobCategory.getMaxInstancesPerChunk`, per player.
const MONSTER_CAP: usize = 70;
const CREATURE_CAP: usize = 10;
/// The area a cap counts for, 17 × 17 chunks (`NaturalSpawner.MAGIC_NUMBER`).
const CAP_AREA: usize = 289;

/// What the plains biome spawns, by weight. Kinds Ferrocraft does not have
/// yet still take their share of the tries.
const MONSTERS: &[(Option<Kind>, u32)] = &[
    (Some(Kind::Spider), 100),
    (Some(Kind::Zombie), 90),
    (None, 5),
    (None, 5),
    (Some(Kind::Skeleton), 100),
    (Some(Kind::Creeper), 100),
    (None, 100),
    (None, 10),
    (None, 5),
];
const CREATURES: &[(Option<Kind>, u32)] = &[
    (Some(Kind::Sheep), 12),
    (Some(Kind::Pig), 10),
    (Some(Kind::Chicken), 10),
    (Some(Kind::Cow), 8),
    (None, 5),
    (None, 1),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    Monster,
    Creature,
}

impl Category {
    fn cap(self) -> usize {
        match self {
            Category::Monster => MONSTER_CAP,
            Category::Creature => CREATURE_CAP,
        }
    }

    fn spawners(self) -> &'static [(Option<Kind>, u32)] {
        match self {
            Category::Monster => MONSTERS,
            Category::Creature => CREATURES,
        }
    }

    fn of(kind: Kind) -> Self {
        if kind.hostile() {
            Category::Monster
        } else {
            Category::Creature
        }
    }
}

fn pick(spawners: &[(Option<Kind>, u32)], rng: &mut impl Rng) -> Option<Kind> {
    let total: u32 = spawners.iter().map(|s| s.1).sum();
    let mut roll = rng.random_range(0..total);
    for &(kind, weight) in spawners {
        if roll < weight {
            return kind;
        }
        roll -= weight;
    }
    None
}

fn chunk_distance(a: ChunkPos, b: ChunkPos) -> i32 {
    (a.0 - b.0).abs().max((a.1 - b.1).abs())
}

impl Game {
    /// One round of the spawner.
    pub(in crate::game) fn spawn_mobs(&mut self) {
        let mut categories = Vec::new();
        if self.context.monsters && self.context.difficulty > 0 {
            categories.push(Category::Monster);
        }
        if self.context.animals && self.tick.is_multiple_of(400) {
            categories.push(Category::Creature);
        }
        if categories.is_empty() {
            return;
        }
        let players: Vec<(i32, [f64; 3], ChunkPos)> = self
            .players
            .values()
            .filter(|p| p.game_mode() != GameMode::Spectator)
            .map(|p| {
                let chunk = (
                    (p.pos[0].floor() as i32) >> 4,
                    (p.pos[2].floor() as i32) >> 4,
                );
                (p.entity_id, p.pos, chunk)
            })
            .collect();
        if players.is_empty() {
            return;
        }
        let mut chunks: HashSet<ChunkPos> = HashSet::new();
        for &(_, _, (cx, cz)) in &players {
            for x in cx - SPAWN_RADIUS..=cx + SPAWN_RADIUS {
                for z in cz - SPAWN_RADIUS..=cz + SPAWN_RADIUS {
                    chunks.insert((x, z));
                }
            }
        }
        for category in categories {
            // What each player already has around them.
            let mut counts: HashMap<i32, usize> = HashMap::new();
            let mut total = 0;
            for mob in self.mobs.values() {
                if Category::of(mob.kind) != category || !mob.alive() {
                    continue;
                }
                total += 1;
                let chunk = (
                    (mob.pos[0].floor() as i32) >> 4,
                    (mob.pos[2].floor() as i32) >> 4,
                );
                for &(id, _, at) in &players {
                    if chunk_distance(chunk, at) <= SPAWN_RADIUS {
                        *counts.entry(id).or_default() += 1;
                    }
                }
            }
            // `canSpawnForCategoryGlobal`.
            if total >= category.cap() * chunks.len() / CAP_AREA {
                continue;
            }
            let mut order: Vec<ChunkPos> = chunks.iter().copied().collect();
            order.sort_unstable();
            for chunk in order {
                // `canSpawnForCategoryLocal`: some player near it has room.
                let room = players.iter().any(|&(id, _, at)| {
                    chunk_distance(chunk, at) <= SPAWN_RADIUS
                        && counts.get(&id).copied().unwrap_or(0) < category.cap()
                });
                if !room {
                    continue;
                }
                for kind in self.spawn_in_chunk(category, chunk) {
                    let at = kind.1;
                    let chunk = ((at[0].floor() as i32) >> 4, (at[2].floor() as i32) >> 4);
                    for &(id, _, near) in &players {
                        if chunk_distance(chunk, near) <= SPAWN_RADIUS {
                            *counts.entry(id).or_default() += 1;
                        }
                    }
                }
            }
        }
    }

    /// `spawnCategoryForChunk`: a random spot in the chunk, then up to three
    /// groups around it. Returns what spawned.
    fn spawn_in_chunk(&mut self, category: Category, (cx, cz): ChunkPos) -> Vec<(Kind, [f64; 3])> {
        let mut rng = rand::rng();
        let x = (cx << 4) + rng.random_range(0..16);
        let z = (cz << 4) + rng.random_range(0..16);
        let top = self.context.world.top_y(x, z) + 2;
        let y = rng.random_range(MIN_Y..=top.max(MIN_Y));
        let mut spawned = Vec::new();
        if y < MIN_Y + 1 {
            return spawned;
        }
        // A solid block where it starts: nothing spawns.
        let start = self.context.world.block(BlockPos { x, y, z });
        if self.context.data.physics(start).dampening == 15
            && !self.context.data.collision_shape(start).is_empty()
        {
            return spawned;
        }
        for _ in 0..3 {
            let (mut px, mut pz) = (x, z);
            let mut kind = None;
            let mut size = (rng.random::<f32>() * 4.0).ceil() as i32;
            let mut group = 0;
            let mut tries = 0;
            while tries < size {
                tries += 1;
                px += rng.random_range(0..6) - rng.random_range(0..6);
                pz += rng.random_range(0..6) - rng.random_range(0..6);
                let at = [f64::from(px) + 0.5, f64::from(y), f64::from(pz) + 0.5];
                let Some(distance) = self.nearest_player_distance_sq(at) else {
                    break;
                };
                // `isRightDistanceToPlayerAndSpawnPoint`.
                let spawn = self.context.level.spawn;
                let from_spawn = (f64::from(spawn.x) + 0.5 - at[0]).powi(2)
                    + (f64::from(spawn.y) + 0.5 - at[1]).powi(2)
                    + (f64::from(spawn.z) + 0.5 - at[2]).powi(2);
                if distance <= 576.0 || from_spawn < 24.0 * 24.0 || distance > 128.0 * 128.0 {
                    continue;
                }
                let chosen = match kind {
                    Some(chosen) => chosen,
                    None => match pick(category.spawners(), &mut rng) {
                        Some(chosen) => {
                            kind = Some(chosen);
                            // Every kind here comes in groups of up to four.
                            size = 4;
                            chosen
                        }
                        None => break,
                    },
                };
                let pos = BlockPos { x: px, y, z: pz };
                if !self.spawnable(category, chosen, pos, &mut rng) {
                    continue;
                }
                if self.spawn_mob(chosen, at).is_some() {
                    spawned.push((chosen, at));
                    group += 1;
                    // `getMaxSpawnClusterSize`: four at most.
                    if spawned.len() >= 4 {
                        return spawned;
                    }
                    if group >= 4 {
                        break;
                    }
                }
            }
        }
        spawned
    }

    /// `SpawnPlacements` for walking mobs, their rules and room for the
    /// body.
    fn spawnable(&self, category: Category, kind: Kind, pos: BlockPos, rng: &mut impl Rng) -> bool {
        let data = &self.context.data;
        let world = &self.context.world;
        let below = BlockPos {
            y: pos.y - 1,
            ..pos
        };
        let below_state = world.block(below);
        // `SpawnPlacementTypes.ON_GROUND` and `isValidEmptySpawnBlock`.
        let empty = |p: BlockPos| {
            let state = world.block(p);
            data.collision_shape(state).is_empty() && data.physics(state).fluid == 0
        };
        if !data.physics(below_state).spawnable
            || !empty(pos)
            || !empty(BlockPos {
                y: pos.y + 1,
                ..pos
            })
        {
            return false;
        }
        let rules = match category {
            // `Animal.checkAnimalSpawnRules`: grass and light.
            Category::Creature => {
                data.block_of(below_state)
                    .is_some_and(|(id, _)| data.block_in_tag("minecraft:animals_spawnable_on", id))
                    && self.brightness(pos) > 8
            }
            // `Monster.isDarkEnoughToSpawn`; the dimension allows no block
            // light and up to 7 of the rest.
            Category::Monster => {
                let sky = i32::from(world.light(crate::light::Kind::Sky, pos));
                sky <= rng.random_range(0..32)
                    && world.light(crate::light::Kind::Block, pos) == 0
                    && self.brightness(pos) <= rng.random_range(0..=7)
            }
        };
        if !rules {
            return false;
        }
        // The whole body fits (`noCollision` of the spawn box).
        let Ok((_, info)) = data.entity(kind.name()) else {
            return false;
        };
        let half = info.width / 2.0;
        let (x, z) = (f64::from(pos.x) + 0.5, f64::from(pos.z) + 0.5);
        let bounds = [
            x - half,
            f64::from(pos.y),
            z - half,
            x + half,
            f64::from(pos.y) + info.height,
            z + half,
        ];
        !crate::game::physics::blocked(world, data, bounds)
    }
}
