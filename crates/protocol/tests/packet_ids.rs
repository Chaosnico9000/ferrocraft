//! Every packet's `ID` must match Mojang's data generator report.

use ferrocraft_protocol::GAME_VERSION;
use ferrocraft_protocol::packets::{Packet, configuration, handshake, login, play, status};

fn report() -> serde_json::Value {
    let path = format!(
        "{}/../../data/{GAME_VERSION}/packets.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn check<P: Packet>(report: &serde_json::Value, state: &str, direction: &str, name: &str) {
    let id = &report[state][direction][format!("minecraft:{name}")]["protocol_id"];
    assert_eq!(
        id.as_i64(),
        Some(i64::from(P::ID)),
        "{state}/{direction}/{name} ({})",
        std::any::type_name::<P>()
    );
}

#[test]
fn packet_ids_match_report() {
    let r = &report();
    let (cb, sb) = ("clientbound", "serverbound");

    check::<handshake::Intention>(r, "handshake", sb, "intention");

    check::<status::StatusRequest>(r, "status", sb, "status_request");
    check::<status::StatusResponse>(r, "status", cb, "status_response");
    check::<status::PingRequest>(r, "status", sb, "ping_request");
    check::<status::PongResponse>(r, "status", cb, "pong_response");

    check::<login::Hello>(r, "login", sb, "hello");
    check::<login::LoginDisconnect>(r, "login", cb, "login_disconnect");
    check::<login::LoginFinished>(r, "login", cb, "login_finished");
    check::<login::LoginCompression>(r, "login", cb, "login_compression");
    check::<login::EncryptionRequest>(r, "login", cb, "hello");
    check::<login::Key>(r, "login", sb, "key");
    check::<login::LoginAcknowledged>(r, "login", sb, "login_acknowledged");

    use configuration as c;
    let cfg = "configuration";
    check::<c::ClientInformation>(r, cfg, sb, "client_information");
    check::<c::CustomPayload>(r, cfg, cb, "custom_payload");
    check::<c::Disconnect>(r, cfg, cb, "disconnect");
    check::<c::ClientboundFinishConfiguration>(r, cfg, cb, "finish_configuration");
    check::<c::ServerboundFinishConfiguration>(r, cfg, sb, "finish_configuration");
    check::<c::ClientboundKeepAlive>(r, cfg, cb, "keep_alive");
    check::<c::ServerboundKeepAlive>(r, cfg, sb, "keep_alive");
    check::<c::RegistryData>(r, cfg, cb, "registry_data");
    check::<c::UpdateEnabledFeatures>(r, cfg, cb, "update_enabled_features");
    check::<c::UpdateTags>(r, cfg, cb, "update_tags");
    check::<c::ClientboundSelectKnownPacks>(r, cfg, cb, "select_known_packs");
    check::<c::ServerboundSelectKnownPacks>(r, cfg, sb, "select_known_packs");

    use play as p;
    check::<p::Login>(r, "play", cb, "login");
    check::<p::Disconnect>(r, "play", cb, "disconnect");
    check::<p::ClientboundKeepAlive>(r, "play", cb, "keep_alive");
    check::<p::ServerboundKeepAlive>(r, "play", sb, "keep_alive");
    check::<p::GameEvent>(r, "play", cb, "game_event");
    check::<p::SetChunkCacheCenter>(r, "play", cb, "set_chunk_cache_center");
    check::<p::SetDefaultSpawnPosition>(r, "play", cb, "set_default_spawn_position");
    check::<p::PlayerAbilities>(r, "play", cb, "player_abilities");
    check::<p::PlayerInfoUpdate>(r, "play", cb, "player_info_update");
    check::<p::PlayerPosition>(r, "play", cb, "player_position");
    check::<p::LevelChunkWithLight>(r, "play", cb, "level_chunk_with_light");
    check::<p::ForgetLevelChunk>(r, "play", cb, "forget_level_chunk");
    check::<p::LightUpdate>(r, "play", cb, "light_update");
    check::<p::SetTime>(r, "play", cb, "set_time");
    check::<p::Commands>(r, "play", cb, "commands");
    check::<p::SetEntityMotion>(r, "play", cb, "set_entity_motion");
    check::<p::TakeItemEntity>(r, "play", cb, "take_item_entity");
    check::<p::OpenScreen>(r, "play", cb, "open_screen");
    check::<p::ClientboundContainerClose>(r, "play", cb, "container_close");
    check::<p::ServerboundContainerClose>(r, "play", sb, "container_close");
    check::<p::SetCursorItem>(r, "play", cb, "set_cursor_item");
    check::<p::ContainerClick>(r, "play", sb, "container_click");
    check::<p::BlockEvent>(r, "play", cb, "block_event");
    check::<p::BlockEntityData>(r, "play", cb, "block_entity_data");
    check::<p::OpenSignEditor>(r, "play", cb, "open_sign_editor");
    check::<p::SignUpdate>(r, "play", sb, "sign_update");
    check::<p::EntityEvent>(r, "play", cb, "entity_event");
    check::<p::ChangeGameMode>(r, "play", sb, "change_game_mode");
    check::<p::PlayerInfoGameMode>(r, "play", cb, "player_info_update");
    check::<p::ChunkBatchStart>(r, "play", cb, "chunk_batch_start");
    check::<p::ChunkBatchFinished>(r, "play", cb, "chunk_batch_finished");
    check::<p::AcceptTeleportation>(r, "play", sb, "accept_teleportation");
    check::<p::ChunkBatchReceived>(r, "play", sb, "chunk_batch_received");
    check::<p::ClientTickEnd>(r, "play", sb, "client_tick_end");
    check::<p::PlayerLoaded>(r, "play", sb, "player_loaded");
    check::<p::ClientInformation>(r, "play", sb, "client_information");

    check::<p::BundleDelimiter>(r, "play", cb, "bundle_delimiter");
    check::<p::AddEntity>(r, "play", cb, "add_entity");
    check::<p::SetEntityData>(r, "play", cb, "set_entity_data");
    check::<p::MoveEntityPos>(r, "play", cb, "move_entity_pos");
    check::<p::MoveEntityPosRot>(r, "play", cb, "move_entity_pos_rot");
    check::<p::MoveEntityRot>(r, "play", cb, "move_entity_rot");
    check::<p::EntityPositionSync>(r, "play", cb, "entity_position_sync");
    check::<p::RotateHead>(r, "play", cb, "rotate_head");
    check::<p::RemoveEntities>(r, "play", cb, "remove_entities");
    check::<p::PlayerInfoRemove>(r, "play", cb, "player_info_remove");
    check::<p::PlayerInfoLatency>(r, "play", cb, "player_info_update");
    check::<p::SwingAnimation>(r, "play", cb, "swing_animation");
    check::<p::SetEquipment>(r, "play", cb, "set_equipment");
    check::<p::PlayerInput>(r, "play", sb, "player_input");
    check::<p::PlayerCommand>(r, "play", sb, "player_command");
    check::<p::Punch>(r, "play", sb, "punch");
    check::<p::ServerboundPlayerAbilities>(r, "play", sb, "player_abilities");

    check::<p::BlockUpdate>(r, "play", cb, "block_update");
    check::<p::BlockChangedAck>(r, "play", cb, "block_changed_ack");
    check::<p::PlayerAction>(r, "play", sb, "player_action");
    check::<p::UseItemOn>(r, "play", sb, "use_item_on");
    check::<p::SetCarriedItem>(r, "play", sb, "set_carried_item");
    check::<p::SetCreativeModeSlot>(r, "play", sb, "set_creative_mode_slot");
    check::<p::ContainerSetContent>(r, "play", cb, "container_set_content");
    check::<p::SetHeldSlot>(r, "play", cb, "set_held_slot");
    check::<p::ContainerSetSlot>(r, "play", cb, "container_set_slot");

    check::<p::SystemChat>(r, "play", cb, "system_chat");
    check::<p::Chat>(r, "play", sb, "chat");
    check::<p::ChatCommand>(r, "play", sb, "chat_command");
    check::<p::SetExperience>(r, "play", cb, "set_experience");
    check::<p::ContainerSetData>(r, "play", cb, "container_set_data");
    check::<p::ChangeDifficulty>(r, "play", cb, "change_difficulty");
    check::<p::Explode>(r, "play", cb, "explode");

    for (id, name) in [
        (p::MOVE_PLAYER_POS, "move_player_pos"),
        (p::MOVE_PLAYER_POS_ROT, "move_player_pos_rot"),
        (p::MOVE_PLAYER_ROT, "move_player_rot"),
        (p::MOVE_PLAYER_STATUS_ONLY, "move_player_status_only"),
    ] {
        let expected = &r["play"][sb][format!("minecraft:{name}")]["protocol_id"];
        assert_eq!(expected.as_i64(), Some(i64::from(id)), "play/{sb}/{name}");
    }
}
