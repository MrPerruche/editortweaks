//! The camera of what the player views (`IViewTargetInterface`, implemented by `ABrickEditor`, the
//! free camera's `ABrickSpectatorPawn`, vehicles, ...), here its speed and its zoom. Found through
//! BRMK 1.11.6's exports, then in game 1.11.7 (build 25831956); see `docs/re/editor-input.md` and
//! `docs/re/editor-camera.md`.

use core::ffi::c_void;

use brickrust::ue::coreuobject::UObjectBase;

use crate::framework::sig::FunctionSig;
use crate::game::reflection;

/// `float IViewTargetInterface::GetCameraSpeed() const` (our name; BRMK 0x323F40, game RVA
/// 0xE6AD60): the camera's speed in cm/s, from its speed ratio (0 to 1, changed in steps by
/// `Action_SpeedUpStep`/`Action_SpeedDownStep`, Ctrl + mouse wheel in the editor):
///
/// ```text
/// const FViewTargetCameraSpeedParams* Params = GetCameraSpeedParams();   // virtual
/// if (!Params) return 0;
/// return Lerp(Params->SpeedRange.Min, Params->SpeedRange.Max,
///             powf(GetCameraSpeedRatio(), Params->Exponent));             // virtual
/// ```
///
/// Called every frame by `ABrickEditor::Tick` (camera movement) and by the free camera, and for
/// the notification showing the new speed.
pub const CAMERA_SPEED: FunctionSig = FunctionSig {
    name: "IViewTargetInterface::GetCameraSpeed",
    // Both virtual calls (their vtable offsets at +8 and +38) around reading the exponent, then
    // powf.
    pattern: "48 8b 01 48 8b d9 ff 90 ?? ?? 00 00 48 8b f8 48 85 c0 74 ?? 48 8b 13 48 8b cb 0f 29 74 24 ?? f3 0f 10 70 ?? ff 92 ?? ?? 00 00 0f 28 ce e8",
};

/// [`CAMERA_SPEED`]'s signature. `interface` is the view target's `IViewTargetInterface`.
pub type CameraSpeedFn = unsafe extern "C" fn(interface: *mut c_void) -> f32;

/// Slot of `_getUObject` in `IViewTargetInterface`'s vtable (BRMK and game, checked on the editor's
/// vtables; `GetCameraSpeedParams` is slot 16, `GetCameraSpeedRatio` slot 18).
const GET_UOBJECT_SLOT: usize = 2;

/// Slot of `GetZoomCache` in `IViewTargetInterface`'s vtable (BRMK and game: `UpdateZoom` calls it
/// at +0xA8, `GetZoomParams` at +0xA0): the view target's `FViewTargetZoomCache`, or null.
const GET_ZOOM_CACHE_SLOT: usize = 21;

/// Offset of the current zoom ratio in `FViewTargetZoomCache` (not reflected: +0x0 the held zoom
/// input, +0x4 the ratio the zoom steps set, +0x8 the current one, gliding there).
const ZOOM_CACHE_RATIO: usize = 8;

pub struct CameraSpeed {
    /// [`CAMERA_SPEED`]'s start.
    pub function: *const u8,
    /// Vtable offsets of `GetCameraSpeedParams` and `GetCameraSpeedRatio`.
    params: usize,
    ratio: usize,
}

// SAFETY: points into the game's code; only used on the game thread (and while loading).
unsafe impl Send for CameraSpeed {}
unsafe impl Sync for CameraSpeed {}

pub fn camera_speed() -> Result<CameraSpeed, String> {
    let found = CAMERA_SPEED.locate()?;
    // SAFETY: both offsets are 32-bit displacements inside the matched pattern (see its comment).
    let (params, ratio) = unsafe {
        (
            found.address.add(8).cast::<u32>().read_unaligned() as usize,
            found.address.add(38).cast::<u32>().read_unaligned() as usize,
        )
    };
    if params % 8 != 0 || ratio % 8 != 0 {
        return Err(format!(
            "{}: unexpected vtable offsets 0x{params:X}, 0x{ratio:X}",
            CAMERA_SPEED.name
        ));
    }
    Ok(CameraSpeed {
        function: found.function,
        params,
        ratio,
    })
}

impl CameraSpeed {
    /// The view target's `FViewTargetCameraSpeedParams` (see [`SpeedParams`]), or null.
    ///
    /// # Safety
    /// `interface` must point to a live object's `IViewTargetInterface`. Game thread only.
    pub unsafe fn params(&self, interface: *mut c_void) -> *mut u8 {
        // SAFETY: per the contract; the vtable offset comes from the game's own call.
        unsafe {
            let get: unsafe extern "C" fn(*mut c_void) -> *mut u8 =
                virtual_at(interface, self.params);
            get(interface)
        }
    }

    /// The view target's speed ratio: 0 (slowest) to 1 (fastest).
    ///
    /// # Safety
    /// As for [`Self::params`].
    pub unsafe fn ratio(&self, interface: *mut c_void) -> f32 {
        // SAFETY: as for `params`.
        unsafe {
            let get: unsafe extern "C" fn(*mut c_void) -> f32 = virtual_at(interface, self.ratio);
            get(interface)
        }
    }
}

/// The view target's zoom ratio: 0 (unzoomed) to 1 (zoomed in the most), if it zooms.
///
/// # Safety
/// `interface` must point to a live object's `IViewTargetInterface`. Game thread only.
pub unsafe fn zoom_ratio(interface: *mut c_void) -> Option<f32> {
    // SAFETY: per the contract; the slot is `GetZoomCache`, which returns null or the view
    // target's own zoom cache.
    unsafe {
        let get: unsafe extern "C" fn(*mut c_void) -> *const u8 =
            virtual_at(interface, GET_ZOOM_CACHE_SLOT * size_of::<usize>());
        let cache = get(interface);
        if cache.is_null() {
            return None;
        }
        let ratio = cache.add(ZOOM_CACHE_RATIO).cast::<f32>().read_unaligned();
        (0.0..=1.0).contains(&ratio).then_some(ratio)
    }
}

/// The object behind an `IViewTargetInterface` pointer, if its `_getUObject` looks like one: code
/// that only subtracts the interface's offset in the object (`lea rax, [rcx - offset]; ret`).
///
/// # Safety
/// `interface` must point to a live object's `IViewTargetInterface`. Game thread only.
pub unsafe fn object_of(interface: *mut c_void) -> Option<*mut UObjectBase> {
    // SAFETY: per the contract; the slot is read, and only called once its code was checked.
    unsafe {
        let get: unsafe extern "C" fn(*mut c_void) -> *mut UObjectBase =
            virtual_at(interface, GET_UOBJECT_SLOT * size_of::<usize>());
        let code = (get as *const u8).cast::<[u8; 8]>().read_unaligned();
        let adjusts = matches!(code, [0x48, 0x8d, 0x81, _, _, _, _, 0xc3])
            || matches!(code, [0x48, 0x8d, 0x41, _, 0xc3, ..]);
        adjusts.then(|| get(interface))
    }
}

/// The function at `offset` bytes into the vtable of `interface`.
///
/// # Safety
/// `interface` must point to an object with a vtable, holding a function of type `F` there.
unsafe fn virtual_at<F: Copy>(interface: *mut c_void, offset: usize) -> F {
    const { assert!(size_of::<F>() == size_of::<usize>()) };
    // SAFETY: per the contract.
    unsafe {
        let vtable = *interface.cast::<*const u8>();
        vtable.add(offset).cast::<F>().read()
    }
}

/// Where the fields of `FViewTargetCameraSpeedParams` are (reflected, read by name): `Step` (by how
/// much each speed step changes the ratio) and `SpeedRange` (an `FFloatInterval`: Min, Max).
/// The editor's are `CameraSpeedParams` in its static info (`UBrickEditorStaticInfo`).
pub struct SpeedParams {
    step: usize,
    range: usize,
}

pub fn speed_params() -> Result<SpeedParams, String> {
    let info = reflection::find_class("BrickEditorStaticInfo")
        .ok_or("class BrickEditorStaticInfo not found")?;
    let params = reflection::field(info, "CameraSpeedParams", "StructProperty", 0x18)?;
    // SAFETY: a live StructProperty record, so its struct is live and linked.
    let params = unsafe { reflection::struct_type(params) };
    let field = |name, kind, size| {
        reflection::struct_field_offset(params, name, kind, size)
            .map_err(|reason| format!("CameraSpeedParams: {reason}"))
    };
    Ok(SpeedParams {
        step: field("Step", "FloatProperty", 4)?,
        range: field("SpeedRange", "StructProperty", 8)?,
    })
}

impl SpeedParams {
    /// The ratio's step and the speed range (cm/s) of `params`.
    ///
    /// # Safety
    /// `params` must point to a live `FViewTargetCameraSpeedParams`.
    pub unsafe fn read(&self, params: *const u8) -> (f32, [f32; 2]) {
        // SAFETY: per the contract; the offsets come from its reflection data.
        unsafe {
            (
                params.add(self.step).cast::<f32>().read(),
                params.add(self.range).cast::<[f32; 2]>().read(),
            )
        }
    }

    /// Sets the ratio's step of `params`: the speed steps then go from 0 to 1 by `step`.
    ///
    /// # Safety
    /// `params` must point to a live `FViewTargetCameraSpeedParams`. Game thread only.
    pub unsafe fn set_step(&self, params: *mut u8, step: f32) {
        // SAFETY: per the contract.
        unsafe { params.add(self.step).cast::<f32>().write(step) }
    }
}
