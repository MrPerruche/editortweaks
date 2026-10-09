//! Holding on to a `UObject` across frames without keeping a dangling pointer, like Unreal's
//! `TWeakObjectPtr`: the object is alive while its slot in the global object array still holds it.

use brickrust::ue::coreuobject::{GObjects, UObjectBase};

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

    /// Whether this refers to `object` (alive or not).
    pub fn is(&self, object: *const UObjectBase) -> bool {
        self.object.cast_const() == object
    }
}
