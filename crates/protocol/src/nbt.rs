//! Named Binary Tag. On the network the root tag carries a type byte but no
//! name (`NbtIo.writeAnyTag`, since 1.20.2); in files it is named, usually
//! with the empty string (`NbtIo.write`). Strings use Java's modified UTF-8.

use crate::codec::{Decode, Encode, Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Nbt {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    /// Elements of mixed types are written as a list of compounds, each
    /// non-compound wrapped as `{"": value}` (`ListTag` in 26.3);
    /// reading unwraps them again. An empty list is written as `End`.
    List(Vec<Nbt>),
    /// Kept in insertion order, which is the order vanilla writes.
    Compound(Vec<(String, Nbt)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

/// Nesting limit vanilla's `NbtAccounter` enforces.
const MAX_DEPTH: usize = 512;

impl Nbt {
    fn type_id(&self) -> u8 {
        match self {
            Nbt::Byte(_) => 1,
            Nbt::Short(_) => 2,
            Nbt::Int(_) => 3,
            Nbt::Long(_) => 4,
            Nbt::Float(_) => 5,
            Nbt::Double(_) => 6,
            Nbt::ByteArray(_) => 7,
            Nbt::String(_) => 8,
            Nbt::List(_) => 9,
            Nbt::Compound(_) => 10,
            Nbt::IntArray(_) => 11,
            Nbt::LongArray(_) => 12,
        }
    }

    /// A plain text component, e.g. a disconnect reason.
    pub fn text(text: &str) -> Self {
        Nbt::Compound(vec![("text".into(), Nbt::String(text.into()))])
    }

    /// Looks up a key in a compound.
    pub fn get(&self, key: &str) -> Option<&Nbt> {
        match self {
            Nbt::Compound(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// A compound whose only key is the empty string: how a mixed list
    /// carries a non-compound element.
    fn is_wrapper(&self) -> bool {
        matches!(self, Nbt::Compound(entries) if entries.len() == 1 && entries[0].0.is_empty())
    }

    fn unwrapped(self) -> Nbt {
        match self {
            Nbt::Compound(mut entries) if entries.len() == 1 && entries[0].0.is_empty() => {
                entries.pop().expect("one entry").1
            }
            other => other,
        }
    }

    /// The file form: type byte, root name, payload.
    pub fn write_named(&self, name: &str, out: &mut Vec<u8>) {
        out.push(self.type_id());
        encode_mutf8(name, out);
        self.encode_payload(out);
    }

    /// Reads the file form, returning the root name and the tag.
    pub fn read_named(buf: &mut &[u8]) -> Result<(String, Self)> {
        match u8::decode(buf)? {
            0 => Err(Error::Invalid("NBT root is End")),
            id => {
                let name = decode_mutf8(buf)?;
                Ok((name, Self::decode_payload(id, buf, 0)?))
            }
        }
    }

    fn encode_payload(&self, out: &mut Vec<u8>) {
        match self {
            Nbt::Byte(v) => v.encode(out),
            Nbt::Short(v) => v.encode(out),
            Nbt::Int(v) => v.encode(out),
            Nbt::Long(v) => v.encode(out),
            Nbt::Float(v) => v.encode(out),
            Nbt::Double(v) => v.encode(out),
            Nbt::ByteArray(v) => {
                (v.len() as i32).encode(out);
                out.extend(v.iter().map(|&b| b as u8));
            }
            Nbt::String(v) => encode_mutf8(v, out),
            Nbt::List(items) => {
                let first = items.first().map_or(0, Nbt::type_id);
                let mixed = items.iter().any(|item| item.type_id() != first);
                let element = if mixed { 10 } else { first };
                out.push(element);
                (items.len() as i32).encode(out);
                for item in items {
                    // `ListTag.wrapIfNeeded`.
                    if element == 10 && (item.type_id() != 10 || item.is_wrapper()) {
                        out.push(item.type_id());
                        encode_mutf8("", out);
                        item.encode_payload(out);
                        out.push(0);
                    } else {
                        item.encode_payload(out);
                    }
                }
            }
            Nbt::Compound(entries) => {
                for (key, value) in entries {
                    out.push(value.type_id());
                    encode_mutf8(key, out);
                    value.encode_payload(out);
                }
                out.push(0);
            }
            Nbt::IntArray(v) => {
                (v.len() as i32).encode(out);
                v.iter().for_each(|x| x.encode(out));
            }
            Nbt::LongArray(v) => {
                (v.len() as i32).encode(out);
                v.iter().for_each(|x| x.encode(out));
            }
        }
    }

    fn decode_payload(type_id: u8, buf: &mut &[u8], depth: usize) -> Result<Self> {
        if depth > MAX_DEPTH {
            return Err(Error::Invalid("NBT nested too deeply"));
        }
        Ok(match type_id {
            1 => Nbt::Byte(i8::decode(buf)?),
            2 => Nbt::Short(i16::decode(buf)?),
            3 => Nbt::Int(i32::decode(buf)?),
            4 => Nbt::Long(i64::decode(buf)?),
            5 => Nbt::Float(f32::decode(buf)?),
            6 => Nbt::Double(f64::decode(buf)?),
            7 => Nbt::ByteArray(decode_array(buf, i8::decode)?),
            8 => Nbt::String(decode_mutf8(buf)?),
            9 => {
                let element = u8::decode(buf)?;
                let len = array_len(buf)?;
                if element == 0 && len > 0 {
                    return Err(Error::Invalid("NBT list of End tags"));
                }
                let items = (0..len)
                    .map(|_| Self::decode_payload(element, buf, depth + 1).map(Nbt::unwrapped))
                    .collect::<Result<_>>()?;
                Nbt::List(items)
            }
            10 => {
                let mut entries = Vec::new();
                loop {
                    let id = u8::decode(buf)?;
                    if id == 0 {
                        break Nbt::Compound(entries);
                    }
                    let key = decode_mutf8(buf)?;
                    entries.push((key, Self::decode_payload(id, buf, depth + 1)?));
                }
            }
            11 => Nbt::IntArray(decode_array(buf, i32::decode)?),
            12 => Nbt::LongArray(decode_array(buf, i64::decode)?),
            _ => return Err(Error::Invalid("unknown NBT tag type")),
        })
    }
}

/// Network NBT: type byte, then the payload, no root name.
impl Encode for Nbt {
    fn encode(&self, out: &mut Vec<u8>) {
        out.push(self.type_id());
        self.encode_payload(out);
    }
}

impl Decode for Nbt {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        match u8::decode(buf)? {
            0 => Err(Error::Invalid("NBT root is End")),
            id => Self::decode_payload(id, buf, 0),
        }
    }
}

fn array_len(buf: &mut &[u8]) -> Result<usize> {
    let len = i32::decode(buf)?;
    let len = usize::try_from(len).map_err(|_| Error::Invalid("negative NBT length"))?;
    // Every element takes at least one byte, so a longer array cannot be real.
    if len > buf.len() {
        return Err(Error::UnexpectedEof);
    }
    Ok(len)
}

fn decode_array<T>(buf: &mut &[u8], item: fn(&mut &[u8]) -> Result<T>) -> Result<Vec<T>> {
    let len = array_len(buf)?;
    (0..len).map(|_| item(buf)).collect()
}

/// Java's modified UTF-8 (`DataOutput.writeUTF`): a `u16` byte length, NUL as
/// two bytes, and characters outside the BMP as two encoded surrogates.
fn encode_mutf8(s: &str, out: &mut Vec<u8>) {
    let start = out.len();
    out.extend_from_slice(&[0, 0]);
    for unit in s.encode_utf16() {
        match unit {
            1..=0x7f => out.push(unit as u8),
            0 | 0x80..=0x7ff => {
                out.push(0xc0 | (unit >> 6) as u8);
                out.push(0x80 | (unit & 0x3f) as u8);
            }
            _ => {
                out.push(0xe0 | (unit >> 12) as u8);
                out.push(0x80 | ((unit >> 6) & 0x3f) as u8);
                out.push(0x80 | (unit & 0x3f) as u8);
            }
        }
    }
    let len = out.len() - start - 2;
    let len = u16::try_from(len).expect("NBT string longer than 65535 bytes");
    out[start..start + 2].copy_from_slice(&len.to_be_bytes());
}

fn decode_mutf8(buf: &mut &[u8]) -> Result<String> {
    let len = usize::from(u16::decode(buf)?);
    if buf.len() < len {
        return Err(Error::UnexpectedEof);
    }
    let (bytes, rest) = buf.split_at(len);
    *buf = rest;
    let invalid = || Error::Invalid("malformed modified UTF-8");
    let mut units = Vec::with_capacity(len);
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let cont = |j: usize| match bytes.get(i + j) {
            Some(&c) if c & 0xc0 == 0x80 => Ok(u16::from(c & 0x3f)),
            _ => Err(invalid()),
        };
        let (unit, used) = match b {
            0x01..=0x7f => (u16::from(b), 1),
            0xc0..=0xdf => ((u16::from(b & 0x1f) << 6) | cont(1)?, 2),
            0xe0..=0xef => ((u16::from(b & 0x0f) << 12) | (cont(1)? << 6) | cont(2)?, 3),
            _ => return Err(invalid()),
        };
        units.push(unit);
        i += used;
    }
    String::from_utf16(&units).map_err(|_| invalid())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(value: Nbt) {
        let mut out = Vec::new();
        value.encode(&mut out);
        let mut buf = out.as_slice();
        assert_eq!(Nbt::decode(&mut buf).unwrap(), value);
        assert!(buf.is_empty());
    }

    #[test]
    fn text_component_matches_vanilla_bytes() {
        // TRUSTED_CONTEXT_FREE_STREAM_CODEC of Component.literal("Hi"):
        // compound, string tag "text" = "Hi", end.
        let mut out = Vec::new();
        Nbt::text("Hi").encode(&mut out);
        assert_eq!(
            out,
            [
                0x0a, 0x08, 0x00, 0x04, b't', b'e', b'x', b't', 0x00, 0x02, b'H', b'i', 0x00
            ]
        );
    }

    #[test]
    fn modified_utf8_edge_cases() {
        let mut out = Vec::new();
        encode_mutf8("\0", &mut out);
        assert_eq!(out, [0, 2, 0xc0, 0x80], "NUL takes two bytes");

        out.clear();
        encode_mutf8("⛏", &mut out);
        assert_eq!(out, [0, 3, 0xe2, 0x9b, 0x8f]);

        out.clear();
        encode_mutf8("😀", &mut out);
        assert_eq!(
            out.len(),
            2 + 6,
            "supplementary char is two 3-byte surrogates"
        );
        assert_eq!(decode_mutf8(&mut out.as_slice()).unwrap(), "😀");
    }

    #[test]
    fn all_tag_types_roundtrip() {
        roundtrip(Nbt::Compound(vec![
            ("b".into(), Nbt::Byte(-1)),
            ("s".into(), Nbt::Short(300)),
            ("i".into(), Nbt::Int(-70000)),
            ("l".into(), Nbt::Long(1 << 40)),
            ("f".into(), Nbt::Float(0.5)),
            ("d".into(), Nbt::Double(-2.25)),
            ("ba".into(), Nbt::ByteArray(vec![1, -2])),
            ("str".into(), Nbt::String("Grüße\0😀".into())),
            ("empty".into(), Nbt::List(vec![])),
            ("list".into(), Nbt::List(vec![Nbt::Int(1), Nbt::Int(2)])),
            ("ia".into(), Nbt::IntArray(vec![i32::MIN, 0])),
            ("la".into(), Nbt::LongArray(vec![i64::MAX])),
            ("nested".into(), Nbt::text("x")),
        ]));
    }

    #[test]
    fn mixed_lists_wrap_their_odd_elements() {
        // A 26.3 palette: a default state by name, another with properties.
        let state = Nbt::Compound(vec![("id".into(), Nbt::String("minecraft:oak_log".into()))]);
        let list = Nbt::List(vec![Nbt::String("minecraft:air".into()), state.clone()]);
        let mut out = Vec::new();
        list.encode(&mut out);
        // List of compounds, two of them; the string wrapped under "".
        assert_eq!(&out[..10], [9, 10, 0, 0, 0, 2, 8, 0, 0, 0]);
        let mut buf = out.as_slice();
        assert_eq!(Nbt::decode(&mut buf).unwrap(), list);
        assert!(buf.is_empty());

        // A compound that looks like a wrapper is wrapped itself, so that it
        // survives.
        let lookalike = Nbt::Compound(vec![(String::new(), Nbt::Int(2))]);
        for tricky in [
            Nbt::List(vec![Nbt::Int(1), lookalike.clone()]),
            Nbt::List(vec![lookalike.clone(), lookalike]),
        ] {
            let mut out = Vec::new();
            tricky.encode(&mut out);
            assert_eq!(Nbt::decode(&mut out.as_slice()).unwrap(), tricky);
        }
    }

    #[test]
    fn named_root_for_files() {
        let tag = Nbt::Compound(vec![("DataVersion".into(), Nbt::Int(5023))]);
        let mut out = Vec::new();
        tag.write_named("", &mut out);
        assert_eq!(&out[..3], [0x0a, 0x00, 0x00], "compound named \"\"");
        let mut buf = out.as_slice();
        assert_eq!(Nbt::read_named(&mut buf).unwrap(), (String::new(), tag));
        assert!(buf.is_empty());
    }

    #[test]
    fn hostile_lengths_are_rejected() {
        // A list claiming two billion entries in a five-byte buffer.
        let bytes = [0x09, 0x01, 0x7f, 0xff, 0xff, 0xff];
        assert_eq!(Nbt::decode(&mut &bytes[..]), Err(Error::UnexpectedEof));
    }
}
