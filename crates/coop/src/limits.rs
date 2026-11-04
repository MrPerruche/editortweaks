//! How much a peer may send us (`design.md`, "Untrusted peers"): token buckets per peer, refilled
//! with time. The rates are far above what a person can do (holding Ctrl+Z, moving thousands of
//! bricks twice a second), so only a broken or hostile client meets them. What a member does past
//! one gets it kicked from the session (and it can't join again): too many bytes, messages, bricks
//! made or other changes. Selections, cameras, hellos and object orders past theirs are dropped: nothing
//! waits for them. Snapshots past theirs are sent later: the member waits for its snapshot, but
//! making one takes the subhost's editor a while on a big vehicle.
//!
//! The other way, a member doesn't trust its subhost either: the subhost relays every player's
//! edits, so its allowance is a member's times [`rates::HOST_SCALE`] (the most players a session
//! holds), a ceiling only a broken or hostile subhost meets, and its snapshots are held to twice
//! what a subhost sends one member. Past either, the member leaves. What really protects a
//! member's game from a subhost sending more than it can apply is [`Workload`], measured by the
//! mod.

use std::collections::{HashMap, HashSet};

use crate::message::PeerId;
use crate::ops::Op;

/// A bucket's refill and size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rate {
    pub per_second: f64,
    pub burst: f64,
}

impl Rate {
    fn scaled(self, by: f64) -> Self {
        Self {
            per_second: self.per_second * by,
            burst: self.burst * by,
        }
    }
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
    /// Camera views a member shows the others (dropped past it).
    pub const CAMERAS: Rate = Rate {
        per_second: 20.0,
        burst: 40.0,
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

    /// A member's allowance for its subhost: a member's rates (bytes, messages, bricks made, other
    /// changes, selections, cameras) times this, the most players a session holds (the mod's
    /// `CoopMaxPlayers` setting goes up to 32), since the subhost relays every player's edits.
    pub const HOST_SCALE: f64 = 32.0;
    /// Snapshots a member takes from its subhost: twice [`SNAPSHOTS`], which a subhost holds
    /// itself to for each member (joining, asking again, another vehicle shared).
    pub const HOST_SNAPSHOTS: Rate = Rate {
        per_second: 0.4,
        burst: 6.0,
    };
}

/// What a member sent too much of, which got it kicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flood {
    Bytes,
    Messages,
    Made,
    Changes,
    /// Snapshots (from the subhost only: a member asks for its own).
    Snapshots,
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
    /// What the shared rates are multiplied by: 1 for a member, [`rates::HOST_SCALE`] for a
    /// member's subhost.
    scale: f64,
    bytes: Bucket,
    messages: Bucket,
    made: Bucket,
    changes: Bucket,
    selections: Bucket,
    cameras: Bucket,
    hellos: Bucket,
    orders: Bucket,
    snapshots: Bucket,
    host_snapshots: Bucket,
    wrong_codes: u8,
}

impl Allowance {
    fn new(now: u64) -> Self {
        Self::scaled(now, 1.0)
    }

    fn scaled(now: u64, scale: f64) -> Self {
        let full = |rate: Rate| Bucket::full(rate.scaled(scale), now);
        Self {
            scale,
            bytes: full(rates::BYTES),
            messages: full(rates::MESSAGES),
            made: full(rates::MADE),
            changes: full(rates::CHANGES),
            selections: full(rates::SELECTIONS),
            cameras: full(rates::CAMERAS),
            hellos: Bucket::full(rates::HELLOS, now),
            orders: Bucket::full(rates::ORDERS, now),
            snapshots: Bucket::full(rates::SNAPSHOTS, now),
            host_snapshots: Bucket::full(rates::HOST_SNAPSHOTS, now),
            wrong_codes: 0,
        }
    }

    fn rate(&self, rate: Rate) -> Rate {
        rate.scaled(self.scale)
    }

    pub fn bytes(&mut self, now: u64, n: usize) -> bool {
        let rate = self.rate(rates::BYTES);
        self.bytes.take(rate, now, n as f64)
    }

    pub fn hello(&mut self, now: u64) -> bool {
        self.hellos.take(rates::HELLOS, now, 1.0)
    }

    pub fn selection(&mut self, now: u64) -> bool {
        let rate = self.rate(rates::SELECTIONS);
        self.selections.take(rate, now, 1.0)
    }

    pub fn camera(&mut self, now: u64) -> bool {
        let rate = self.rate(rates::CAMERAS);
        self.cameras.take(rate, now, 1.0)
    }

    /// A snapshot from the subhost: whether it's within [`rates::HOST_SNAPSHOTS`].
    pub fn host_snapshot(&mut self, now: u64) -> bool {
        self.host_snapshots.take(rates::HOST_SNAPSHOTS, now, 1.0)
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
        let (messages, made_rate, changes_rate) = (
            self.rate(rates::MESSAGES),
            self.rate(rates::MADE),
            self.rate(rates::CHANGES),
        );
        if !self.messages.take(messages, now, 1.0) {
            return Err(Flood::Messages);
        }
        let (mut made, mut changes) = (0, 0);
        for op in ops {
            match op {
                Op::Create { ids, .. } => made += ids.len(),
                _ => changes += 1,
            }
        }
        if made > 0 && !self.made.take(made_rate, now, made as f64) {
            return Err(Flood::Made);
        }
        if changes > 0 && !self.changes.take(changes_rate, now, changes as f64) {
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
    /// Member: our subhost's allowance (made when it first sends something).
    host: Option<Allowance>,
    pub banned: HashSet<PeerId>,
    /// Who the subhost's player banned.
    pub banned_until: HashMap<PeerId, Ban>,
}

/// A ban by the subhost's player.
#[derive(Debug)]
pub(crate) struct Ban {
    /// [`crate::Session::tick`]'s clock.
    pub until: u64,
    /// What the player gave as the reason, told to the banned player.
    pub reason: String,
}

impl Limits {
    /// Peers remembered at most; past it, only those in the session are kept.
    const PEERS: usize = 1024;

    pub fn of(&mut self, peer: PeerId, now: u64) -> &mut Allowance {
        self.peers
            .entry(peer)
            .or_insert_with(|| Allowance::new(now))
    }

    /// Member: our subhost's allowance.
    pub fn host(&mut self, now: u64) -> &mut Allowance {
        self.host
            .get_or_insert_with(|| Allowance::scaled(now, rates::HOST_SCALE))
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

/// How much of a member's time goes to applying what its subhost sends (snapshots loaded, edits
/// made): a leaky bucket of busy milliseconds, drained by [`Workload::SHARE`] of the time that
/// passes. Past [`Workload::SLACK`], the game spent most of a long while on the subhost's traffic
/// (a subhost sending more than this game can apply, hostile or not), and the member leaves
/// rather than stay frozen. A big vehicle loading (joining, a resync, another one shared) or a
/// big paste takes seconds at once and passes; the game stuck for most of a minute doesn't.
#[derive(Debug, Clone, Default)]
pub struct Workload {
    level: f64,
    at: Option<u64>,
}

impl Workload {
    /// The share of the time the game may spend on the subhost's traffic for as long as it likes.
    pub const SHARE: f64 = 0.5;
    /// Busy milliseconds past that share it may build up: one 20 s load at once, or about 40 s of
    /// the game doing nothing else.
    pub const SLACK: f64 = 20_000.0;

    /// The game spent `busy` milliseconds on the subhost's traffic, finishing at `now` (any
    /// clock in milliseconds): whether that's now too much.
    pub fn spent(&mut self, now: u64, busy: u64) -> bool {
        let at = *self.at.get_or_insert(now);
        let passed = now.saturating_sub(at) as f64;
        self.at = Some(at.max(now));
        self.level = (self.level - passed * Self::SHARE).max(0.0) + busy as f64;
        self.level > Self::SLACK
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
    fn a_subhost_may_send_what_every_player_may_and_no_more() {
        let mut host = Allowance::scaled(0, rates::HOST_SCALE);
        // As many messages as every player at their fastest...
        for _ in 0..(rates::MESSAGES.burst * rates::HOST_SCALE) as usize {
            assert_eq!(host.message(0, &[]), Ok(()));
        }
        assert_eq!(host.message(0, &[]), Err(Flood::Messages));
        // ...and bytes, but snapshots only twice what it sends one member.
        assert!(host.bytes(0, 32 * (32 << 20)));
        assert!(!host.bytes(0, 1));
        for _ in 0..rates::HOST_SNAPSHOTS.burst as usize {
            assert!(host.host_snapshot(0));
        }
        assert!(!host.host_snapshot(0));
        assert!(host.host_snapshot(2500));
        assert!(!host.host_snapshot(2500));
    }

    #[test]
    fn what_a_subhost_lets_out_to_a_member_always_gets_in() {
        let mut sent = Bucket::full(rates::SNAPSHOTS, 0);
        let mut taken = Allowance::scaled(0, rates::HOST_SCALE);
        let mut now = 0;
        for _ in 0..1000 {
            now = sent.take_due(rates::SNAPSHOTS, now, 1.0);
            assert!(taken.host_snapshot(now), "at {now}");
        }
    }

    #[test]
    fn big_loads_pass_but_a_game_kept_busy_leaves() {
        // One 15 s load, then normal play: fine.
        let mut work = Workload::default();
        assert!(!work.spent(15_000, 15_000));
        for frame in 1..=10_000u64 {
            assert!(!work.spent(15_000 + frame * 16, 1));
        }
        // Two 10 s loads 10 s apart (joining, then a resync): fine.
        let mut work = Workload::default();
        assert!(!work.spent(10_000, 10_000));
        assert!(!work.spent(30_000, 10_000));
        // Busy 90 % of every frame: leaves within a minute, not in the first 20 s.
        let mut work = Workload::default();
        let left = (1..=10_000u64)
            .map(|frame| frame * 100)
            .find(|&now| work.spent(now, 90))
            .expect("left");
        assert!((20_000..60_000).contains(&left), "{left}");
        // Half the time, forever: stays.
        let mut work = Workload::default();
        for frame in 1..=100_000u64 {
            assert!(!work.spent(frame * 100, 50));
        }
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
