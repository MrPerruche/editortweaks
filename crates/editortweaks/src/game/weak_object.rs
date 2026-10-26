//! Holding on to a `UObject` across frames without keeping a dangling pointer, like Unreal's
//! `TWeakObjectPtr`: the object is alive while its slot in the global object array still holds it.
//! Also reading the game's own weak pointers ([`resolve`]).

use brickrust::ue::coreuobject::{GObjects, UObjectBase};
use brickrust::ue::tshared::FWeakObjectPtr;

/// `EInternalObjectFlags` of objects about to be destroyed: `PendingKill` and `Unreachable`.
const DYING: i32 = (1 << 29) | (1 << 28);

/// The object a game `FWeakObjectPtr` refers to, if it still exists, like `FWeakObjectPtr::Get`:
/// its slot in the global object array must still hold an object with the same serial number.
/// Game thread only.
pub fn resolve(pointer: FWeakObjectPtr) -> Option<*mut UObjectBase> {
    // SAFETY: the object array lives as long as the game, and every slot below its count is
    // allocated; only the slot is read.
    unsafe {
        let array = &GObjects().array;
        if pointer.object_serial_number == 0 || !(0..array.Count()).contains(&pointer.object_index)
        {
            return None;
        }
        let item = &*array.Get(pointer.object_index);
        let alive = !item.object.is_null()
            && item.serial == pointer.object_serial_number
            && item.flags & DYING == 0;
        alive.then_some(item.object.cast())
    }
}

/// A game `FWeakObjectPtr` to `object`, for game code taking one, if the object has a serial number
/// (given the first time the game makes a weak pointer to it; `None` if it never did).
///
/// # Safety
/// `object` must be a live object. Game thread only.
pub unsafe fn game_pointer(object: *mut UObjectBase) -> Option<FWeakObjectPtr> {
    // SAFETY: per the contract, so its index is a slot of the object array, which lives as long as
    // the game; only the slot is read.
    unsafe {
        let index = (*object).internal_index;
        let array = &GObjects().array;
        if !(0..array.Count()).contains(&index) {
            return None;
        }
        let item = &*array.Get(index);
        (item.object.cast() == object && item.serial != 0).then_some(FWeakObjectPtr {
            object_index: index,
            object_serial_number: item.serial,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WeakObject {
    object: *mut UObjectBase,
    index: i32,
}

impl WeakObject {
    /// # Safety
    /// `object` must be a live object.
    pub unsafe fn new(object: *mut UObjectBase) -> Self {
        Self {
            object,
            // SAFETY: per the contract.
            index: unsafe { (*object).internal_index },
        }
    }

    /// The object, unless it was destroyed since. Game thread only.
    pub fn get(&self) -> Option<*mut UObjectBase> {
        // SAFETY: the object array lives as long as the game, and every slot below its count is
        // allocated; only the slot is read, never the object itself.
        let alive = unsafe {
            let array = &GObjects().array;
            (0..array.Count()).contains(&self.index)
                && (*array.Get(self.index)).object == self.object
        };
        alive.then_some(self.object)
    }

    /// The object, unless it was destroyed since or is being destroyed (pending kill or
    /// unreachable, like an actor's components once it's destroyed). Game thread only.
    pub fn get_live(&self) -> Option<*mut UObjectBase> {
        // SAFETY: as in `get`; only the slot is read.
        let live = unsafe {
            let array = &GObjects().array;
            (0..array.Count()).contains(&self.index) && {
                let item = &*array.Get(self.index);
                item.object == self.object && item.flags & DYING == 0
            }
        };
        live.then_some(self.object)
    }

    /// Whether this refers to `object` (alive or not).
    pub fn is(&self, object: *const UObjectBase) -> bool {
        self.object.cast_const() == object
    }
}
