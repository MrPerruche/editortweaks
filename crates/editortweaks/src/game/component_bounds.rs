//! Where a component is in the world, roughly: the box around it (`static void
//! UKismetSystemLibrary::GetComponentBounds(const USceneComponent* Component, FVector& Origin,
//! FVector& BoxExtent, float& SphereRadius)`, engine, called through reflection).

use brickrust::ue::coreuobject::UObjectBase;

use crate::game::editor_object::Vector;
use crate::game::reflection::{self, Function, Parameters, put, take};

pub struct ComponentBounds {
    /// `UKismetSystemLibrary`'s default object, which static functions are called on.
    library: *mut UObjectBase,
    function: Function,
    component: usize,
    origin: usize,
    extent: usize,
}

// SAFETY: a reflection record and a default object living as long as the game; only used on the
// game thread.
unsafe impl Send for ComponentBounds {}
unsafe impl Sync for ComponentBounds {}

/// Finds `GetComponentBounds`. After engine init.
pub fn find() -> Result<ComponentBounds, String> {
    const LIBRARY: &str = "KismetSystemLibrary";
    let class = reflection::find_class(LIBRARY).ok_or("class KismetSystemLibrary not found")?;
    let function = Function::find(class, "GetComponentBounds")?;
    Ok(ComponentBounds {
        library: reflection::find_object("Default__KismetSystemLibrary", LIBRARY)
            .ok_or("KismetSystemLibrary's default object not found")?,
        component: function.parameter("Component", "ObjectProperty", 8)?,
        origin: function.parameter("Origin", "StructProperty", 12)?,
        extent: function.parameter("BoxExtent", "StructProperty", 12)?,
        function,
    })
}

impl ComponentBounds {
    /// The box around `component` in the world, `[min, max]`.
    ///
    /// # Safety
    /// `component` must be a live scene component. Game thread only.
    pub unsafe fn of(&self, component: *mut UObjectBase) -> [Vector; 2] {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; each value goes to its parameter's reflected offset.
        unsafe {
            put(&mut parameters, self.component, component);
            self.function.call(self.library, &mut parameters);
            let origin: Vector = take(&parameters, self.origin);
            let extent: Vector = take(&parameters, self.extent);
            [
                core::array::from_fn(|axis| origin[axis] - extent[axis]),
                core::array::from_fn(|axis| origin[axis] + extent[axis]),
            ]
        }
    }
}
