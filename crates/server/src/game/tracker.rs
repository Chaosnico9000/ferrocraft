//! What other clients learn about an entity's movement: vanilla's
//! `ServerEntity.sendChanges`, shared by players and item entities.

use ferrocraft_protocol::packets::play::{
    EntityPositionSync, MoveEntityPos, MoveEntityPosRot, MoveEntityRot, RotateHead, VecDelta,
    pack_degrees,
};

use super::Encoded;

/// Where an entity is and how it is turned, as the game sees it now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub head_yaw: f32,
    pub on_ground: bool,
}

/// What clients last heard about an entity.
#[derive(Debug, Clone)]
pub struct Tracker {
    pub sent_pos: [f64; 3],
    pub sent_yaw: u8,
    pub sent_pitch: u8,
    pub sent_head: u8,
    was_on_ground: bool,
    tick_count: u32,
    teleport_delay: u32,
    /// Ticks between updates (`EntityType.updateInterval`).
    interval: u32,
}

impl Tracker {
    pub fn new(pose: &Pose, interval: u32) -> Self {
        Self {
            sent_pos: pose.pos,
            sent_yaw: pack_degrees(pose.yaw),
            sent_pitch: pack_degrees(pose.pitch),
            sent_head: pack_degrees(pose.head_yaw),
            was_on_ground: pose.on_ground,
            tick_count: 0,
            teleport_delay: 0,
            interval,
        }
    }

    /// Called every tick; returns the movement packets that are due. `force`
    /// makes this tick an update even off the interval, as changed entity
    /// data does in vanilla.
    pub fn sync(&mut self, entity_id: i32, now: &Pose, force: bool) -> Vec<Encoded> {
        let mut packets = Vec::new();
        if self.tick_count.is_multiple_of(self.interval) || force {
            let yaw = pack_degrees(now.yaw);
            let pitch = pack_degrees(now.pitch);
            let rot_changed = yaw != self.sent_yaw || pitch != self.sent_pitch;
            self.teleport_delay += 1;
            let moved = now
                .pos
                .iter()
                .zip(self.sent_pos)
                .map(|(a, b)| (a - b) * (a - b))
                .sum::<f64>()
                >= 7.629_394_5e-6;
            let send_pos = moved || self.tick_count.is_multiple_of(60);
            let delta = VecDelta::between(self.sent_pos, now.pos);
            let (sent_pos, sent_rot) = if self.teleport_delay > 400
                || self.was_on_ground != now.on_ground
                || (send_pos && delta.is_none())
            {
                self.was_on_ground = now.on_ground;
                self.teleport_delay = 0;
                packets.push(Encoded::new(&EntityPositionSync {
                    entity_id,
                    pos: now.pos,
                    yaw: now.yaw,
                    pitch: now.pitch,
                    on_ground: now.on_ground,
                }));
                (true, true)
            } else if let (true, Some(delta)) = (send_pos, delta) {
                // One step over the update interval, as vanilla's
                // interpolation tracker sends it.
                let delta = VecDelta::Stepped(vec![(self.interval as i32, delta)]);
                if rot_changed {
                    packets.push(Encoded::new(&MoveEntityPosRot {
                        entity_id,
                        delta,
                        yaw,
                        pitch,
                        on_ground: now.on_ground,
                    }));
                } else {
                    packets.push(Encoded::new(&MoveEntityPos {
                        entity_id,
                        delta,
                        on_ground: now.on_ground,
                    }));
                }
                (true, rot_changed)
            } else if rot_changed {
                packets.push(Encoded::new(&MoveEntityRot {
                    entity_id,
                    on_ground: now.on_ground,
                    yaw,
                    pitch,
                }));
                (false, true)
            } else {
                (false, false)
            };
            if sent_pos {
                self.sent_pos = now.pos;
            }
            if sent_rot {
                self.sent_yaw = yaw;
                self.sent_pitch = pitch;
            }
            let head = pack_degrees(now.head_yaw);
            if head != self.sent_head {
                self.sent_head = head;
                packets.push(Encoded::new(&RotateHead {
                    entity_id,
                    head_yaw: head,
                }));
            }
        }
        self.tick_count += 1;
        packets
    }
}
