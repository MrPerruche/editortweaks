//! How much a peer may send us (`design.md`, "Untrusted peers"): token buckets per peer, refilled
//! with time. The rates are far above what a person can do (holding Ctrl+Z, moving thousands of
//! bricks twice a second), so only a broken or hostile client meets them. What a member does past
//! one gets it kicked from the session (and it can't join again): too many bytes, messages, bricks
//! made or other changes. Selections, hellos and object orders past theirs are dropped: nothing
//! waits for them. Snapshots past theirs are sent later: the member waits for its snapshot, but
//! making one takes the subhost's editor a while on a big vehicle.

use std::collections::{HashMap, HashSet};

use crate::message::PeerId;
use crate::ops::Op;

/// A bucket's refill and size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rate {
    pub per_second: f64,
    pub burst: f64,
}

pub mod rates {
    use super::Rate;

    /// Bytes from any peer. A member's biggest message, a pasted vehicle, is far below the burst.
    pub const BYTES: Rate = Rate {
        per_second: (4 << 20) as f64,
        burst: (32 << 20) as f64,
    };
    /// Messages from a member to the subhost (edits and requests).
    pub const MESSAGES: Rate = Rate {
        per_second: 50.0,
        burst: 500.0,
    };
    /// Bricks a member makes (each makes the subhost's editor load them).
    pub const MADE: Rate = Rate {
        per_second: 5_000.0,
        burst: 70_000.0,
    };
    /// Other changes (moves, values, groups...) a member makes, per brick.
    pub const CHANGES: Rate = Rate {
        per_second: 100_000.0,
        burst: 300_000.0,
    };
    /// Selections a member shows the others (dropped past it).
    pub const SELECTIONS: Rate = Rate {
        per_second: 10.0,
        burst: 30.0,
    };
    /// Hellos from any peer, each answered (dropped past it).
    pub const HELLOS: Rate = Rate {
        per_second: 1.0,
        burst: 5.0,
    };
    /// Object orders a member asks for (dropped past it: it asks again after the next audit).
    pub const ORDERS: Rate = Rate {
        per_second: 0.5,
        burst: 3.0,
    };
    /// Snapshots a member asks for, joining included (sent later past it).
    pub const SNAPSHOTS: Rate = Rate {
        per_second: 0.2,
        burst: 3.0,
    };
    /// Wrong join codes a peer may send before it can't join any more.
    pub const WRONG_CODES: u8 = 5;
}

/// What a member sent too much of, which got it kicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flood {
    Bytes,
    Messages,
    Made,
    Changes,
}

/// A bucket: full, it holds `burst`; it refills by `per_second`. Times are milliseconds.
#[derive(Debug, Clone)]
struct Bucket {
    level: f64,
    at: u64,
}

impl Bucket {
    fn full(rate: Rate, now: u64) -> Self {
        Self {
            level: rate.burst,
            at: now,
        }
    }

    fn refill(&mut self, rate: Rate, now: u64) {
        let seconds = now.saturating_sub(self.at) as f64 / 1000.0;
        self.level = (self.level + seconds * rate.per_second).min(rate.burst);
        self.at = self.at.max(now);
    }

    /// Takes `amount` if the bucket holds that much.
    fn take(&mut self, rate: Rate, now: u64, amount: f64) -> bool {
        self.refill(rate, now);
        if self.level >= amount {
            self.level -= amount;
            true
        } else {
            false
        }
    }

    /// Takes `amount` even if the bucket doesn't hold it yet: when it will have (now, if it did).
    fn take_due(&mut self, rate: Rate, now: u64, amount: f64) -> u64 {
        self.refill(rate, now);
        self.level -= amount;
        if self.level >= 0.0 {
            now
        } else {
            now + (-self.level / rate.per_second * 1000.0).ceil() as u64
        }
    }
}

/// One peer's buckets.
#[derive(Debug, Clone)]
pub(crate) struct Allowance {
    bytes: Bucket,
    messages: Bucket,
    made: Bucket,
    changes: Bucket,
    selections: Bucket,
    hellos: Bucket,
    orders: Bucket,
    snapshots: Bucket,
    wrong_codes: u8,
}

impl Allowance {
    fn new(now: u64) -> Self {
        Self {
            bytes: Bucket::full(rates::BYTES, now),
            messages: Bucket::full(rates::MESSAGES, now),
            made: Bucket::full(rates::MADE, now),
            changes: Bucket::full(rates::CHANGES, now),
            selections: Bucket::full(rates::SELECTIONS, now),
            hellos: Bucket::full(rates::HELLOS, now),
            orders: Bucket::full(rates::ORDERS, now),
            snapshots: Bucket::full(rates::SNAPSHOTS, now),
            wrong_codes: 0,
        }
    }

    pub fn bytes(&mut self, now: u64, n: usize) -> bool {
        self.bytes.take(rates::BYTES, now, n as f64)
    }

    pub fn hello(&mut self, now: u64) -> bool {
        self.hellos.take(rates::HELLOS, now, 1.0)
    }

    pub fn selection(&mut self, now: u64) -> bool {
        self.selections.take(rates::SELECTIONS, now, 1.0)
    }

    pub fn order(&mut self, now: u64) -> bool {
        self.orders.take(rates::ORDERS, now, 1.0)
    }

    /// When the next snapshot may be sent.
    pub fn snapshot_due(&mut self, now: u64) -> u64 {
        self.snapshots.take_due(rates::SNAPSHOTS, now, 1.0)
    }

    /// A message from a member, with `ops` if it's an edit: what it sent too much of, if anything.
    pub fn message(&mut self, now: u64, ops: &[Op]) -> Result<(), Flood> {
        if !self.messages.take(rates::MESSAGES, now, 1.0) {
            return Err(Flood::Messages);
        }
        let (mut made, mut changes) = (0, 0);
        for op in ops {
            match op {
                Op::Create { ids, .. } => made += ids.len(),
                _ => changes += 1,
            }
        }
        if made > 0 && !self.made.take(rates::MADE, now, made as f64) {
            return Err(Flood::Made);
        }
        if changes > 0 && !self.changes.take(rates::CHANGES, now, changes as f64) {
            return Err(Flood::Changes);
        }
        Ok(())
    }

    /// Counts a wrong join code: whether the peer has now sent too many.
    pub fn wrong_code(&mut self) -> bool {
        self.wrong_codes = self.wrong_codes.saturating_add(1);
        self.wrong_codes >= rates::WRONG_CODES
    }
}

/// Every peer's allowance, and who may not join again.
#[derive(Debug, Default)]
pub(crate) struct Limits {
    peers: HashMap<PeerId, Allowance>,
    pub banned: HashSet<PeerId>,
}

impl Limits {
    /// Peers remembered at most; past it, only those in the session are kept.
    const PEERS: usize = 1024;

    pub fn of(&mut self, peer: PeerId, now: u64) -> &mut Allowance {
        self.peers
            .entry(peer)
            .or_insert_with(|| Allowance::new(now))
    }

    /// Whether it remembers so many peers it should forget some ([`Limits::prune`]).
    pub fn crowded(&self) -> bool {
        self.peers.len() > Self::PEERS
    }

    /// Forgets the peers not in `keep`.
    pub fn prune(&mut self, keep: &[PeerId]) {
        self.peers.retain(|peer, _| keep.contains(peer));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::CoopId;
    use crate::ops::Transform;

    const RATE: Rate = Rate {
        per_second: 2.0,
        burst: 4.0,
    };

    #[test]
    fn buckets_refill_with_time_up_to_their_burst() {
        let mut bucket = Bucket::full(RATE, 1000);
        assert!(bucket.take(RATE, 1000, 3.0));
        assert!(!bucket.take(RATE, 1000, 2.0));
        assert!(bucket.take(RATE, 1500, 2.0)); // 1 left + 1 refilled
        assert!(!bucket.take(RATE, 1500, 0.5));
        // An hour later: full, not more.
        assert!(bucket.take(RATE, 3_601_500, 4.0));
        assert!(!bucket.take(RATE, 3_601_500, 0.5));
        // A clock going back takes nothing back.
        assert!(!bucket.take(RATE, 0, 0.5));
    }

    #[test]
    fn what_is_due_later_waits_its_turn() {
        let mut bucket = Bucket::full(RATE, 0);
        assert_eq!(bucket.take_due(RATE, 0, 4.0), 0);
        assert_eq!(bucket.take_due(RATE, 0, 1.0), 500);
        assert_eq!(bucket.take_due(RATE, 0, 1.0), 1000);
        assert_eq!(bucket.take_due(RATE, 2000, 1.0), 2000);
    }

    #[test]
    fn edits_count_bricks_made_apart_from_other_changes() {
        let id = |n| CoopId::new(2, n).unwrap();
        let create = |n: u32| Op::Create {
            ids: (1..=n).map(id).collect(),
            archive: Vec::new(),
        };
        let mut allowance = Allowance::new(0);
        assert_eq!(allowance.message(0, &[create(60_000)]), Ok(()));
        assert_eq!(allowance.message(0, &[create(20_000)]), Err(Flood::Made));
        let moves: Vec<Op> = (1..=1000)
            .map(|n| Op::Move {
                id: id(n),
                transform: Transform {
                    location: [0.0; 3],
                    rotation: [0.0; 3],
                },
            })
            .collect();
        for _ in 0..300 {
            assert_eq!(allowance.message(0, &moves), Ok(()));
        }
        assert_eq!(allowance.message(0, &moves), Err(Flood::Changes));
        // A second later, room again.
        assert_eq!(allowance.message(1000, &moves), Ok(()));
    }

    #[test]
    fn messages_and_wrong_codes_run_out() {
        let mut allowance = Allowance::new(0);
        for _ in 0..500 {
            assert_eq!(allowance.message(0, &[]), Ok(()));
        }
        assert_eq!(allowance.message(0, &[]), Err(Flood::Messages));
        assert_eq!(allowance.message(100, &[]), Ok(()));
        for _ in 1..rates::WRONG_CODES {
            assert!(!allowance.wrong_code());
        }
        assert!(allowance.wrong_code());
    }
}
