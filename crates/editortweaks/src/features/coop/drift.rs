//! Coop's local test only: the drift key makes a member's editor differ from the session on
//! purpose, as a change coop's hooks missed would, to see an audit notice it and mend it (with a
//! snapshot, or the subhost's object order). Each press does the next of: moving the last brick
//! 50 cm up, putting it in another editor group, swapping the last two bricks in the object order,
//! deleting the last brick. Made as coop makes someone else's edit, so nothing is sent.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use ::coop::{CoopId, GroupKind, Op};

use super::apply::Target;
use super::changes;
use super::log::coop_log;
use super::{groups_of, objects, place_of};
use crate::features::keybinds::COOP_DRIFT;

/// Whether the drift key went down since the last call (each editor tick).
pub fn pressed() -> bool {
    static HELD: AtomicBool = AtomicBool::new(false);
    let held = COOP_DRIFT.is_held();
    let was = HELD.swap(held, Ordering::Relaxed);
    held && !was
}

/// Makes the next drift in `target`'s editor.
pub fn drift(target: &Target) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    // SAFETY: the editor is live (it's ticking); game thread.
    let ids: Vec<CoopId> = unsafe { objects(target.game, target.editor) }
        .into_iter()
        .filter_map(changes::id_of)
        .collect();
    let (Some(&id), Some(object)) = (ids.last(), ids.last().and_then(|&id| changes::object(id)))
    else {
        return coop_log!("drift on purpose: no bricks");
    };
    let (op, what) = match NEXT.fetch_add(1, Ordering::Relaxed) % 4 {
        0 => {
            // SAFETY: a live editor object (it has a coop id); game thread.
            let Some(mut transform) = (unsafe { place_of(target.game, object) }) else {
                return coop_log!("drift on purpose: where {id:?} is isn't known");
            };
            transform.location[2] += 50.0;
            (Op::Move { id, transform }, format!("moved {id:?} 50 cm up"))
        }
        1 => {
            // SAFETY: as above.
            let groups = unsafe { groups_of(target.game, object) }.unwrap_or_default();
            let group = groups[GroupKind::Editor as usize].wrapping_add(1).max(1);
            let op = Op::Group {
                id,
                kind: GroupKind::Editor,
                group,
            };
            (op, format!("put {id:?} in editor group {group}"))
        }
        2 if ids.len() >= 2 => {
            let other = ids[ids.len() - 2];
            let op = Op::Order {
                ids: vec![id, other],
            };
            (
                op,
                format!("put {id:?} before {other:?} in the object order"),
            )
        }
        _ => (Op::Delete { id }, format!("deleted {id:?}")),
    };
    let outcome = target.apply(&[op], false);
    coop_log!(
        "drift on purpose: {what}, not sent{}: an audit should notice within seconds",
        if outcome.refused > 0 || outcome.drifted {
            " (the editor refused part of it)"
        } else {
            ""
        }
    );
}
