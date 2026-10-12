//! The editor's static settings (`UBrickEditorStaticInfo`): a Blueprint class whose default object
//! holds the editor's fixed parameters, here `EditorBounds`. Found through BRMK 1.11.6's exports;
//! see `docs/re/editor-bounds.md`.
//!
//! `ABrickEditor::GetStaticInfo` returns the default object of the editor's `StaticInfoClass` (a
//! Blueprint subclass) on every call, so changing that default object changes the editor at once.

use brickrust::ue::coreuobject::{EObjectFlags, GObjects, UObjectBase};
use brickrust::ue::uclass::UClass;

use crate::game::reflection;

/// `FVector EditorBounds`: half the size of the box (centred on the vehicle's origin) that the
/// editor keeps bricks and its camera in, in cm along each axis.
pub struct BoundsField {
    class: *const UClass,
    /// Offset of `EditorBounds` in the static info objects.
    offset: usize,
}

// SAFETY: a reflection record living as long as the game; only used on the game thread.
unsafe impl Send for BoundsField {}
unsafe impl Sync for BoundsField {}

/// Finds `BrickEditorStaticInfo.EditorBounds`. After engine init.
pub fn find() -> Result<BoundsField, String> {
    let class = reflection::find_class("BrickEditorStaticInfo")
        .ok_or("class BrickEditorStaticInfo not found")?;
    let property = reflection::field(class, "EditorBounds", "StructProperty", 12)?;
    // SAFETY: `field` only returns live property records; a StructProperty's struct lives as long.
    unsafe {
        let vector = reflection::struct_type(property);
        if vector.is_null() || !reflection::name_is(reflection::struct_name(vector), "Vector") {
            return Err("EditorBounds isn't an FVector".into());
        }
        Ok(BoundsField {
            class,
            offset: (*property).offset_internal as usize,
        })
    }
}

impl BoundsField {
    /// Every loaded default object of `BrickEditorStaticInfo` and its (Blueprint) subclasses.
    /// Walks every live object. Game thread only.
    pub fn defaults(&self) -> Vec<*mut UObjectBase> {
        // SAFETY: on the game thread, GObjects slots hold live objects or null, and every object
        // has a valid class.
        unsafe {
            GObjects()
                .iter()
                .filter(|&object| {
                    !object.is_null()
                        && (*object)
                            .object_flags
                            .contains(EObjectFlags::RF_ClassDefaultObject)
                        && reflection::with_parents(reflection::class_of(object.cast()))
                            .any(|class| class == self.class.cast())
                })
                .collect()
        }
    }

    /// The bounds of `info`.
    ///
    /// # Safety
    /// `info` must be a live `BrickEditorStaticInfo` (one of [`Self::defaults`]). Game thread only.
    pub unsafe fn get(&self, info: *const UObjectBase) -> [f32; 3] {
        // SAFETY: per the contract; `offset` is the place of its FVector `EditorBounds`.
        unsafe {
            info.byte_add(self.offset)
                .cast::<[f32; 3]>()
                .read_unaligned()
        }
    }

    /// Sets the bounds of `info`.
    ///
    /// # Safety
    /// As [`Self::get`].
    pub unsafe fn set(&self, info: *mut UObjectBase, bounds: [f32; 3]) {
        // SAFETY: as in `get`.
        unsafe {
            info.byte_add(self.offset)
                .cast::<[f32; 3]>()
                .write_unaligned(bounds)
        }
    }
}
