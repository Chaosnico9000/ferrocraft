use super::Packet;
use crate::codec::{Decode, Encode, Result};

/// `minecraft:status_request`, serverbound. Empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusRequest;

impl Packet for StatusRequest {
    const ID: i32 = 0x00;
}

impl Encode for StatusRequest {
    fn encode(&self, _: &mut Vec<u8>) {}
}

impl Decode for StatusRequest {
    fn decode(_: &mut &[u8]) -> Result<Self> {
        Ok(Self)
    }
}

/// `minecraft:status_response`, clientbound: the server list entry as JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusResponse {
    pub json: String,
}

impl Packet for StatusResponse {
    const ID: i32 = 0x00;
}

impl Encode for StatusResponse {
    fn encode(&self, out: &mut Vec<u8>) {
        self.json.encode(out);
    }
}

impl Decode for StatusResponse {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            json: String::decode(buf)?,
        })
    }
}

/// `minecraft:ping_request`, serverbound. The payload is echoed back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PingRequest {
    pub payload: i64,
}

impl Packet for PingRequest {
    const ID: i32 = 0x01;
}

impl Encode for PingRequest {
    fn encode(&self, out: &mut Vec<u8>) {
        self.payload.encode(out);
    }
}

impl Decode for PingRequest {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            payload: i64::decode(buf)?,
        })
    }
}

/// `minecraft:pong_response`, clientbound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PongResponse {
    pub payload: i64,
}

impl Packet for PongResponse {
    const ID: i32 = 0x01;
}

impl Encode for PongResponse {
    fn encode(&self, out: &mut Vec<u8>) {
        self.payload.encode(out);
    }
}

impl Decode for PongResponse {
    fn decode(buf: &mut &[u8]) -> Result<Self> {
        Ok(Self {
            payload: i64::decode(buf)?,
        })
    }
}
