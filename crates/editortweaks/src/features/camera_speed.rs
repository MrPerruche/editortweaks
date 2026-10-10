//! Editor camera speed: each speed step (Ctrl + mouse wheel) multiplies the speed by the same factor,
//! from [`SLOWEST`] to [`FASTEST`] in [`STEPS`] steps. The game's own 20 steps follow `ratio^1.5`
//! from 50 to 24000 cm/s: 50, 318, 807, 1441, ... 22226, 24000, steps of 500 to 1800 cm/s that
//! feel nearly linear. The free camera keeps the game's speeds.
//!
//! How: the function computing camera speeds (`game::view_target::CAMERA_SPEED`) is hooked. For the
//! editor's camera it returns `SLOWEST * (FASTEST / SLOWEST)^ratio`, and sets the editor's speed
//! step (`CameraSpeedParams.Step`, which the speed steps add to the ratio) to `1 / STEPS`.

use core::ffi::c_void;
use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UClass;

use crate::framework::hook;
use crate::game::reflection;
use crate::game::view_target::{self, CameraSpeedFn, SpeedParams};
use crate::prelude::*;

/// Slowest editor camera speed, in cm/s (the game's: 50, i.e. 0.5 m/s).
const SLOWEST: f32 = 5.0;
/// Fastest editor camera speed, in cm/s (the game's: 24000, i.e. 240 m/s).
const FASTEST: f32 = 192_000.0;
/// Speed steps from the slowest to the fastest speed (the game's: 20). Each step multiplies the
/// speed by `(FASTEST / SLOWEST)^(1 / STEPS)`: ×1.36 with the comments' values above.
const STEPS: u16 = 30;

const _: () = assert!(SLOWEST > 0.0 && FASTEST >= SLOWEST && STEPS > 0);

pub struct CameraSpeed;

static ORIGINAL: OnceLock<CameraSpeedFn> = OnceLock::new();
static SPEED: OnceLock<view_target::CameraSpeed> = OnceLock::new();
/// Set once reflection is available; until then (or if it fails) speeds stay the game's.
static EDITOR: OnceLock<Editor> = OnceLock::new();

struct Editor {
    /// `BrickEditor`.
    class: *const UClass,
    params: SpeedParams,
}

// SAFETY: a reflection record living as long as the game; only used on the game thread.
unsafe impl Send for Editor {}
unsafe impl Sync for Editor {}

impl Feature for CameraSpeed {
    fn id(&self) -> &'static str {
        "camera_speed"
    }

    fn on_load(&self) -> LoadResult {
        let speed = view_target::camera_speed()?;
        let target = speed.function;
        let _ = SPEED.set(speed);
        // SAFETY: `target` is the start of GetCameraSpeed, and `camera_speed` has its signature.
        unsafe {
            hook::install_typed(
                view_target::CAMERA_SPEED.name,
                target,
                camera_speed as CameraSpeedFn,
                &ORIGINAL,
            )
        }
    }

    fn on_engine_init(&self) {
        match engine_init() {
            Ok(()) => log!(
                "active: {SLOWEST} to {FASTEST} cm/s in {STEPS} steps of x{:.2}",
                (FASTEST / SLOWEST).powf(1.0 / f32::from(STEPS))
            ),
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

fn engine_init() -> Result<(), String> {
    let editor = Editor {
        class: reflection::find_class("BrickEditor").ok_or("class BrickEditor not found")?,
        params: view_target::speed_params()?,
    };
    let _ = EDITOR.set(editor);
    Ok(())
}

unsafe extern "C" fn camera_speed(interface: *mut c_void) -> f32 {
    let original = *ORIGINAL.get().expect("hook recorded before use");
    // SAFETY: the caller's view target, alive during the call.
    unsafe { editor_speed(interface).unwrap_or_else(|| original(interface)) }
}

/// The speed of the editor's camera, if `interface` is the editor's.
///
/// # Safety
/// `interface` must point to a live object's `IViewTargetInterface`. Game thread only.
unsafe fn editor_speed(interface: *mut c_void) -> Option<f32> {
    let (editor, speed) = (EDITOR.get()?, SPEED.get()?);
    // SAFETY: per the contract.
    unsafe {
        if !editor.is_editor(view_target::object_of(interface)?) {
            return None;
        }
        let params = speed.params(interface);
        if params.is_null() {
            return None;
        }
        editor.use_steps(params);
        let ratio = speed.ratio(interface).clamp(0.0, 1.0);
        Some(SLOWEST * (FASTEST / SLOWEST).powf(ratio))
    }
}

impl Editor {
    /// # Safety
    /// `object` must be a live object.
    unsafe fn is_editor(&self, object: *mut UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { reflection::with_parents(reflection::class_of(object.cast())) }
            .any(|class| class == self.class.cast())
    }

    /// Makes the speed steps of the editor's `params` go from 0 to 1 in [`STEPS`] steps.
    ///
    /// # Safety
    /// `params` must point to the editor's live `FViewTargetCameraSpeedParams`. Game thread only.
    unsafe fn use_steps(&self, params: *mut u8) {
        let step = 1.0 / f32::from(STEPS);
        // SAFETY: per the contract.
        unsafe {
            let (game_step, [min, max]) = self.params.read(params);
            if game_step != step {
                log!("the game's editor camera: {min} to {max} cm/s, ratio steps of {game_step}");
                self.params.set_step(params, step);
            }
        }
    }
}
