//! Surfaces swept along an arc, from the way the game makes a curved brick's (a spinner's)
//! collision: a chain of convex pieces, piece `i` holding the cross-section at step `i` and the one
//! at step `i + 1`, point by point (`[a0, b0, a1, b1, ...]`), the next piece starting with the
//! section this one ends with. The pieces overlap and each is filled in by itself; a swept surface
//! is the brick's outside only, as its visible mesh draws it.
//!
//! The collision uses a coarser cross-section than the visible mesh. [`reswept`] finds the arc
//! the pieces follow and sweeps the visible cross-section along it instead; [`chained`] joins the
//! pieces' own sections.

use core::f64::consts::TAU;

/// Steps of a swept surface per full turn (at least the collision's): the visible mesh has up to 32
/// over its whole angle.
const STEPS_PER_TURN: f64 = 64.0;

/// A closed triangle mesh (unless the input's sections don't make one); the triangles all face
/// the same way, inward or outward.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Loft {
    pub positions: Vec<[f32; 3]>,
    pub corners: Vec<[u32; 3]>,
}

/// The surface through the sections of `pieces`, or `None` if they aren't such a chain: every
/// piece the same even number of points (at least three per section), each piece starting where
/// the previous one ends.
pub fn chained(pieces: &[Vec<[f32; 3]>]) -> Option<Loft> {
    Some(through(sections_of(pieces)?))
}

/// The surface of `pieces` made again with the cross-section `fine` in place of `coarse`, the one
/// they were made from (both turned by `quarter_turns`, see [`turned`]), with more steps along the
/// arc. `None` if the pieces don't chain or aren't `coarse` swept along an [`Arc`].
pub fn reswept(
    pieces: &[Vec<[f32; 3]>],
    coarse: &[[f32; 2]],
    fine: &[[f32; 2]],
    quarter_turns: i32,
) -> Option<Loft> {
    if fine.len() < 3 {
        return None;
    }
    let sections = sections_of(pieces)?;
    let arc = Arc::fit(&sections, &turned(coarse, quarter_turns))?;
    let steps = ((arc.end - arc.start).abs() / TAU * STEPS_PER_TURN).ceil() as usize;
    Some(arc.sweep(&turned(fine, quarter_turns), steps.max(sections.len() - 1)))
}

/// A cross-section turned counterclockwise by `quarter_turns` quarter turns, as the game turns a
/// spinner shape's for its mirrored and rotated variants.
pub fn turned(profile: &[[f32; 2]], quarter_turns: i32) -> Vec<[f32; 2]> {
    profile
        .iter()
        .map(|&[x, y]| match quarter_turns.rem_euclid(4) {
            1 => [-y, x],
            2 => [-x, -y],
            3 => [y, -x],
            _ => [x, y],
        })
        .collect()
}

/// An elliptic arc a cross-section is swept along, as the game builds spinners: the section's
/// point `(r, z)` at angle `a` is `(-sin a (r1 + r sx), cos a (r2 + r sx), z sy)`, for angles from
/// `start` to `end` (radians).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arc {
    pub r1: f64,
    pub r2: f64,
    pub sx: f64,
    pub sy: f64,
    pub start: f64,
    pub end: f64,
}

impl Arc {
    fn place(&self, [r, z]: [f32; 2], angle: f64) -> [f32; 3] {
        let (r, z) = (f64::from(r), f64::from(z));
        [
            -angle.sin() * (self.r1 + r * self.sx),
            angle.cos() * (self.r2 + r * self.sx),
            z * self.sy,
        ]
        .map(|axis| axis as f32)
    }

    /// The angle of step `step` of `steps`.
    fn angle(&self, step: usize, steps: usize) -> f64 {
        self.start + (self.end - self.start) * step as f64 / steps.max(1) as f64
    }

    /// The arc `sections` (equal steps from the first to the last) are `profile` placed along, if
    /// they are, to within rounding.
    ///
    /// Each section's points are linear in the profile's `r`: the slope gives the section's angle
    /// and `sx`, the offset the radii; the heights give `sy`. The arc found must then place every
    /// point where it is.
    pub fn fit(sections: &[Vec<[f32; 3]>], profile: &[[f32; 2]]) -> Option<Arc> {
        let points = profile.len();
        if points < 3 || sections.len() < 2 || sections.iter().any(|s| s.len() != points) {
            return None;
        }
        let r: Vec<f64> = profile.iter().map(|p| f64::from(p[0])).collect();
        let z: Vec<f64> = profile.iter().map(|p| f64::from(p[1])).collect();
        let mean_r = r.iter().sum::<f64>() / points as f64;
        let spread_r: f64 = r.iter().map(|r| (r - mean_r).powi(2)).sum();
        let spread_z: f64 = z.iter().map(|z| z * z).sum();
        if spread_r < 1e-12 || spread_z < 1e-12 {
            return None;
        }
        // Per section: (offset, slope) of x and of y against r.
        let line = |values: &mut dyn Iterator<Item = f64>| {
            let values: Vec<f64> = values.collect();
            let mean = values.iter().sum::<f64>() / points as f64;
            let slope = r
                .iter()
                .zip(&values)
                .map(|(r, v)| (r - mean_r) * (v - mean))
                .sum::<f64>()
                / spread_r;
            (mean - slope * mean_r, slope)
        };
        let mut angles: Vec<f64> = Vec::with_capacity(sections.len());
        let (mut sx, mut sy, mut r1_sum, mut r1_weight, mut r2_sum, mut r2_weight) =
            (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        for section in sections {
            let (x_offset, x_slope) = line(&mut section.iter().map(|p| f64::from(p[0])));
            let (y_offset, y_slope) = line(&mut section.iter().map(|p| f64::from(p[1])));
            let mut angle = (-x_slope).atan2(y_slope);
            // Unwrapped: each angle within half a turn of the one before.
            if let Some(&before) = angles.last() {
                angle += ((before - angle) / TAU).round() * TAU;
            }
            angles.push(angle);
            sx += x_slope.hypot(y_slope);
            let (sin, cos) = angle.sin_cos();
            r1_sum += -sin * x_offset;
            r1_weight += sin * sin;
            r2_sum += cos * y_offset;
            r2_weight += cos * cos;
            sy += section
                .iter()
                .zip(&z)
                .map(|(p, z)| f64::from(p[2]) * z)
                .sum::<f64>()
                / spread_z;
        }
        let count = sections.len() as f64;
        // A radius only shows where the angle's sine (or cosine) does; else it's the other one.
        let (r1, r2) = match (r1_weight > 1e-6, r2_weight > 1e-6) {
            (true, true) => (r1_sum / r1_weight, r2_sum / r2_weight),
            (true, false) => (r1_sum / r1_weight, r1_sum / r1_weight),
            _ => (r2_sum / r2_weight, r2_sum / r2_weight),
        };
        let arc = Arc {
            r1,
            r2,
            sx: sx / count,
            sy: sy / count,
            start: angles[0],
            end: *angles.last()?,
        };
        let steps = sections.len() - 1;
        let extent = sections
            .iter()
            .flatten()
            .flatten()
            .fold(0.0f32, |most, axis| most.max(axis.abs()));
        let tolerance = 0.01 + 1e-4 * extent;
        let fits = sections.iter().enumerate().all(|(step, section)| {
            let angle = arc.angle(step, steps);
            section.iter().zip(profile).all(|(&actual, &point)| {
                let placed = arc.place(point, angle);
                (0..3).all(|axis| (placed[axis] - actual[axis]).abs() <= tolerance)
            })
        });
        fits.then_some(arc)
    }

    /// `profile` swept along the arc in `steps` equal steps.
    pub fn sweep(&self, profile: &[[f32; 2]], steps: usize) -> Loft {
        let steps = steps.max(1);
        let sections = (0..=steps)
            .map(|step| {
                let angle = self.angle(step, steps);
                profile
                    .iter()
                    .map(|&point| self.place(point, angle))
                    .collect()
            })
            .collect();
        through(sections)
    }
}

/// The cross-sections of a chain of pieces, in order (one more than the pieces), or `None` if they
/// don't chain.
fn sections_of(pieces: &[Vec<[f32; 3]>]) -> Option<Vec<Vec<[f32; 3]>>> {
    let points = pieces.first()?.len();
    if points / 2 < 3 || points % 2 != 0 || pieces.iter().any(|piece| piece.len() != points) {
        return None;
    }
    let start = |piece: &[[f32; 3]]| piece.iter().step_by(2).copied().collect::<Vec<_>>();
    let end = |piece: &[[f32; 3]]| piece.iter().skip(1).step_by(2).copied().collect::<Vec<_>>();
    for pair in pieces.windows(2) {
        if !same_section(&end(&pair[0]), &start(&pair[1])) {
            return None;
        }
    }
    let mut sections: Vec<Vec<[f32; 3]>> = pieces.iter().map(|piece| start(piece)).collect();
    sections.push(end(pieces.last()?));
    Some(sections)
}

/// The surface through `sections` (at least two, the same number of points each): each joined
/// point to point with the next one, and the two ends closed with a fan from their first point,
/// unless the last section comes back onto the first (a full turn).
fn through(mut sections: Vec<Vec<[f32; 3]>>) -> Loft {
    let per_section = sections[0].len();
    let full_turn = sections.len() > 2 && same_section(&sections[sections.len() - 1], &sections[0]);
    if full_turn {
        sections.pop();
    }
    let mut loft = Loft {
        positions: sections.concat(),
        corners: Vec::new(),
    };
    let index = |section: usize, point: usize| (section * per_section + point) as u32;
    let joins = if full_turn {
        sections.len()
    } else {
        sections.len() - 1
    };
    for section in 0..joins {
        let next_section = (section + 1) % sections.len();
        for point in 0..per_section {
            let next_point = (point + 1) % per_section;
            let (a, b) = (index(section, point), index(section, next_point));
            let (c, d) = (index(next_section, next_point), index(next_section, point));
            loft.push([a, b, c]);
            loft.push([a, c, d]);
        }
    }
    if !full_turn {
        let last = sections.len() - 1;
        for point in 1..per_section - 1 {
            // Opposite ways round at the two ends, so both face the way the sides do.
            loft.push([index(0, 0), index(0, point + 1), index(0, point)]);
            loft.push([index(last, 0), index(last, point), index(last, point + 1)]);
        }
    }
    loft
}

impl Loft {
    /// Adds a triangle, unless it has no area (a section's points can repeat, e.g. at a corner
    /// with two normals).
    fn push(&mut self, triangle: [u32; 3]) {
        let [a, b, c] = triangle.map(|i| self.positions[i as usize]);
        let (ab, ac) = (sub(b, a), sub(c, a));
        let normal = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        let area_squared = normal.iter().map(|axis| axis * axis).sum::<f32>();
        if area_squared > 1e-12 {
            self.corners.push(triangle);
        }
    }
}

/// Whether two sections are the same points, up to rounding (the game computes each twice).
fn same_section(a: &[[f32; 3]], b: &[[f32; 3]]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            (0..3).all(|axis| {
                let tolerance = 1e-3 + 1e-5 * a[axis].abs().max(b[axis].abs());
                (a[axis] - b[axis]).abs() <= tolerance
            })
        })
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const SQUARE: [[f32; 2]; 4] = [[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]];

    /// An octagon around the square's center, the "visible" cross-section in these tests.
    fn octagon() -> Vec<[f32; 2]> {
        (0..8)
            .map(|i| {
                let a = core::f32::consts::TAU * i as f32 / 8.0;
                [0.5 * a.cos(), 0.5 * a.sin()]
            })
            .collect()
    }

    /// The game's pieces for `profile` along `arc` in `steps` steps.
    fn pieces(arc: &Arc, profile: &[[f32; 2]], steps: usize) -> Vec<Vec<[f32; 3]>> {
        (0..steps)
            .map(|step| {
                let (a, b) = (arc.angle(step, steps), arc.angle(step + 1, steps));
                profile
                    .iter()
                    .flat_map(|&point| [arc.place(point, a), arc.place(point, b)])
                    .collect()
            })
            .collect()
    }

    fn ring(steps: usize, angle: f64) -> Vec<Vec<[f32; 3]>> {
        let arc = Arc {
            r1: 105.0,
            r2: 105.0,
            sx: 10.0,
            sy: 10.0,
            start: 0.0,
            end: angle,
        };
        pieces(&arc, &SQUARE, steps)
    }

    /// Whether every edge is used once each way: a closed surface, all facing the same way.
    fn is_closed(loft: &Loft) -> bool {
        let mut edges: HashMap<(u32, u32), i32> = HashMap::new();
        for &[a, b, c] in &loft.corners {
            for (from, to) in [(a, b), (b, c), (c, a)] {
                *edges.entry((from, to)).or_default() += 1;
            }
        }
        edges
            .iter()
            .all(|(&(from, to), &count)| count == 1 && edges.get(&(to, from)) == Some(&1))
    }

    #[test]
    fn an_arc_is_closed_with_its_ends() {
        let loft = chained(&ring(4, 1.0)).unwrap();
        // 5 sections of 4 points; 4 joins of 4 quads, and 2 ends of 2 triangles.
        assert_eq!(loft.positions.len(), 20);
        assert_eq!(loft.corners.len(), 4 * 4 * 2 + 2 * 2);
        assert!(is_closed(&loft));
    }

    #[test]
    fn a_full_turn_has_no_ends() {
        let loft = chained(&ring(8, TAU)).unwrap();
        assert_eq!(loft.positions.len(), 8 * 4);
        assert_eq!(loft.corners.len(), 8 * 4 * 2);
        assert!(is_closed(&loft));
    }

    #[test]
    fn repeated_points_make_no_triangles() {
        // Every corner of the square twice, as for a mesh with hard edges.
        let pieces: Vec<Vec<[f32; 3]>> = ring(2, 1.0)
            .into_iter()
            .map(|piece| {
                piece
                    .chunks(2)
                    .flat_map(|pair| [pair[0], pair[1], pair[0], pair[1]])
                    .collect()
            })
            .collect();
        let loft = chained(&pieces).unwrap();
        assert_eq!(loft.corners.len(), 2 * 4 * 2 + 2 * 2);
        for &[a, b, c] in &loft.corners {
            assert!(a != b && b != c && c != a);
        }
    }

    #[test]
    fn pieces_that_dont_chain_are_refused() {
        let mut pieces = ring(3, 1.0);
        pieces.swap(0, 2);
        assert_eq!(chained(&pieces), None);
        // Odd point counts, sections too small, or pieces of different sizes.
        assert_eq!(chained(&[vec![[0.0; 3]; 7]]), None);
        assert_eq!(chained(&[vec![[0.0; 3]; 4]]), None);
        let mut pieces = ring(2, 1.0);
        pieces[1].truncate(6);
        assert_eq!(chained(&pieces), None);
        assert_eq!(chained(&[]), None);
    }

    #[test]
    fn quarter_turns_go_counterclockwise() {
        let point = [[2.0, 1.0]];
        assert_eq!(turned(&point, 0), vec![[2.0, 1.0]]);
        assert_eq!(turned(&point, 1), vec![[-1.0, 2.0]]);
        assert_eq!(turned(&point, 2), vec![[-2.0, -1.0]]);
        assert_eq!(turned(&point, 3), vec![[1.0, -2.0]]);
        assert_eq!(turned(&point, -1), turned(&point, 3));
    }

    #[test]
    fn the_arc_is_found_from_the_pieces() {
        // An elliptic arc of 120 degrees, as the game's 1 x 1.1 m spinner of radii 0.8 and 0.9 m.
        let arc = Arc {
            r1: 80.0,
            r2: 90.0,
            sx: 100.0,
            sy: 110.0,
            start: 0.0,
            end: 2.094,
        };
        let profile = turned(&SQUARE, 1);
        let found = Arc::fit(&sections_of(&pieces(&arc, &profile, 5)).unwrap(), &profile).unwrap();
        for (found, wanted) in [
            (found.r1, arc.r1),
            (found.r2, arc.r2),
            (found.sx, arc.sx),
            (found.sy, arc.sy),
            (found.start, arc.start),
            (found.end, arc.end),
        ] {
            assert!((found - wanted).abs() < 1e-3, "{found} vs {wanted}");
        }
    }

    #[test]
    fn reswept_follows_the_arc_with_the_fine_section() {
        let arc = Arc {
            r1: 80.0,
            r2: 90.0,
            sx: 100.0,
            sy: 110.0,
            start: 0.0,
            end: -2.094,
        };
        let pieces = pieces(&arc, &turned(&SQUARE, 3), 4);
        let loft = reswept(&pieces, &SQUARE, &octagon(), 3).unwrap();
        // 120 degrees at 64 steps a turn: 22 steps, each an octagon.
        assert_eq!(loft.positions.len(), 23 * 8);
        let wanted = arc.sweep(&turned(&octagon(), 3), 22);
        assert_eq!(loft.corners, wanted.corners);
        for (found, wanted) in loft.positions.iter().zip(&wanted.positions) {
            assert!((0..3).all(|axis| (found[axis] - wanted[axis]).abs() < 1e-2));
        }
        assert!(is_closed(&loft));
    }

    #[test]
    fn pieces_of_another_section_arent_reswept() {
        let arc = Arc {
            r1: 80.0,
            r2: 80.0,
            sx: 100.0,
            sy: 100.0,
            start: 0.0,
            end: 1.0,
        };
        let pieces = pieces(&arc, &SQUARE, 4);
        let flatter = SQUARE.map(|[x, y]| [x, y * 0.5 + 0.1 * x]);
        assert_eq!(reswept(&pieces, &flatter, &octagon(), 0), None);
        // The same section turned another way doesn't fit either.
        assert_eq!(reswept(&pieces, &SQUARE, &octagon(), 1), None);
    }
}
