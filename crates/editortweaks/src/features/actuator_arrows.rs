//! A selected linear actuator (cylinder, slider) shows its limits as two arrows: green for its max
//! extension (`MaxLimit`), red for its max contraction (`MinLimit`). The game only draws each one
//! while its limit stays on its usual side of zero, as its sliders keep it (see
//! `game::actuator`); with `property_limits` a limit can cross zero, and its arrow vanished. Now it
//! shows turned around: pointing the other way from the same base.
//!
//! How: while the game draws an actuator's arrows, its limits are on their usual sides (their sizes
//! kept) so that it draws both, then put back; each arrow whose limit crossed zero is turned around
//! as the game sets it up. Only the editor's arrows change: the limits are back before anything
//! else runs.

use std::cell::Cell;
use std::sync::{Once, OnceLock};

use brickrust::ue::coreuobject::UObjectBase;
use core::ffi::c_void;

use crate::framework::hook;
use crate::game::actuator::{self, Actuators, SetArrowFn, UpdateEditorVisualizationFn};
use crate::game::editor_object::Vector;
use crate::math::Quat;
use crate::prelude::*;

pub struct ActuatorArrows;

static UPDATE_EDITOR_VISUALIZATION: OnceLock<UpdateEditorVisualizationFn> = OnceLock::new();
static SET_ARROW: OnceLock<SetArrowFn> = OnceLock::new();
/// Set at engine init; until then (or if that fails) the hooks change nothing.
static ACTUATORS: OnceLock<Actuators> = OnceLock::new();
/// Logs the first time arrows are turned around.
static FIRST_TURN: Once = Once::new();

thread_local! {
    /// While the game draws an actuator's arrows, which ones to turn around.
    static TURNING: Cell<Option<Turning>> = const { Cell::new(None) };
}

/// The arrows of the actuator being drawn whose limits crossed zero.
#[derive(Clone, Copy)]
struct Turning {
    /// `MaxLimit` is above zero: turn the extension arrow around.
    extension: bool,
    /// `MinLimit` is below zero: turn the contraction arrow around.
    contraction: bool,
}

impl Feature for ActuatorArrows {
    fn id(&self) -> &'static str {
        "actuator_arrows"
    }

    fn on_load(&self) -> LoadResult {
        let update = actuator::UPDATE_EDITOR_VISUALIZATION.resolve()?;
        let set_arrow = actuator::SET_ARROW.resolve()?;
        // SAFETY: each target is the start of the function its signature names, and each detour
        // has that function's signature.
        unsafe {
            hook::install_typed(
                actuator::SET_ARROW.name,
                set_arrow,
                set_arrow_detour as SetArrowFn,
                &SET_ARROW,
            )?;
            hook::install_typed(
                actuator::UPDATE_EDITOR_VISUALIZATION.name,
                update,
                update_detour as UpdateEditorVisualizationFn,
                &UPDATE_EDITOR_VISUALIZATION,
            )?;
        }
        Ok(())
    }

    fn on_engine_init(&self) {
        match Actuators::find() {
            Ok(actuators) => {
                let _ = ACTUATORS.set(actuators);
                log!("active");
            }
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

/// Draws `actuator`'s arrows with its limits on their usual sides, turning around the arrows of
/// those that crossed zero.
unsafe extern "C" fn update_detour(actuator: *mut UObjectBase) {
    let original = *UPDATE_EDITOR_VISUALIZATION
        .get()
        .expect("hook recorded before use");
    // SAFETY: the game's call, passed on; `actuator` is the live actuator it updates, on the game
    // thread, and its limits are put back right after.
    unsafe {
        let Some(actuators) = ACTUATORS.get() else {
            return original(actuator);
        };
        if !actuators.is_linear(actuator) {
            return original(actuator);
        }
        let [max, min] = actuators.limits(actuator);
        let turning = Turning {
            extension: max > 0.0,
            contraction: min < 0.0,
        };
        if !turning.extension && !turning.contraction {
            return original(actuator);
        }
        actuators.set_limits(actuator, [-max.abs(), min.abs()]);
        TURNING.set(Some(turning));
        original(actuator);
        TURNING.set(None);
        actuators.set_limits(actuator, [max, min]);
        FIRST_TURN.call_once(|| {
            log!("turned arrows around for limits past zero (max {max}, min {min})");
        });
    }
}

/// Sets up an arrow as the game asks, turned around if it's a limit's that crossed zero.
unsafe extern "C" fn set_arrow_detour(
    captures: *mut c_void,
    location: *const Vector,
    rotation: *const [f32; 4],
    angle: f32,
    size: f32,
    arrow_type: u8,
    color_style: u8,
) {
    let original = *SET_ARROW.get().expect("hook recorded before use");
    // SAFETY: the game's call, passed on; `location` and `rotation` point to its values, or to
    // ours, which live until the call returns.
    unsafe {
        let turning = TURNING.get().filter(|turning| {
            ACTUATORS.get().is_some_and(|actuators| {
                arrow_type == actuators.linear_arrow
                    && (turning.extension && color_style == actuators.positive
                        || turning.contraction && color_style == actuators.negative)
            })
        });
        if turning.is_none() {
            return original(
                captures,
                location,
                rotation,
                angle,
                size,
                arrow_type,
                color_style,
            );
        };
        let rotation = turned_around(*rotation);
        original(
            captures,
            location,
            &rotation,
            angle,
            size,
            arrow_type,
            color_style,
        );
    }
}

/// The rotation of a straight arrow (pointing along its Z axis) turned around on its base: a half
/// turn about its own X axis reverses its Z axis.
fn turned_around(rotation: [f32; 4]) -> [f32; 4] {
    let Quat { x, y, z, w } = Quat::widen(rotation) * Quat::from_axis_angle([1.0, 0.0, 0.0], 180.0);
    [x, y, z, w].map(|value| value as f32)
}
