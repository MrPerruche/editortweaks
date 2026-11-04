//! The vehicle editor itself (`ABrickEditor`): its tick, its operations (one undo step each) and its
//! player controller (its modes: `game::editor_mode`). Found through BRMK 1.11.6's exports, then in
//! game 1.11.7 (build 25831956); see `docs/re/editor-operations.md`.
//!
//! Every editor action that changes the vehicle runs as a scoped operation: the editor keeps the
//! outermost one open (nested ones join it); finishing it calls `ABrickEditor::PostModify`, which
//! saves the undo snapshot, re-pairs mirrored bricks and so on. Neither is reflected, and both are
//! inlined where used, so [`Operations`] does what the game's code does, with the field and vtable
//! offsets read from one of the game's own operations.

use std::panic::Location;
use std::sync::{Mutex, OnceLock, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;

use crate::framework::hook;
use crate::framework::sig::{FunctionSig, SigMatch};
use crate::framework::timings;
use crate::game::reflection;

/// `void ABrickEditor::Tick(float DeltaSeconds)` (BRMK 0xEFA30, game RVA 0xD0B0C0): every frame
/// the editor is open (the vehicle editor's own `Tick` calls it first). After `AActor::Tick` it
/// moves the camera by its input (three floats compared with zero), then updates the camera speed,
/// hovered object and gizmo.
pub const TICK: FunctionSig = FunctionSig {
    name: "ABrickEditor::Tick",
    // After AActor::Tick: a call on a member, then the camera input vector compared with zero.
    pattern: "0f 28 ce 48 8d 8b ?? ?? 00 00 e8 ?? ?? ?? ?? f3 0f 10 bb ?? ?? 00 00 0f 57 c0 0f 2e f8 75 ?? 0f 2e 83 ?? ?? 00 00 75 ?? 0f 2e 83",
};

pub type TickFn = unsafe extern "C" fn(editor: *mut UObjectBase, delta: f32);

/// What runs after each editor tick (see [`after_tick`]): the live editor ticking and the frame's
/// time. Called on the game thread.
pub type AfterTick = fn(editor: *mut UObjectBase, delta: f32);

static ORIGINAL_TICK: OnceLock<TickFn> = OnceLock::new();
/// The callbacks, with where they were asked for (see [`caller_name`]).
static AFTER_TICK: Mutex<Vec<(AfterTick, &'static str)>> = Mutex::new(Vec::new());

/// Runs `callback` after every `ABrickEditor::Tick`, in the order features asked. The loader allows
/// one hook per function, so features share this one; the first call installs it.
#[track_caller]
pub fn after_tick(callback: AfterTick) -> Result<(), String> {
    if ORIGINAL_TICK.get().is_none() {
        let tick = TICK.resolve()?;
        // SAFETY: `tick` is the start of ABrickEditor::Tick, and the detour has its signature.
        unsafe { hook::install_typed(TICK.name, tick, tick_detour as TickFn, &ORIGINAL_TICK)? };
    }
    AFTER_TICK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((callback, caller_name(Location::caller())));
    Ok(())
}

/// Where a callback was asked for, as timings name it: `brick_resize/mod.rs:236`. Made once per
/// callback, so kept for good.
fn caller_name(caller: &'static Location<'static>) -> &'static str {
    let file = caller.file().replace('\\', "/");
    let file = file
        .rsplit_once("features/")
        .map_or(&*file, |(_, rest)| rest);
    String::leak(format!("{file}:{}", caller.line()))
}

unsafe extern "C" fn tick_detour(editor: *mut UObjectBase, delta: f32) {
    timings::editor_frame();
    let original = *ORIGINAL_TICK.get().expect("hook recorded before use");
    // SAFETY: the original function with its own arguments.
    timings::game(TICK.name, || unsafe { original(editor, delta) });
    // Copied out, so that a callback may ask for more without a deadlock.
    let callbacks = AFTER_TICK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    for (callback, name) in callbacks {
        let _span = timings::span(name);
        callback(editor, delta);
    }
}

/// `void ABrickEditor::EndPlay(EEndPlayReason::Type)` (BRMK 0x9D8C0, game RVA 0xCD8F70, vtable slot
/// 104): the editor is going, with its vehicle still there. The player left it (it's destroyed), the
/// map changed or the game is closing (`UGameEngine::PreExit`). See `docs/re/editor-operations.md`.
pub const END_PLAY: FunctionSig = FunctionSig {
    name: "ABrickEditor::EndPlay",
    // `AActor::EndPlay`, the user settings, then saving the zoom ratio to them (setter inlined).
    pattern: "48 89 5c 24 10 57 48 81 ec ?? ?? 00 00 0f 29 74 24 ?? 48 8b f9 e8 ?? ?? ?? ?? e8 ?? ?? ?? ?? f3 0f 10 b7 ?? ?? 00 00 48 8d 15 ?? ?? ?? ?? 41 b8 01 00 00 00 48 8d 8c 24 ?? ?? 00 00 48 8b d8 e8 ?? ?? ?? ?? 0f 2e b3 ?? ?? 00 00 74",
};

type EndPlayFn = unsafe extern "C" fn(editor: *mut UObjectBase, reason: u8);

/// What runs as an editor goes (see [`before_end_play`]). Called on the game thread.
pub type BeforeEndPlay = fn(editor: *mut UObjectBase);

static ORIGINAL_END_PLAY: OnceLock<EndPlayFn> = OnceLock::new();
static BEFORE_END_PLAY: Mutex<Vec<BeforeEndPlay>> = Mutex::new(Vec::new());

/// Runs `callback` as an editor goes, before any of it is taken down: the last moment its vehicle
/// can be read. A crash or a killed process never gets here. The first call installs the hook.
pub fn before_end_play(callback: BeforeEndPlay) -> Result<(), String> {
    if ORIGINAL_END_PLAY.get().is_none() {
        let end_play = END_PLAY.resolve()?;
        // SAFETY: `end_play` is the start of ABrickEditor::EndPlay, and the detour has its
        // signature.
        unsafe {
            hook::install_typed(
                END_PLAY.name,
                end_play,
                end_play_detour as EndPlayFn,
                &ORIGINAL_END_PLAY,
            )?
        };
    }
    BEFORE_END_PLAY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(callback);
    Ok(())
}

unsafe extern "C" fn end_play_detour(editor: *mut UObjectBase, reason: u8) {
    let _span = timings::span("game::brick_editor::end_play_detour");
    // Copied out, so that a callback may ask for more without a deadlock.
    let callbacks = BEFORE_END_PLAY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    for callback in callbacks {
        callback(editor);
    }
    let original = *ORIGINAL_END_PLAY.get().expect("hook recorded before use");
    // SAFETY: the original function with its own arguments.
    timings::game(END_PLAY.name, || unsafe { original(editor, reason) });
}

/// The start of an operation, inlined in a `UBrickEditorMode` action (game RVA 0xD0EAF0; it takes
/// the editor from the mode, opens an operation, changes the selection and finishes it):
///
/// ```text
/// Operation.Flags = 0; Operation.Bits &= ~3; Operation.Editor = Editor;     // on the stack
/// if (Editor && !Editor->OpenOperation) Editor->OpenOperation = &Operation;  // +0x390 (BRMK +0x468)
/// Operation.Outermost = Editor ? Editor->OpenOperation : &Operation;
/// ```
pub const OPERATION_START: FunctionSig = FunctionSig {
    name: "ABrickEditor scoped operation start",
    // The stack offsets of the bits (+3), flags (+8) and editor (+17), then the editor's open
    // operation (its offset at +26).
    pattern: "80 64 24 ?? fc c7 44 24 ?? 00 00 00 00 48 89 5c 24 ?? 48 85 db 74 ?? 48 8b 83 ?? ?? 00 00 48 85 c0 75 ?? 48 8d 44 24 ?? 48 89 83",
};

/// The end of the same operation (BRMK `FinishScopedOperation`, 0xA10B0, inlined):
///
/// ```text
/// if (Operation.Editor && Operation.Editor->OpenOperation == &Operation) {
///     Editor->OpenOperation = nullptr;
///     Editor->PostModify(Operation);                                         // vtable +0x698
/// }
/// ```
pub const OPERATION_FINISH: FunctionSig = FunctionSig {
    name: "ABrickEditor scoped operation finish",
    // After the action's last call: the open operation compared (its offset at +23) and cleared
    // (+40), then PostModify called (its vtable offset at +50).
    pattern: "e8 ?? ?? ?? ?? 48 8b 4c 24 ?? 48 85 c9 74 ?? 48 8d 44 24 ?? 48 3b 81 ?? ?? 00 00 75 ?? 48 8b 01 48 8d 54 24 ?? 48 c7 81 ?? ?? 00 00 00 00 00 00 ff 90 ?? ?? 00 00 48 8b 5c 24",
};

/// `FBrickEditorScopedOperation`, as the game lays it out on the stack (0x18 bytes).
#[repr(C)]
pub struct Operation {
    editor: *mut UObjectBase,
    /// The open operation this one joined, or itself.
    outermost: *mut Operation,
    /// What changed, or-ed into the outermost operation; `PostModify` reads them bit by bit.
    flags: u32,
    /// Two flags the game clears when it starts an operation.
    bits: u8,
}

/// The flags the move mode sets when it commits a move (`UBrickEditorMoveMode::EndMode`, BRMK
/// 0x9CE60): bricks' places changed (0x1, 0x4 and 0x10; their meanings aren't mapped).
pub const MOVED: u32 = 0x1 | 0x4 | 0x10;

/// The flag the panel's write helpers set after a final edit of a brick property (e.g. the color
/// one, game 0xEB43D0: `1 << 5`, or `1 << 6` for `FObjectOrderBrickProperty`).
pub const PROPERTY_CHANGED: u32 = 0x20;

/// An operation's bit that makes `PostModify` take no undo step for it (and so its snapshot is
/// dropped), as undo and redo do themselves: `test byte [op+0x14], 1` (game 0xCF3CA2,
/// `docs/re/coop/change-hooks.md`, section 5).
/// Only coop uses it.
pub const NO_UNDO_STEP: u8 = 1;

/// An operation's bit that keeps `PostModify` from marking the vehicle modified (`test byte
/// [op+0x14], 2` at game 0xCF39DC). Loading a whole vehicle sets it (game 0xCEF44F, see
/// `game::vehicle_changes::keeping_selection`). Only coop uses it.
pub const UNMODIFIED: u8 = 2;

type PostModifyFn = unsafe extern "C" fn(editor: *mut UObjectBase, operation: *const Operation);

/// `PostModify` as hooked (or why not), see [`BrickEditor::after_modify`].
static POST_MODIFY_HOOK: OnceLock<Result<usize, String>> = OnceLock::new();
static ORIGINAL_POST_MODIFY: OnceLock<PostModifyFn> = OnceLock::new();
static AFTER_MODIFY: Mutex<Vec<fn(u32)>> = Mutex::new(Vec::new());

unsafe extern "C" fn post_modify_detour(editor: *mut UObjectBase, operation: *const Operation) {
    let original = *ORIGINAL_POST_MODIFY
        .get()
        .expect("hook recorded before use");
    // SAFETY: the game's operation, live for the call; its flags are what changed.
    let flags = unsafe { (*operation).flags };
    // SAFETY: the original function with its own arguments.
    unsafe { original(editor, operation) };
    // Copied out, so that a callback may ask for more without a deadlock.
    let callbacks = AFTER_MODIFY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    for callback in callbacks {
        callback(flags);
    }
}

/// The editor's operations and reflected parts (after engine init).
pub struct BrickEditor {
    /// Offset of the editor's open operation.
    open_operation: usize,
    /// Offset of `PostModify` in the editor's vtable.
    post_modify: usize,
    /// Offset of `ABrickEditor::PlayerController`.
    player_controller: usize,
}

// SAFETY: offsets living as long as the game; only used on the game thread.
unsafe impl Send for BrickEditor {}
unsafe impl Sync for BrickEditor {}

pub fn find() -> Result<BrickEditor, String> {
    let start = OPERATION_START.locate()?;
    let finish = OPERATION_FINISH.locate()?;
    if start.function != finish.function {
        return Err(format!(
            "{} and {} found in different functions",
            OPERATION_START.name, OPERATION_FINISH.name
        ));
    }
    let byte = |found: &SigMatch, offset| {
        // SAFETY: callers only read displacement bytes inside the matched patterns.
        unsafe { found.address.add(offset).read() }
    };
    let displacement = |found: &SigMatch, offset| {
        // SAFETY: as above.
        unsafe { found.address.add(offset).cast::<u32>().read_unaligned() as usize }
    };
    // The game's operation must be laid out like ours.
    let (bits, flags, editor) = (byte(&start, 3), byte(&start, 8), byte(&start, 17));
    if usize::from(flags.wrapping_sub(editor)) != core::mem::offset_of!(Operation, flags)
        || usize::from(bits.wrapping_sub(editor)) != core::mem::offset_of!(Operation, bits)
    {
        return Err("the editor's operations changed their layout".into());
    }
    let open_operation = displacement(&start, 26);
    if displacement(&finish, 23) != open_operation || displacement(&finish, 40) != open_operation {
        return Err(format!(
            "{} and {} use different fields",
            OPERATION_START.name, OPERATION_FINISH.name
        ));
    }
    let editor = reflection::find_class("BrickEditor").ok_or("class BrickEditor not found")?;
    Ok(BrickEditor {
        open_operation,
        post_modify: displacement(&finish, 50),
        player_controller: reflection::field_offset(
            editor,
            "PlayerController",
            "ObjectProperty",
            8,
        )?,
    })
}

impl BrickEditor {
    /// Runs `change` as one editor operation: one undo step, after which the editor handles what
    /// `flags` says changed (see [`MOVED`]). Inside an open operation, `change` joins it instead.
    ///
    /// # Safety
    /// `editor` must be a live `ABrickEditor`. Game thread only.
    pub unsafe fn operation(&self, editor: *mut UObjectBase, flags: u32, change: impl FnOnce()) {
        // SAFETY: per the contract.
        unsafe { self.operation_with(editor, flags, 0, change) }
    }

    /// [`Self::operation`] with `bits` (e.g. [`NO_UNDO_STEP`]) set on the outermost operation.
    ///
    /// # Safety
    /// As for [`Self::operation`].
    pub unsafe fn operation_with(
        &self,
        editor: *mut UObjectBase,
        flags: u32,
        bits: u8,
        change: impl FnOnce(),
    ) {
        let mut operation = Operation {
            editor,
            outermost: core::ptr::null_mut(),
            flags: 0,
            bits: 0,
        };
        let this: *mut Operation = &mut operation;
        // SAFETY: per the contract; the field and vtable offsets come from the game's own
        // operation code (see `find`), and the operation stays on this stack frame until the
        // editor lets go of it in `PostModify`.
        unsafe {
            let open = editor
                .cast::<u8>()
                .add(self.open_operation)
                .cast::<*mut Operation>();
            if open.read().is_null() {
                open.write(this);
            }
            (*this).outermost = open.read();
            (*(*this).outermost).flags |= flags;
            (*(*this).outermost).bits |= bits;
            change();
            if open.read() == this {
                open.write(core::ptr::null_mut());
                let vtable = (*editor).vtable.cast::<u8>();
                let post_modify = vtable.add(self.post_modify).cast::<PostModifyFn>().read();
                post_modify(editor, this);
            }
        }
    }

    /// Clears `bits` (e.g. [`UNMODIFIED`]) of `editor`'s open operation, if one is open.
    ///
    /// # Safety
    /// As for [`Self::operation`].
    pub unsafe fn clear_open_bits(&self, editor: *mut UObjectBase, bits: u8) {
        // SAFETY: per the contract; the open operation is the outermost one, live on the stack of
        // whoever opened it until `PostModify` (see `operation_with`).
        unsafe {
            let open = editor
                .cast::<u8>()
                .add(self.open_operation)
                .cast::<*mut Operation>()
                .read();
            if !open.is_null() {
                (*open).bits &= !bits;
            }
        }
    }

    /// Runs `callback` (given what the operation changed, see [`MOVED`]) after every operation of
    /// editors like `editor` ends (`PostModify`), from now on. The function is the one in `editor`'s vtable, hooked the first time; editors of
    /// another class (with another `PostModify`) later are an error.
    ///
    /// # Safety
    /// `editor` must be a live `ABrickEditor`. Game thread only.
    pub unsafe fn after_modify(
        &self,
        editor: *mut UObjectBase,
        callback: fn(u32),
    ) -> Result<(), String> {
        const NAME: &str = "ABrickEditor::PostModify";
        // SAFETY: per the contract; the slot's offset comes from the game's own code.
        let target = unsafe {
            (*editor)
                .vtable
                .cast::<u8>()
                .add(self.post_modify)
                .cast::<*const u8>()
                .read()
        };
        let hooked = POST_MODIFY_HOOK.get_or_init(|| {
            if !crate::framework::sig::is_function_start(target) {
                return Err(format!(
                    "{NAME}: the editor's vtable slot isn't a game function"
                ));
            }
            // SAFETY: `target` is the live editor's `PostModify`, and the detour has its signature.
            unsafe {
                hook::install_typed(
                    NAME,
                    target,
                    post_modify_detour as PostModifyFn,
                    &ORIGINAL_POST_MODIFY,
                )
            }
            .map(|()| target as usize)
        });
        match hooked {
            Ok(hooked) if *hooked == target as usize => {
                AFTER_MODIFY
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(callback);
                Ok(())
            }
            Ok(_) => Err(format!(
                "{NAME}: this editor has another one than the one hooked"
            )),
            Err(reason) => Err(reason.clone()),
        }
    }

    /// The editor's player controller (its view and mouse), or null.
    ///
    /// # Safety
    /// `editor` must be a live `ABrickEditor`.
    pub unsafe fn player_controller(&self, editor: *const UObjectBase) -> *mut UObjectBase {
        // SAFETY: per the contract; the offset comes from reflection.
        unsafe { read_object(editor, self.player_controller) }
    }
}

/// The object pointer at `offset` in `object`.
///
/// # Safety
/// `object` must point to an object with an object pointer at `offset`.
unsafe fn read_object(object: *const UObjectBase, offset: usize) -> *mut UObjectBase {
    // SAFETY: per the contract.
    unsafe {
        object
            .cast::<u8>()
            .add(offset)
            .cast::<*mut UObjectBase>()
            .read()
    }
}
