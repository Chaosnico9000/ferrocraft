//! What animals want (`Animal` and its goals: `PanicGoal`, `TemptGoal`,
//! `LookAtPlayerGoal`, `RandomLookAroundGoal`, `RandomStrollGoal`), and
//! what a right click on a mob does: shearing sheep, milking cows,
//! lighting creepers.

use std::sync::Arc;

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{GameMode, ItemStack, SOUND_SOURCE_PLAYERS};
use rand::Rng;

use super::{DYES, Goal, Kind, Look, Mob, approach, block_at, wrap};
use crate::data::GameData;
use crate::game::Game;
use crate::world::World;

impl Kind {
    /// The speed factors of their goals: panic, tempt.
    fn speeds(self) -> (f64, f64) {
        match self {
            Kind::Cow => (2.0, 1.25),
            Kind::Pig => (1.25, 1.2),
            Kind::Sheep => (1.25, 1.1),
            Kind::Chicken => (1.4, 1.0),
            _ => (1.0, 1.0),
        }
    }

    /// What makes them follow (`*_FOOD` tags).
    fn food_tag(self) -> &'static str {
        match self {
            Kind::Cow => "minecraft:cow_food",
            Kind::Pig => "minecraft:pig_food",
            Kind::Sheep => "minecraft:sheep_food",
            _ => "minecraft:chicken_food",
        }
    }
}

/// `DefaultRandomPos.getPos`: somewhere within `horizontal` blocks and
/// `vertical` up or down where the mob could stand, if the first try finds
/// such a place.
pub(super) fn random_spot(
    world: &World,
    data: &GameData,
    mob: &Mob,
    horizontal: i32,
    vertical: i32,
    rng: &mut impl Rng,
) -> Option<[f64; 3]> {
    for _ in 0..10 {
        let x = mob.pos[0].floor() as i32 + rng.random_range(-horizontal..=horizontal);
        let z = mob.pos[2].floor() as i32 + rng.random_range(-horizontal..=horizontal);
        let base = mob.pos[1].floor() as i32;
        let tall = mob.height.ceil() as i32;
        for y in (base - vertical..=base + vertical).rev() {
            let at = |dy| BlockPos { x, y: y + dy, z };
            let free = (0..tall).all(|dy| {
                let state = world.block(at(dy));
                data.collision_shape(state).is_empty() && data.physics(state).fluid == 0
            });
            let below = world.block(at(-1));
            if free && !data.collision_shape(below).is_empty() && data.physics(below).fluid == 0 {
                return Some([f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5]);
            }
        }
    }
    None
}

impl Game {
    /// The goals, by priority: panic, follow food, look, stroll.
    pub(super) fn think_animal(&mut self, entity_id: i32) {
        let data = Arc::clone(&self.context.data);
        let world = Arc::clone(&self.context.world);
        let Some(mob) = self.mobs.get(&entity_id) else {
            return;
        };
        let mut rng = rand::rng();
        let (panic_speed, tempt_speed) = mob.kind.speeds();
        // `TemptGoal`: the nearest player within 10 blocks holding food.
        let food = data.item_tag(mob.kind.food_tag()).unwrap_or(&[]);
        let tempter = self
            .players
            .values()
            .filter(|p| p.game_mode() != GameMode::Spectator && !p.dead)
            .filter(|p| {
                [
                    ferrocraft_protocol::packets::play::equipment_slot::MAIN_HAND,
                    ferrocraft_protocol::packets::play::equipment_slot::OFF_HAND,
                ]
                .iter()
                .any(|&hand| {
                    p.inventory
                        .equipment(hand)
                        .is_some_and(|s| food.contains(&s.item))
                })
            })
            .map(|p| {
                (
                    p.entity_id,
                    p.pos,
                    (p.pos[0] - mob.pos[0]).hypot(p.pos[2] - mob.pos[2]),
                )
            })
            .filter(|&(_, _, d)| d < 10.0)
            .min_by(|a, b| a.2.total_cmp(&b.2));
        let watcher = self.watcher(mob, 6.0);
        let looked_at = match mob.look {
            Look::At { player, .. } => self.players.get(&player).map(|p| p.eyes()),
            _ => None,
        };
        let panic_spot = (mob.panic > 0 && !mob.has_path())
            .then(|| random_spot(&world, &data, mob, 5, 4, &mut rng))
            .flatten();
        let stroll_spot = (mob.goal == Goal::Idle && rng.random_range(0..120) == 0)
            .then(|| random_spot(&world, &data, mob, 10, 7, &mut rng))
            .flatten();
        let Some(mob) = self.mobs.get_mut(&entity_id) else {
            return;
        };
        if mob.panic > 0 {
            mob.panic -= 1;
            if let Some(spot) = panic_spot {
                mob.goal = Goal::Walk {
                    target: spot,
                    speed: panic_speed,
                };
            }
        } else if let Some((player, at, distance)) = tempter {
            if distance > 2.5 {
                // Follow where the player is now, every half second anew.
                if mob.navigation.goal != Some(block_at(at))
                    && (mob.navigation.ticks > 10 || !mob.has_path())
                {
                    mob.stop();
                }
                mob.goal = Goal::Walk {
                    target: at,
                    speed: tempt_speed,
                };
            } else {
                mob.goal = Goal::Idle;
                mob.stop();
            }
            mob.look = Look::At { player, ticks: 2 };
        } else if let Some(spot) = stroll_spot {
            // `RandomStrollGoal`: now and then, somewhere within ten blocks.
            mob.goal = Goal::Walk {
                target: spot,
                speed: 1.0,
            };
        }
        look_around(mob, watcher, looked_at, &mut rng);
    }

    /// The nearest player within `range` blocks for `LookAtPlayerGoal`.
    pub(super) fn watcher(&self, mob: &Mob, range: f64) -> Option<(i32, f64)> {
        self.players
            .values()
            .filter(|p| p.game_mode() != GameMode::Spectator)
            .map(|p| {
                (
                    p.entity_id,
                    (p.pos[0] - mob.pos[0]).hypot(p.pos[2] - mob.pos[2]),
                )
            })
            .filter(|&(_, d)| d < range)
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// A right click on an animal with shears or a bucket
    /// (`Sheep.mobInteract`, `AbstractCow.mobInteract`). Returns whether
    /// something happened.
    pub(in crate::game) fn interact_mob(
        &mut self,
        entity_id: i32,
        player_id: i32,
        hand: i32,
    ) -> bool {
        let data = Arc::clone(&self.context.data);
        let Some(player) = self.players.get(&player_id) else {
            return false;
        };
        let slot = if hand == 1 {
            crate::inventory::OFF_HAND
        } else {
            player.inventory.selected_slot()
        };
        let creative = player.game_mode() == GameMode::Creative;
        let Some(held) = player.inventory.get(slot).cloned() else {
            return false;
        };
        let held_name = data.item(held.item).map_or("", |i| i.name.as_str());
        let Some(mob) = self.mobs.get_mut(&entity_id).filter(|m| m.alive()) else {
            return false;
        };
        match (mob.kind, held_name) {
            (Kind::Sheep, "minecraft:shears") if !mob.sheared => {
                mob.sheared = true;
                mob.data_dirty = true;
                let wool = data.item_id(&format!(
                    "minecraft:{}_wool",
                    DYES[usize::from(mob.wool & 15)]
                ));
                let pos = mob.pos;
                let count = rand::rng().random_range(1..=3);
                if let Some(mob) = self.mobs.get(&entity_id) {
                    self.mob_sound(mob, "shear", 1.0);
                }
                if let Some(wool) = wool {
                    for _ in 0..count {
                        let mut rng = rand::rng();
                        let velocity = [
                            (rng.random::<f64>() - rng.random::<f64>()) * 0.1,
                            rng.random::<f64>() * 0.05,
                            (rng.random::<f64>() - rng.random::<f64>()) * 0.1,
                        ];
                        self.spawn_item(
                            [pos[0], pos[1] + 1.0, pos[2]],
                            velocity,
                            ItemStack {
                                item: wool,
                                count: 1,
                                patch: Default::default(),
                            },
                            10,
                        );
                    }
                }
                if !creative {
                    self.wear(player_id, slot, 1);
                }
                true
            }
            (Kind::Cow, "minecraft:bucket") => {
                let at = mob.pos;
                self.play_sound_from(
                    None,
                    "minecraft:entity.cow.milk",
                    at,
                    SOUND_SOURCE_PLAYERS,
                    1.0,
                    1.0,
                );
                let Some(milk) = data.item_id("minecraft:milk_bucket") else {
                    return false;
                };
                let milk = ItemStack {
                    item: milk,
                    count: 1,
                    patch: Default::default(),
                };
                // `ItemUtils.createFilledResult`.
                if creative {
                    let has = self.players.get(&player_id).is_some_and(|p| {
                        p.inventory
                            .slots()
                            .iter()
                            .flatten()
                            .any(|s| s.item == milk.item)
                    });
                    if !has {
                        self.give(player_id, milk);
                    }
                } else if held.count == 1 {
                    self.update_inventory(player_id, |inventory| inventory.set(slot, Some(milk)));
                    self.send_slots(player_id, &[slot]);
                } else {
                    self.update_inventory(player_id, |inventory| {
                        inventory.take(slot, 1);
                    });
                    self.send_slots(player_id, &[slot]);
                    self.give(player_id, milk);
                }
                true
            }
            // `Creeper.mobInteract` with `#creeper_igniters`.
            (Kind::Creeper, "minecraft:flint_and_steel" | "minecraft:fire_charge")
                if !mob.ignited =>
            {
                let at = mob.pos;
                let fire_charge = held_name == "minecraft:fire_charge";
                let sound = if fire_charge {
                    "minecraft:item.firecharge.use"
                } else {
                    "minecraft:item.flintandsteel.use"
                };
                let pitch = rand::rng().random::<f32>() * 0.4 + 0.8;
                self.play_sound_from(None, sound, at, SOUND_SOURCE_PLAYERS, 1.0, pitch);
                self.ignite_creeper(entity_id);
                if !creative {
                    if fire_charge {
                        self.update_inventory(player_id, |inventory| {
                            inventory.take(slot, 1);
                        });
                        self.send_slots(player_id, &[slot]);
                    } else {
                        self.wear(player_id, slot, 1);
                    }
                }
                true
            }
            _ => false,
        }
    }
}

/// `LookAtPlayerGoal`, `RandomLookAroundGoal` and `LookControl`: where the
/// head turns this tick.
pub(super) fn look_around(
    mob: &mut Mob,
    watcher: Option<(i32, f64)>,
    looked_at: Option<[f64; 3]>,
    rng: &mut impl Rng,
) {
    let eyes = mob.eyes();
    mob.look = match mob.look {
        Look::At { player, ticks } if ticks > 0 && looked_at.is_some() => Look::At {
            player,
            ticks: ticks - 1,
        },
        Look::Around { yaw, ticks } if ticks > 0 => Look::Around {
            yaw,
            ticks: ticks - 1,
        },
        _ => {
            if let Some((player, _)) = watcher.filter(|_| rng.random::<f32>() < 0.02) {
                Look::At {
                    player,
                    ticks: 40 + rng.random_range(0..40),
                }
            } else if rng.random::<f32>() < 0.02 {
                Look::Around {
                    yaw: mob.yaw + rng.random_range(-90.0..90.0),
                    ticks: 20 + rng.random_range(0..20),
                }
            } else {
                Look::Ahead
            }
        }
    };
    let (head_yaw, pitch) = match (mob.look, looked_at) {
        (Look::At { .. }, Some(target)) => face(eyes, target),
        (Look::Around { yaw, .. }, _) => (yaw, 0.0),
        _ => (mob.yaw, 0.0),
    };
    turn_head(mob, head_yaw, pitch);
}

/// The yaw and pitch from `eyes` to `target`.
pub(super) fn face(eyes: [f64; 3], target: [f64; 3]) -> (f32, f32) {
    let [dx, dy, dz] = [
        target[0] - eyes[0],
        target[1] - eyes[1],
        target[2] - eyes[2],
    ];
    (
        (-dx).atan2(dz).to_degrees() as f32,
        (-dy.atan2(dx.hypot(dz))).to_degrees() as f32,
    )
}

/// `LookControl`: ten degrees a tick, the head at most 75 degrees off the
/// body.
pub(super) fn turn_head(mob: &mut Mob, head_yaw: f32, pitch: f32) {
    let head = approach(mob.head_yaw, head_yaw, 10.0);
    mob.head_yaw = mob.yaw + wrap(head - mob.yaw).clamp(-75.0, 75.0);
    mob.pitch = approach(mob.pitch, pitch, 10.0);
}
