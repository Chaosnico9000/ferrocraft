//! Explosions (`ServerExplosion`): rays from the centre eat through blocks
//! by their blast resistance, entities nearby are hurt and thrown by how
//! much of them the blast sees, and the blocks it breaks drop their loot,
//! each with a chance of one in the radius (`explosion_decay`).

use std::collections::HashSet;
use std::sync::Arc;

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{Explode, ExplosionParticle, GameMode, text};
use rand::Rng;
use rand::seq::SliceRandom;

use super::physics;
use super::survival::Hit;
use super::{Encoded, Game, Outbound};
use crate::data::Aabb;
use crate::loot;
use crate::world::{MAX_Y, MIN_Y};

/// `ServerExplosion.getSeenPercent`: the share of points spread over the
/// box from which the centre can be seen.
fn exposure(game: &Game, center: [f64; 3], b: &Aabb) -> f32 {
    let step = |lo: f64, hi: f64| 1.0 / ((hi - lo) * 2.0 + 1.0);
    let (xs, ys, zs) = (step(b[0], b[3]), step(b[1], b[4]), step(b[2], b[5]));
    let x_offset = (1.0 - (1.0 / xs).floor() * xs) / 2.0;
    let z_offset = (1.0 - (1.0 / zs).floor() * zs) / 2.0;
    let (mut hits, mut count) = (0, 0);
    let mut xx = 0.0;
    while xx <= 1.0 {
        let mut yy = 0.0;
        while yy <= 1.0 {
            let mut zz = 0.0;
            while zz <= 1.0 {
                let from = [
                    b[0] + (b[3] - b[0]) * xx + x_offset,
                    b[1] + (b[4] - b[1]) * yy,
                    b[2] + (b[5] - b[2]) * zz + z_offset,
                ];
                if physics::clip(&game.context.world, &game.context.data, from, center).is_none() {
                    hits += 1;
                }
                count += 1;
                zz += zs;
            }
            yy += ys;
        }
        xx += xs;
    }
    if count == 0 {
        0.0
    } else {
        hits as f32 / count as f32
    }
}

/// An explosion about to go off (`Level.explode`).
pub(super) struct Blast {
    pub center: [f64; 3],
    pub radius: f32,
    /// What blew up, which the blast spares: a creeper, primed TNT.
    pub source: Option<i32>,
    /// Who is to blame: the creeper itself, whoever lit the TNT.
    pub attacker: Option<i32>,
    pub breaks_blocks: bool,
    /// Each broken block's loot survives with a chance of one in the
    /// radius: on for mobs (`mob_explosion_drop_decay`), off for TNT.
    pub decay: bool,
}

impl Game {
    /// `ServerExplosion.explode`, then the packet for everyone near.
    pub(super) fn explode(&mut self, blast: Blast) {
        let Blast {
            center,
            radius,
            source,
            attacker,
            breaks_blocks,
            decay,
        } = blast;
        let blocks = self.blast_blocks(center, radius);
        self.blast_entities(center, radius, source, attacker);
        if breaks_blocks {
            self.blow_up(&blocks, radius, decay, attacker);
        }
        // Everyone within 64 blocks sees and hears it.
        let small = radius < 2.0 || !breaks_blocks;
        let data = &self.context.data;
        let particle = |name| data.particle_type(name).unwrap_or(0);
        let explosion = particle(if small {
            "minecraft:explosion"
        } else {
            "minecraft:explosion_emitter"
        });
        let sound = data
            .sound_event("minecraft:entity.generic.explode")
            .unwrap_or(0);
        let block_particles = vec![
            ExplosionParticle {
                particle: particle("minecraft:poof"),
                scaling: 0.5,
                speed: 1.0,
                weight: 1,
            },
            ExplosionParticle {
                particle: particle("minecraft:smoke"),
                scaling: 1.0,
                speed: 1.0,
                weight: 1,
            },
        ];
        let knockbacks = std::mem::take(&mut self.explosion_knockback);
        for player in self.players.values() {
            let d: f64 = (0..3).map(|i| (player.pos[i] - center[i]).powi(2)).sum();
            if d >= 4096.0 {
                continue;
            }
            let knockback = knockbacks
                .iter()
                .find(|(id, _)| *id == player.entity_id)
                .map(|(_, k)| *k);
            player.send(Outbound::Packet(Arc::new(Encoded::new(&Explode {
                center,
                radius,
                block_count: blocks.len() as i32,
                knockback,
                particle: explosion,
                sound: sound + 1,
                block_particles: block_particles.clone(),
                play_sound: true,
            }))));
        }
    }

    /// `calculateExplodedPositions`: 1352 rays from the centre, each with a
    /// power that blocks wear down.
    fn blast_blocks(&self, center: [f64; 3], radius: f32) -> Vec<BlockPos> {
        let data = &self.context.data;
        let world = &self.context.world;
        let mut rng = rand::rng();
        let mut found = HashSet::new();
        for xx in 0..16 {
            for yy in 0..16 {
                for zz in 0..16 {
                    if !(xx == 0 || xx == 15 || yy == 0 || yy == 15 || zz == 0 || zz == 15) {
                        continue;
                    }
                    let mut d = [xx, yy, zz].map(|v| f64::from(v as f32 / 15.0 * 2.0 - 1.0));
                    let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    d = d.map(|v| v / length);
                    let mut power = radius * (0.7 + rng.random::<f32>() * 0.6);
                    let mut p = center;
                    while power > 0.0 {
                        let pos = BlockPos {
                            x: p[0].floor() as i32,
                            y: p[1].floor() as i32,
                            z: p[2].floor() as i32,
                        };
                        if !(MIN_Y..=MAX_Y).contains(&pos.y) {
                            break;
                        }
                        let state = world.block(pos);
                        if state != self.air {
                            // `ExplosionDamageCalculator`: the block's and its
                            // fluid's resistance.
                            let resistance = data
                                .block_of(state)
                                .map_or(0.0, |(_, b)| b.explosion_resistance);
                            power -= (resistance + 0.3) * 0.3;
                        }
                        if power > 0.0 && state != self.air {
                            found.insert(pos);
                        }
                        for i in 0..3 {
                            p[i] += d[i] * 0.3;
                        }
                        power -= 0.225_000_01;
                    }
                }
            }
        }
        found.into_iter().collect()
    }

    /// `hurtEntities`: everything within twice the radius, by distance and
    /// how much the blast sees of it.
    fn blast_entities(
        &mut self,
        center: [f64; 3],
        radius: f32,
        source: Option<i32>,
        attacker: Option<i32>,
    ) {
        let reach = f64::from(radius) * 2.0;
        let source_name = attacker.and_then(|a| self.entity_name(a));
        // `DamageSources.explosion`: someone to blame makes it theirs.
        let damage_type = if attacker.is_some() {
            "minecraft:player_explosion"
        } else {
            "minecraft:explosion"
        };
        let mut targets: Vec<(i32, Aabb, [f64; 3], bool)> = self
            .players
            .values()
            .filter(|p| !p.dead && p.game_mode() != GameMode::Spectator)
            .map(|p| (p.entity_id, p.bounds(), p.eyes(), true))
            .collect();
        for (id, b) in self.mob_boxes() {
            if Some(id) == source {
                continue;
            }
            let eyes = self.mob_eyes(id).unwrap_or([b[0], b[1], b[2]]);
            targets.push((id, b, eyes, false));
        }
        // Primed TNT is thrown about too (from its feet, not its eyes).
        for (id, b) in self.tnt_boxes() {
            if Some(id) != source {
                targets.push((
                    id,
                    b,
                    [(b[0] + b[3]) / 2.0, b[1], (b[2] + b[5]) / 2.0],
                    false,
                ));
            }
        }
        for (id, bounds, eyes, is_player) in targets {
            let feet = [
                (bounds[0] + bounds[3]) / 2.0,
                bounds[1],
                (bounds[2] + bounds[5]) / 2.0,
            ];
            let distance = (0..3)
                .map(|i| (feet[i] - center[i]).powi(2))
                .sum::<f64>()
                .sqrt()
                / reach;
            if distance > 1.0 {
                continue;
            }
            let direction = {
                let d = [
                    eyes[0] - center[0],
                    eyes[1] - center[1],
                    eyes[2] - center[2],
                ];
                let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                if l < 1.0e-4 {
                    [0.0; 3]
                } else {
                    d.map(|v| v / l)
                }
            };
            let seen = exposure(self, center, &bounds);
            // `ExplosionDamageCalculator.getEntityDamageAmount`.
            let impact = (1.0 - distance) * f64::from(seen);
            let damage = ((impact * impact + impact) / 2.0 * 7.0 * reach + 1.0) as f32;
            let knockback = direction.map(|v| v * impact);
            if is_player {
                let Some(player) = self.players.get(&id) else {
                    continue;
                };
                let victim = text::player_name(player.name(), player.uuid());
                let message = match &source_name {
                    Some(name) => text::translate(
                        "death.attack.explosion.player",
                        vec![victim, name.clone()],
                        None,
                    ),
                    None => text::translate("death.attack.explosion", vec![victim], None),
                };
                let flying = player.flying;
                self.hurt_player(
                    id,
                    Hit {
                        amount: damage,
                        damage_type,
                        attacker,
                        direct: source,
                        knockback_from: None,
                        message,
                    },
                );
                // The client moves itself; creative flyers stay put.
                if !flying {
                    self.explosion_knockback.push((id, knockback));
                }
            } else if self.mobs.contains_key(&id) {
                self.hurt_mob_by(id, damage, attacker, None, damage_type);
                self.fling_mob(id, knockback);
            } else {
                self.fling_tnt(id, knockback);
            }
        }
    }

    /// `interactWithBlocks`: the blocks go, in random order, with their
    /// loot (with `decay` each survives with a chance of one in the
    /// radius); TNT among them is lit with a short fuse.
    fn blow_up(&mut self, blocks: &[BlockPos], radius: f32, decay: bool, attacker: Option<i32>) {
        let mut order = blocks.to_vec();
        order.shuffle(&mut rand::rng());
        let mut drops = Vec::new();
        for pos in order {
            let state = self.context.world.block(pos);
            if state == self.air {
                continue;
            }
            let data = Arc::clone(&self.context.data);
            let Some((_, block)) = data.block_of(state) else {
                continue;
            };
            // `TntBlock.wasExploded`: no drop, but a fuse of half a second
            // to a second and a half.
            if block.name == "minecraft:tnt" {
                self.remove_block(pos, None, false);
                let fuse = rand::rng().random_range(0..80 / 4) + 80 / 8;
                self.prime_tnt(pos, attacker, fuse);
                continue;
            }
            {
                let world = &self.context.world;
                let context = loot::Context {
                    data: &data,
                    table: loot::block_table(block),
                    state,
                    pos,
                    tool: None,
                    block_at: &|p| world.block(p),
                    entity: None,
                    by_player: false,
                    explosion: decay.then_some(radius),
                };
                drops.extend(
                    context
                        .drops(&mut rand::rng())
                        .into_iter()
                        .map(|s| (pos, s)),
                );
            }
            // Containers spill, furnaces give up their experience.
            self.remove_block(pos, None, false);
        }
        for (pos, stack) in drops {
            self.pop_resource(pos, stack);
        }
    }
}
