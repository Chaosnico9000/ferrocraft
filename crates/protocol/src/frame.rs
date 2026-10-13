//! Packet framing on the TCP stream: `VarInt length` followed by
//! `VarInt packet id` and the body. Compression and encryption are not
//! negotiated yet, so frames are always plain.

use crate::codec::{Decode, Encode, Error, Result, VarInt};

/// Largest frame vanilla accepts: the length must fit in a 3-byte VarInt.
pub const MAX_FRAME_LEN: usize = 2_097_151;

/// One decoded frame: the packet id and its still-encoded body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPacket {
    pub id: i32,
    pub body: Vec<u8>,
}

/// Tries to cut one frame off the front of `buf`. Returns `Ok(None)` while the
/// frame is still incomplete, otherwise the packet and how many bytes it used.
pub fn decode_frame(buf: &[u8]) -> Result<Option<(RawPacket, usize)>> {
    let Some((VarInt(len), header)) = VarInt::decode_partial(buf)? else {
        return Ok(None);
    };
    let len = usize::try_from(len).map_err(|_| Error::Invalid("negative frame length"))?;
    if len > MAX_FRAME_LEN {
        return Err(Error::FrameTooLarge(len));
    }
    let Some(frame) = buf.get(header..header + len) else {
        return Ok(None);
    };
    let mut payload = frame;
    let VarInt(id) = VarInt::decode(&mut payload)?;
    Ok(Some((
        RawPacket {
            id,
            body: payload.to_vec(),
        },
        header + len,
    )))
}

/// Encodes a packet id and body into one length-prefixed frame.
pub fn encode_frame(id: i32, body: &[u8], out: &mut Vec<u8>) {
    let id = VarInt(id);
    VarInt((id.encoded_len() + body.len()) as i32).encode(out);
    id.encode(out);
    out.extend_from_slice(body);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip_and_partial_reads() {
        let mut out = Vec::new();
        encode_frame(0x01, &[1, 2, 3], &mut out);
        assert_eq!(out, [4, 0x01, 1, 2, 3]);

        for cut in 0..out.len() {
            assert_eq!(decode_frame(&out[..cut]).unwrap(), None, "cut at {cut}");
        }
        let (packet, used) = decode_frame(&out).unwrap().unwrap();
        assert_eq!(used, out.len());
        assert_eq!(
            packet,
            RawPacket {
                id: 1,
                body: vec![1, 2, 3]
            }
        );
    }

    #[test]
    fn oversized_frame_is_rejected_before_buffering() {
        let mut out = Vec::new();
        VarInt(MAX_FRAME_LEN as i32 + 1).encode(&mut out);
        assert_eq!(
            decode_frame(&out),
            Err(Error::FrameTooLarge(MAX_FRAME_LEN + 1))
        );
    }
}
