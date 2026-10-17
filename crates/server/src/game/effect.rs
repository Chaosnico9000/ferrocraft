//! Status effects on players and mobs (`MobEffectInstance`, the effect
//! parts of `LivingEntity`): they run down, tick (regeneration heals,
//! poison and wither hurt, hunger makes hungry), change attributes, make
//! invisible or glow, and show their particles. A player's own client
//! hears of its effects; the others only see the particles.

use std::collections::BTreeMap;
use std::sync::Arc;

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::packets::play::{
    EntityData, EntityDataValue, Particle, RemoveMobEffect, UpdateMobEffect, data_index,
    effect_flags, text,
};

use super::{Encoded, Game, Outbound};
use crate::data::GameData;

/// A player or a mob: what effects, healing and hurting apply to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Living {
    Player(i32),
    Mob(i32),
}

impl Living {
    pub fn id(self) -> i32 {
        match self {
            Living::Player(id) | Living::Mob(id) => id,
        }
    }
}

/// An effect with its level and time (`MobEffectInstance`).
#[derive(Debug, Clone, PartialEq)]
pub struct EffectInstance {
    /// Network id in `minecraft:mob_effect`.
    pub effect: usize,
    /// The level less one, 0 to 255.
    pub amplifier: i32,
    /// Ticks left; -1 for ever.
    pub duration: i32,
    /// From a beacon or conduit: fainter particles.
    pub ambient: bool,
    pub visible: bool,
    pub show_icon: bool,
    /// A weaker but longer effect of the same kind that takes over when
    /// this one ends.
    pub hidden: Option<Box<EffectInstance>>,
}

impl EffectInstance {
    pub fn new(effect: usize, duration: i32, amplifier: i32) -> Self {
        Self {
            effect,
            amplifier: amplifier.clamp(0, 255),
            duration,
            ambient: false,
            visible: true,
            show_icon: true,
            hidden: None,
        }
    }

    pub fn infinite(&self) -> bool {
        self.duration == -1
    }

    fn shorter_than(&self, other: &Self) -> bool {
        !self.infinite() && (self.duration < other.duration || other.infinite())
    }

    /// `endsWithin`.
    pub fn ends_within(&self, ticks: i32) -> bool {
        !self.infinite() && self.duration <= ticks
    }

    fn set_details_from(&mut self, other: &Self) {
        self.duration = other.duration;
        self.amplifier = other.amplifier;
        self.ambient = other.ambient;
        self.visible = other.visible;
        self.show_icon = other.show_icon;
    }

    /// `MobEffectInstance.update`: a stronger effect replaces this one and
    /// keeps it hidden if it lasts longer; a longer one of the same level
    /// extends it; a longer weaker one waits hidden. Returns whether it
    /// changed.
    pub fn update(&mut self, take_over: &Self) -> bool {
        let mut changed = false;
        if take_over.amplifier > self.amplifier {
            if take_over.shorter_than(self) {
                let previous = self.hidden.take();
                let mut hidden = self.clone();
                hidden.hidden = previous;
                self.hidden = Some(Box::new(hidden));
            }
            self.amplifier = take_over.amplifier;
            self.duration = take_over.duration;
            changed = true;
        } else if self.shorter_than(take_over) {
            if take_over.amplifier == self.amplifier {
                self.duration = take_over.duration;
                changed = true;
            } else if let Some(hidden) = &mut self.hidden {
                hidden.update(take_over);
            } else {
                let mut hidden = take_over.clone();
                hidden.hidden = None;
                self.hidden = Some(Box::new(hidden));
            }
        }
        if (!take_over.ambient && self.ambient) || changed {
            self.ambient = take_over.ambient;
            changed = true;
        }
        if take_over.visible != self.visible {
            self.visible = take_over.visible;
            changed = true;
        }
        if take_over.show_icon != self.show_icon {
            self.show_icon = take_over.show_icon;
            changed = true;
        }
        changed
    }

    fn tick_down(&mut self) {
        if let Some(hidden) = &mut self.hidden {
            hidden.tick_down();
        }
        if !self.infinite() && self.duration != 0 {
            self.duration -= 1;
        }
    }

    /// When it ran out, the hidden one takes over; returns whether one did.
    fn downgrade(&mut self) -> bool {
        if self.duration != 0 {
            return false;
        }
        let Some(hidden) = self.hidden.take() else {
            return false;
        };
        self.set_details_from(&hidden);
        self.hidden = hidden.hidden;
        true
    }

    /// `withScaledDuration`: for lingering clouds and arrows.
    pub fn scaled(&self, scale: f32) -> Self {
        let mut copy = self.clone();
        if !copy.infinite() && copy.duration != 0 {
            copy.duration = ((copy.duration as f32 * scale).floor() as i32).max(1);
        }
        copy
    }

    fn flags(&self, blend: bool) -> u8 {
        let mut flags = 0;
        if self.ambient {
            flags |= effect_flags::AMBIENT;
        }
        if self.visible {
            flags |= effect_flags::VISIBLE;
        }
        if self.show_icon {
            flags |= effect_flags::SHOW_ICON;
        }
        if blend {
            flags |= effect_flags::BLEND;
        }
        flags
    }

    /// The form vanilla saves (`MobEffectInstance.CODEC`), the same as in
    /// a potion's `custom_effects`.
    pub fn to_nbt(&self, data: &GameData) -> Option<Nbt> {
        let name = &data.alchemy.effects.get(self.effect)?.name;
        let mut entries = vec![("id".to_owned(), Nbt::String(name.clone()))];
        entries.extend(self.details());
        Some(Nbt::Compound(entries))
    }

    fn details(&self) -> Vec<(String, Nbt)> {
        let mut entries = Vec::new();
        if self.amplifier != 0 {
            entries.push(("amplifier".to_owned(), Nbt::Int(self.amplifier)));
        }
        if self.duration != 0 {
            entries.push(("duration".to_owned(), Nbt::Int(self.duration)));
        }
        if self.ambient {
            entries.push(("ambient".to_owned(), Nbt::Byte(1)));
        }
        if !self.visible {
            entries.push(("show_particles".to_owned(), Nbt::Byte(0)));
        }
        entries.push(("show_icon".to_owned(), Nbt::Byte(i8::from(self.show_icon))));
        if let Some(hidden) = &self.hidden {
            entries.push(("hidden_effect".to_owned(), Nbt::Compound(hidden.details())));
        }
        entries
    }

    pub fn from_nbt(nbt: &Nbt, data: &GameData) -> Option<Self> {
        let Some(Nbt::String(name)) = nbt.get("id") else {
            return None;
        };
        let (effect, _) = data.effect(name)?;
        Some(Self::details_from_nbt(effect, nbt))
    }

    fn details_from_nbt(effect: usize, nbt: &Nbt) -> Self {
        let int = |key: &str, default: i32| match nbt.get(key) {
            Some(Nbt::Int(v)) => *v,
            Some(Nbt::Byte(v)) => i32::from(*v),
            _ => default,
        };
        let visible = int("show_particles", 1) != 0;
        Self {
            effect,
            amplifier: int("amplifier", 0).clamp(0, 255),
            duration: int("duration", 0),
            ambient: int("ambient", 0) != 0,
            visible,
            show_icon: int("show_icon", i32::from(visible)) != 0,
            hidden: nbt
                .get("hidden_effect")
                .map(|h| Box::new(Self::details_from_nbt(effect, h))),
        }
    }
}

/// The effects on one entity, by effect id.
#[derive(Debug, Clone, Default)]
pub struct Effects {
    pub active: BTreeMap<usize, EffectInstance>,
    /// Flags and particles need refreshing (`effectsDirty`).
    pub dirty: bool,
    /// What others see of them, as of the last refresh.
    pub look: Look,
}

/// What the effects show: shared flags and particles.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Look {
    pub invisible: bool,
    pub glowing: bool,
    pub particles: Vec<Particle>,
    /// Every visible effect is ambient (fainter particles).
    pub ambient: bool,
}

impl Look {
    /// The synchronized values for the particles; with `all`, also when
    /// they are at their defaults.
    pub fn data(&self, all: bool) -> Vec<EntityDataValue> {
        let mut values = Vec::new();
        if all || !self.particles.is_empty() {
            values.push(EntityDataValue {
                index: data_index::EFFECT_PARTICLES,
                value: EntityData::Particles(self.particles.clone()),
            });
        }
        if all || self.ambient {
            values.push(EntityDataValue {
                index: data_index::EFFECT_AMBIENCE,
                value: EntityData::Boolean(self.ambient),
            });
        }
        values
    }
}

impl Effects {
    pub fn get(&self, effect: usize) -> Option<&EffectInstance> {
        self.active.get(&effect)
    }

    pub fn has(&self, effect: usize) -> bool {
        self.active.contains_key(&effect)
    }

    /// The amplifier of an effect, if present.
    pub fn level(&self, effect: usize) -> Option<i32> {
        self.active.get(&effect).map(|e| e.amplifier)
    }

    /// `LivingEntity.areAllEffectsAmbient`.
    fn all_ambient(&self) -> bool {
        self.active.values().all(|e| !e.visible || e.ambient)
    }

    /// The particles to show (`updateSynchronizedMobEffectParticles`).
    pub fn particles(&self, data: &GameData) -> Vec<Particle> {
        self.active
            .values()
            .filter(|e| e.visible)
            .filter_map(|e| {
                let info = data.alchemy.effects.get(e.effect)?;
                let kind = data.particle_type(&info.particle).ok()?;
                // `ColorParticleOption` with the alpha of
                // `MobEffect.AMBIENT_ALPHA` for ambient effects.
                let alpha: u32 = if e.ambient { 38 } else { 255 };
                let color = info
                    .colored_particle
                    .then_some(((alpha << 24) | (info.color as u32 & 0xff_ffff)) as i32);
                Some(Particle { kind, color })
            })
            .collect()
    }

    pub fn to_nbt(&self, data: &GameData) -> Option<Nbt> {
        if self.active.is_empty() {
            return None;
        }
        Some(Nbt::List(
            self.active
                .values()
                .filter_map(|e| e.to_nbt(data))
                .collect(),
        ))
    }

    pub fn from_nbt(nbt: Option<&Nbt>, data: &GameData) -> Self {
        let mut effects = Self::default();
        if let Some(Nbt::List(list)) = nbt {
            for entry in list {
                if let Some(instance) = EffectInstance::from_nbt(entry, data) {
                    effects.active.insert(instance.effect, instance);
                }
            }
        }
        effects.dirty = !effects.active.is_empty();
        effects
    }
}

/// `shouldApplyEffectTickThisTick` of the effects that do something over
/// time: every `interval >> amplifier` ticks.
fn ticks_now(name: &str, tick: i32, amplifier: i32) -> bool {
    let every = |interval: i32| {
        let interval = interval >> amplifier.min(31);
        interval <= 0 || tick % interval == 0
    };
    match name {
        "minecraft:regeneration" => every(50),
        "minecraft:poison" => every(25),
        "minecraft:wither" => every(40),
        "minecraft:hunger" | "minecraft:absorption" => true,
        _ => false,
    }
}

impl Game {
    pub(super) fn effects(&self, target: Living) -> Option<&Effects> {
        match target {
            Living::Player(id) => self.players.get(&id).map(|p| &p.effects),
            Living::Mob(id) => self.mobs.get(&id).map(|m| &m.effects),
        }
    }

    fn effects_mut(&mut self, target: Living) -> Option<&mut Effects> {
        match target {
            Living::Player(id) => self.players.get_mut(&id).map(|p| &mut p.effects),
            Living::Mob(id) => self.mobs.get_mut(&id).map(|m| &mut m.effects),
        }
    }

    /// Whether the entity has an effect, by name.
    pub(super) fn has_effect(&self, target: Living, name: &str) -> bool {
        self.effect_level(target, name).is_some()
    }

    /// The amplifier of an effect on the entity, by name.
    pub(super) fn effect_level(&self, target: Living, name: &str) -> Option<i32> {
        let (effect, _) = self.context.data.effect(name)?;
        self.effects(target)?.level(effect)
    }

    /// `LivingEntity.canBeAffected`: undead ignore poison and regeneration.
    fn can_be_affected(&self, target: Living, effect: usize) -> bool {
        let name = self
            .context
            .data
            .alchemy
            .effects
            .get(effect)
            .map_or("", |e| e.name.as_str());
        match target {
            Living::Mob(id) => {
                let undead = self.mobs.get(&id).is_some_and(|m| m.kind.undead());
                !(undead && matches!(name, "minecraft:regeneration" | "minecraft:poison"))
            }
            Living::Player(id) => self.players.get(&id).is_some_and(|p| !p.dead),
        }
    }

    /// `LivingEntity.addEffect`. Returns whether anything changed.
    pub(super) fn add_effect(&mut self, target: Living, effect: EffectInstance) -> bool {
        if !self.can_be_affected(target, effect.effect) {
            return false;
        }
        let id = effect.effect;
        let Some(effects) = self.effects_mut(target) else {
            return false;
        };
        let (changed, added) = match effects.active.get_mut(&id) {
            None => {
                effects.active.insert(id, effect.clone());
                (true, true)
            }
            Some(existing) => (existing.update(&effect), false),
        };
        if changed {
            effects.dirty = true;
            if let Some(current) = effects.active.get(&id).cloned() {
                self.effect_changed(target, &current, added);
            }
        }
        // `onEffectStarted`: absorption fills the golden hearts.
        let data = Arc::clone(&self.context.data);
        if data.alchemy.effects.get(id).map(|e| e.name.as_str()) == Some("minecraft:absorption") {
            let amount = (4 * (1 + effect.amplifier)) as f32;
            let current = self.absorption(target);
            self.set_absorption(target, current.max(amount));
        }
        changed
    }

    /// The client of a player hears of a new or changed effect; attributes
    /// follow it.
    fn effect_changed(&mut self, target: Living, effect: &EffectInstance, blend: bool) {
        if let Living::Player(id) = target
            && let Some(player) = self.players.get(&id)
        {
            player.send(Outbound::Packet(Arc::new(Encoded::new(&UpdateMobEffect {
                entity_id: id,
                effect: effect.effect as i32,
                amplifier: effect.amplifier,
                duration: effect.duration,
                flags: effect.flags(blend),
            }))));
        }
        self.sync_attributes(target);
    }

    /// `LivingEntity.removeEffect`.
    pub(super) fn remove_effect(&mut self, target: Living, effect: usize) -> bool {
        let Some(effects) = self.effects_mut(target) else {
            return false;
        };
        let Some(removed) = effects.active.remove(&effect) else {
            return false;
        };
        effects.dirty = true;
        self.effects_removed(target, &[removed]);
        true
    }

    /// `LivingEntity.removeAllEffects`, as milk does.
    pub(super) fn clear_effects(&mut self, target: Living) -> bool {
        let Some(effects) = self.effects_mut(target) else {
            return false;
        };
        if effects.active.is_empty() {
            return false;
        }
        let removed: Vec<EffectInstance> =
            std::mem::take(&mut effects.active).into_values().collect();
        effects.dirty = true;
        self.effects_removed(target, &removed);
        true
    }

    fn effects_removed(&mut self, target: Living, removed: &[EffectInstance]) {
        if let Living::Player(id) = target
            && let Some(player) = self.players.get(&id)
        {
            for effect in removed {
                player.send(Outbound::Packet(Arc::new(Encoded::new(&RemoveMobEffect {
                    entity_id: id,
                    effect: effect.effect as i32,
                }))));
            }
        }
        self.sync_attributes(target);
    }

    /// Every effect a player has, for its client after joining or
    /// respawning (`PlayerList.sendActivePlayerEffects`).
    pub(super) fn send_effects(&self, entity_id: i32) {
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        for effect in player.effects.active.values() {
            player.send(Outbound::Packet(Arc::new(Encoded::new(&UpdateMobEffect {
                entity_id,
                effect: effect.effect as i32,
                amplifier: effect.amplifier,
                duration: effect.duration,
                flags: effect.flags(false),
            }))));
        }
    }

    /// `LivingEntity.tickEffects`: each effect runs down and does its work.
    pub(super) fn tick_effects(&mut self, target: Living) {
        let data = Arc::clone(&self.context.data);
        let tick = self.tick as i32;
        let Some(effects) = self.effects(target) else {
            return;
        };
        let ids: Vec<usize> = effects.active.keys().copied().collect();
        for id in ids {
            let Some(effect) = self.effects(target).and_then(|e| e.get(id)).cloned() else {
                continue;
            };
            let name = data.alchemy.effects.get(id).map_or("", |e| e.name.as_str());
            if !effect.infinite() && effect.duration <= 0 {
                self.remove_effect(target, id);
                continue;
            }
            let count = if effect.infinite() {
                tick
            } else {
                effect.duration
            };
            let instant = data.alchemy.effects.get(id).is_some_and(|e| e.instant);
            let applies = if instant {
                count >= 1
            } else {
                ticks_now(name, count, effect.amplifier)
            };
            if applies && !self.apply_effect_tick(target, name, effect.amplifier) {
                self.remove_effect(target, id);
                continue;
            }
            let Some(effects) = self.effects_mut(target) else {
                return;
            };
            let Some(current) = effects.active.get_mut(&id) else {
                continue;
            };
            current.tick_down();
            let downgraded = current.downgrade();
            let ended = !current.infinite() && current.duration <= 0;
            let snapshot = current.clone();
            if ended {
                self.remove_effect(target, id);
            } else if downgraded {
                if let Some(effects) = self.effects_mut(target) {
                    effects.dirty = true;
                }
                self.effect_changed(target, &snapshot, false);
            } else if snapshot.duration % 600 == 0 {
                // Now and then the client is told the time left again.
                self.effect_changed(target, &snapshot, false);
            }
        }
        self.refresh_effect_flags(target);
    }

    /// `MobEffect.applyEffectTick`; `false` ends the effect.
    fn apply_effect_tick(&mut self, target: Living, name: &str, amplifier: i32) -> bool {
        match name {
            "minecraft:regeneration" => {
                if self.health(target) < self.max_health(target) {
                    self.heal(target, 1.0);
                }
            }
            "minecraft:poison" => {
                if self.health(target) > 1.0 {
                    self.hurt_by_effect(target, 1.0, "minecraft:magic");
                }
            }
            "minecraft:wither" => self.hurt_by_effect(target, 1.0, "minecraft:wither"),
            "minecraft:hunger" => {
                if let Living::Player(id) = target {
                    self.exhaust(id, 0.005 * (amplifier + 1) as f32);
                }
            }
            "minecraft:absorption" => return self.absorption(target) > 0.0,
            "minecraft:instant_health" | "minecraft:instant_damage" | "minecraft:saturation" => {
                self.apply_instant(target, name, amplifier, 1.0, None);
            }
            _ => {}
        }
        true
    }

    /// `MobEffect.applyInstantaneousEffect`: healing and harming scaled by
    /// how close a splash came, saturation.
    pub(super) fn apply_instant(
        &mut self,
        target: Living,
        name: &str,
        amplifier: i32,
        scale: f64,
        source: Option<i32>,
    ) {
        let undead = matches!(target, Living::Mob(id) if self.mobs.get(&id).is_some_and(|m| m.kind.undead()));
        let shift = amplifier.clamp(0, 30);
        match name {
            "minecraft:instant_health" | "minecraft:instant_damage" => {
                let harm = name == "minecraft:instant_damage";
                if harm == undead {
                    let amount = (scale * f64::from(4 << shift) + 0.5) as i32;
                    self.heal(target, amount.max(0) as f32);
                } else {
                    let amount = (scale * f64::from(6 << shift) + 0.5) as i32;
                    let damage_type = if source.is_some() {
                        "minecraft:indirect_magic"
                    } else {
                        "minecraft:magic"
                    };
                    self.hurt_living_by(target, amount as f32, damage_type, source);
                }
            }
            "minecraft:saturation" => {
                if let Living::Player(id) = target
                    && let Some(player) = self.players.get_mut(&id)
                {
                    player.food.eat(amplifier + 1, 1.0);
                    self.send_health(id);
                }
            }
            _ => {}
        }
    }

    /// Damage from an effect itself: poison, wither.
    fn hurt_by_effect(&mut self, target: Living, amount: f32, damage_type: &str) {
        self.hurt_living_by(target, amount, damage_type, None);
    }

    /// Damage of a type to a player or mob, with someone to blame.
    pub(super) fn hurt_living_by(
        &mut self,
        target: Living,
        amount: f32,
        damage_type: &str,
        attacker: Option<i32>,
    ) -> bool {
        match target {
            Living::Mob(id) => self.hurt_mob_by(id, amount, attacker, None, damage_type),
            Living::Player(id) => {
                let Some(player) = self.players.get(&id) else {
                    return false;
                };
                let victim = text::player_name(player.name(), player.uuid());
                let key = format!(
                    "death.attack.{}",
                    match damage_type {
                        "minecraft:indirect_magic" => "indirectMagic",
                        other => other.trim_start_matches("minecraft:"),
                    }
                );
                let mut with = vec![victim];
                if let Some(name) = attacker.and_then(|a| self.entity_name(a)) {
                    with.push(name);
                }
                let message = text::translate(&key, with, None);
                self.hurt_player(
                    id,
                    super::survival::Hit {
                        amount,
                        damage_type,
                        attacker,
                        direct: None,
                        knockback_from: None,
                        message,
                    },
                )
            }
        }
    }

    pub(super) fn health(&self, target: Living) -> f32 {
        match target {
            Living::Player(id) => self.players.get(&id).map_or(0.0, |p| p.health),
            Living::Mob(id) => self.mobs.get(&id).map_or(0.0, |m| m.health),
        }
    }

    pub(super) fn max_health(&self, target: Living) -> f32 {
        self.attribute(target, "minecraft:max_health") as f32
    }

    /// `LivingEntity.heal`.
    pub(super) fn heal(&mut self, target: Living, amount: f32) {
        let max = self.max_health(target);
        match target {
            Living::Player(id) => {
                if let Some(player) = self.players.get_mut(&id)
                    && !player.dead
                    && player.health > 0.0
                {
                    player.health = (player.health + amount).min(max);
                    player.data_dirty = true;
                    player.state = None;
                    self.send_health(id);
                }
            }
            Living::Mob(id) => {
                if let Some(mob) = self.mobs.get_mut(&id)
                    && mob.alive()
                {
                    mob.health = (mob.health + amount).min(max);
                    mob.mark_dirty();
                }
            }
        }
    }

    /// Golden hearts (`getAbsorptionAmount`).
    pub(super) fn absorption(&self, target: Living) -> f32 {
        match target {
            Living::Player(id) => self.players.get(&id).map_or(0.0, |p| p.absorption),
            Living::Mob(id) => self.mobs.get(&id).map_or(0.0, |m| m.absorption),
        }
    }

    /// `setAbsorptionAmount`, at most `max_absorption`.
    pub(super) fn set_absorption(&mut self, target: Living, amount: f32) {
        let max = self.attribute(target, "minecraft:max_absorption") as f32;
        let amount = amount.clamp(0.0, max.max(0.0));
        match target {
            Living::Player(id) => {
                if let Some(player) = self.players.get_mut(&id)
                    && player.absorption != amount
                {
                    player.absorption = amount;
                    player.data_dirty = true;
                    player.state = None;
                    self.send_own_data(id);
                }
            }
            Living::Mob(id) => {
                if let Some(mob) = self.mobs.get_mut(&id) {
                    mob.absorption = amount;
                }
            }
        }
    }

    /// `updateInvisibilityStatus` and `updateGlowingStatus` once effects
    /// changed: the shared flags and the particles.
    fn refresh_effect_flags(&mut self, target: Living) {
        let data = Arc::clone(&self.context.data);
        let invisible = data.effect("minecraft:invisibility").map(|(id, _)| id);
        let glowing = data.effect("minecraft:glowing").map(|(id, _)| id);
        let Some(effects) = self.effects_mut(target) else {
            return;
        };
        if !std::mem::take(&mut effects.dirty) {
            return;
        }
        effects.look = Look {
            invisible: invisible.is_some_and(|id| effects.has(id)),
            glowing: glowing.is_some_and(|id| effects.has(id)),
            particles: effects.particles(&data),
            ambient: !effects.active.is_empty() && effects.all_ambient(),
        };
        match target {
            Living::Player(id) => {
                if let Some(player) = self.players.get_mut(&id) {
                    player.data_dirty = true;
                    player.state = None;
                }
                self.send_own_data(id);
            }
            Living::Mob(id) => {
                if let Some(mob) = self.mobs.get_mut(&id) {
                    mob.mark_dirty();
                }
            }
        }
        // Less absorption may now be allowed.
        let max = self.attribute(target, "minecraft:max_absorption") as f32;
        if self.absorption(target) > max {
            self.set_absorption(target, max);
        }
        let max_health = self.max_health(target);
        match target {
            Living::Player(id) => {
                if let Some(player) = self.players.get_mut(&id)
                    && player.health > max_health
                {
                    player.health = max_health;
                    self.send_health(id);
                }
            }
            Living::Mob(id) => {
                if let Some(mob) = self.mobs.get_mut(&id)
                    && mob.health > max_health
                {
                    mob.health = max_health;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stronger_effects_hide_the_longer_weaker_ones() {
        // Speed I for 60 s, then speed II for 10 s: II now, I after.
        let mut speed = EffectInstance::new(1, 1200, 0);
        assert!(speed.update(&EffectInstance::new(1, 200, 1)));
        assert_eq!((speed.amplifier, speed.duration), (1, 200));
        assert_eq!(speed.hidden.as_ref().unwrap().duration, 1200);
        for _ in 0..200 {
            speed.tick_down();
            speed.downgrade();
        }
        assert_eq!((speed.amplifier, speed.duration), (0, 1000));
        assert!(speed.hidden.is_none());
        // The same level for longer extends it, a shorter one does nothing.
        assert!(speed.update(&EffectInstance::new(1, 2000, 0)));
        assert_eq!(speed.duration, 2000);
        assert!(!speed.update(&EffectInstance::new(1, 100, 0)));
    }

    #[test]
    fn effects_tick_at_their_pace() {
        assert!(ticks_now("minecraft:regeneration", 100, 0));
        assert!(!ticks_now("minecraft:regeneration", 101, 0));
        assert!(ticks_now("minecraft:regeneration", 25, 1));
        assert!(ticks_now("minecraft:poison", 7, 5), "every tick from V on");
        assert!(!ticks_now("minecraft:speed", 0, 0));
    }

    #[test]
    fn saved_like_vanilla() {
        let data = GameData::load().unwrap();
        let (speed, _) = data.effect("minecraft:speed").unwrap();
        let mut effect = EffectInstance::new(speed, 600, 1);
        effect.hidden = Some(Box::new(EffectInstance::new(speed, 1200, 0)));
        let nbt = effect.to_nbt(&data).unwrap();
        assert_eq!(nbt.get("id"), Some(&Nbt::String("minecraft:speed".into())));
        assert_eq!(EffectInstance::from_nbt(&nbt, &data), Some(effect));
        // The same as a potion's custom effect.
        let wire = ferrocraft_protocol::component::EFFECT_INSTANCE
            .read(
                &mut {
                    let mut out = Vec::new();
                    ferrocraft_protocol::component::EFFECT_INSTANCE
                        .write(&nbt, &mut out, &data)
                        .unwrap();
                    out
                }
                .as_slice(),
                &data,
            )
            .unwrap();
        assert_eq!(wire, nbt);
    }
}
