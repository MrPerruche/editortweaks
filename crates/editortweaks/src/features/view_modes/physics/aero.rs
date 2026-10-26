//! The air's push on one lift surface, from plain data: the game's formulas (`UBrick::
//! TickPartRootFluidDynamics`, see `docs/re/physics-centers.md`, "Lift and drag in full"), for a
//! surface moving through still air.
//!
//! The game multiplies every force by the air's density and the speed squared, the same for every
//! surface of a vehicle flying straight; so the pushes here leave both out (cm², the force per unit
//! of `density * speed^2`):
//!
//! ```text
//! cos   = Velocity . Normal;  Angle = asin(|cos|)        // between the velocity and the surface
//! Lift  = LiftCoefficient * PI R^2 * Factor(Angle)       // across the velocity, away from the air
//! Drag  = DragCoefficient * (c X + (PI R^2 - c X) |cos|)  // against the velocity
//! ```
//!
//! `R` is the surface's radius (`PI R^2` its face's area), `X` its cross section (the brick's sides),
//! `c` [`Coefficients::cross_section`]: the drag of a face meeting the air edge-on.

use crate::math::{self, Vec3};

/// The game's coefficients, its console variables `BR.FluidDynamics.*` (read in game by
/// `game::console_variable`; [`Coefficients::DEFAULT`] are their defaults).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coefficients {
    /// `AirLiftCoefficient`.
    pub lift: f64,
    /// `AirDragCoefficient`.
    pub drag: f64,
    /// `CrossSectionCoefficient`: the share of the cross section facing the air edge-on.
    pub cross_section: f64,
    /// `PeakLiftAngle` and `MaxLiftAngle`, in degrees: lift is strongest at the first and gone from
    /// the second.
    pub peak_lift_angle: f64,
    pub max_lift_angle: f64,
}

impl Coefficients {
    pub const DEFAULT: Self = Self {
        lift: 0.75,
        drag: 0.3,
        cross_section: 0.000_667,
        peak_lift_angle: 22.5,
        max_lift_angle: 80.0,
    };
}

/// A lift surface, in the vehicle's space.
#[derive(Clone, Copy, Debug)]
pub struct Surface {
    pub location: Vec3,
    /// Across the surface.
    pub normal: Vec3,
    /// `LiftSurfaceRadius`, in cm.
    pub radius: f64,
    /// `LiftSurfaceCrossSectionArea`, in cm².
    pub cross_section: f64,
}

/// The air's push on a surface, per unit of `density * speed^2` (cm²).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Push {
    pub lift: Vec3,
    pub drag: Vec3,
}

/// The push on `surface` moving with `velocity` (unit length).
pub fn push(surface: &Surface, velocity: Vec3, coefficients: &Coefficients) -> Push {
    let Some(normal) = math::normalized(surface.normal) else {
        return Push::default();
    };
    let disk = core::f64::consts::PI * surface.radius * surface.radius;
    let edge_on = coefficients.cross_section * surface.cross_section;
    let facing = math::dot(velocity, normal).abs().min(1.0);
    let drag = coefficients.drag * (edge_on + (disk - edge_on) * facing);
    Push {
        lift: lift(surface, velocity, coefficients).unwrap_or_default(),
        drag: math::scale(velocity, -drag),
    }
}

/// The lift on `surface` moving with `velocity` (unit length), per unit of `density * speed^2`;
/// `None` if it lifts nothing.
pub fn lift(surface: &Surface, velocity: Vec3, coefficients: &Coefficients) -> Option<Vec3> {
    let normal = math::normalized(surface.normal)?;
    let along = math::dot(velocity, normal);
    let angle = along.abs().min(1.0).asin().to_degrees();
    let factor = lift_factor(angle, coefficients);
    if factor <= 0.0 {
        return None;
    }
    // Across the velocity, toward the side the surface moves away from.
    let across = math::sub(normal, math::scale(velocity, along));
    let direction = math::normalized(math::scale(across, -along.signum()))?;
    let disk = core::f64::consts::PI * surface.radius * surface.radius;
    Some(math::scale(direction, coefficients.lift * disk * factor))
}

/// The game's lift factor at `angle` degrees between the velocity and the surface: rising as a
/// sine to 1 at the peak angle, falling back to 0 at the max angle.
fn lift_factor(angle: f64, coefficients: &Coefficients) -> f64 {
    let (peak, max) = (coefficients.peak_lift_angle, coefficients.max_lift_angle);
    if angle <= 0.0 || angle >= max {
        0.0
    } else if angle < peak {
        (angle / peak * core::f64::consts::FRAC_PI_2).sin()
    } else {
        let past = (angle - peak) / (max - peak);
        ((past * core::f64::consts::PI).cos() + 1.0) / 2.0
    }
}
