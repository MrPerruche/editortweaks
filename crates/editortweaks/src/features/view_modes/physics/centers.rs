//! Where the forces on a vehicle act, from plain data: the center of lift for a vehicle flying
//! forward, and the center of thrust. Both are where the forces' sum acts, each force counted by
//! how much of it goes along the sum (forces pushing elsewhere count less, against it negatively),
//! so the sum must be big enough to have a direction.
//!
//! Lift follows the game's formula (`docs/re/physics-centers.md`): a surface moving through the air
//! with velocity `V` is pushed across `V`, away from the air hitting it, by
//! `PI * Radius^2 * Speed^2 * Density * Factor(Angle)`, `Angle` being between `V` and the surface
//! (`asin(|V.Normal|)`): `Factor` rises as a sine to 1 at [`PEAK_LIFT_ANGLE`] and falls back to 0 at
//! [`MAX_LIFT_ANGLE`]. Speed and density are the same for every surface, so each counts by
//! `Radius^2 * Factor`.

use crate::math::{self, Vec3};

/// `BR.FluidDynamics.PeakLiftAngle` and `MaxLiftAngle`, the game's defaults (console variables), in
/// degrees: lift is strongest at the first and gone from the second.
const PEAK_LIFT_ANGLE: f64 = 22.5;
const MAX_LIFT_ANGLE: f64 = 80.0;

/// The vehicle's forward and up (its own X and Z axes, the editor's).
const FORWARD: Vec3 = [1.0, 0.0, 0.0];
const UP: Vec3 = [0.0, 0.0, 1.0];

/// How far the nose points above the way the vehicle flies, in degrees, for the center of lift:
/// enough for flat wings to lift, far below the peak.
const ANGLE_OF_ATTACK: f64 = 5.0;

/// A sum of forces smaller than this (relative to the sum of their sizes) has no direction: they
/// cancel out, e.g. only vertical fins, or thrusters pushing against each other.
const CANCELLED: f64 = 1e-6;

/// A lift surface, in the vehicle's space.
#[derive(Clone, Copy, Debug)]
pub struct LiftSurface {
    pub location: Vec3,
    pub normal: Vec3,
    pub radius: f64,
}

/// A force and where it pushes, in the vehicle's space.
#[derive(Clone, Copy, Debug)]
pub struct Force {
    pub location: Vec3,
    pub force: Vec3,
}

/// The center of lift of `surfaces` for the vehicle flying forward with its nose
/// [`ANGLE_OF_ATTACK`] up; `None` if they lift nothing then.
pub fn center_of_lift(surfaces: &[LiftSurface]) -> Option<Vec3> {
    let (sin, cos) = ANGLE_OF_ATTACK.to_radians().sin_cos();
    // Flying slightly below where the nose points.
    let velocity = math::sub(math::scale(FORWARD, cos), math::scale(UP, sin));
    let forces: Vec<Force> = surfaces
        .iter()
        .filter_map(|surface| {
            Some(Force {
                location: surface.location,
                force: lift(surface, velocity)?,
            })
        })
        .collect();
    center_of(&forces)
}

/// The lift on `surface` moving with `velocity` (unit length), up to the factor every surface
/// shares; `None` if it lifts nothing.
fn lift(surface: &LiftSurface, velocity: Vec3) -> Option<Vec3> {
    let normal = math::normalized(surface.normal)?;
    let along = math::dot(velocity, normal);
    let factor = lift_factor(along.abs().min(1.0).asin().to_degrees());
    if factor <= 0.0 {
        return None;
    }
    // Across the velocity, toward the side the surface moves away from.
    let across = math::sub(normal, math::scale(velocity, along));
    let direction = math::normalized(math::scale(across, -along.signum()))?;
    Some(math::scale(
        direction,
        surface.radius * surface.radius * factor,
    ))
}

/// The game's lift factor at `angle` degrees between the velocity and the surface.
fn lift_factor(angle: f64) -> f64 {
    if angle <= 0.0 || angle >= MAX_LIFT_ANGLE {
        0.0
    } else if angle < PEAK_LIFT_ANGLE {
        (angle / PEAK_LIFT_ANGLE * core::f64::consts::FRAC_PI_2).sin()
    } else {
        let past = (angle - PEAK_LIFT_ANGLE) / (MAX_LIFT_ANGLE - PEAK_LIFT_ANGLE);
        ((past * core::f64::consts::PI).cos() + 1.0) / 2.0
    }
}

/// Where the sum of `forces` acts; `None` if they cancel out.
pub fn center_of(forces: &[Force]) -> Option<Vec3> {
    let sum = forces
        .iter()
        .fold([0.0; 3], |sum, force| math::add(sum, force.force));
    let sizes: f64 = forces.iter().map(|force| math::length(force.force)).sum();
    let total = math::length(sum);
    if total <= sizes * CANCELLED || total == 0.0 {
        return None;
    }
    let direction = math::scale(sum, 1.0 / total);
    // Each force's share along the sum; the shares add up to `total`.
    let weighted = forces.iter().fold([0.0; 3], |weighted, force| {
        let share = math::dot(force.force, direction);
        math::add(weighted, math::scale(force.location, share))
    });
    Some(math::scale(weighted, 1.0 / total))
}
