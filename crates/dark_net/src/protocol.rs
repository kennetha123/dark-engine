//! Control-channel messages and the wire codec.

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{PlayerId, RejectReason};

/// Bump on any incompatible wire change.
pub const PROTOCOL_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMessage {
    Hello {
        protocol_version: u32,
        player: PlayerId,
    },
    /// Graceful quit. Sent before disconnecting so the host can tell a quit from a crash.
    Goodbye,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMessage {
    Welcome,
    Rejected(RejectReason),
}

pub fn encode<T: Serialize>(message: &T) -> Vec<u8> {
    postcard::to_allocvec(message)
        .expect("in-memory serialization of a protocol message cannot fail")
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    postcard::from_bytes(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let hello = ClientMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            player: PlayerId::random(),
        };
        assert_eq!(decode::<ClientMessage>(&encode(&hello)), Some(hello));
        assert_eq!(decode::<ClientMessage>(&[0xFF, 0xFF]), None);
    }
}
