//! A player as the game sees it.

use std::sync::Arc;
use std::time::Instant;

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    EntityData, EntityDataValue, GameMode, ItemStack, PlayerAbilities, PlayerInfoEntry,
    Pose as EntityPose, data_index,
};
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

use super::menu::{Drag, Menu};
use super::tracker::{Pose, Tracker};
use super::{EntityState, Outbound};
use crate::data::Aabb;
use crate::inventory::Inventory;
use crate::storage::PlayerData;

/// Height of a standing and a crouching player.
const HEIGHT: f64 = 1.8;
const CROUCHING_HEIGHT: f64 = 1.5;
/// Eye height of a standing and a crouching player.
const EYE_HEIGHT: f64 = 1.62;
const CROUCHING_EYE_HEIGHT: f64 = 1.27;
/// Players send position updates to others every this many ticks.
pub const PLAYER_UPDATE_INTERVAL: u32 = 2;

pub struct Player {
    pub entity_id: i32,
    pub info: PlayerInfoEntry,
    pub outbound: UnboundedSender<Outbound>,
    pub last_keep_alive: Instant,
    pub pending: Option<i64>,
    /// A disconnect is on its way; the connection will report `Left`.
    pub kicked: bool,
    pub latency: i32,
    /// 0 for everyone, 4 for operators.
    pub permission: u8,

    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub sneaking: bool,
    pub sprinting: bool,
    pub flying: bool,
    pub skin_parts: u8,
    pub main_hand: i32,
    pub inventory: Inventory,
    pub tracker: Tracker,
    /// Entity data changed since the last sync.
    pub data_dirty: bool,
    pub state: Option<Arc<EntityState>>,
    /// Counts inventory changes the client heard of (`stateId`).
    pub state_id: i32,
    /// The open screen; `None` while at most the inventory is open.
    pub menu: Option<Menu>,
    /// The inventory screen with its crafting grid.
    pub inventory_menu: Menu,
    /// The stack on the mouse pointer in a screen.
    pub carried: Option<ItemStack>,
    pub drag: Drag,
    /// The ender chest's 27 slots.
    pub ender: Vec<Option<ItemStack>>,
    /// The last window id handed out, 1 to 100.
    pub window: i32,
    /// A block being dug in survival.
    pub digging: Option<Digging>,
    pub health: f32,
    pub food: Food,
    /// How far the player fell since last on the ground.
    pub fall_distance: f64,
    /// Ticks until the player can be hurt again.
    pub invulnerable: u32,
    /// Waiting on the death screen.
    pub dead: bool,
    /// Eating or drinking.
    pub using: Option<Using>,
    /// Game tick of the last swing, from which the next one charges.
    pub last_swing: u64,
    pub experience: Experience,
    /// Ticks until the next experience orb can be taken.
    pub xp_delay: u32,
    /// Game tick of the last fanfare for a new level.
    pub last_level_up: u64,
    /// Ticks left burning (`remainingFireTicks`); below zero, ticks of
    /// fire immunity left.
    pub fire: i32,
    /// Breath left (`airSupply`), 300 when full.
    pub air: i32,
    /// Where the player was when the tick began (`firstGood`), and how
    /// many moves came since.
    pub first_good: [f64; 3],
    pub move_packets: u32,
    /// Hanging in the air without being allowed to fly, and for how many
    /// ticks (`clientIsFloating`, `aboveGroundTickCount`).
    pub floating: bool,
    pub floating_ticks: u32,
    /// Status effects, and the golden hearts absorption gives.
    pub effects: super::effect::Effects,
    pub absorption: f32,
    /// The attributes its client was last told of.
    pub sent_attributes: Vec<ferrocraft_protocol::packets::play::AttributeSnapshot>,
    /// What the enchanting table offers next (`enchantmentSeed`).
    pub enchantment_seed: i32,
}

/// A player's experience: the level, the way to the next as a fraction,
/// all points ever collected since the last death, and the score.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Experience {
    pub level: i32,
    pub progress: f32,
    pub total: i32,
    pub score: i32,
}

/// Slots of the ender chest.
pub const ENDER_SLOTS: usize = 27;

/// `FoodData`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Food {
    pub level: i32,
    pub saturation: f32,
    pub exhaustion: f32,
    /// Ticks towards the next heal or starvation damage.
    pub timer: u32,
}

impl Food {
    /// `FoodData.add`.
    pub fn add(&mut self, nutrition: i32, saturation: f32) {
        self.level = (self.level + nutrition).clamp(0, 20);
        self.saturation = (self.saturation + saturation).clamp(0.0, self.level as f32);
    }

    /// `FoodData.eat`: saturation by its modifier
    /// (`FoodConstants.saturationByModifier`).
    pub fn eat(&mut self, nutrition: i32, modifier: f32) {
        self.add(nutrition, nutrition as f32 * modifier * 2.0);
    }
}

impl Default for Food {
    fn default() -> Self {
        Self {
            level: 20,
            saturation: 5.0,
            exhaustion: 0.0,
            timer: 0,
        }
    }
}

/// An item being used up over time (`LivingEntity.useItem`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Using {
    /// 0 main hand, 1 off hand.
    pub hand: i32,
    /// The inventory menu slot it is in.
    pub slot: usize,
    pub item: i32,
    pub ticks_left: u32,
}

/// `ServerPlayerGameMode`'s destroy progress.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Digging {
    pub pos: BlockPos,
    /// Game tick the digging started.
    pub started: u64,
    /// The crack stage others were last shown, 0–9.
    pub stage: i32,
    /// The client said it finished, but too early; it breaks once the
    /// time is up (`hasDelayedDestroy`).
    pub delayed: bool,
}

impl Player {
    pub fn name(&self) -> &str {
        &self.info.profile.name
    }

    pub fn uuid(&self) -> Uuid {
        self.info.profile.uuid
    }

    pub fn game_mode(&self) -> GameMode {
        self.info.game_mode
    }

    pub fn send(&self, message: Outbound) {
        // A closed channel means the connection is going away and will
        // report `Left`.
        let _ = self.outbound.send(message);
    }

    pub fn kick(&mut self, reason: &str) {
        if !self.kicked {
            self.kicked = true;
            self.send(Outbound::Disconnect(reason.into()));
        }
    }

    pub fn saved(&self, data: &crate::data::GameData) -> PlayerData {
        PlayerData {
            pos: self.pos,
            rot: [self.yaw, self.pitch],
            selected: self.inventory.selected(),
            inventory: self.inventory.stacks(),
            game_mode: self.game_mode() as i32,
            ender: (0..)
                .zip(&self.ender)
                .filter_map(|(slot, stack)| Some((slot, stack.clone()?)))
                .collect(),
            health: self.health,
            xp_level: self.experience.level,
            xp_progress: self.experience.progress,
            xp_total: self.experience.total,
            xp_score: self.experience.score,
            fire: self.fire,
            air: self.air,
            effects: self
                .effects
                .active
                .values()
                .filter_map(|e| e.to_nbt(data))
                .collect(),
            absorption: self.absorption,
            enchantment_seed: self.enchantment_seed,
        }
    }

    pub fn pose(&self) -> Pose {
        Pose {
            pos: self.pos,
            yaw: self.yaw,
            pitch: self.pitch,
            // A player's head turns with its view.
            head_yaw: self.yaw,
            on_ground: self.on_ground,
        }
    }

    pub fn crouching(&self) -> bool {
        self.sneaking && !self.flying
    }

    pub fn eyes(&self) -> [f64; 3] {
        let height = if self.crouching() {
            CROUCHING_EYE_HEIGHT
        } else {
            EYE_HEIGHT
        };
        [self.pos[0], self.pos[1] + height, self.pos[2]]
    }

    /// What the client may do in its game mode (`Abilities`).
    pub fn abilities(&self) -> PlayerAbilities {
        abilities(self.game_mode(), self.flying)
    }

    /// `DATA_LIVING_ENTITY_FLAGS`.
    pub fn living_flags(&self) -> u8 {
        match self.using {
            Some(using) if using.hand == 1 => 0x03,
            Some(_) => 0x01,
            None => 0,
        }
    }

    fn shared_flags(&self) -> u8 {
        let mut flags = u8::from(self.fire > 0);
        if self.sneaking {
            flags |= 0x02;
        }
        if self.sprinting {
            flags |= 0x08;
        }
        if self.effects.look.invisible {
            flags |= 0x20;
        }
        if self.effects.look.glowing {
            flags |= 0x40;
        }
        flags
    }

    /// Synchronized data that differs from the defaults.
    pub fn data(&self) -> Vec<EntityDataValue> {
        let mut data = Vec::new();
        let mut push = |index, value| data.push(EntityDataValue { index, value });
        if self.shared_flags() != 0 {
            push(
                data_index::SHARED_FLAGS,
                EntityData::Byte(self.shared_flags()),
            );
        }
        if self.crouching() {
            push(data_index::POSE, EntityData::Pose(EntityPose::Crouching));
        }
        if self.living_flags() != 0 {
            push(
                data_index::LIVING_ENTITY_FLAGS,
                EntityData::Byte(self.living_flags()),
            );
        }
        if self.air != crate::storage::MAX_AIR {
            push(data_index::AIR_SUPPLY, EntityData::Int(self.air));
        }
        push(data_index::HEALTH, EntityData::Float(self.health));
        for value in self.effects.look.data(false) {
            push(value.index, value.value);
        }
        if self.absorption > 0.0 {
            push(data_index::ABSORPTION, EntityData::Float(self.absorption));
        }
        if self.main_hand != 1 {
            push(
                data_index::MAIN_HAND,
                EntityData::HumanoidArm(self.main_hand),
            );
        }
        if self.skin_parts != 0 {
            push(data_index::SKIN_PARTS, EntityData::Byte(self.skin_parts));
        }
        data
    }

    /// The data values that may have changed, defaults included, so that a
    /// value going back to its default reaches the client too.
    pub fn changeable_data(&self) -> Vec<EntityDataValue> {
        let pose = if self.crouching() {
            EntityPose::Crouching
        } else {
            EntityPose::Standing
        };
        let mut values = vec![
            EntityDataValue {
                index: data_index::SHARED_FLAGS,
                value: EntityData::Byte(self.shared_flags()),
            },
            EntityDataValue {
                index: data_index::POSE,
                value: EntityData::Pose(pose),
            },
            EntityDataValue {
                index: data_index::LIVING_ENTITY_FLAGS,
                value: EntityData::Byte(self.living_flags()),
            },
            EntityDataValue {
                index: data_index::AIR_SUPPLY,
                value: EntityData::Int(self.air),
            },
            EntityDataValue {
                index: data_index::HEALTH,
                value: EntityData::Float(self.health),
            },
            EntityDataValue {
                index: data_index::MAIN_HAND,
                value: EntityData::HumanoidArm(self.main_hand),
            },
            EntityDataValue {
                index: data_index::SKIN_PARTS,
                value: EntityData::Byte(self.skin_parts),
            },
            EntityDataValue {
                index: data_index::ABSORPTION,
                value: EntityData::Float(self.absorption),
            },
        ];
        values.extend(self.effects.look.data(true));
        values.sort_by_key(|v| v.index);
        values
    }

    pub fn state(&mut self, player_type: i32) -> Arc<EntityState> {
        if let Some(state) = &self.state {
            return Arc::clone(state);
        }
        let state = Arc::new(EntityState {
            entity_id: self.entity_id,
            uuid: self.uuid(),
            entity_type: player_type,
            pos: self.tracker.sent_pos,
            yaw: self.tracker.sent_yaw,
            pitch: self.tracker.sent_pitch,
            head_yaw: self.tracker.sent_head,
            velocity: [0.0; 3],
            object_data: 0,
            data: self.data(),
            equipment: self
                .inventory
                .visible()
                .into_iter()
                .filter_map(|(slot, stack)| Some((slot, stack?)))
                .collect(),
            spectator: self.game_mode() == GameMode::Spectator,
            range: super::PLAYER_TRACKING_RANGE,
        });
        self.state = Some(Arc::clone(&state));
        state
    }

    /// The player's bounding box.
    pub fn bounds(&self) -> Aabb {
        let height = if self.crouching() {
            CROUCHING_HEIGHT
        } else {
            HEIGHT
        };
        let [x, y, z] = self.pos;
        [x - 0.3, y, z - 0.3, x + 0.3, y + height, z + 0.3]
    }

    /// Whether a block with this collision shape at `pos` would stick into
    /// the player (`Level.isUnobstructed`). Spectators are never in the way.
    pub fn obstructs(&self, pos: BlockPos, shape: &[Aabb]) -> bool {
        if self.game_mode() == GameMode::Spectator {
            return false;
        }
        let player = self.bounds();
        let origin = [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)];
        shape.iter().any(|b| {
            (0..3).all(|axis| {
                player[axis] < origin[axis] + b[axis + 3]
                    && player[axis + 3] > origin[axis] + b[axis]
            })
        })
    }
}

/// `GameType.updatePlayerAbilities`: 1 invulnerable, 2 flying, 4 may fly,
/// 8 builds instantly.
pub fn abilities(mode: GameMode, flying: bool) -> PlayerAbilities {
    let flags = match mode {
        GameMode::Creative => 1 | 4 | 8 | if flying { 2 } else { 0 },
        GameMode::Spectator => 1 | 2 | 4,
        GameMode::Survival | GameMode::Adventure => 0,
    };
    PlayerAbilities {
        flags,
        flying_speed: 0.05,
        walking_speed: 0.1,
    }
}
