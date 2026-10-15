//! Play: puts the player into the world, keeps the chunks around them sent,
//! shows them the entities near them and passes what they do on to the
//! game, which decides what it means.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use anyhow::bail;
use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::frame::RawPacket;
use ferrocraft_protocol::packets::Packet;
use ferrocraft_protocol::packets::login::GameProfile;
use ferrocraft_protocol::packets::play::{
    AcceptTeleportation, AddEntity, Attack, BlockChangedAck, BlockUpdate, BundleDelimiter,
    CLIENT_COMMAND_RESPAWN, ChangeGameMode, Chat, ChatCommand, ChunkBatchFinished,
    ChunkBatchReceived, ChunkBatchStart, ClientCommand, ClientInformation, ClientTickEnd,
    ClientboundKeepAlive, ContainerClick, ContainerSetContent, Disconnect, EntityData,
    EntityDataValue, EntityDataValues, EntityEvent, EntityIds, ForgetLevelChunk, GameEvent,
    GameEventKind, GameMode, Interact, Login, LpVec3, MovePlayer, PERMISSION_LEVEL_EVENT,
    PickItemFromBlock, PlayerAction, PlayerCommand, PlayerInfoEntry, PlayerInfoRemove,
    PlayerInfoUpdate, PlayerInput, PlayerLoaded, PlayerPosition, Punch, RemoveEntities, Respawn,
    ServerboundContainerClose, ServerboundKeepAlive, ServerboundPlayerAbilities, SetCarriedItem,
    SetChunkCacheCenter, SetCreativeModeSlot, SetDefaultSpawnPosition, SetEntityData, SetEquipment,
    SetHeldSlot, SignUpdate, Slot, Slots, SpawnInfo, UseItem, UseItemOn, data_index,
    player_command,
};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tracing::debug;

use crate::Server;
use crate::configuration::ClientSettings;
use crate::connection::Connection;
use crate::game::{self, EntityState, Event, Joined, Outbound};
use crate::inventory::Inventory;
use crate::storage::MAX_HEALTH;
use crate::world::{self, ChunkPos, chunk_of};

const SPAWN_TELEPORT: i32 = 1;
/// Vanilla clamps positions to these before using them.
const MAX_HORIZONTAL: f64 = 3.0e7;
const MAX_VERTICAL: f64 = 2.0e7;

/// Counts the player as online for as long as it lives.
struct Presence<'a> {
    server: &'a Server,
    entity_id: i32,
}

impl Drop for Presence<'_> {
    fn drop(&mut self) {
        self.server.online_players.fetch_sub(1, Ordering::Relaxed);
        self.server.tick_event(Event::Left {
            entity_id: self.entity_id,
        });
    }
}

/// The chunks a client has, and around which chunk.
struct ChunkView {
    center: ChunkPos,
    radius: i32,
    loaded: HashSet<ChunkPos>,
}

impl ChunkView {
    /// `ChunkTrackingView.contains`, which decides what gets sent: a circle,
    /// widened by two chunks so that the client can render the edge.
    fn contains(&self, x: i32, z: i32) -> bool {
        let dx = i64::from(((x - self.center.0).abs() - 2).max(0));
        let dz = i64::from(((z - self.center.1).abs() - 2).max(0));
        dx * dx + dz * dz < i64::from(self.radius) * i64::from(self.radius)
    }

    /// Chunks in view that the client does not have yet, nearest first.
    fn missing(&self) -> Vec<ChunkPos> {
        let (cx, cz) = self.center;
        let r = self.radius + 2;
        let mut chunks: Vec<_> = (cx - r..=cx + r)
            .flat_map(|x| (cz - r..=cz + r).map(move |z| (x, z)))
            .filter(|&(x, z)| self.contains(x, z) && !self.loaded.contains(&(x, z)))
            .collect();
        chunks.sort_by_key(|&(x, z)| (x - cx).pow(2) + (z - cz).pow(2));
        chunks
    }
}

/// Another entity as this client knows it.
struct Other {
    state: Arc<EntityState>,
    /// Spawned on this client.
    tracked: bool,
}

/// One player's connection in play.
struct Session<'a> {
    server: &'a Server,
    conn: &'a mut Connection,
    entity_id: i32,
    view: ChunkView,
    pos: [f64; 3],
    game_mode: GameMode,
    sneaking: bool,
    permission: u8,
    skin_parts: u8,
    awaiting_teleport: Option<i32>,
    /// The id of the last teleport sent.
    teleports: i32,
    others: HashMap<i32, Other>,
}

pub async fn handle(
    server: &Server,
    conn: &mut Connection,
    profile: GameProfile,
    settings: ClientSettings,
) -> anyhow::Result<()> {
    let entity_id = server.next_entity_id();
    let (outbound, mut inbound) = unbounded_channel();
    server.online_players.fetch_add(1, Ordering::Relaxed);
    // From here on, however this ends, the player counts as gone again.
    let _presence = Presence { server, entity_id };
    let information = settings.information;
    let saved = match &server.storage {
        Some(storage) => storage.load_player(profile.uuid, &server.data)?,
        None => None,
    };
    let (ender, health) = saved
        .as_ref()
        .map_or((Vec::new(), MAX_HEALTH), |s| (s.ender.clone(), s.health));
    let (fire, air) = saved
        .as_ref()
        .map_or((0, crate::storage::MAX_AIR), |s| (s.fire, s.air));
    let experience = saved
        .as_ref()
        .map_or_else(game::Experience::default, |s| game::Experience {
            level: s.xp_level,
            progress: s.xp_progress,
            total: s.xp_total,
            score: s.xp_score,
        });
    let (pos, rot, inventory, game_mode) = match saved {
        Some(saved) => (
            saved.pos,
            saved.rot,
            Inventory::new(&saved.inventory, saved.selected),
            GameMode::from_id(saved.game_mode).unwrap_or(server.default_game_mode),
        ),
        None => (
            [
                f64::from(server.spawn.x) + 0.5,
                f64::from(server.spawn.y),
                f64::from(server.spawn.z) + 0.5,
            ],
            server.spawn_angle,
            Inventory::default(),
            server.default_game_mode,
        ),
    };
    let permission = server.permission(&profile.name);
    server.tick_event(Event::Joined(Box::new(Joined {
        entity_id,
        profile: profile.clone(),
        game_mode,
        permission,
        pos,
        rot,
        inventory: inventory.clone(),
        ender,
        health,
        experience,
        fire,
        air,
        skin_parts: information.as_ref().map_or(0, |i| i.model_customisation),
        main_hand: information.as_ref().map_or(1, |i| i.main_hand),
        outbound,
    })));

    let requested = information
        .as_ref()
        .map_or(server.config.view_distance as i32, |i| {
            i32::from(i.view_distance)
        });
    let mut session = Session {
        server,
        conn,
        entity_id,
        view: ChunkView {
            center: chunk_at(pos),
            radius: view_radius(server, requested),
            loaded: HashSet::new(),
        },
        pos,
        game_mode,
        sneaking: false,
        permission,
        skin_parts: information.as_ref().map_or(0, |i| i.model_customisation),
        awaiting_teleport: Some(SPAWN_TELEPORT),
        teleports: SPAWN_TELEPORT,
        others: HashMap::new(),
    };
    session
        .join(
            profile,
            information.map_or(0, |i| i.model_customisation),
            rot,
            &inventory,
            permission,
        )
        .await?;
    session.send_chunks().await?;

    loop {
        match next(session.conn, &mut inbound).await? {
            Next::Packet(raw) => session.on_packet(&raw).await?,
            Next::Outbound(Outbound::Disconnect(reason)) => {
                session
                    .conn
                    .send(&Disconnect {
                        reason: Nbt::text(&reason),
                    })
                    .await?;
                return Ok(());
            }
            Next::Outbound(message) => session.on_outbound(message).await?,
            Next::Closed => return Ok(()),
        }
    }
}

fn chunk_at(pos: [f64; 3]) -> ChunkPos {
    ((pos[0].floor() as i32) >> 4, (pos[2].floor() as i32) >> 4)
}

/// Vanilla clamps the client's wish between 2 and the server's distance.
fn view_radius(server: &Server, requested: i32) -> i32 {
    requested.clamp(2, server.config.view_distance.max(2) as i32)
}

impl Session<'_> {
    fn event(&self, event: Event) {
        self.server.tick_event(event);
    }

    /// `ServerPlayer.createCommonSpawnInfo`.
    fn spawn_info(&self) -> SpawnInfo {
        SpawnInfo {
            dimension_type: self.server.world.dimension_type,
            dimension: world::DIMENSION.into(),
            hashed_seed: self.server.hashed_seed,
            game_mode: self.game_mode,
            previous_game_mode: None,
            debug: false,
            flat: true,
            death_location: None,
            portal_cooldown: 0,
            sea_level: world::SEA_LEVEL,
        }
    }

    /// The packets vanilla's `PlayerList.placeNewPlayer` sends that a client
    /// needs to show the world: dimension, abilities, its own tab list entry
    /// (where the client reads its game mode from), spawn, position, then
    /// the signal that chunks are coming.
    async fn join(
        &mut self,
        profile: GameProfile,
        skin_parts: u8,
        [yaw, pitch]: [f32; 2],
        inventory: &Inventory,
        permission: u8,
    ) -> anyhow::Result<()> {
        let server = self.server;
        self.conn
            .send(&Login {
                entity_id: self.entity_id,
                hardcore: false,
                dimensions: vec![world::DIMENSION.into()],
                max_players: server.config.max_players as i32,
                view_distance: server.config.view_distance as i32,
                simulation_distance: server.config.view_distance as i32,
                reduced_debug_info: false,
                show_death_screen: true,
                limited_crafting: false,
                spawn: self.spawn_info(),
                online_mode: server.config.online_mode,
                enforces_secure_chat: false,
            })
            .await?;
        self.conn
            .send(&game::abilities(self.game_mode, false))
            .await?;
        // What the player may do: F3+F4 and the commands it can complete.
        self.conn
            .send(&EntityEvent {
                entity_id: self.entity_id,
                event: PERMISSION_LEVEL_EVENT + permission,
            })
            .await?;
        self.conn
            .send(&game::command_tree(&server.data, permission))
            .await?;
        self.conn
            .send(&PlayerInfoUpdate {
                entries: vec![PlayerInfoEntry {
                    profile,
                    game_mode: self.game_mode,
                    listed: true,
                    latency: 0,
                    list_order: 0,
                    show_hat: true,
                }],
            })
            .await?;
        // The own skin layers, for third person view.
        self.conn
            .send(&SetEntityData {
                entity_id: self.entity_id,
                values: EntityDataValues(vec![EntityDataValue {
                    index: data_index::SKIN_PARTS,
                    value: EntityData::Byte(skin_parts),
                }]),
            })
            .await?;
        self.conn
            .send(&SetDefaultSpawnPosition {
                dimension: world::DIMENSION.into(),
                pos: server.spawn,
                yaw: server.spawn_angle[0],
                pitch: server.spawn_angle[1],
            })
            .await?;
        self.conn
            .send(&PlayerPosition {
                teleport_id: SPAWN_TELEPORT,
                pos: self.pos,
                delta: [0.0; 3],
                yaw,
                pitch,
                relatives: 0,
            })
            .await?;
        // The inventory from last time, as vanilla's inventory menu sends it.
        self.conn
            .send(&ContainerSetContent {
                container_id: 0,
                state_id: 0,
                items: Slots(
                    inventory
                        .slots()
                        .iter()
                        .map(|stack| Slot(stack.clone()))
                        .collect(),
                ),
                carried: Slot(None),
            })
            .await?;
        self.conn
            .send(&SetHeldSlot {
                slot: inventory.selected() as i32,
            })
            .await?;
        self.conn
            .send(&GameEvent::new(GameEventKind::LevelChunksLoadStart, 0.0))
            .await?;
        Ok(())
    }

    /// Sends the chunks in view that the client lacks as one batch.
    async fn send_chunks(&mut self) -> anyhow::Result<()> {
        self.conn
            .send(&SetChunkCacheCenter {
                x: self.view.center.0,
                z: self.view.center.1,
            })
            .await?;
        let missing = self.view.missing();
        if missing.is_empty() {
            return Ok(());
        }
        self.conn.send(&ChunkBatchStart).await?;
        for &(x, z) in &missing {
            self.conn.send(&self.server.world.chunk(x, z)).await?;
            self.view.loaded.insert((x, z));
        }
        self.conn
            .send(&ChunkBatchFinished {
                batch_size: missing.len() as i32,
            })
            .await
    }

    /// Re-centres the view, forgets the chunks that left it and sends the
    /// new ones.
    async fn update_view(&mut self, center: ChunkPos, radius: i32) -> anyhow::Result<()> {
        if (center, radius) == (self.view.center, self.view.radius) {
            return Ok(());
        }
        self.view.center = center;
        self.view.radius = radius;
        let gone: Vec<_> = self
            .view
            .loaded
            .iter()
            .copied()
            .filter(|&(x, z)| !self.view.contains(x, z))
            .collect();
        for (x, z) in gone {
            self.view.loaded.remove(&(x, z));
            self.conn.send(&ForgetLevelChunk { x, z }).await?;
        }
        self.send_chunks().await
    }

    async fn on_packet(&mut self, raw: &RawPacket) -> anyhow::Result<()> {
        let entity_id = self.entity_id;
        match raw.id {
            ServerboundKeepAlive::ID => {
                let id = ServerboundKeepAlive::from_raw(raw)?.id;
                self.event(Event::KeepAlive { entity_id, id });
            }
            AcceptTeleportation::ID => {
                let accepted = AcceptTeleportation::from_raw(raw)?;
                if self.awaiting_teleport == Some(accepted.teleport_id) {
                    self.awaiting_teleport = None;
                    self.moved(
                        Some(accepted.pos),
                        Some([accepted.yaw, accepted.pitch]),
                        true,
                    )
                    .await?;
                }
            }
            PlayerInput::ID => {
                let sneaking = PlayerInput::from_raw(raw)?.flags & 0x20 != 0;
                if sneaking != self.sneaking {
                    self.sneaking = sneaking;
                    self.event(Event::Sneaking {
                        entity_id,
                        sneaking,
                    });
                }
            }
            PlayerCommand::ID => {
                let command = PlayerCommand::from_raw(raw)?;
                let sprinting = match command.action {
                    player_command::START_SPRINTING => true,
                    player_command::STOP_SPRINTING => false,
                    _ => return Ok(()),
                };
                self.event(Event::Sprinting {
                    entity_id,
                    sprinting,
                });
            }
            ServerboundPlayerAbilities::ID => {
                let flying = ServerboundPlayerAbilities::from_raw(raw)?.flags & 0x02 != 0;
                self.event(Event::Flying { entity_id, flying });
            }
            Punch::ID => self.event(Event::Swing { entity_id }),
            ClientInformation::ID => {
                let info = ClientInformation::from_raw(raw)?.0;
                self.skin_parts = info.model_customisation;
                self.event(Event::Settings {
                    entity_id,
                    skin_parts: info.model_customisation,
                    main_hand: info.main_hand,
                });
                let radius = view_radius(self.server, i32::from(info.view_distance));
                self.update_view(self.view.center, radius).await?;
                self.update_tracking().await?;
            }
            SetCarriedItem::ID => {
                let slot = SetCarriedItem::from_raw(raw)?.slot;
                self.event(Event::SelectSlot { entity_id, slot });
            }
            SetCreativeModeSlot::ID => {
                let packet = SetCreativeModeSlot::from_raw(raw)?;
                self.event(Event::CreativeSlot {
                    entity_id,
                    slot: packet.slot,
                    stack: packet.stack.0,
                });
            }
            UseItemOn::ID => {
                let click = UseItemOn::from_raw(raw)?;
                self.event(Event::UseItemOn { entity_id, click });
            }
            PlayerAction::ID => {
                let action = PlayerAction::from_raw(raw)?;
                self.event(Event::Action { entity_id, action });
            }
            ChangeGameMode::ID => {
                let mode = ChangeGameMode::from_raw(raw)?.mode;
                self.event(Event::ChangeGameMode { entity_id, mode });
            }
            Chat::ID => {
                let message = Chat::from_raw(raw)?.message;
                self.event(Event::Chat { entity_id, message });
            }
            ChatCommand::ID => {
                let command = ChatCommand::from_raw(raw)?.command;
                self.event(Event::Command { entity_id, command });
            }
            ContainerClick::ID => {
                let click = ContainerClick::from_raw(raw)?;
                self.event(Event::ContainerClick { entity_id, click });
            }
            ServerboundContainerClose::ID => {
                let container_id = ServerboundContainerClose::from_raw(raw)?.container_id;
                self.event(Event::ContainerClose {
                    entity_id,
                    container_id,
                });
            }
            SignUpdate::ID => {
                let update = SignUpdate::from_raw(raw)?;
                self.event(Event::SignUpdate { entity_id, update });
            }
            PickItemFromBlock::ID => {
                let pick = PickItemFromBlock::from_raw(raw)?;
                self.event(Event::PickBlock {
                    entity_id,
                    pos: pick.pos,
                    include_data: pick.include_data,
                });
            }
            UseItem::ID => {
                let use_item = UseItem::from_raw(raw)?;
                self.event(Event::UseItem {
                    entity_id,
                    hand: use_item.hand,
                    sequence: use_item.sequence,
                    rot: [use_item.yaw, use_item.pitch],
                });
            }
            Attack::ID => {
                let target = Attack::from_raw(raw)?.entity_id;
                self.event(Event::Attack { entity_id, target });
            }
            Interact::ID => {
                let interact = Interact::from_raw(raw)?;
                self.event(Event::Interact {
                    entity_id,
                    target: interact.entity_id,
                    hand: interact.hand,
                });
            }
            ClientCommand::ID => {
                if ClientCommand::from_raw(raw)?.action == CLIENT_COMMAND_RESPAWN {
                    self.event(Event::Respawn { entity_id });
                }
            }
            // Acknowledgements that need no answer.
            ChunkBatchReceived::ID | ClientTickEnd::ID | PlayerLoaded::ID => {}
            id => match MovePlayer::decode_variant(id, &raw.body) {
                Some(movement) => {
                    let movement = movement?;
                    // Like vanilla, movement before the teleport is confirmed
                    // is from before the teleport and does not count.
                    if self.awaiting_teleport.is_none() {
                        self.moved(movement.pos, movement.rot, movement.flags & 1 != 0)
                            .await?;
                    }
                }
                None => debug!("ignoring play packet 0x{id:02x}"),
            },
        }
        Ok(())
    }

    async fn moved(
        &mut self,
        pos: Option<[f64; 3]>,
        rot: Option<[f32; 2]>,
        on_ground: bool,
    ) -> anyhow::Result<()> {
        let pos = match pos {
            Some(pos) if pos.iter().any(|v| !v.is_finite()) => bail!("invalid position {pos:?}"),
            Some([x, y, z]) => Some([
                x.clamp(-MAX_HORIZONTAL, MAX_HORIZONTAL),
                y.clamp(-MAX_VERTICAL, MAX_VERTICAL),
                z.clamp(-MAX_HORIZONTAL, MAX_HORIZONTAL),
            ]),
            None => None,
        };
        if let Some(rot) = rot
            && rot.iter().any(|v| !v.is_finite())
        {
            bail!("invalid rotation {rot:?}");
        }
        self.event(Event::Moved {
            entity_id: self.entity_id,
            pos,
            rot,
            on_ground,
        });
        if let Some(pos) = pos {
            self.pos = pos;
            self.update_view(chunk_at(pos), self.view.radius).await?;
            self.update_tracking().await?;
        }
        Ok(())
    }

    async fn on_outbound(&mut self, message: Outbound) -> anyhow::Result<()> {
        match message {
            Outbound::KeepAlive(id) => self.conn.send(&ClientboundKeepAlive { id }).await?,
            Outbound::Disconnect(_) => unreachable!("handled by the caller"),
            Outbound::Packet(packet) => self.conn.send_raw(packet.id, &packet.body).await?,
            Outbound::PlayerAdded { info, state } => {
                self.conn
                    .send(&PlayerInfoUpdate {
                        entries: vec![*info],
                    })
                    .await?;
                let entity_id = state.entity_id;
                self.others.insert(
                    entity_id,
                    Other {
                        state,
                        tracked: false,
                    },
                );
                self.track(entity_id).await?;
            }
            Outbound::PlayerRemoved { entity_id, uuid } => {
                if let Some(other) = self.others.remove(&entity_id)
                    && other.tracked
                {
                    self.conn
                        .send(&RemoveEntities {
                            entity_ids: EntityIds(vec![entity_id]),
                        })
                        .await?;
                }
                self.conn
                    .send(&PlayerInfoRemove { uuids: vec![uuid] })
                    .await?;
            }
            Outbound::Entity { state, packets } => {
                let entity_id = state.entity_id;
                let Some(other) = self.others.get_mut(&entity_id) else {
                    return Ok(());
                };
                other.state = state;
                let tracked = other.tracked;
                if tracked && self.sees(&self.others[&entity_id].state) {
                    for packet in packets.iter() {
                        self.conn.send_raw(packet.id, &packet.body).await?;
                    }
                } else {
                    self.track(entity_id).await?;
                }
            }
            Outbound::BlockChanged { pos, state } => {
                // Chunks the client does not have yet are read after the
                // change, so they already show it.
                if self.view.loaded.contains(&chunk_of(pos)) {
                    self.conn
                        .send(&BlockUpdate {
                            pos,
                            state: state as i32,
                        })
                        .await?;
                }
            }
            Outbound::BlockAck(sequence) => self.conn.send(&BlockChangedAck { sequence }).await?,
            Outbound::InChunk { chunk, packet } => {
                if self.view.loaded.contains(&chunk) {
                    self.conn.send_raw(packet.id, &packet.body).await?;
                }
            }
            Outbound::EntityAdded(state) => {
                let entity_id = state.entity_id;
                self.others.insert(
                    entity_id,
                    Other {
                        state,
                        tracked: false,
                    },
                );
                self.track(entity_id).await?;
            }
            Outbound::EntityRemoved(entity_id) => {
                if let Some(other) = self.others.remove(&entity_id)
                    && other.tracked
                {
                    self.conn
                        .send(&RemoveEntities {
                            entity_ids: EntityIds(vec![entity_id]),
                        })
                        .await?;
                }
            }
            Outbound::GameMode(mode) => {
                self.game_mode = mode;
                // Spectators see other spectators, nobody else does.
                self.update_tracking().await?;
            }
            Outbound::Teleport { pos, rot } => {
                self.teleports += 1;
                self.awaiting_teleport = Some(self.teleports);
                self.conn
                    .send(&PlayerPosition {
                        teleport_id: self.teleports,
                        pos,
                        delta: [0.0; 3],
                        yaw: rot[0],
                        pitch: rot[1],
                        relatives: 0,
                    })
                    .await?;
                self.pos = pos;
                self.update_view(chunk_at(pos), self.view.radius).await?;
                self.update_tracking().await?;
            }
            Outbound::Respawn {
                pos,
                rot,
                game_mode,
            } => self.respawn(pos, rot, game_mode).await?,
            Outbound::Respawned(state) => {
                let entity_id = state.entity_id;
                if let Some(other) = self.others.get_mut(&entity_id) {
                    let was_tracked = std::mem::replace(&mut other.tracked, false);
                    other.state = state;
                    if was_tracked {
                        self.conn
                            .send(&RemoveEntities {
                                entity_ids: EntityIds(vec![entity_id]),
                            })
                            .await?;
                    }
                    self.track(entity_id).await?;
                }
            }
        }
        Ok(())
    }

    /// `PlayerList.respawn`: a new player entity at the spawn. The client
    /// keeps its chunks, as the dimension stays the same.
    async fn respawn(
        &mut self,
        pos: [f64; 3],
        [yaw, pitch]: [f32; 2],
        game_mode: GameMode,
    ) -> anyhow::Result<()> {
        self.game_mode = game_mode;
        self.conn
            .send(&Respawn {
                spawn: self.spawn_info(),
                keep: 0,
            })
            .await?;
        self.teleports += 1;
        self.awaiting_teleport = Some(self.teleports);
        self.conn
            .send(&PlayerPosition {
                teleport_id: self.teleports,
                pos,
                delta: [0.0; 3],
                yaw,
                pitch,
                relatives: 0,
            })
            .await?;
        let server = self.server;
        self.conn
            .send(&SetDefaultSpawnPosition {
                dimension: world::DIMENSION.into(),
                pos: server.spawn,
                yaw: server.spawn_angle[0],
                pitch: server.spawn_angle[1],
            })
            .await?;
        self.conn.send(&game::abilities(game_mode, false)).await?;
        self.conn
            .send(&EntityEvent {
                entity_id: self.entity_id,
                event: PERMISSION_LEVEL_EVENT + self.permission,
            })
            .await?;
        self.conn
            .send(&SetEntityData {
                entity_id: self.entity_id,
                values: EntityDataValues(vec![EntityDataValue {
                    index: data_index::SKIN_PARTS,
                    value: EntityData::Byte(self.skin_parts),
                }]),
            })
            .await?;
        self.conn
            .send(&GameEvent::new(GameEventKind::LevelChunksLoadStart, 0.0))
            .await?;
        self.pos = pos;
        self.update_view(chunk_at(pos), self.view.radius).await?;
        self.update_tracking().await
    }

    /// `ChunkMap.TrackedEntity.updatePlayer`: near enough, horizontally, and
    /// in a chunk the client has; spectators only for spectators.
    fn sees(&self, other: &EntityState) -> bool {
        if other.spectator && self.game_mode != GameMode::Spectator {
            return false;
        }
        let range = other.range.min(f64::from(self.view.radius) * 16.0);
        let dx = self.pos[0] - other.pos[0];
        let dz = self.pos[2] - other.pos[2];
        let chunk = (
            (other.pos[0].floor() as i32) >> 4,
            (other.pos[2].floor() as i32) >> 4,
        );
        dx * dx + dz * dz <= range * range && self.view.loaded.contains(&chunk)
    }

    /// Spawns or removes another player on this client as they come into
    /// or leave view.
    async fn track(&mut self, entity_id: i32) -> anyhow::Result<()> {
        let Some(other) = self.others.get(&entity_id) else {
            return Ok(());
        };
        let visible = self.sees(&other.state);
        match (other.tracked, visible) {
            (false, true) => {
                let state = Arc::clone(&other.state);
                self.spawn(&state).await?;
            }
            (true, false) => {
                self.conn
                    .send(&RemoveEntities {
                        entity_ids: EntityIds(vec![entity_id]),
                    })
                    .await?;
            }
            _ => return Ok(()),
        }
        if let Some(other) = self.others.get_mut(&entity_id) {
            other.tracked = visible;
        }
        Ok(())
    }

    async fn update_tracking(&mut self) -> anyhow::Result<()> {
        let ids: Vec<i32> = self.others.keys().copied().collect();
        for entity_id in ids {
            self.track(entity_id).await?;
        }
        Ok(())
    }

    /// `ServerEntity.sendPairingData` for a player, as one bundle.
    async fn spawn(&mut self, state: &EntityState) -> anyhow::Result<()> {
        self.conn.send(&BundleDelimiter).await?;
        self.conn
            .send(&AddEntity {
                entity_id: state.entity_id,
                uuid: state.uuid,
                entity_type: state.entity_type,
                pos: state.pos,
                velocity: LpVec3(state.velocity),
                pitch: state.pitch,
                yaw: state.yaw,
                head_yaw: state.head_yaw,
                data: state.object_data,
            })
            .await?;
        if !state.data.is_empty() {
            self.conn
                .send(&SetEntityData {
                    entity_id: state.entity_id,
                    values: EntityDataValues(state.data.clone()),
                })
                .await?;
        }
        if !state.equipment.is_empty() {
            self.conn
                .send(&SetEquipment {
                    entity_id: state.entity_id,
                    slots: state
                        .equipment
                        .iter()
                        .map(|(slot, stack)| (*slot, Some(stack.clone())))
                        .collect(),
                })
                .await?;
        }
        self.conn.send(&BundleDelimiter).await
    }
}

enum Next {
    Packet(RawPacket),
    Outbound(Outbound),
    Closed,
}

/// Waits for whichever comes first: a packet from the client or a message
/// from the tick thread. Reading a frame is cancel safe, so losing the race
/// drops no bytes.
async fn next(
    conn: &mut Connection,
    inbound: &mut UnboundedReceiver<Outbound>,
) -> anyhow::Result<Next> {
    tokio::select! {
        packet = conn.read_raw() => Ok(packet?.map_or(Next::Closed, Next::Packet)),
        message = inbound.recv() => Ok(message.map_or(Next::Closed, Next::Outbound)),
    }
}
