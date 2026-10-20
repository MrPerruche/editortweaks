//! Clicking on things: where a ray from the camera meets the triangles of a reference image or
//! model.
//!
//! A [`Shape`] holds the triangles in the thing's own space. To test it, bring the ray into that
//! space with [`local_ray`] (so the triangles never have to move), then ask [`Shape::hit`]. The `t`
//! of a hit is the same in both spaces, so the hits on several shapes compare directly: the
//! smallest `t` is the one in front.

/// A line from `origin` along `direction` (any length, not zero); points on it are
/// `origin + t * direction`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: [f64; 3],
    pub direction: [f64; 3],
}

/// Triangles something can be clicked by, in its own space, with their box for a quick first test.
#[derive(Clone, Debug, Default)]
pub struct Shape {
    positions: Vec<[f32; 3]>,
    corners: Vec<[u32; 3]>,
    /// `[min, max]` around the triangles.
    bounds: Option<[[f32; 3]; 2]>,
}

impl Shape {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds triangles (corner indices into `positions`); out-of-range indices are skipped.
    pub fn add(&mut self, positions: &[[f32; 3]], corners: &[[u32; 3]]) {
        // This call's indices start from zero; ours continue after the positions we have.
        let first = self.positions.len() as u32;
        self.positions.extend_from_slice(positions);
        for triangle in corners {
            if triangle.iter().any(|&i| i as usize >= positions.len()) {
                continue;
            }
            for &i in triangle {
                self.grow_bounds(positions[i as usize]);
            }
            self.corners.push(triangle.map(|i| first + i));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.corners.is_empty()
    }

    pub fn triangle_count(&self) -> usize {
        self.corners.len()
    }

    /// Its triangles' corners.
    pub fn triangles(&self) -> impl Iterator<Item = [[f32; 3]; 3]> + '_ {
        self.corners
            .iter()
            .map(|triangle| triangle.map(|i| self.positions[i as usize]))
    }

    /// The smallest box around the triangles, if any: `[min, max]`.
    pub fn bounds(&self) -> Option<[[f32; 3]; 2]> {
        self.bounds
    }

    /// The smallest `t >= 0` where `ray` meets a triangle (either side; computed in `f64`), if any.
    /// Tests the box first (slab test) and returns `None` at once when it's missed.
    pub fn hit(&self, ray: &Ray) -> Option<f64> {
        let [min, max] = self.bounds?;
        if !meets_box(ray, min.map(f64::from), max.map(f64::from)) {
            return None;
        }
        self.corners
            .iter()
            .filter_map(|triangle| {
                let [a, b, c] = triangle.map(|i| self.positions[i as usize].map(f64::from));
                meets_triangle(ray, a, b, c)
            })
            .min_by(f64::total_cmp)
    }

    fn grow_bounds(&mut self, point: [f32; 3]) {
        let [min, max] = self.bounds.get_or_insert([point; 2]);
        for axis in 0..3 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }
}

/// `ray` (in the parent's space) seen from a thing placed at `position`, turned by `rotation`
/// (an Unreal `FRotator` in degrees: `[pitch, yaw, roll]`) and scaled by `scale` (per axis, may be
/// negative for mirroring), as Unreal places a component: `parent = Rotate(scale * local) +
/// position`. The `t` of a point is the same along both rays, so hits on differently placed things
/// compare by `t`. `None` if a scale is zero.
pub fn local_ray(
    ray: &Ray,
    position: [f64; 3],
    rotation: [f64; 3],
    scale: [f64; 3],
) -> Option<Ray> {
    if scale.contains(&0.0) {
        return None;
    }
    let axes = rotation_axes(rotation);
    // Undo the placement in reverse: move, turn back, then scale back.
    let unplace = |vector: [f64; 3]| {
        let turned_back = axes.map(|axis| dot(vector, axis));
        [0, 1, 2].map(|i| turned_back[i] / scale[i])
    };
    Some(Ray {
        origin: unplace(sub(ray.origin, position)),
        direction: unplace(ray.direction),
    })
}

/// Unreal's `FRotationMatrix` axes for a rotation (pitch, yaw, roll in degrees): where a thing's
/// forward (+X), right (+Y) and up (+Z) end up. A local vector `v` is turned into
/// `v.x * X + v.y * Y + v.z * Z`; going back, the components are the dot products with each axis.
pub(crate) fn rotation_axes(rotation: [f64; 3]) -> [[f64; 3]; 3] {
    let [pitch, yaw, roll] = rotation.map(f64::to_radians);
    let (sp, cp) = pitch.sin_cos();
    let (sy, cy) = yaw.sin_cos();
    let (sr, cr) = roll.sin_cos();
    [
        [cp * cy, cp * sy, sp],
        [sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, -sr * cp],
        [-(cr * sp * cy + sr * sy), cy * sr - cr * sp * sy, cr * cp],
    ]
}

/// Whether the ray passes through the box `[min, max]` at some `t >= 0`: the slab test, narrowing
/// down the range of `t` that is inside the box on every axis in turn.
fn meets_box(ray: &Ray, min: [f64; 3], max: [f64; 3]) -> bool {
    let mut near = 0.0_f64;
    let mut far = f64::INFINITY;
    for axis in 0..3 {
        let (origin, direction) = (ray.origin[axis], ray.direction[axis]);
        if direction == 0.0 {
            // Parallel to this pair of faces: inside them all along, or never.
            if origin < min[axis] || origin > max[axis] {
                return false;
            }
            continue;
        }
        let a = (min[axis] - origin) / direction;
        let b = (max[axis] - origin) / direction;
        near = near.max(a.min(b));
        far = far.min(a.max(b));
    }
    near <= far
}

/// The `t >= 0` where the ray meets triangle `a`, `b`, `c` (Möller-Trumbore), if it does.
fn meets_triangle(ray: &Ray, a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> Option<f64> {
    let edge1 = sub(b, a);
    let edge2 = sub(c, a);
    let p = cross(ray.direction, edge2);
    let det = dot(edge1, p);
    // Parallel to the triangle (or a triangle with no area). Compared with the sizes involved, so
    // it works whatever the units.
    let scale = length(ray.direction) * length(edge1) * length(edge2);
    if det.abs() <= 1e-12 * scale {
        return None;
    }
    let from_a = sub(ray.origin, a);
    let u = dot(from_a, p) / det;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(from_a, edge1);
    let v = dot(ray.direction, q) / det;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = dot(edge2, q) / det;
    (t >= 0.0).then_some(t)
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit square in the XY plane at height `z`, as two triangles.
    fn quad(z: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        (
            vec![[0.0, 0.0, z], [1.0, 0.0, z], [1.0, 1.0, z], [0.0, 1.0, z]],
            vec![[0, 1, 2], [0, 2, 3]],
        )
    }

    fn shape_of(z: f32) -> Shape {
        let (positions, corners) = quad(z);
        let mut shape = Shape::new();
        shape.add(&positions, &corners);
        shape
    }

    fn ray(origin: [f64; 3], direction: [f64; 3]) -> Ray {
        Ray { origin, direction }
    }

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-9)
    }

    fn along(ray: &Ray, t: f64) -> [f64; 3] {
        [0, 1, 2].map(|i| ray.origin[i] + t * ray.direction[i])
    }

    #[test]
    fn quad_is_hit_from_both_sides() {
        let shape = shape_of(0.0);
        assert!(!shape.is_empty());
        let from_above = shape.hit(&ray([0.25, 0.75, 5.0], [0.0, 0.0, -1.0]));
        assert_eq!(from_above, Some(5.0));
        let from_below = shape.hit(&ray([0.75, 0.25, -3.0], [0.0, 0.0, 1.0]));
        assert_eq!(from_below, Some(3.0));
        // t counts in lengths of the direction.
        let long = shape.hit(&ray([0.5, 0.5, 5.0], [0.0, 0.0, -2.0]));
        assert_eq!(long, Some(2.5));
    }

    #[test]
    fn misses_outside_and_behind() {
        let shape = shape_of(0.0);
        assert_eq!(shape.hit(&ray([2.0, 0.5, 5.0], [0.0, 0.0, -1.0])), None);
        assert_eq!(shape.hit(&ray([0.5, 2.0, 5.0], [0.0, 0.0, -1.0])), None);
        // The plane is behind the ray's start.
        assert_eq!(shape.hit(&ray([0.5, 0.5, 5.0], [0.0, 0.0, 1.0])), None);
    }

    #[test]
    fn starting_inside_the_box_works() {
        let mut shape = Shape::new();
        // Two triangles, one above and one below the ray's start.
        shape.add(
            &[
                [0.0, 0.0, 1.0],
                [4.0, 0.0, 1.0],
                [0.0, 4.0, 1.0],
                [0.0, 0.0, -1.0],
                [4.0, 0.0, -1.0],
                [0.0, 4.0, -1.0],
            ],
            &[[0, 1, 2], [3, 4, 5]],
        );
        let up = ray([1.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
        assert_eq!(shape.hit(&up), Some(1.0));
    }

    #[test]
    fn box_misses_return_none() {
        let shape = shape_of(0.0);
        assert_eq!(shape.bounds(), Some([[0.0, 0.0, 0.0], [1.0, 1.0, 0.0]]));
        // Parallel to the plane of the quad but beside it, and above it.
        assert_eq!(shape.hit(&ray([-1.0, 0.5, 0.0], [0.0, 1.0, 0.0])), None);
        assert_eq!(shape.hit(&ray([-1.0, 0.5, 1.0], [1.0, 0.0, 0.0])), None);
        // Passing the box by, though the line would cross its plane.
        assert_eq!(shape.hit(&ray([3.0, 3.0, 3.0], [-1.0, -2.0, -1.0])), None);
    }

    #[test]
    fn nearest_triangle_wins() {
        let mut shape = shape_of(0.0);
        let (positions, corners) = quad(1.0);
        shape.add(&positions, &corners);
        assert_eq!(shape.bounds(), Some([[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]]));
        let down = shape.hit(&ray([0.5, 0.5, 5.0], [0.0, 0.0, -1.0]));
        assert_eq!(down, Some(4.0));
        let up = shape.hit(&ray([0.5, 0.5, -5.0], [0.0, 0.0, 1.0]));
        assert_eq!(up, Some(5.0));
    }

    #[test]
    fn parallel_ray_does_not_hit() {
        let shape = shape_of(0.0);
        // In the plane of the quad, and just above it.
        assert_eq!(shape.hit(&ray([-1.0, 0.5, 0.0], [1.0, 0.0, 0.0])), None);
        assert_eq!(shape.hit(&ray([-1.0, 0.5, 0.5], [1.0, 0.0, 0.0])), None);
    }

    #[test]
    fn bad_triangles_are_skipped() {
        let mut shape = Shape::new();
        let (positions, _) = quad(0.0);
        shape.add(&positions, &[[0, 1, 4]]);
        assert!(shape.is_empty());
        assert_eq!(shape.bounds(), None);
        assert_eq!(shape.hit(&ray([0.5, 0.5, 5.0], [0.0, 0.0, -1.0])), None);
    }

    #[test]
    fn local_ray_keeps_the_hit_point() {
        let shape = shape_of(0.0);
        let position = [100.0, -40.0, 25.0];
        let rotation = [0.0, 90.0, 0.0];
        let scale = [200.0, -300.0, 50.0];
        let [x, y, z] = rotation_axes(rotation);
        // Where a point of the shape ends up in the parent's space.
        let place = |p: [f64; 3]| {
            let scaled = [p[0] * scale[0], p[1] * scale[1], p[2] * scale[2]];
            [0, 1, 2].map(|i| scaled[0] * x[i] + scaled[1] * y[i] + scaled[2] * z[i] + position[i])
        };

        // The quad's point (0.25, 0.75, 0) in the parent's space, looked at from somewhere else.
        let target = place([0.25, 0.75, 0.0]);
        let eye = [-500.0, 300.0, 700.0];
        let parent = ray(eye, sub(target, eye));
        let local = local_ray(&parent, position, rotation, scale).unwrap();

        let t = shape.hit(&local).expect("the ray goes through the quad");
        assert!((t - 1.0).abs() < 1e-9, "t = {t}");
        let local_point = along(&local, t);
        assert!(close(local_point, [0.25, 0.75, 0.0]), "{local_point:?}");
        assert!(close(place(local_point), along(&parent, t)));
    }

    #[test]
    fn local_ray_misses_where_the_thing_is_not() {
        let shape = shape_of(0.0);
        // Scaled to 2 x 2 and moved: a ray through the old unit square's place now misses it.
        let scale = [2.0, 2.0, 1.0];
        let position = [10.0, 0.0, 0.0];
        let aimed_at_old_place = ray([0.5, 0.5, 5.0], [0.0, 0.0, -1.0]);
        let local = local_ray(&aimed_at_old_place, position, [0.0; 3], scale).unwrap();
        assert_eq!(shape.hit(&local), None);
        let aimed_at_new_place = ray([11.0, 1.0, 5.0], [0.0, 0.0, -1.0]);
        let local = local_ray(&aimed_at_new_place, position, [0.0; 3], scale).unwrap();
        assert_eq!(shape.hit(&local), Some(5.0));
    }

    #[test]
    fn zero_scale_has_no_local_ray() {
        let r = ray([0.0; 3], [1.0, 0.0, 0.0]);
        assert_eq!(local_ray(&r, [0.0; 3], [0.0; 3], [1.0, 0.0, 1.0]), None);
    }
}
