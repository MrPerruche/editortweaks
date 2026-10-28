//! Undoing one's own edits (`design.md`, "Undo"). The game's undo loads a whole-vehicle snapshot,
//! which would also undo everyone else's work since; in a session each member undoes and redoes
//! only its own edits, as new edits that travel like any other.
//!
//! The editor side records each local edit as [`Change`]s: the ops it sent, and the ops that undo
//! them, worked out from the vehicle as it was just before. Undoing makes the newest step's undo
//! ops (bricks brought back first, so that the rest can refer to them), and the step becomes one
//! to redo, the two the other way round. A new edit drops what could be redone.
//!
//! Someone else's change wins: a change is dropped from a step once an edit of someone else's,
//! ordered after the step, writes something it touches (the same brick's place, property or group,
//! the object order, or anything on a brick it made or deleted). An edit of someone else's ordered
//! before a step that still waits for its commit was skipped here (the step overwrites it), but it
//! is what the step really overwrote: it becomes what undoing the step brings back.

use crate::ids::{CoopId, LiveSet};
use crate::ops::Op;

/// Caps on what's kept to undo.
pub mod caps {
    /// Steps kept each way.
    pub const STEPS: usize = 100;
    /// Bytes of brick archives kept, to bring deleted bricks back; the oldest steps go first.
    pub const ARCHIVE_BYTES: usize = 64 << 20;
}

/// Part of an edit that can be undone on its own: what it did, and what undoes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// The ops sent for it.
    pub done: Vec<Op>,
    /// The ops that undo it, as worked out when it was done: deleted bricks made again (with the
    /// same ids), values as they were, ... May be empty for a change only redoing does something
    /// for, and `done` for one only undoing does something for (putting a brick made again back
    /// in its place in the object order).
    pub undo: Vec<Op>,
}

impl Change {
    /// Whether `op` writes something this change touches.
    fn touched_by(&self, op: &Op) -> bool {
        self.done
            .iter()
            .chain(&self.undo)
            .any(|mine| mine.touches(op))
    }

    /// Whether undoing it makes bricks.
    fn makes_bricks(&self) -> bool {
        self.undo.iter().any(|op| matches!(op, Op::Create { .. }))
    }

    fn archive_bytes(&self) -> usize {
        self.done
            .iter()
            .chain(&self.undo)
            .map(|op| match op {
                Op::Create { archive, .. } => archive.len(),
                _ => 0,
            })
            .sum()
    }
}

/// One local edit (or undo, or redo), as changes.
#[derive(Debug)]
struct Step {
    changes: Vec<Change>,
    /// The proposal that sent it (`Session::edit`), to tell whether it's committed yet; 0 at the
    /// subhost, whose edits are committed at once.
    local: u32,
    /// Its changes dropped since, as someone else changed what they touched.
    dropped: usize,
}

impl Step {
    /// `op`, someone else's, comes after this step: what it overwrote is theirs now.
    fn overwritten_after(&mut self, op: &Op) {
        let before = self.changes.len();
        self.changes.retain(|change| !change.touched_by(op));
        self.dropped += before - self.changes.len();
    }

    /// `op`, someone else's, comes before this step, which waits for its commit: it's what the
    /// step overwrites, so undoing the step brings it back. A change on a brick `op` deleted (or
    /// anything else odd) is moot.
    fn overwritten_before(&mut self, op: &Op) {
        let theirs = op.keys();
        let brick_op = matches!(op, Op::Create { .. } | Op::Delete { .. });
        let mut dropped = 0;
        self.changes.retain_mut(|change| {
            if !change.touched_by(op) {
                return true;
            }
            if !brick_op {
                let same = |mine: &Op| mine.keys() == theirs;
                if let Some(mine) = change.undo.iter_mut().find(|mine| same(mine)) {
                    *mine = op.clone();
                    return true;
                }
                // A brick the change deletes: its archive was saved before `op`.
                let deleted = theirs[0]
                    .brick()
                    .is_some_and(|id| change.done.contains(&Op::Delete { id }));
                if deleted {
                    change.undo.push(op.clone());
                    return true;
                }
            }
            dropped += 1;
            false
        });
        self.dropped += dropped;
    }
}

/// What undoing or redoing a step made.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StepBack {
    /// The ops, already sent: for the editor to make.
    pub ops: Vec<Op>,
    /// Changes of the step left out: someone else changed what they touched since, or (rarely)
    /// they can't be made any more (a brick to make again exists again).
    pub skipped: usize,
}

/// A member's steps to undo and redo.
#[derive(Debug, Default)]
pub(crate) struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
}

impl History {
    /// A new local edit, sent as proposal `local`: it can be undone, and nothing redone.
    pub fn record(&mut self, changes: Vec<Change>, local: u32) {
        self.redo.clear();
        self.push(false, changes, local);
    }

    /// A new local edit that can't be undone: nothing can be redone either any more.
    pub fn forget_redo(&mut self) {
        self.redo.clear();
    }

    /// Adds a step to undo (`redo` false) or to redo, sent as proposal `local`.
    pub fn push(&mut self, redo: bool, changes: Vec<Change>, local: u32) {
        if changes.is_empty() {
            return;
        }
        let step = Step {
            changes,
            local,
            dropped: 0,
        };
        if redo {
            self.redo.push(step);
        } else {
            self.undo.push(step);
        }
        self.trim();
    }

    /// Drops the oldest steps over the caps.
    fn trim(&mut self) {
        for stack in [&mut self.undo, &mut self.redo] {
            let excess = stack.len().saturating_sub(caps::STEPS);
            stack.drain(..excess);
        }
        let bytes = |step: &Step| {
            step.changes
                .iter()
                .map(Change::archive_bytes)
                .sum::<usize>()
        };
        let mut total: usize = self.undo.iter().chain(&self.redo).map(bytes).sum();
        while total > caps::ARCHIVE_BYTES {
            // The step to redo last is the oldest one there.
            let stack = if self.undo.is_empty() {
                &mut self.redo
            } else {
                &mut self.undo
            };
            total -= bytes(&stack.remove(0));
        }
    }

    /// Takes the newest step to undo (`redo` false) or redo: the ops of it that can still be
    /// made, in the order to make them, with `live` the bricks the local editor has; and the
    /// step the other way. Steps left with nothing to make (someone else changed all they
    /// touched) are passed over, counted as skipped, so that one undo always does something if
    /// it can. `None` if there's no step.
    pub fn take(&mut self, redo: bool, mut live: LiveSet) -> Option<(StepBack, Vec<Change>)> {
        let stack = if redo { &mut self.redo } else { &mut self.undo };
        let mut taken = None;
        let mut other_way = Vec::new();
        while other_way.is_empty() {
            let Some(step) = stack.pop() else { break };
            let back = taken.get_or_insert_with(StepBack::default);
            back.skipped += step.dropped;
            // Newest change first, but bricks are made first so that the rest can refer to them.
            let (making, rest): (Vec<Change>, Vec<Change>) = step
                .changes
                .into_iter()
                .rev()
                .partition(Change::makes_bricks);
            for change in making.into_iter().chain(rest) {
                match possible(&change.undo, &mut live) {
                    Some(ops) if !ops.is_empty() || change.undo.is_empty() => {
                        back.ops.extend(ops.iter().cloned());
                        other_way.push(Change {
                            done: ops,
                            undo: change.done,
                        });
                    }
                    _ => back.skipped += 1,
                }
            }
        }
        taken.map(|back| (back, other_way))
    }

    /// Someone else's committed edit: `ops`, those of it that did something (see
    /// [`Op::is_effective`]). `pending(local)`: whether our proposal `local` still waits for its
    /// commit, which then comes after this one.
    pub fn foreign<'a>(
        &mut self,
        ops: impl IntoIterator<Item = &'a Op>,
        pending: impl Fn(u32) -> bool,
    ) {
        if self.undo.is_empty() && self.redo.is_empty() {
            return;
        }
        for op in ops {
            for step in &mut self.undo {
                if pending(step.local) {
                    step.overwritten_before(op);
                } else {
                    step.overwritten_after(op);
                }
            }
            // A redo step still waiting is our undo, ordered after `op`: redoing it makes our
            // change again, whatever came before.
            for step in self.redo.iter_mut().filter(|step| !pending(step.local)) {
                step.overwritten_after(op);
            }
        }
    }
}

/// `ops` as far as they can be made with the bricks in `live` (updated if so): `None` if they'd
/// make a brick that exists; ops on bricks that don't exist are left out.
fn possible(ops: &[Op], live: &mut LiveSet) -> Option<Vec<Op>> {
    let mut made = Vec::new();
    let mut gone = Vec::new();
    let mut kept = Vec::new();
    for op in ops {
        let exists = |id: CoopId| !gone.contains(&id) && (made.contains(&id) || live.contains(id));
        match op {
            Op::Create { ids, .. } => {
                if ids.iter().any(|&id| exists(id)) {
                    return None;
                }
                made.extend(ids.iter().copied());
                gone.retain(|id| !ids.contains(id));
            }
            Op::Delete { id } => {
                if !exists(*id) {
                    continue;
                }
                gone.push(*id);
                made.retain(|made| made != id);
            }
            Op::Move { id, .. } | Op::Set { id, .. } | Op::Group { id, .. } => {
                if !exists(*id) {
                    continue;
                }
            }
            Op::Order { .. } => {}
        }
        kept.push(op.clone());
    }
    kept.iter().for_each(|op| op.apply_to(live));
    Some(kept)
}

/// The order list putting bricks made again back where they were, after deleting `deleted` (in
/// the order undoing makes them again, which puts them last) from the object order `was`; `None`
/// if they come back where they were anyway.
pub fn restore_order(was: &[CoopId], deleted: &[CoopId]) -> Option<Vec<CoopId>> {
    let mut back: Vec<CoopId> = was
        .iter()
        .copied()
        .filter(|id| !deleted.contains(id))
        .collect();
    back.extend(deleted.iter().copied().filter(|id| was.contains(id)));
    crate::order::change(&back, was)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{Transform, Value};

    fn id(n: u32) -> CoopId {
        CoopId::new(2, n).unwrap()
    }

    fn moved(n: u32, x: f32) -> Op {
        Op::Move {
            id: id(n),
            transform: Transform {
                location: [x, 0.0, 0.0],
                rotation: [0.0; 3],
            },
        }
    }

    fn color(n: u32, c: u8) -> Op {
        Op::Set {
            id: id(n),
            property: "Color".into(),
            value: Value::Raw(vec![c]),
        }
    }

    fn change(done: Op, undo: Op) -> Change {
        Change {
            done: vec![done],
            undo: vec![undo],
        }
    }

    fn live(ns: &[u32]) -> LiveSet {
        LiveSet::from_ids(ns.iter().map(|&n| id(n)))
    }

    #[test]
    fn undo_then_redo_swaps() {
        let mut history = History::default();
        history.record(vec![change(moved(1, 5.0), moved(1, 0.0))], 0);
        let (back, other_way) = history.take(false, live(&[1])).unwrap();
        assert_eq!(back.ops, vec![moved(1, 0.0)]);
        assert_eq!(back.skipped, 0);
        history.push(true, other_way, 0);
        assert!(history.take(false, live(&[1])).is_none());
        let (back, _) = history.take(true, live(&[1])).unwrap();
        assert_eq!(back.ops, vec![moved(1, 5.0)]);
    }

    #[test]
    fn a_new_edit_drops_redo() {
        let mut history = History::default();
        history.push(true, vec![change(moved(1, 5.0), moved(1, 0.0))], 0);
        history.record(vec![change(color(1, 2), color(1, 1))], 0);
        assert!(history.take(true, live(&[1])).is_none());
    }

    #[test]
    fn bricks_come_back_first() {
        let mut history = History::default();
        let create = Op::Create {
            ids: vec![id(2)],
            archive: vec![1],
        };
        let refer = |ids: Vec<CoopId>| Op::Set {
            id: id(1),
            property: "Inputs.SourceBricks".into(),
            value: Value::Bricks(ids),
        };
        history.record(
            vec![
                change(Op::Delete { id: id(2) }, create.clone()),
                // The game dropped the reference to the deleted brick.
                change(refer(vec![]), refer(vec![id(2)])),
            ],
            0,
        );
        let (back, _) = history.take(false, live(&[1])).unwrap();
        assert_eq!(back.ops, vec![create, refer(vec![id(2)])]);
    }

    #[test]
    fn what_cant_be_made_is_skipped() {
        let mut history = History::default();
        history.record(
            vec![
                change(
                    Op::Delete { id: id(2) },
                    Op::Create {
                        ids: vec![id(2)],
                        archive: vec![],
                    },
                ),
                change(moved(3, 1.0), moved(3, 0.0)),
                change(moved(1, 1.0), moved(1, 0.0)),
            ],
            0,
        );
        // Brick 2 exists again, brick 3 is gone.
        let (back, other_way) = history.take(false, live(&[1, 2])).unwrap();
        assert_eq!(back.ops, vec![moved(1, 0.0)]);
        assert_eq!(back.skipped, 2);
        assert_eq!(other_way, vec![change(moved(1, 0.0), moved(1, 1.0))]);
    }

    #[test]
    fn later_edits_of_others_win() {
        let mut history = History::default();
        history.record(
            vec![
                change(moved(1, 5.0), moved(1, 0.0)),
                change(color(1, 2), color(1, 1)),
            ],
            0,
        );
        // Someone moved brick 1 after us.
        history.foreign(&[moved(1, 9.0)], |_| false);
        let (back, _) = history.take(false, live(&[1])).unwrap();
        assert_eq!(back.ops, vec![color(1, 1)]);
        assert_eq!(back.skipped, 1);
    }

    #[test]
    fn steps_others_overwrote_entirely_are_passed_over() {
        let mut history = History::default();
        history.record(vec![change(color(1, 2), color(1, 1))], 0);
        history.record(vec![change(moved(1, 5.0), moved(1, 0.0))], 0);
        history.record(vec![change(moved(1, 6.0), moved(1, 5.0))], 0);
        // Someone moved brick 1 after us: both moves are theirs now.
        history.foreign(&[moved(1, 9.0)], |_| false);
        let (back, other_way) = history.take(false, live(&[1])).unwrap();
        assert_eq!(back.ops, vec![color(1, 1)]);
        assert_eq!(back.skipped, 2);
        assert_eq!(other_way, vec![change(color(1, 1), color(1, 2))]);
        assert!(history.take(false, live(&[1])).is_none());
        // Only steps with nothing left: still an answer, so the log can say why nothing happened.
        let mut history = History::default();
        history.record(vec![change(moved(1, 5.0), moved(1, 0.0))], 0);
        history.foreign(&[moved(1, 9.0)], |_| false);
        let (back, other_way) = history.take(false, live(&[1])).unwrap();
        assert_eq!((back.ops.len(), back.skipped, other_way.len()), (0, 1, 0));
        assert!(history.take(false, live(&[1])).is_none());
    }

    #[test]
    fn earlier_edits_of_others_are_what_we_overwrote() {
        let mut history = History::default();
        history.record(
            vec![
                change(moved(1, 5.0), moved(1, 0.0)),
                change(color(1, 2), color(1, 1)),
            ],
            4,
        );
        // Committed before our proposal 4: our move overwrote theirs.
        history.foreign(&[moved(1, 9.0)], |local| local == 4);
        let (back, _) = history.take(false, live(&[1])).unwrap();
        assert_eq!(back.ops, vec![color(1, 1), moved(1, 9.0)]);
    }

    #[test]
    fn earlier_edits_of_a_brick_we_deleted_come_back_with_it() {
        let mut history = History::default();
        let create = Op::Create {
            ids: vec![id(1)],
            archive: vec![],
        };
        history.record(vec![change(Op::Delete { id: id(1) }, create.clone())], 4);
        history.foreign(&[color(1, 3)], |local| local == 4);
        let (back, _) = history.take(false, live(&[])).unwrap();
        assert_eq!(back.ops, vec![create, color(1, 3)]);
    }

    #[test]
    fn deleting_first_makes_our_change_moot() {
        let mut history = History::default();
        history.record(vec![change(moved(1, 5.0), moved(1, 0.0))], 4);
        history.foreign(&[Op::Delete { id: id(1) }], |local| local == 4);
        let (back, _) = history.take(false, live(&[])).unwrap();
        assert_eq!(
            back,
            StepBack {
                ops: vec![],
                skipped: 1
            }
        );
    }

    #[test]
    fn redo_waiting_is_not_overwritten_before() {
        let mut history = History::default();
        history.push(true, vec![change(moved(1, 0.0), moved(1, 5.0))], 4);
        history.foreign(&[moved(1, 9.0)], |local| local == 4);
        let (back, _) = history.take(true, live(&[1])).unwrap();
        assert_eq!(back.ops, vec![moved(1, 5.0)]);
        // Once committed, a later move of theirs wins.
        history.push(true, vec![change(moved(1, 0.0), moved(1, 5.0))], 4);
        history.foreign(&[moved(1, 9.0)], |_| false);
        let (back, _) = history.take(true, live(&[1])).unwrap();
        assert_eq!(back.skipped, 1);
    }

    #[test]
    fn order_after_bricks_come_back() {
        let ids: Vec<CoopId> = (1..=5).map(id).collect();
        assert_eq!(restore_order(&ids, &[id(5)]), None);
        let list = restore_order(&ids, &[id(2), id(4)]).unwrap();
        let mut now = vec![id(1), id(3), id(5), id(2), id(4)];
        now = crate::order::reordered(&now, &list);
        assert_eq!(now, ids);
    }

    #[test]
    fn archives_are_capped() {
        let mut history = History::default();
        let big = |n| Change {
            done: vec![Op::Delete { id: id(n) }],
            undo: vec![Op::Create {
                ids: vec![id(n)],
                archive: vec![0; caps::ARCHIVE_BYTES / 2],
            }],
        };
        for n in 1..=3 {
            history.record(vec![big(n)], 0);
        }
        assert_eq!(history.undo.len(), 2);
    }
}
