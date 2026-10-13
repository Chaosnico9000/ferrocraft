//! Primitive wire types: how a value is written to and read from a packet body.

use uuid::Uuid;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("packet ended early")]
    UnexpectedEof,
    #[error("VarInt is longer than 5 bytes")]
    VarIntTooLong,
    #[error("string has {len} chars, at most {max} allowed")]
    StringTooLong { len: usize, max: usize },
    #[error("string is not valid UTF-8")]
    InvalidUtf8,
    #[error("frame of {0} bytes exceeds the protocol limit")]
    FrameTooLarge(usize),
    #[error("{0}")]
    Invalid(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;

pub trait Encode {
    fn encode(&self, out: &mut Vec<u8>);
}

pub trait Decode: Sized {
    /// Reads a value from the front of `buf` and advances it past the value.
    fn decode(buf: &mut &[u8]) -> Result<Self>;
}

fn take<'a>(buf: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
    if buf.len() < n {
        return Err(Error::UnexpectedEof);
    }
    let (head, tail) = buf.split_at(n);
    *buf = tail;
    Ok(head)
}

/// A signed 32-bit integer in LEB128-style encoding, 1 to 5 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VarInt(pub i32);

impl VarInt {
    pub const MAX_LEN: usize = 5;

    /// Number of bytes this value takes on the wire.
    pub fn encoded_len(self) -> usize {
        match self.0 as u32 {
            0..=0x7f => 1,
            0x80..=0x3fff => 2,
            0x4000..=0x1f_ffff => 3,
            0x20_0000..=0xfff_ffff => 4,
            _ => 5,
        }
    }

    /// Like [`Decode::decode`], but tells a truncated VarInt (`Ok(None)`) apart
    /// from a malformed one, so a stream reader knows to wait for more bytes.
    pub fn decode_partial(buf: &[u8]) -> Result<Option<(Self, usize)>> {
        let mut value: u32 = 0;
        for i in 0..Self::MAX_LEN {
            let Some(&byte) = buf.get(i) else {
                return Ok(None);
            };
            value |= u32::from(byte & 0x7f) << (7 * i);
            if byte & 0x80 == 0 {
                return Ok(Some((VarInt(value as i32), i + 1)));
            }
        }
        Err(Error::VarIntTooLong)
    }
}

impl Encode for VarInt {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut value = self.0 as u32;
        loop {
            if value & !0x7f == 0 {
                out.push(value as u8);
                return;
            }
            out.push((value & 0x7f) as u8 | 0x80);
            value >>= 7;
        }
    }
}

impl Decode for VarInt {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        match Self::decode_partial(buf)? {
            Some((value, len)) => {
                *buf = &buf[len..];
                Ok(value)
            }
            None => Err(Error::UnexpectedEof),
        }
    }
}

macro_rules! big_endian {
    ($($t:ty),*) => {$(
        impl Encode for $t {
            fn encode(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_be_bytes());
            }
        }

        impl Decode for $t {
            fn decode(buf: &mut &[u8]) -> Result<Self> {
                let bytes = take(buf, size_of::<$t>())?;
                Ok(<$t>::from_be_bytes(bytes.try_into().expect("length checked by take")))
            }
        }
    )*};
}

big_endian!(u8, i8, u16, i16, i32, i64, u128, f32, f64);

impl Encode for bool {
    fn encode(&self, out: &mut Vec<u8>) {
        out.push(u8::from(*self));
    }
}

impl Decode for bool {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        match u8::decode(buf)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::Invalid("bool must be 0 or 1")),
        }
    }
}

impl Encode for Uuid {
    fn encode(&self, out: &mut Vec<u8>) {
        self.as_u128().encode(out);
    }
}

impl Decode for Uuid {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        u128::decode(buf).map(Uuid::from_u128)
    }
}

/// Longest string the protocol allows unless a field says otherwise.
pub const MAX_STRING_CHARS: usize = 32767;

impl Encode for str {
    fn encode(&self, out: &mut Vec<u8>) {
        VarInt(self.len() as i32).encode(out);
        out.extend_from_slice(self.as_bytes());
    }
}

impl Encode for String {
    fn encode(&self, out: &mut Vec<u8>) {
        self.as_str().encode(out);
    }
}

impl Decode for String {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        decode_string(buf, MAX_STRING_CHARS)
    }
}

/// Reads a string capped at `max_chars`. Vanilla counts the limit in UTF-16
/// code units and allows up to three bytes per unit on the wire.
pub fn decode_string(buf: &mut &[u8], max_chars: usize) -> Result<String> {
    let len = VarInt::decode(buf)?.0;
    let len = usize::try_from(len).map_err(|_| Error::Invalid("negative string length"))?;
    if len > max_chars * 3 {
        return Err(Error::StringTooLong {
            len,
            max: max_chars,
        });
    }
    let bytes = take(buf, len)?;
    let s = std::str::from_utf8(bytes).map_err(|_| Error::InvalidUtf8)?;
    let chars = s.encode_utf16().count();
    if chars > max_chars {
        return Err(Error::StringTooLong {
            len: chars,
            max: max_chars,
        });
    }
    Ok(s.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip<T: Encode + Decode + PartialEq + std::fmt::Debug>(value: T) {
        let mut out = Vec::new();
        value.encode(&mut out);
        let mut buf = out.as_slice();
        assert_eq!(T::decode(&mut buf).unwrap(), value);
        assert!(buf.is_empty(), "decode left {} bytes", buf.len());
    }

    #[test]
    fn varint_matches_reference_encodings() {
        // Sample values from the protocol documentation.
        let cases: &[(i32, &[u8])] = &[
            (0, &[0x00]),
            (1, &[0x01]),
            (127, &[0x7f]),
            (128, &[0x80, 0x01]),
            (255, &[0xff, 0x01]),
            (25565, &[0xdd, 0xc7, 0x01]),
            (2097151, &[0xff, 0xff, 0x7f]),
            (i32::MAX, &[0xff, 0xff, 0xff, 0xff, 0x07]),
            (-1, &[0xff, 0xff, 0xff, 0xff, 0x0f]),
            (i32::MIN, &[0x80, 0x80, 0x80, 0x80, 0x08]),
        ];
        for &(value, bytes) in cases {
            let mut out = Vec::new();
            VarInt(value).encode(&mut out);
            assert_eq!(out, bytes, "encoding {value}");
            assert_eq!(VarInt(value).encoded_len(), bytes.len(), "len of {value}");
            assert_eq!(VarInt::decode(&mut &bytes[..]).unwrap(), VarInt(value));
        }
    }

    #[test]
    fn varint_partial_and_overlong() {
        assert_eq!(VarInt::decode_partial(&[0x80]).unwrap(), None);
        assert_eq!(VarInt::decode_partial(&[]).unwrap(), None);
        assert_eq!(
            VarInt::decode_partial(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]),
            Err(Error::VarIntTooLong)
        );
    }

    #[test]
    fn primitives_roundtrip() {
        roundtrip(25565u16);
        roundtrip(-42i64);
        roundtrip(true);
        roundtrip(String::from("Grüße, Welt ⛏"));
        roundtrip(Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef));
    }

    #[test]
    fn string_limit_counts_utf16_units() {
        let mut out = Vec::new();
        "abcdef".encode(&mut out);
        assert_eq!(
            decode_string(&mut out.as_slice(), 5),
            Err(Error::StringTooLong { len: 6, max: 5 })
        );
        assert_eq!(decode_string(&mut out.as_slice(), 6).unwrap(), "abcdef");
    }
}
