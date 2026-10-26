//! Where the forces on a vehicle act, from plain data: the centers of lift for a vehicle turning
//! away from the way it flies (one per axis), and the center of any set of forces (thrust, drag).
//! Each is where the forces' sum acts, each force counted by how much of it goes along the sum
//! (forces pushing elsewhere count less, against it negatively), so the sum must be big enough to
//! have a direction.
//!
//! Lift follows the game's formula ([`aero`]). Speed and density are the same for every surface, so
//! each counts by its push per unit of both.
//!
//! Stability is about the lift that *changes* when the nose turns away from the airflow: behind the
//! center of mass it turns the nose back. Pitch and yaw are separate (a plane can be stable in one
//! and not the other), so each axis gets its own center: the vehicle flown with its nose
//! [`TURN_ANGLE`] up minus down (pitch: wings, tailplanes), or left minus right (yaw: fins). Lift
//! that stays the same both ways (a wing's incidence, wings in a side slip) cancels out, so each
//! center only counts the surfaces working on its axis: what aircraft designers call the neutral
//! point.

use super::aero::{self, Coefficients, Surface};
use crate::math::{self, Vec3};

/// The vehicle's forward, right and up (its own X, Y and Z axes, the editor's).
pub const FORWARD: Vec3 = [1.0, 0.0, 0.0];
const RIGHT: Vec3 = [0.0, 1.0, 0.0];
const UP: Vec3 = [0.0, 0.0, 1.0];

/// How far the nose turns away from the way the vehicle flies, in degrees, for the centers of lift:
/// enough for flat surfaces to lift, far below the peak.
const TURN_ANGLE: f64 = 5.0;

/// A sum of forces smaller than this (relative to the sum of their sizes) has no direction: they
/// cancel out, e.g. only vertical fins, or thrusters pushing against each other.
const CANCELLED: f64 = 1e-6;

/// A force and where it pushes, in the vehicle's space.
#[derive(Clone, Copy, Debug)]
pub struct Force {
    pub location: Vec3,
    pub force: Vec3,
}

/// The axis the nose turns around, for a center of lift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Nose up and down: horizontal surfaces.
    Pitch,
    /// Nose left and right: vertical surfaces.
    Yaw,
}

impl Axis {
    /// The way the vehicle flies (unit length) with its nose turned `degrees` around this axis: up
    /// for pitch, left for yaw, so flying a little below or right of where the nose points.
    fn velocity(self, degrees: f64) -> Vec3 {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let side = match self {
            Axis::Pitch => math::scale(UP, -sin),
            Axis::Yaw => math::scale(RIGHT, sin),
        };
        math::add(math::scale(FORWARD, cos), side)
    }
}

/// The center of lift of `surfaces` around `axis`: where the lift changing as the nose turns
/// [`TURN_ANGLE`] one way and the other acts; `None` if no surface works on that axis.
pub fn center_of_lift(
    surfaces: &[Surface],
    axis: Axis,
    coefficients: &Coefficients,
) -> Option<Vec3> {
    let (one_way, other_way) = (axis.velocity(TURN_ANGLE), axis.velocity(-TURN_ANGLE));
    let forces: Vec<Force> = surfaces
        .iter()
        .filter_map(|surface| {
            let lift = |velocity| aero::lift(surface, velocity, coefficients).unwrap_or_default();
            let change = math::sub(lift(one_way), lift(other_way));
            (change != [0.0; 3]).then_some(Force {
                location: surface.location,
                force: change,
            })
        })
        .collect();
    center_of(&forces)
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
