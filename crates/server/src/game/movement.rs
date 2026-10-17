//! Checking where players say they moved
//! (`ServerGamePacketListenerImpl.handlePlayerPositionChange`): clients
//! move themselves, the server only refuses what cannot be. A jump of more
//! than ten blocks a tick, or a move into blocks the player was not in
//! before, is turned back with a teleport; in survival also a move that
//! the blocks would have stopped. Floating in the air for four seconds
//! without being allowed to fly ends the connection. The game rule
//! `player_movement_check` turns the speed check off.

use ferrocraft_protocol::packets::play::GameMode;
use tracing::warn;

use super::{Game, Outbound, physics};
use crate::data::Aabb;

/// `MAX_MOVEMENT_PER_TICK`… squared: at most this many blocks squared a
/// move packet.
const MOVED_TOO_QUICKLY: f64 = 100.0;
/// How far, squared, the blocks may move a player off what the client
/// said before it counts as wrong.
const MOVED_WRONGLY: f64 = 0.0625;
/// How high players step up by themselves (`Attributes.STEP_HEIGHT`).
const STEP_HEIGHT: f64 = 0.6;
/// `getMaximumFlyingTicks` at normal gravity.
const MAX_FLOATING_TICKS: u32 = 80;
/// Coordinates beyond these are nonsense (`clampHorizontal`,
/// `clampVertical`).
const MAX_HORIZONTAL: f64 = 3.0e7;
const MAX_VERTICAL: f64 = 2.0e7;

fn intersects(a: &Aabb, b: &Aabb) -> bool {
    (0..3).all(|i| a[i] < b[i + 3] && a[i + 3] > b[i])
}

impl Game {
    /// Whether a player's move to `target` stands; if not, the player is
    /// sent back or disconnected.
    pub(super) fn check_move(&mut self, entity_id: i32, target: [f64; 3]) -> bool {
        let checking = self.rules.flag("player_movement_check");
        let allow_flight = self.context.allow_flight;
        let Some(player) = self.players.get_mut(&entity_id) else {
            return false;
        };
        let nonsense = target.iter().any(|v| !v.is_finite())
            || target[0].abs() > MAX_HORIZONTAL
            || target[2].abs() > MAX_HORIZONTAL
            || target[1].abs() > MAX_VERTICAL;
        if nonsense {
            player.kick("Ungültige Bewegung");
            return false;
        }
        if player.dead || target == player.pos {
            return true;
        }
        let mode = player.game_mode();
        player.move_packets += 1;
        let packets = if player.move_packets > 5 {
            1
        } else {
            player.move_packets
        };
        let from_first: [f64; 3] = std::array::from_fn(|i| target[i] - player.first_good[i]);
        let (name, pos, old, on_ground) = (
            player.name().to_owned(),
            player.pos,
            player.bounds(),
            player.on_ground,
        );
        let moved_sq: f64 = from_first.iter().map(|d| d * d).sum();
        if checking && moved_sq > MOVED_TOO_QUICKLY * f64::from(packets) {
            warn!(
                "{name} moved too quickly! {},{},{}",
                from_first[0], from_first[1], from_first[2]
            );
            self.send_back(entity_id);
            return false;
        }
        if mode == GameMode::Spectator {
            // `noPhysics`: through anything.
            player.floating = false;
            return true;
        }
        let world = &self.context.world;
        let data = &self.context.data;
        let delta: [f64; 3] = std::array::from_fn(|i| target[i] - pos[i]);
        let moved = physics::collide_stepping(world, data, old, delta, STEP_HEIGHT, on_ground);
        let after: [f64; 3] = std::array::from_fn(|i| pos[i] + moved.movement[i]);
        // Vanilla leaves the height out of this.
        let off = (target[0] - after[0]).powi(2) + (target[2] - after[2]).powi(2);
        let wrong = off > MOVED_WRONGLY && matches!(mode, GameMode::Survival | GameMode::Adventure);
        let was_stuck = !physics::colliding_boxes(world, data, old).is_empty();
        let new: Aabb = std::array::from_fn(|i| old[i] + delta[i % 3]);
        let shrunk_old: Aabb = std::array::from_fn(|i| {
            if i < 3 {
                old[i] + 1.0e-5
            } else {
                old[i] - 1.0e-5
            }
        });
        // `isEntityCollidingWithAnythingNew`.
        let into_new = physics::colliding_boxes(world, data, new)
            .iter()
            .any(|b| !intersects(b, &shrunk_old));
        if (wrong && !was_stuck) || into_new {
            warn!("{name} moved wrongly!");
            self.send_back(entity_id);
            return false;
        }
        // `clientIsFloating`: nothing below or around, and no right to fly.
        let may_fly = matches!(mode, GameMode::Creative | GameMode::Spectator) || allow_flight;
        let around: Aabb = [
            new[0] - 0.0625,
            new[1] - 0.0625 - 0.55,
            new[2] - 0.0625,
            new[3] + 0.0625,
            new[4] + 0.0625,
            new[5] + 0.0625,
        ];
        let levitating = self.has_effect(
            super::effect::Living::Player(entity_id),
            "minecraft:levitation",
        );
        let floating = !moved.on_ground && !may_fly && !levitating && self.only_air_in(around);
        if let Some(player) = self.players.get_mut(&entity_id) {
            player.floating = floating;
        }
        true
    }

    /// `noBlocksAround`.
    fn only_air_in(&self, area: Aabb) -> bool {
        let world = &self.context.world;
        for x in area[0].floor() as i32..=area[3].floor() as i32 {
            for y in area[1].floor() as i32..=area[4].floor() as i32 {
                for z in area[2].floor() as i32..=area[5].floor() as i32 {
                    let state = world.block(ferrocraft_protocol::codec::BlockPos { x, y, z });
                    if !self.is_air(state) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Back to where the server has the player.
    fn send_back(&mut self, entity_id: i32) {
        if let Some(player) = self.players.get(&entity_id) {
            player.send(Outbound::Teleport {
                pos: player.pos,
                rot: [player.yaw, player.pitch],
            });
        }
    }

    /// The part of `tickPlayer` about moving: a new tick starts from where
    /// the player is; floating too long ends the connection.
    pub(super) fn tick_movement(&mut self, entity_id: i32) {
        let Some(player) = self.players.get_mut(&entity_id) else {
            return;
        };
        player.first_good = player.pos;
        player.move_packets = 0;
        if player.floating && !player.dead {
            player.floating_ticks += 1;
            if player.floating_ticks > MAX_FLOATING_TICKS {
                warn!("{} was kicked for floating too long!", player.name());
                player.kick("Fliegen ist auf diesem Server nicht erlaubt");
            }
        } else {
            player.floating = false;
            player.floating_ticks = 0;
        }
    }
}
