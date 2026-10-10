//! The "Duplicates" view mode: bricks sharing their place with copies of themselves show red, the
//! deeper the more copies, relative to the most copies at one place in the vehicle (see
//! [`shade`]); the others gray. Copies are bricks of the same type, location, rotation and size,
//! whatever their color or other properties.
//!
//! How (`game::editor_object`): bricks are grouped by type, location and rotation as the editor
//! sees them, rounded to [`PLACE`] and [`TURN`] (so two bricks a hair apart can, rarely, round
//! apart); bricks of a group are then told apart by size: their box in their own space, as each
//! brick type reports it, so no brick type's properties are known here.

use std::collections::HashMap;

use brickrust::ue::coreuobject::UObjectBase;

use super::{Coloring, Editor, Painting};
use crate::game::editor_object::{LocalBounds, Places, Vector};

/// The colors, free to change (see `Colors`): bricks with one copy, and bricks with the most copies
/// in the vehicle; [`shade`] blends between them.
const FEWEST: [f32; 3] = [1.0, 0.45, 0.45];
const MOST: [f32; 3] = [1.0, 0.0, 0.0];

/// How close bricks count as at the same place, in centimeters per coordinate of their locations
/// and sizes.
const PLACE: f32 = 0.01;
/// How close their rotations count as the same, per coordinate of their rotated axes (0.001 is
/// about 0.06°).
const TURN: f32 = 0.001;

/// The color of a brick with `copies` copies at its place, when the most copies a brick has in the
/// vehicle is `most` (both at least 1). Free to change.
fn shade(copies: usize, most: usize) -> [f32; 3] {
    // How far from fewest to most, on a log scale: 1, 10, 100, ... copies are evenly apart.
    let share = if most > 1 {
        (copies as f32).ln() / (most as f32).ln()
    } else {
        1.0
    };
    // Blended as a color picker's values (sRGB), where halfway looks halfway; blending the linear
    // values themselves keeps most shades close to the paler end.
    core::array::from_fn(|channel| {
        let [from, to] = [FEWEST[channel], MOST[channel]].map(to_srgb);
        to_linear(from + (to - from) * share)
    })
}

/// A linear color value as a color picker's (sRGB, 0 to 1): the standard sRGB curve.
fn to_srgb(linear: f32) -> f32 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

/// The inverse of [`to_srgb`].
fn to_linear(srgb: f32) -> f32 {
    if srgb <= 0.040_45 {
        srgb / 12.92
    } else {
        ((srgb + 0.055) / 1.055).powf(2.4)
    }
}

struct Duplicates {
    places: Places,
    bounds: LocalBounds,
}

pub fn new() -> Result<Box<dyn Coloring>, String> {
    Ok(Box::new(Duplicates {
        places: Places::find()?,
        bounds: LocalBounds::find()?,
    }))
}

impl Coloring for Duplicates {
    unsafe fn paint(&self, editor: &Editor) -> Painting {
        // SAFETY: per the contract; the editor's bricks are live.
        let bricks = unsafe { editor.brick_list() };
        // SAFETY: as above.
        let copies = unsafe { self.copies(&bricks) };
        let most = copies.iter().map(|&(_, count)| count).max().unwrap_or(0);
        let summary = format!(
            "of {} bricks, {} share their place with copies, up to {most} copies",
            bricks.len(),
            copies.len(),
        );
        let colors = copies
            .into_iter()
            .map(|(brick, count)| {
                let color = shade(count, most);
                (brick as usize, [color, color])
            })
            .collect();
        Painting { colors, summary }
    }
}

impl Duplicates {
    /// The bricks of `bricks` with copies, and how many each has.
    ///
    /// # Safety
    /// `bricks` must be live bricks in the editor. Game thread only.
    unsafe fn copies(&self, bricks: &[*mut UObjectBase]) -> Vec<(*mut UObjectBase, usize)> {
        let mut places: HashMap<_, Vec<_>> = HashMap::new();
        for &brick in bricks {
            // SAFETY: per the contract.
            if let Some(place) = unsafe { self.places.of(brick) } {
                let key = (
                    place.class as usize,
                    round(place.location, PLACE),
                    round(axes(place.rotation), TURN),
                );
                places.entry(key).or_default().push(brick);
            }
        }
        let mut copies = Vec::new();
        // Then by size, only where it matters: one more call per brick.
        for group in places.values().filter(|group| group.len() > 1) {
            let mut sizes: HashMap<_, Vec<_>> = HashMap::new();
            for &brick in group {
                // SAFETY: per the contract.
                let size = unsafe { self.bounds.of(brick) }
                    .map(|[min, max]| (round(min, PLACE), round(max, PLACE)));
                sizes.entry(size).or_default().push(brick);
            }
            for same in sizes.values().filter(|same| same.len() > 1) {
                copies.extend(same.iter().map(|&brick| (brick, same.len() - 1)));
            }
        }
        copies
    }
}

/// `values` in steps of `step`, rounded.
fn round<const N: usize>(values: [f32; N], step: f32) -> [i32; N] {
    values.map(|value| (value / step).round() as i32)
}

/// The rotated X and Y axes of `rotator` (pitch, yaw, roll in degrees), as `FRotationMatrix` makes
/// them: the same rotation written differently (pitch 180, or yaw and roll 180) gives the same
/// axes.
fn axes([pitch, yaw, roll]: Vector) -> [f32; 6] {
    let (sin_pitch, cos_pitch) = pitch.to_radians().sin_cos();
    let (sin_yaw, cos_yaw) = yaw.to_radians().sin_cos();
    let (sin_roll, cos_roll) = roll.to_radians().sin_cos();
    [
        cos_pitch * cos_yaw,
        cos_pitch * sin_yaw,
        sin_pitch,
        sin_roll * sin_pitch * cos_yaw - cos_roll * sin_yaw,
        sin_roll * sin_pitch * sin_yaw + cos_roll * cos_yaw,
        -sin_roll * cos_pitch,
    ]
}
