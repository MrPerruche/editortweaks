//! The editor's undo and redo, for features to take over: coop (the game's undo loads a snapshot of
//! the whole vehicle, which in a coop session would also undo everyone else's work since,
//! `docs/re/coop/design.md`, "Undo") and the references, which aren't in the game's history.
//!
//! `void ABrickEditor::UndoRedo(bool bUndo)` (BRMK 0xF4740, game 1.11.7 RVA 0xD0E300) is what both
//! come through: its only callers are two wrappers (0xD0E2F0 passing `true` for undo, 0xCF49C0
//! `false` for redo; Ctrl+Z reached the hook with `true`), each called once by the editor's undo
//! and redo actions (0xDCEE4D, 0xDD0C69), which use no result (`docs/re/coop/change-hooks.md`,
//! section 5).
//!
//! The game's history isn't reflected: its code gives the fields ([`Stack`], see
//! `docs/re/editor-operations.md`, "The undo history"). [`position`] counts where it is, for
//! histories kept beside it (`model_io::history`).

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;

use crate::framework::hook;
use crate::framework::sig::FunctionSig;
use crate::framework::timings;

pub const UNDO_REDO: FunctionSig = FunctionSig {
    name: "ABrickEditor::UndoRedo",
    // Its start, up to reading the steps' count (displacement at +26) and how many are undone (+36).
    pattern: "48 89 4c 24 08 55 56 41 55 41 57 48 8d 6c 24 98 48 81 ec 68 01 00 00 44 8b 81 ?? ?? 00 00 4c 8b f9 44 8b 89 ?? ?? 00 00",
};

/// Where [`UNDO_REDO`]'s pattern holds the offsets of [`Stack`]'s fields.
const COUNT_DISPLACEMENT: usize = 26;
const UNDONE_DISPLACEMENT: usize = 36;

/// Asked about every undo (`redo` false) or redo in `editor`, before the game makes it: true if
/// it's taken care of, and the game's mustn't run.
pub type TakeOver = fn(editor: *mut UObjectBase, redo: bool) -> bool;

type UndoRedoFn = unsafe extern "C" fn(editor: *mut UObjectBase, undo: bool);

/// Where the editor keeps its undo history: `int32` fields read from [`UNDO_REDO`]'s code.
#[derive(Clone, Copy)]
struct Stack {
    /// How many steps it holds (`TArray`'s count).
    count: usize,
    /// How many of them are undone, from the newest.
    undone: usize,
}

impl Stack {
    /// # Safety
    /// `editor` must be a live `ABrickEditor`.
    unsafe fn read(&self, editor: *mut UObjectBase) -> (i32, i32) {
        let base = editor.cast::<u8>();
        // SAFETY: per the contract; the offsets come from the game's own code.
        unsafe {
            (
                base.add(self.count).cast::<i32>().read(),
                base.add(self.undone).cast::<i32>().read(),
            )
        }
    }
}

static ORIGINAL: OnceLock<UndoRedoFn> = OnceLock::new();
static STACK: OnceLock<Stack> = OnceLock::new();
/// Whether the hook is in place, or why not.
static HOOKED: OnceLock<Result<(), String>> = OnceLock::new();
static TAKE_OVERS: Mutex<Vec<TakeOver>> = Mutex::new(Vec::new());
static POSITION: AtomicI64 = AtomicI64::new(0);
static STEPS_TAKEN: AtomicU64 = AtomicU64::new(0);

/// Asks `callback` about every undo and redo from now on, after the callbacks before it, until one
/// takes it.
pub fn take_over(callback: TakeOver) -> Result<(), String> {
    HOOKED.get_or_init(install).clone()?;
    TAKE_OVERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(callback);
    Ok(())
}

fn install() -> Result<(), String> {
    let found = UNDO_REDO.locate()?;
    let displacement = |offset: usize| {
        // SAFETY: displacement bytes inside the matched pattern.
        unsafe { found.address.add(offset).cast::<u32>().read_unaligned() as usize }
    };
    let _ = STACK.set(Stack {
        count: displacement(COUNT_DISPLACEMENT),
        undone: displacement(UNDONE_DISPLACEMENT),
    });
    // SAFETY: `found.function` is the start of ABrickEditor::UndoRedo, and the detour has its
    // signature.
    unsafe {
        hook::install_typed(
            UNDO_REDO.name,
            found.function,
            detour as UndoRedoFn,
            &ORIGINAL,
        )
    }
}

/// Where the game's history is: one up for each step taken or redone, one down for each undone. Only
/// comparable with itself, and only while [`step_taken`] is told of the steps.
pub fn position() -> i64 {
    POSITION.load(Ordering::Relaxed)
}

/// How many new steps the game's history took: each ends its redos.
pub fn steps_taken() -> u64 {
    STEPS_TAKEN.load(Ordering::Relaxed)
}

/// To be called when the game's history takes a new step (`BrickEditor::after_undo_step`).
pub fn step_taken() {
    POSITION.fetch_add(1, Ordering::Relaxed);
    STEPS_TAKEN.fetch_add(1, Ordering::Relaxed);
}

/// Whether the game's own undo (or redo, `redo`) in `editor` would change anything.
///
/// # Safety
/// `editor` must be a live `ABrickEditor`.
pub unsafe fn can_step(editor: *mut UObjectBase, redo: bool) -> bool {
    let Some(stack) = STACK.get() else {
        return false;
    };
    // SAFETY: per the contract.
    let (count, undone) = unsafe { stack.read(editor) };
    // As the game: the wanted number of undone steps stays within the history.
    let wanted = if redo { undone - 1 } else { undone + 1 };
    count > 0 && wanted.clamp(0, count - 1) != undone
}

unsafe extern "C" fn detour(editor: *mut UObjectBase, undo: bool) {
    let _span = timings::span("game::undo_redo::detour");
    // Copied out, so that a callback may take over too without a deadlock.
    let take_overs = TAKE_OVERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if take_overs.iter().any(|take| take(editor, !undo)) {
        return;
    }
    let original = *ORIGINAL.get().expect("hook recorded before use");
    let stack = *STACK.get().expect("found before the hook");
    // SAFETY: the game's live editor, and the original function with its own arguments.
    unsafe {
        let (_, before) = stack.read(editor);
        timings::game(UNDO_REDO.name, || original(editor, undo));
        let (_, after) = stack.read(editor);
        POSITION.fetch_add(i64::from(before - after), Ordering::Relaxed);
    }
}
