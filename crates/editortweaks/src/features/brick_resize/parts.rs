//! The parts of a brick's box the mouse can point at: faces, edges and corners.

use super::geometry::{self, Bounds, Frame, Ray, Vec3};

/// How close the mouse must be to a corner of a box to point at the corner, then to an edge to
/// point at the edge (pixels, inside or outside the box); further in, it points at a face.
/// Tweakable.
pub const CORNER_PIXELS: f32 = 14.0;
pub const EDGE_PIXELS: f32 = 9.0;

/// On a face, how much of its width on the screen (across an edge, where the mouse is) the zone of
/// that edge or of a corner takes at most, so that a face seen from the side, or far away, still
/// has a middle pointing at the face: 0.3 leaves at least 40% of it. Tweakable (below 0.5).
pub const PART_SHARE: f32 = 0.3;

/// A part of a box: per axis, the side it's on (-1 the smallest, 1 the largest), or 0 along the
/// axes it spans. A face has one side, an edge two, a corner three.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Part {
    pub sides: [i8; 3],
}

impl Part {
    pub fn face(axis: usize, positive: bool) -> Self {
        let mut sides = [0; 3];
        sides[axis] = if positive { 1 } else { -1 };
        Self { sides }
    }

    /// The axes it has a side on, which dragging it moves along.
    pub fn axes(self) -> impl Iterator<Item = usize> {
        (0..3).filter(move |&axis| self.sides[axis] != 0)
    }

    /// 1 for a face, 2 for an edge, 3 for a corner.
    pub fn dimensions_fixed(self) -> usize {
        self.axes().count()
    }

    /// Its corners: a face's four in order around it, an edge's two ends, or the corner.
    pub fn corners(self, bounds: &Bounds) -> Vec<Vec3> {
        let spanned: Vec<usize> = (0..3).filter(|&axis| self.sides[axis] == 0).collect();
        // Around a face: (-, -), (+, -), (+, +), (-, +) along the two axes it spans.
        let around: &[[i8; 2]] = match spanned.len() {
            0 => &[[0, 0]],
            1 => &[[-1, 0], [1, 0]],
            _ => &[[-1, -1], [1, -1], [1, 1], [-1, 1]],
        };
        around
            .iter()
            .map(|signs| {
                let mut sides = self.sides;
                for (&axis, &sign) in spanned.iter().zip(signs) {
                    sides[axis] = sign;
                }
                bounds.point(sides)
            })
            .collect()
    }
}

/// The part of a box under the mouse at `mouse` (screen pixels), given the face of the box under
/// it: one of the face's corners within [`CORNER_PIXELS`], else one of its edges within
/// [`EDGE_PIXELS`], else the face; less on a face that looks thin (see [`PART_SHARE`]). `project`
/// shows where a point of the vehicle is on the screen.
pub fn part_under(
    frame: &Frame,
    bounds: &Bounds,
    face: Part,
    mouse: [f32; 2],
    project: impl Fn(Vec3) -> Option<[f32; 2]>,
) -> Part {
    if face.dimensions_fixed() != 1 {
        return face;
    }
    let Some(corners) = face
        .corners(bounds)
        .into_iter()
        .map(|corner| project(frame.point(corner)))
        .collect::<Option<Vec<[f32; 2]>>>()
    else {
        // A corner behind the camera (it's inside the brick's box, or nearly).
        return face;
    };
    let corner_parts = corner_parts(face);
    // Edge `index` runs from corner `index` to the next one; the opposite edge is two further.
    let edges: [f32; 4] = core::array::from_fn(|index| {
        geometry::screen_distance(mouse, corners[index], corners[(index + 1) % 4])
    });
    // The face's width across edge `index`, through the mouse.
    let width = |index: usize| edges[index] + edges[(index + 2) % 4];
    // Corner `index` is between edges `index - 1` and `index`.
    let nearest_corner = (0..4)
        .map(|index| {
            let across = width(index).min(width((index + 3) % 4));
            let reach = CORNER_PIXELS.min(PART_SHARE * across);
            (
                pixel_distance(mouse, corners[index]) / reach,
                corner_parts[index],
            )
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((share, part)) = nearest_corner
        && share <= 1.0
    {
        return part;
    }
    let nearest_edge = (0..4)
        .map(|index| {
            let reach = EDGE_PIXELS.min(PART_SHARE * width(index));
            let next = (index + 1) % 4;
            (
                edges[index] / reach,
                edge_between(corner_parts[index], corner_parts[next]),
            )
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    match nearest_edge {
        Some((share, part)) if share <= 1.0 => part,
        _ => face,
    }
}

/// The corner or edge of a box the mouse is beside, outside the box on the screen: the nearest
/// corner within [`CORNER_PIXELS`], else the nearest edge within [`EDGE_PIXELS`]; with its point
/// nearest to `ray` (the mouse's), in the vehicle. `project` as for [`part_under`].
pub fn beside(
    frame: &Frame,
    bounds: &Bounds,
    ray: &Ray,
    mouse: [f32; 2],
    project: impl Fn(Vec3) -> Option<[f32; 2]>,
) -> Option<(Part, Vec3)> {
    let corners: Vec<(Part, Vec3, Option<[f32; 2]>)> = (0..8u8)
        .map(|bits| {
            let part = Part {
                sides: core::array::from_fn(|axis| if bits >> axis & 1 == 1 { 1 } else { -1 }),
            };
            let point = frame.point(bounds.point(part.sides));
            (part, point, project(point))
        })
        .collect();
    let nearest_corner = corners
        .iter()
        .filter_map(|&(part, point, screen)| Some((pixel_distance(mouse, screen?), part, point)))
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((pixels, part, point)) = nearest_corner
        && pixels <= CORNER_PIXELS
    {
        return Some((part, point));
    }
    // The twelve edges: pairs of corners on different sides along one axis only.
    let mut nearest_edge: Option<(f32, Part, Vec3, Vec3)> = None;
    for (index, &(a, a_point, a_screen)) in corners.iter().enumerate() {
        for &(b, b_point, b_screen) in &corners[index + 1..] {
            let differing = (0..3)
                .filter(|&axis| a.sides[axis] != b.sides[axis])
                .count();
            let (Some(a_screen), Some(b_screen)) = (a_screen, b_screen) else {
                continue;
            };
            if differing != 1 {
                continue;
            }
            let pixels = geometry::screen_distance(mouse, a_screen, b_screen);
            if nearest_edge.is_none_or(|(nearest, ..)| pixels < nearest) {
                nearest_edge = Some((pixels, edge_between(a, b), a_point, b_point));
            }
        }
    }
    let (pixels, edge, start, end) = nearest_edge?;
    (pixels <= EDGE_PIXELS).then(|| (edge, geometry::closest_on_segment(ray, start, end)))
}

/// The distance between two points of the screen (pixels).
fn pixel_distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// A face's four corners, as parts, in the order of [`Part::corners`].
fn corner_parts(face: Part) -> [Part; 4] {
    let spanned: Vec<usize> = (0..3).filter(|&axis| face.sides[axis] == 0).collect();
    [[-1, -1], [1, -1], [1, 1], [-1, 1]].map(|[first, second]| {
        let mut sides = face.sides;
        sides[spanned[0]] = first;
        sides[spanned[1]] = second;
        Part { sides }
    })
}

/// The edge between two corners of a face: their common sides.
fn edge_between(a: Part, b: Part) -> Part {
    Part {
        sides: core::array::from_fn(|axis| {
            if a.sides[axis] == b.sides[axis] {
                a.sides[axis]
            } else {
                0
            }
        }),
    }
}
