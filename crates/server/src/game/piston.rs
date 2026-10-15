//! Pistons as vanilla moves them (`PistonBaseBlock`,
//! `PistonStructureResolver`, `PistonMovingBlockEntity`): power makes a
//! piston queue a block event; the event moves up to twelve blocks one
//! block on, slime and honey taking their neighbours along. Moving blocks
//! sit in `moving_piston` blocks for two ticks and land after the third.
//! Sticky pistons pull the block in front back. Clients animate the move
//! themselves from the block event; the moving blocks are not sent.

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::Direction;

use super::Game;
use super::blocks::{DIRECTIONS, Kind, center, update};
use crate::block_entity::Moving;
use crate::data::{Aabb, push};
use crate::placement::opposite;
use crate::world::{MAX_Y, MIN_Y};

/// `PistonStructureResolver.MAX_PUSH_DEPTH`.
const MAX_PUSH: usize = 12;
/// Block event types (`TRIGGER_EXTEND`, `TRIGGER_CONTRACT`,
/// `TRIGGER_DROP`).
const EXTEND: u8 = 0;
const CONTRACT: u8 = 1;
const DROP: u8 = 2;

/// `PistonStructureResolver`: which blocks a piston moves and which break.
struct Resolver {
    piston: BlockPos,
    facing: Direction,
    extending: bool,
    start: BlockPos,
    push: Direction,
    to_push: Vec<BlockPos>,
    to_destroy: Vec<BlockPos>,
}

fn steps(direction: Direction, n: i32) -> [i32; 3] {
    let one = direction.offset(BlockPos { x: 0, y: 0, z: 0 });
    [one.x * n, one.y * n, one.z * n]
}

fn relative(pos: BlockPos, direction: Direction, n: i32) -> BlockPos {
    let [x, y, z] = steps(direction, n);
    BlockPos {
        x: pos.x + x,
        y: pos.y + y,
        z: pos.z + z,
    }
}

fn axis(direction: Direction) -> u8 {
    match direction {
        Direction::West | Direction::East => 0,
        Direction::Down | Direction::Up => 1,
        Direction::North | Direction::South => 2,
    }
}

impl Game {
    fn is_piston_base(&self, state: u32) -> bool {
        self.kind(state) == Kind::Piston
    }

    fn is_sticky_block(&self, state: u32) -> bool {
        self.is(state, "minecraft:slime_block") || self.is(state, "minecraft:honey_block")
    }

    /// `canStickToEachOther`: slime and honey do not stick together.
    fn stick_together(&self, a: u32, b: u32) -> bool {
        let (slime, honey) = ("minecraft:slime_block", "minecraft:honey_block");
        if (self.is(a, honey) && self.is(b, slime)) || (self.is(a, slime) && self.is(b, honey)) {
            return false;
        }
        self.is_sticky_block(a) || self.is_sticky_block(b)
    }

    pub(super) fn moving_at(&self, pos: BlockPos) -> Option<Moving> {
        self.context.world.block_entity(pos)?.moving
    }

    /// `PistonBaseBlock.isPushable`.
    fn is_pushable(
        &self,
        state: u32,
        pos: BlockPos,
        direction: Direction,
        allow_destroy: bool,
        connection: Direction,
    ) -> bool {
        if pos.y < MIN_Y || pos.y > MAX_Y {
            return false;
        }
        if self.is_air(state) {
            return true;
        }
        if (direction == Direction::Down && pos.y == MIN_Y)
            || (direction == Direction::Up && pos.y == MAX_Y)
        {
            return false;
        }
        let data = &self.context.data;
        let Some((_, block)) = data.block_of(state) else {
            return false;
        };
        if self.is_piston_base(state) {
            if self.bool_value(state, "extended") {
                return false;
            }
        } else {
            if block.hardness == -1.0 {
                return false;
            }
            match data.physics(state).push_reaction() {
                push::IMMOVABLE => return false,
                push::POPPED => return allow_destroy,
                push::PUSH_ONLY => return direction == connection,
                _ => {}
            }
        }
        block.block_entity.is_none()
    }

    fn resolver(&self, piston: BlockPos, facing: Direction, extending: bool) -> Resolver {
        let (push, start) = if extending {
            (facing, facing.offset(piston))
        } else {
            (opposite(facing), relative(piston, facing, 2))
        };
        Resolver {
            piston,
            facing,
            extending,
            start,
            push,
            to_push: Vec::new(),
            to_destroy: Vec::new(),
        }
    }

    /// `PistonStructureResolver.resolve`.
    fn resolve(&self, r: &mut Resolver) -> bool {
        r.to_push.clear();
        r.to_destroy.clear();
        let world = &self.context.world;
        let next = world.block(r.start);
        if !self.is_pushable(next, r.start, r.push, false, r.facing) {
            if r.extending && self.context.data.physics(next).push_reaction() == push::POPPED {
                r.to_destroy.push(r.start);
                return true;
            }
            return false;
        }
        if !self.add_block_line(r, r.start, r.push) {
            return false;
        }
        let mut i = 0;
        while i < r.to_push.len() {
            let pos = r.to_push[i];
            if self.is_sticky_block(world.block(pos)) && !self.add_branching_blocks(r, pos) {
                return false;
            }
            i += 1;
        }
        true
    }

    /// `addBlockLine`.
    fn add_block_line(&self, r: &mut Resolver, start: BlockPos, direction: Direction) -> bool {
        let world = &self.context.world;
        let mut next = world.block(start);
        if self.is_air(next)
            || !self.is_pushable(next, start, r.push, false, direction)
            || start == r.piston
            || r.to_push.contains(&start)
        {
            return true;
        }
        let mut count = 1;
        if count + r.to_push.len() > MAX_PUSH {
            return false;
        }
        let back = opposite(r.push);
        while self.is_sticky_block(next) {
            let pos = relative(start, back, count as i32);
            let previous = next;
            next = world.block(pos);
            if self.is_air(next)
                || !self.stick_together(previous, next)
                || !self.is_pushable(next, pos, r.push, false, back)
                || pos == r.piston
            {
                break;
            }
            count += 1;
            if count + r.to_push.len() > MAX_PUSH {
                return false;
            }
        }
        let mut added = 0;
        for i in (0..count).rev() {
            r.to_push.push(relative(start, back, i as i32));
            added += 1;
        }
        let mut i = 1;
        loop {
            let pos = relative(start, r.push, i);
            if let Some(collision) = r.to_push.iter().position(|&p| p == pos) {
                self.reorder_at_collision(r, added, collision);
                for j in 0..=collision + added {
                    let at = r.to_push[j];
                    if self.is_sticky_block(world.block(at)) && !self.add_branching_blocks(r, at) {
                        return false;
                    }
                }
                return true;
            }
            next = world.block(pos);
            if self.is_air(next) {
                return true;
            }
            if !self.is_pushable(next, pos, r.push, true, r.push) || pos == r.piston {
                return false;
            }
            if self.context.data.physics(next).push_reaction() == push::POPPED {
                r.to_destroy.push(pos);
                return true;
            }
            if r.to_push.len() >= MAX_PUSH {
                return false;
            }
            r.to_push.push(pos);
            added += 1;
            i += 1;
        }
    }

    /// `reorderListAtCollision`.
    fn reorder_at_collision(&self, r: &mut Resolver, added: usize, collision: usize) {
        let len = r.to_push.len();
        let head = r.to_push[..collision].to_vec();
        let last_line = r.to_push[len - added..].to_vec();
        let middle = r.to_push[collision..len - added].to_vec();
        r.to_push = head;
        r.to_push.extend(last_line);
        r.to_push.extend(middle);
    }

    /// `addBranchingBlocks`: slime and honey take what sticks to their
    /// sides along.
    fn add_branching_blocks(&self, r: &mut Resolver, from: BlockPos) -> bool {
        let world = &self.context.world;
        let from_state = world.block(from);
        for direction in DIRECTIONS {
            if axis(direction) == axis(r.push) {
                continue;
            }
            let neighbour = direction.offset(from);
            let neighbour_state = world.block(neighbour);
            if self.stick_together(neighbour_state, from_state)
                && !self.add_block_line(r, neighbour, direction)
            {
                return false;
            }
        }
        true
    }

    // --- The base ---------------------------------------------------------------

    /// `getNeighborSignal`: power from any side but the front, or into the
    /// block above (quasi-connectivity).
    fn piston_powered(&self, pos: BlockPos, facing: Direction) -> bool {
        for direction in DIRECTIONS {
            if direction != facing && self.signal(direction.offset(pos), direction) > 0 {
                return true;
            }
        }
        if self.signal(pos, Direction::Down) > 0 {
            return true;
        }
        let above = Direction::Up.offset(pos);
        DIRECTIONS
            .iter()
            .any(|&d| d != Direction::Down && self.signal(d.offset(above), d) > 0)
    }

    /// `checkIfExtend`.
    pub(super) fn piston_check(&mut self, pos: BlockPos, state: u32) {
        let Some(facing) = self.direction_value(state, "facing") else {
            return;
        };
        let extend = self.piston_powered(pos, facing);
        let extended = self.bool_value(state, "extended");
        let block = self.block_id(state);
        if extend && !extended {
            let mut resolver = self.resolver(pos, facing, true);
            if self.resolve(&mut resolver) {
                self.block_event(pos, block, EXTEND, facing as u8);
            }
        } else if !extend && extended {
            // A head still on its way out is dropped rather than pulled
            // back with its block.
            let pushed = relative(pos, facing, 2);
            let pushed_state = self.context.world.block(pushed);
            let mut event = CONTRACT;
            if self.kind(pushed_state) == Kind::MovingPiston
                && self.direction_value(pushed_state, "facing") == Some(facing)
                && let Some(moving) = self.moving_at(pushed)
                && moving.extending
                && (moving.progress < 0.5
                    || self.context.level.game_time == moving.last_ticked
                    || self.handling_tick)
            {
                event = DROP;
            }
            self.block_event(pos, block, event, facing as u8);
        }
    }

    /// `PistonBaseBlock.triggerEvent`: whether the event happened, so that
    /// clients are told.
    pub(super) fn piston_event(&mut self, pos: BlockPos, state: u32, b0: u8, b1: u8) -> bool {
        let Some(facing) = self.direction_value(state, "facing") else {
            return false;
        };
        let sticky = self.is(state, "minecraft:sticky_piston");
        let extended = self.with(state, "extended", "true");
        let extend = self.piston_powered(pos, facing);
        if extend && (b0 == CONTRACT || b0 == DROP) {
            self.set_block_with(pos, extended, update::CLIENTS);
            return false;
        }
        if !extend && b0 == EXTEND {
            return false;
        }
        if b0 == EXTEND {
            if !self.move_blocks(pos, facing, true, sticky) {
                return false;
            }
            self.set_block_with(
                pos,
                extended,
                update::MOVE_BY_PISTON | update::CLIENTS | update::NEIGHBORS,
            );
            let pitch = rand::random::<f32>() * 0.25 + 0.6;
            self.play_sound(
                None,
                "minecraft:block.piston.extend",
                center(pos),
                0.5,
                pitch,
            );
            return true;
        }
        let arm = facing.offset(pos);
        if self.moving_at(arm).is_some() {
            self.final_tick(arm);
        }
        let moving_state = self.moving_piston_state(facing, sticky);
        self.set_block_with(
            pos,
            moving_state,
            update::INVISIBLE | update::KNOWN_SHAPE | update::SKIP_BLOCK_ENTITY_SIDEEFFECTS,
        );
        let base = self.default_of(if sticky {
            "minecraft:sticky_piston"
        } else {
            "minecraft:piston"
        });
        let retracted = Direction::from_id(i32::from(b1 & 7))
            .map_or(base, |d| self.with(base, "facing", d.name()));
        self.start_moving(pos, retracted, facing, false, true);
        let moving_id = self.block_id(moving_state);
        self.update_neighbors_at(pos, moving_id);
        self.update_neighbour_shapes_of(pos, moving_state, update::CLIENTS);
        if sticky {
            let two = relative(pos, facing, 2);
            let moving_two = self.context.world.block(two);
            let mut piece = false;
            if self.kind(moving_two) == Kind::MovingPiston
                && let Some(entity) = self.moving_at(two)
                && entity.facing == facing
                && entity.extending
            {
                self.final_tick(two);
                piece = true;
            }
            if !piece {
                let pullable = b0 == CONTRACT
                    && !self.is_air(moving_two)
                    && self.is_pushable(moving_two, two, opposite(facing), false, facing)
                    && (self.context.data.physics(moving_two).push_reaction() == push::PUSH_PULL
                        || self.is_piston_base(moving_two));
                if pullable {
                    self.move_blocks(pos, facing, false, sticky);
                } else {
                    self.remove_block_keeping_fluid(arm);
                }
            }
        } else {
            self.remove_block_keeping_fluid(arm);
        }
        let pitch = rand::random::<f32>() * 0.15 + 0.6;
        self.play_sound(
            None,
            "minecraft:block.piston.contract",
            center(pos),
            0.5,
            pitch,
        );
        true
    }

    fn moving_piston_state(&self, facing: Direction, sticky: bool) -> u32 {
        let state = self.default_of("minecraft:moving_piston");
        let state = self.with(state, "facing", facing.name());
        self.with(state, "type", if sticky { "sticky" } else { "normal" })
    }

    /// Puts the block entity of a moving block at `pos`, where a
    /// `moving_piston` block was just set.
    fn start_moving(
        &mut self,
        pos: BlockPos,
        state: u32,
        facing: Direction,
        extending: bool,
        source: bool,
    ) {
        self.context.world.update_block_entity(pos, |entity| {
            entity.moving = Some(Moving {
                state,
                facing,
                progress: 0.0,
                progress_old: 0.0,
                extending,
                source,
                last_ticked: 0,
            });
        });
    }

    /// `moveBlocks`.
    fn move_blocks(
        &mut self,
        piston: BlockPos,
        facing: Direction,
        extending: bool,
        sticky: bool,
    ) -> bool {
        let arm = facing.offset(piston);
        if !extending && self.kind(self.context.world.block(arm)) == Kind::PistonHead {
            self.set_block_with(
                arm,
                self.air,
                update::INVISIBLE | update::KNOWN_SHAPE | update::SKIP_BLOCK_ENTITY_SIDEEFFECTS,
            );
        }
        let mut resolver = self.resolver(piston, facing, extending);
        if !self.resolve(&mut resolver) {
            return false;
        }
        let Resolver {
            to_push,
            to_destroy,
            push: push_direction,
            ..
        } = resolver;
        // In insertion order, as vanilla's hash map keeps them for the
        // updates below.
        let mut delete_after_move: Vec<(BlockPos, u32)> = to_push
            .iter()
            .map(|&p| (p, self.context.world.block(p)))
            .collect();
        let shapes: Vec<u32> = delete_after_move.iter().map(|&(_, s)| s).collect();
        let mut to_update = Vec::with_capacity(to_push.len() + to_destroy.len());
        for &pos in to_destroy.iter().rev() {
            let state = self.context.world.block(pos);
            self.drop_resources(pos, state);
            self.set_block_with(pos, self.air, update::KNOWN_SHAPE | update::CLIENTS);
            to_update.push(state);
        }
        for (i, &from) in to_push.iter().enumerate().rev() {
            let state = self.context.world.block(from);
            let pos = push_direction.offset(from);
            delete_after_move.retain(|&(p, _)| p != pos);
            let moving = self.with(
                self.default_of("minecraft:moving_piston"),
                "facing",
                facing.name(),
            );
            self.set_block_with(
                pos,
                moving,
                update::SKIP_BLOCK_ENTITY_SIDEEFFECTS | update::MOVE_BY_PISTON | update::INVISIBLE,
            );
            self.start_moving(pos, shapes[i], facing, extending, false);
            to_update.push(state);
        }
        if extending {
            let head = self.default_of("minecraft:piston_head");
            let head = self.with(head, "facing", facing.name());
            let head = self.with(head, "type", if sticky { "sticky" } else { "normal" });
            let moving = self.moving_piston_state(facing, sticky);
            delete_after_move.retain(|&(p, _)| p != arm);
            self.set_block_with(
                arm,
                moving,
                update::SKIP_BLOCK_ENTITY_SIDEEFFECTS | update::MOVE_BY_PISTON | update::INVISIBLE,
            );
            self.start_moving(arm, head, facing, true, true);
        }
        for &(pos, _) in &delete_after_move {
            self.set_block_with(
                pos,
                self.air,
                update::MOVE_BY_PISTON | update::KNOWN_SHAPE | update::CLIENTS,
            );
        }
        for &(pos, old) in &delete_after_move {
            self.update_indirect_shapes_of(pos, old, update::CLIENTS);
            self.update_neighbour_shapes_of(pos, self.air, update::CLIENTS);
        }
        let mut index = 0;
        for &pos in to_destroy.iter().rev() {
            let state = to_update[index];
            index += 1;
            self.after_removal_moved(pos, state, false);
            self.update_indirect_shapes_of(pos, state, update::CLIENTS);
            let id = self.block_id(state);
            self.update_neighbors_at(pos, id);
        }
        for &pos in to_push.iter().rev() {
            let id = self.block_id(to_update[index]);
            index += 1;
            self.update_neighbors_at(pos, id);
        }
        if extending {
            let head = self.block_id(self.default_of("minecraft:piston_head"));
            self.update_neighbors_at(arm, head);
        }
        true
    }

    // --- Moving blocks -------------------------------------------------------------

    /// `PistonMovingBlockEntity.tick`.
    pub(super) fn tick_moving(&mut self, pos: BlockPos) {
        let Some(mut moving) = self.moving_at(pos) else {
            return;
        };
        moving.last_ticked = self.context.level.game_time;
        moving.progress_old = moving.progress;
        if moving.progress_old >= 1.0 {
            self.land(pos, moving);
            return;
        }
        let progress = moving.progress + 0.5;
        self.push_entities(pos, &moving, progress);
        moving.progress = progress.min(1.0);
        self.context.world.change_block_entity(pos, |entity| {
            entity.moving = Some(moving);
            false
        });
    }

    /// The moving block becomes a block again (the end of `tick`).
    fn land(&mut self, pos: BlockPos, moving: Moving) {
        let state = self.context.world.block(pos);
        if self.kind(state) != Kind::MovingPiston {
            return;
        }
        let shaped = self.shaped_by_neighbours(pos, moving.state);
        if self.is_air(shaped) {
            self.set_block_with(
                pos,
                moving.state,
                update::INVISIBLE
                    | update::KNOWN_SHAPE
                    | update::MOVE_BY_PISTON
                    | update::SKIP_BLOCK_ENTITY_SIDEEFFECTS,
            );
            self.update_or_destroy_at(pos, moving.state, shaped, update::ALL);
        } else {
            let landed = if self.bool_value(shaped, "waterlogged") {
                self.with(shaped, "waterlogged", "false")
            } else {
                shaped
            };
            self.set_block_with(
                pos,
                landed,
                update::MOVE_BY_PISTON | update::CLIENTS | update::NEIGHBORS,
            );
            let id = self.block_id(landed);
            self.neighbor_changed_at(pos, id);
        }
    }

    /// `finalTick`: the move ends at once, as when the piston turns back
    /// before it is done.
    pub(super) fn final_tick(&mut self, pos: BlockPos) {
        let Some(moving) = self.moving_at(pos) else {
            return;
        };
        if moving.progress_old >= 1.0 {
            return;
        }
        // The block entity goes before the block changes.
        self.context.world.change_block_entity(pos, |entity| {
            entity.moving = None;
            false
        });
        if self.kind(self.context.world.block(pos)) != Kind::MovingPiston {
            return;
        }
        let new = if moving.source {
            self.air
        } else {
            self.shaped_by_neighbours(pos, moving.state)
        };
        self.set_block(pos, new);
        let id = self.block_id(new);
        self.neighbor_changed_at(pos, id);
    }

    /// `moveCollidedEntities`: mobs, items and lit TNT in the way of a
    /// moving block go along. Players move themselves: their clients see
    /// the piston too.
    fn push_entities(&mut self, pos: BlockPos, moving: &Moving, new_progress: f32) {
        let movement = if moving.extending {
            moving.facing
        } else {
            opposite(moving.facing)
        };
        let delta = f64::from(new_progress - moving.progress);
        // `getCollisionRelatedBlockState`: a retracting piston's own head.
        let related = if !moving.extending && moving.source && self.is_piston_base(moving.state) {
            let head = self.default_of("minecraft:piston_head");
            let head = self.with(
                head,
                "short",
                if moving.progress > 0.25 {
                    "true"
                } else {
                    "false"
                },
            );
            let sticky = self.is(moving.state, "minecraft:sticky_piston");
            let head = self.with(head, "type", if sticky { "sticky" } else { "normal" });
            let facing = self
                .value(moving.state, "facing")
                .unwrap_or("north")
                .to_owned();
            self.with(head, "facing", &facing)
        } else {
            moving.state
        };
        let shape = self.context.data.collision_shape(related).to_vec();
        if shape.is_empty() {
            return;
        }
        let extended = if moving.extending {
            moving.progress - 1.0
        } else {
            1.0 - moving.progress
        };
        let [sx, sy, sz] = steps(moving.facing, 1).map(f64::from);
        let offset = [
            f64::from(pos.x) + f64::from(extended) * sx,
            f64::from(pos.y) + f64::from(extended) * sy,
            f64::from(pos.z) + f64::from(extended) * sz,
        ];
        let areas: Vec<Aabb> = shape
            .iter()
            .map(|b| {
                let moved = [
                    b[0] + offset[0],
                    b[1] + offset[1],
                    b[2] + offset[2],
                    b[3] + offset[0],
                    b[4] + offset[1],
                    b[5] + offset[2],
                ];
                movement_area(moved, movement, delta)
            })
            .collect();
        let shift = |bounds: Aabb| -> Option<[f64; 3]> {
            let mut most: f64 = 0.0;
            for area in &areas {
                if intersects(area, &bounds) {
                    most = most.max(movement_into(area, movement, &bounds));
                    if most >= delta {
                        break;
                    }
                }
            }
            if most <= 0.0 {
                return None;
            }
            let distance = most.min(delta) + 0.01;
            Some(steps(movement, 1).map(|s| f64::from(s) * distance))
        };
        let mut moved = Vec::new();
        for (&id, mob) in &self.mobs {
            if mob.alive()
                && let Some(by) = shift(mob.bounds())
            {
                moved.push((id, by));
            }
        }
        for (id, by) in moved {
            if let Some(mob) = self.mobs.get_mut(&id) {
                for (axis, d) in by.iter().enumerate() {
                    mob.pos[axis] += d;
                }
            }
        }
        let mut moved = Vec::new();
        for (&id, item) in &self.items {
            if let Some(by) = shift(item.bounds()) {
                moved.push((id, by));
            }
        }
        for (id, by) in moved {
            self.shift_item(id, by);
        }
        for (id, bounds) in self.tnt_boxes() {
            if let Some(by) = shift(bounds) {
                self.shift_tnt(id, by);
            }
        }
    }

    // --- The head --------------------------------------------------------------------

    /// `PistonHeadBlock.isFittingBase`.
    fn fitting_base(&self, head: u32, base: u32) -> bool {
        let sticky = self.value(head, "type") == Some("sticky");
        let name = if sticky {
            "minecraft:sticky_piston"
        } else {
            "minecraft:piston"
        };
        self.is(base, name)
            && self.bool_value(base, "extended")
            && self.value(base, "facing") == self.value(head, "facing")
    }

    /// `PistonHeadBlock.canSurvive`.
    pub(super) fn head_survives(&self, pos: BlockPos, state: u32) -> bool {
        let Some(facing) = self.direction_value(state, "facing") else {
            return false;
        };
        let base = self.context.world.block(opposite(facing).offset(pos));
        self.fitting_base(state, base)
            || (self.kind(base) == Kind::MovingPiston
                && self.value(base, "facing") == self.value(state, "facing"))
    }

    /// `PistonHeadBlock.neighborChanged`: the base hears it.
    pub(super) fn head_neighbor_changed(&mut self, pos: BlockPos, state: u32, source: usize) {
        if self.head_survives(pos, state)
            && let Some(facing) = self.direction_value(state, "facing")
        {
            self.neighbor_changed_at(opposite(facing).offset(pos), source);
        }
    }

    /// `PistonHeadBlock.affectNeighborsAfterRemoval`: without its head the
    /// base breaks.
    pub(super) fn head_removed(&mut self, pos: BlockPos, state: u32) {
        let Some(facing) = self.direction_value(state, "facing") else {
            return;
        };
        let base = opposite(facing).offset(pos);
        if self.fitting_base(state, self.context.world.block(base)) {
            self.destroy_block(base, true);
        }
    }

    /// `PistonHeadBlock.playerWillDestroy`: in creative the base goes
    /// without drops.
    pub(super) fn head_broken_by_creative(&mut self, pos: BlockPos, state: u32) {
        let Some(facing) = self.direction_value(state, "facing") else {
            return;
        };
        let base = opposite(facing).offset(pos);
        if self.fitting_base(state, self.context.world.block(base)) {
            self.destroy_block(base, false);
        }
    }
}

/// `PistonMath.getMovementArea`: the slab a box sweeps through moving
/// `delta` towards `direction`.
fn movement_area(b: Aabb, direction: Direction, delta: f64) -> Aabb {
    let [min_x, min_y, min_z, max_x, max_y, max_z] = b;
    match direction {
        Direction::Up => [min_x, max_y, min_z, max_x, max_y + delta, max_z],
        Direction::Down => [min_x, min_y - delta, min_z, max_x, min_y, max_z],
        Direction::North => [min_x, min_y, min_z - delta, max_x, max_y, min_z],
        Direction::South => [min_x, min_y, max_z, max_x, max_y, max_z + delta],
        Direction::West => [min_x - delta, min_y, min_z, min_x, max_y, max_z],
        Direction::East => [max_x, min_y, min_z, max_x + delta, max_y, max_z],
    }
}

fn intersects(a: &Aabb, b: &Aabb) -> bool {
    (0..3).all(|i| a[i] < b[i + 3] && a[i + 3] > b[i])
}

/// `getMovement`: how far `b` must go to be out of `area`.
fn movement_into(area: &Aabb, direction: Direction, b: &Aabb) -> f64 {
    match direction {
        Direction::East => area[3] - b[0],
        Direction::West => b[3] - area[0],
        Direction::Up => area[4] - b[1],
        Direction::Down => b[4] - area[1],
        Direction::South => area[5] - b[2],
        Direction::North => b[5] - area[2],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moving_box_sweeps_ahead_of_itself() {
        let block = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        assert_eq!(
            movement_area(block, Direction::East, 0.5),
            [1.0, 0.0, 0.0, 1.5, 1.0, 1.0]
        );
        let player = [1.2, 0.0, 0.2, 1.8, 1.8, 0.8];
        let area = movement_area(block, Direction::East, 0.5);
        assert!(intersects(&area, &player));
        assert!((movement_into(&area, Direction::East, &player) - 0.3).abs() < 1e-9);
    }
}
