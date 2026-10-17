//! Commands: the tree the client completes against and what running them
//! does. Operators (permission level 4) get `gamemode`, `time`, `tp`,
//! `give`, `kill`, `summon`, `difficulty` and `experience`; everyone gets
//! `list`.

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::codec::BlockPos;
use ferrocraft_protocol::packets::play::{
    CommandNode, Commands, GameMode, ItemStack, NodeKind, text,
};
use tracing::info;

use super::Game;
use crate::data::GameData;

/// Permission level that operators have and the commands below need.
pub const OPERATOR: u8 = 4;

/// Builds the tree node by node; children are added before their parents.
struct Tree<'a> {
    data: &'a GameData,
    nodes: Vec<CommandNode>,
}

impl Tree<'_> {
    fn add(&mut self, kind: NodeKind, executable: bool, children: Vec<i32>) -> i32 {
        self.nodes.push(CommandNode {
            kind,
            executable,
            children,
        });
        self.nodes.len() as i32 - 1
    }

    fn literal(&mut self, name: &str, executable: bool, children: Vec<i32>) -> i32 {
        self.add(NodeKind::Literal(name.into()), executable, children)
    }

    /// An argument with the given parser and its encoded settings.
    fn argument(
        &mut self,
        name: &str,
        parser: &str,
        properties: Vec<u8>,
        executable: bool,
        children: Vec<i32>,
    ) -> i32 {
        let parser = self
            .data
            .command_argument_type(parser)
            .expect("vanilla argument types");
        self.add(
            NodeKind::Argument {
                name: name.into(),
                parser,
                properties,
                suggestions: None,
            },
            executable,
            children,
        )
    }

    /// `minecraft:entity` with `EntityArgument.Info`'s flags.
    fn entities(&mut self, name: &str, single: bool, executable: bool, children: Vec<i32>) -> i32 {
        let flags = u8::from(single) | 0x02;
        self.argument(name, "minecraft:entity", vec![flags], executable, children)
    }
}

/// The tree a player with `permission` sees.
pub fn tree(data: &GameData, permission: u8) -> Commands {
    let mut t = Tree {
        data,
        nodes: Vec::new(),
    };
    let mut root = vec![t.literal("list", true, Vec::new())];
    if permission >= OPERATOR {
        let target = t.entities("target", false, true, Vec::new());
        let mode = t.argument(
            "gamemode",
            "minecraft:gamemode",
            Vec::new(),
            true,
            vec![target],
        );
        root.push(t.literal("gamemode", false, vec![mode]));

        let mut set = Vec::new();
        for name in ["day", "noon", "night", "midnight"] {
            set.push(t.literal(name, true, Vec::new()));
        }
        set.push(t.argument(
            "time",
            "minecraft:time",
            0i32.to_be_bytes().to_vec(),
            true,
            Vec::new(),
        ));
        let set = t.literal("set", false, set);
        let amount = t.argument(
            "time",
            "minecraft:time",
            i32::MIN.to_be_bytes().to_vec(),
            true,
            Vec::new(),
        );
        let add = t.literal("add", false, vec![amount]);
        let daytime = t.literal("daytime", true, Vec::new());
        let gametime = t.literal("gametime", true, Vec::new());
        let query = t.literal("query", false, vec![daytime, gametime]);
        root.push(t.literal("time", false, vec![set, add, query]));

        let location = t.argument("location", "minecraft:vec3", Vec::new(), true, Vec::new());
        let destination = t.entities("destination", true, true, Vec::new());
        let to_location = t.argument("location", "minecraft:vec3", Vec::new(), true, Vec::new());
        let to_destination = t.entities("destination", true, true, Vec::new());
        let targets = t.entities("targets", false, false, vec![to_location, to_destination]);
        let tp_children = vec![location, destination, targets];
        root.push(t.literal("tp", false, tp_children.clone()));
        root.push(t.literal("teleport", false, tp_children));

        // `brigadier:integer` with a minimum of 1.
        let count = t.argument(
            "count",
            "brigadier:integer",
            [&[1u8][..], &1i32.to_be_bytes()].concat(),
            true,
            Vec::new(),
        );
        let item = t.argument(
            "item",
            "minecraft:item_stack",
            Vec::new(),
            true,
            vec![count],
        );
        let receivers = t.entities("targets", false, false, vec![item]);
        root.push(t.literal("give", false, vec![receivers]));

        let victims = t.entities("targets", false, true, Vec::new());
        root.push(t.literal("kill", true, vec![victims]));

        // `ResourceArgument` over `minecraft:entity_type`.
        let registry = "minecraft:entity_type";
        let mut properties = Vec::new();
        ferrocraft_protocol::codec::Encode::encode(registry, &mut properties);
        let at = t.argument("pos", "minecraft:vec3", Vec::new(), true, Vec::new());
        let entity = t.argument("entity", "minecraft:resource", properties, true, vec![at]);
        root.push(t.literal("summon", false, vec![entity]));

        let levels: Vec<i32> = DIFFICULTIES
            .iter()
            .map(|d| t.literal(d, true, Vec::new()))
            .collect();
        root.push(t.literal("difficulty", true, levels));

        // `experience add|set <targets> <amount> [levels|points]`,
        // `experience query <target> levels|points`.
        let mut experience = Vec::new();
        for (action, min) in [("add", i32::MIN), ("set", 0)] {
            let units = vec![
                t.literal("levels", true, Vec::new()),
                t.literal("points", true, Vec::new()),
            ];
            let amount = t.argument(
                "amount",
                "brigadier:integer",
                [&[1u8][..], &min.to_be_bytes()].concat(),
                true,
                units,
            );
            let targets = t.entities("targets", false, false, vec![amount]);
            experience.push(t.literal(action, false, vec![targets]));
        }
        let units = vec![
            t.literal("levels", true, Vec::new()),
            t.literal("points", true, Vec::new()),
        ];
        let target = t.entities("target", true, false, units);
        experience.push(t.literal("query", false, vec![target]));
        root.push(t.literal("experience", false, experience.clone()));
        root.push(t.literal("xp", false, experience));

        // `setblock <pos> <block> [destroy|keep|replace]`.
        let modes = ["destroy", "keep", "replace"]
            .iter()
            .map(|m| t.literal(m, true, Vec::new()))
            .collect();
        let block = t.argument("block", "minecraft:block_state", Vec::new(), true, modes);
        let pos = t.argument("pos", "minecraft:block_pos", Vec::new(), false, vec![block]);
        root.push(t.literal("setblock", false, vec![pos]));

        // `fill <from> <to> <block> [destroy|hollow|keep|outline|replace]`.
        let modes = ["destroy", "hollow", "keep", "outline", "replace"]
            .iter()
            .map(|m| t.literal(m, true, Vec::new()))
            .collect();
        let block = t.argument("block", "minecraft:block_state", Vec::new(), true, modes);
        let to = t.argument("to", "minecraft:block_pos", Vec::new(), false, vec![block]);
        let from = t.argument("from", "minecraft:block_pos", Vec::new(), false, vec![to]);
        root.push(t.literal("fill", false, vec![from]));

        // `gamerule <rule> [value]` for every rule.
        let mut rules = Vec::new();
        for rule in crate::rules::RULES {
            let value = match rule.default {
                crate::rules::Value::Int(_) => {
                    // `IntegerArgumentType` flags: 1 has a minimum, 2 a
                    // maximum.
                    let mut properties = vec![1u8];
                    properties.extend(rule.min.to_be_bytes());
                    if rule.max != i32::MAX {
                        properties[0] |= 2;
                        properties.extend(rule.max.to_be_bytes());
                    }
                    t.argument("value", "brigadier:integer", properties, true, Vec::new())
                }
                crate::rules::Value::Bool(_) => {
                    t.argument("value", "brigadier:bool", Vec::new(), true, Vec::new())
                }
            };
            rules.push(t.literal(rule.name, true, vec![value]));
        }
        root.push(t.literal("gamerule", false, rules));

        // `weather clear|rain|thunder [duration]`.
        let mut kinds = Vec::new();
        for kind in ["clear", "rain", "thunder"] {
            let duration = t.argument(
                "duration",
                "minecraft:time",
                1i32.to_be_bytes().to_vec(),
                true,
                Vec::new(),
            );
            kinds.push(t.literal(kind, true, vec![duration]));
        }
        root.push(t.literal("weather", false, kinds));

        // `effect give <targets> <effect> [<seconds>|infinite] [<amplifier>]
        // [<hideParticles>]`, `effect clear [<targets>] [<effect>]`.
        let resource = |registry: &str| {
            let mut properties = Vec::new();
            ferrocraft_protocol::codec::Encode::encode(registry, &mut properties);
            properties
        };
        let integer =
            |min: i32, max: i32| [&[3u8][..], &min.to_be_bytes(), &max.to_be_bytes()].concat();
        let hide = t.argument(
            "hideParticles",
            "brigadier:bool",
            Vec::new(),
            true,
            Vec::new(),
        );
        let amplifier = t.argument(
            "amplifier",
            "brigadier:integer",
            integer(0, 255),
            true,
            vec![hide],
        );
        let seconds = t.argument(
            "seconds",
            "brigadier:integer",
            integer(1, 1_000_000),
            true,
            vec![amplifier],
        );
        let infinite = t.literal("infinite", true, vec![amplifier]);
        let effect = t.argument(
            "effect",
            "minecraft:resource",
            resource("minecraft:mob_effect"),
            true,
            vec![seconds, infinite],
        );
        let give_targets = t.entities("targets", false, false, vec![effect]);
        let give = t.literal("give", false, vec![give_targets]);
        let clear_effect = t.argument(
            "effect",
            "minecraft:resource",
            resource("minecraft:mob_effect"),
            true,
            Vec::new(),
        );
        let clear_targets = t.entities("targets", false, true, vec![clear_effect]);
        let clear = t.literal("clear", true, vec![clear_targets]);
        root.push(t.literal("effect", false, vec![give, clear]));

        // `enchant <targets> <enchantment> [<level>]`.
        let level = t.argument(
            "level",
            "brigadier:integer",
            [&[1u8][..], &0i32.to_be_bytes()].concat(),
            true,
            Vec::new(),
        );
        let enchantment = t.argument(
            "enchantment",
            "minecraft:resource",
            resource("minecraft:enchantment"),
            true,
            vec![level],
        );
        let enchant_targets = t.entities("targets", false, false, vec![enchantment]);
        root.push(t.literal("enchant", false, vec![enchant_targets]));

        // `clear [<targets>] [<item>] [<maxCount>]`.
        let max_count = t.argument(
            "maxCount",
            "brigadier:integer",
            [&[1u8][..], &0i32.to_be_bytes()].concat(),
            true,
            Vec::new(),
        );
        let item = t.argument(
            "item",
            "minecraft:item_predicate",
            Vec::new(),
            true,
            vec![max_count],
        );
        let clear_targets = t.entities("targets", false, true, vec![item]);
        root.push(t.literal("clear", true, vec![clear_targets]));
    }
    let root = t.add(NodeKind::Root, false, root);
    Commands {
        nodes: t.nodes,
        root,
    }
}

/// `/fill` and `/setblock` change at most this many blocks
/// (`max_block_modifications`).
const MAX_FILL: i64 = 32768;

/// A command's answer: shown to the sender, red if it failed.
type Reply = Result<Nbt, Nbt>;

fn error(key: &str, with: Vec<Nbt>) -> Nbt {
    text::translate(key, with, Some("red"))
}

/// `gameMode.creative` and so on, as vanilla names them.
fn mode_name(mode: GameMode) -> Nbt {
    let key = match mode {
        GameMode::Survival => "gameMode.survival",
        GameMode::Creative => "gameMode.creative",
        GameMode::Adventure => "gameMode.adventure",
        GameMode::Spectator => "gameMode.spectator",
    };
    text::translate(key, Vec::new(), None)
}

/// `Difficulty` by name, 0 peaceful to 3 hard.
pub fn parse_difficulty(word: &str) -> Option<u8> {
    DIFFICULTIES
        .iter()
        .position(|&d| d == word)
        .map(|d| d as u8)
}

pub const DIFFICULTIES: [&str; 4] = ["peaceful", "easy", "normal", "hard"];

pub fn parse_mode(word: &str) -> Option<GameMode> {
    Some(match word {
        "survival" => GameMode::Survival,
        "creative" => GameMode::Creative,
        "adventure" => GameMode::Adventure,
        "spectator" => GameMode::Spectator,
        _ => return None,
    })
}

/// `TimeArgument`: ticks, or a number with `d` (days), `s` (seconds) or `t`.
fn parse_time(word: &str) -> Option<i64> {
    let (number, factor) = match word.chars().last()? {
        'd' => (&word[..word.len() - 1], 24000.0),
        's' => (&word[..word.len() - 1], 20.0),
        't' => (&word[..word.len() - 1], 1.0),
        _ => (word, 1.0),
    };
    let value: f64 = number.parse().ok()?;
    Some((value * factor).round() as i64)
}

/// A coordinate, absolute or relative to `base` with `~`.
fn parse_coordinate(word: &str, base: f64, center: bool) -> Option<f64> {
    if let Some(offset) = word.strip_prefix('~') {
        let offset = if offset.is_empty() {
            0.0
        } else {
            offset.parse().ok()?
        };
        return Some(base + offset);
    }
    let value: f64 = word.parse().ok()?;
    // Whole numbers mean the middle of the block, like vanilla's
    // `Vec3Argument` with centring.
    Some(if center && !word.contains('.') {
        value + 0.5
    } else {
        value
    })
}

impl Game {
    /// The sender as the target of a command without one; the console is
    /// no player.
    fn myself(&self, sender: i32) -> Result<Vec<i32>, Nbt> {
        if self.players.contains_key(&sender) {
            Ok(vec![sender])
        } else {
            Err(error("permissions.requires.player", Vec::new()))
        }
    }

    /// Where a command runs: the player's position, the world spawn for the
    /// console.
    fn source_pos(&self, sender: i32) -> [f64; 3] {
        self.players.get(&sender).map_or_else(
            || {
                let spawn = self.context.level.spawn;
                [f64::from(spawn.x), f64::from(spawn.y), f64::from(spawn.z)]
            },
            |p| p.pos,
        )
    }

    /// Players a selector or name stands for.
    fn targets(&self, sender: i32, word: &str) -> Vec<i32> {
        match word {
            "@s" if !self.players.contains_key(&sender) => Vec::new(),
            "@p" if !self.players.contains_key(&sender) => {
                // The player nearest to the spawn, where the console is.
                let from = self.source_pos(sender);
                let distance = |p: &[f64; 3]| -> f64 {
                    p.iter().zip(from).map(|(a, b)| (a - b) * (a - b)).sum()
                };
                self.players
                    .values()
                    .min_by(|a, b| distance(&a.pos).total_cmp(&distance(&b.pos)))
                    .map(|p| p.entity_id)
                    .into_iter()
                    .collect()
            }
            "@s" | "@p" => vec![sender],
            "@a" | "@e" => self.players.keys().copied().collect(),
            "@r" => self
                .players
                .keys()
                .copied()
                .nth(rand::random::<u32>() as usize % self.players.len().max(1))
                .into_iter()
                .collect(),
            name => self
                .players
                .values()
                .filter(|p| p.name().eq_ignore_ascii_case(name))
                .map(|p| p.entity_id)
                .collect(),
        }
    }

    fn player_name(&self, entity_id: i32) -> Nbt {
        self.players.get(&entity_id).map_or_else(
            || text::plain("?"),
            |p| text::player_name(p.name(), p.uuid()),
        )
    }

    pub(super) fn command(&mut self, sender: i32, line: &str) {
        let Some(player) = self.players.get(&sender) else {
            return;
        };
        info!("{} issued server command: /{line}", player.name());
        let permission = player.permission;
        let reply = self.execute(sender, permission, line);
        let content = match reply {
            // `send_command_feedback`: successes may go unsaid, failures not.
            Ok(_) if !self.rules.flag("send_command_feedback") => return,
            Ok(message) | Err(message) => message,
        };
        if let Some(player) = self.players.get(&sender) {
            player.send(super::Outbound::Packet(std::sync::Arc::new(
                super::Encoded::new(&ferrocraft_protocol::packets::play::SystemChat {
                    content,
                    overlay: false,
                }),
            )));
        }
    }

    /// Runs a command for a player or the console with the given
    /// permission; the answer.
    pub(super) fn execute(&mut self, sender: i32, permission: u8, line: &str) -> Reply {
        let words = split_arguments(line);
        match words.as_slice() {
            ["list"] => Ok(self.list()),
            [name, ..] if permission < OPERATOR && *name != "list" => {
                Err(error("command.unknown.command", Vec::new()))
            }
            ["gamemode", mode, rest @ ..] => self.gamemode_command(sender, mode, rest),
            ["time", rest @ ..] => self.time_command(rest),
            ["tp" | "teleport", rest @ ..] => self.tp_command(sender, rest),
            ["give", target, item, rest @ ..] => self.give_command(sender, target, item, rest),
            ["summon", entity, rest @ ..] => self.summon_command(sender, entity, rest),
            ["difficulty", rest @ ..] => self.difficulty_command(rest),
            ["experience" | "xp", rest @ ..] => self.experience_command(sender, rest),
            ["setblock", x, y, z, block, rest @ ..] => {
                self.setblock_command(sender, [x, y, z], block, rest)
            }
            ["fill", x1, y1, z1, x2, y2, z2, block, rest @ ..] => {
                self.fill_command(sender, [x1, y1, z1], [x2, y2, z2], block, rest)
            }
            ["gamerule", rule, rest @ ..] => self.gamerule_command(rule, rest),
            ["weather", kind, rest @ ..] => self.weather_command(kind, rest),
            ["effect", "give", target, effect, rest @ ..] => {
                self.effect_give_command(sender, target, effect, rest)
            }
            ["effect", "clear", rest @ ..] => self.effect_clear_command(sender, rest),
            ["enchant", target, enchantment, rest @ ..] => {
                self.enchant_command(sender, target, enchantment, rest)
            }
            ["clear", rest @ ..] => self.clear_command(sender, rest),
            ["kill", rest @ ..] => {
                let victims = match rest {
                    [] => self.myself(sender)?,
                    [target] => self.targets(sender, target),
                    _ => Vec::new(),
                };
                if victims.is_empty() {
                    Err(error("argument.entity.notfound.entity", Vec::new()))
                } else {
                    let name = self.player_name(victims[0]);
                    for victim in &victims {
                        self.kill(*victim);
                    }
                    Ok(text::translate(
                        "commands.kill.success.single",
                        vec![name],
                        None,
                    ))
                }
            }
            _ => Err(error("command.unknown.command", Vec::new())),
        }
    }

    fn list(&self) -> Nbt {
        let mut names: Vec<&str> = self.players.values().map(|p| p.name()).collect();
        names.sort_unstable();
        text::translate(
            "commands.list.players",
            vec![
                text::plain(&names.len().to_string()),
                text::plain(&self.context.max_players.to_string()),
                text::plain(&names.join(", ")),
            ],
            None,
        )
    }

    fn gamemode_command(&mut self, sender: i32, mode: &str, rest: &[&str]) -> Reply {
        let Some(mode) = parse_mode(mode) else {
            return Err(error("argument.gamemode.invalid", vec![text::plain(mode)]));
        };
        let targets = match rest {
            [] => self.myself(sender)?,
            [target] => self.targets(sender, target),
            _ => return Err(error("command.unknown.argument", Vec::new())),
        };
        if targets.is_empty() {
            return Err(error("argument.player.unknown", Vec::new()));
        }
        for &target in &targets {
            self.set_game_mode(target, mode);
        }
        if targets == [sender] {
            Ok(text::translate(
                "commands.gamemode.success.self",
                vec![mode_name(mode)],
                None,
            ))
        } else {
            Ok(text::translate(
                "commands.gamemode.success.other",
                vec![self.player_name(targets[0]), mode_name(mode)],
                None,
            ))
        }
    }

    fn time_command(&mut self, rest: &[&str]) -> Reply {
        let level = &mut self.context.level;
        let reply = match rest {
            ["set", value] => {
                let ticks = match *value {
                    "day" => 1000,
                    "noon" => 6000,
                    "night" => 13000,
                    "midnight" => 18000,
                    other => parse_time(other)
                        .ok_or_else(|| error("argument.time.invalid_unit", Vec::new()))?,
                };
                // Like vanilla, set the time of the current day.
                level.day_time = level.day_time - level.day_time.rem_euclid(24000) + ticks;
                text::translate(
                    "commands.time.set",
                    vec![text::plain(&ticks.to_string())],
                    None,
                )
            }
            ["add", value] => {
                let ticks = parse_time(value)
                    .ok_or_else(|| error("argument.time.invalid_unit", Vec::new()))?;
                level.day_time += ticks;
                text::translate(
                    "commands.time.set",
                    vec![text::plain(&level.day_time.rem_euclid(24000).to_string())],
                    None,
                )
            }
            ["query", "daytime"] => {
                return Ok(text::translate(
                    "commands.time.query",
                    vec![text::plain(&level.day_time.rem_euclid(24000).to_string())],
                    None,
                ));
            }
            ["query", "gametime"] => {
                return Ok(text::translate(
                    "commands.time.query",
                    vec![text::plain(&level.game_time.to_string())],
                    None,
                ));
            }
            _ => return Err(error("command.unknown.argument", Vec::new())),
        };
        let packet = std::sync::Arc::new(self.time_packet());
        self.broadcast(None, &super::Outbound::Packet(packet));
        Ok(reply)
    }

    fn tp_command(&mut self, sender: i32, rest: &[&str]) -> Reply {
        let position = |game: &Game, who: i32| game.players.get(&who).map(|p| p.pos);
        let (movers, words) = match rest {
            [_] | [_, _, _] => (self.myself(sender)?, rest),
            [target, more @ ..] => (self.targets(sender, target), more),
            [] => return Err(error("command.unknown.argument", Vec::new())),
        };
        if movers.is_empty() {
            return Err(error("argument.player.unknown", Vec::new()));
        }
        let base = position(self, movers[0]).unwrap_or_default();
        let (destination, reply) = match words {
            [x, y, z] => {
                let at = [
                    parse_coordinate(x, base[0], true),
                    parse_coordinate(y, base[1], false),
                    parse_coordinate(z, base[2], true),
                ];
                let [Some(x), Some(y), Some(z)] = at else {
                    return Err(error("argument.pos3d.incomplete", Vec::new()));
                };
                let fmt = |v: f64| text::plain(&format!("{v:.2}"));
                (
                    [x, y, z],
                    text::translate(
                        "commands.teleport.success.location.single",
                        vec![self.player_name(movers[0]), fmt(x), fmt(y), fmt(z)],
                        None,
                    ),
                )
            }
            [target] => {
                let Some(&to) = self.targets(sender, target).first() else {
                    return Err(error("argument.player.unknown", Vec::new()));
                };
                (
                    position(self, to).unwrap_or(base),
                    text::translate(
                        "commands.teleport.success.entity.single",
                        vec![self.player_name(movers[0]), self.player_name(to)],
                        None,
                    ),
                )
            }
            _ => return Err(error("command.unknown.argument", Vec::new())),
        };
        for mover in movers {
            self.teleport(mover, destination);
        }
        Ok(reply)
    }

    /// `/summon <entity> [x y z]`, for the mobs Ferrocraft knows.
    fn summon_command(&mut self, sender: i32, entity: &str, rest: &[&str]) -> Reply {
        let name = if entity.contains(':') {
            entity.to_owned()
        } else {
            format!("minecraft:{entity}")
        };
        let lightning = name == "minecraft:lightning_bolt";
        let kind = super::mob::Kind::from_name(&name);
        if kind.is_none() && !lightning {
            return Err(error("commands.summon.failed", Vec::new()));
        }
        let base = self.source_pos(sender);
        let pos = match rest {
            [] => base,
            [x, y, z] => {
                let at = [
                    parse_coordinate(x, base[0], true),
                    parse_coordinate(y, base[1], false),
                    parse_coordinate(z, base[2], true),
                ];
                let [Some(x), Some(y), Some(z)] = at else {
                    return Err(error("argument.pos3d.incomplete", Vec::new()));
                };
                [x, y, z]
            }
            _ => return Err(error("command.unknown.argument", Vec::new())),
        };
        match kind {
            Some(kind) => {
                if self.spawn_mob(kind, pos).is_none() {
                    return Err(error("commands.summon.failed", Vec::new()));
                }
            }
            None => self.strike(
                BlockPos {
                    x: pos[0].floor() as i32,
                    y: pos[1].floor() as i32,
                    z: pos[2].floor() as i32,
                },
                false,
            ),
        }
        let key = format!("entity.{}", name.replace(':', "."));
        Ok(text::translate(
            "commands.summon.success",
            vec![text::translate(&key, Vec::new(), None)],
            None,
        ))
    }

    /// A block position from three words, `~` relative to the sender.
    fn block_pos(&self, sender: i32, words: [&&str; 3]) -> Result<BlockPos, Nbt> {
        let base = self.source_pos(sender);
        let mut at = [0; 3];
        for (i, word) in words.iter().enumerate() {
            let value = parse_coordinate(word, base[i].floor(), false)
                .ok_or_else(|| error("argument.pos3d.incomplete", Vec::new()))?;
            at[i] = value.floor() as i32;
        }
        Ok(BlockPos {
            x: at[0],
            y: at[1],
            z: at[2],
        })
    }

    /// `BlockStateArgument`: `stone`, `minecraft:oak_log[axis=x]`; with the
    /// properties named.
    fn block_state(&self, word: &str) -> Result<(u32, Vec<(String, String)>), Nbt> {
        let (name, properties) = match word.split_once('[') {
            Some((name, rest)) => (name, rest.strip_suffix(']').unwrap_or(rest)),
            None => (word, ""),
        };
        let name = if name.contains(':') {
            name.to_owned()
        } else {
            format!("minecraft:{name}")
        };
        let Some(block) = self.context.data.block_named(&name) else {
            return Err(error("argument.block.id.invalid", vec![text::plain(&name)]));
        };
        let mut state = block.default;
        let mut named = Vec::new();
        for pair in properties.split(',').filter(|p| !p.is_empty()) {
            let Some((key, value)) = pair.split_once('=') else {
                return Err(error("argument.block.property.unclosed", Vec::new()));
            };
            named.push((key.trim().to_owned(), value.trim().to_owned()));
            state = block.with(state, key.trim(), value.trim()).ok_or_else(|| {
                error(
                    "argument.block.property.invalid",
                    vec![text::plain(&name), text::plain(value), text::plain(key)],
                )
            })?;
        }
        Ok((state, named))
    }

    /// Puts a block as `/setblock` and `/fill` do (`BlockInput.place`):
    /// shaped by its neighbours except for the properties named; `destroy`
    /// breaks the old one with drops first, `keep` only fills air. Whether
    /// it changed.
    fn command_place(
        &mut self,
        pos: BlockPos,
        block: &(u32, Vec<(String, String)>),
        mode: &str,
    ) -> bool {
        let old = self.context.world.block(pos);
        if mode == "keep" && !self.is_air(old) {
            return false;
        }
        if mode == "destroy" {
            self.destroy_block(pos, true);
        }
        let (state, named) = block;
        let mut shaped = self.shaped_by_neighbours(pos, *state);
        if self.is_air(shaped) {
            shaped = *state;
        }
        for (key, value) in named {
            shaped = self.with(shaped, key, value);
        }
        self.set_block_with(pos, shaped, super::blocks::update::CLIENTS)
    }

    /// `/setblock <pos> <block> [destroy|keep|replace]`.
    fn setblock_command(
        &mut self,
        sender: i32,
        at: [&&str; 3],
        block: &str,
        rest: &[&str],
    ) -> Reply {
        let pos = self.block_pos(sender, at)?;
        let block = self.block_state(block)?;
        let mode = rest.first().copied().unwrap_or("replace");
        if !crate::world::in_height(pos.y) {
            return Err(error("argument.pos.outofworld", Vec::new()));
        }
        let old = self.context.world.block(pos);
        if !self.command_place(pos, &block, mode) {
            return Err(error("commands.setblock.failed", Vec::new()));
        }
        self.updates_after_set(pos, old);
        Ok(text::translate(
            "commands.setblock.success",
            vec![
                text::plain(&pos.x.to_string()),
                text::plain(&pos.y.to_string()),
                text::plain(&pos.z.to_string()),
            ],
            None,
        ))
    }

    /// `/fill <from> <to> <block> [destroy|hollow|keep|outline|replace]`.
    fn fill_command(
        &mut self,
        sender: i32,
        from: [&&str; 3],
        to: [&&str; 3],
        block: &str,
        rest: &[&str],
    ) -> Reply {
        let (a, b) = (self.block_pos(sender, from)?, self.block_pos(sender, to)?);
        let block = self.block_state(block)?;
        let air = (self.air, Vec::new());
        let mode = rest.first().copied().unwrap_or("replace");
        let (min, max) = (
            [a.x.min(b.x), a.y.min(b.y), a.z.min(b.z)],
            [a.x.max(b.x), a.y.max(b.y), a.z.max(b.z)],
        );
        let volume: i64 = (0..3).map(|i| i64::from(max[i] - min[i] + 1)).product();
        if volume > MAX_FILL {
            return Err(error(
                "commands.fill.toobig",
                vec![
                    text::plain(&MAX_FILL.to_string()),
                    text::plain(&volume.to_string()),
                ],
            ));
        }
        if !crate::world::in_height(min[1]) || !crate::world::in_height(max[1]) {
            return Err(error("argument.pos.outofworld", Vec::new()));
        }
        let mut changed = Vec::new();
        for x in min[0]..=max[0] {
            for y in min[1]..=max[1] {
                for z in min[2]..=max[2] {
                    let pos = BlockPos { x, y, z };
                    let edge = x == min[0]
                        || x == max[0]
                        || y == min[1]
                        || y == max[1]
                        || z == min[2]
                        || z == max[2];
                    let (place, mode) = match mode {
                        "hollow" if !edge => (&air, "replace"),
                        "outline" if !edge => continue,
                        "hollow" | "outline" => (&block, "replace"),
                        other => (&block, other),
                    };
                    let old = self.context.world.block(pos);
                    if self.command_place(pos, place, mode) {
                        changed.push((pos, old));
                    }
                }
            }
        }
        if changed.is_empty() {
            return Err(error("commands.fill.failed", Vec::new()));
        }
        for &(pos, old) in &changed {
            self.updates_after_set(pos, old);
        }
        Ok(text::translate(
            "commands.fill.success",
            vec![text::plain(&changed.len().to_string())],
            None,
        ))
    }

    /// `/gamerule <rule> [value]`.
    fn gamerule_command(&mut self, rule: &str, rest: &[&str]) -> Reply {
        let Some(current) = self.rules.get(rule) else {
            return Err(error("command.unknown.argument", Vec::new()));
        };
        let name = rule.strip_prefix("minecraft:").unwrap_or(rule);
        match rest {
            [] => Ok(text::translate(
                "commands.gamerule.query",
                vec![text::plain(name), text::plain(&current.to_string())],
                None,
            )),
            [value] => {
                use crate::rules::Refused;
                let plain = text::plain(value);
                let set = self.rules.set(rule, value).map_err(|why| match why {
                    Refused::Unknown => error("command.unknown.argument", Vec::new()),
                    Refused::NotABool => error("parsing.bool.invalid", vec![plain.clone()]),
                    Refused::NotANumber => error("parsing.int.invalid", vec![plain.clone()]),
                    Refused::TooLow(min) => error(
                        "argument.integer.low",
                        vec![text::plain(&min.to_string()), plain.clone()],
                    ),
                    Refused::TooHigh(max) => error(
                        "argument.integer.big",
                        vec![text::plain(&max.to_string()), plain.clone()],
                    ),
                })?;
                self.rule_changed(name);
                Ok(text::translate(
                    "commands.gamerule.set",
                    vec![text::plain(name), text::plain(&set.to_string())],
                    None,
                ))
            }
            _ => Err(error("command.unknown.argument", Vec::new())),
        }
    }

    /// `/weather clear|rain|thunder [duration]`.
    fn weather_command(&mut self, kind: &str, rest: &[&str]) -> Reply {
        let duration = match rest {
            [] => None,
            [time] => match parse_time(time) {
                Some(t) if t >= 1 => Some(t.min(i64::from(i32::MAX)) as i32),
                _ => return Err(error("argument.time.invalid_unit", Vec::new())),
            },
            _ => return Err(error("command.unknown.argument", Vec::new())),
        };
        let key = match kind {
            "clear" => "commands.weather.set.clear",
            "rain" => "commands.weather.set.rain",
            "thunder" => "commands.weather.set.thunder",
            _ => return Err(error("command.unknown.argument", Vec::new())),
        };
        self.set_weather(kind, duration);
        Ok(text::translate(key, Vec::new(), None))
    }

    /// `/difficulty [peaceful|easy|normal|hard]`.
    fn difficulty_command(&mut self, rest: &[&str]) -> Reply {
        let name = |d: u8| {
            text::translate(
                &format!("options.difficulty.{}", DIFFICULTIES[usize::from(d)]),
                Vec::new(),
                None,
            )
        };
        match rest {
            [] => Ok(text::translate(
                "commands.difficulty.query",
                vec![name(self.context.difficulty)],
                None,
            )),
            [word] => {
                let Some(difficulty) = parse_difficulty(word) else {
                    return Err(error("command.unknown.argument", Vec::new()));
                };
                if difficulty == self.context.difficulty {
                    return Err(error("commands.difficulty.failure", vec![name(difficulty)]));
                }
                self.context.difficulty = difficulty;
                self.send_difficulty(None);
                Ok(text::translate(
                    "commands.difficulty.success",
                    vec![name(difficulty)],
                    None,
                ))
            }
            _ => Err(error("command.unknown.argument", Vec::new())),
        }
    }

    /// `/experience add|set <targets> <amount> [levels|points]` and
    /// `/experience query <target> levels|points`.
    fn experience_command(&mut self, sender: i32, rest: &[&str]) -> Reply {
        let (action, target, amount, unit) = match rest {
            ["query", target, unit] => ("query", *target, "0", *unit),
            [action @ ("add" | "set"), target, amount] => (*action, *target, *amount, "points"),
            [action @ ("add" | "set"), target, amount, unit] => (*action, *target, *amount, *unit),
            _ => return Err(error("command.unknown.argument", Vec::new())),
        };
        let levels = match unit {
            "levels" => true,
            "points" => false,
            _ => return Err(error("command.unknown.argument", Vec::new())),
        };
        let Ok(amount) = amount.parse::<i32>() else {
            return Err(error("parsing.int.invalid", vec![text::plain(amount)]));
        };
        let targets = self.targets(sender, target);
        let Some(&first) = targets.first() else {
            return Err(error("argument.player.unknown", Vec::new()));
        };
        let unit = if levels { "levels" } else { "points" };
        match action {
            "query" => {
                let experience = self.players[&first].experience;
                let value = if levels {
                    experience.level
                } else {
                    (experience.progress * super::orb::needed_for(experience.level) as f32) as i32
                };
                return Ok(text::translate(
                    &format!("commands.experience.query.{unit}"),
                    vec![self.player_name(first), text::plain(&value.to_string())],
                    None,
                ));
            }
            "add" => {
                for &target in &targets {
                    if levels {
                        self.give_levels(target, amount);
                    } else {
                        self.give_experience(target, amount);
                    }
                }
            }
            _ => {
                for &target in &targets {
                    let Some(player) = self.players.get_mut(&target) else {
                        continue;
                    };
                    if levels {
                        player.experience.level = amount;
                    } else {
                        // `setExperiencePoints`: within the current level.
                        let needed = super::orb::needed_for(player.experience.level);
                        if amount >= needed {
                            return Err(error(
                                "commands.experience.set.points.invalid",
                                Vec::new(),
                            ));
                        }
                        player.experience.progress = amount as f32 / needed as f32;
                    }
                    self.send_experience(target);
                }
            }
        }
        Ok(text::translate(
            &format!("commands.experience.{action}.{unit}.success.single"),
            vec![text::plain(&amount.to_string()), self.player_name(first)],
            None,
        ))
    }

    /// `MinecraftServer.onGameRuleChanged`: rules the clients act on.
    fn rule_changed(&mut self, name: &str) {
        match name {
            "immediate_respawn" | "reduced_debug_info" => {
                let ids: Vec<i32> = self.players.keys().copied().collect();
                for id in ids {
                    self.send_client_rules(id);
                }
            }
            _ => {}
        }
    }

    /// The rules a client acts on itself: the death screen
    /// (`immediate_respawn`, game event 11) and the debug screen
    /// (`reduced_debug_info`, entity events 22 and 23).
    pub(super) fn send_client_rules(&self, entity_id: i32) {
        let Some(player) = self.players.get(&entity_id) else {
            return;
        };
        let immediate = self.rules.flag("immediate_respawn");
        player.send(super::Outbound::Packet(std::sync::Arc::new(
            super::Encoded::new(&ferrocraft_protocol::packets::play::GameEvent {
                event: 11,
                param: if immediate { 1.0 } else { 0.0 },
            }),
        )));
        let reduced = self.rules.flag("reduced_debug_info");
        player.send(super::Outbound::Packet(std::sync::Arc::new(
            super::Encoded::new(&ferrocraft_protocol::packets::play::EntityEvent {
                entity_id,
                event: if reduced { 22 } else { 23 },
            }),
        )));
    }

    /// Players a selector or name stands for, and with `@e` the mobs too.
    fn living_targets(&self, sender: i32, word: &str) -> Vec<i32> {
        let mut targets = self.targets(sender, word);
        if word == "@e" {
            targets.extend(
                self.mobs
                    .values()
                    .filter(|m| m.alive())
                    .map(|m| m.entity_id),
            );
        }
        targets
    }

    /// The name of a player or mob, as commands show it.
    fn display_name(&self, entity_id: i32) -> Nbt {
        if self.players.contains_key(&entity_id) {
            self.player_name(entity_id)
        } else {
            self.entity_name(entity_id)
                .unwrap_or_else(|| text::plain("?"))
        }
    }

    /// `effect give`.
    fn effect_give_command(
        &mut self,
        sender: i32,
        target: &str,
        effect: &str,
        rest: &[&str],
    ) -> Reply {
        let data = std::sync::Arc::clone(&self.context.data);
        let name = qualify(effect);
        let Some((id, info)) = data.effect(&name) else {
            return Err(error(
                "argument.resource.not_found",
                vec![text::plain(&name), text::plain("minecraft:mob_effect")],
            ));
        };
        let seconds = match rest.first() {
            None => None,
            Some(&"infinite") => Some(-1),
            Some(word) => Some(
                word.parse::<i32>()
                    .ok()
                    .filter(|s| (1..=1_000_000).contains(s))
                    .ok_or_else(|| error("parsing.int.invalid", vec![text::plain(word)]))?,
            ),
        };
        let amplifier = match rest.get(1) {
            None => 0,
            Some(word) => word
                .parse::<i32>()
                .ok()
                .filter(|a| (0..=255).contains(a))
                .ok_or_else(|| error("parsing.int.invalid", vec![text::plain(word)]))?,
        };
        let hide = match rest.get(2) {
            None => false,
            Some(&"true") => true,
            Some(&"false") => false,
            Some(word) => return Err(error("parsing.bool.invalid", vec![text::plain(word)])),
        };
        // `computeDurationInTicks`.
        let duration = match seconds {
            Some(s) if info.instant => s,
            Some(-1) => -1,
            Some(s) => s * 20,
            None if info.instant => 1,
            None => 600,
        };
        let mut given = Vec::new();
        for entity in self.living_targets(sender, target) {
            let Some(living) = self.living(entity) else {
                continue;
            };
            let mut instance = super::effect::EffectInstance::new(id, duration, amplifier);
            instance.visible = !hide;
            instance.show_icon = !hide;
            if self.add_effect(living, instance) {
                given.push(entity);
            }
        }
        let effect_name = text::translate(
            &format!("effect.{}", name.replace(':', ".")),
            Vec::new(),
            None,
        );
        match given.as_slice() {
            [] => Err(error("commands.effect.give.failed", Vec::new())),
            [one] => Ok(text::translate(
                "commands.effect.give.success.single",
                vec![
                    effect_name,
                    self.display_name(*one),
                    text::plain(&(duration / 20).to_string()),
                ],
                None,
            )),
            many => Ok(text::translate(
                "commands.effect.give.success.multiple",
                vec![
                    effect_name,
                    text::plain(&many.len().to_string()),
                    text::plain(&(duration / 20).to_string()),
                ],
                None,
            )),
        }
    }

    /// `effect clear [<targets>] [<effect>]`.
    fn effect_clear_command(&mut self, sender: i32, rest: &[&str]) -> Reply {
        let data = std::sync::Arc::clone(&self.context.data);
        let targets = match rest.first() {
            None => self.myself(sender)?,
            Some(target) => self.living_targets(sender, target),
        };
        let effect = match rest.get(1) {
            None => None,
            Some(word) => {
                let name = qualify(word);
                let Some((id, _)) = data.effect(&name) else {
                    return Err(error(
                        "argument.resource.not_found",
                        vec![text::plain(&name), text::plain("minecraft:mob_effect")],
                    ));
                };
                Some((id, name))
            }
        };
        let mut cleared = Vec::new();
        for entity in targets {
            let Some(living) = self.living(entity) else {
                continue;
            };
            let done = match &effect {
                Some((id, _)) => self.remove_effect(living, *id),
                None => self.clear_effects(living),
            };
            if done {
                cleared.push(entity);
            }
        }
        match (&effect, cleared.as_slice()) {
            (None, []) => Err(error("commands.effect.clear.everything.failed", Vec::new())),
            (Some(_), []) => Err(error("commands.effect.clear.specific.failed", Vec::new())),
            (None, [one]) => Ok(text::translate(
                "commands.effect.clear.everything.success.single",
                vec![self.display_name(*one)],
                None,
            )),
            (None, many) => Ok(text::translate(
                "commands.effect.clear.everything.success.multiple",
                vec![text::plain(&many.len().to_string())],
                None,
            )),
            (Some((_, name)), [one]) => Ok(text::translate(
                "commands.effect.clear.specific.success.single",
                vec![
                    text::translate(
                        &format!("effect.{}", name.replace(':', ".")),
                        Vec::new(),
                        None,
                    ),
                    self.display_name(*one),
                ],
                None,
            )),
            (Some((_, name)), many) => Ok(text::translate(
                "commands.effect.clear.specific.success.multiple",
                vec![
                    text::translate(
                        &format!("effect.{}", name.replace(':', ".")),
                        Vec::new(),
                        None,
                    ),
                    text::plain(&many.len().to_string()),
                ],
                None,
            )),
        }
    }

    /// `enchant <targets> <enchantment> [<level>]`: the item in the main
    /// hand, if the enchantment suits it and its others.
    fn enchant_command(
        &mut self,
        sender: i32,
        target: &str,
        enchantment: &str,
        rest: &[&str],
    ) -> Reply {
        let data = std::sync::Arc::clone(&self.context.data);
        let name = qualify(enchantment);
        let Some((_, json)) = data.enchantment(&name) else {
            return Err(error(
                "argument.resource.not_found",
                vec![text::plain(&name), text::plain("minecraft:enchantment")],
            ));
        };
        let level = match rest.first() {
            None => 1,
            Some(word) => word
                .parse::<i32>()
                .ok()
                .filter(|l| *l >= 0)
                .ok_or_else(|| error("parsing.int.invalid", vec![text::plain(word)]))?,
        };
        let max = super::enchant::max_level(json);
        if level > max {
            return Err(error(
                "commands.enchant.failed.level",
                vec![
                    text::plain(&level.to_string()),
                    text::plain(&max.to_string()),
                ],
            ));
        }
        let targets = self.living_targets(sender, target);
        let single = targets.len() == 1;
        let mut enchanted = Vec::new();
        for entity in targets {
            let Some(player) = self.players.get(&entity) else {
                if single {
                    return Err(error(
                        "commands.enchant.failed.itemless",
                        vec![self.display_name(entity)],
                    ));
                }
                continue;
            };
            let slot = player.inventory.selected_slot();
            let Some(mut stack) = player.inventory.get(slot).cloned() else {
                if single {
                    return Err(error(
                        "commands.enchant.failed.itemless",
                        vec![self.display_name(entity)],
                    ));
                }
                continue;
            };
            let existing = crate::stack::enchantments(&data, &stack);
            let fits = super::enchant::supports(&data, json, stack.item)
                && existing
                    .iter()
                    .all(|(other, _)| super::enchant::compatible(&data, other, &name));
            if !fits {
                if single {
                    let item = data
                        .item(stack.item)
                        .map_or(String::new(), |i| i.name.clone());
                    return Err(error(
                        "commands.enchant.failed.incompatible",
                        vec![text::translate(
                            &format!("item.{}", item.replace(':', ".")),
                            Vec::new(),
                            None,
                        )],
                    ));
                }
                continue;
            }
            // `ItemEnchantments.Mutable.upgrade`.
            let mut enchantments = existing;
            match enchantments.iter_mut().find(|(n, _)| *n == name) {
                Some((_, l)) => *l = (*l).max(level),
                None => enchantments.push((name.clone(), level)),
            }
            crate::stack::set_enchantments_in(
                &data,
                &mut stack,
                "minecraft:enchantments",
                &enchantments,
            );
            self.update_inventory(entity, |inventory| inventory.set(slot, Some(stack)));
            self.send_slots(entity, &[slot]);
            enchanted.push(entity);
        }
        let full = enchantment_name(&data, &name, level);
        match enchanted.as_slice() {
            [] => Err(error("commands.enchant.failed", Vec::new())),
            [one] => Ok(text::translate(
                "commands.enchant.success.single",
                vec![full, self.display_name(*one)],
                None,
            )),
            many => Ok(text::translate(
                "commands.enchant.success.multiple",
                vec![full, text::plain(&many.len().to_string())],
                None,
            )),
        }
    }

    /// `clear [<targets>] [<item>] [<maxCount>]`: takes items out of
    /// inventories (`Inventory.clearOrCountMatchingItems`), the crafting
    /// grid and the hand; a count of 0 only counts.
    fn clear_command(&mut self, sender: i32, rest: &[&str]) -> Reply {
        let data = std::sync::Arc::clone(&self.context.data);
        let players = match rest.first() {
            None => self.myself(sender)?,
            Some(target) => self.targets(sender, target),
        };
        let matches = |stack: &ItemStack| match rest.get(1) {
            None | Some(&"*") => true,
            Some(word) => match word.strip_prefix('#') {
                Some(tag) => data
                    .item_tag(&qualify(tag))
                    .is_some_and(|items| items.contains(&stack.item)),
                None => {
                    data.item_id(&qualify(word.split('[').next().unwrap_or(word)))
                        == Some(stack.item)
                }
            },
        };
        let max = match rest.get(2) {
            None => -1,
            Some(word) => word
                .parse::<i32>()
                .ok()
                .filter(|m| *m >= 0)
                .ok_or_else(|| error("parsing.int.invalid", vec![text::plain(word)]))?,
        };
        let counting = max == 0;
        let mut total = 0;
        let mut touched = Vec::new();
        for &entity in &players {
            let Some(player) = self.players.get_mut(&entity) else {
                continue;
            };
            let mut left = if max < 0 { i32::MAX } else { max };
            let mut count = 0;
            let mut take = |stack: &mut Option<ItemStack>| {
                let Some(s) = stack.as_mut().filter(|s| matches(s)) else {
                    return;
                };
                if counting {
                    count += s.count;
                    return;
                }
                let taken = s.count.min(left);
                s.count -= taken;
                left -= taken;
                count += taken;
                if s.count == 0 {
                    *stack = None;
                }
            };
            let mut slots: Vec<Option<ItemStack>> = player.inventory.slots().to_vec();
            for slot in slots.iter_mut().skip(5) {
                take(slot);
            }
            for cell in player.inventory_menu.grid.iter_mut() {
                take(cell);
            }
            take(&mut player.carried);
            if !counting {
                let changed: Vec<usize> = (5..slots.len()).collect();
                self.update_inventory(entity, |inventory| {
                    for slot in changed {
                        inventory.set(slot, slots[slot].clone());
                    }
                });
                touched.push(entity);
            }
            total += count;
        }
        for entity in touched {
            self.send_menu(entity);
        }
        let name = |id| self.display_name(id);
        match (players.as_slice(), total) {
            ([one], 0) => Err(error("clear.failed.single", vec![name(*one)])),
            (many, 0) => Err(error(
                "clear.failed.multiple",
                vec![text::plain(&many.len().to_string())],
            )),
            ([one], n) => Ok(text::translate(
                if counting {
                    "commands.clear.test.single"
                } else {
                    "commands.clear.success.single"
                },
                vec![text::plain(&n.to_string()), name(*one)],
                None,
            )),
            (many, n) => Ok(text::translate(
                if counting {
                    "commands.clear.test.multiple"
                } else {
                    "commands.clear.success.multiple"
                },
                vec![
                    text::plain(&n.to_string()),
                    text::plain(&many.len().to_string()),
                ],
                None,
            )),
        }
    }

    fn give_command(&mut self, sender: i32, target: &str, item: &str, rest: &[&str]) -> Reply {
        let stack = parse_item(&self.context.data, item)?;
        let id = stack.item;
        let name = self
            .context
            .data
            .item(id)
            .map_or_else(String::new, |i| i.name.clone());
        let count: i32 = match rest {
            [] => 1,
            [count] => count
                .parse()
                .ok()
                .filter(|c| (1..=6400).contains(c))
                .ok_or_else(|| error("argument.integer.low", Vec::new()))?,
            _ => return Err(error("command.unknown.argument", Vec::new())),
        };
        let receivers = self.targets(sender, target);
        if receivers.is_empty() {
            return Err(error("argument.player.unknown", Vec::new()));
        }
        for &receiver in &receivers {
            self.give(
                receiver,
                ItemStack {
                    count,
                    ..stack.clone()
                },
            );
        }
        let item_name = text::translate(
            &format!("item.{}", name.replace(':', ".")),
            Vec::new(),
            None,
        );
        Ok(text::translate(
            "commands.give.success.single",
            vec![
                text::plain(&count.to_string()),
                item_name,
                self.player_name(receivers[0]),
            ],
            None,
        ))
    }
}

/// The words of a command, keeping brackets, braces and quotes together
/// (`give @s stick[custom_name="a b"]` is three words after `give`).
fn split_arguments(line: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut start = None;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if start.is_none() && !c.is_whitespace() {
            start = Some(i);
        }
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '[' | '{' => depth += 1,
            ']' | '}' => depth -= 1,
            c if c.is_whitespace() && depth <= 0 => {
                if let Some(s) = start.take() {
                    words.push(&line[s..i]);
                }
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        words.push(&line[s..]);
    }
    words
}

/// `ItemParser`: an item id, then components in brackets:
/// `diamond_sword[enchantments={sharpness:5},!repair_cost]`.
fn parse_item(data: &GameData, text: &str) -> Result<ItemStack, Nbt> {
    let (name, components) = match text.find('[') {
        Some(at) => (&text[..at], Some(&text[at + 1..])),
        None => (text, None),
    };
    let name = qualify(name);
    let Some(id) = data.item_id(&name).filter(|&id| id != 0) else {
        return Err(error("argument.item.id.invalid", vec![text::plain(&name)]));
    };
    let mut stack = crate::stack::plain(id, 1);
    let Some(mut rest) = components else {
        return Ok(stack);
    };
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix(']') {
            if !after.trim().is_empty() {
                return Err(error("argument.item.malformed", vec![text::plain(text)]));
            }
            return Ok(stack);
        }
        if let Some(removed) = rest.strip_prefix('!') {
            let end = removed
                .find([',', ']'])
                .ok_or_else(|| error("argument.item.malformed", vec![text::plain(text)]))?;
            crate::stack::remove(data, &mut stack, &qualify(removed[..end].trim()));
            rest = &removed[end..];
        } else {
            let equals = rest
                .find('=')
                .ok_or_else(|| error("argument.item.malformed", vec![text::plain(text)]))?;
            let component = qualify(rest[..equals].trim());
            if data.data_component_type(&component).is_err() {
                return Err(error(
                    "arguments.item.component.unknown",
                    vec![text::plain(&component)],
                ));
            }
            let (value, after) = ferrocraft_protocol::snbt::parse_prefix(&rest[equals + 1..])
                .map_err(|e| error("argument.item.malformed", vec![text::plain(e.message)]))?;
            let shape = ferrocraft_protocol::component::component(&component).ok_or_else(|| {
                error(
                    "arguments.item.component.unknown",
                    vec![text::plain(&component)],
                )
            })?;
            let bytes = shape.to_wire(&value, data).map_err(|_| {
                error(
                    "arguments.item.component.malformed",
                    vec![text::plain(&component), text::plain("")],
                )
            })?;
            let kind = data
                .data_component_type(&component)
                .map_err(|_| error("argument.item.malformed", vec![text::plain(text)]))?;
            stack.patch.added.retain(|(t, _)| *t != kind);
            stack.patch.added.push((kind, bytes));
            crate::stack::normalize(&mut stack);
            rest = after;
        }
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix(',') {
            rest = after;
        }
    }
}

/// `Enchantment.getFullname`: the name, and the level unless it can only
/// be I.
pub(super) fn enchantment_name(data: &GameData, name: &str, level: i32) -> Nbt {
    let key = format!("enchantment.{}", name.replace(':', "."));
    let curse = super::enchant::in_tag(data, name, "minecraft:curse");
    let max = data
        .enchantment(name)
        .map_or(1, |(_, json)| super::enchant::max_level(json));
    let mut parts = vec![text::translate(&key, Vec::new(), None)];
    if level != 1 || max != 1 {
        parts.push(text::plain(" "));
        parts.push(text::translate(
            &format!("enchantment.level.{level}"),
            Vec::new(),
            None,
        ));
    }
    let mut entries = vec![
        ("text".to_owned(), Nbt::String(String::new())),
        ("extra".into(), Nbt::List(parts)),
    ];
    entries.push((
        "color".into(),
        Nbt::String(if curse { "red" } else { "gray" }.into()),
    ));
    Nbt::Compound(entries)
}

/// `minecraft:` in front of an id without a namespace.
fn qualify(name: &str) -> String {
    if name.contains(':') {
        name.to_owned()
    } else {
        format!("minecraft:{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_like_vanilla() {
        assert_eq!(parse_time("1d"), Some(24000));
        assert_eq!(parse_time("2s"), Some(40));
        assert_eq!(parse_time("300"), Some(300));
        assert_eq!(parse_time("x"), None);
        assert_eq!(parse_coordinate("~", 4.5, true), Some(4.5));
        assert_eq!(parse_coordinate("~-2", 4.5, true), Some(2.5));
        assert_eq!(parse_coordinate("10", 0.0, true), Some(10.5));
        assert_eq!(parse_coordinate("10", 0.0, false), Some(10.0));
        assert_eq!(parse_coordinate("10.25", 0.0, true), Some(10.25));
        assert_eq!(parse_mode("spectator"), Some(GameMode::Spectator));
    }

    #[test]
    fn operators_see_more_commands() {
        use ferrocraft_protocol::Encode;
        let data = GameData::load().unwrap();
        let everyone = tree(&data, 0);
        assert_eq!(everyone.nodes.len(), 2, "list and the root");
        let operator = tree(&data, OPERATOR);
        assert!(operator.nodes.len() > 20);
        // Children always come before their parents, the root last.
        assert_eq!(operator.root as usize, operator.nodes.len() - 1);
        for (index, node) in operator.nodes.iter().enumerate() {
            assert!(node.children.iter().all(|&c| (c as usize) < index));
        }
        let mut out = Vec::new();
        operator.encode(&mut out);
        assert!(!out.is_empty());
    }
}
