//! Undo and redo of the references, with the game's keys and in one order with its steps: the
//! editor's undo goes back through both, newest first, whichever made it (`model_io::history`, the
//! game's position from `game::undo_redo`, which asks us about every undo before the game makes it).
//!
//! A step is one change as the player sees it: what changed until nothing did for [`WRITE_DELAY`],
//! when it's also written to the sidecar ([`State::commit`], [`State::flush`]), so that dragging a
//! slider is one step. References going back to how they were are selected, as they're what changed.
//! Going to another vehicle, or the sidecar changing on disk, forgets the steps: what they'd bring
//! back is no longer what's shown ([`State::forget_outside_changes`]).
//!
//! A coop member's references are the host's: they aren't undone. In a host's session coop takes
//! the undos after us, for its own steps, and the game's position doesn't move then: a reference's
//! step is only undone while it's the last thing done.
//!
//! [`WRITE_DELAY`]: super::WRITE_DELAY

use std::sync::Once;
use std::time::Instant;

use brickrust::ue::coreuobject::UObjectBase;
use model_io::history;
use model_io::sidecar::Reference;

use super::{Code, State, edit, state};
use crate::game::undo_redo;
use crate::prelude::*;

/// Asks to be told of undos from now on.
pub(super) fn listen() -> Result<(), String> {
    undo_redo::take_over(asked)
}

/// Follows the game's undo steps from now on (hooked from a live editor).
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
pub(super) unsafe fn watch_steps(code: &Code, editor: *mut UObjectBase) {
    static WATCHING: Once = Once::new();
    WATCHING.call_once(|| {
        let Some(input) = &code.input else {
            return;
        };
        // SAFETY: per the contract.
        if let Err(reason) = unsafe { input.editor.after_undo_step(editor, undo_redo::step_taken) }
        {
            log!("undoing references doesn't follow the game's order: {reason}");
        }
    });
}

/// The player asked to undo (or redo, `redo`) in `editor`: whether it's ours to make, then made.
fn asked(editor: *mut UObjectBase, redo: bool) -> bool {
    let Some(mut guard) = state() else {
        return false;
    };
    let state = &mut *guard;
    if state.received.is_some() || !state.editor.is_some_and(|known| known.is(editor)) {
        return false;
    }
    state.commit();
    let steps = undo_redo::steps_taken();
    if state.seen_steps != steps {
        state.seen_steps = steps;
        state.history.forget_redo();
    }
    let position = undo_redo::position();
    // SAFETY: the game's own argument, its live editor.
    let game_can = unsafe { undo_redo::can_step(editor, redo) };
    let restored = if redo {
        state.history.redo(position, game_can)
    } else {
        state.history.undo(position, game_can)
    };
    let Some(restored) = restored.cloned() else {
        return false;
    };
    log!(
        "{}: {} references",
        if redo { "redone" } else { "undone" },
        restored.len()
    );
    restore(state, restored);
    true
}

/// Goes back to `restored`, showing it at the next tick and writing it a moment later. What differs
/// from what's shown gets selected.
fn restore(state: &mut State, restored: Vec<Reference>) {
    let selected = history::changed(&state.sidecar.references, &restored)
        .into_iter()
        .map(|reference| reference.id.clone())
        .collect();
    if let Some(folder) = state.folder.clone() {
        for problem in edit::remove_copies_left(&state.sidecar.references, &restored, &folder) {
            log!("{problem}");
        }
    }
    state.baseline.clone_from(&restored);
    state.sidecar.references = restored;
    state.select(selected);
    state.refresh_panel = true;
    state.last_poll = None;
    state.unwritten = Some(Instant::now());
}
