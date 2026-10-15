//! The view through a player controller, from its reflected functions (engine, UE 4.27): where the
//! mouse is, the ray under a point of the screen, where a point of the world shows on screen, and
//! the viewport's size. Screen positions are in viewport pixels, like the mouse's.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UFunction;

use crate::game::editor_object::Vector;
use crate::game::reflection::{self, Parameters};

/// A ray from the camera: its origin and unit direction.
#[derive(Clone, Copy)]
pub struct Ray {
    pub origin: Vector,
    pub direction: Vector,
}

/// `bool APlayerController::GetMousePosition(float& LocationX, float& LocationY) const`.
struct MousePosition {
    function: *const UFunction,
    x: usize,
    y: usize,
    result: usize,
}

/// `bool APlayerController::DeprojectScreenPositionToWorld(float ScreenX, float ScreenY, FVector&
/// WorldLocation, FVector& WorldDirection) const`.
struct Deproject {
    function: *const UFunction,
    x: usize,
    y: usize,
    location: usize,
    direction: usize,
    result: usize,
}

/// `bool APlayerController::ProjectWorldLocationToScreen(FVector WorldLocation, FVector2D&
/// ScreenLocation, bool bPlayerViewportRelative) const`.
struct Project {
    function: *const UFunction,
    location: usize,
    screen: usize,
    result: usize,
}

/// `void APlayerController::GetViewportSize(int32& SizeX, int32& SizeY) const`.
struct ViewportSize {
    function: *const UFunction,
    x: usize,
    y: usize,
}

/// A player controller's view functions (after engine init).
pub struct Viewport {
    mouse_position: MousePosition,
    deproject: Deproject,
    project: Project,
    size: ViewportSize,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for Viewport {}
unsafe impl Sync for Viewport {}

pub fn find() -> Result<Viewport, String> {
    let controller =
        reflection::find_class("PlayerController").ok_or("class PlayerController not found")?;
    let function = |name| {
        let function = reflection::find_function(controller, name)?;
        // SAFETY: a live function record.
        if unsafe { Parameters::fit(function) } {
            Ok(function)
        } else {
            Err(format!("{name}: unexpected parameters"))
        }
    };
    let parameter = reflection::parameter_offset;

    const MOUSE: &str = "GetMousePosition";
    let mouse = function(MOUSE)?;
    const DEPROJECT: &str = "DeprojectScreenPositionToWorld";
    let deproject = function(DEPROJECT)?;
    const PROJECT: &str = "ProjectWorldLocationToScreen";
    let project = function(PROJECT)?;
    const SIZE: &str = "GetViewportSize";
    let size = function(SIZE)?;
    Ok(Viewport {
        mouse_position: MousePosition {
            function: mouse,
            x: parameter(mouse, MOUSE, "LocationX", "FloatProperty", 4)?,
            y: parameter(mouse, MOUSE, "LocationY", "FloatProperty", 4)?,
            result: parameter(mouse, MOUSE, "ReturnValue", "BoolProperty", 1)?,
        },
        deproject: Deproject {
            function: deproject,
            x: parameter(deproject, DEPROJECT, "ScreenX", "FloatProperty", 4)?,
            y: parameter(deproject, DEPROJECT, "ScreenY", "FloatProperty", 4)?,
            location: parameter(deproject, DEPROJECT, "WorldLocation", "StructProperty", 12)?,
            direction: parameter(deproject, DEPROJECT, "WorldDirection", "StructProperty", 12)?,
            result: parameter(deproject, DEPROJECT, "ReturnValue", "BoolProperty", 1)?,
        },
        project: Project {
            function: project,
            location: parameter(project, PROJECT, "WorldLocation", "StructProperty", 12)?,
            screen: parameter(project, PROJECT, "ScreenLocation", "StructProperty", 8)?,
            result: parameter(project, PROJECT, "ReturnValue", "BoolProperty", 1)?,
        },
        size: ViewportSize {
            function: size,
            x: parameter(size, SIZE, "SizeX", "IntProperty", 4)?,
            y: parameter(size, SIZE, "SizeY", "IntProperty", 4)?,
        },
    })
}

impl Viewport {
    /// Where the mouse is on the screen, if it's over the game's viewport.
    ///
    /// # Safety
    /// `controller` must be a live `APlayerController`. Game thread only.
    pub unsafe fn mouse(&self, controller: *mut UObjectBase) -> Option<[f32; 2]> {
        let function = &self.mouse_position;
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; every field lies within the checked parameter size.
        unsafe {
            let base = parameters.as_mut_ptr();
            reflection::call(controller, function.function, base);
            if base.add(function.result).read() == 0 {
                return None;
            }
            Some([
                base.add(function.x).cast::<f32>().read(),
                base.add(function.y).cast::<f32>().read(),
            ])
        }
    }

    /// The viewport's width and height in pixels, if it has any.
    ///
    /// # Safety
    /// As for [`Self::mouse`].
    pub unsafe fn size(&self, controller: *mut UObjectBase) -> Option<[i32; 2]> {
        let function = &self.size;
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; every field lies within the checked parameter size.
        let size = unsafe {
            let base = parameters.as_mut_ptr();
            reflection::call(controller, function.function, base);
            [
                base.add(function.x).cast::<i32>().read_unaligned(),
                base.add(function.y).cast::<i32>().read_unaligned(),
            ]
        };
        (size[0] > 0 && size[1] > 0).then_some(size)
    }

    /// The ray from the camera through the screen position `screen`.
    ///
    /// # Safety
    /// As for [`Self::mouse`].
    pub unsafe fn ray(&self, controller: *mut UObjectBase, [x, y]: [f32; 2]) -> Option<Ray> {
        let function = &self.deproject;
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; every field lies within the checked parameter size.
        unsafe {
            let base = parameters.as_mut_ptr();
            base.add(function.x).cast::<f32>().write(x);
            base.add(function.y).cast::<f32>().write(y);
            reflection::call(controller, function.function, base);
            if base.add(function.result).read() == 0 {
                return None;
            }
            Some(Ray {
                origin: base
                    .add(function.location)
                    .cast::<Vector>()
                    .read_unaligned(),
                direction: base
                    .add(function.direction)
                    .cast::<Vector>()
                    .read_unaligned(),
            })
        }
    }

    /// Where `location` shows on the screen, if in front of the camera.
    ///
    /// # Safety
    /// As for [`Self::mouse`].
    pub unsafe fn project(
        &self,
        controller: *mut UObjectBase,
        location: Vector,
    ) -> Option<[f32; 2]> {
        let function = &self.project;
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; every field lies within the checked parameter size (the
        // viewport-relative flag stays false: positions like the mouse's).
        unsafe {
            let base = parameters.as_mut_ptr();
            base.add(function.location)
                .cast::<Vector>()
                .write_unaligned(location);
            reflection::call(controller, function.function, base);
            if base.add(function.result).read() == 0 {
                return None;
            }
            Some(
                base.add(function.screen)
                    .cast::<[f32; 2]>()
                    .read_unaligned(),
            )
        }
    }
}
