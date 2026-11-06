//! Coop ids: names for bricks that every member agrees on. The editor's own brick ids are
//! positions in its object list, so they shift on every delete; coop ids never change.

use std::collections::HashSet;
use std::fmt;

/// A member's place in the session. Its bricks' ids carry it, so members never hand out the same
/// id. Slot 0 is the bricks that were there when the session started, slot 1 the subhost.
pub type Slot = u8;

/// The bricks the vehicle had when the session started.
pub const ORIGINAL_SLOT: Slot = 0;
/// The member that hosts the session.
pub const SUBHOST_SLOT: Slot = 1;

/// A brick's id in the session: the member slot in the top 8 bits, a per-member counter below.
/// The counter starts at 1, so no id is 0.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CoopId(u32);

impl CoopId {
    const COUNTER_BITS: u32 = 24;
    /// The biggest counter a slot can reach (16 M bricks per member).
    pub const MAX_COUNTER: u32 = (1 << Self::COUNTER_BITS) - 1;

    /// `None` if `counter` is 0 or too big.
    pub fn new(slot: Slot, counter: u32) -> Option<Self> {
        (1..=Self::MAX_COUNTER)
            .contains(&counter)
            .then(|| Self((u32::from(slot) << Self::COUNTER_BITS) | counter))
    }

    /// Reads an id as sent on the wire; `None` for a counter of 0.
    pub fn from_raw(raw: u32) -> Option<Self> {
        (raw & Self::MAX_COUNTER != 0).then_some(Self(raw))
    }

    pub fn raw(self) -> u32 {
        self.0
    }

    pub fn slot(self) -> Slot {
        (self.0 >> Self::COUNTER_BITS) as Slot
    }

    pub fn counter(self) -> u32 {
        self.0 & Self::MAX_COUNTER
    }
}

impl fmt::Debug for CoopId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}.{}", self.slot(), self.counter())
    }
}

/// Hands out a slot's ids in order.
#[derive(Debug, Clone)]
pub struct IdAllocator {
    slot: Slot,
    next: u32,
}

impl IdAllocator {
    pub fn new(slot: Slot) -> Self {
        Self { slot, next: 1 }
    }

    /// `None` once the slot ran out of ids.
    pub fn next_id(&mut self) -> Option<CoopId> {
        let id = CoopId::new(self.slot, self.next)?;
        self.next += 1;
        Some(id)
    }
}

/// A short summary of which bricks exist, sent with every commit so members notice when they
/// drifted: the brick count and an order-independent hash of their ids.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Check {
    pub count: u32,
    pub hash: u64,
}

impl Check {
    pub fn of(ids: impl IntoIterator<Item = CoopId>) -> Self {
        let mut check = Self::default();
        ids.into_iter().for_each(|id| check.toggle(id, true));
        check
    }

    /// Adds (`added`) or removes an id. XOR, so the order doesn't matter and removing undoes
    /// adding.
    fn toggle(&mut self, id: CoopId, added: bool) {
        self.hash ^= mix(id.raw());
        self.count = if added {
            self.count + 1
        } else {
            self.count - 1
        };
    }
}

/// splitmix64's finalizer: spreads neighbouring ids over the whole `u64`, so their XOR doesn't
/// cancel out.
fn mix(raw: u32) -> u64 {
    let mut z = u64::from(raw).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The bricks that exist, with their [`Check`] kept up to date.
#[derive(Debug, Clone, Default)]
pub struct LiveSet {
    ids: HashSet<CoopId>,
    check: Check,
}

impl LiveSet {
    pub fn from_ids(ids: impl IntoIterator<Item = CoopId>) -> Self {
        let mut set = Self::default();
        ids.into_iter().for_each(|id| {
            set.insert(id);
        });
        set
    }

    /// `false` if it was there already.
    pub fn insert(&mut self, id: CoopId) -> bool {
        let added = self.ids.insert(id);
        if added {
            self.check.toggle(id, true);
        }
        added
    }

    /// `false` if it wasn't there.
    pub fn remove(&mut self, id: CoopId) -> bool {
        let removed = self.ids.remove(&id);
        if removed {
            self.check.toggle(id, false);
        }
        removed
    }

    pub fn contains(&self, id: CoopId) -> bool {
        self.ids.contains(&id)
    }

    /// Every brick, in no particular order.
    pub fn ids(&self) -> impl Iterator<Item = CoopId> + '_ {
        self.ids.iter().copied()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn check(&self) -> Check {
        self.check
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_parts() {
        let id = CoopId::new(3, 42).unwrap();
        assert_eq!((id.slot(), id.counter()), (3, 42));
        assert_eq!(CoopId::from_raw(id.raw()), Some(id));
        assert_eq!(CoopId::new(3, 0), None);
        assert_eq!(CoopId::new(3, CoopId::MAX_COUNTER + 1), None);
        assert_eq!(CoopId::from_raw(5 << 24), None);
    }

    #[test]
    fn allocator_runs_out() {
        let mut ids = IdAllocator {
            slot: 2,
            next: CoopId::MAX_COUNTER,
        };
        assert!(ids.next_id().is_some());
        assert_eq!(ids.next_id(), None);
    }

    #[test]
    fn check_ignores_order_and_undoes() {
        let a: Vec<CoopId> = (1..50).map(|n| CoopId::new(0, n).unwrap()).collect();
        let mut reversed = a.clone();
        reversed.reverse();
        assert_eq!(Check::of(a.clone()), Check::of(reversed));

        let mut set = LiveSet::from_ids(a.clone());
        let extra = CoopId::new(4, 1).unwrap();
        assert!(set.insert(extra));
        assert!(!set.insert(extra));
        assert_ne!(set.check(), Check::of(a.clone()));
        assert!(set.remove(extra));
        assert_eq!(set.check(), Check::of(a));
    }
}
