//! Object order ([`Op::Order`](crate::Op::Order)): the order of the editor's objects, which the
//! game shows as seat order, camera order and the like. Generic over what names an object (coop
//! ids when sending, the game's objects when applying), so the mod and the tests share the rules.
//!
//! An order list names bricks in their new order: they go first, the others follow in the order
//! they had. Every member ends with the same order whatever else changed meanwhile (bricks deleted
//! drop out, bricks created meanwhile keep their place among the unlisted ones).

use std::collections::{HashMap, HashSet};
use std::hash::Hash;

/// `current` reordered by the order list `listed`: the listed objects first, in that order
/// (unknown ones and repeats skipped), then the others in the order they had.
pub fn reordered<K: Copy + Eq + Hash>(current: &[K], listed: &[K]) -> Vec<K> {
    let present: HashSet<K> = current.iter().copied().collect();
    let mut taken = HashSet::new();
    let mut order: Vec<K> = listed
        .iter()
        .copied()
        .filter(|k| present.contains(k) && taken.insert(*k))
        .collect();
    order.extend(current.iter().copied().filter(|k| !taken.contains(k)));
    order
}

/// The shortest order list that turns `was` into `now` (see [`reordered`]), or `None` if the
/// objects both have are in the same order. Objects only one of them has are left out: they were
/// created or deleted, which their own ops tell.
pub fn change<K: Copy + Eq + Hash>(was: &[K], now: &[K]) -> Option<Vec<K>> {
    let in_was: HashSet<K> = was.iter().copied().collect();
    let in_now: HashSet<K> = now.iter().copied().collect();
    let was: Vec<K> = was.iter().copied().filter(|k| in_now.contains(k)).collect();
    let now: Vec<K> = now.iter().copied().filter(|k| in_was.contains(k)).collect();
    // After the last difference both hold the same objects in the same order, so listing the
    // objects up to it is enough: they're the same set in both.
    let last = was.iter().zip(&now).rposition(|(a, b)| a != b)?;
    Some(now[..=last].to_vec())
}

/// Moves turning `current` into `target` (the same objects, each once), as the editor makes them:
/// each `(object, index)` takes the object out and puts it back at `index`. Only the objects off
/// the longest run already in target order move, each once.
pub fn moves<K: Copy + Eq + Hash>(current: &[K], target: &[K]) -> Vec<(K, usize)> {
    let rank: HashMap<K, usize> = target.iter().enumerate().map(|(i, &k)| (k, i)).collect();
    let ranks: Vec<usize> = current.iter().map(|k| rank[k]).collect();
    let mut placed: HashSet<K> = longest_increasing(&ranks)
        .into_iter()
        .map(|i| current[i])
        .collect();
    // Placed objects stay in target order: each newly placed one goes right after the one before
    // it in the target, which is placed already.
    let mut work = current.to_vec();
    let mut moves = Vec::new();
    for (t, &object) in target.iter().enumerate() {
        if !placed.insert(object) {
            continue;
        }
        let from = position(&work, object);
        let to = match t {
            0 => 0,
            _ => {
                let before = position(&work, target[t - 1]);
                if from < before { before } else { before + 1 }
            }
        };
        if from != to {
            work.remove(from);
            work.insert(to, object);
            moves.push((object, to));
        }
    }
    debug_assert!(work == target, "the moves reach the target");
    moves
}

fn position<K: Eq>(list: &[K], object: K) -> usize {
    list.iter()
        .position(|k| *k == object)
        .expect("the same objects")
}

/// Indices of a longest strictly increasing subsequence of `values` (patience sorting).
fn longest_increasing(values: &[usize]) -> Vec<usize> {
    // tails[l]: the index ending the best run of length l + 1 found so far.
    let mut tails: Vec<usize> = Vec::new();
    let mut previous = vec![usize::MAX; values.len()];
    for (i, &value) in values.iter().enumerate() {
        let length = tails.partition_point(|&j| values[j] < value);
        if length > 0 {
            previous[i] = tails[length - 1];
        }
        if length == tails.len() {
            tails.push(i);
        } else {
            tails[length] = i;
        }
    }
    let mut run = Vec::with_capacity(tails.len());
    let mut at = tails.last().copied();
    while let Some(i) = at {
        run.push(i);
        at = (previous[i] != usize::MAX).then(|| previous[i]);
    }
    run.reverse();
    run
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small deterministic generator (xorshift).
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }

        fn shuffle(&mut self, list: &mut [u32]) {
            for i in (1..list.len()).rev() {
                list.swap(i, self.below(i + 1));
            }
        }
    }

    fn apply(current: &[u32], moves: &[(u32, usize)]) -> Vec<u32> {
        let mut list = current.to_vec();
        for &(object, to) in moves {
            let from = position(&list, object);
            list.remove(from);
            list.insert(to, object);
        }
        list
    }

    #[test]
    fn listed_go_first() {
        assert_eq!(reordered(&[1, 2, 3, 4], &[4, 9, 2, 4]), vec![4, 2, 1, 3]);
    }

    #[test]
    fn a_swap_is_two_moves() {
        let current = [1, 2, 3, 4, 5, 6];
        let target = [1, 5, 3, 4, 2, 6];
        let moves = moves(&current, &target);
        assert_eq!(apply(&current, &moves), target);
        assert_eq!(moves.len(), 2);
    }

    #[test]
    fn moves_reach_any_order() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for size in 0..40 {
            for _ in 0..20 {
                let current: Vec<u32> = (0..size).collect();
                let mut target = current.clone();
                rng.shuffle(&mut target);
                assert_eq!(apply(&current, &moves(&current, &target)), target);
            }
        }
    }

    #[test]
    fn change_lists_up_to_the_last_difference() {
        assert_eq!(change(&[1, 2, 3, 4, 5], &[1, 2, 3, 4, 5]), None);
        assert_eq!(
            change(&[1, 2, 3, 4, 5], &[1, 3, 2, 4, 5]),
            Some(vec![1, 3, 2])
        );
        // Created (6) and deleted (4) objects don't count.
        assert_eq!(change(&[1, 2, 3, 4], &[1, 2, 6, 3]), None);
    }

    #[test]
    fn change_then_reordered_gives_the_new_order() {
        let mut rng = Rng(42);
        for size in 1..30 {
            for _ in 0..20 {
                let was: Vec<u32> = (0..size).collect();
                let mut now = was.clone();
                // Mostly small changes, as the editor's panel makes them.
                let (a, b) = (rng.below(size as usize), rng.below(size as usize));
                now.swap(a, b);
                if rng.below(3) == 0 {
                    rng.shuffle(&mut now);
                }
                let list = change(&was, &now).unwrap_or_default();
                assert_eq!(reordered(&was, &list), now);
            }
        }
    }
}
