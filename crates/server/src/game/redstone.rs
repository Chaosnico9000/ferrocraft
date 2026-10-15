//! Redstone as vanilla's `SignalGetter` and the blocks around it define it:
//! levers, buttons, pressure plates, redstone torches and blocks give a
//! signal; wire carries it, one level less per block; solid blocks pass on
//! what powers them strongly; repeaters delay and refresh it. Lamps, doors,
//! trapdoors, fence gates and TNT react to it.

use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::Direction;

use super::Game;
use super::blocks::{
    DIRECTIONS, HORIZONTAL, Kind, above, below, center, is_horizontal, priority, update,
};
use crate::data::{Aabb, flags};
use crate::placement::{clockwise, counter_clockwise, opposite};

/// `RedstoneSide`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Up,
    Flat,
    None,
}

impl Side {
    fn name(self) -> &'static str {
        match self {
            Side::Up => "up",
            Side::Flat => "side",
            Side::None => "none",
        }
    }

    fn of(name: Option<&str>) -> Side {
        match name {
            Some("up") => Side::Up,
            Some("side") => Side::Flat,
            _ => Side::None,
        }
    }

    fn connected(self) -> bool {
        self != Side::None
    }
}

/// How a redstone torch's recent toggles are remembered
/// (`RedstoneTorchBlock.RECENT_TOGGLES`).
#[derive(Debug, Clone, Copy)]
pub struct Toggle {
    pos: BlockPos,
    when: u64,
}

impl Game {
    // --- Signals ---------------------------------------------------------------

    /// `getSignal` of the block itself: what it gives towards a block on
    /// its side opposite `direction` (vanilla's direction points from the
    /// asking block to this one).
    fn block_signal(&self, pos: BlockPos, state: u32, direction: Direction) -> i32 {
        match self.kind(state) {
            Kind::Wire => {
                if !self.wire_signals || direction == Direction::Down {
                    return 0;
                }
                let power = self.int_value(state, "power").unwrap_or(0);
                if power == 0 {
                    return 0;
                }
                if direction != Direction::Up {
                    let connections = self.wire_connections(pos, state);
                    if !self.wire_side(connections, opposite(direction)).connected() {
                        return 0;
                    }
                }
                power
            }
            Kind::RedstoneTorch => {
                if direction != Direction::Up && self.bool_value(state, "lit") {
                    15
                } else {
                    0
                }
            }
            Kind::RedstoneWallTorch => {
                if self.direction_value(state, "facing") != Some(direction)
                    && self.bool_value(state, "lit")
                {
                    15
                } else {
                    0
                }
            }
            Kind::Repeater | Kind::Comparator | Kind::Observer => {
                if self.direction_value(state, "facing") == Some(direction)
                    && self.bool_value(state, "powered")
                {
                    self.diode_output(pos, state)
                } else {
                    0
                }
            }
            Kind::Lever | Kind::Button => {
                if self.bool_value(state, "powered") {
                    15
                } else {
                    0
                }
            }
            Kind::PressurePlate | Kind::WeightedPressurePlate => self.plate_signal(state),
            Kind::RedstoneBlock => 15,
            // `TrappedChestBlock`: as many as look inside.
            Kind::TrappedChest => self.openers.get(&pos).map_or(0, |&n| n.min(15) as i32),
            _ => 0,
        }
    }

    /// `getOutputSignal`: a repeater or observer gives 15, a comparator
    /// what it worked out last.
    fn diode_output(&self, pos: BlockPos, state: u32) -> i32 {
        if self.kind(state) == Kind::Comparator {
            self.context
                .world
                .block_entity(pos)
                .and_then(|e| e.comparator)
                .unwrap_or(0)
        } else {
            15
        }
    }

    /// `getDirectSignal`: the strong signal into the block on the side
    /// opposite `direction`.
    fn direct_signal(&self, pos: BlockPos, state: u32, direction: Direction) -> i32 {
        match self.kind(state) {
            Kind::Wire => {
                if self.wire_signals {
                    self.block_signal(pos, state, direction)
                } else {
                    0
                }
            }
            Kind::RedstoneTorch | Kind::RedstoneWallTorch => {
                if direction == Direction::Down {
                    self.block_signal(pos, state, direction)
                } else {
                    0
                }
            }
            Kind::Repeater | Kind::Comparator | Kind::Observer => {
                self.block_signal(pos, state, direction)
            }
            Kind::TrappedChest if direction == Direction::Up => {
                self.block_signal(pos, state, direction)
            }
            Kind::Lever | Kind::Button => {
                let connected = self.attached_towards(state).map(opposite);
                if self.bool_value(state, "powered") && connected == Some(direction) {
                    15
                } else {
                    0
                }
            }
            Kind::PressurePlate | Kind::WeightedPressurePlate if direction == Direction::Up => {
                self.plate_signal(state)
            }
            _ => 0,
        }
    }

    /// `SignalGetter.getDirectSignalTo`: the strongest signal any neighbour
    /// powers `pos` with strongly.
    fn direct_signal_to(&self, pos: BlockPos) -> i32 {
        let mut best = 0;
        for direction in DIRECTIONS {
            let neighbour = direction.offset(pos);
            let state = self.context.world.block(neighbour);
            best = best.max(self.direct_signal(neighbour, state, direction));
            if best >= 15 {
                break;
            }
        }
        best
    }

    /// `SignalGetter.getSignal`: the signal at `pos` towards `direction`,
    /// a conductor's strong power included.
    pub(super) fn signal(&self, pos: BlockPos, direction: Direction) -> i32 {
        let state = self.context.world.block(pos);
        let signal = self.block_signal(pos, state, direction);
        if self.flag(state, flags::CONDUCTOR) {
            signal.max(self.direct_signal_to(pos))
        } else {
            signal
        }
    }

    /// `hasNeighborSignal`.
    pub(super) fn has_neighbor_signal(&self, pos: BlockPos) -> bool {
        DIRECTIONS
            .iter()
            .any(|&d| self.signal(d.offset(pos), d) > 0)
    }

    /// `getBestNeighborSignal`.
    fn best_neighbor_signal(&self, pos: BlockPos) -> i32 {
        let mut best = 0;
        for direction in DIRECTIONS {
            let signal = self.signal(direction.offset(pos), direction);
            if signal >= 15 {
                return 15;
            }
            best = best.max(signal);
        }
        best
    }

    /// `getControlInputSignal`: what a repeater's side takes for a lock.
    fn control_input_signal(&self, pos: BlockPos, direction: Direction, diodes_only: bool) -> i32 {
        let state = self.context.world.block(pos);
        if diodes_only {
            return if matches!(self.kind(state), Kind::Repeater | Kind::Comparator) {
                self.direct_signal(pos, state, direction)
            } else {
                0
            };
        }
        match self.kind(state) {
            Kind::RedstoneBlock => 15,
            Kind::Wire => self.int_value(state, "power").unwrap_or(0),
            _ if self.flag(state, flags::SIGNAL_SOURCE) => {
                self.direct_signal(pos, state, direction)
            }
            _ => 0,
        }
    }

    // --- Wire ------------------------------------------------------------------------

    fn wire_side(&self, state: u32, direction: Direction) -> Side {
        Side::of(self.value(state, direction.name()))
    }

    fn set_wire_side(&self, state: u32, direction: Direction, side: Side) -> u32 {
        self.with(state, direction.name(), side.name())
    }

    fn is_dot(&self, state: u32) -> bool {
        HORIZONTAL
            .iter()
            .all(|&d| !self.wire_side(state, d).connected())
    }

    fn is_cross(&self, state: u32) -> bool {
        HORIZONTAL
            .iter()
            .all(|&d| self.wire_side(state, d).connected())
    }

    /// The wire with all four sides, and `power`.
    fn wire_cross(&self, power: i32) -> u32 {
        let mut state = self.default_of("minecraft:redstone_wire");
        for direction in HORIZONTAL {
            state = self.set_wire_side(state, direction, Side::Flat);
        }
        self.with(state, "power", &power.to_string())
    }

    /// `canSurviveOn`.
    pub(super) fn wire_survives_on(&self, below_state: u32) -> bool {
        self.context
            .data
            .physics(below_state)
            .sturdy(Direction::Up as usize)
            || self.is(below_state, "minecraft:hopper")
    }

    /// `shouldConnectTo`.
    fn wire_connects_to(&self, state: u32, direction: Option<Direction>) -> bool {
        match self.kind(state) {
            Kind::Wire => true,
            Kind::Repeater => {
                let facing = self.direction_value(state, "facing");
                direction.is_some_and(|d| facing == Some(d) || facing == Some(opposite(d)))
            }
            Kind::Observer => {
                direction.is_some() && direction == self.direction_value(state, "facing")
            }
            _ => self.flag(state, flags::SIGNAL_SOURCE) && direction.is_some(),
        }
    }

    /// `getConnectingSide`.
    fn connecting_side(&self, pos: BlockPos, direction: Direction, can_connect_up: bool) -> Side {
        let world = &self.context.world;
        let relative = direction.offset(pos);
        let relative_state = world.block(relative);
        if can_connect_up {
            let placeable_above = self.kind(relative_state) == Kind::Trapdoor
                || self.wire_survives_on(relative_state);
            if placeable_above && self.wire_connects_to(world.block(above(relative)), None) {
                if self.sturdy(relative, opposite(direction)) {
                    return Side::Up;
                }
                return Side::Flat;
            }
        }
        let conductor = self.flag(relative_state, flags::CONDUCTOR);
        if !self.wire_connects_to(relative_state, Some(direction))
            && (conductor || !self.wire_connects_to(world.block(below(relative)), None))
        {
            Side::None
        } else {
            Side::Flat
        }
    }

    fn can_connect_up(&self, pos: BlockPos) -> bool {
        !self.conductor(above(pos))
    }

    /// `getMissingConnections`.
    fn missing_connections(&self, pos: BlockPos, state: u32) -> u32 {
        let up = self.can_connect_up(pos);
        let mut state = state;
        for direction in HORIZONTAL {
            if !self.wire_side(state, direction).connected() {
                state =
                    self.set_wire_side(state, direction, self.connecting_side(pos, direction, up));
            }
        }
        state
    }

    /// `getConnectionState`: a lone dot stays a dot; a wire with one side
    /// runs straight through.
    fn wire_connections(&self, pos: BlockPos, state: u32) -> u32 {
        let was_dot = self.is_dot(state);
        let power = self.value(state, "power").unwrap_or("0").to_owned();
        let base = self.with(self.default_of("minecraft:redstone_wire"), "power", &power);
        let mut state = self.missing_connections(pos, base);
        if was_dot && self.is_dot(state) {
            return state;
        }
        let side = |s, d| self.wire_side(s, d).connected();
        let (north, south, east, west) = (
            side(state, Direction::North),
            side(state, Direction::South),
            side(state, Direction::East),
            side(state, Direction::West),
        );
        let north_south_empty = !north && !south;
        let east_west_empty = !east && !west;
        if !west && north_south_empty {
            state = self.set_wire_side(state, Direction::West, Side::Flat);
        }
        if !east && north_south_empty {
            state = self.set_wire_side(state, Direction::East, Side::Flat);
        }
        if !north && east_west_empty {
            state = self.set_wire_side(state, Direction::North, Side::Flat);
        }
        if !south && east_west_empty {
            state = self.set_wire_side(state, Direction::South, Side::Flat);
        }
        state
    }

    /// The state a placed wire gets (`getStateForPlacement`).
    pub(super) fn wire_placement(&self, pos: BlockPos) -> u32 {
        self.wire_connections(pos, self.wire_cross(0))
    }

    /// `RedstoneWireBlock.updateShape`.
    pub(super) fn wire_shape(
        &self,
        pos: BlockPos,
        state: u32,
        direction: Direction,
        _neighbour: BlockPos,
        neighbour_state: u32,
    ) -> u32 {
        match direction {
            Direction::Down => {
                if self.wire_survives_on(neighbour_state) {
                    state
                } else {
                    self.air
                }
            }
            Direction::Up => self.wire_connections(pos, state),
            side => {
                let connection = self.connecting_side(pos, side, self.can_connect_up(pos));
                if connection.connected() == self.wire_side(state, side).connected()
                    && !self.is_cross(state)
                {
                    self.set_wire_side(state, side, connection)
                } else {
                    let power = self.int_value(state, "power").unwrap_or(0);
                    let cross = self.set_wire_side(self.wire_cross(power), side, connection);
                    self.wire_connections(pos, cross)
                }
            }
        }
    }

    /// `updateIndirectNeighbourShapes`: wires one block up or down
    /// diagonally re-check their connection.
    pub(super) fn wire_indirect_shapes(
        &mut self,
        pos: BlockPos,
        state: u32,
        flags: u32,
        limit: u32,
    ) {
        let _ = (flags, limit);
        for direction in HORIZONTAL {
            if self.wire_side(state, direction) == Side::None {
                continue;
            }
            let side = direction.offset(pos);
            if self.kind(self.context.world.block(side)) == Kind::Wire {
                continue;
            }
            for dy in [-1, 1] {
                let diagonal = super::blocks::offset(side, 0, dy, 0);
                let diagonal_state = self.context.world.block(diagonal);
                if self.kind(diagonal_state) == Kind::Wire {
                    let towards = opposite(direction);
                    let updated = self.wire_shape(
                        diagonal,
                        diagonal_state,
                        towards,
                        towards.offset(diagonal),
                        self.context.world.block(towards.offset(diagonal)),
                    );
                    if updated != diagonal_state {
                        if self.is_air(updated) {
                            self.destroy_block(diagonal, true);
                        } else {
                            self.set_block_with(diagonal, updated, update::CLIENTS);
                        }
                    }
                }
            }
        }
    }

    /// `RedstoneWireBlock.getBlockSignal`: what powers the wire, other
    /// wires left out.
    fn wire_block_signal(&mut self, pos: BlockPos) -> i32 {
        self.wire_signals = false;
        let signal = self.best_neighbor_signal(pos);
        self.wire_signals = true;
        signal
    }

    fn wire_power(&self, state: u32) -> i32 {
        if self.kind(state) == Kind::Wire {
            self.int_value(state, "power").unwrap_or(0)
        } else {
            0
        }
    }

    /// `getIncomingWireSignal`: the strongest neighbouring wire, also one
    /// up or down, less one.
    fn incoming_wire_signal(&self, pos: BlockPos) -> i32 {
        let world = &self.context.world;
        let mut signal = 0;
        let above_conducts = self.conductor(above(pos));
        for direction in HORIZONTAL {
            let neighbour = direction.offset(pos);
            let state = world.block(neighbour);
            signal = signal.max(self.wire_power(state));
            let conducts = self.flag(state, flags::CONDUCTOR);
            if conducts && !above_conducts {
                signal = signal.max(self.wire_power(world.block(above(neighbour))));
            } else if !conducts {
                signal = signal.max(self.wire_power(world.block(below(neighbour))));
            }
        }
        (signal - 1).max(0)
    }

    /// `DefaultRedstoneWireEvaluator.updatePowerStrength`.
    fn update_wire_power(&mut self, pos: BlockPos, state: u32) {
        let block_signal = self.wire_block_signal(pos);
        let target = if block_signal == 15 {
            15
        } else {
            block_signal.max(self.incoming_wire_signal(pos))
        };
        if self.int_value(state, "power") == Some(target) {
            return;
        }
        if self.context.world.block(pos) == state {
            let powered = self.with(state, "power", &target.to_string());
            self.set_block_with(pos, powered, update::CLIENTS);
        }
        let wire = self.block_id(state);
        self.update_neighbors_at(pos, wire);
        for direction in DIRECTIONS {
            self.update_neighbors_at(direction.offset(pos), wire);
        }
    }

    pub(super) fn wire_on_place(&mut self, pos: BlockPos, state: u32) {
        self.update_wire_power(pos, state);
        let wire = self.block_id(state);
        self.update_neighbors_at(above(pos), wire);
        self.update_neighbors_at(below(pos), wire);
        self.update_neighbouring_wires(pos, wire);
    }

    pub(super) fn wire_after_removal(&mut self, pos: BlockPos, old: u32) {
        let wire = self.block_id(old);
        for direction in DIRECTIONS {
            self.update_neighbors_at(direction.offset(pos), wire);
        }
        self.update_wire_power(pos, old);
        self.update_neighbouring_wires(pos, wire);
    }

    /// `updateNeighborsOfNeighboringWires`.
    fn update_neighbouring_wires(&mut self, pos: BlockPos, wire: usize) {
        for direction in HORIZONTAL {
            self.check_corner(direction.offset(pos), wire);
        }
        for direction in HORIZONTAL {
            let target = direction.offset(pos);
            if self.conductor(target) {
                self.check_corner(above(target), wire);
            } else {
                self.check_corner(below(target), wire);
            }
        }
    }

    /// `checkCornerChangeAt`.
    fn check_corner(&mut self, pos: BlockPos, wire: usize) {
        if self.kind(self.context.world.block(pos)) == Kind::Wire {
            self.update_neighbors_at(pos, wire);
            for direction in DIRECTIONS {
                self.update_neighbors_at(direction.offset(pos), wire);
            }
        }
    }

    pub(super) fn wire_neighbor_changed(&mut self, pos: BlockPos, state: u32) {
        if self.can_survive(pos, state) {
            self.update_wire_power(pos, state);
        } else {
            self.drop_resources(pos, state);
            self.set_block(pos, self.air);
        }
    }

    /// `RedstoneWireBlock.useWithoutItem`: a dot becomes a cross and back.
    pub(super) fn toggle_wire(&mut self, pos: BlockPos, state: u32) -> bool {
        if !self.is_cross(state) && !self.is_dot(state) {
            return false;
        }
        let power = self.int_value(state, "power").unwrap_or(0);
        let new = if self.is_cross(state) {
            self.with(
                self.default_of("minecraft:redstone_wire"),
                "power",
                &power.to_string(),
            )
        } else {
            self.wire_cross(power)
        };
        let new = self.wire_connections(pos, new);
        if new == state {
            return false;
        }
        self.set_block(pos, new);
        let wire = self.block_id(state);
        for direction in HORIZONTAL {
            let relative = direction.offset(pos);
            if self.wire_side(state, direction).connected()
                != self.wire_side(new, direction).connected()
                && self.conductor(relative)
            {
                self.update_neighbors_except(relative, wire, opposite(direction));
            }
        }
        true
    }

    // --- Torches ---------------------------------------------------------------------

    /// `RedstoneTorchBlock.notifyNeighbors`.
    pub(super) fn torch_notify(&mut self, pos: BlockPos, state: u32) {
        let torch = self.block_id(state);
        for direction in DIRECTIONS {
            self.update_neighbors_at(direction.offset(pos), torch);
        }
    }

    /// Whether the block the torch stands on or hangs on is powered.
    fn torch_powered(&self, pos: BlockPos, state: u32) -> bool {
        if self.kind(state) == Kind::RedstoneWallTorch {
            let Some(facing) = self.direction_value(state, "facing") else {
                return false;
            };
            let back = opposite(facing);
            self.signal(back.offset(pos), back) > 0
        } else {
            self.signal(below(pos), Direction::Down) > 0
        }
    }

    pub(super) fn torch_neighbor_changed(&mut self, pos: BlockPos, state: u32) {
        if self.bool_value(state, "lit") == self.torch_powered(pos, state)
            && !self.block_tick_runs_now(pos, state)
        {
            self.schedule_block_tick(pos, state, 2, priority::NORMAL);
        }
    }

    /// Whether the torch at `pos` toggled 8 times in the last 60 ticks.
    fn toggled_too_often(&mut self, pos: BlockPos, add: bool) -> bool {
        if add {
            self.torch_toggles.push(Toggle {
                pos,
                when: self.context.level.game_time as u64,
            });
        }
        self.torch_toggles.iter().filter(|t| t.pos == pos).count() >= 8
    }

    /// `RedstoneTorchBlock.tick`: it goes out when its block is powered,
    /// and burns out for a while if that happens too fast.
    pub(super) fn torch_tick(&mut self, pos: BlockPos, state: u32) {
        let powered = self.torch_powered(pos, state);
        let now = self.context.level.game_time as u64;
        self.torch_toggles.retain(|t| now - t.when <= 60);
        if self.bool_value(state, "lit") {
            if powered {
                let out = self.with(state, "lit", "false");
                self.set_block(pos, out);
                if self.toggled_too_often(pos, true) {
                    self.level_event(1502, pos, 0);
                    self.schedule_block_tick(pos, out, 160, priority::NORMAL);
                }
            }
        } else if !powered && !self.toggled_too_often(pos, false) {
            let lit = self.with(state, "lit", "true");
            self.set_block(pos, lit);
        }
    }

    // --- Repeaters and comparators (`DiodeBlock`) -------------------------------------

    fn repeater_delay(&self, state: u32) -> u64 {
        if self.kind(state) == Kind::Comparator {
            return 2;
        }
        self.int_value(state, "delay").unwrap_or(1) as u64 * 2
    }

    /// `DiodeBlock.getInputSignal`: from behind; a wire counts with its
    /// own power.
    fn repeater_input(&self, pos: BlockPos, state: u32) -> i32 {
        let Some(facing) = self.direction_value(state, "facing") else {
            return 0;
        };
        let behind = facing.offset(pos);
        let input = self.signal(behind, facing);
        if input >= 15 {
            return input;
        }
        input.max(self.wire_power(self.context.world.block(behind)))
    }

    /// `ComparatorBlock.getInputSignal`: what is behind may be read
    /// instead, also through a solid block.
    fn comparator_input(&self, pos: BlockPos, state: u32) -> i32 {
        let mut signal = self.repeater_input(pos, state);
        let Some(facing) = self.direction_value(state, "facing") else {
            return signal;
        };
        let world = &self.context.world;
        let target = facing.offset(pos);
        let target_state = world.block(target);
        if self.flag(target_state, flags::ANALOG) {
            signal = self.analog_output(target, target_state, opposite(facing));
        } else if signal < 15 && self.flag(target_state, flags::CONDUCTOR) {
            let further = facing.offset(target);
            let further_state = world.block(further);
            if self.flag(further_state, flags::ANALOG) {
                signal = self.analog_output(further, further_state, opposite(facing));
            }
        }
        signal
    }

    /// `getAlternateSignal`: the stronger side.
    fn alternate_signal(&self, pos: BlockPos, state: u32) -> i32 {
        let Some(facing) = self.direction_value(state, "facing") else {
            return 0;
        };
        let (cw, ccw) = (clockwise(facing), counter_clockwise(facing));
        let diodes_only = self.kind(state) == Kind::Repeater;
        self.control_input_signal(cw.offset(pos), cw, diodes_only)
            .max(self.control_input_signal(ccw.offset(pos), ccw, diodes_only))
    }

    fn subtracts(&self, state: u32) -> bool {
        self.value(state, "mode") == Some("subtract")
    }

    /// `shouldTurnOn`.
    fn diode_should_turn_on(&self, pos: BlockPos, state: u32) -> bool {
        if self.kind(state) != Kind::Comparator {
            return self.repeater_input(pos, state) > 0;
        }
        let input = self.comparator_input(pos, state);
        if input == 0 {
            return false;
        }
        let side = self.alternate_signal(pos, state);
        input > side || (input == side && !self.subtracts(state))
    }

    /// `ComparatorBlock.calculateOutputSignal`.
    fn comparator_output(&self, pos: BlockPos, state: u32) -> i32 {
        let input = self.comparator_input(pos, state);
        if input == 0 {
            return 0;
        }
        let side = self.alternate_signal(pos, state);
        if side > input {
            0
        } else if self.subtracts(state) {
            input - side
        } else {
            input
        }
    }

    /// `RepeaterBlock.isLocked`: a powered repeater at its side.
    pub(super) fn repeater_locked(&self, pos: BlockPos, state: u32) -> bool {
        self.kind(state) == Kind::Repeater && self.alternate_signal(pos, state) > 0
    }

    pub(super) fn repeater_placement(&self, pos: BlockPos, state: u32) -> u32 {
        let locked = self.repeater_locked(pos, state);
        self.with(state, "locked", if locked { "true" } else { "false" })
    }

    /// `RepeaterBlock` and `ComparatorBlock.updateShape`.
    pub(super) fn diode_shape(
        &self,
        pos: BlockPos,
        state: u32,
        direction: Direction,
        neighbour_state: u32,
    ) -> u32 {
        if direction == Direction::Down
            && !self
                .context
                .data
                .physics(neighbour_state)
                .sturdy(Direction::Up as usize)
        {
            return self.air;
        }
        if self.kind(state) == Kind::Comparator {
            return state;
        }
        let facing = self.direction_value(state, "facing");
        let same_axis = facing.is_some_and(|f| f == direction || opposite(f) == direction);
        if is_horizontal(direction) && !same_axis {
            return self.repeater_placement(pos, state);
        }
        state
    }

    /// `DiodeBlock.tick`.
    pub(super) fn repeater_tick(&mut self, pos: BlockPos, state: u32) {
        if self.repeater_locked(pos, state) {
            return;
        }
        let on = self.bool_value(state, "powered");
        let should = self.diode_should_turn_on(pos, state);
        if on && !should {
            let off = self.with(state, "powered", "false");
            self.set_block_with(pos, off, update::CLIENTS);
        } else if !on {
            let powered = self.with(state, "powered", "true");
            self.set_block_with(pos, powered, update::CLIENTS);
            if !should {
                let delay = self.repeater_delay(state);
                self.schedule_block_tick(pos, powered, delay, priority::VERY_HIGH);
            }
        }
    }

    /// `DiodeBlock.neighborChanged`.
    pub(super) fn diode_neighbor_changed(&mut self, pos: BlockPos, state: u32) {
        if !self.can_survive(pos, state) {
            self.drop_resources(pos, state);
            self.set_block(pos, self.air);
            let id = self.block_id(state);
            for direction in DIRECTIONS {
                self.update_neighbors_at(direction.offset(pos), id);
            }
            return;
        }
        if self.kind(state) == Kind::Comparator {
            self.comparator_check(pos, state);
            return;
        }
        if self.repeater_locked(pos, state) {
            return;
        }
        let on = self.bool_value(state, "powered");
        let should = self.diode_should_turn_on(pos, state);
        if on != should && !self.block_tick_runs_now(pos, state) {
            let priority = if self.diode_prioritized(pos, state) {
                priority::EXTREMELY_HIGH
            } else if on {
                priority::VERY_HIGH
            } else {
                priority::HIGH
            };
            let delay = self.repeater_delay(state);
            self.schedule_block_tick(pos, state, delay, priority);
        }
    }

    /// `shouldPrioritize`: a repeater or comparator in front that does not
    /// face back.
    fn diode_prioritized(&self, pos: BlockPos, state: u32) -> bool {
        let Some(facing) = self.direction_value(state, "facing") else {
            return false;
        };
        let front_state = self.context.world.block(opposite(facing).offset(pos));
        matches!(self.kind(front_state), Kind::Repeater | Kind::Comparator)
            && self.direction_value(front_state, "facing") != Some(opposite(facing))
    }

    /// `ComparatorBlock.checkTickOnNeighbor`.
    fn comparator_check(&mut self, pos: BlockPos, state: u32) {
        if self.block_tick_runs_now(pos, state) {
            return;
        }
        let output = self.comparator_output(pos, state);
        let old = self.diode_output(pos, state);
        if output != old
            || self.bool_value(state, "powered") != self.diode_should_turn_on(pos, state)
        {
            let priority = if self.diode_prioritized(pos, state) {
                priority::HIGH
            } else {
                priority::NORMAL
            };
            self.schedule_block_tick(pos, state, 2, priority);
        }
    }

    /// `ComparatorBlock.refreshOutputState`, its tick.
    pub(super) fn comparator_refresh(&mut self, pos: BlockPos, state: u32) {
        let output = self.comparator_output(pos, state);
        let mut old = 0;
        self.context.world.change_block_entity(pos, |entity| {
            let Some(signal) = &mut entity.comparator else {
                return false;
            };
            old = *signal;
            *signal = output;
            old != output
        });
        if old != output || !self.subtracts(state) {
            let should = self.diode_should_turn_on(pos, state);
            let on = self.bool_value(state, "powered");
            if on && !should {
                let off = self.with(state, "powered", "false");
                self.set_block_with(pos, off, update::CLIENTS);
            } else if !on && should {
                let powered = self.with(state, "powered", "true");
                self.set_block_with(pos, powered, update::CLIENTS);
            }
            self.diode_update_front(pos, state);
        }
    }

    /// `ComparatorBlock.useWithoutItem`: compare and subtract in turn.
    pub(super) fn cycle_comparator(&mut self, pos: BlockPos, state: u32) {
        let subtract = !self.subtracts(state);
        let next = self.with(state, "mode", if subtract { "subtract" } else { "compare" });
        let pitch = if subtract { 0.55 } else { 0.5 };
        self.play_sound(
            None,
            "minecraft:block.comparator.click",
            center(pos),
            0.3,
            pitch,
        );
        self.set_block_with(pos, next, update::CLIENTS);
        if self.kind(self.context.world.block(pos)) == Kind::Comparator {
            self.comparator_refresh(pos, next);
        }
    }

    /// `DiodeBlock.updateNeighborsInFront`.
    pub(super) fn diode_update_front(&mut self, pos: BlockPos, state: u32) {
        let Some(facing) = self.direction_value(state, "facing") else {
            return;
        };
        let front = opposite(facing).offset(pos);
        let id = self.block_id(state);
        self.neighbor_changed_at(front, id);
        self.update_neighbors_except(front, id, facing);
    }

    /// `RepeaterBlock.useWithoutItem`: the next delay.
    pub(super) fn cycle_repeater(&mut self, pos: BlockPos, state: u32) {
        let delay = self.int_value(state, "delay").unwrap_or(1) % 4 + 1;
        let next = self.with(state, "delay", &delay.to_string());
        self.set_block(pos, next);
    }

    /// `DiodeBlock.setPlacedBy`.
    pub(super) fn repeater_placed(&mut self, pos: BlockPos, state: u32) {
        if self.diode_should_turn_on(pos, state) {
            self.schedule_block_tick(pos, state, 1, priority::NORMAL);
        }
    }

    // --- Observers -----------------------------------------------------------------------

    /// `ObserverBlock.updateShape`: a change in front starts a pulse.
    pub(super) fn observer_shape(&mut self, pos: BlockPos, state: u32, direction: Direction) {
        if self.direction_value(state, "facing") == Some(direction)
            && !self.bool_value(state, "powered")
            && !self.block_tick_scheduled(pos, state)
        {
            self.schedule_block_tick(pos, state, 2, priority::NORMAL);
        }
    }

    /// `ObserverBlock.tick`: on for two ticks, then off.
    pub(super) fn observer_tick(&mut self, pos: BlockPos, state: u32) {
        let next = if self.bool_value(state, "powered") {
            self.with(state, "powered", "false")
        } else {
            let on = self.with(state, "powered", "true");
            self.schedule_block_tick(pos, on, 2, priority::NORMAL);
            on
        };
        self.set_block_with(pos, next, update::CLIENTS);
        self.diode_update_front(pos, state);
    }

    /// `ObserverBlock.onPlace`: one placed powered goes off at once.
    pub(super) fn observer_on_place(&mut self, pos: BlockPos, state: u32) {
        if self.bool_value(state, "powered") && !self.block_tick_scheduled(pos, state) {
            let off = self.with(state, "powered", "false");
            self.set_block_with(pos, off, update::CLIENTS | update::KNOWN_SHAPE);
            self.diode_update_front(pos, off);
        }
    }

    /// `ObserverBlock.affectNeighborsAfterRemoval`.
    pub(super) fn observer_removed(&mut self, pos: BlockPos, state: u32) {
        if self.bool_value(state, "powered") && self.block_tick_scheduled(pos, state) {
            let off = self.with(state, "powered", "false");
            self.diode_update_front(pos, off);
        }
    }

    // --- What comparators read -----------------------------------------------------

    /// `getAnalogOutputSignal`.
    pub(super) fn analog_output(&self, pos: BlockPos, state: u32, _direction: Direction) -> i32 {
        let int = |key| self.int_value(state, key).unwrap_or(0);
        match self.kind(state) {
            Kind::TrappedChest => return self.container_signal(&self.chest_halves(pos, false)),
            Kind::Hopper | Kind::Dispenser | Kind::Dropper => {
                return self.container_signal(&[pos]);
            }
            _ => {}
        }
        let data = &self.context.data;
        let Some((_, block)) = data.block_of(state) else {
            return 0;
        };
        let name = block.name.as_str();
        match name {
            "minecraft:chest" => self.container_signal(&self.chest_halves(pos, false)),
            "minecraft:cake" => (7 - int("bites")) * 2,
            "minecraft:composter"
            | "minecraft:water_cauldron"
            | "minecraft:powder_snow_cauldron" => int("level"),
            "minecraft:lava_cauldron" => 3,
            "minecraft:end_portal_frame" => {
                if self.bool_value(state, "eye") {
                    15
                } else {
                    0
                }
            }
            "minecraft:respawn_anchor" => (int("charges") as f32 / 4.0 * 15.0).floor() as i32,
            _ if name.ends_with("candle_cake") => 14,
            _ if name.ends_with("copper_bulb") => {
                if self.bool_value(state, "lit") {
                    15
                } else {
                    0
                }
            }
            _ if block.block_entity.is_some() => self.container_signal(&[pos]),
            _ => 0,
        }
    }

    /// `getRedstoneSignalFromContainer` over the containers that make one
    /// (both halves of a double chest).
    pub(super) fn container_signal(&self, containers: &[BlockPos]) -> i32 {
        let data = &self.context.data;
        let mut total = 0.0f32;
        let mut size = 0;
        for &pos in containers {
            let Some(entity) = self.context.world.block_entity(pos) else {
                continue;
            };
            size += entity.items.len();
            for stack in entity.items.iter().flatten() {
                let max = data.item(stack.item).map_or(64, |i| i.stack).clamp(1, 99);
                total += stack.count as f32 / max as f32;
            }
        }
        if size == 0 {
            return 0;
        }
        let fill = total / size as f32;
        // `Mth.lerpDiscrete(fill, 0, 15)`.
        (fill * 14.0).floor() as i32 + i32::from(fill > 0.0)
    }

    /// `Level.updateNeighbourForOutputSignal`: comparators next to `pos`,
    /// or behind a solid block next to it, look again.
    pub(super) fn update_neighbour_for_output_signal(&mut self, pos: BlockPos, source: usize) {
        for direction in HORIZONTAL {
            let mut neighbour = direction.offset(pos);
            let mut state = self.context.world.block(neighbour);
            if self.kind(state) != Kind::Comparator && self.flag(state, flags::CONDUCTOR) {
                neighbour = direction.offset(neighbour);
                state = self.context.world.block(neighbour);
            }
            if self.kind(state) == Kind::Comparator {
                self.diode_neighbor_changed(neighbour, state);
                let _ = source;
            }
        }
    }

    /// `BlockEntity.setChanged` of a container: comparators reading it
    /// look again.
    pub(super) fn container_changed(&mut self, pos: BlockPos) {
        let state = self.context.world.block(pos);
        if !self.is_air(state) {
            let id = self.block_id(state);
            self.update_neighbour_for_output_signal(pos, id);
        }
    }

    // --- Levers and buttons -------------------------------------------------------

    /// `LeverBlock` and `ButtonBlock.updateNeighbours`: the block itself and
    /// the one it hangs on.
    pub(super) fn attached_update(&mut self, pos: BlockPos, state: u32) {
        let id = self.block_id(state);
        self.update_neighbors_at(pos, id);
        if let Some(towards) = self.attached_towards(state) {
            self.update_neighbors_at(towards.offset(pos), id);
        }
    }

    /// `LeverBlock.pull`.
    pub(super) fn pull_lever(&mut self, pos: BlockPos, state: u32) {
        let powered = self.bool_value(state, "powered");
        let pulled = self.with(state, "powered", if powered { "false" } else { "true" });
        self.set_block(pos, pulled);
        self.attached_update(pos, pulled);
        let pitch = if powered { 0.5 } else { 0.6 };
        self.play_sound(None, "minecraft:block.lever.click", center(pos), 0.3, pitch);
    }

    fn button_sound(&self, state: u32, on: bool) -> String {
        let name = self
            .context
            .data
            .block_of(state)
            .map(|(_, b)| b.name.clone())
            .unwrap_or_default();
        let kind = if name.contains("stone") || name.contains("blackstone") {
            "stone_button"
        } else {
            "wooden_button"
        };
        format!(
            "minecraft:block.{kind}.click_{}",
            if on { "on" } else { "off" }
        )
    }

    fn button_ticks(&self, state: u32) -> u64 {
        let name = self
            .context
            .data
            .block_of(state)
            .map(|(_, b)| b.name.clone())
            .unwrap_or_default();
        if name.contains("stone") || name.contains("blackstone") {
            20
        } else {
            30
        }
    }

    /// `ButtonBlock.press`.
    pub(super) fn press_button(&mut self, pos: BlockPos, state: u32) {
        let pressed = self.with(state, "powered", "true");
        self.set_block(pos, pressed);
        self.attached_update(pos, pressed);
        let ticks = self.button_ticks(state);
        self.schedule_block_tick(pos, pressed, ticks, priority::NORMAL);
        let sound = self.button_sound(state, true);
        self.play_sound(None, &sound, center(pos), 1.0, 1.0);
    }

    /// `ButtonBlock.tick`: it pops out again.
    pub(super) fn button_tick(&mut self, pos: BlockPos, state: u32) {
        if !self.bool_value(state, "powered") {
            return;
        }
        let released = self.with(state, "powered", "false");
        self.set_block(pos, released);
        self.attached_update(pos, released);
        let sound = self.button_sound(state, false);
        self.play_sound(None, &sound, center(pos), 1.0, 1.0);
    }

    // --- Pressure plates ---------------------------------------------------------

    pub(super) fn plate_signal(&self, state: u32) -> i32 {
        if self.kind(state) == Kind::WeightedPressurePlate {
            self.int_value(state, "power").unwrap_or(0)
        } else if self.bool_value(state, "powered") {
            15
        } else {
            0
        }
    }

    fn set_plate_signal(&self, state: u32, signal: i32) -> u32 {
        if self.kind(state) == Kind::WeightedPressurePlate {
            self.with(state, "power", &signal.clamp(0, 15).to_string())
        } else {
            self.with(state, "powered", if signal > 0 { "true" } else { "false" })
        }
    }

    /// Entities on the plate (`TOUCH_AABB`, 1 to 15 sixteenths, 4 high).
    fn entities_on_plate(&self, pos: BlockPos, state: u32) -> usize {
        let (x, y, z) = (f64::from(pos.x), f64::from(pos.y), f64::from(pos.z));
        let touch: Aabb = [x + 0.0625, y, z + 0.0625, x + 0.9375, y + 0.25, z + 0.9375];
        let overlaps = |b: Aabb| (0..3).all(|i| b[i] < touch[i + 3] && b[i + 3] > touch[i]);
        let name = self
            .context
            .data
            .block_of(state)
            .map(|(_, b)| b.name.clone())
            .unwrap_or_default();
        // Stone plates only feel living things; wooden and weighted ones
        // any entity.
        let living_only = name.contains("stone") && !name.contains("weighted");
        let players = self
            .players
            .values()
            .filter(|p| {
                !p.dead && p.game_mode() != ferrocraft_protocol::packets::play::GameMode::Spectator
            })
            .filter(|p| overlaps(p.bounds()))
            .count();
        let mobs = self
            .mobs
            .values()
            .filter(|m| m.alive() && overlaps(m.bounds()))
            .count();
        let items = if living_only {
            0
        } else {
            self.items.values().filter(|i| overlaps(i.bounds())).count()
        };
        players + mobs + items
    }

    /// `getSignalStrength`.
    fn plate_strength(&self, pos: BlockPos, state: u32) -> i32 {
        let count = self.entities_on_plate(pos, state);
        if self.kind(state) != Kind::WeightedPressurePlate {
            return if count > 0 { 15 } else { 0 };
        }
        let name = self
            .context
            .data
            .block_of(state)
            .map(|(_, b)| b.name.clone())
            .unwrap_or_default();
        // Light (gold) plates count up to 15, heavy (iron) ones to 150.
        let max_weight = if name.contains("light") { 15 } else { 150 };
        let count = count.min(max_weight);
        if count > 0 {
            (count as f32 / max_weight as f32 * 15.0).ceil() as i32
        } else {
            0
        }
    }

    /// `BasePressurePlateBlock.checkPressed`.
    fn check_plate(&mut self, pos: BlockPos, state: u32, old_signal: i32) {
        let signal = self.plate_strength(pos, state);
        let (was, is) = (old_signal > 0, signal > 0);
        if old_signal != signal {
            let new = self.set_plate_signal(state, signal);
            self.set_block_with(pos, new, update::CLIENTS);
            self.plate_update(pos, new);
        }
        let name = self
            .context
            .data
            .block_of(state)
            .map(|(_, b)| b.name.clone())
            .unwrap_or_default();
        let material = if name.contains("weighted") {
            "metal"
        } else if name.contains("stone") {
            "stone"
        } else {
            "wooden"
        };
        if !is && was {
            let sound = format!("minecraft:block.{material}_pressure_plate.click_off");
            self.play_sound(None, &sound, center(pos), 0.3, 0.5);
        } else if is && !was {
            let sound = format!("minecraft:block.{material}_pressure_plate.click_on");
            self.play_sound(None, &sound, center(pos), 0.3, 0.6);
        }
        if is {
            let delay = if self.kind(state) == Kind::WeightedPressurePlate {
                10
            } else {
                20
            };
            let now = self.context.world.block(pos);
            self.schedule_block_tick(pos, now, delay, priority::NORMAL);
        }
    }

    /// `updateNeighbours`: the plate and the block below.
    pub(super) fn plate_update(&mut self, pos: BlockPos, state: u32) {
        let id = self.block_id(state);
        self.update_neighbors_at(pos, id);
        self.update_neighbors_at(below(pos), id);
    }

    pub(super) fn plate_tick(&mut self, pos: BlockPos, state: u32) {
        let signal = self.plate_signal(state);
        if signal > 0 {
            self.check_plate(pos, state, signal);
        }
    }

    /// `entityInside`: something stepped on a plate that is up.
    pub(super) fn plates_touched(&mut self, bounds: Aabb) {
        let (min, max) = (
            [
                bounds[0].floor() as i32,
                bounds[1].floor() as i32,
                bounds[2].floor() as i32,
            ],
            [
                bounds[3].floor() as i32,
                bounds[4].floor() as i32,
                bounds[5].floor() as i32,
            ],
        );
        for x in min[0]..=max[0] {
            for y in min[1]..=max[1] {
                for z in min[2]..=max[2] {
                    let pos = BlockPos { x, y, z };
                    let state = self.context.world.block(pos);
                    if matches!(
                        self.kind(state),
                        Kind::PressurePlate | Kind::WeightedPressurePlate
                    ) && self.plate_signal(state) == 0
                    {
                        self.check_plate(pos, state, 0);
                    }
                }
            }
        }
    }

    // --- What redstone moves -----------------------------------------------------

    /// `RedstoneLampBlock.neighborChanged`: on at once, off after four
    /// ticks.
    pub(super) fn lamp_neighbor_changed(&mut self, pos: BlockPos, state: u32) {
        let lit = self.bool_value(state, "lit");
        if lit == self.has_neighbor_signal(pos) {
            return;
        }
        if lit {
            self.schedule_block_tick(pos, state, 4, priority::NORMAL);
        } else {
            let on = self.with(state, "lit", "true");
            self.set_block_with(pos, on, update::CLIENTS);
        }
    }

    /// `DoorBlock.neighborChanged`: both halves open with power at either.
    pub(super) fn door_neighbor_changed(&mut self, pos: BlockPos, state: u32, source: usize) {
        let lower = self.value(state, "half") == Some("lower");
        let other = if lower { above(pos) } else { below(pos) };
        let signal = self.has_neighbor_signal(pos) || self.has_neighbor_signal(other);
        if source == self.block_id(state) || signal == self.bool_value(state, "powered") {
            return;
        }
        let flag = if signal { "true" } else { "false" };
        if signal != self.bool_value(state, "open") {
            self.door_sound(pos, state, signal);
        }
        let new = self.with(self.with(state, "powered", flag), "open", flag);
        self.set_block_with(pos, new, update::CLIENTS);
        let other_state = self.context.world.block(other);
        if self.block_id(other_state) == self.block_id(state) {
            let other_new = self.with(self.with(other_state, "powered", flag), "open", flag);
            self.set_block_with(other, other_new, update::CLIENTS);
        }
    }

    fn door_sound(&self, pos: BlockPos, state: u32, open: bool) {
        let name = self
            .context
            .data
            .block_of(state)
            .map(|(_, b)| b.name.clone())
            .unwrap_or_default();
        let kind = if name.ends_with("_trapdoor") {
            "trapdoor"
        } else if name.ends_with("_door") {
            "door"
        } else {
            "fence_gate"
        };
        let material = if name.contains("iron") {
            "iron_"
        } else if name.contains("copper") {
            "copper_"
        } else if name.contains("bamboo") {
            "bamboo_wood_"
        } else if name.contains("cherry") {
            "cherry_wood_"
        } else if name.contains("crimson") || name.contains("warped") {
            "nether_wood_"
        } else if kind == "fence_gate" {
            ""
        } else {
            "wooden_"
        };
        let sound = format!(
            "minecraft:block.{material}{kind}.{}",
            if open { "open" } else { "close" }
        );
        let pitch = rand::random::<f32>() * 0.1 + 0.9;
        self.play_sound(None, &sound, center(pos), 1.0, pitch);
    }

    /// `TrapDoorBlock` and `FenceGateBlock.neighborChanged`.
    pub(super) fn opening_neighbor_changed(&mut self, pos: BlockPos, state: u32) {
        let signal = self.has_neighbor_signal(pos);
        if signal == self.bool_value(state, "powered") {
            return;
        }
        let flag = if signal { "true" } else { "false" };
        let mut new = self.with(state, "powered", flag);
        if self.bool_value(state, "open") != signal {
            new = self.with(new, "open", flag);
            self.door_sound(pos, state, signal);
        }
        self.set_block_with(pos, new, update::CLIENTS);
    }

    /// The state a placed redstone-moved block starts in: lamps lit and
    /// doors open with power (`getStateForPlacement`).
    pub(super) fn powered_placement(&self, pos: BlockPos, state: u32) -> u32 {
        match self.kind(state) {
            Kind::Lamp => {
                let lit = self.has_neighbor_signal(pos);
                self.with(state, "lit", if lit { "true" } else { "false" })
            }
            Kind::Trapdoor | Kind::FenceGate => {
                let powered = self.has_neighbor_signal(pos);
                let flag = if powered { "true" } else { "false" };
                let state = self.with(state, "powered", flag);
                if powered {
                    self.with(state, "open", "true")
                } else {
                    state
                }
            }
            Kind::Door => {
                let powered = self.has_neighbor_signal(pos) || self.has_neighbor_signal(above(pos));
                let flag = if powered { "true" } else { "false" };
                self.with(self.with(state, "powered", flag), "open", flag)
            }
            _ => state,
        }
    }
}
