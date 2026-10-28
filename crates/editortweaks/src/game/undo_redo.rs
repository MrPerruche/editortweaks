//! The editor's undo and redo, for coop to take over (only in builds with the `coop` feature). The
//! game's undo loads a snapshot of the whole vehicle, which in a coop session would also undo
//! everyone else's work since (`docs/re/coop/design.md`, "Undo").
//!
//! `void ABrickEditor::UndoRedo(bool bUndo)` (BRMK 0xF4740, game 1.11.7 RVA 0xD0E300) is what both
//! come through: its only callers are two wrappers (0xD0E2F0 passing `true` for undo, 0xCF49C0
//! `false` for redo; Ctrl+Z reached the hook with `true`), each called once by the editor's undo
//! and redo actions (0xDCEE4D, 0xDD0C69), which use no result (`docs/re/coop/change-hooks.md`,
//! section 5).

use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;

use crate::framework::hook;
use crate::framework::sig::FunctionSig;

pub const UNDO_REDO: FunctionSig = FunctionSig {
    name: "ABrickEditor::UndoRedo",
    // Its start, up to reading the undo steps' count.
    pattern: "48 89 4c 24 08 55 56 41 55 41 57 48 8d 6c 24 98 48 81 ec 68 01 00 00 44 8b 81 ?? ?? 00 00 4c 8b f9",
};

/// Asked about every undo (`redo` false) or redo in `editor`, before the game makes it: true if
/// it's taken care of, and the game's mustn't run.
pub type TakeOver = fn(editor: *mut UObjectBase, redo: bool) -> bool;

type UndoRedoFn = unsafe extern "C" fn(editor: *mut UObjectBase, undo: bool);

static ORIGINAL: OnceLock<UndoRedoFn> = OnceLock::new();
static TAKE_OVER: OnceLock<TakeOver> = OnceLock::new();

/// Asks `callback` about every undo and redo from now on. Only the first callback counts.
pub fn take_over(callback: TakeOver) -> Result<(), String> {
    if TAKE_OVER.set(callback).is_err() {
        return Ok(());
    }
    let target = UNDO_REDO.resolve()?;
    // SAFETY: `target` is the start of ABrickEditor::UndoRedo, and the detour has its signature.
    unsafe { hook::install_typed(UNDO_REDO.name, target, detour as UndoRedoFn, &ORIGINAL) }
}

unsafe extern "C" fn detour(editor: *mut UObjectBase, undo: bool) {
    if TAKE_OVER.get().is_some_and(|take| take(editor, !undo)) {
        return;
    }
    let original = *ORIGINAL.get().expect("hook recorded before use");
    // SAFETY: the original function with its own arguments.
    unsafe { original(editor, undo) }
}
