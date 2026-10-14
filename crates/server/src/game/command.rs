//! Commands: the tree the client completes against and what running them
//! does. Operators (permission level 4) get `gamemode`, `time`, `tp`, `give`
//! and `kill`; everyone gets `list`.

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::packets::play::{
    CommandNode, Commands, ComponentPatch, GameMode, ItemStack, NodeKind, text,
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
    }
    let root = t.add(NodeKind::Root, false, root);
    Commands {
        nodes: t.nodes,
        root,
    }
}

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
    /// Players a selector or name stands for.
    fn targets(&self, sender: i32, word: &str) -> Vec<i32> {
        match word {
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
        let words: Vec<&str> = line.split_whitespace().collect();
        let reply = match words.as_slice() {
            ["list"] => Ok(self.list()),
            [name, ..] if permission < OPERATOR && *name != "list" => {
                Err(error("command.unknown.command", Vec::new()))
            }
            ["gamemode", mode, rest @ ..] => self.gamemode_command(sender, mode, rest),
            ["time", rest @ ..] => self.time_command(rest),
            ["tp" | "teleport", rest @ ..] => self.tp_command(sender, rest),
            ["give", target, item, rest @ ..] => self.give_command(sender, target, item, rest),
            ["summon", entity, rest @ ..] => self.summon_command(sender, entity, rest),
            ["kill", rest @ ..] => {
                let victims = match rest {
                    [] => vec![sender],
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
        };
        let content = match reply {
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
            [] => vec![sender],
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
            [_] | [_, _, _] => (vec![sender], rest),
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

    /// `/summon <entity> [x y z]`, for the animals Ferrocraft knows.
    fn summon_command(&mut self, sender: i32, entity: &str, rest: &[&str]) -> Reply {
        let name = if entity.contains(':') {
            entity.to_owned()
        } else {
            format!("minecraft:{entity}")
        };
        let Some(kind) = super::mob::Kind::from_name(&name) else {
            return Err(error("commands.summon.failed", Vec::new()));
        };
        let base = self.players.get(&sender).map(|p| p.pos).unwrap_or_default();
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
        if self.spawn_mob(kind, pos).is_none() {
            return Err(error("commands.summon.failed", Vec::new()));
        }
        let key = format!("entity.{}", name.replace(':', "."));
        Ok(text::translate(
            "commands.summon.success",
            vec![text::translate(&key, Vec::new(), None)],
            None,
        ))
    }

    fn give_command(&mut self, sender: i32, target: &str, item: &str, rest: &[&str]) -> Reply {
        // Components in brackets are not supported; the item id is enough.
        let name = item.split('[').next().unwrap_or(item);
        let name = if name.contains(':') {
            name.to_owned()
        } else {
            format!("minecraft:{name}")
        };
        let Some(id) = self.context.data.item_id(&name).filter(|&id| id != 0) else {
            return Err(error("argument.item.id.invalid", vec![text::plain(&name)]));
        };
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
                    item: id,
                    count,
                    patch: ComponentPatch::default(),
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
