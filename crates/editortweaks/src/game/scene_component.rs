//! A scene component's place and scale (`USceneComponent`, engine, reflected): its relative scale,
//! `SetRelativeScale3D` and its world location.

use brickrust::ue::coreuobject::UObjectBase;

use crate::game::editor_object::Vector;
use crate::game::reflection::{self, Getter, SingleCall};

pub struct SceneComponent {
    /// Offset of `RelativeScale3D`.
    relative_scale: usize,
    /// `void SetRelativeScale3D(FVector NewScale3D)`.
    set_relative_scale: SingleCall,
    /// `FVector K2_GetComponentLocation() const`.
    location: Getter<Vector>,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for SceneComponent {}
unsafe impl Sync for SceneComponent {}

/// Finds the scene component's fields and functions. After engine init.
pub fn find() -> Result<SceneComponent, String> {
    let class = reflection::find_class("SceneComponent").ok_or("class SceneComponent not found")?;
    Ok(SceneComponent {
        relative_scale: reflection::field_offset(class, "RelativeScale3D", "StructProperty", 12)?,
        set_relative_scale: SingleCall::find(
            class,
            "SetRelativeScale3D",
            "NewScale3D",
            "StructProperty",
            12,
        )?,
        location: Getter::find(class, "K2_GetComponentLocation", "StructProperty")?,
    })
}

impl SceneComponent {
    /// The component's scale relative to its parent.
    ///
    /// # Safety
    /// `component` must be a live `USceneComponent`. Game thread only.
    pub unsafe fn scale(&self, component: *const UObjectBase) -> Vector {
        // SAFETY: per the contract; the offset comes from reflection.
        unsafe {
            component
                .cast::<u8>()
                .add(self.relative_scale)
                .cast::<Vector>()
                .read_unaligned()
        }
    }

    /// Sets the component's scale relative to its parent.
    ///
    /// # Safety
    /// As for [`Self::scale`].
    pub unsafe fn set_scale(&self, component: *mut UObjectBase, scale: Vector) {
        // SAFETY: per the contract; the parameter is an FVector.
        unsafe { self.set_relative_scale.call(component, scale) }
    }

    /// Where the component is in the world.
    ///
    /// # Safety
    /// As for [`Self::scale`].
    pub unsafe fn location(&self, component: *mut UObjectBase) -> Vector {
        // SAFETY: per the contract; the function returns an FVector.
        unsafe { self.location.call(component) }
    }
}
