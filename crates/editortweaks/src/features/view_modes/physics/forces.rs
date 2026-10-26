//! Lift and drag for one airflow, from plain data: each brick's push ([`aero`]), their sums where
//! they push ([`middle_of`]), and the arrows showing them. Everything is per unit of
//! `density * speed^2` (cm²) until it's told in newtons ([`newtons`]).

use super::aero::{self, Coefficients, Push, Surface};
use super::airflow::Ball;
use super::arrows::Arrow;
use super::centers::Force;
use crate::math::{self, Vec3};

/// Forces smaller than this share of their scale's biggest ([`Scale`]) get no arrow: they'd be dots.
const TOO_SMALL: f64 = 0.01;

/// What the forces are worked out from: the lift surfaces and thrusters of the bricks counted (the
/// selection's, if anything is selected), each surface with whether to show its own arrows.
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    pub surfaces: Vec<(Surface, bool)>,
    /// Thrusters at full throttle (kg cm/s²).
    pub thrust: Vec<Force>,
}

/// The forces for one airflow.
#[derive(Clone, Debug, Default)]
pub struct Forces {
    /// Each shown surface's push, where it acts.
    pub bricks: Vec<(Vec3, Push)>,
    /// The sums, where they push ([`middle_of`]); `None` if there's none.
    pub lift: Option<Force>,
    pub drag: Option<Force>,
    /// The thrusters' push along the way the vehicle moves (kg cm/s²).
    pub thrust: f64,
}

/// Works out the forces of `inputs` for the vehicle moving `direction` (unit length).
pub fn forces(inputs: &Inputs, direction: Vec3, coefficients: &Coefficients) -> Forces {
    let mut bricks = Vec::new();
    let (mut lifts, mut drags) = (Vec::new(), Vec::new());
    for (surface, shown) in &inputs.surfaces {
        let push = aero::push(surface, direction, coefficients);
        if *shown {
            bricks.push((surface.location, push));
        }
        lifts.push(Force {
            location: surface.location,
            force: push.lift,
        });
        drags.push(Force {
            location: surface.location,
            force: push.drag,
        });
    }
    let total = |forces: &[Force]| {
        let sum = forces
            .iter()
            .fold([0.0; 3], |sum, force| math::add(sum, force.force));
        Some(Force {
            location: middle_of(forces)?,
            force: sum,
        })
    };
    Forces {
        bricks,
        lift: total(&lifts),
        drag: total(&drags),
        thrust: inputs
            .thrust
            .iter()
            .map(|push| math::dot(push.force, direction))
            .sum(),
    }
}

/// Where `forces` push, together: their locations weighted by their sizes, so always among them;
/// `None` if they're all nothing.
///
/// For drag (all along the airflow) that's where their sum acts. Lift points every which way (fins
/// sideways, wings up or down), and where the sum of such forces acts lies anywhere along a line,
/// far outside the vehicle when they nearly cancel out: this is where a single arrow can stand for
/// them. Where lift acts for stability is what the centers of lift tell (`super::centers`).
pub fn middle_of(forces: &[Force]) -> Option<Vec3> {
    let (weighted, total) = forces
        .iter()
        .fold(([0.0; 3], 0.0), |(weighted, total), force| {
            let size = math::length(force.force);
            (
                math::add(weighted, math::scale(force.location, size)),
                total + size,
            )
        });
    (total > 0.0).then(|| math::scale(weighted, 1.0 / total))
}

/// What an arrow shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shows {
    /// A brick's lift or drag (its index in [`Forces::bricks`]).
    BrickLift(usize),
    BrickDrag(usize),
    /// The sums.
    Lift,
    Drag,
}

impl Shows {
    pub fn is_lift(self) -> bool {
        matches!(self, Shows::BrickLift(_) | Shows::Lift)
    }
}

/// How many airflows [`Scale::of`] tries for the biggest sums, spread over every way (besides the
/// vehicle's axes). Between two of them the sums can be a few percent bigger still: those arrows
/// come out a little longer than the longest.
const TRIED_AIRFLOWS: usize = 512;

/// The biggest forces some inputs get from any airflow: the arrows are to these scales, so their
/// lengths compare between airflows (turned from the front to the side, drag grows).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scale {
    /// A brick's lift or drag.
    pub brick: f64,
    /// The sum of the lift or of the drag.
    pub sum: f64,
}

impl Scale {
    /// The scales for `inputs` (only the shown bricks count for [`Scale::brick`]).
    pub fn of(inputs: &Inputs, coefficients: &Coefficients) -> Self {
        // A surface's lift and drag are strongest at the peak lift angle and face-on.
        let brick = inputs
            .surfaces
            .iter()
            .filter(|(_, shown)| *shown)
            .map(|(surface, _)| {
                let disk = core::f64::consts::PI * surface.radius * surface.radius;
                let edge_on = coefficients.cross_section * surface.cross_section;
                (coefficients.lift * disk).max(coefficients.drag * disk.max(edge_on))
            })
            .fold(0.0, f64::max);

        let surfaces = alike(inputs.surfaces.iter().map(|(surface, _)| surface));
        let axes = (0..3).map(|axis| core::array::from_fn(|index| f64::from(index == axis)));
        // Lift and drag only turn around for the air coming the other way: half the ways do.
        let sum = axes
            .chain(spread(TRIED_AIRFLOWS))
            .map(|direction| {
                let (lift, drag) = surfaces.iter().fold(([0.0; 3], [0.0; 3]), |sums, surface| {
                    let push = aero::push(surface, direction, coefficients);
                    (math::add(sums.0, push.lift), math::add(sums.1, push.drag))
                });
                math::length(lift).max(math::length(drag))
            })
            .fold(0.0, f64::max);
        Self { brick, sum }
    }
}

/// `surfaces` with those facing alike (either way: a surface lifts and drags the same turned over)
/// merged into one, their faces and cross sections added up, located nowhere: the same sums of
/// lift and drag, worked out once per way bricks face rather than per brick.
fn alike<'a>(surfaces: impl Iterator<Item = &'a Surface>) -> Vec<Surface> {
    let mut merged: Vec<(Surface, f64)> = Vec::new();
    let mut by_facing = std::collections::HashMap::new();
    for surface in surfaces {
        let Some(mut normal) = math::normalized(surface.normal) else {
            continue;
        };
        // Turned over so that its first clear component is positive.
        if normal
            .iter()
            .find(|part| part.abs() > 1e-6)
            .is_some_and(|&part| part < 0.0)
        {
            normal = math::scale(normal, -1.0);
        }
        let key = normal.map(|part| (part * 1000.0).round() as i32);
        let disk = surface.radius * surface.radius;
        let index = *by_facing.entry(key).or_insert_with(|| {
            let empty = Surface {
                location: [0.0; 3],
                normal,
                radius: 0.0,
                cross_section: 0.0,
            };
            merged.push((empty, 0.0));
            merged.len() - 1
        });
        let (into, disks) = &mut merged[index];
        *disks += disk;
        into.cross_section += surface.cross_section;
    }
    merged
        .into_iter()
        .map(|(surface, disks)| Surface {
            radius: disks.sqrt(),
            ..surface
        })
        .collect()
}

/// `count` unit directions spread evenly over a half of the ball (a Fibonacci spiral).
fn spread(count: usize) -> impl Iterator<Item = Vec3> {
    let golden_angle = core::f64::consts::PI * (3.0 - 5f64.sqrt());
    (0..count).map(move |index| {
        let z = (index as f64 + 0.5) / count as f64;
        let across = (1.0 - z * z).sqrt();
        let (sin, cos) = (index as f64 * golden_angle).sin_cos();
        [across * cos, across * sin, z]
    })
}

/// The arrows showing `forces` on a vehicle within `ball`: each brick's lift and drag to one scale,
/// the sums to another (both proportioned to the vehicle, their longest for the biggest forces of
/// `scale`), small ones left out. The sums start where they push, or `together` both from the middle
/// of the two (weighted by their sizes, so nearer the bigger one: where the air pushes the vehicle
/// overall), for comparing them.
pub fn arrows(forces: &Forces, scale: &Scale, ball: &Ball, together: bool) -> Vec<(Shows, Arrow)> {
    let radius = ball.radius;
    let mut arrows = Vec::new();
    let mut add = |shows, location: Vec3, force: Vec3, longest: f64, biggest: f64, thickness| {
        let size = math::length(force);
        if biggest <= 0.0 || size < biggest * TOO_SMALL {
            return;
        }
        let tip = math::add(location, math::scale(force, longest / biggest));
        arrows.push((
            shows,
            Arrow {
                tail: location,
                tip,
                thickness,
            },
        ));
    };

    let biggest = scale.brick;
    let (longest, thickness) = (
        (0.35 * radius).clamp(40.0, 250.0),
        (0.015 * radius).clamp(2.0, 8.0),
    );
    for (index, (location, push)) in forces.bricks.iter().enumerate() {
        add(
            Shows::BrickLift(index),
            *location,
            push.lift,
            longest,
            biggest,
            thickness,
        );
        add(
            Shows::BrickDrag(index),
            *location,
            push.drag,
            longest,
            biggest,
            thickness,
        );
    }

    let biggest = scale.sum;
    let (longest, thickness) = (
        (0.8 * radius).clamp(80.0, 600.0),
        (0.03 * radius).clamp(5.0, 16.0),
    );
    let sums: Vec<Force> = [forces.lift, forces.drag].into_iter().flatten().collect();
    let shared = if together { middle_of(&sums) } else { None };
    for (shows, force) in [(Shows::Lift, forces.lift), (Shows::Drag, forces.drag)] {
        if let Some(force) = force {
            add(
                shows,
                shared.unwrap_or(force.location),
                force.force,
                longest,
                biggest,
                thickness,
            );
        }
    }
    arrows
}

/// A force of `per_unit` (cm², per unit of `density * speed^2`) in newtons, at `density` (kg/cm³)
/// and `speed` (cm/s): kg cm/s² are a hundredth of a newton.
pub fn newtons(per_unit: f64, density: f64, speed: f64) -> f64 {
    per_unit * density * speed * speed / 100.0
}

/// The speed (cm/s) at which `thrust` (kg cm/s²) meets a drag of `drag` (cm², per unit of
/// `density * speed^2`) at `density` (kg/cm³); `None` without thrust, drag or air.
pub fn top_speed(thrust: f64, drag: f64, density: f64) -> Option<f64> {
    (thrust > 0.0 && drag > 0.0 && density > 0.0).then(|| (thrust / (drag * density)).sqrt())
}
