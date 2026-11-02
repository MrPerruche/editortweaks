//! Where the vehicle editor's objects come and go, for coop to pass on. Found statically in game 1.11.7 (build 25831956)
//! against BRMK 1.11.6 (`docs/re/coop/change-hooks.md`, sections 1, 2 and 5):
//!
//! - every new object passes `virtual void AVehicleEditor::OnBrickEditorObjectsCreated(const
//!   TArray<UBrickEditorObject*>&, EBrickEditorObjectCreationContext)` (BRMK 0xC18D0, game RVA
//!   0xCEF690): new bricks, duplicates, mirrors, loads, imports and undo steps, mirror
//!   counterparts included, all through one of three calls;
//! - every deleted one `UBrickEditorInterfaceComponent::DestroyBrickEditorObjects`
//!   ([`vehicle_archive::DESTROY_OBJECTS`]): deleting, undo and loads.

use std::cell::Cell;
use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::tarray::TArray;

use crate::framework::hook;
use crate::framework::sig::{self, FunctionSig};
use crate::framework::timings;
use crate::game::vehicle_archive::{self, DestroyObjectsFn};

/// The call of `OnBrickEditorObjectsCreated` for a new brick (in
/// `UBrickEditorInterfaceComponent::CreateSingleBrickEditorObject`, game RVA 0xCD4630).
pub const CREATED_CALL: FunctionSig = FunctionSig {
    name: "AVehicleEditor::OnBrickEditorObjectsCreated call",
    // Context 5 (a new brick), then the virtual call: its vtable offset at +8.
    pattern: "41 b0 05 48 8b cb ff 90 ?? ?? 00 00",
};

/// Why objects were created (`EBrickEditorObjectCreationContext`): 0 a whole vehicle loaded
/// (opened, or an undo or redo step), 1 loaded into it (imported), 2 duplicated, 3 duplicated
/// mirrored, 4 mirrored, 5 a new brick.
pub mod context {
    pub const VEHICLE: u8 = 0;
    pub const IMPORT: u8 = 1;
    pub const DUPLICATE: u8 = 2;
    pub const NEW_BRICK: u8 = 5;
}

/// Whether objects created for `context` are still being placed: the editor moves them with the
/// mouse until a click puts them down (a move commit, `brick_editor::MOVED`) or Escape deletes
/// them again.
pub fn is_placing(context: u8) -> bool {
    matches!(context, context::DUPLICATE | context::NEW_BRICK)
}

/// After objects were created: the editor, them, and why (see [`context`]).
pub type Created = fn(editor: *mut UObjectBase, objects: &[*mut UObjectBase], context: u8);
/// Before objects are destroyed: their component, and them (still live).
pub type Destroyed = fn(component: *mut UObjectBase, objects: &[*mut UObjectBase]);

type CreatedFn = unsafe extern "C" fn(
    editor: *mut UObjectBase,
    objects: *const TArray<*mut UObjectBase>,
    context: u8,
);

static CREATED_OFFSET: OnceLock<Result<usize, String>> = OnceLock::new();
static CREATED_HOOK: OnceLock<Result<usize, String>> = OnceLock::new();
static ORIGINAL_CREATED: OnceLock<CreatedFn> = OnceLock::new();
static ON_CREATED: OnceLock<Created> = OnceLock::new();

static ORIGINAL_DESTROY: OnceLock<DestroyObjectsFn> = OnceLock::new();
static ON_DESTROYED: OnceLock<Destroyed> = OnceLock::new();

thread_local! {
    // The game thread's, see `keeping_selection`.
    static KEEPING_SELECTION: Cell<bool> = const { Cell::new(false) };
}

/// Runs `load` with what it imports left unselected. The game selects imported objects in place
/// of the selection (`ABrickEditor::OnBrickEditorObjectsCreated`, game 0xCEF495: `SelectObjects`
/// mode 4 for every context but 0 and 3), but not a whole vehicle's: for those it only sets
/// [`crate::game::brick_editor::UNMODIFIED`] on the operation (0xCEF44F), and the code after doesn't look at
/// the context. So imports are passed on to the game as a whole vehicle's objects meanwhile; the
/// caller decides about that flag. Hooked editors only (see [`on_created`]).
pub fn keeping_selection<R>(load: impl FnOnce() -> R) -> R {
    let was = KEEPING_SELECTION.replace(true);
    let result = load();
    KEEPING_SELECTION.set(was);
    result
}

/// The objects of a game array.
///
/// # Safety
/// `array` must be a live `TArray` of object pointers.
unsafe fn objects_of(array: *const TArray<*mut UObjectBase>) -> Vec<*mut UObjectBase> {
    // SAFETY: per the contract.
    unsafe {
        let array = &*array;
        match usize::try_from(array.num) {
            Ok(len) if len > 0 && !array.data.is_null() => {
                core::slice::from_raw_parts(array.data, len).to_vec()
            }
            _ => Vec::new(),
        }
    }
}

unsafe extern "C" fn created_detour(
    editor: *mut UObjectBase,
    objects: *const TArray<*mut UObjectBase>,
    context: u8,
) {
    let _span = timings::span("game::vehicle_changes::created_detour");
    let original = *ORIGINAL_CREATED.get().expect("hook recorded before use");
    // SAFETY: the game's array of new objects; copied first, as the editor may change it.
    let created = unsafe { objects_of(objects) };
    let passed = if context == context::IMPORT && KEEPING_SELECTION.get() {
        context::VEHICLE
    } else {
        context
    };
    // SAFETY: the original function with its own arguments (the context one it knows).
    unsafe {
        timings::game("AVehicleEditor::OnBrickEditorObjectsCreated", || {
            original(editor, objects, passed)
        })
    };
    if let Some(callback) = ON_CREATED.get() {
        callback(editor, &created, context);
    }
}

unsafe extern "C" fn destroy_detour(
    component: *mut UObjectBase,
    objects: *const TArray<*mut UObjectBase>,
    all: bool,
) -> bool {
    let _span = timings::span("game::vehicle_changes::destroy_detour");
    let original = *ORIGINAL_DESTROY.get().expect("hook recorded before use");
    // SAFETY: the game's array; copied, as it may be the component's own array, which emptying
    // the vehicle changes.
    let doomed = unsafe { objects_of(objects) };
    if let Some(callback) = ON_DESTROYED.get()
        && !doomed.is_empty()
    {
        callback(component, &doomed);
    }
    // SAFETY: the original function with its own arguments.
    unsafe {
        timings::game(vehicle_archive::DESTROY_OBJECTS.name, || {
            original(component, objects, all)
        })
    }
}

/// Calls `callback` before any editor objects are destroyed, from now on. Only the first
/// callback counts.
pub fn on_destroyed(callback: Destroyed) -> Result<(), String> {
    if ON_DESTROYED.set(callback).is_err() {
        return Ok(());
    }
    let target = vehicle_archive::destroy_objects()?;
    // SAFETY: `target` is the start of DestroyBrickEditorObjects, and the detour has its
    // signature.
    unsafe {
        hook::install_typed(
            vehicle_archive::DESTROY_OBJECTS.name,
            target,
            destroy_detour as DestroyObjectsFn,
            &ORIGINAL_DESTROY,
        )
    }
}

/// Calls `callback` after objects are created in editors like `editor`, from now on. The function
/// is the one in `editor`'s vtable, hooked the first time; editors with another one later are an
/// error. Only the first callback counts.
///
/// # Safety
/// `editor` must be a live `AVehicleEditor`. Game thread only.
pub unsafe fn on_created(editor: *mut UObjectBase, callback: Created) -> Result<(), String> {
    const NAME: &str = "AVehicleEditor::OnBrickEditorObjectsCreated";
    let offset = CREATED_OFFSET
        .get_or_init(|| {
            let found = CREATED_CALL.locate()?;
            // SAFETY: the displacement lies within the matched pattern.
            Ok(unsafe { found.address.add(8).cast::<u32>().read_unaligned() } as usize)
        })
        .clone()?;
    // SAFETY: per the contract; the slot's offset comes from the game's own call.
    let target = unsafe {
        (*editor)
            .vtable
            .cast::<u8>()
            .add(offset)
            .cast::<*const u8>()
            .read()
    };
    let hooked = CREATED_HOOK.get_or_init(|| {
        if !sig::is_function_start(target) {
            return Err(format!(
                "{NAME}: the editor's vtable slot isn't a game function"
            ));
        }
        // SAFETY: `target` is the live editor's function, and the detour has its signature.
        unsafe { hook::install_typed(NAME, target, created_detour as CreatedFn, &ORIGINAL_CREATED) }
            .map(|()| target as usize)
    });
    match hooked {
        Ok(hooked) if *hooked == target as usize => {
            let _ = ON_CREATED.set(callback);
            Ok(())
        }
        Ok(_) => Err(format!(
            "{NAME}: this editor has another one than the one hooked"
        )),
        Err(reason) => Err(reason.clone()),
    }
}
