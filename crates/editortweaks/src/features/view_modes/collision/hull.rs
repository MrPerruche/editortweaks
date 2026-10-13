//! Convex hulls of points: the faces of a collision hull, whose points are all the game keeps
//! (PhysX makes the faces when cooking).
//!
//! Incremental: start from four points spanning a tetrahedron, then add each point outside the
//! hull so far, replacing the faces it sees with faces from it to their outline (the horizon).
//! Points within a tolerance of a face count as inside, so flat sides come out as flat triangles
//! without slivers. Quadratic in the number of points: collision hulls have at most 256.

use std::collections::HashSet;

use crate::math::{self, Vec3};

/// A hull's triangle: three indices into the points, counterclockwise seen from outside (its
/// normal, `(b - a) x (c - a)`, points out).
pub type Triangle = [usize; 3];

/// Distances below this fraction of the points' extent count as zero.
const TOLERANCE: f64 = 1e-6;

/// The faces of the convex hull of `points`, or `None` if they're all in a plane (no volume).
pub fn convex_hull(points: &[Vec3]) -> Option<Vec<Triangle>> {
    let extent = points
        .iter()
        .flat_map(|point| point.iter())
        .fold(0.0f64, |largest, value| largest.max(value.abs()));
    let tolerance = extent.max(1.0) * TOLERANCE;
    let start = tetrahedron(points, tolerance)?;

    let inside = math::scale(
        start
            .iter()
            .fold([0.0; 3], |sum, &index| math::add(sum, points[index])),
        0.25,
    );
    let [a, b, c, d] = start;
    let mut faces: Vec<Face> = [[a, b, c], [a, b, d], [a, c, d], [b, c, d]]
        .into_iter()
        .filter_map(|corners| Face::new(points, corners, inside))
        .collect();
    if faces.len() != 4 {
        return None;
    }

    for (index, &point) in points.iter().enumerate() {
        if start.contains(&index) {
            continue;
        }
        let seen: Vec<bool> = faces
            .iter()
            .map(|face| face.distance(point) > tolerance)
            .collect();
        if !seen.contains(&true) {
            continue;
        }
        // The horizon: edges of seen faces whose other face isn't seen. Each edge is stored in the
        // direction its face goes around it; its neighbor goes around it the other way.
        let seen_edges: HashSet<(usize, usize)> = faces
            .iter()
            .zip(&seen)
            .filter(|(_, seen)| **seen)
            .flat_map(|(face, _)| face.edges())
            .collect();
        let horizon: Vec<(usize, usize)> = seen_edges
            .iter()
            .copied()
            .filter(|&(from, to)| !seen_edges.contains(&(to, from)))
            .collect();
        let mut kept: Vec<Face> = faces
            .into_iter()
            .zip(seen)
            .filter(|(_, seen)| !seen)
            .map(|(face, _)| face)
            .collect();
        // Each new face keeps its edge's direction, so it faces out like the face it replaces.
        kept.extend(
            horizon
                .into_iter()
                .filter_map(|(from, to)| Face::new(points, [from, to, index], inside)),
        );
        faces = kept;
    }
    Some(faces.into_iter().map(|face| face.corners).collect())
}

/// Four points spanning a tetrahedron: the extreme X, the farthest from it, the farthest from the
/// line through both, the farthest from their plane.
fn tetrahedron(points: &[Vec3], tolerance: f64) -> Option<[usize; 4]> {
    let farthest = |distance: &dyn Fn(Vec3) -> f64| {
        points
            .iter()
            .enumerate()
            .map(|(index, &point)| (index, distance(point)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
    };
    let a = farthest(&|point| -point[0])?.0;
    let (b, distance) = farthest(&|point| math::length(math::sub(point, points[a])))?;
    if distance <= tolerance {
        return None;
    }
    let along = math::normalized(math::sub(points[b], points[a]))?;
    let (c, distance) =
        farthest(&|point| math::length(math::cross(math::sub(point, points[a]), along)))?;
    if distance <= tolerance {
        return None;
    }
    let normal = math::normalized(math::cross(
        math::sub(points[b], points[a]),
        math::sub(points[c], points[a]),
    ))?;
    let (d, distance) = farthest(&|point| math::dot(math::sub(point, points[a]), normal).abs())?;
    (distance > tolerance).then_some([a, b, c, d])
}

/// A face of the hull being built, and its plane.
struct Face {
    corners: Triangle,
    normal: Vec3,
    /// The plane's distance from the origin along `normal`.
    offset: f64,
}

impl Face {
    /// The face through `corners`, turned to face away from `inside`; `None` if they're in a line.
    fn new(points: &[Vec3], corners: Triangle, inside: Vec3) -> Option<Self> {
        let [a, b, c] = corners.map(|corner| points[corner]);
        let normal = math::normalized(math::cross(math::sub(b, a), math::sub(c, a)))?;
        let face = Self {
            corners,
            normal,
            offset: math::dot(normal, a),
        };
        if face.distance(inside) > 0.0 {
            let [a, b, c] = corners;
            return Some(Self {
                corners: [a, c, b],
                normal: math::scale(normal, -1.0),
                offset: -face.offset,
            });
        }
        Some(face)
    }

    /// How far `point` is in front of the face (negative behind it).
    fn distance(&self, point: Vec3) -> f64 {
        math::dot(self.normal, point) - self.offset
    }

    /// Its edges, in the direction it goes around them.
    fn edges(&self) -> [(usize, usize); 3] {
        let [a, b, c] = self.corners;
        [(a, b), (b, c), (c, a)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hull's volume (from its faces, by the divergence theorem) and whether every face
    /// faces out.
    fn volume(points: &[Vec3], faces: &[Triangle]) -> f64 {
        faces
            .iter()
            .map(|&[a, b, c]| math::dot(points[a], math::cross(points[b], points[c])) / 6.0)
            .sum()
    }

    fn cube() -> Vec<Vec3> {
        let mut points = Vec::new();
        for x in [-1.0, 1.0] {
            for y in [-2.0, 2.0] {
                for z in [-3.0, 3.0] {
                    points.push([x, y, z]);
                }
            }
        }
        points
    }

    #[test]
    fn box_has_twelve_triangles_and_its_volume() {
        let points = cube();
        let faces = convex_hull(&points).expect("a hull");
        assert_eq!(faces.len(), 12);
        assert!((volume(&points, &faces) - 2.0 * 4.0 * 6.0).abs() < 1e-9);
    }

    #[test]
    fn inner_and_face_points_are_ignored() {
        let mut points = cube();
        points.push([0.0, 0.0, 0.0]);
        points.push([0.5, 0.5, 3.0]);
        points.push([1.0, 0.0, 0.0]);
        let faces = convex_hull(&points).expect("a hull");
        assert!((volume(&points, &faces) - 48.0).abs() < 1e-9);
        assert!(faces.iter().flatten().all(|&corner| corner < 8));
    }

    #[test]
    fn faces_point_out() {
        let points = cube();
        let faces = convex_hull(&points).expect("a hull");
        for &[a, b, c] in &faces {
            let normal = math::cross(
                math::sub(points[b], points[a]),
                math::sub(points[c], points[a]),
            );
            let center = math::scale(
                math::add(math::add(points[a], points[b]), points[c]),
                1.0 / 3.0,
            );
            assert!(math::dot(normal, center) > 0.0);
        }
    }

    #[test]
    fn flat_points_have_no_hull() {
        let points = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
        ];
        assert!(convex_hull(&points).is_none());
        assert!(convex_hull(&[]).is_none());
    }

    #[test]
    fn round_points_make_a_closed_hull() {
        // Points on a sphere: every point is a corner, and the faces close up (each edge twice,
        // once each way).
        let mut points = Vec::new();
        for ring in 1..8 {
            let latitude = std::f64::consts::PI * f64::from(ring) / 8.0;
            for step in 0..12 {
                let longitude = std::f64::consts::TAU * f64::from(step) / 12.0;
                points.push([
                    latitude.sin() * longitude.cos(),
                    latitude.sin() * longitude.sin(),
                    latitude.cos(),
                ]);
            }
        }
        points.push([0.0, 0.0, 1.0]);
        points.push([0.0, 0.0, -1.0]);
        let faces = convex_hull(&points).expect("a hull");
        let edges: HashSet<(usize, usize)> = faces
            .iter()
            .flat_map(|&[a, b, c]| [(a, b), (b, c), (c, a)])
            .collect();
        assert_eq!(edges.len(), faces.len() * 3);
        assert!(edges.iter().all(|&(a, b)| edges.contains(&(b, a))));
        // Euler: V - E + F = 2.
        let corners: HashSet<usize> = faces.iter().flatten().copied().collect();
        assert_eq!(
            corners.len() as i64 - (edges.len() / 2) as i64 + faces.len() as i64,
            2
        );
    }
}
