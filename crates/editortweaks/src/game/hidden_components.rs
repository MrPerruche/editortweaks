//! What the vehicle editor hides from the player's view: `ABrickPlayerController::
//! UpdateHiddenComponents(const FVector& ViewLocation, TSet<FPrimitiveComponentId>& Out)`, the
//! engine virtual (UE 4.27 `APlayerController::BuildHiddenComponentList` calls it for each view)
//! the game overrides (BRMK 0x2E3560, game 1.11.7 RVA 0xE48BF0, vtable +0xDC8). See
//! `docs/re/map-in-editor.md`.
//!
//! While the vehicle editor is open it lists every component of every actor but the editor and the
//! world setup actor as hidden, behind this test (game 0xE48D5F):
//!
//! ```text
//! if (Controller->BrickEditor && Controller->BrickEditor->EditorInterfaceComponent->GetUGCType() == 1)
//! ```
//!
//! [`install`] hooks the function. While [`show_map`] is on, the detour runs the game's function
//! with the controller's `BrickEditor` field null, so the test fails and nothing else is listed
//! (the thumbnails' hidden components, listed before it, stay): the map shows around the editor.
//! The field is put back right after, before anything else reads it. The detour needs no layout of
//! the hidden set (a `TSet`), which the game's function fills.

use core::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use brickrust::ue::coreuobject::UObjectBase;

use crate::framework::hook;
use crate::framework::sig::FunctionSig;
use crate::game::reflection;

/// The test above, found by the code around it (anywhere inside the function, see
/// `FunctionSig`): the controller's `BrickEditor` loaded and tested (its offset at +3), the editor's
/// interface component loaded and checked alive (its offset at +19), then `GetUGCType` called
/// through the vtable and compared with 1.
pub const UPDATE_HIDDEN_COMPONENTS: FunctionSig = FunctionSig {
    name: "ABrickPlayerController::UpdateHiddenComponents",
    pattern: "49 8b 9d ?? ?? 00 00 48 85 db 0f 84 ?? ?? ?? ?? 48 8b 9b ?? ?? 00 00 48 85 db 74 ?? e8 ?? ?? ?? ?? 48 8b 53 10 4c 8d 40 30 48 63 40 38 3b 42 38 7f ?? 48 8b c8 48 8b 42 30 4c 39 04 c8 74 ?? 33 db 48 8b 03 48 8b cb ff 90 ?? ?? 00 00 41 3a c7 0f 85",
};

type UpdateFn =
    unsafe extern "C" fn(controller: *mut UObjectBase, view: *const [f32; 3], hidden: *mut c_void);

static ORIGINAL: OnceLock<UpdateFn> = OnceLock::new();
/// Offset of `ABrickPlayerController::BrickEditor`, checked against the game's code.
static EDITOR_FIELD: OnceLock<usize> = OnceLock::new();
/// What to do with each view: see [`on_view`].
static ON_VIEW: OnceLock<fn(bool)> = OnceLock::new();
static SHOW_MAP: AtomicBool = AtomicBool::new(false);

/// Whether the map shows around the editor (see the module's documentation).
pub fn show_map() -> bool {
    SHOW_MAP.load(Ordering::Relaxed)
}

/// Shows (or hides again) the map in the vehicle editor's view, from the next view on.
pub fn set_show_map(show: bool) {
    SHOW_MAP.store(show, Ordering::Relaxed);
}

/// Calls `callback` at each view the controller makes (the game thread, before the game lists what
/// to hide), with whether its vehicle editor is open.
pub fn on_view(callback: fn(bool)) {
    let _ = ON_VIEW.set(callback);
}

/// Finds the function, checks the field it tests against reflection, and hooks it. After engine
/// init (the reflection check), though it only has to be before the first view of the editor.
pub fn install() -> Result<(), String> {
    let found = UPDATE_HIDDEN_COMPONENTS.locate()?;
    // SAFETY: the displacement lies inside the matched pattern.
    let code_offset = unsafe { found.address.add(3).cast::<u32>().read_unaligned() } as usize;
    let controller = reflection::find_class("BrickPlayerController")
        .ok_or("class BrickPlayerController not found")?;
    let field = reflection::field_offset(controller, "BrickEditor", "ObjectProperty", 8)?;
    if field != code_offset {
        return Err(format!(
            "the game's test reads BrickEditor at +0x{code_offset:X}, reflection says +0x{field:X}"
        ));
    }
    let _ = EDITOR_FIELD.set(field);
    // SAFETY: `function` is the start of UpdateHiddenComponents, which `update` matches.
    unsafe {
        hook::install_typed(
            UPDATE_HIDDEN_COMPONENTS.name,
            found.function,
            update as UpdateFn,
            &ORIGINAL,
        )
    }
}

unsafe extern "C" fn update(
    controller: *mut UObjectBase,
    view: *const [f32; 3],
    hidden: *mut c_void,
) {
    let original = *ORIGINAL.get().expect("hook recorded before use");
    let Some(&field) = EDITOR_FIELD.get() else {
        // SAFETY: the original function with its own arguments.
        return unsafe { original(controller, view, hidden) };
    };
    // SAFETY: the game calls this with a live controller, whose `BrickEditor` field is at `field`
    // (checked in `install`). The field is restored before the detour returns; the game thread is
    // the only one running the game's code that reads it, and nothing in between uses it but the
    // function we call.
    unsafe {
        let slot = controller
            .cast::<u8>()
            .add(field)
            .cast::<*mut UObjectBase>();
        let editor = slot.read();
        if let Some(callback) = ON_VIEW.get() {
            callback(!editor.is_null());
        }
        if show_map() && !editor.is_null() {
            slot.write(core::ptr::null_mut());
            original(controller, view, hidden);
            slot.write(editor);
        } else {
            original(controller, view, hidden);
        }
    }
}
