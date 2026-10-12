//! A selected axle shows its suspension like an actuator shows its limits: a green arrow from each
//! wheel connector, down along the axle, `SuspensionLength` long. The suspension lets a wheel move
//! that far down from where it was built, never above it (see `docs/re/axle-arrows.md`); the game
//! showed only steering and drive arrows.
//!
//! How: while the game draws an axle's arrows, its steering arrow (the first one of each wheel
//! connector, at the connector) brings the suspension arrow along, drawn through the actuators'
//! arrow lambda (it makes straight arrows); an axle that doesn't steer gets a steering angle for
//! as long as the game draws, and that arrow is left out. The axle's own lambda doesn't set the
//! arrow type, assuming circular arrows, so its arrows go through the actuators' lambda too:
//! otherwise one could reuse a straight suspension arrow and stay straight.

use std::cell::Cell;
use std::sync::{Once, OnceLock};

use brickrust::ue::coreuobject::UObjectBase;
use core::ffi::c_void;

use crate::framework::hook;
use crate::game::actuator::{self, SetArrowFn as LinearSetArrowFn};
use crate::game::axle::{self, Axles, SetArrowFn, UpdateEditorVisualizationFn};
use crate::game::editor_object::Vector;
use crate::math::{self, Quat};
use crate::prelude::*;

pub struct SuspensionArrows;

static UPDATE_EDITOR_VISUALIZATION: OnceLock<UpdateEditorVisualizationFn> = OnceLock::new();
static SET_ARROW: OnceLock<SetArrowFn> = OnceLock::new();
/// The actuators' arrow lambda, which sets every property of an arrow.
static SET_ANY_ARROW: OnceLock<LinearSetArrowFn> = OnceLock::new();
/// Set at engine init; until then (or if that fails) the hooks change nothing.
static AXLES: OnceLock<Axles> = OnceLock::new();
/// Logs the first time suspension arrows are drawn.
static FIRST_DRAW: Once = Once::new();

/// The game draws a steering arrow only for angles above this (`SMALL_NUMBER`).
const STEERING_THRESHOLD: f32 = 1e-8;
/// The steering angle given to an axle that doesn't steer while its arrows are drawn.
const STAND_IN_STEERING: f32 = 1.0;

thread_local! {
    /// While the game draws an axle's arrows, how to draw its suspension.
    static DRAWING: Cell<Option<Suspension>> = const { Cell::new(None) };
}

/// The suspension of the axle being drawn.
#[derive(Clone, Copy)]
struct Suspension {
    /// How far a wheel moves down (cm).
    travel: f32,
    /// The steering arrows are stand-ins (the axle doesn't steer): leave them out.
    hide_steering: bool,
}

impl Feature for SuspensionArrows {
    fn id(&self) -> &'static str {
        "suspension_arrows"
    }

    fn on_load(&self) -> LoadResult {
        let update = axle::UPDATE_EDITOR_VISUALIZATION.locate()?;
        // SAFETY: the call lies within the matched pattern.
        let set_arrow = unsafe { update.call_target(axle::ARROW_LAMBDA_CALL) }
            .ok_or("axle arrow lambda: no call at the pattern's end")?;
        let set_any_arrow = actuator::SET_ARROW.resolve()?;
        // SAFETY: the start of the actuators' arrow lambda, which has this signature (called
        // through its hook if `actuator_arrows` installed one).
        let _ = SET_ANY_ARROW
            .set(unsafe { core::mem::transmute::<*const u8, LinearSetArrowFn>(set_any_arrow) });
        // SAFETY: each target is the start of the function its signature names, and each detour
        // has that function's signature.
        unsafe {
            hook::install_typed(
                "UAxleBrick::UpdateEditorVisualization arrow lambda",
                set_arrow,
                set_arrow_detour as SetArrowFn,
                &SET_ARROW,
            )?;
            hook::install_typed(
                axle::UPDATE_EDITOR_VISUALIZATION.name,
                update.function,
                update_detour as UpdateEditorVisualizationFn,
                &UPDATE_EDITOR_VISUALIZATION,
            )?;
        }
        Ok(())
    }

    fn on_engine_init(&self) {
        match Axles::find() {
            Ok(axles) => {
                let _ = AXLES.set(axles);
                log!("active");
            }
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

/// Draws `brick`'s arrows, with the suspension's if it's an axle with one.
unsafe extern "C" fn update_detour(brick: *mut UObjectBase) {
    let original = *UPDATE_EDITOR_VISUALIZATION
        .get()
        .expect("hook recorded before use");
    // SAFETY: the game's call, passed on; `brick` is the live brick it updates, on the game
    // thread, and its steering angle is put back right after.
    unsafe {
        let Some(axles) = AXLES.get() else {
            return original(brick);
        };
        if !axles.is_axle(brick) {
            return original(brick);
        }
        let travel = axles.suspension_length(brick);
        if travel <= 0.0 {
            return original(brick);
        }
        let steering = axles.steering_angle(brick);
        let hide_steering = steering.abs() <= STEERING_THRESHOLD;
        if hide_steering {
            axles.set_steering_angle(brick, STAND_IN_STEERING);
        }
        DRAWING.set(Some(Suspension {
            travel,
            hide_steering,
        }));
        original(brick);
        DRAWING.set(None);
        if hide_steering {
            axles.set_steering_angle(brick, steering);
        }
        FIRST_DRAW.call_once(|| log!("drew suspension arrows ({travel} cm down)"));
    }
}

/// Sets up an axle's arrow as the game asks (through the actuators' lambda, so that it's circular
/// whatever it was before), and after a steering arrow the suspension's.
unsafe extern "C" fn set_arrow_detour(
    captures: *mut c_void,
    location: *const Vector,
    rotation: *const [f32; 3],
    angle: f32,
    size: f32,
    color_style: u8,
) {
    let original = *SET_ARROW.get().expect("hook recorded before use");
    let (Some(axles), Some(&set_any_arrow)) = (AXLES.get(), SET_ANY_ARROW.get()) else {
        // SAFETY: the game's call, passed on.
        return unsafe { original(captures, location, rotation, angle, size, color_style) };
    };
    // SAFETY: the game's call; both lambdas take the same captures (the axle's editor params, arrow
    // counter and arrow class, and the axle), and `location` and `rotation` point to its values.
    unsafe {
        let steering = color_style == axles.positive;
        let suspension = DRAWING.get();
        if !(steering && suspension.is_some_and(|suspension| suspension.hide_steering)) {
            let rotation = narrow(Quat::from_rotator(math::widen(*rotation)));
            set_any_arrow(
                captures,
                location,
                &rotation,
                angle,
                size,
                axles.circular_arrow,
                color_style,
            );
        }
        let Some(suspension) = suspension.filter(|_| steering) else {
            return;
        };
        // A straight arrow points along its Z axis: the axle's, turned around to point down.
        let down = narrow(Quat::from_axis_angle([1.0, 0.0, 0.0], 180.0));
        set_any_arrow(
            captures,
            location,
            &down,
            0.0,
            suspension.travel,
            axles.linear_arrow,
            axles.positive,
        );
    }
}

/// A rotation as the game stores it.
fn narrow(Quat { x, y, z, w }: Quat) -> [f32; 4] {
    [x, y, z, w].map(|value| value as f32)
}
