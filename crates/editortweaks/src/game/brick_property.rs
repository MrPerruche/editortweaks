//! Editing an object's brick properties through the editor (`IBrickPropertyInterface`, implemented
//! by every brick). For each edited object, the panel's write helpers (one per value type) first
//! check that the object may be modified ([`CAN_MODIFY`]), then write the value; afterwards the
//! object is told what changed (`PostModifyBrickProperty(const FBrickPropertyChangedEvent&)`,
//! virtual, e.g. `game::scalable_brick::POST_MODIFY`). Found through BRMK 1.11.6's exports, then
//! in game 1.11.7 (build 25831956); see `docs/re/property-panel.md`.

use core::ffi::c_void;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;
use brickrust::ue::fproperty::FProperty;
use brickrust::ue::tarray::TArray;
use brickrust::ue::tshared::FWeakObjectPtr;
use brickrust::ue::uclass::UStruct;

use crate::framework::sig::{self, FunctionSig};
use crate::game::numeric_property::Container;
use crate::game::reflection;

/// `bool FBrickPropertyEditInfo::CanModify(const FBrickPropertyContainer&)` (our name; BRMK
/// 0x3102A0, game RVA 0xE5C810): whether the edited property may be written to the container's
/// object. Called by every write helper right before writing each object (BRMK 0x389150 etc.),
/// and by the panel to show rows read-only:
///
/// ```text
/// Object = Cast<UBrickEditorObject>(Container.Root);
/// if (this->bReadOnly) return false;                    // +0xB1 (BRMK +0xB9)
/// if (!Object || !Object->GetBrickEditor()) return true;
/// return Editor->CanModifyBrickProperty(Object, AsShared());   // game 0xCD1BA0
/// ```
pub const CAN_MODIFY: FunctionSig = FunctionSig {
    name: "FBrickPropertyEditInfo::CanModify",
    // The read-only check, then the null checks before calling CanModifyBrickProperty.
    pattern: "38 9d ?? ?? 00 00 75 ?? 48 85 ff 74 ?? 48 85 f6 74 ?? 48 8b 4d ?? 41 bf 01 00 00 00",
};

pub type CanModifyFn =
    unsafe extern "C" fn(edit_info: *const c_void, container: *const Container) -> u8;

/// `PostModifyBrickProperty`'s signature. `interface` is the object's `IBrickPropertyInterface`
/// (see [`object_of`]).
pub type PostModifyFn = unsafe extern "C" fn(interface: *mut c_void, event: *mut ChangedEvent);

/// Slot of `_getUObject` in `IBrickPropertyInterface`'s vtable: [0] destructor, [1] `_getUObject`,
/// [2] `ReflectBrickProperties`, [3] `CanModifyBrickProperty`, [4] `PostModifyBrickProperty`,
/// [5] `UpdateFocusedBrickProperty` (same order in BRMK and game).
const GET_UOBJECT_SLOT: usize = 1;

/// Slot of `PostModifyBrickProperty`, see [`GET_UOBJECT_SLOT`].
const POST_MODIFY_SLOT: usize = 4;

/// The object behind an `IBrickPropertyInterface` pointer (in the game, 0x28 bytes into it).
///
/// # Safety
/// `interface` must point to a live object's `IBrickPropertyInterface`.
pub unsafe fn object_of(interface: *mut c_void) -> *mut UObjectBase {
    // SAFETY: per the contract; the slot holds `_getUObject` (see GET_UOBJECT_SLOT), which only
    // adjusts the pointer.
    unsafe {
        let vtable = *interface.cast::<*const usize>();
        let get: unsafe extern "C" fn(*mut c_void) -> *mut UObjectBase =
            core::mem::transmute(*vtable.add(GET_UOBJECT_SLOT));
        get(interface)
    }
}

/// Tells the object behind `interface` that a property changed, as the editor does after writing
/// one (the object's own `PostModifyBrickProperty`, including its class's handling).
///
/// # Safety
/// `interface` must point to a live object's `IBrickPropertyInterface`, and `event` describe a
/// property of it. Game thread only.
pub unsafe fn post_modify(interface: *mut c_void, event: &mut ChangedEvent) {
    // SAFETY: per the contract; the slot holds `PostModifyBrickProperty` (see GET_UOBJECT_SLOT).
    unsafe {
        let vtable = *interface.cast::<*const usize>();
        let post: PostModifyFn = core::mem::transmute(*vtable.add(POST_MODIFY_SLOT));
        post(interface, event);
    }
}

/// `EPropertyFlags`: `CPF_Edit` marks the properties the panel may list (bricks' are all
/// `EditDefaultsOnly` in C++: this and `CPF_DisableEditOnInstance`), `CPF_EditConst` the read-only
/// ones and `CPF_Transient` state the game doesn't save. Read from BRMK 1.11.6's generated property
/// records, see `docs/re/editor-operations.md`.
const EDIT: u64 = 0x1;
const EDIT_CONST: u64 = 0x2_0000;
const TRANSIENT: u64 = 0x2000;

/// Editable properties of brick classes that aren't brick properties: they're the same for every
/// brick of a class. `bEnableBlueprintEvents`: whether a brick Blueprint's events run.
const NOT_BRICK_PROPERTIES: [&str; 1] = ["bEnableBlueprintEvents"];

/// Whether `property`, of a brick's class, is a brick property: declared editable and saved
/// ([`EDIT`]) by a native class from `brick` (`UBrick`) on, but [`NOT_BRICK_PROPERTIES`]. They're
/// those the panel lists (checked against BRMK 1.11.6's brick classes), of any type.
///
/// # Safety
/// `property` must be a live property record, and `brick` the class `Brick`.
pub unsafe fn is_brick_property(property: *const FProperty, brick: *const UStruct) -> bool {
    // SAFETY: per the contract; a property's owner lives as long as it does.
    unsafe {
        let flags = (*property).property_flags;
        let owner = reflection::declaring_struct(property);
        let name = &(*property).field.name_private;
        let brick_class = !owner.is_null()
            && reflection::is_native_class(owner)
            && reflection::with_parents(owner).any(|parent| parent == brick);
        brick_class
            && flags & EDIT != 0
            && flags & (EDIT_CONST | TRANSIENT) == 0
            && !NOT_BRICK_PROPERTIES
                .iter()
                .any(|excluded| reflection::name_is(name, excluded))
    }
}

/// Where objects of `class` keep their `IBrickPropertyInterface` (see [`object_of`]): its offset in
/// them, from reflection, checked on `object` (one of them) to lead back to it.
///
/// # Safety
/// `object` must be a live object of `class`. Game thread only.
pub unsafe fn interface_offset(class: *const UStruct, object: *mut UObjectBase) -> Option<usize> {
    // SAFETY: per the contract; the vtable pointer is checked to lead to game code in the
    // `_getUObject` slot before it's called (a leaf function, `lea rax, [rcx - offset]; ret`, so
    // not in the exe's function table).
    unsafe {
        let offset = reflection::interface_offset(class, "BrickPropertyInterface")?;
        let interface = object.cast::<u8>().add(offset).cast::<c_void>();
        let vtable = *interface.cast::<*const *const u8>();
        if vtable.is_null() || !sig::is_code(*vtable.add(GET_UOBJECT_SLOT)) {
            return None;
        }
        (object_of(interface) == object).then_some(offset)
    }
}

/// `FBrickPropertyChangedEvent`, as far as known: the names of the properties leading to the
/// changed value, and which of them is this object's. An empty path means everything changed. The
/// game's `FName` is 8 bytes (BRMK's 12).
///
/// ```text
/// bool IsPropertyChanged(Event, Name) {   // inlined, e.g. in UScalableBrick::PostModifyBrickProperty
///     if (Event.Level in 0..Event.Path.Num()) return Event.Path[Event.Level] == Name;
///     return Event.Path.Num() == 0;
/// }
/// ```
///
/// Then (BRMK 1.11.6, same in the game as far as used): the edited objects (read by
/// `ABrickEditor::OnAnyPropertyModified`), the object the edit started from
/// (`UBrickEditorObject::IsPostModifyBrickPropertyMirrored`: a mirrored copy's source) and how it
/// changed, [`INTERACTIVE`] while a slider is dragged, when bricks skip their slow updates (e.g. a
/// scalable brick's connectors and aerodynamics).
#[repr(C)]
pub struct ChangedEvent {
    _unknown: [u8; 0x18],
    path: TArray<FName>,
    level: i32,
    objects: TArray<FWeakObjectPtr>,
    source: FWeakObjectPtr,
    kind: u8,
}

/// [`ChangedEvent`]'s kind while a value is being dragged; others are final.
const INTERACTIVE: u8 = 2;
/// The kind of a final change made here (anything but [`INTERACTIVE`] is handled alike).
const FINAL: u8 = 0;

impl ChangedEvent {
    /// Runs `f` with an event telling that `property` of an object changed, `interactive`ly (while
    /// dragging: more changes follow) or not. The event names no objects and no source: an edit
    /// started on the object itself.
    pub fn with_property(property: FName, interactive: bool, f: impl FnOnce(&mut Self)) {
        Self::with(property, &mut [], interactive, f);
    }

    /// Runs `f` with an event telling that `property` of `object` changed for good, naming it as
    /// the edited object, like the panel's events to the editor (`OnAnyPropertyModified`).
    pub fn with_object(property: FName, object: FWeakObjectPtr, f: impl FnOnce(&mut Self)) {
        Self::with(property, &mut [object], false, f);
    }

    fn with(
        property: FName,
        objects: &mut [FWeakObjectPtr],
        interactive: bool,
        f: impl FnOnce(&mut Self),
    ) {
        /// The event, with room for fields beyond those known, zeroed like the known unused ones.
        #[repr(C)]
        struct Padded {
            event: ChangedEvent,
            _rest: [u8; 0x40],
        }
        let mut path = [property];
        let count = objects.len() as i32;
        let mut padded = Padded {
            event: ChangedEvent {
                _unknown: [0; 0x18],
                path: TArray {
                    data: path.as_mut_ptr(),
                    num: 1,
                    max: 1,
                },
                level: 0,
                objects: TArray {
                    data: if objects.is_empty() {
                        core::ptr::null_mut()
                    } else {
                        objects.as_mut_ptr()
                    },
                    num: count,
                    max: count,
                },
                source: FWeakObjectPtr {
                    object_index: 0,
                    object_serial_number: 0,
                },
                kind: if interactive { INTERACTIVE } else { FINAL },
            },
            _rest: [0; 0x40],
        };
        f(&mut padded.event);
    }

    /// The changed property, or `None` if everything changed.
    pub fn property(&self) -> Option<FName> {
        let level = usize::try_from(self.level).ok()?;
        let len = usize::try_from(self.path.num).ok()?;
        if level >= len || self.path.data.is_null() {
            return None;
        }
        // SAFETY: the path holds `num` names, and `level` is one of them.
        Some(unsafe { *self.path.data.add(level) })
    }

    /// Runs `f` with the event naming `property` as the changed one instead, then restores it.
    /// Does nothing if the event doesn't name a single property.
    pub fn renamed(&mut self, property: FName, f: impl FnOnce(&mut Self)) {
        if self.property().is_none() {
            return;
        }
        // SAFETY: `property` checked that `level` is within the path.
        let slot = unsafe { self.path.data.add(self.level as usize) };
        // SAFETY: as above; the slot stays valid during `f`, which gets the event, not the slot.
        let original = unsafe { slot.replace(property) };
        f(self);
        // SAFETY: as above.
        unsafe { slot.write(original) };
    }
}
