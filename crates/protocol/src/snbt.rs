//! Stringified NBT (`TagParser`), as commands write it:
//! `{name:"Steve",level:3b,ids:[I;1,2]}`. Numbers take a type suffix
//! (`b`, `s`, `l`, `f`, `d`); without one, whole numbers are ints and the
//! rest doubles. `true` and `false` are bytes. Unquoted words are strings.

use crate::nbt::Nbt;

/// Why a text is not SNBT, with where it went wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub message: &'static str,
    pub at: usize,
}

type Result<T> = std::result::Result<T, Error>;

/// Parses a whole text as one tag.
pub fn parse(text: &str) -> Result<Nbt> {
    let mut parser = Parser { text, at: 0 };
    let value = parser.value()?;
    parser.skip_space();
    if parser.at != text.len() {
        return Err(parser.error("trailing characters"));
    }
    Ok(value)
}

/// Parses one tag from the start of a text; returns it and the rest.
pub fn parse_prefix(text: &str) -> Result<(Nbt, &str)> {
    let mut parser = Parser { text, at: 0 };
    let value = parser.value()?;
    Ok((value, &text[parser.at..]))
}

struct Parser<'a> {
    text: &'a str,
    at: usize,
}

/// Characters an unquoted word may hold (`StringReader.isAllowedInUnquotedString`
/// and the namespaced ids of 26.3).
fn unquoted(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+' | ':' | '/')
}

impl Parser<'_> {
    fn error(&self, message: &'static str) -> Error {
        Error {
            message,
            at: self.at,
        }
    }

    fn peek(&self) -> Option<char> {
        self.text[self.at..].chars().next()
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.at += 1;
        }
    }

    fn expect(&mut self, c: char) -> Result<()> {
        self.skip_space();
        if self.peek() == Some(c) {
            self.at += c.len_utf8();
            Ok(())
        } else {
            Err(self.error("expected another character"))
        }
    }

    fn value(&mut self) -> Result<Nbt> {
        self.skip_space();
        match self.peek() {
            Some('{') => self.compound(),
            Some('[') => self.list(),
            Some('"' | '\'') => Ok(Nbt::String(self.quoted()?)),
            Some(_) => {
                let word = self.word();
                if word.is_empty() {
                    return Err(self.error("expected a value"));
                }
                Ok(scalar(word))
            }
            None => Err(self.error("expected a value")),
        }
    }

    fn word(&mut self) -> &str {
        self.word_of(unquoted)
    }

    fn word_of(&mut self, allowed: fn(char) -> bool) -> &str {
        let start = self.at;
        while self.peek().is_some_and(allowed) {
            self.at += 1;
        }
        &self.text[start..self.at]
    }

    fn quoted(&mut self) -> Result<String> {
        let quote = self.peek().expect("a quote");
        self.at += 1;
        let mut out = String::new();
        loop {
            let Some(c) = self.peek() else {
                return Err(self.error("unclosed string"));
            };
            self.at += c.len_utf8();
            match c {
                '\\' => {
                    let Some(escaped) = self.peek() else {
                        return Err(self.error("unclosed string"));
                    };
                    self.at += escaped.len_utf8();
                    out.push(match escaped {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                }
                c if c == quote => return Ok(out),
                c => out.push(c),
            }
        }
    }

    fn key(&mut self) -> Result<String> {
        self.skip_space();
        match self.peek() {
            Some('"' | '\'') => self.quoted(),
            _ => {
                // Keys stop at the colon after them.
                let word = self.word_of(|c| unquoted(c) && c != ':');
                if word.is_empty() {
                    Err(self.error("expected a key"))
                } else {
                    Ok(word.to_owned())
                }
            }
        }
    }

    fn compound(&mut self) -> Result<Nbt> {
        self.expect('{')?;
        let mut entries: Vec<(String, Nbt)> = Vec::new();
        self.skip_space();
        if self.peek() == Some('}') {
            self.at += 1;
            return Ok(Nbt::Compound(entries));
        }
        loop {
            let key = self.key()?;
            self.expect(':')?;
            let value = self.value()?;
            entries.retain(|(k, _)| *k != key);
            entries.push((key, value));
            self.skip_space();
            match self.peek() {
                Some(',') => self.at += 1,
                Some('}') => {
                    self.at += 1;
                    return Ok(Nbt::Compound(entries));
                }
                _ => return Err(self.error("expected , or }")),
            }
        }
    }

    fn list(&mut self) -> Result<Nbt> {
        self.expect('[')?;
        self.skip_space();
        // `[B;`, `[I;`, `[L;`: typed arrays.
        let rest = &self.text[self.at..];
        let array = match rest.as_bytes() {
            [k @ (b'B' | b'I' | b'L'), b';', ..] => Some(*k),
            _ => None,
        };
        if array.is_some() {
            self.at += 2;
        }
        let mut items = Vec::new();
        self.skip_space();
        if self.peek() == Some(']') {
            self.at += 1;
        } else {
            loop {
                items.push(self.value()?);
                self.skip_space();
                match self.peek() {
                    Some(',') => self.at += 1,
                    Some(']') => {
                        self.at += 1;
                        break;
                    }
                    _ => return Err(self.error("expected , or ]")),
                }
            }
        }
        let number = |n: &Nbt| match n {
            Nbt::Byte(v) => Some(i64::from(*v)),
            Nbt::Short(v) => Some(i64::from(*v)),
            Nbt::Int(v) => Some(i64::from(*v)),
            Nbt::Long(v) => Some(*v),
            _ => None,
        };
        let numbers = || {
            items
                .iter()
                .map(|n| number(n).ok_or(self.error("arrays hold numbers")))
                .collect::<Result<Vec<i64>>>()
        };
        Ok(match array {
            Some(b'B') => Nbt::ByteArray(numbers()?.into_iter().map(|v| v as i8).collect()),
            Some(b'I') => Nbt::IntArray(numbers()?.into_iter().map(|v| v as i32).collect()),
            Some(_) => Nbt::LongArray(numbers()?),
            None => Nbt::List(items),
        })
    }
}

/// A word without quotes: a number with its type, a boolean, or a string.
fn scalar(word: &str) -> Nbt {
    match word {
        "true" => return Nbt::Byte(1),
        "false" => return Nbt::Byte(0),
        _ => {}
    }
    let lower = word.to_ascii_lowercase();
    let (body, suffix) = match lower.chars().last() {
        Some(c @ ('b' | 's' | 'l' | 'f' | 'd')) => (&lower[..lower.len() - 1], Some(c)),
        _ => (lower.as_str(), None),
    };
    let integer = body.parse::<i64>().ok();
    let float = body
        .parse::<f64>()
        .ok()
        .filter(|_| body.chars().any(|c| c.is_ascii_digit()));
    match (suffix, integer, float) {
        (Some('b'), Some(v), _) => Nbt::Byte(v as i8),
        (Some('s'), Some(v), _) => Nbt::Short(v as i16),
        (Some('l'), Some(v), _) => Nbt::Long(v),
        (Some('f'), _, Some(v)) => Nbt::Float(v as f32),
        (Some('d'), _, Some(v)) => Nbt::Double(v),
        (None, Some(v), _) if i32::try_from(v).is_ok() => Nbt::Int(v as i32),
        (None, _, Some(v)) if body.contains('.') || body.contains('e') => Nbt::Double(v),
        _ => Nbt::String(word.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snbt_like_tag_parser() {
        assert_eq!(
            parse(r#"{a:1,b:2.5,c:3b,d:"x y",e:[1,2],f:{},g:minecraft:stone,h:true}"#).unwrap(),
            Nbt::Compound(vec![
                ("a".into(), Nbt::Int(1)),
                ("b".into(), Nbt::Double(2.5)),
                ("c".into(), Nbt::Byte(3)),
                ("d".into(), Nbt::String("x y".into())),
                ("e".into(), Nbt::List(vec![Nbt::Int(1), Nbt::Int(2)])),
                ("f".into(), Nbt::Compound(vec![])),
                ("g".into(), Nbt::String("minecraft:stone".into())),
                ("h".into(), Nbt::Byte(1)),
            ])
        );
        assert_eq!(parse("[I; 1, -2]").unwrap(), Nbt::IntArray(vec![1, -2]));
        assert_eq!(parse("1.5f").unwrap(), Nbt::Float(1.5));
        assert_eq!(parse(r"'it\'s'").unwrap(), Nbt::String("it's".into()));
        assert!(parse("{a:1").is_err());
        let (value, rest) = parse_prefix("{sharpness:5},unbreakable={}").unwrap();
        assert_eq!(
            value,
            Nbt::Compound(vec![("sharpness".into(), Nbt::Int(5))])
        );
        assert_eq!(rest, ",unbreakable={}");
    }
}
