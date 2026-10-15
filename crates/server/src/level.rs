//! `level.dat` and the world-wide saved data next to it, in vanilla 26.3's
//! form: spawn, game time, the clock behind day and night, the generator
//! settings. With these files vanilla opens a Ferrocraft world as it is.

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::codec::BlockPos;

use crate::storage::{DATA_VERSION, Storage, read_nbt_file, write_nbt_file};
use crate::world::DIMENSION;

/// `level.dat`'s format version, unchanged for years.
const LEVEL_VERSION: i32 = 19133;

#[derive(Debug, Clone, PartialEq)]
pub struct Level {
    pub name: String,
    /// Ticks since the world was created.
    pub game_time: i64,
    /// Ticks of the overworld clock: the time of day is this modulo 24000.
    pub day_time: i64,
    pub spawn: BlockPos,
    pub spawn_angle: [f32; 2],
    /// 0 survival, 1 creative, 2 adventure, 3 spectator.
    pub game_type: i32,
    pub seed: i64,
}

impl Level {
    pub fn new(spawn: BlockPos, game_type: i32) -> Self {
        Self {
            name: "world".into(),
            game_time: 0,
            day_time: 0,
            spawn,
            spawn_angle: [0.0, 0.0],
            game_type,
            seed: rand::random(),
        }
    }

    /// Reads `level.dat`, the clocks and the seed; `None` for a new world.
    pub fn load(storage: &Storage) -> anyhow::Result<Option<Self>> {
        let Some(root) = read_nbt_file(&storage.path("level.dat"))? else {
            return Ok(None);
        };
        let data = root.get("Data").unwrap_or(&root);
        let long = |nbt: Option<&Nbt>| match nbt {
            Some(Nbt::Long(v)) => *v,
            _ => 0,
        };
        let spawn = data.get("spawn");
        let pos = match spawn.and_then(|s| s.get("pos")) {
            Some(Nbt::IntArray(pos)) if pos.len() == 3 => BlockPos {
                x: pos[0],
                y: pos[1],
                z: pos[2],
            },
            _ => BlockPos { x: 0, y: 0, z: 0 },
        };
        let float = |key| match spawn.and_then(|s| s.get(key)) {
            Some(Nbt::Float(v)) => *v,
            _ => 0.0,
        };
        let game_time = long(data.get("Time"));
        let clocks = read_nbt_file(&storage.path("data/minecraft/world_clocks.dat"))?;
        let day_time = clocks
            .as_ref()
            .and_then(|c| c.get("data")?.get(DIMENSION)?.get("total_ticks"))
            .map_or(game_time, |t| long(Some(t)));
        let seed = read_nbt_file(&storage.path("data/minecraft/world_gen_settings.dat"))?
            .as_ref()
            .and_then(|s| s.get("data")?.get("seed").cloned())
            .map_or(0, |s| long(Some(&s)));
        Ok(Some(Self {
            name: match data.get("LevelName") {
                Some(Nbt::String(name)) => name.clone(),
                _ => "world".into(),
            },
            game_time,
            day_time,
            spawn: pos,
            spawn_angle: [float("yaw"), float("pitch")],
            game_type: match data.get("GameType") {
                Some(Nbt::Int(t)) => *t,
                _ => 0,
            },
            seed,
        }))
    }

    /// Writes `level.dat` and the clocks, and the generator settings if the
    /// world has none yet: Ferrocraft's superflat preset, so that vanilla
    /// generates the same ground where Ferrocraft saved nothing.
    pub fn save(
        &self,
        storage: &Storage,
        layers: &[(&str, i32)],
        biome: &str,
        difficulty: &str,
    ) -> anyhow::Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        let s = |v: &str| Nbt::String(v.into());
        let level = Nbt::Compound(vec![(
            "Data".into(),
            Nbt::Compound(vec![
                (
                    "difficulty_settings".into(),
                    Nbt::Compound(vec![
                        ("difficulty".into(), s(difficulty)),
                        ("hardcore".into(), Nbt::Byte(0)),
                        ("locked".into(), Nbt::Byte(0)),
                    ]),
                ),
                ("Time".into(), Nbt::Long(self.game_time)),
                ("version_history".into(), Nbt::IntArray(vec![DATA_VERSION])),
                ("GameType".into(), Nbt::Int(self.game_type)),
                ("ServerBrands".into(), Nbt::List(vec![s("ferrocraft")])),
                ("version".into(), Nbt::Int(LEVEL_VERSION)),
                ("LastPlayed".into(), Nbt::Long(now)),
                (
                    "spawn".into(),
                    Nbt::Compound(vec![
                        (
                            "pos".into(),
                            Nbt::IntArray(vec![self.spawn.x, self.spawn.y, self.spawn.z]),
                        ),
                        ("pitch".into(), Nbt::Float(self.spawn_angle[1])),
                        ("dimension".into(), s(DIMENSION)),
                        ("yaw".into(), Nbt::Float(self.spawn_angle[0])),
                    ]),
                ),
                (
                    "Version".into(),
                    Nbt::Compound(vec![
                        ("Snapshot".into(), Nbt::Byte(0)),
                        ("Series".into(), s("main")),
                        ("Id".into(), Nbt::Int(DATA_VERSION)),
                        ("Name".into(), s(ferrocraft_protocol::GAME_VERSION)),
                    ]),
                ),
                ("LevelName".into(), s(&self.name)),
                ("initialized".into(), Nbt::Byte(1)),
                // Not vanilla, which is what this flag says.
                ("WasModded".into(), Nbt::Byte(1)),
                ("DataVersion".into(), Nbt::Int(DATA_VERSION)),
                ("allowCommands".into(), Nbt::Byte(0)),
                (
                    "DataPacks".into(),
                    Nbt::Compound(vec![
                        ("Enabled".into(), Nbt::List(vec![s("vanilla")])),
                        ("Disabled".into(), Nbt::List(Vec::new())),
                    ]),
                ),
            ]),
        )]);
        write_nbt_file(&storage.path("level.dat"), &level)?;

        let clock = |ticks| Nbt::Compound(vec![("total_ticks".into(), Nbt::Long(ticks))]);
        let clocks = Nbt::Compound(vec![
            (
                "data".into(),
                Nbt::Compound(vec![
                    (DIMENSION.into(), clock(self.day_time)),
                    ("minecraft:the_end".into(), clock(self.game_time)),
                ]),
            ),
            ("DataVersion".into(), Nbt::Int(DATA_VERSION)),
        ]);
        write_nbt_file(&storage.path("data/minecraft/world_clocks.dat"), &clocks)?;

        let settings = storage.path("data/minecraft/world_gen_settings.dat");
        if !settings.exists() {
            write_nbt_file(&settings, &generator_settings(self.seed, layers, biome))?;
        }
        Ok(())
    }
}

/// `world_gen_settings.dat` for a superflat overworld with these layers; the
/// nether and the end as vanilla makes them.
fn generator_settings(seed: i64, layers: &[(&str, i32)], biome: &str) -> Nbt {
    let s = |v: &str| Nbt::String(v.into());
    let noise = |settings: &str, source: Nbt, kind: &str| {
        Nbt::Compound(vec![
            (
                "generator".into(),
                Nbt::Compound(vec![
                    ("settings".into(), s(settings)),
                    ("biome_source".into(), source),
                    ("type".into(), s("minecraft:noise")),
                ]),
            ),
            ("type".into(), s(kind)),
        ])
    };
    Nbt::Compound(vec![
        (
            "data".into(),
            Nbt::Compound(vec![
                ("bonus_chest".into(), Nbt::Byte(0)),
                ("seed".into(), Nbt::Long(seed)),
                ("generate_structures".into(), Nbt::Byte(0)),
                (
                    "dimensions".into(),
                    Nbt::Compound(vec![
                        (
                            DIMENSION.into(),
                            Nbt::Compound(vec![
                                (
                                    "generator".into(),
                                    Nbt::Compound(vec![
                                        (
                                            "settings".into(),
                                            Nbt::Compound(vec![
                                                ("features".into(), Nbt::Byte(0)),
                                                ("biome".into(), s(biome)),
                                                (
                                                    "layers".into(),
                                                    Nbt::List(
                                                        layers
                                                            .iter()
                                                            .map(|(block, height)| {
                                                                Nbt::Compound(vec![
                                                                    ("block".into(), s(block)),
                                                                    (
                                                                        "height".into(),
                                                                        Nbt::Int(*height),
                                                                    ),
                                                                ])
                                                            })
                                                            .collect(),
                                                    ),
                                                ),
                                                ("lakes".into(), Nbt::Byte(0)),
                                            ]),
                                        ),
                                        ("type".into(), s("minecraft:flat")),
                                    ]),
                                ),
                                ("type".into(), s(DIMENSION)),
                            ]),
                        ),
                        (
                            "minecraft:the_nether".into(),
                            noise(
                                "minecraft:nether",
                                Nbt::Compound(vec![
                                    ("preset".into(), s("minecraft:nether")),
                                    ("type".into(), s("minecraft:multi_noise")),
                                ]),
                                "minecraft:the_nether",
                            ),
                        ),
                        (
                            "minecraft:the_end".into(),
                            noise(
                                "minecraft:end",
                                Nbt::Compound(vec![("type".into(), s("minecraft:the_end"))]),
                                "minecraft:the_end",
                            ),
                        ),
                    ]),
                ),
            ]),
        ),
        ("DataVersion".into(), Nbt::Int(DATA_VERSION)),
    ])
}

/// `BiomeManager.obfuscateSeed`: the first eight bytes of the seed's
/// SHA-256, which clients use for biome noise.
pub fn hashed_seed(seed: i64) -> i64 {
    let digest = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, &seed.to_le_bytes());
    i64::from_le_bytes(digest.as_ref()[..8].try_into().expect("32 bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_survives_a_save() {
        let dir = std::env::temp_dir().join(format!("ferrocraft-level-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let storage = Storage::open(&dir).unwrap();
        assert_eq!(Level::load(&storage).unwrap(), None);
        let mut level = Level::new(
            BlockPos {
                x: 3,
                y: -60,
                z: -7,
            },
            1,
        );
        level.game_time = 12_345;
        level.day_time = 30_000;
        level
            .save(
                &storage,
                &[("minecraft:bedrock", 1)],
                "minecraft:plains",
                "normal",
            )
            .unwrap();
        assert_eq!(Level::load(&storage).unwrap(), Some(level));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
