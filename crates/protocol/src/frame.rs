//! Packet framing on the TCP stream: `VarInt length` followed by
//! `VarInt packet id` and the body. After `login_compression` every frame
//! carries one more `VarInt`, the uncompressed size, and the packet behind it
//! is zlib-compressed if it reached the threshold (`CompressionEncoder`,
//! `CompressionDecoder`).

use flate2::{Compress, Decompress, FlushCompress, FlushDecompress, Status};

use crate::codec::{Decode, Encode, Error, Result, VarInt};

/// Largest frame vanilla accepts: the length must fit in a 3-byte VarInt.
pub const MAX_FRAME_LEN: usize = 2_097_151;
/// Largest packet a compressed frame may unpack to.
pub const MAX_UNCOMPRESSED_LEN: usize = 8_388_608;

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
    Ok(Some((split_packet(frame)?, header + len)))
}

/// Encodes a packet id and body into one length-prefixed frame.
pub fn encode_frame(id: i32, body: &[u8], out: &mut Vec<u8>) {
    let id = VarInt(id);
    VarInt((id.encoded_len() + body.len()) as i32).encode(out);
    id.encode(out);
    out.extend_from_slice(body);
}

fn split_packet(packet: &[u8]) -> Result<RawPacket> {
    let mut payload = packet;
    let VarInt(id) = VarInt::decode(&mut payload)?;
    Ok(RawPacket {
        id,
        body: payload.to_vec(),
    })
}

/// Framing of one connection: plain until [`FrameCodec::enable_compression`],
/// then in the compressed format. Keeps its zlib streams between packets.
#[derive(Default)]
pub struct FrameCodec {
    compression: Option<Compression>,
}

struct Compression {
    threshold: usize,
    deflate: Compress,
    inflate: Decompress,
}

impl FrameCodec {
    /// Packets of at least `threshold` bytes (id included) get compressed.
    /// Vanilla sends `login_compression` with this value first.
    pub fn enable_compression(&mut self, threshold: usize) {
        self.compression = Some(Compression {
            threshold,
            // Java's `Deflater()` default: level 6.
            deflate: Compress::new(flate2::Compression::new(6), true),
            inflate: Decompress::new(true),
        });
    }

    pub fn compression_threshold(&self) -> Option<usize> {
        self.compression.as_ref().map(|c| c.threshold)
    }

    /// Like [`decode_frame`], in this connection's format. Compressed packets
    /// are checked the way vanilla's server checks them.
    pub fn decode(&mut self, buf: &[u8]) -> Result<Option<(RawPacket, usize)>> {
        let Some(compression) = &mut self.compression else {
            return decode_frame(buf);
        };
        let Some((VarInt(len), header)) = VarInt::decode_partial(buf)? else {
            return Ok(None);
        };
        let len = usize::try_from(len).map_err(|_| Error::Invalid("negative frame length"))?;
        if len > MAX_FRAME_LEN {
            return Err(Error::FrameTooLarge(len));
        }
        let Some(mut frame) = buf.get(header..header + len) else {
            return Ok(None);
        };
        let VarInt(data_len) = VarInt::decode(&mut frame)?;
        let packet = match usize::try_from(data_len) {
            Ok(0) => split_packet(frame)?,
            Ok(n) if n < compression.threshold => {
                return Err(Error::Invalid("compressed packet below the threshold"));
            }
            Ok(n) if n > MAX_UNCOMPRESSED_LEN => return Err(Error::FrameTooLarge(n)),
            Ok(n) => split_packet(&compression.inflate(frame, n)?)?,
            Err(_) => return Err(Error::Invalid("negative uncompressed length")),
        };
        Ok(Some((packet, header + len)))
    }

    /// Like [`encode_frame`], in this connection's format.
    pub fn encode(&mut self, id: i32, body: &[u8], out: &mut Vec<u8>) {
        let Some(compression) = &mut self.compression else {
            return encode_frame(id, body, out);
        };
        let id = VarInt(id);
        let packet_len = id.encoded_len() + body.len();
        if packet_len < compression.threshold {
            VarInt(packet_len as i32 + 1).encode(out);
            VarInt(0).encode(out);
            id.encode(out);
            out.extend_from_slice(body);
            return;
        }
        let mut packet = Vec::with_capacity(packet_len);
        id.encode(&mut packet);
        packet.extend_from_slice(body);
        let mut compressed = Vec::new();
        compression.deflate(&packet, &mut compressed);
        let data_len = VarInt(packet_len as i32);
        VarInt((data_len.encoded_len() + compressed.len()) as i32).encode(out);
        data_len.encode(out);
        out.extend_from_slice(&compressed);
    }
}

impl Compression {
    fn deflate(&mut self, input: &[u8], out: &mut Vec<u8>) {
        self.deflate.reset();
        loop {
            let consumed = self.deflate.total_in() as usize;
            out.reserve(input.len() / 2 + 64);
            let status = self
                .deflate
                .compress_vec(&input[consumed..], out, FlushCompress::Finish)
                .expect("deflating from memory cannot fail");
            if status == Status::StreamEnd {
                return;
            }
        }
    }

    /// Unpacks exactly `len` bytes; anything else is a broken packet.
    fn inflate(&mut self, input: &[u8], len: usize) -> Result<Vec<u8>> {
        self.inflate.reset(true);
        let mut out = Vec::with_capacity(len);
        let status = self
            .inflate
            .decompress_vec(input, &mut out, FlushDecompress::Finish)
            .map_err(|_| Error::Invalid("malformed zlib data"))?;
        if status != Status::StreamEnd || out.len() != len {
            return Err(Error::Invalid(
                "compressed packet does not match its declared size",
            ));
        }
        Ok(out)
    }
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

    fn compressed(threshold: usize) -> FrameCodec {
        let mut codec = FrameCodec::default();
        codec.enable_compression(threshold);
        codec
    }

    #[test]
    fn small_packets_stay_uncompressed() {
        let mut out = Vec::new();
        compressed(256).encode(0x01, &[1, 2, 3], &mut out);
        // Frame length, uncompressed size 0, id, body.
        assert_eq!(out, [5, 0, 0x01, 1, 2, 3]);
        let (packet, used) = compressed(256).decode(&out).unwrap().unwrap();
        assert_eq!(
            (packet.id, packet.body, used),
            (1, vec![1, 2, 3], out.len())
        );
    }

    #[test]
    fn large_packets_are_compressed_and_roundtrip() {
        let body = vec![7u8; 10_000];
        let mut out = Vec::new();
        let mut codec = compressed(256);
        codec.encode(0x2e, &body, &mut out);
        codec.encode(0x2e, &body, &mut out);
        assert!(
            out.len() < 200,
            "two frames of repeated bytes: {}",
            out.len()
        );
        // Uncompressed size: one byte of id plus the body.
        assert_eq!(&out[1..3], [0x91, 0x4e]);

        let mut reader = compressed(256);
        let mut buf = out.as_slice();
        for _ in 0..2 {
            let (packet, used) = reader.decode(buf).unwrap().unwrap();
            for cut in 0..used {
                assert_eq!(reader.decode(&buf[..cut]).unwrap(), None, "cut at {cut}");
            }
            assert_eq!(packet.id, 0x2e);
            assert_eq!(packet.body, body);
            buf = &buf[used..];
        }
        assert!(buf.is_empty());
    }

    #[test]
    fn badly_compressed_packets_are_rejected() {
        let mut out = Vec::new();
        compressed(64).encode(0x01, &[0; 100], &mut out);
        // The same frame read with a higher threshold claims too small a size.
        assert_eq!(
            compressed(256).decode(&out),
            Err(Error::Invalid("compressed packet below the threshold"))
        );
        // A declared size that does not match what the data unpacks to.
        let mut lying = out.clone();
        lying[1] += 1;
        assert_eq!(
            compressed(64).decode(&lying),
            Err(Error::Invalid(
                "compressed packet does not match its declared size"
            ))
        );
        // More than the protocol allows to unpack.
        let mut huge = Vec::new();
        VarInt(MAX_UNCOMPRESSED_LEN as i32 + 1).encode(&mut huge);
        let mut frame = Vec::new();
        VarInt(huge.len() as i32).encode(&mut frame);
        frame.extend_from_slice(&huge);
        assert_eq!(
            compressed(64).decode(&frame),
            Err(Error::FrameTooLarge(MAX_UNCOMPRESSED_LEN + 1))
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
