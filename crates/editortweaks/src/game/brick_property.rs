//! Editing an object's brick properties through the editor (`IBrickPropertyInterface`, implemented
//! by every brick). For each edited object, the panel's write helpers (one per value type) first
//! check that the object may be modified ([`CAN_MODIFY`]), then write the value; afterwards the
//! object is told what changed (`PostModifyBrickProperty(const FBrickPropertyChangedEvent&)`,
//! virtual, e.g. `game::scalable_brick::POST_MODIFY`). Found through BRMK 1.11.6's exports, then
//! in game 1.11.7 (build 25831956); see `docs/re/property-panel.md`.

use core::ffi::c_void;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;
use brickrust::ue::tarray::TArray;

use crate::framework::sig::FunctionSig;
use crate::game::numeric_property::Container;

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
#[repr(C)]
pub struct ChangedEvent {
    _unknown: [u8; 0x18],
    path: TArray<FName>,
    level: i32,
}

impl ChangedEvent {
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
