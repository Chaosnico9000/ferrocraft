//! The server console (`DedicatedServer.handleConsoleInputs`): commands
//! typed into the server's terminal run with an operator's permission from
//! the world spawn, and their answers go to the log in English, the way
//! vanilla's console shows them.

use ferrocraft_protocol::Nbt;
use ferrocraft_protocol::packets::play::{SystemChat, text};
use tracing::info;

use super::Game;

/// The sender id of the console; players' entity ids start at 1.
pub const CONSOLE: i32 = -1;

/// A text component as plain English text (`Component.getString`).
pub fn render(component: &Nbt, language: &std::collections::HashMap<String, String>) -> String {
    match component {
        Nbt::String(s) => s.clone(),
        Nbt::List(parts) => parts.iter().map(|p| render(p, language)).collect(),
        Nbt::Compound(_) => {
            let mut out = String::new();
            if let Some(Nbt::String(s)) = component.get("text") {
                out.push_str(s);
            }
            if let Some(Nbt::String(key)) = component.get("translate") {
                let args: Vec<String> = match component.get("with") {
                    Some(Nbt::List(with)) => with.iter().map(|a| render(a, language)).collect(),
                    _ => Vec::new(),
                };
                let fallback = match component.get("fallback") {
                    Some(Nbt::String(f)) => Some(f.as_str()),
                    _ => None,
                };
                let format = language
                    .get(key)
                    .map(String::as_str)
                    .or(fallback)
                    .unwrap_or(key);
                out.push_str(&format_args(format, &args));
            }
            if let Some(Nbt::List(extra)) = component.get("extra") {
                for part in extra {
                    out.push_str(&render(part, language));
                }
            }
            out
        }
        other => format!("{other:?}"),
    }
}

/// `TranslatableContents.decomposeTemplate`: `%s` takes the next argument,
/// `%2$s` the second, `%%` is a percent sign.
fn format_args(format: &str, args: &[String]) -> String {
    let mut out = String::new();
    let mut next = 0;
    let mut rest = format;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        if let Some(after) = rest.strip_prefix('%') {
            out.push('%');
            rest = after;
            continue;
        }
        if let Some(after) = rest.strip_prefix('s') {
            out.push_str(args.get(next).map_or("", String::as_str));
            next += 1;
            rest = after;
            continue;
        }
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty()
            && let Some(after) = rest[digits.len()..].strip_prefix("$s")
        {
            let index: usize = digits.parse().unwrap_or(1);
            out.push_str(args.get(index.saturating_sub(1)).map_or("", String::as_str));
            rest = after;
            continue;
        }
        out.push('%');
    }
    out.push_str(rest);
    out
}

impl Game {
    /// A line typed into the console; what it answered, in English.
    pub(super) fn console_command(&mut self, line: &str) -> String {
        let line = line.trim().trim_start_matches('/');
        let reply = match line.split_whitespace().collect::<Vec<_>>().as_slice() {
            ["say", words @ ..] if !words.is_empty() => {
                // `chat.type.announcement`: `[Server] message` for everyone.
                let message = text::translate(
                    "chat.type.announcement",
                    vec![text::plain("Server"), text::plain(&words.join(" "))],
                    None,
                );
                self.broadcast_packet(
                    None,
                    &SystemChat {
                        content: message.clone(),
                        overlay: false,
                    },
                );
                message
            }
            ["save-all", ..] => {
                self.save_all();
                text::translate("commands.save.success", Vec::new(), None)
            }
            _ => match self.execute(CONSOLE, super::OPERATOR, line) {
                Ok(message) | Err(message) => message,
            },
        };
        let rendered = render(&reply, &self.context.data.language);
        info!("{rendered}");
        rendered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_render_like_vanilla() {
        let mut language = std::collections::HashMap::new();
        language.insert("a.b".to_owned(), "%s and %s, %2$s again: 100%%".to_owned());
        let component = text::translate("a.b", vec![text::plain("one"), text::plain("two")], None);
        assert_eq!(
            render(&component, &language),
            "one and two, two again: 100%"
        );
        let unknown = text::translate("missing.key", Vec::new(), None);
        assert_eq!(render(&unknown, &language), "missing.key");
    }
}
