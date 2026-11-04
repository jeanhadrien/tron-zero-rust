//! Server discovery probe: one raw UDP datagram out, one back.
//!
//! The client asks "is a Tron Zero server listening here?" without opening a
//! session. The server answers from its link layer and drops the throwaway
//! link at once (see `server::probe`). The first byte is `0xFF` so it can never
//! be mistaken for a Lightyear packet header; the request is padded to at least
//! the reply's size so a spoofed source cannot use the server to amplify
//! traffic. Old servers ignore it, so no protocol version bump is needed.

use crate::protocol::PROTOCOL_ID;

const MAGIC: [u8; 3] = [0xFF, b'T', b'Z'];
const REQUEST_TAG: u8 = b'?';
const REPLY_TAG: u8 = b'!';
const NONCE_LEN: usize = 4;

/// Size of every probe request (zero padded).
pub const PROBE_REQUEST_LEN: usize = 32;
/// Size of every probe reply; never larger than a request.
pub const PROBE_REPLY_LEN: usize = 4 + 4 + 2 + NONCE_LEN;
const _: () = assert!(PROBE_REPLY_LEN <= PROBE_REQUEST_LEN);

/// What a server says about itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbeReply {
    /// The server's `PROTOCOL_ID`; differs from ours when the builds are incompatible.
    pub protocol: u32,
    pub players: u8,
    pub max_players: u8,
}

impl ProbeReply {
    /// Whether a client of this build could join.
    pub fn compatible(&self) -> bool {
        self.protocol == PROTOCOL_ID
    }
}

/// A probe request carrying `nonce`, which the reply echoes.
pub fn encode_request(nonce: [u8; NONCE_LEN]) -> [u8; PROBE_REQUEST_LEN] {
    let mut out = [0u8; PROBE_REQUEST_LEN];
    out[..3].copy_from_slice(&MAGIC);
    out[3] = REQUEST_TAG;
    out[4..4 + NONCE_LEN].copy_from_slice(&nonce);
    out
}

/// The nonce of a well-formed request, `None` for anything else (including
/// requests too short to be padded, which are not answered).
pub fn decode_request(bytes: &[u8]) -> Option<[u8; NONCE_LEN]> {
    if bytes.len() < PROBE_REQUEST_LEN || bytes[..3] != MAGIC || bytes[3] != REQUEST_TAG {
        return None;
    }
    bytes[4..4 + NONCE_LEN].try_into().ok()
}

/// The reply to the request with `nonce`.
pub fn encode_reply(nonce: [u8; NONCE_LEN], reply: ProbeReply) -> [u8; PROBE_REPLY_LEN] {
    let mut out = [0u8; PROBE_REPLY_LEN];
    out[..3].copy_from_slice(&MAGIC);
    out[3] = REPLY_TAG;
    out[4..8].copy_from_slice(&reply.protocol.to_le_bytes());
    out[8] = reply.players;
    out[9] = reply.max_players;
    out[10..].copy_from_slice(&nonce);
    out
}

/// The reply, if `bytes` is a well-formed reply to the request with `nonce`.
pub fn decode_reply(bytes: &[u8], nonce: [u8; NONCE_LEN]) -> Option<ProbeReply> {
    if bytes.len() != PROBE_REPLY_LEN
        || bytes[..3] != MAGIC
        || bytes[3] != REPLY_TAG
        || bytes[10..] != nonce
    {
        return None;
    }
    Some(ProbeReply {
        protocol: u32::from_le_bytes(bytes[4..8].try_into().ok()?),
        players: bytes[8],
        max_players: bytes[9],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_and_reply_round_trip() {
        let nonce = [1, 2, 3, 4];
        assert_eq!(decode_request(&encode_request(nonce)), Some(nonce));
        let reply = ProbeReply {
            protocol: PROTOCOL_ID,
            players: 3,
            max_players: 10,
        };
        assert_eq!(
            decode_reply(&encode_reply(nonce, reply), nonce),
            Some(reply)
        );
        assert!(reply.compatible());
    }

    #[test]
    fn foreign_or_mismatched_datagrams_are_rejected() {
        let nonce = [9, 9, 9, 9];
        let reply = encode_reply(
            nonce,
            ProbeReply {
                protocol: 1,
                players: 0,
                max_players: 0,
            },
        );
        assert_eq!(decode_reply(&reply, [0, 0, 0, 0]), None);
        assert_eq!(decode_reply(&reply[..PROBE_REPLY_LEN - 1], nonce), None);
        assert_eq!(decode_request(&[0u8; 4]), None);
        assert_eq!(decode_request(&reply), None);
        assert_eq!(decode_request(&encode_request(nonce)[..31]), None);
    }
}
