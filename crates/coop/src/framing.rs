//! Splitting messages too big for one transport message (Steam's cap is 512 KiB) into parts, and
//! putting them back together. Parts of one message are sent reliable and back to back, and a
//! peer's reliable messages arrive in order, so a receiver only ever assembles one message per
//! peer.

use std::collections::HashMap;

use crate::message::{HEADER_LEN, Message, PeerId};
use crate::wire::{DecodeError, Result};

/// The most bytes one transport message carries, under Steam's 524,288 with room to spare.
pub const MAX_FRAME: usize = 500_000;
/// The biggest message after assembling: a full snapshot plus its ids.
pub const MAX_ASSEMBLED: usize = 24 << 20;
const PART_OVERHEAD: usize = HEADER_LEN + 8;

/// The frames to send for one message: just the message when it fits, else its parts.
pub fn frames(session: u32, message: &Message) -> Vec<Vec<u8>> {
    let bytes = message.encode(session);
    if bytes.len() <= MAX_FRAME {
        return vec![bytes];
    }
    let chunks: Vec<&[u8]> = bytes.chunks(MAX_FRAME - PART_OVERHEAD).collect();
    let count = chunks.len() as u32;
    chunks
        .into_iter()
        .enumerate()
        .map(|(index, data)| {
            Message::Part {
                index: index as u32,
                count,
                data: data.to_vec(),
            }
            .encode(session)
        })
        .collect()
}

struct Partial {
    count: u32,
    next: u32,
    bytes: Vec<u8>,
}

/// Puts parts back together, per peer.
#[derive(Default)]
pub struct Assembler {
    partial: HashMap<PeerId, Partial>,
}

impl Assembler {
    /// Decodes a frame from `peer`. `Ok(None)` while a message is still missing parts.
    pub fn receive(&mut self, peer: PeerId, frame: &[u8]) -> Result<Option<(u32, Message)>> {
        match Message::decode(frame)? {
            (_, Message::Part { index, count, data }) => {
                let max_parts = (MAX_ASSEMBLED / (MAX_FRAME - PART_OVERHEAD) + 1) as u32;
                if count > max_parts || count < 2 || index >= count {
                    self.partial.remove(&peer);
                    return Err(DecodeError::TooLong {
                        what: "parts",
                        len: count as usize,
                        max: max_parts as usize,
                    });
                }
                if index == 0 {
                    self.partial.insert(
                        peer,
                        Partial {
                            count,
                            next: 0,
                            bytes: Vec::new(),
                        },
                    );
                }
                let Some(partial) = self.partial.get_mut(&peer) else {
                    return Err(DecodeError::Truncated);
                };
                if partial.count != count || partial.next != index {
                    self.partial.remove(&peer);
                    return Err(DecodeError::Truncated);
                }
                partial.bytes.extend_from_slice(&data);
                partial.next += 1;
                if partial.next < count {
                    return Ok(None);
                }
                let whole = self.partial.remove(&peer).expect("just used").bytes;
                match Message::decode(&whole)? {
                    (_, Message::Part { .. }) => Err(DecodeError::BadTag {
                        what: "nested part",
                        tag: 15,
                    }),
                    decoded => Ok(Some(decoded)),
                }
            }
            decoded => Ok(Some(decoded)),
        }
    }

    /// Forgets a peer's half-received message (it left).
    pub fn forget(&mut self, peer: PeerId) {
        self.partial.remove(&peer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::CoopId;

    #[test]
    fn small_messages_go_whole() {
        let frames = frames(5, &Message::Resync);
        assert_eq!(frames.len(), 1);
        assert_eq!(
            Assembler::default().receive(1, &frames[0]),
            Ok(Some((5, Message::Resync)))
        );
    }

    #[test]
    fn big_messages_go_in_parts() {
        // Random-ish bytes so compression doesn't shrink the snapshot under one frame.
        let mut state = 1u32;
        let vehicle: Vec<u8> = (0..2_000_000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        let message = Message::Snapshot {
            commit: 3,
            ids: vec![CoopId::new(0, 1).unwrap()],
            vehicle,
        };
        let frames = frames(7, &message);
        assert!(frames.len() >= 4);
        assert!(frames.iter().all(|f| f.len() <= MAX_FRAME));

        let mut assembler = Assembler::default();
        for frame in &frames[..frames.len() - 1] {
            assert_eq!(assembler.receive(9, frame), Ok(None));
            // Another peer's messages don't get mixed in.
            assert_eq!(
                assembler.receive(10, &Message::Ping.encode(7)),
                Ok(Some((7, Message::Ping)))
            );
        }
        assert_eq!(
            assembler.receive(9, frames.last().unwrap()),
            Ok(Some((7, message)))
        );

        // A missing part drops the message instead of gluing the wrong bytes together.
        assert!(assembler.receive(9, &frames[0]).unwrap().is_none());
        assert!(assembler.receive(9, &frames[2]).is_err());
        assert!(assembler.receive(9, &frames[3]).is_err());
    }
}
