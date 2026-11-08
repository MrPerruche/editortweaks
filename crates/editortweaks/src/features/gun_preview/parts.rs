//! Which bricks make one part of a vehicle: those held together by connections, directly or through
//! others (welded bricks hold together too). A gun's barrels are the barrel bricks of its part only:
//! `UBrick::CalcMuzzleTransform` (BRMK 0x8DF30) looks through the bricks of the gun's
//! `BrickPartRoot` ("Current root of the part this brick belongs to", brick +0xC0; the root lists
//! them at +0xD0), which a spawned vehicle sets up from its connections. A barrel that isn't
//! connected to the gun, like one of a target's guns built in front of it, isn't the gun's.
//!
//! Plain numbers (brick addresses) in, so no game code here.

use std::collections::HashMap;

/// Bricks by part: each brick's part, as a number shared by the bricks of that part.
#[derive(Debug, Default)]
pub struct Parts {
    part: HashMap<usize, usize>,
}

impl Parts {
    /// The parts of bricks joined by `connections` (pairs of bricks) and by their weld groups
    /// (`welds`: a brick and its weld group, 0 for none).
    pub fn new(
        connections: impl IntoIterator<Item = (usize, usize)>,
        welds: impl IntoIterator<Item = (usize, u16)>,
    ) -> Self {
        let mut joined = Joined::default();
        for (brick, other) in connections {
            joined.join(brick, other);
        }
        let mut first_of_group: HashMap<u16, usize> = HashMap::new();
        for (brick, group) in welds {
            joined.add(brick);
            if group != 0 {
                let first = *first_of_group.entry(group).or_insert(brick);
                joined.join(first, brick);
            }
        }
        let bricks: Vec<usize> = joined.parent.keys().copied().collect();
        let part = bricks
            .into_iter()
            .map(|brick| (brick, joined.root(brick)))
            .collect();
        Self { part }
    }

    /// Whether `brick` and `other` are of one part. A brick nothing was told about is a part of its
    /// own.
    pub fn same(&self, brick: usize, other: usize) -> bool {
        brick == other
            || matches!(
                (self.part.get(&brick), self.part.get(&other)),
                (Some(a), Some(b)) if a == b
            )
    }
}

/// Union-find over brick addresses.
#[derive(Default)]
struct Joined {
    parent: HashMap<usize, usize>,
}

impl Joined {
    fn add(&mut self, brick: usize) {
        self.parent.entry(brick).or_insert(brick);
    }

    fn root(&mut self, brick: usize) -> usize {
        self.add(brick);
        let mut root = brick;
        while self.parent[&root] != root {
            root = self.parent[&root];
        }
        // Point the way there straight at the root.
        let mut at = brick;
        while at != root {
            let next = self.parent[&at];
            self.parent.insert(at, root);
            at = next;
        }
        root
    }

    fn join(&mut self, a: usize, b: usize) {
        let (a, b) = (self.root(a), self.root(b));
        if a != b {
            self.parent.insert(a, b);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Parts;

    #[test]
    fn connected_through_others() {
        let parts = Parts::new([(1, 2), (2, 3), (10, 11)], []);
        assert!(parts.same(1, 3));
        assert!(parts.same(10, 11));
        assert!(!parts.same(1, 10));
    }

    #[test]
    fn welded_bricks_hold_together() {
        // 1-2 connected, 3 welded to 2 (welded bricks aren't reported as connected), 4 alone.
        let parts = Parts::new([(1, 2)], [(1, 0), (2, 7), (3, 7), (4, 0)]);
        assert!(parts.same(1, 3));
        assert!(!parts.same(1, 4));
        // Weld group 0 joins nothing.
        let parts = Parts::new([], [(5, 0), (6, 0)]);
        assert!(!parts.same(5, 6));
    }

    #[test]
    fn unknown_bricks_are_alone() {
        let parts = Parts::new([(1, 2)], []);
        assert!(parts.same(9, 9));
        assert!(!parts.same(1, 9));
    }
}
