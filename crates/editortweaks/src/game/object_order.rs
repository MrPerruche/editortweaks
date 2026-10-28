//! Reordering the vehicle editor's objects as its panel does for seat and camera order
//! (`FObjectOrderBrickProperty`): coop applies someone else's reorder this way
//! (`docs/re/coop/change-hooks.md`, section 5); only in builds with the `coop` feature.
//!
//! `bool UBrickEditorInterfaceComponent::ReorderBrickEditorObject(UBrickEditorObject* Object,
//! const UClass* Class, int32 NewIndex)` (BRMK 0xD7AC0, game 1.11.7 RVA 0xCFFFC0) takes the objects
//! that are a `Class` in array order, moves `Object` to `NewIndex` among them (out, then back in at
//! that index), writes them back into their slots and renumbers their `FBrickEditorObjectID`s
//! (array position + 1). It checks both indices and returns false if one is out of range. It opens
//! no operation and sets no flag: its callers add [`ORDER_CHANGED`]. It isn't reflected (the exe
//! has no `ReorderBrickEditorObject` name), so it's found by its code. Passing `BrickEditorObject`
//! as the class makes the objects every object of the editor, in any order.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UClass;

use crate::framework::sig::FunctionSig;
use crate::game::reflection;

/// The class test in `ReorderBrickEditorObject`'s loop over the object array (game 0xD00030): the
/// array (its offset at +3), the class' depth and the object's class.
pub const REORDER: FunctionSig = FunctionSig {
    name: "UBrickEditorInterfaceComponent::ReorderBrickEditorObject",
    pattern: "49 8b 86 ?? ?? 00 00 49 63 48 08 4a 8b 14 28 48 89 54 24 ?? 48 8b 42 10",
};

/// The flag the panel sets after reordering (`1 << 6`, see `brick_editor::PROPERTY_CHANGED`).
pub const ORDER_CHANGED: u32 = 0x40;

type ReorderFn = unsafe extern "C" fn(
    component: *mut UObjectBase,
    object: *mut UObjectBase,
    class: *const UClass,
    index: i32,
) -> bool;

/// The game's reorder, and the class every editor object is.
pub struct ObjectOrder {
    reorder: ReorderFn,
    /// `BrickEditorObject`.
    every_object: *const UClass,
}

// SAFETY: a function and a class record living as long as the game; only used on the game thread.
unsafe impl Send for ObjectOrder {}
unsafe impl Sync for ObjectOrder {}

/// Finds the reorder, checking it reads the object array where reflection says it is.
pub fn find() -> Result<ObjectOrder, String> {
    let interface = reflection::find_class("BrickEditorInterfaceComponent")
        .ok_or("class BrickEditorInterfaceComponent not found")?;
    let every_object =
        reflection::find_class("BrickEditorObject").ok_or("class BrickEditorObject not found")?;
    let objects = reflection::field_offset(interface, "BrickEditorObjects", "ArrayProperty", 0x10)?;
    let found = REORDER.locate()?;
    // SAFETY: the pattern matched, so its bytes are there; the offset is a 32-bit displacement.
    let read = unsafe { found.address.add(3).cast::<u32>().read_unaligned() } as usize;
    if read != objects {
        return Err(format!(
            "{} reads the objects at {read:#x}, reflection says {objects:#x}",
            REORDER.name
        ));
    }
    Ok(ObjectOrder {
        // SAFETY: the function the pattern is in, with the signature above (BRMK's export).
        reorder: unsafe { core::mem::transmute::<*const u8, ReorderFn>(found.function) },
        every_object,
    })
}

impl ObjectOrder {
    /// Moves `object` to `index` in `component`'s object array, as the panel does; false if the
    /// game refused (an index out of range). The caller runs it in an editor operation with
    /// [`ORDER_CHANGED`].
    ///
    /// # Safety
    /// `component` must be a live `UBrickEditorInterfaceComponent` whose object array holds no
    /// null entries (the game reads each one's class), `object` one of them. Game thread only.
    pub unsafe fn move_to(
        &self,
        component: *mut UObjectBase,
        object: *mut UObjectBase,
        index: usize,
    ) -> bool {
        let Ok(index) = i32::try_from(index) else {
            return false;
        };
        // SAFETY: per the contract; the function checks the indices itself.
        unsafe { (self.reorder)(component, object, self.every_object, index) }
    }
}
