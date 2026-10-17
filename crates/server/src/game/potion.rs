//! Thrown potions (`ThrownSplashPotion`, `ThrownLingeringPotion`): they fly
//! like snowballs and burst where they land. A splash potion gives its
//! effects to everyone within four blocks, less the farther away; a
//! lingering one leaves a cloud (`AreaEffectCloud`) that shrinks for half a
//! minute and gives a quarter of the effect to whoever stands in it. Water
//! puts out fire and those burning.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    EntityData, EntityDataValue, EntityDataValues, ItemStack, LevelEvent, LpVec3, Particle,
    SOUND_SOURCE_NEUTRAL, SOUND_SOURCE_PLAYERS, SetEntityData, SetEntityMotion, Slot,
};
use rand::Rng;
use uuid::Uuid;

use ferrocraft_protocol::component::Registries;

use super::consume::potion_effects;
use super::effect::EffectInstance;
use super::mob::save::{doubles, read_doubles, uuid_from_ints, uuid_from_nbt, uuid_to_nbt};
use super::physics;
use super::tracker::{Pose, Tracker};
use super::{Encoded, EntityState, Game, Outbound};
use crate::data::{Aabb, GameData};
use crate::world::chunk_of;

/// `ThrowableItemProjectile.DATA_ITEM_STACK`, after `Entity`'s 0–7.
const ITEM: u8 = 8;
/// `AreaEffectCloud.DATA_RADIUS`, `DATA_WAITING`, `DATA_PARTICLE`.
const RADIUS: u8 = 8;
const WAITING: u8 = 9;
const PARTICLE: u8 = 10;
const GRAVITY: f64 = 0.05;
const DRAG: f64 = 0.99;
const WATER_DRAG: f64 = 0.8;
const HALF: f64 = 0.125;
const HEIGHT: f64 = 0.25;
/// `PotionContents.BASE_POTION_COLOR`.
const BASE_COLOR: i32 = -13_083_194;
/// `LevelEvent.PARTICLES_SPELL_POTION_SPLASH` and
/// `PARTICLES_INSTANT_POTION_SPLASH`, with their sounds.
const SPLASH: i32 = 2002;
const INSTANT_SPLASH: i32 = 2007;
const SPLASH_SOUND: i32 = 1053;
const INSTANT_SPLASH_SOUND: i32 = 1054;

/// `PotionContents.getColor`: its own colour, or the effects' colours
/// mixed by level.
pub fn potion_color(data: &GameData, contents: &Nbt) -> i32 {
    if let Some(Nbt::Int(color)) = contents.get("custom_color") {
        return *color;
    }
    let (mut r, mut g, mut b, mut total) = (0, 0, 0, 0);
    for effect in potion_effects(data, contents, 1.0)
        .into_iter()
        .filter(|e| e.visible)
    {
        let color = data.alchemy.effects[effect.effect].color;
        let weight = effect.amplifier + 1;
        r += weight * ((color >> 16) & 0xff);
        g += weight * ((color >> 8) & 0xff);
        b += weight * (color & 0xff);
        total += weight;
    }
    if total == 0 {
        BASE_COLOR
    } else {
        (0xff << 24) | ((r / total) << 16) | ((g / total) << 8) | (b / total)
    }
}

pub struct ThrownPotion {
    entity_id: i32,
    uuid: Uuid,
    owner: Option<i32>,
    /// Out of the thrower's box, so that it may hit them too.
    left_owner: bool,
    pos: [f64; 3],
    velocity: [f64; 3],
    stack: ItemStack,
    lingering: bool,
    life: u32,
    tracker: Tracker,
    sent_velocity: [f64; 3],
    state: Option<Arc<EntityState>>,
}

impl ThrownPotion {
    fn bounds(&self) -> Aabb {
        let [x, y, z] = self.pos;
        [x - HALF, y, z - HALF, x + HALF, y + HEIGHT, z + HALF]
    }

    fn pose(&self) -> Pose {
        Pose {
            pos: self.pos,
            yaw: 0.0,
            pitch: 0.0,
            head_yaw: 0.0,
            on_ground: false,
        }
    }

    fn state(&mut self, entity_type: i32, range: f64) -> Arc<EntityState> {
        if let Some(state) = &self.state {
            return Arc::clone(state);
        }
        let state = Arc::new(EntityState {
            entity_id: self.entity_id,
            uuid: self.uuid,
            entity_type,
            pos: self.tracker.sent_pos,
            yaw: self.tracker.sent_yaw,
            pitch: self.tracker.sent_pitch,
            head_yaw: 0,
            velocity: self.sent_velocity,
            object_data: self.owner.unwrap_or(0),
            data: vec![EntityDataValue {
                index: ITEM,
                value: EntityData::Item(Slot(Some(self.stack.clone()))),
            }],
            equipment: Vec::new(),
            spectator: false,
            range,
        });
        self.state = Some(Arc::clone(&state));
        state
    }
}

/// A lingering potion's cloud.
pub struct Cloud {
    entity_id: i32,
    uuid: Uuid,
    owner: Option<i32>,
    pos: [f64; 3],
    radius: f32,
    /// Ticks after the wait; -1 for ever.
    duration: i32,
    wait_time: i32,
    age: i32,
    reapplication_delay: i32,
    duration_on_use: i32,
    radius_on_use: f32,
    radius_per_tick: f32,
    contents: Nbt,
    scale: f32,
    /// Who it gave its effects to, and from when they may get them again.
    victims: HashMap<i32, i32>,
    waiting: bool,
    data_dirty: bool,
    state: Option<Arc<EntityState>>,
}

impl Cloud {
    fn data(&self, data: &GameData) -> Vec<EntityDataValue> {
        let color = if self.contents == Nbt::Compound(Vec::new()) {
            0
        } else {
            potion_color(data, &self.contents)
        };
        let kind = data.particle_type("minecraft:entity_effect").unwrap_or(0);
        vec![
            EntityDataValue {
                index: RADIUS,
                value: EntityData::Float(self.radius),
            },
            EntityDataValue {
                index: WAITING,
                value: EntityData::Boolean(self.waiting),
            },
            EntityDataValue {
                index: PARTICLE,
                value: EntityData::Particle(Particle {
                    kind,
                    color: Some(color | (0xff << 24)),
                }),
            },
        ]
    }

    fn state(&mut self, entity_type: i32, data: &GameData) -> Arc<EntityState> {
        if let Some(state) = &self.state {
            return Arc::clone(state);
        }
        let state = Arc::new(EntityState {
            entity_id: self.entity_id,
            uuid: self.uuid,
            entity_type,
            pos: self.pos,
            yaw: 0,
            pitch: 0,
            head_yaw: 0,
            velocity: [0.0; 3],
            object_data: 0,
            data: self.data(data),
            equipment: Vec::new(),
            spectator: false,
            range: 10.0 * 16.0,
        });
        self.state = Some(Arc::clone(&state));
        state
    }
}

impl Game {
    fn potion_type(&self, lingering: bool) -> i32 {
        let name = if lingering {
            "minecraft:lingering_potion"
        } else {
            "minecraft:splash_potion"
        };
        self.context.data.entity_type(name).unwrap_or(0)
    }

    fn cloud_type(&self) -> i32 {
        self.context
            .data
            .entity_type("minecraft:area_effect_cloud")
            .unwrap_or(0)
    }

    /// `ThrowablePotionItem.use`: thrown from the eyes, a little upwards
    /// (`spawnProjectileFromRotation` with -20° and 0.5 blocks a tick).
    pub(super) fn throw_potion(&mut self, entity_id: i32, slot: usize, stack: &ItemStack) {
        let data = Arc::clone(&self.context.data);
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let lingering = data
            .item(stack.item)
            .is_some_and(|i| i.name == "minecraft:lingering_potion");
        let (yaw, pitch) = (
            f64::from(player.yaw).to_radians(),
            f64::from(player.pitch).to_radians(),
        );
        let lifted = (f64::from(player.pitch) - 20.0).to_radians();
        let direction = [
            -yaw.sin() * pitch.cos(),
            -lifted.sin(),
            yaw.cos() * pitch.cos(),
        ];
        let eyes = player.eyes();
        let from = [eyes[0], eyes[1] - 0.1, eyes[2]];
        let creative = player.game_mode() == ferrocraft_protocol::packets::play::GameMode::Creative;
        let at = player.pos;
        let mut thrown = stack.clone();
        thrown.count = 1;
        self.spawn_potion(
            Some(entity_id),
            from,
            direction,
            0.5,
            1.0,
            thrown,
            lingering,
        );
        let pitch = 0.4 / (rand::rng().random::<f32>() * 0.4 + 0.8);
        let (sound, source) = if lingering {
            (
                "minecraft:entity.lingering_potion.throw",
                SOUND_SOURCE_NEUTRAL,
            )
        } else {
            ("minecraft:entity.splash_potion.throw", SOUND_SOURCE_PLAYERS)
        };
        self.play_sound_from(None, sound, at, source, 0.5, pitch);
        if !creative {
            self.update_inventory(entity_id, |inventory| {
                inventory.take(slot, 1);
            });
            self.send_slots(entity_id, &[slot]);
        }
    }

    /// A potion on its way (`Projectile.shoot`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn spawn_potion(
        &mut self,
        owner: Option<i32>,
        from: [f64; 3],
        direction: [f64; 3],
        speed: f64,
        inaccuracy: f64,
        stack: ItemStack,
        lingering: bool,
    ) {
        let mut rng = rand::rng();
        let length = direction
            .iter()
            .map(|v| v * v)
            .sum::<f64>()
            .sqrt()
            .max(1.0e-6);
        let velocity = direction.map(|v| {
            (v / length + super::mob::triangle(&mut rng, 0.0, 0.017_227_5 * inaccuracy)) * speed
        });
        let entity_id = self.context.entity_ids.fetch_add(1, Ordering::Relaxed);
        let pose = Pose {
            pos: from,
            yaw: 0.0,
            pitch: 0.0,
            head_yaw: 0.0,
            on_ground: false,
        };
        let mut potion = ThrownPotion {
            entity_id,
            uuid: Uuid::new_v4(),
            owner,
            left_owner: owner.is_none(),
            pos: from,
            velocity,
            stack,
            lingering,
            life: 0,
            tracker: Tracker::new(&pose, 10),
            sent_velocity: velocity,
            state: None,
        };
        let state = potion.state(self.potion_type(lingering), 4.0 * 16.0);
        self.potions.insert(entity_id, potion);
        self.broadcast(None, &Outbound::EntityAdded(state));
    }

    pub(super) fn potion_states(&mut self) -> Vec<Arc<EntityState>> {
        let data = Arc::clone(&self.context.data);
        let (splash, lingering, cloud) = (
            self.potion_type(false),
            self.potion_type(true),
            self.cloud_type(),
        );
        let mut states: Vec<_> = self
            .potions
            .values_mut()
            .map(|p| {
                let kind = if p.lingering { lingering } else { splash };
                p.state(kind, 4.0 * 16.0)
            })
            .collect();
        states.extend(self.clouds.values_mut().map(|c| c.state(cloud, &data)));
        states
    }

    pub(super) fn tick_potions(&mut self) {
        let ids: Vec<i32> = self.potions.keys().copied().collect();
        for entity_id in ids {
            self.tick_potion(entity_id);
        }
        let ids: Vec<i32> = self.clouds.keys().copied().collect();
        for entity_id in ids {
            self.tick_cloud(entity_id);
        }
    }

    /// `ThrowableProjectile.tick`: gravity, drag, then the move and what
    /// it hits on the way.
    fn tick_potion(&mut self, entity_id: i32) {
        let data = Arc::clone(&self.context.data);
        let world = Arc::clone(&self.context.world);
        let owner_box = self
            .potions
            .get(&entity_id)
            .and_then(|p| p.owner)
            .and_then(|o| self.players.get(&o).map(super::player::Player::bounds));
        let Some(potion) = self.potions.get_mut(&entity_id) else {
            return;
        };
        if !potion.left_owner {
            let b = potion.bounds();
            potion.left_owner =
                owner_box.is_none_or(|o| !(0..3).all(|i| b[i] < o[i + 3] && b[i + 3] > o[i]));
        }
        potion.velocity[1] -= GRAVITY;
        let in_water = data.physics(world.block(BlockPos {
            x: potion.pos[0].floor() as i32,
            y: potion.pos[1].floor() as i32,
            z: potion.pos[2].floor() as i32,
        }));
        let drag = if in_water.fluid == 1 {
            WATER_DRAG
        } else {
            DRAG
        };
        potion.velocity = potion.velocity.map(|v| v * drag);
        let from = potion.pos;
        let mut to = [
            from[0] + potion.velocity[0],
            from[1] + potion.velocity[1],
            from[2] + potion.velocity[2],
        ];
        let block_hit = physics::clip(&world, &data, from, to);
        if let Some(hit) = &block_hit {
            to = hit.at;
        }
        let excluded = if potion.left_owner {
            None
        } else {
            potion.owner
        };
        let mut victims: Vec<(i32, f64)> = self
            .players
            .values()
            .filter(|p| !p.dead && Some(p.entity_id) != excluded)
            .filter(|p| p.game_mode() != ferrocraft_protocol::packets::play::GameMode::Spectator)
            .filter_map(|p| Some((p.entity_id, physics::clip_box(from, to, &p.bounds())?)))
            .collect();
        victims.extend(
            self.mob_boxes()
                .into_iter()
                .filter(|&(id, _)| Some(id) != excluded)
                .filter_map(|(id, b)| Some((id, physics::clip_box(from, to, &b)?))),
        );
        let victim = victims.into_iter().min_by(|a, b| a.1.total_cmp(&b.1));
        let Some(potion) = self.potions.get_mut(&entity_id) else {
            return;
        };
        if let Some((victim, t)) = victim {
            potion.pos = std::array::from_fn(|i| from[i] + (to[i] - from[i]) * t);
            return self.burst(entity_id, Some(victim), None);
        }
        potion.pos = to;
        potion.life += 1;
        if let Some(hit) = block_hit {
            // The block on the side it came from.
            let mut before = [hit.block.x, hit.block.y, hit.block.z];
            before[hit.axis] -= potion.velocity[hit.axis].signum() as i32;
            let before = BlockPos {
                x: before[0],
                y: before[1],
                z: before[2],
            };
            return self.burst(entity_id, None, Some((hit.block, before)));
        }
        if potion.pos[1] < f64::from(crate::world::MIN_Y - 64) || potion.life > 1200 {
            self.potions.remove(&entity_id);
            self.broadcast(None, &Outbound::EntityRemoved(entity_id));
            return;
        }
        let pose = potion.pose();
        let lingering = potion.lingering;
        let mut packets = potion.tracker.sync(entity_id, &pose, false);
        if !packets.is_empty() {
            potion.sent_velocity = potion.velocity;
            packets.push(Encoded::new(&SetEntityMotion {
                entity_id,
                velocity: LpVec3(potion.velocity),
            }));
            potion.state = None;
            let kind = self.potion_type(lingering);
            let Some(potion) = self.potions.get_mut(&entity_id) else {
                return;
            };
            let message = Outbound::Entity {
                state: potion.state(kind, 4.0 * 16.0),
                packets: packets.into(),
            };
            self.broadcast(None, &message);
        }
    }

    /// `AbstractThrownPotion.onHit`: water douses and puts out, a splash
    /// gives its effects around, a lingering one leaves its cloud.
    /// `block` is the block hit and the one before its face.
    fn burst(&mut self, entity_id: i32, hit: Option<i32>, block: Option<(BlockPos, BlockPos)>) {
        let data = Arc::clone(&self.context.data);
        let Some(potion) = self.potions.remove(&entity_id) else {
            return;
        };
        self.broadcast(None, &Outbound::EntityRemoved(entity_id));
        let contents = crate::stack::get(&data, &potion.stack, "minecraft:potion_contents")
            .unwrap_or(Nbt::Compound(Vec::new()));
        let potion_name = match contents.get("potion") {
            Some(Nbt::String(name)) => Some(name.clone()),
            _ => None,
        };
        let in_tag = |tag: &str| {
            potion_name
                .as_deref()
                .and_then(|n| Registries::id(&*data, "minecraft:potion", n))
                .is_some_and(|id| data.tag_contains("minecraft:potion", tag, id))
        };
        // `onHitBlock`: water puts out fire around where it landed.
        if let Some((pos, effect)) = block
            && in_tag("minecraft:douses_fire")
        {
            let mut around = vec![effect, pos];
            for [dx, dz] in [[0, -1], [0, 1], [-1, 0], [1, 0]] {
                around.push(BlockPos {
                    x: effect.x + dx,
                    y: effect.y,
                    z: effect.z + dz,
                });
            }
            for at in around {
                self.douse(at);
            }
        }
        let bounds = potion.bounds();
        let centre = potion.pos;
        if in_tag("minecraft:extinguishes_entities") {
            for (id, b) in self.living_boxes() {
                if box_distance_sq(&bounds, &b) < 16.0 && self.on_fire(id) {
                    self.extinguish(id);
                }
            }
        }
        let effects = potion_effects(&data, &contents, 1.0);
        if !effects.is_empty() {
            if potion.lingering {
                let at = hit
                    .and_then(|h| {
                        self.players
                            .get(&h)
                            .map(|p| p.pos)
                            .or_else(|| self.mobs.get(&h).map(|m| m.pos))
                    })
                    .unwrap_or(centre);
                let scale =
                    crate::stack::float(&data, &potion.stack, "minecraft:potion_duration_scale")
                        .unwrap_or(0.25);
                self.spawn_cloud(at, potion.owner, contents.clone(), scale);
            } else {
                let scale =
                    crate::stack::float(&data, &potion.stack, "minecraft:potion_duration_scale")
                        .unwrap_or(1.0);
                self.splash(&bounds, &effects, scale, potion.owner);
            }
        }
        let instant = potion_name
            .as_deref()
            .and_then(|n| data.potion(n))
            .is_some_and(|p| {
                p.effects
                    .iter()
                    .any(|e| data.effect(&e.id).is_some_and(|(_, i)| i.instant))
            });
        let (event, sound) = if instant {
            (INSTANT_SPLASH, INSTANT_SPLASH_SOUND)
        } else {
            (SPLASH, SPLASH_SOUND)
        };
        let color = potion_color(&data, &contents);
        let pos = BlockPos {
            x: centre[0].floor() as i32,
            y: centre[1].floor() as i32,
            z: centre[2].floor() as i32,
        };
        for (event, data) in [(event, color), (sound, 0)] {
            let packet = Arc::new(Encoded::new(&LevelEvent {
                event,
                pos,
                data,
                global: false,
            }));
            self.broadcast(
                None,
                &Outbound::InChunk {
                    chunk: chunk_of(pos),
                    packet,
                },
            );
        }
    }

    /// `ThrownSplashPotion.onHitAsPotion`.
    fn splash(
        &mut self,
        bounds: &Aabb,
        effects: &[EffectInstance],
        scale: f32,
        owner: Option<i32>,
    ) {
        let data = Arc::clone(&self.context.data);
        for (id, b) in self.living_boxes() {
            let distance = box_distance_sq(bounds, &b);
            if distance >= 16.0 {
                continue;
            }
            let nearness = 1.0 - distance.sqrt() / 4.0;
            let Some(target) = self.living(id) else {
                continue;
            };
            for effect in effects {
                let info = &data.alchemy.effects[effect.effect];
                if info.instant {
                    let name = info.name.clone();
                    self.apply_instant(target, &name, effect.amplifier, nearness, owner);
                } else {
                    let mut instance = effect.clone();
                    if !instance.infinite() && instance.duration != 0 {
                        instance.duration =
                            (nearness * f64::from(instance.duration) * f64::from(scale) + 0.5)
                                as i32;
                    }
                    if !instance.ends_within(20) {
                        self.add_effect(target, instance);
                    }
                }
            }
        }
    }

    /// Every player and mob that potions reach, with its box.
    fn living_boxes(&self) -> Vec<(i32, Aabb)> {
        let mut boxes: Vec<(i32, Aabb)> = self
            .players
            .values()
            .filter(|p| {
                !p.dead && p.game_mode() != ferrocraft_protocol::packets::play::GameMode::Spectator
            })
            .map(|p| (p.entity_id, p.bounds()))
            .collect();
        boxes.extend(self.mob_boxes());
        boxes
    }

    /// `ThrownLingeringPotion.onHitAsPotion`: a cloud of three blocks that
    /// shrinks away over half a minute.
    fn spawn_cloud(&mut self, pos: [f64; 3], owner: Option<i32>, contents: Nbt, scale: f32) {
        let data = Arc::clone(&self.context.data);
        let entity_id = self.context.entity_ids.fetch_add(1, Ordering::Relaxed);
        let mut cloud = Cloud {
            entity_id,
            uuid: Uuid::new_v4(),
            owner,
            pos,
            radius: 3.0,
            duration: 600,
            wait_time: 10,
            age: 0,
            reapplication_delay: 20,
            duration_on_use: 0,
            radius_on_use: -0.5,
            radius_per_tick: -3.0 / 600.0,
            contents,
            scale,
            victims: HashMap::new(),
            waiting: true,
            data_dirty: false,
            state: None,
        };
        let state = cloud.state(self.cloud_type(), &data);
        self.clouds.insert(entity_id, cloud);
        self.broadcast(None, &Outbound::EntityAdded(state));
    }

    fn remove_cloud(&mut self, entity_id: i32) {
        if self.clouds.remove(&entity_id).is_some() {
            self.broadcast(None, &Outbound::EntityRemoved(entity_id));
        }
    }

    /// `AreaEffectCloud.serverTick`.
    fn tick_cloud(&mut self, entity_id: i32) {
        let data = Arc::clone(&self.context.data);
        let living = self.living_boxes();
        let Some(cloud) = self.clouds.get_mut(&entity_id) else {
            return;
        };
        cloud.age += 1;
        let age = cloud.age;
        if cloud.duration != -1 && age - cloud.wait_time >= cloud.duration {
            return self.remove_cloud(entity_id);
        }
        let waiting = age < cloud.wait_time;
        if waiting != cloud.waiting {
            cloud.waiting = waiting;
            cloud.data_dirty = true;
        }
        let mut to_apply: Vec<(i32, Vec<EffectInstance>)> = Vec::new();
        if !waiting {
            if cloud.radius_per_tick != 0.0 {
                cloud.radius += cloud.radius_per_tick;
                if cloud.radius < 0.5 {
                    return self.remove_cloud(entity_id);
                }
                cloud.data_dirty = true;
            }
            if age % 5 == 0 {
                cloud.victims.retain(|_, until| age < *until);
                let effects = potion_effects(&data, &cloud.contents, cloud.scale);
                if effects.is_empty() {
                    cloud.victims.clear();
                } else {
                    let [x, y, z] = cloud.pos;
                    let r = f64::from(cloud.radius);
                    let area = [x - r, y, z - r, x + r, y + 0.5, z + r];
                    for (id, b) in &living {
                        if cloud.victims.contains_key(id)
                            || !(0..3).all(|i| b[i] < area[i + 3] && b[i + 3] > area[i])
                        {
                            continue;
                        }
                        let (dx, dz) = ((b[0] + b[3]) / 2.0 - x, (b[2] + b[5]) / 2.0 - z);
                        if dx * dx + dz * dz > r * r {
                            continue;
                        }
                        cloud.victims.insert(*id, age + cloud.reapplication_delay);
                        to_apply.push((*id, effects.clone()));
                        if cloud.radius_on_use != 0.0 {
                            cloud.radius += cloud.radius_on_use;
                            cloud.data_dirty = true;
                            if cloud.radius < 0.5 {
                                break;
                            }
                        }
                        if cloud.duration_on_use != 0 && cloud.duration != -1 {
                            cloud.duration += cloud.duration_on_use;
                        }
                    }
                }
            }
        }
        let gone = cloud.radius < 0.5 || (cloud.duration != -1 && cloud.duration <= 0);
        let owner = cloud.owner;
        for (id, effects) in to_apply {
            let Some(target) = self.living(id) else {
                continue;
            };
            for effect in effects {
                let info = &data.alchemy.effects[effect.effect];
                if info.instant {
                    let name = info.name.clone();
                    self.apply_instant(target, &name, effect.amplifier, 0.5, owner);
                } else {
                    self.add_effect(target, effect);
                }
            }
        }
        if gone {
            return self.remove_cloud(entity_id);
        }
        let cloud_type = self.cloud_type();
        let Some(cloud) = self.clouds.get_mut(&entity_id) else {
            return;
        };
        if std::mem::take(&mut cloud.data_dirty) {
            cloud.state = None;
            let packet = Encoded::new(&SetEntityData {
                entity_id,
                values: EntityDataValues(cloud.data(&data)),
            });
            let message = Outbound::Entity {
                state: cloud.state(cloud_type, &data),
                packets: vec![packet].into(),
            };
            self.broadcast(None, &message);
        }
    }

    /// The thrown potions and clouds as vanilla saves them.
    pub(super) fn potion_nbt(&self) -> Vec<([f64; 3], Nbt)> {
        let data = &self.context.data;
        let mut saved: Vec<([f64; 3], Nbt)> = self
            .potions
            .values()
            .filter_map(|p| {
                let mut entries = vec![
                    (
                        "id".to_owned(),
                        Nbt::String(
                            if p.lingering {
                                "minecraft:lingering_potion"
                            } else {
                                "minecraft:splash_potion"
                            }
                            .into(),
                        ),
                    ),
                    ("Pos".into(), doubles(&p.pos)),
                    ("Motion".into(), doubles(&p.velocity)),
                    ("UUID".into(), uuid_to_nbt(p.uuid)),
                    ("HasBeenShot".into(), Nbt::Byte(1)),
                    ("LeftOwner".into(), Nbt::Byte(i8::from(p.left_owner))),
                    ("Item".into(), crate::storage::item_to_nbt(&p.stack, data)?),
                ];
                if let Some(owner) = p.owner.and_then(|o| self.uuid_of(o)) {
                    entries.push(("Owner".into(), uuid_to_nbt(owner)));
                }
                Some((p.pos, Nbt::Compound(entries)))
            })
            .collect();
        saved.extend(self.clouds.values().map(|c| {
            let mut entries = vec![
                (
                    "id".to_owned(),
                    Nbt::String("minecraft:area_effect_cloud".into()),
                ),
                ("Pos".into(), doubles(&c.pos)),
                ("UUID".into(), uuid_to_nbt(c.uuid)),
                ("Age".into(), Nbt::Int(c.age)),
                ("Duration".into(), Nbt::Int(c.duration)),
                ("WaitTime".into(), Nbt::Int(c.wait_time)),
                ("ReapplicationDelay".into(), Nbt::Int(c.reapplication_delay)),
                ("DurationOnUse".into(), Nbt::Int(c.duration_on_use)),
                ("RadiusOnUse".into(), Nbt::Float(c.radius_on_use)),
                ("RadiusPerTick".into(), Nbt::Float(c.radius_per_tick)),
                ("Radius".into(), Nbt::Float(c.radius)),
            ];
            if c.contents != Nbt::Compound(Vec::new()) {
                entries.push(("potion_contents".into(), c.contents.clone()));
            }
            if c.scale != 1.0 {
                entries.push(("potion_duration_scale".into(), Nbt::Float(c.scale)));
            }
            if let Some(owner) = c.owner.and_then(|o| self.uuid_of(o)) {
                entries.push(("Owner".into(), uuid_to_nbt(owner)));
            }
            (c.pos, Nbt::Compound(entries))
        }));
        saved
    }

    /// A thrown potion or cloud from the entity store.
    pub(super) fn revive_potion(&mut self, entity_id: i32, nbt: &Nbt) {
        let data = Arc::clone(&self.context.data);
        let Some(pos) = read_doubles(nbt, "Pos") else {
            return;
        };
        let owner = nbt
            .get("Owner")
            .and_then(uuid_from_ints)
            .and_then(|u| self.entity_of(u));
        let uuid = uuid_from_nbt(nbt).unwrap_or_else(Uuid::new_v4);
        let int = |key: &str, default: i32| match nbt.get(key) {
            Some(Nbt::Int(v)) => *v,
            _ => default,
        };
        let float = |key: &str, default: f32| match nbt.get(key) {
            Some(Nbt::Float(v)) => *v,
            _ => default,
        };
        match nbt.get("id") {
            Some(Nbt::String(id)) if id == "minecraft:area_effect_cloud" => {
                let mut cloud = Cloud {
                    entity_id,
                    uuid,
                    owner,
                    pos,
                    radius: float("Radius", 3.0),
                    duration: int("Duration", -1),
                    wait_time: int("WaitTime", 20),
                    age: int("Age", 0),
                    reapplication_delay: int("ReapplicationDelay", 20),
                    duration_on_use: int("DurationOnUse", 0),
                    radius_on_use: float("RadiusOnUse", 0.0),
                    radius_per_tick: float("RadiusPerTick", 0.0),
                    contents: match nbt.get("potion_contents") {
                        Some(Nbt::String(name)) => {
                            Nbt::Compound(vec![("potion".into(), Nbt::String(name.clone()))])
                        }
                        Some(contents) => contents.clone(),
                        None => Nbt::Compound(Vec::new()),
                    },
                    scale: float("potion_duration_scale", 1.0),
                    victims: HashMap::new(),
                    waiting: false,
                    data_dirty: false,
                    state: None,
                };
                let state = cloud.state(self.cloud_type(), &data);
                self.clouds.insert(entity_id, cloud);
                self.broadcast(None, &Outbound::EntityAdded(state));
            }
            Some(Nbt::String(id)) => {
                let lingering = id == "minecraft:lingering_potion";
                let Some(stack) = nbt
                    .get("Item")
                    .and_then(|i| crate::storage::item_from_nbt(i, &data))
                else {
                    return;
                };
                let velocity = read_doubles(nbt, "Motion").unwrap_or_default();
                let pose = Pose {
                    pos,
                    yaw: 0.0,
                    pitch: 0.0,
                    head_yaw: 0.0,
                    on_ground: false,
                };
                let mut potion = ThrownPotion {
                    entity_id,
                    uuid,
                    owner,
                    left_owner: matches!(nbt.get("LeftOwner"), Some(Nbt::Byte(1))),
                    pos,
                    velocity,
                    stack,
                    lingering,
                    life: 0,
                    tracker: Tracker::new(&pose, 10),
                    sent_velocity: velocity,
                    state: None,
                };
                let state = potion.state(self.potion_type(lingering), 4.0 * 16.0);
                self.potions.insert(entity_id, potion);
                self.broadcast(None, &Outbound::EntityAdded(state));
            }
            _ => {}
        }
    }
}

impl Game {
    /// `AbstractThrownPotion.douseFire`: fire goes out, candles and
    /// campfires too.
    fn douse(&mut self, pos: BlockPos) {
        let data = Arc::clone(&self.context.data);
        let state = self.context.world.block(pos);
        let Some((_, block)) = data.block_of(state) else {
            return;
        };
        if data.state_in_tag(state, "minecraft:fire") {
            self.destroy_block(pos, false);
        } else if (block.name.ends_with("candle") || block.name.ends_with("candle_cake"))
            && block.value(state, "lit") == Some("true")
        {
            if let Some(out) = block.with(state, "lit", "false") {
                self.set_block(pos, out);
                let at = [
                    f64::from(pos.x) + 0.5,
                    f64::from(pos.y) + 0.5,
                    f64::from(pos.z) + 0.5,
                ];
                self.play_sound_from(
                    None,
                    "minecraft:block.candle.extinguish",
                    at,
                    ferrocraft_protocol::packets::play::SOUND_SOURCE_BLOCKS,
                    1.0,
                    1.0,
                );
            }
        } else if block.name.ends_with("campfire") && block.value(state, "lit") == Some("true") {
            self.level_event(1009, pos, 0);
            if let Some(out) = block.with(state, "lit", "false") {
                self.set_block(pos, out);
            }
        }
    }

    /// `Entity.extinguishFire`.
    pub(super) fn extinguish(&mut self, entity_id: i32) {
        if let Some(player) = self.players.get_mut(&entity_id)
            && player.fire > 0
        {
            player.fire = 0;
            player.data_dirty = true;
            player.state = None;
            self.send_own_data(entity_id);
        } else if let Some(mob) = self.mobs.get_mut(&entity_id)
            && mob.fire > 0
        {
            mob.fire = 0;
            mob.mark_dirty();
        }
    }
}

/// The squared distance between two boxes (`AABB.distanceToSqr`), the
/// target grown by the projectile margin of thrown items, none.
fn box_distance_sq(a: &Aabb, b: &Aabb) -> f64 {
    (0..3)
        .map(|i| {
            let d = (a[i] - b[i + 3]).max(b[i] - a[i + 3]).max(0.0);
            d * d
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn potion_colors_like_vanilla() {
        let data = GameData::load().unwrap();
        let potion = |name: &str| Nbt::Compound(vec![("potion".into(), Nbt::String(name.into()))]);
        // One effect: its own colour, opaque.
        let (_, speed) = data.effect("minecraft:speed").unwrap();
        assert_eq!(
            potion_color(&data, &potion("minecraft:swiftness")),
            speed.color | (0xff << 24)
        );
        assert_eq!(potion_color(&data, &potion("minecraft:water")), BASE_COLOR);
        // Turtle master: slowness IV and resistance III, mixed 4:3.
        let (_, slow) = data.effect("minecraft:slowness").unwrap();
        let (_, res) = data.effect("minecraft:resistance").unwrap();
        let mix = |shift: i32| {
            (4 * ((slow.color >> shift) & 0xff) + 3 * ((res.color >> shift) & 0xff)) / 7
        };
        assert_eq!(
            potion_color(&data, &potion("minecraft:turtle_master")),
            (0xff << 24) | (mix(16) << 16) | (mix(8) << 8) | mix(0)
        );
        assert_eq!(
            box_distance_sq(&[0.0; 6], &[2.0, 0.0, 0.0, 3.0, 1.0, 1.0]),
            4.0
        );
    }
}
