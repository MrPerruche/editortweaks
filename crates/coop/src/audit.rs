//! Telling a member's editor apart from the subhost's: the safety net under the session's own
//! bookkeeping (`design.md`, "Syncing and conflicts", point 2). A commit's [`Check`] only compares
//! what the sessions believe; an editor can still differ (an edit the game made differently, a
//! change the mod's hooks missed). So every few seconds the subhost describes its editor in short:
//! how many bricks it has, a hash of their order, and digests of some of them, going round the
//! vehicle. A member at the same commit, with no edit of its own on the way, compares its editor:
//! a brick that differs asks for a snapshot; the same bricks in another order ask for the subhost's
//! order only (bricks two members make at the same moment can land in either order).
//!
//! [`Check`]: crate::Check

use std::collections::HashSet;

use crate::ids::CoopId;
use crate::ops::{Groups, Transform, Value, write_value};
use crate::wire::Writer;

/// How often the subhost describes its editor, and how many bricks each time: a vehicle of 1000
/// bricks is gone round in about half a minute.
pub mod pace {
    pub const SECONDS: u64 = 2;
    pub const BRICKS: usize = 64;
}

/// A brick in short: hashes of its place, its synced property values and its groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Digest {
    pub place: u32,
    pub values: u32,
    pub groups: u32,
}

impl Digest {
    /// A brick at `place`, with the synced property values `values` (each property's path and its
    /// value as it travels, in any order) and the groups `groups`. Places are rounded to 0.1 mm and
    /// 0.01°, angles taken within a turn: a loaded copy of a brick may differ in the last bits.
    pub fn of<'a>(
        place: &Transform,
        values: impl IntoIterator<Item = (&'a str, &'a Value)>,
        groups: Groups,
    ) -> Self {
        let mut w = Writer::new();
        for v in place.location {
            w.u64(hundredths(v) as u64);
        }
        for v in place.rotation {
            w.u64(hundredths(v).rem_euclid(36_000) as u64);
        }
        let place = hash(&w.finish());

        let mut values: Vec<(&str, &Value)> = values.into_iter().collect();
        values.sort_by(|a, b| a.0.cmp(b.0));
        let mut w = Writer::new();
        for (name, value) in values {
            w.str(name);
            write_value(&mut w, value);
        }
        let values = hash(&w.finish());

        let mut w = Writer::new();
        groups.iter().for_each(|&group| w.u16(group));
        Self {
            place,
            values,
            groups: hash(&w.finish()),
        }
    }
}

fn hundredths(v: f32) -> i64 {
    (f64::from(v) * 100.0).round() as i64
}

/// FNV-1a, folded to 32 bits: stable everywhere, unlike std's hashers.
fn hash(bytes: &[u8]) -> u32 {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for &byte in bytes {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x0100_0000_01B3);
    }
    (h ^ (h >> 32)) as u32
}

/// A hash of coop ids in this order (the object order).
pub fn order_hash(ids: impl IntoIterator<Item = CoopId>) -> u32 {
    let mut w = Writer::new();
    ids.into_iter().for_each(|id| w.u32(id.raw()));
    hash(&w.finish())
}

/// The subhost's editor in short, as of a commit.
#[derive(Debug, Clone, PartialEq)]
pub struct Audit {
    pub commit: u32,
    /// How many bricks it has.
    pub count: u32,
    /// [`order_hash`] of their coop ids in object order.
    pub order: u32,
    /// Some of them, with their digests.
    pub bricks: Vec<(CoopId, Digest)>,
}

/// How an editor differs from an [`Audit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Difference {
    /// Another number of bricks (objects without coop ids count too).
    Count {
        subhost: u32,
        here: u32,
    },
    Missing(CoopId),
    Place(CoopId),
    Values(CoopId),
    Groups(CoopId),
    /// The same bricks as far as the audit tells, in another object order.
    Order,
}

impl Difference {
    /// Whether only a snapshot mends it; the subhost's order mends an order.
    pub fn needs_snapshot(self) -> bool {
        !matches!(self, Self::Order)
    }
}

impl Audit {
    /// How an editor with `count` bricks in the order `order` ([`order_hash`]) differs from this
    /// one; `digest(id)` is its brick's digest (`None`: it has no such brick). An order is only
    /// compared when nothing else differs.
    pub fn compare(
        &self,
        count: u32,
        order: u32,
        mut digest: impl FnMut(CoopId) -> Option<Digest>,
    ) -> Vec<Difference> {
        let mut differences = Vec::new();
        if count != self.count {
            differences.push(Difference::Count {
                subhost: self.count,
                here: count,
            });
        }
        for &(id, theirs) in &self.bricks {
            let Some(ours) = digest(id) else {
                differences.push(Difference::Missing(id));
                continue;
            };
            if ours.place != theirs.place {
                differences.push(Difference::Place(id));
            }
            if ours.values != theirs.values {
                differences.push(Difference::Values(id));
            }
            if ours.groups != theirs.groups {
                differences.push(Difference::Groups(id));
            }
        }
        if differences.is_empty() && order != self.order {
            differences.push(Difference::Order);
        }
        differences
    }
}

/// Which bricks the subhost describes next: those changed lately first, then the next ones going
/// round the vehicle.
#[derive(Debug, Default)]
pub struct Rounds {
    next: usize,
}

impl Rounds {
    /// Up to `n` of `ids` (the vehicle's bricks in object order): those of `first` it has, then
    /// the ones after the last picked, going round. None twice.
    pub fn pick(&mut self, ids: &[CoopId], first: &[CoopId], n: usize) -> Vec<CoopId> {
        let mut picked: Vec<CoopId> = Vec::new();
        if !first.is_empty() {
            let existing: HashSet<CoopId> = ids.iter().copied().collect();
            for &id in first {
                if picked.len() < n && existing.contains(&id) && !picked.contains(&id) {
                    picked.push(id);
                }
            }
        }
        for _ in 0..ids.len() {
            if picked.len() >= n {
                break;
            }
            let id = ids[self.next % ids.len()];
            self.next = (self.next + 1) % ids.len();
            if !picked.contains(&id) {
                picked.push(id);
            }
        }
        picked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u32) -> CoopId {
        CoopId::new(2, n).unwrap()
    }

    fn at(location: [f32; 3], rotation: [f32; 3]) -> Transform {
        Transform { location, rotation }
    }

    fn digest(place: Transform, values: &[(&str, Value)], groups: Groups) -> Digest {
        Digest::of(&place, values.iter().map(|(n, v)| (*n, v)), groups)
    }

    #[test]
    fn digests_ignore_what_doesnt_matter() {
        let values = [
            ("Color", Value::Raw(vec![1, 2])),
            ("Name", Value::Text("a".into())),
        ];
        let reversed = [values[1].clone(), values[0].clone()];
        let place = at([1.0, -2.5, 30.0], [0.0, 90.0, -180.0]);
        let a = digest(place, &values, [1, 2]);
        assert_eq!(a, digest(place, &reversed, [1, 2]));
        // The last bits, signed zeros and whole turns don't count.
        let close = at([1.000_001, -2.5, 30.0], [-0.0, 450.0, 180.0]);
        assert_eq!(a, digest(close, &values, [1, 2]));
    }

    #[test]
    fn digests_tell_each_part_apart() {
        let values = [("Color", Value::Raw(vec![1]))];
        let place = at([1.0, 2.0, 3.0], [0.0; 3]);
        let a = digest(place, &values, [0, 4]);
        let moved = digest(at([1.0, 2.0, 3.01], [0.0; 3]), &values, [0, 4]);
        assert_ne!(a.place, moved.place);
        assert_eq!((a.values, a.groups), (moved.values, moved.groups));
        let painted = digest(place, &[("Color", Value::Raw(vec![2]))], [0, 4]);
        assert_ne!(a.values, painted.values);
        assert_eq!((a.place, a.groups), (painted.place, painted.groups));
        // A value's kind counts, not only its bytes; so does which property holds it.
        let text = digest(place, &[("Color", Value::Text("\u{1}".into()))], [0, 4]);
        assert_ne!(a.values, text.values);
        let other = digest(place, &[("Size", Value::Raw(vec![1]))], [0, 4]);
        assert_ne!(a.values, other.values);
        let regrouped = digest(place, &values, [4, 0]);
        assert_ne!(a.groups, regrouped.groups);
        assert_eq!((a.place, a.values), (regrouped.place, regrouped.values));
    }

    #[test]
    fn comparing() {
        let d = |n: u32| digest(at([n as f32, 0.0, 0.0], [0.0; 3]), &[], [0, 0]);
        let audit = Audit {
            commit: 3,
            count: 3,
            order: order_hash([id(1), id(2), id(3)]),
            bricks: vec![(id(1), d(1)), (id(2), d(2))],
        };
        let same = |n: CoopId| Some(d(n.counter()));
        assert_eq!(audit.compare(3, audit.order, same), vec![]);
        assert_eq!(
            audit.compare(3, order_hash([id(2), id(1), id(3)]), same),
            vec![Difference::Order]
        );
        let differences = audit.compare(2, 0, |n| (n != id(2)).then(|| d(9)));
        assert_eq!(
            differences,
            vec![
                Difference::Count {
                    subhost: 3,
                    here: 2
                },
                Difference::Place(id(1)),
                Difference::Missing(id(2)),
            ],
            "no order while bricks differ"
        );
        assert!(differences.iter().all(|d| d.needs_snapshot()));
        assert!(!Difference::Order.needs_snapshot());
    }

    #[test]
    fn rounds_go_round_with_changed_ones_first() {
        let ids: Vec<CoopId> = (1..=5).map(id).collect();
        let mut rounds = Rounds::default();
        assert_eq!(rounds.pick(&ids, &[], 2), vec![id(1), id(2)]);
        // Changed lately (one gone since): first, then on from where it was.
        assert_eq!(
            rounds.pick(&ids, &[id(5), id(9), id(5)], 3),
            vec![id(5), id(3), id(4)]
        );
        assert_eq!(rounds.pick(&ids, &[], 3), vec![id(5), id(1), id(2)]);
        // More than there are: each once.
        assert_eq!(rounds.pick(&ids, &[id(2)], 10).len(), 5);
        assert_eq!(rounds.pick(&[], &[id(1)], 10), vec![]);
        // The vehicle shrank.
        assert_eq!(rounds.pick(&ids[..1], &[], 4), vec![id(1)]);
    }
}
