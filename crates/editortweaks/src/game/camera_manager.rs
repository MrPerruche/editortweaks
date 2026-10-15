//! The player's camera manager (`ABrickCameraManager`): each frame it asks the view target (the
//! editor, a vehicle, the free camera, ...) for its view (`CalcCamera`), then applies its effects to
//! it (`ApplyCameraModifiers`), the view it fills in being an `FMinimalViewInfo`. Found through BRMK
//! 1.11.6's exports, then in game 1.11.7 (build 25831956); see `docs/re/editor-camera.md`.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UStruct;

use crate::framework::sig::FunctionSig;
use crate::game::editor_object::Vector;
use crate::game::reflection;

/// `void ABrickCameraManager::ApplyCameraModifiers(float DeltaTime, FMinimalViewInfo& InOutPOV)`
/// (BRMK 0x2977F0, game RVA 0xE142C0): the last step of each view target's view, every frame
/// whatever the view target (`APlayerCameraManager::UpdateViewTarget`, after the view target's
/// `CalcCamera`). The engine's (camera shakes and other modifiers), then the game's effects:
/// post-process settings (by the player's speed, in restricted areas, ...).
pub const APPLY_CAMERA_MODIFIERS: FunctionSig = FunctionSig {
    name: "ABrickCameraManager::ApplyCameraModifiers",
    // Turning on two post-process overrides of the view (`bOverride_...` bits) around a lerp of
    // one of their values.
    pattern: "80 4f ?? 02 0f 28 f8 f3 0f 10 8f ?? ?? 00 00 f3 0f 10 93 ?? ?? 00 00 80 4f ?? 20",
};

/// [`APPLY_CAMERA_MODIFIERS`]'s signature. `view` is the `FMinimalViewInfo` (see [`ViewInfo`]).
pub type ApplyCameraModifiersFn =
    unsafe extern "C" fn(manager: *mut UObjectBase, delta: f32, view: *mut u8);

/// Where the fields of `FMinimalViewInfo` (engine, reflected) are: the view a camera makes.
pub struct ViewInfo {
    location: usize,
    rotation: usize,
    fov: usize,
    ortho_width: usize,
    ortho_near: usize,
    ortho_far: usize,
    projection_mode: usize,
    /// `ECameraProjectionMode::Orthographic`.
    orthographic: u8,
}

/// Where the main view target and its view are in the camera manager: `ViewTarget`, an
/// `FTViewTarget` (engine, reflected), holds `Target` (the actor) and `POV` (its view).
pub struct ViewTargets {
    /// Offset of `APlayerCameraManager::ViewTarget`.
    view_target: usize,
    /// Offsets of `Target` and `POV` in `FTViewTarget`.
    target: usize,
    pov: usize,
}

// SAFETY: offsets living as long as the game.
unsafe impl Send for ViewInfo {}
unsafe impl Sync for ViewInfo {}
unsafe impl Send for ViewTargets {}
unsafe impl Sync for ViewTargets {}

pub fn view_info() -> Result<ViewInfo, String> {
    let view = script_struct("MinimalViewInfo")?;
    let float = |name| reflection::struct_field_offset(view, name, "FloatProperty", 4);
    let vector = |name| reflection::struct_field_offset(view, name, "StructProperty", 12);
    let orthographic = reflection::enum_value(
        "ECameraProjectionMode",
        "ECameraProjectionMode::Orthographic",
    )?;
    Ok(ViewInfo {
        location: vector("Location")?,
        rotation: vector("Rotation")?,
        fov: float("FOV")?,
        ortho_width: float("OrthoWidth")?,
        ortho_near: float("OrthoNearClipPlane")?,
        ortho_far: float("OrthoFarClipPlane")?,
        projection_mode: reflection::struct_field_offset(
            view,
            "ProjectionMode",
            "ByteProperty",
            1,
        )?,
        orthographic: u8::try_from(orthographic)
            .map_err(|_| format!("ECameraProjectionMode::Orthographic is {orthographic}"))?,
    })
}

pub fn view_targets() -> Result<ViewTargets, String> {
    let manager = reflection::find_class("PlayerCameraManager")
        .ok_or("class PlayerCameraManager not found")?;
    let target = script_struct("TViewTarget")?;
    Ok(ViewTargets {
        view_target: struct_field_offset(manager.cast(), "ViewTarget")?,
        target: reflection::struct_field_offset(target, "Target", "ObjectProperty", 8)?,
        pov: struct_field_offset(target, "POV")?,
    })
}

/// The reflected struct called `name` (e.g. `"MinimalViewInfo"`).
fn script_struct(name: &'static str) -> Result<*const UStruct, String> {
    reflection::find_object(name, "ScriptStruct")
        .map(|found| found.cast_const().cast::<UStruct>())
        .ok_or(format!("struct {name} not found"))
}

/// The offset of the field `name` of `ustruct` (a class or struct found through reflection), a
/// struct of any size.
fn struct_field_offset(ustruct: *const UStruct, name: &'static str) -> Result<usize, String> {
    // SAFETY: `ustruct` comes from reflection, so it's live and linked; a found property is a live
    // record.
    unsafe {
        let property = reflection::find_property(ustruct, name);
        if property.is_null() || !reflection::is_kind(property, "StructProperty") {
            return Err(format!("struct field '{name}' not found"));
        }
        Ok((*property).offset_internal as usize)
    }
}

impl ViewTargets {
    /// The actor whose view `view` is, if it's the camera manager's main view (not a view target
    /// being blended to).
    ///
    /// # Safety
    /// `manager` must be a live `APlayerCameraManager`. Game thread only.
    pub unsafe fn of(
        &self,
        manager: *const UObjectBase,
        view: *const u8,
    ) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; the offsets come from reflection.
        unsafe {
            let view_target = manager.cast::<u8>().add(self.view_target);
            if view_target.add(self.pov) != view {
                return None;
            }
            let target = view_target
                .add(self.target)
                .cast::<*mut UObjectBase>()
                .read();
            (!target.is_null()).then_some(target)
        }
    }
}

impl ViewInfo {
    /// Where the view is seen from.
    ///
    /// # Safety
    /// `view` must point to a live `FMinimalViewInfo`.
    pub unsafe fn location(&self, view: *const u8) -> Vector {
        // SAFETY: per the contract; the offset comes from reflection.
        unsafe { view.add(self.location).cast::<Vector>().read_unaligned() }
    }

    /// Which way it looks: pitch, yaw and roll, in degrees.
    ///
    /// # Safety
    /// As for [`Self::location`].
    pub unsafe fn rotation(&self, view: *const u8) -> Vector {
        // SAFETY: per the contract; the offset comes from reflection.
        unsafe { view.add(self.rotation).cast::<Vector>().read_unaligned() }
    }

    /// The horizontal field of view, in degrees.
    ///
    /// # Safety
    /// As for [`Self::location`].
    pub unsafe fn fov(&self, view: *const u8) -> f32 {
        // SAFETY: per the contract; the offset comes from reflection.
        unsafe { view.add(self.fov).cast::<f32>().read() }
    }

    /// Moves `view` to `location`.
    ///
    /// # Safety
    /// `view` must point to a live `FMinimalViewInfo`. Game thread only.
    pub unsafe fn set_location(&self, view: *mut u8, location: Vector) {
        // SAFETY: per the contract; the offset comes from reflection.
        unsafe {
            view.add(self.location)
                .cast::<Vector>()
                .write_unaligned(location)
        }
    }

    /// Makes `view` a perspective one from `location` with a horizontal field of view of `fov`
    /// degrees.
    ///
    /// # Safety
    /// `view` must point to a live `FMinimalViewInfo`. Game thread only.
    pub unsafe fn set_perspective(&self, view: *mut u8, location: Vector, fov: f32) {
        // SAFETY: per the contract; the offset comes from reflection.
        unsafe {
            self.set_location(view, location);
            view.add(self.fov).cast::<f32>().write(fov);
        }
    }

    /// Makes `view` orthographic, `width` cm wide (its height follows the screen's shape), showing
    /// what lies from `near` to `far` cm in front of its place (negative: behind it).
    ///
    /// # Safety
    /// As for [`Self::set_perspective`].
    pub unsafe fn set_orthographic(&self, view: *mut u8, width: f32, [near, far]: [f32; 2]) {
        // SAFETY: per the contract; the offsets come from reflection.
        unsafe {
            view.add(self.ortho_width).cast::<f32>().write(width);
            view.add(self.ortho_near).cast::<f32>().write(near);
            view.add(self.ortho_far).cast::<f32>().write(far);
            view.add(self.projection_mode).write(self.orthographic);
        }
    }
}
