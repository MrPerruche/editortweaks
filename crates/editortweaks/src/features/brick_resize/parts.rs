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

/// How many times the stretch where an edge goes behind the camera is halved to find where.
const CLIP_STEPS: usize = 14;

/// A part of a box: per axis, the side it's on (-1 the smallest, 1 the largest), or 0 along the
/// axes it spans. A face has one side, an edge two, a corner three.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Part {
    pub sides: [i8; 3],
}

impl Part {
    pub fn face(axis: usize, positive: bool) -> Self {
        let mut sides = [0; 3];
        sides[axis] = if positive { 1 } else { -1 };
        Self { sides }
    }

    /// Every part of a box with `fixed` sides (see [`Self::dimensions_fixed`]): its 6 faces, 12
    /// edges or 8 corners.
    pub fn all(fixed: usize) -> impl Iterator<Item = Part> {
        (0..27i8)
            .map(|code| Part {
                sides: [code % 3 - 1, code / 3 % 3 - 1, code / 9 - 1],
            })
            .filter(move |part| part.dimensions_fixed() == fixed)
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
/// [`EDGE_PIXELS`], else the face; less on a face that looks thin (see [`PART_SHARE`]). What's behind
/// the camera doesn't count, the rest of an edge going there does (see [`Seen`]). `project` shows
/// where a point of the vehicle is on the screen.
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
    let corners: Vec<Seen> = face
        .corners(bounds)
        .into_iter()
        .map(|corner| Seen::of(frame.point(corner), &project))
        .collect();
    let corner_parts = corner_parts(face);
    // Edge `index` runs from corner `index` to the next one; the opposite edge is two further. One
    // wholly behind the camera is infinitely far: it has no zone, and leaves the face all its width.
    let edges: [f32; 4] = core::array::from_fn(|index| {
        match on_screen(corners[index], corners[(index + 1) % 4], &project) {
            Some([start, end]) => geometry::screen_distance(mouse, start, end),
            None => f32::INFINITY,
        }
    });
    // The face's width across edge `index`, through the mouse.
    let width = |index: usize| edges[index] + edges[(index + 2) % 4];
    // Corner `index` is between edges `index - 1` and `index`.
    let nearest_corner = (0..4)
        .map(|index| {
            let across = width(index).min(width((index + 3) % 4));
            let reach = CORNER_PIXELS.min(PART_SHARE * across);
            let distance = corners[index]
                .screen
                .map_or(f32::INFINITY, |corner| pixel_distance(mouse, corner));
            (distance / reach, corner_parts[index])
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
    let corners: Vec<(Part, Seen)> = (0..8u8)
        .map(|bits| {
            let part = Part {
                sides: core::array::from_fn(|axis| if bits >> axis & 1 == 1 { 1 } else { -1 }),
            };
            let point = frame.point(bounds.point(part.sides));
            (part, Seen::of(point, &project))
        })
        .collect();
    let nearest_corner = corners
        .iter()
        .filter_map(|&(part, seen)| Some((pixel_distance(mouse, seen.screen?), part, seen.point)))
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((pixels, part, point)) = nearest_corner
        && pixels <= CORNER_PIXELS
    {
        return Some((part, point));
    }
    // The twelve edges: pairs of corners on different sides along one axis only.
    let mut nearest_edge: Option<(f32, Part, Vec3, Vec3)> = None;
    for (index, &(a, a_seen)) in corners.iter().enumerate() {
        for &(b, b_seen) in &corners[index + 1..] {
            let differing = (0..3)
                .filter(|&axis| a.sides[axis] != b.sides[axis])
                .count();
            if differing != 1 {
                continue;
            }
            let Some([start, end]) = on_screen(a_seen, b_seen, &project) else {
                continue;
            };
            let pixels = geometry::screen_distance(mouse, start, end);
            if nearest_edge.is_none_or(|(nearest, ..)| pixels < nearest) {
                nearest_edge = Some((pixels, edge_between(a, b), a_seen.point, b_seen.point));
            }
        }
    }
    let (pixels, edge, start, end) = nearest_edge?;
    (pixels <= EDGE_PIXELS).then(|| (edge, geometry::closest_on_segment(ray, start, end)))
}

/// A point of the vehicle, and where it shows on the screen (`None` behind the camera).
#[derive(Clone, Copy)]
struct Seen {
    point: Vec3,
    screen: Option<[f32; 2]>,
}

impl Seen {
    fn of(point: Vec3, project: &impl Fn(Vec3) -> Option<[f32; 2]>) -> Self {
        Self {
            point,
            screen: project(point),
        }
    }
}

/// The ends on the screen of what shows of the line between two points: close to a big brick, an
/// edge often goes behind the camera, while the rest of it is in plain view. `None` if it's all
/// behind (both ends are: the part in front of a camera is convex).
fn on_screen(
    a: Seen,
    b: Seen,
    project: &impl Fn(Vec3) -> Option<[f32; 2]>,
) -> Option<[[f32; 2]; 2]> {
    match (a.screen, b.screen) {
        (Some(start), Some(end)) => Some([start, end]),
        (Some(start), None) => Some([start, last_in_front(a.point, start, b.point, project)]),
        (None, Some(end)) => Some([last_in_front(b.point, end, a.point, project), end]),
        (None, None) => None,
    }
}

/// Where the line from `front` (showing at `screen`) to `behind` (behind the camera) last shows.
/// Each step's projection is a reflected call, but only edges going behind the camera get here.
fn last_in_front(
    mut front: Vec3,
    mut screen: [f32; 2],
    mut behind: Vec3,
    project: &impl Fn(Vec3) -> Option<[f32; 2]>,
) -> [f32; 2] {
    for _ in 0..CLIP_STEPS {
        let middle = geometry::lerp([front, behind], 0.5);
        match project(middle) {
            Some(shown) => {
                front = middle;
                screen = shown;
            }
            None => behind = middle,
        }
    }
    screen
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Quat;

    /// A camera at the origin looking along X, the screen 1000 pixels wide with its center at (500,
    /// 500) and one pixel per thousandth of the depth at a depth of one.
    fn project(point: Vec3) -> Option<[f32; 2]> {
        let [x, y, z] = point;
        (x > 0.0).then(|| {
            [
                (500.0 + 1000.0 * y / x) as f32,
                (500.0 - 1000.0 * z / x) as f32,
            ]
        })
    }

    fn frame() -> Frame {
        Frame::new([0.0; 3], Quat::IDENTITY)
    }

    /// The ray from the camera through a point of the screen.
    fn ray_through([x, y]: [f32; 2]) -> Ray {
        Ray {
            origin: [0.0; 3],
            direction: geometry::normalized([
                1.0,
                f64::from(x - 500.0) / 1000.0,
                f64::from(500.0 - y) / 1000.0,
            ])
            .unwrap(),
        }
    }

    /// A giant brick to the camera's left and below it: its top's right edge runs from behind the
    /// camera (x -5000) far ahead (x 5000), `offset` centimeters right of the camera's axis.
    fn giant_beside(offset: f64) -> Bounds {
        Bounds {
            min: [-5000.0, offset - 10000.0, -10000.0],
            max: [5000.0, offset, -100.0],
        }
    }

    const TOP: Part = Part { sides: [0, 0, 1] };
    const RIGHT_EDGE_OF_TOP: Part = Part { sides: [0, 1, 1] };

    /// The screen point of a point of the top's right edge, `x` ahead of the camera.
    fn on_right_edge(bounds: &Bounds, x: f64) -> [f32; 2] {
        project([x, bounds.max[1], bounds.max[2]]).unwrap()
    }

    #[test]
    fn an_edge_going_behind_the_camera_is_still_pointed_at() {
        let bounds = giant_beside(0.0);
        let [x, y] = on_right_edge(&bounds, 300.0);
        // Just left of the edge, on the top: two of the face's corners are behind the camera.
        let under = part_under(&frame(), &bounds, TOP, [x - 4.0, y], project);
        assert_eq!(under, RIGHT_EDGE_OF_TOP);
    }

    #[test]
    fn past_an_edges_zone_it_is_the_face() {
        let bounds = giant_beside(0.0);
        let [x, y] = on_right_edge(&bounds, 300.0);
        let under = part_under(&frame(), &bounds, TOP, [x - 100.0, y], project);
        assert_eq!(under, TOP);
    }

    #[test]
    fn the_zone_stays_capped_in_pixels_on_a_giant_face() {
        let bounds = giant_beside(0.0);
        let [x, y] = on_right_edge(&bounds, 300.0);
        let edge = EDGE_PIXELS;
        assert_eq!(
            part_under(&frame(), &bounds, TOP, [x - edge + 1.0, y], project),
            RIGHT_EDGE_OF_TOP
        );
        assert_eq!(
            part_under(&frame(), &bounds, TOP, [x - edge - 1.0, y], project),
            TOP
        );
    }

    #[test]
    fn a_corner_ahead_counts_with_the_rest_of_the_face_behind_the_camera() {
        let bounds = giant_beside(0.0);
        let [x, y] = on_right_edge(&bounds, 5000.0);
        let under = part_under(&frame(), &bounds, TOP, [x - 1.0, y + 1.0], project);
        assert_eq!(under, Part { sides: [1, 1, 1] });
    }

    #[test]
    fn a_face_wholly_behind_the_camera_is_only_a_face() {
        let bounds = Bounds {
            min: [-5000.0, -5000.0, -5000.0],
            max: [-100.0, 5000.0, 5000.0],
        };
        let under = part_under(
            &frame(),
            &bounds,
            Part { sides: [1, 0, 0] },
            [500.0, 500.0],
            project,
        );
        assert_eq!(under, Part { sides: [1, 0, 0] });
    }

    #[test]
    fn an_edge_all_in_front_is_pointed_at_as_before() {
        let bounds = Bounds {
            min: [1000.0, -100.0, -100.0],
            max: [1200.0, 100.0, 100.0],
        };
        let face = Part { sides: [-1, 0, 0] };
        let [x, y] = project([1000.0, 100.0, 0.0]).unwrap();
        assert_eq!(
            part_under(&frame(), &bounds, face, [x - 3.0, y], project),
            Part { sides: [-1, 1, 0] }
        );
        assert_eq!(
            part_under(&frame(), &bounds, face, [500.0, 500.0], project),
            face
        );
    }

    #[test]
    fn beside_an_edge_going_behind_the_camera() {
        let bounds = giant_beside(100.0);
        let [x, y] = on_right_edge(&bounds, 300.0);
        let mouse = [x + 4.0, y];
        let ray = ray_through(mouse);
        let (part, point) = beside(&frame(), &bounds, &ray, mouse, project).unwrap();
        assert_eq!(part, RIGHT_EDGE_OF_TOP);
        assert!(point[0] > 0.0, "in front of the camera: {point:?}");
        assert!((point[1] - bounds.max[1]).abs() < 1e-6);
        assert!((point[2] - bounds.max[2]).abs() < 1e-6);
    }

    #[test]
    fn far_beside_an_edge_going_behind_the_camera_is_nothing() {
        let bounds = giant_beside(100.0);
        let [x, y] = on_right_edge(&bounds, 300.0);
        let mouse = [x + 60.0, y];
        let ray = ray_through(mouse);
        assert!(beside(&frame(), &bounds, &ray, mouse, project).is_none());
    }

    #[test]
    fn the_shown_end_of_a_clipped_line_is_close_to_the_camera_plane() {
        let front = Seen::of([1000.0, 500.0, -100.0], &project);
        let behind = Seen::of([-1000.0, 500.0, -100.0], &project);
        let [start, end] = on_screen(front, behind, &project).unwrap();
        assert_eq!(Some(start), front.screen);
        // Nearly at the camera's plane, the line reaches far off the screen.
        assert!(end[0] > 100_000.0, "{end:?}");
        // The other way around gives the same line.
        let [other_end, other_start] = on_screen(behind, front, &project).unwrap();
        assert_eq!((other_start, other_end), (start, end));
    }

    #[test]
    fn a_line_all_behind_the_camera_doesnt_show() {
        let a = Seen::of([-1000.0, 0.0, 0.0], &project);
        let b = Seen::of([-10.0, 500.0, 0.0], &project);
        assert!(on_screen(a, b, &project).is_none());
    }
}
