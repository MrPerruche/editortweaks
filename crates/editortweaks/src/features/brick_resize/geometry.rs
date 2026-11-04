//! Vector math for bricks' boxes, in double precision (see [`crate::math`]): frames (a brick's own
//! axes, or the vehicle's in the world), boxes, and rays against boxes, lines and planes.
//!
//! The tool works in the vehicle's space, where bricks keep their places (their spawn location and
//! rotation): those are exact, unlike their places in the world, which the game computes in `f32`
//! far from the world's origin (the editor isn't at it), a few thousandths of a millimeter off.

use crate::game::editor_object::{Transform, Vector};
use crate::math::{self, Quat};
pub use crate::math::{Vec3, add, cross, dot, length, normalized, scale, sub};

/// A ray: its origin and unit direction.
#[derive(Clone, Copy)]
pub struct Ray {
    pub origin: Vec3,
    pub direction: Vec3,
}

/// The point `distance` along `ray`.
pub fn along(ray: &Ray, distance: f64) -> Vec3 {
    add(ray.origin, scale(ray.direction, distance))
}

/// The point `share` of the way from `start` to `end`.
pub fn lerp([start, end]: [Vec3; 2], share: f64) -> Vec3 {
    add(start, scale(sub(end, start), share))
}

/// A place and rotation: a brick's own axes and origin in the vehicle, or the vehicle's in the
/// world.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub origin: Vec3,
    pub rotation: Quat,
    /// The rotated axes (length 1).
    pub axes: [Vec3; 3],
}

impl Frame {
    pub fn new(origin: Vec3, rotation: Quat) -> Self {
        Self {
            origin,
            rotation,
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
                .map(|axis| rotation.rotate(axis)),
        }
    }

    /// Where the editor places a brick: its location and rotation (pitch, yaw, roll).
    pub fn of_place(location: Vector, rotation: Vector) -> Self {
        Self::new(
            math::widen(location),
            Quat::from_rotator(math::widen(rotation)),
        )
    }

    /// A transform's place and rotation (bricks' transforms aren't scaled).
    pub fn of_transform(transform: &Transform) -> Self {
        let [x, y, z, _] = transform.translation;
        Self::new(math::widen([x, y, z]), Quat::widen(transform.rotation))
    }

    /// The frame whose space this one is given in, from where this one is in the world:
    /// `world = parent * self`.
    pub fn parent(&self, world: &Frame) -> Frame {
        let rotation = world.rotation * self.rotation.inverse();
        Frame::new(sub(world.origin, rotation.rotate(self.origin)), rotation)
    }

    /// A direction given along the frame's axes, outside it.
    pub fn direction(&self, local: Vec3) -> Vec3 {
        let [x, y, z] = self.axes;
        add(
            add(scale(x, local[0]), scale(y, local[1])),
            scale(z, local[2]),
        )
    }

    /// A point given in the frame, outside it.
    pub fn point(&self, local: Vec3) -> Vec3 {
        add(self.origin, self.direction(local))
    }

    /// A direction outside the frame, along its axes.
    pub fn local_direction(&self, outside: Vec3) -> Vec3 {
        self.axes.map(|axis| dot(outside, axis))
    }

    /// A point outside the frame, in it.
    pub fn local(&self, outside: Vec3) -> Vec3 {
        self.local_direction(sub(outside, self.origin))
    }

    /// A ray outside the frame, in it.
    pub fn local_ray(&self, ray: &Ray) -> Ray {
        Ray {
            origin: self.local(ray.origin),
            direction: self.local_direction(ray.direction),
        }
    }
}

/// A box along a frame's axes: its smallest and largest corner.
#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
}

impl Bounds {
    /// The coordinate of its side along `axis`: the smallest (`side` < 0), the largest (> 0), or
    /// the middle (0).
    pub fn side(&self, axis: usize, side: i8) -> f64 {
        match side.signum() {
            -1 => self.min[axis],
            1 => self.max[axis],
            _ => (self.min[axis] + self.max[axis]) / 2.0,
        }
    }

    /// The point on the given side along each axis (see [`Self::side`]).
    pub fn point(&self, sides: [i8; 3]) -> Vec3 {
        core::array::from_fn(|axis| self.side(axis, sides[axis]))
    }

    pub fn extent(&self, axis: usize) -> f64 {
        self.max[axis] - self.min[axis]
    }

    /// The box grown by `margin` on every side.
    pub fn grown(&self, margin: f64) -> Self {
        Self {
            min: self.min.map(|value| value - margin),
            max: self.max.map(|value| value + margin),
        }
    }
}

/// Where a ray crosses a side of a box: its distance along the ray, and the face (axis, and whether
/// the largest side).
pub type Crossing = (f64, usize, bool);

/// Where the line of a ray (in the box's frame) enters and leaves the box, also behind the ray's
/// start. `None` if it misses.
fn crossings(ray: &Ray, bounds: &Bounds) -> Option<[Crossing; 2]> {
    let mut enter = (f64::NEG_INFINITY, 0, false);
    let mut exit = (f64::INFINITY, 0, false);
    for axis in 0..3 {
        let (origin, direction) = (ray.origin[axis], ray.direction[axis]);
        if direction.abs() < 1e-12 {
            if origin < bounds.min[axis] || origin > bounds.max[axis] {
                return None;
            }
            continue;
        }
        let to_min = (bounds.min[axis] - origin) / direction;
        let to_max = (bounds.max[axis] - origin) / direction;
        // Entering through the side facing the ray, leaving through the opposite one.
        let (near, far, positive) = if to_min < to_max {
            (to_min, to_max, false)
        } else {
            (to_max, to_min, true)
        };
        if near > enter.0 {
            enter = (near, axis, positive);
        }
        if far < exit.0 {
            exit = (far, axis, !positive);
        }
    }
    (enter.0 <= exit.0).then_some([enter, exit])
}

/// Where a ray (in the box's frame) enters the box from outside. `None` if it misses, or starts
/// inside (see [`ray_out_of_box`]).
pub fn ray_box(ray: &Ray, bounds: &Bounds) -> Option<Crossing> {
    let [enter, _] = crossings(ray, bounds)?;
    (enter.0 >= 0.0).then_some(enter)
}

/// Where a ray (in the box's frame) starting inside the box leaves it. `None` if it starts outside.
pub fn ray_out_of_box(ray: &Ray, bounds: &Bounds) -> Option<Crossing> {
    let [enter, exit] = crossings(ray, bounds)?;
    (enter.0 < 0.0 && exit.0 >= 0.0).then_some(exit)
}

/// Whether a ray (in the box's frame) goes through the box at all, starting inside it or not.
pub fn ray_meets_box(ray: &Ray, bounds: &Bounds) -> bool {
    crossings(ray, bounds).is_some_and(|[_, exit]| exit.0 >= 0.0)
}

/// Where to cut the line from `start` to `end` (as shares of it, from 0 to 1, in order) so that
/// along each piece the distance to `camera` changes by a factor of about `ratio` (above 1) at most,
/// and a width set from a piece's distance fits all of it. Just the ends when it needs no cut.
/// Distances count as at least `nearest`, which keeps the pieces next to the camera few.
fn distance_cuts(start: Vec3, end: Vec3, camera: Vec3, ratio: f64, nearest: f64) -> Vec<f64> {
    let line = sub(end, start);
    let span = length(line);
    let to_camera = sub(camera, start);
    let Some(direction) = normalized(line) else {
        return vec![0.0, 1.0];
    };
    // Along the line, how far its point nearest the camera is, and how far that is from the camera.
    let closest = dot(to_camera, direction);
    let height = length(sub(to_camera, scale(direction, closest)));
    let farthest = length(to_camera).max(length(sub(camera, end)));
    let mut cuts = vec![0.0, span];
    let mut distance = height.max(nearest) * ratio;
    while distance < farthest {
        let away = (distance * distance - height * height).sqrt();
        cuts.extend(
            [closest - away, closest + away]
                .into_iter()
                .filter(|&along| along > 0.0 && along < span),
        );
        distance *= ratio;
    }
    cuts.sort_by(f64::total_cmp);
    cuts.into_iter().map(|along| along / span).collect()
}

/// How a line's width may follow its distance: a line has one width set by the distance to its
/// middle, so it looks wider where it's nearer than that and thinner where it's further. How much is
/// limited, by giving a stretch of it another width where it would go too far.
pub struct WidthRule {
    /// How finely to cut the line: see [`distance_cuts`]'s `ratio` and `nearest`.
    pub ratio: f64,
    pub nearest: f64,
    /// How thin and how wide a stretch may look, as a share of the width asked for.
    pub thinnest: f64,
    pub widest: f64,
}

/// A stretch of a line, from share `from` of it to share `to`, as wide as the width asked for is at
/// `width_distance` from the camera.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Stretch {
    pub from: f64,
    pub to: f64,
    pub width_distance: f64,
}

/// The stretches of the line from `start` to `end`, as `rule` wants them seen from `camera`: each
/// counts the distance to the line's middle, unless that's out of the rule's limits at its own.
pub fn stretches(start: Vec3, end: Vec3, camera: Vec3, rule: &WidthRule) -> Vec<Stretch> {
    let distance = |share| length(sub(lerp([start, end], share), camera));
    let whole = distance(0.5);
    distance_cuts(start, end, camera, rule.ratio, rule.nearest)
        .windows(2)
        .map(|pair| {
            let own = distance((pair[0] + pair[1]) / 2.0);
            Stretch {
                from: pair[0],
                to: pair[1],
                width_distance: whole.max(own * rule.thinnest).min(own * rule.widest),
            }
        })
        .collect()
}

/// `stretches` with neighbors as wide as each other made one, so a line within its limits is one.
pub fn merged(stretches: Vec<Stretch>) -> Vec<Stretch> {
    let mut joined: Vec<Stretch> = Vec::with_capacity(stretches.len());
    for stretch in stretches {
        match joined.last_mut() {
            Some(last) if last.width_distance == stretch.width_distance => last.to = stretch.to,
            _ => joined.push(stretch),
        }
    }
    joined
}

/// Whether the segment between `ends` meets the box (both in the box's frame).
pub fn segment_box(ends: [Vec3; 2], bounds: &Bounds) -> bool {
    // The part of the segment inside the box, as shares of its length.
    let (mut enter, mut exit) = (0.0f64, 1.0f64);
    let sides = bounds.min.iter().zip(&bounds.max);
    for ((&start, &end), (&min, &max)) in ends[0].iter().zip(&ends[1]).zip(sides) {
        let along = end - start;
        if along.abs() < 1e-12 {
            if start < min || start > max {
                return false;
            }
            continue;
        }
        let to_min = (min - start) / along;
        let to_max = (max - start) / along;
        enter = enter.max(to_min.min(to_max));
        exit = exit.min(to_min.max(to_max));
        if enter > exit {
            return false;
        }
    }
    true
}

/// How far along the line through `point` along `direction` (length 1) its point closest to the
/// ray is. `None` if the line runs along the ray.
pub fn closest_on_line(ray: &Ray, point: Vec3, direction: Vec3) -> Option<f64> {
    let between = sub(ray.origin, point);
    let cosine = dot(ray.direction, direction);
    let denominator = 1.0 - cosine * cosine;
    if denominator < 1e-6 {
        return None;
    }
    Some((dot(direction, between) - cosine * dot(ray.direction, between)) / denominator)
}

/// The point of the segment from `start` to `end` closest to the ray.
pub fn closest_on_segment(ray: &Ray, start: Vec3, end: Vec3) -> Vec3 {
    let Some(direction) = normalized(sub(end, start)) else {
        return start;
    };
    let length = length(sub(end, start));
    let distance = closest_on_line(ray, start, direction).unwrap_or(0.0);
    add(start, scale(direction, distance.clamp(0.0, length)))
}

/// How far `point` is from the ray's line.
pub fn ray_distance(ray: &Ray, point: Vec3) -> f64 {
    let offset = sub(point, ray.origin);
    let along = dot(offset, ray.direction);
    length(sub(offset, scale(ray.direction, along)))
}

/// The point of the line through `points` (in order) nearest the ray, and where it is along them:
/// the index of the point it's after, plus the share of the way to the next one. `None` without
/// points.
pub fn closest_on_polyline(ray: &Ray, points: &[Vec3]) -> Option<(f64, Vec3)> {
    if let [only] = points {
        return Some((0.0, *only));
    }
    points
        .windows(2)
        .enumerate()
        .map(|(index, pair)| {
            let point = closest_on_segment(ray, pair[0], pair[1]);
            let span = length(sub(pair[1], pair[0]));
            let share = if span > 0.0 {
                length(sub(point, pair[0])) / span
            } else {
                0.0
            };
            (index as f64 + share, point)
        })
        .min_by(|a, b| ray_distance(ray, a.1).total_cmp(&ray_distance(ray, b.1)))
}

/// How far along the ray it meets the plane through `point` across `normal`, if in front.
/// `min_cosine` is the smallest angle's cosine between the ray and the plane accepted (rays almost
/// along the plane give unsteady points).
pub fn ray_plane(ray: &Ray, point: Vec3, normal: Vec3, min_cosine: f64) -> Option<f64> {
    let cosine = dot(ray.direction, normal);
    if cosine.abs() < min_cosine {
        return None;
    }
    let distance = dot(sub(point, ray.origin), normal) / cosine;
    (distance >= 0.0).then_some(distance)
}

/// The distance on the screen from `point` to the segment from `start` to `end` (pixels).
pub fn screen_distance(point: [f32; 2], start: [f32; 2], end: [f32; 2]) -> f32 {
    let along = [end[0] - start[0], end[1] - start[1]];
    let to_point = [point[0] - start[0], point[1] - start[1]];
    let squared = along[0] * along[0] + along[1] * along[1];
    let share = if squared > 0.0 {
        ((to_point[0] * along[0] + to_point[1] * along[1]) / squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let closest = [start[0] + along[0] * share, start[1] + along[1] * share];
    ((point[0] - closest[0]).powi(2) + (point[1] - closest[1]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIANT: Bounds = Bounds {
        min: [0.0, -5000.0, -5000.0],
        max: [10000.0, 5000.0, 5000.0],
    };

    fn ray(origin: Vec3, direction: Vec3) -> Ray {
        Ray {
            origin,
            direction: normalized(direction).unwrap(),
        }
    }

    #[test]
    fn a_ray_from_outside_enters_through_the_side_facing_it() {
        let from_front = ray([-20.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        assert_eq!(ray_box(&from_front, &GIANT), Some((20.0, 0, false)));
        assert_eq!(ray_out_of_box(&from_front, &GIANT), None);
        assert!(ray_meets_box(&from_front, &GIANT));
    }

    #[test]
    fn a_ray_from_inside_leaves_through_the_side_ahead() {
        let inside = ray([100.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        assert_eq!(ray_box(&inside, &GIANT), None);
        assert_eq!(ray_out_of_box(&inside, &GIANT), Some((9900.0, 0, true)));
        assert!(ray_meets_box(&inside, &GIANT));
        let backward = ray([100.0, 0.0, 0.0], [-1.0, 0.0, 0.0]);
        assert_eq!(ray_out_of_box(&backward, &GIANT), Some((100.0, 0, false)));
    }

    #[test]
    fn a_ray_past_or_away_from_the_box_doesnt_meet_it() {
        let away = ray([-20.0, 0.0, 0.0], [-1.0, 0.0, 0.0]);
        let past = ray([-20.0, 6000.0, 0.0], [1.0, 0.0, 0.0]);
        for ray in [away, past] {
            assert_eq!(ray_box(&ray, &GIANT), None);
            assert_eq!(ray_out_of_box(&ray, &GIANT), None);
            assert!(!ray_meets_box(&ray, &GIANT));
        }
    }

    /// The camera close to a giant brick: inside the margin the mouse's reach grows its box by, the
    /// ray still starts outside the box itself. The filter must keep it.
    #[test]
    fn the_filter_keeps_a_brick_right_in_front_of_the_camera() {
        let close = ray([-20.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        let grown = GIANT.grown(90.0);
        assert_eq!(ray_box(&close, &grown), None);
        assert!(ray_meets_box(&close, &grown));
        assert_eq!(ray_box(&close, &GIANT), Some((20.0, 0, false)));
    }

    fn distance_at(start: Vec3, end: Vec3, camera: Vec3, share: f64) -> f64 {
        length(sub(lerp([start, end], share), camera))
    }

    /// Every piece's distances stay within `ratio` of each other (allowing for the `nearest` floor),
    /// and the cuts go in order from one end to the other.
    fn check_cuts(start: Vec3, end: Vec3, camera: Vec3) -> usize {
        const RATIO: f64 = 1.25;
        const NEAREST: f64 = 1.0;
        let cuts = distance_cuts(start, end, camera, RATIO, NEAREST);
        assert_eq!(cuts.first(), Some(&0.0));
        assert_eq!(cuts.last(), Some(&1.0));
        for pair in cuts.windows(2) {
            assert!(pair[0] < pair[1], "out of order: {cuts:?}");
            // Sampled, as the nearest point of a piece may be inside it.
            let distances: Vec<f64> = (0..=16)
                .map(|step| {
                    let share = pair[0] + (pair[1] - pair[0]) * f64::from(step) / 16.0;
                    distance_at(start, end, camera, share).max(NEAREST)
                })
                .collect();
            let least = distances.iter().copied().fold(f64::INFINITY, f64::min);
            let most = distances.iter().copied().fold(0.0, f64::max);
            assert!(most / least <= RATIO * 1.0001, "{most} / {least}: {cuts:?}");
        }
        cuts.len() - 1
    }

    #[test]
    fn a_short_line_far_away_stays_whole() {
        let pieces = check_cuts([5000.0, 0.0, 0.0], [5000.0, 100.0, 0.0], [0.0; 3]);
        assert_eq!(pieces, 1);
    }

    #[test]
    fn a_line_leaving_the_camera_is_cut_more_finely_near_it() {
        let (start, end) = ([20.0, 0.0, 0.0], [100000.0, 0.0, 0.0]);
        let camera = [0.0; 3];
        let pieces = check_cuts(start, end, camera);
        assert!(pieces > 10 && pieces < 60, "{pieces} pieces");
        let cuts = distance_cuts(start, end, camera, 1.25, 1.0);
        assert!(cuts[1] - cuts[0] < cuts[pieces] - cuts[pieces - 1]);
    }

    #[test]
    fn a_line_passing_the_camera_is_cut_on_both_sides() {
        let pieces = check_cuts([-10000.0, 30.0, 0.0], [10000.0, 30.0, 0.0], [0.0; 3]);
        assert!(pieces > 20 && pieces < 100, "{pieces} pieces");
    }

    #[test]
    fn a_line_through_the_camera_stays_within_bounds() {
        let pieces = check_cuts([-10000.0, 0.0, 0.0], [10000.0, 0.0, 0.0], [0.0; 3]);
        assert!(pieces < 100, "{pieces} pieces");
    }

    const RULE: WidthRule = WidthRule {
        ratio: 1.25,
        nearest: 1.0,
        thinnest: 0.5,
        widest: 3.0,
    };

    /// The stretches of a line as drawn, checked to cover it all without a gap.
    fn drawn(start: Vec3, end: Vec3, camera: Vec3) -> Vec<Stretch> {
        let drawn = merged(stretches(start, end, camera, &RULE));
        assert_eq!(drawn[0].from, 0.0);
        assert_eq!(drawn[drawn.len() - 1].to, 1.0);
        for pair in drawn.windows(2) {
            assert_eq!(pair[0].to, pair[1].from);
        }
        drawn
    }

    /// How wide a stretch looks at its ends, as a share of the width asked for.
    fn looks(stretch: &Stretch, start: Vec3, end: Vec3, camera: Vec3) -> [f64; 2] {
        [stretch.from, stretch.to]
            .map(|share| stretch.width_distance / distance_at(start, end, camera, share))
    }

    #[test]
    fn a_mark_keeps_the_width_of_its_middle() {
        let (start, end) = ([500.0, -2.0, 0.0], [500.0, 2.0, 0.0]);
        let drawn = drawn(start, end, [0.0; 3]);
        assert_eq!(drawn.len(), 1);
        assert!((drawn[0].width_distance - 500.0).abs() < 1e-9);
    }

    /// The edge of a 4 m brick from 2 m away: wider at the near end, as it always was.
    #[test]
    fn a_normal_edge_stays_one_line_wider_up_close() {
        let (start, end) = ([200.0, 0.0, 0.0], [600.0, 0.0, 0.0]);
        let drawn = drawn(start, end, [0.0; 3]);
        assert_eq!(drawn.len(), 1);
        assert!((drawn[0].width_distance - 400.0).abs() < 1e-9);
    }

    /// The edge of a 500 m brick going away from 3 m off: what took over the screen, at 83 times
    /// the width asked for next to the camera.
    #[test]
    fn a_giant_edge_going_away_is_capped_next_to_the_camera() {
        let (start, end) = ([300.0, 0.0, 0.0], [50_300.0, 0.0, 0.0]);
        let camera = [0.0; 3];
        let drawn = drawn(start, end, camera);
        assert!(drawn.len() > 1);
        for stretch in &drawn {
            for look in looks(stretch, start, end, camera) {
                assert!(look <= RULE.widest * RULE.ratio, "looks {look} wide");
            }
        }
        let [nearest, _] = looks(&drawn[0], start, end, camera);
        assert!(nearest > RULE.widest * 0.8, "looks {nearest} wide");
        // Past where the cap matters, it's the line's own width again.
        let last = drawn[drawn.len() - 1];
        assert!((last.width_distance - 25_300.0).abs() < 1e-9);
    }

    /// The same edge passing by 3 m away: its far ends stay seen rather than thinning to nothing.
    #[test]
    fn a_giant_edge_passing_by_stays_seen_far_away() {
        let (start, end) = ([300.0, -25_000.0, 0.0], [300.0, 25_000.0, 0.0]);
        let camera = [0.0; 3];
        for stretch in &drawn(start, end, camera) {
            for look in looks(stretch, start, end, camera) {
                assert!(look >= RULE.thinnest / RULE.ratio, "looks {look} wide");
                assert!(look <= RULE.widest * RULE.ratio, "looks {look} wide");
            }
        }
    }

    /// No step in width where a capped stretch meets the next, even in the worst place.
    #[test]
    fn a_capped_line_has_no_big_steps_in_width() {
        let (start, end) = ([300.0, 0.0, 0.0], [50_300.0, 0.0, 0.0]);
        for pair in drawn(start, end, [0.0; 3]).windows(2) {
            let step = pair[1].width_distance / pair[0].width_distance;
            assert!(step <= RULE.ratio * 1.01, "a step of {step}");
        }
    }

    #[test]
    fn a_line_through_the_camera_gets_a_valid_width() {
        let (start, end) = ([-100.0, 0.0, 0.0], [100.0, 0.0, 0.0]);
        for stretch in &drawn(start, end, [0.0; 3]) {
            assert!(stretch.width_distance.is_finite() && stretch.width_distance >= 0.0);
        }
    }

    #[test]
    fn a_line_without_length_stays_whole() {
        let point = [10.0, 20.0, 30.0];
        assert_eq!(
            distance_cuts(point, point, [0.0; 3], 1.25, 1.0),
            vec![0.0, 1.0]
        );
    }
}
