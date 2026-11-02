//! How well a vehicle fits a reference model: the distance from points to the model's surface,
//! and a way to show it as color.
//!
//! A [`Surface`] holds the reference triangles with a bounding volume hierarchy, so
//! [`Surface::distance`] doesn't test every triangle. The game's mesh builder has UVs but no vertex
//! colors, so the distance is shown through a one-pixel-tall [`gradient`] texture: [`scale_u`]
//! gives the U that reads a distance's color, and [`sample`] turns a brick's surface into a mesh
//! with those UVs, subdivided where the distance changes too fast for linear interpolation, as
//! finely as a [`Detail`] says (a quick preview, then the fine version).
//!
//! The gradient is a log scale (a hair away and a meter away both matter), with a gray band at
//! the end for surfaces that have no reference. It is wide so that the game may sample it without
//! filtering and the steps between neighbouring texels still can't be seen.

use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::image::Image;
use crate::model::{Part, SMOOTH_ANGLE};

/// Width of the gradient texture in texels. The game may point-sample it, so it is wide enough
/// that one texel is a color step too small to see.
pub const GRADIENT_WIDTH: usize = 4096;

/// How many texels at the start of the gradient hold the distance scale. The rest are gray.
pub const GRADIENT_TEXELS: usize = 4080;

/// How finely [`sample`] subdivides.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Detail {
    /// The most triangles [`sample`] returns, unless its input already has more (it isn't
    /// subdivided then). This counts the triangles cut up next to subdivided ones to close the
    /// cracks, so fewer are subdivided: as many as fit once those are cut up.
    pub max_triangles: usize,
    /// Triangles whose longest edge (cm, in the reference's space) is this short or shorter aren't
    /// subdivided.
    pub min_edge: f32,
    /// The most the U in the middle of a triangle may differ from what linear interpolation gives
    /// (on the 0..1 range of U, a seventh of which is a factor of ten in distance) before the
    /// triangle is subdivided.
    pub max_u_error: f32,
    /// Triangles with a longer edge (cm) near the reference are subdivided with what budget is
    /// left once every triangle whose colors are measurably off is, since a dip in the distance
    /// (the surface touching the reference) can hide between the points measured.
    pub max_edge: f32,
}

impl Detail {
    /// Quick to measure (a brick being dragged, the first colors after a change): centimeter
    /// detail, colors off by up to a factor of about 2.2 inside a triangle, and few precautions.
    pub const PREVIEW: Self = Self {
        max_triangles: 600,
        min_edge: 2.0,
        max_u_error: 0.05,
        max_edge: 50.0,
    };

    /// Sharp: half-millimeter detail where the distance changes, colors off by up to a factor of
    /// about 1.3 inside a triangle.
    pub const FINE: Self = Self {
        max_triangles: 6000,
        min_edge: 0.05,
        max_u_error: 0.015,
        max_edge: 10.0,
    };
}

/// The distance scale starts at `10^LOG_MIN` cm and ends at `10^LOG_MAX` cm.
const LOG_MIN: f32 = -2.0;
const LOG_MAX: f32 = 5.0;

/// Colors of the scale (sRGB), at 0.01, 0.1, 1, 10, ... 100000 cm: one per power of ten from
/// `LOG_MIN` to `LOG_MAX`.
const STOPS: [[u8; 3]; 8] = [
    [0xff, 0xff, 0xff],  // 0.01 cm: white
    [0x50, 0xf0, 0xd0],   // 0.1 cm: teal
    [0x90, 0xe0, 0x50], // 1 cm: green
    [0xff, 0xd0, 0x30], // 10 cm: yellow
    [0xff, 0x40, 0x30], // 100 cm: red
    [0xc0, 0x30, 0x90],  // 1000 cm: magenta
    [0x60, 0x20, 0x70],    // 10000 cm: purple
    [0x10, 0,    0x30],    // 100000 cm: ultra dark blue
];

/// The color of the texels after the scale: no reference to measure against.
const GRAY: [u8; 3] = [128, 128, 128];

/// Triangles per leaf of the hierarchy (at most).
const LEAF_SIZE: usize = 8;

/// Reference triangles in one space (the vehicle's), with a BVH for nearest-distance queries.
///
/// Distances are computed in `f64`: the ones near zero (down to 0.01 cm) are the ones that matter.
#[derive(Clone, Debug, Default)]
pub struct Surface {
    /// Reordered so every node's triangles are next to each other.
    triangles: Vec<[Vec3; 3]>,
    /// The root is the first node; empty if there are no triangles.
    nodes: Vec<Node>,
}

type Vec3 = [f64; 3];

#[derive(Clone, Debug)]
struct Node {
    min: Vec3,
    max: Vec3,
    contents: Contents,
}

#[derive(Clone, Debug)]
enum Contents {
    /// `count` triangles from `first` on.
    Leaf { first: usize, count: usize },
    /// Indices into the nodes.
    Inner { left: usize, right: usize },
}

impl Surface {
    /// Builds the hierarchy: each node splits its triangles at the median of their centers, along
    /// the longest side of the box around those centers. Triangles without an area (or with
    /// coordinates that aren't numbers) are left out; they have no surface to be close to.
    pub fn new(triangles: &[[[f32; 3]; 3]]) -> Self {
        let mut triangles: Vec<[Vec3; 3]> = triangles
            .iter()
            .map(|triangle| triangle.map(|corner| corner.map(f64::from)))
            .filter(|&[a, b, c]| {
                let area = length(cross(sub(b, a), sub(c, a)));
                area > 0.0 && area.is_finite()
            })
            .collect();
        let mut surface = Self::default();
        if !triangles.is_empty() {
            let count = triangles.len();
            surface.build(&mut triangles, 0, count);
        }
        surface.triangles = triangles;
        surface
    }

    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    /// Distance from `point` to the nearest triangle, `None` if there are none.
    pub fn distance(&self, point: [f32; 3]) -> Option<f32> {
        let squared = self.nearest_squared(point, f64::INFINITY)?;
        Some(squared.sqrt() as f32)
    }

    /// Like [`Surface::distance`], but only looks within `bound` of `point`: `None` if no triangle
    /// is that near. A good bound lets the search skip most of the hierarchy from the start, which
    /// is much faster when an estimate is known (a neighbouring point's distance, say).
    pub fn distance_below(&self, point: [f32; 3], bound: f32) -> Option<f32> {
        let bound_squared = f64::from(bound).powi(2);
        let squared = self
            .nearest_squared(point, bound_squared)
            .filter(|&squared| squared < bound_squared)?;
        Some(squared.sqrt() as f32)
    }

    /// The squared distance to the nearest triangle, or `limit_squared` if none is nearer.
    /// `None` only if there are no triangles.
    fn nearest_squared(&self, point: [f32; 3], limit_squared: f64) -> Option<f64> {
        if self.is_empty() {
            return None;
        }
        let mut best = limit_squared;
        self.nearest(0, point.map(f64::from), &mut best);
        Some(best)
    }

    /// Adds the node for `triangles[first..first + count]` (and below it), returns its index.
    /// Reorders that range.
    fn build(&mut self, triangles: &mut [[Vec3; 3]], first: usize, count: usize) -> usize {
        let range = &mut triangles[first..first + count];
        let (min, max) = bounds_of(range.iter().flatten().copied());
        let (centers_min, centers_max) = bounds_of(range.iter().map(centroid));
        let extent = sub(centers_max, centers_min);
        let axis = (0..3)
            .max_by(|&a, &b| extent[a].total_cmp(&extent[b]))
            .unwrap_or(0);

        let index = self.nodes.len();
        // All centers at one point can't be told apart: a leaf, however many there are.
        if count <= LEAF_SIZE || extent[axis] <= 0.0 {
            self.nodes.push(Node {
                min,
                max,
                contents: Contents::Leaf { first, count },
            });
            return index;
        }

        let half = count / 2;
        range.select_nth_unstable_by(half, |a, b| centroid(a)[axis].total_cmp(&centroid(b)[axis]));
        // Filled in below, once the children's indices are known.
        self.nodes.push(Node {
            min,
            max,
            contents: Contents::Leaf { first, count },
        });
        let left = self.build(triangles, first, half);
        let right = self.build(triangles, first + half, count - half);
        self.nodes[index].contents = Contents::Inner { left, right };
        index
    }

    /// Lowers `best` (a squared distance) to the nearest triangle below `node` if that is nearer.
    fn nearest(&self, node: usize, point: Vec3, best: &mut f64) {
        match self.nodes[node].contents {
            Contents::Leaf { first, count } => {
                for triangle in &self.triangles[first..first + count] {
                    let closest = closest_point_on_triangle(point, triangle);
                    *best = best.min(distance_squared(point, closest));
                }
            }
            Contents::Inner { left, right } => {
                let (near, far) = (self.box_distance_squared(left, point), {
                    self.box_distance_squared(right, point)
                });
                // The nearer box first: it is likelier to lower `best` so the other can be skipped.
                let ((first, first_distance), (second, second_distance)) = if near <= far {
                    ((left, near), (right, far))
                } else {
                    ((right, far), (left, near))
                };
                if first_distance < *best {
                    self.nearest(first, point, best);
                }
                if second_distance < *best {
                    self.nearest(second, point, best);
                }
            }
        }
    }

    /// Squared distance from `point` to a node's box (0 inside it).
    fn box_distance_squared(&self, node: usize, point: Vec3) -> f64 {
        let Node { min, max, .. } = &self.nodes[node];
        (0..3)
            .map(|axis| {
                let outside = (min[axis] - point[axis])
                    .max(point[axis] - max[axis])
                    .max(0.0);
                outside * outside
            })
            .sum()
    }
}

/// The gradient texture, [`GRADIENT_WIDTH`] x 1: the distance scale (see [`scale_u`]), then gray.
/// Colors are sRGB, blended between the [`STOPS`] by the log of the distance.
pub fn gradient() -> Image {
    let mut rgba = Vec::with_capacity(GRADIENT_WIDTH * 4);
    for texel in 0..GRADIENT_WIDTH {
        let color = if texel < GRADIENT_TEXELS {
            scale_color(texel as f32 / (GRADIENT_TEXELS - 1) as f32)
        } else {
            GRAY
        };
        rgba.extend_from_slice(&[color[0], color[1], color[2], 255]);
    }
    Image {
        width: GRADIENT_WIDTH as u32,
        height: 1,
        rgba,
    }
}

/// U for a distance in cm: the center of its texel on the scale, clamped to the first and last
/// texel of the scale (so bilinear filtering never reaches the gray texels or wraps around).
pub fn scale_u(distance: f32) -> f32 {
    // `max` also takes care of zero and NaN, which have no log.
    let log = distance.max(f32::MIN_POSITIVE).log10();
    let along = ((log - LOG_MIN) / (LOG_MAX - LOG_MIN)).clamp(0.0, 1.0);
    let texel = along * (GRADIENT_TEXELS - 1) as f32;
    (texel + 0.5) / GRADIENT_WIDTH as f32
}

/// V of the vertex at `index`: far from its neighbours' (up to 65472, in whole numbers).
///
/// The gradient is one texel tall, so V doesn't change its color, but the material showing it (the
/// Bricksville sign's) also reads a worn-plastic roughness texture with the same coordinates. With
/// U following the distance, that texture showed as shiny and dull bands along the lines of equal
/// distance. V jumping by thousands between neighbouring vertices makes the GPU read the
/// roughness texture from its smallest mip, one average value: even roughness everywhere. The
/// gradient has no mips (it's imported at run time), so its colors stay sharp.
pub fn scattered_v(index: usize) -> f32 {
    // A multiplicative hash (Knuth's) sends neighbouring indices far apart, to 0..1024.
    let spread = (index as u32).wrapping_mul(2_654_435_761) >> 22;
    spread as f32 * 64.0
}

/// U of a gray texel's center, for surfaces with no reference.
pub fn no_reference_u() -> f32 {
    let middle_of_gray = GRADIENT_TEXELS + (GRADIENT_WIDTH - GRADIENT_TEXELS) / 2;
    (middle_of_gray as f32 + 0.5) / GRADIENT_WIDTH as f32
}

/// The scale's color at `along` (0 at the first stop, 1 at the last), blended in sRGB bytes.
fn scale_color(along: f32) -> [u8; 3] {
    let position = along.clamp(0.0, 1.0) * (STOPS.len() - 1) as f32;
    let below = (position as usize).min(STOPS.len() - 2);
    let blend = position - below as f32;
    let (from, to) = (STOPS[below], STOPS[below + 1]);
    [0, 1, 2].map(|channel| {
        let (from, to) = (f32::from(from[channel]), f32::from(to[channel]));
        (from + (to - from) * blend).round() as u8
    })
}

/// A brick's surface, ready for the mesh builder.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sampled {
    /// In the space of the input.
    pub positions: Vec<[f32; 3]>,
    /// One per position: `[u, v]`, U to be read from the [`gradient`], V [`scattered_v`].
    pub uvs: Vec<[f32; 2]>,
    /// Three indices into `positions` per triangle, wound as the input's.
    pub corners: Vec<[u32; 3]>,
    /// One per position, unit length: the triangles' normals around the vertex, weighted by area.
    /// They point out of the front of triangles that are clockwise seen from the front, which is
    /// Unreal's winding (and so the input's).
    pub normals: Vec<[f32; 3]>,
}

/// Samples the distance to `surface` over a triangle mesh given in local space; `to_reference`
/// maps a local point into the surface's space (it may scale non-uniformly).
///
/// Hard edges get their own vertices first (the game shades smoothly across shared ones), then
/// triangles are subdivided where the distance varies too much to be read from the corners alone,
/// as finely as `detail` says. Where a subdivided triangle meets a coarser one, the coarser one is
/// cut up to match, so the mesh has no cracks. Triangles with indices out of range are dropped.
pub fn sample(
    positions: &[[f32; 3]],
    corners: &[[u32; 3]],
    to_reference: &dyn Fn([f32; 3]) -> [f32; 3],
    surface: &Surface,
    detail: &Detail,
) -> Sampled {
    let mut part = Part {
        positions: positions.to_vec(),
        corners: corners
            .iter()
            .copied()
            .filter(|triangle| triangle.iter().all(|&i| (i as usize) < positions.len()))
            .collect(),
        ..Part::default()
    };
    part.split_sharp_edges(SMOOTH_ANGLE);

    if surface.is_empty() {
        return Sampled {
            uvs: (0..part.positions.len())
                .map(|index| [no_reference_u(), scattered_v(index)])
                .collect(),
            normals: vertex_normals(&part.positions, &part.corners),
            positions: part.positions,
            corners: part.corners,
        };
    }
    let mut vertices = Vertices::new(part.positions, to_reference, surface, detail);
    let subdivision = vertices.subdivide(part.corners);
    let corners = vertices.within_budget(&subdivision);
    vertices.finish(corners)
}

/// Why a triangle should be subdivided, least pressing first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Need {
    /// Its colors look right where they were measured, but it's long and near the reference, so the
    /// distance may dip between those points.
    Precaution,
    /// Its colors are measurably off.
    Evidence,
}

/// A triangle waiting to be subdivided (by its index in [`Subdivision::triangles`]). The most
/// pressing comes first: by [`Need`], then the longest, then the first made (so the order of
/// splits doesn't depend on how the heap breaks ties).
struct Wanted {
    need: Need,
    longest: f32,
    index: usize,
}

impl Ord for Wanted {
    fn cmp(&self, other: &Self) -> Ordering {
        self.need
            .cmp(&other.need)
            .then(self.longest.total_cmp(&other.longest))
            .then(Reverse(self.index).cmp(&Reverse(other.index)))
    }
}

impl PartialOrd for Wanted {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Wanted {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Wanted {}

/// Every triangle a subdivision made, and in which order it split them: the mesh after any number
/// of its first splits can be told from it ([`Subdivision::after`]).
struct Subdivision {
    /// The input's, then four for each split.
    triangles: Vec<[u32; 3]>,
    /// For each triangle, which split (counting from 0) cut it in four; `usize::MAX` if none did.
    split_at: Vec<usize>,
    /// How many triangles the input had.
    input: usize,
    /// How many splits were made.
    splits: usize,
}

impl Subdivision {
    /// The mesh after the first `splits` splits: the triangles made by then and not split by then.
    fn after(&self, splits: usize) -> Vec<[u32; 3]> {
        let made = self.input + 4 * splits;
        (0..made)
            .filter(|&index| self.split_at[index] >= splits)
            .map(|index| self.triangles[index])
            .collect()
    }
}

/// The vertices of a mesh being subdivided, with what is known about each.
struct Vertices<'a> {
    local: Vec<[f32; 3]>,
    reference: Vec<[f32; 3]>,
    /// Distance to the surface, cm.
    distance: Vec<f32>,
    /// The vertex halfway along an edge (its two vertices, smaller first), shared by the triangles
    /// on both sides so there are no cracks. Not every one ends up in the mesh: some are made
    /// only to test whether a triangle needs splitting.
    midpoints: HashMap<(u32, u32), u32>,
    to_reference: &'a dyn Fn([f32; 3]) -> [f32; 3],
    surface: &'a Surface,
    detail: &'a Detail,
}

impl<'a> Vertices<'a> {
    fn new(
        local: Vec<[f32; 3]>,
        to_reference: &'a dyn Fn([f32; 3]) -> [f32; 3],
        surface: &'a Surface,
        detail: &'a Detail,
    ) -> Self {
        let mut vertices = Self {
            local: Vec::with_capacity(local.len()),
            reference: Vec::with_capacity(local.len()),
            distance: Vec::with_capacity(local.len()),
            midpoints: HashMap::new(),
            to_reference,
            surface,
            detail,
        };
        for position in local {
            vertices.add(position, &[]);
        }
        vertices
    }

    /// Adds a vertex, measuring its distance. `known` are vertices near it, which make the
    /// measurement faster (see [`Vertices::measure_near`]); there may be none.
    fn add(&mut self, position: [f32; 3], known: &[u32]) -> u32 {
        let reference = (self.to_reference)(position);
        let distance = self.measure_near(reference, known);
        self.local.push(position);
        self.reference.push(reference);
        self.distance.push(distance);
        (self.local.len() - 1) as u32
    }

    fn measure(&self, reference: [f32; 3]) -> f32 {
        self.surface.distance(reference).unwrap_or(f32::INFINITY)
    }

    /// The distance at `reference`, a point near the `known` vertices. Moving some distance changes
    /// the distance to a surface by at most that much, so every known vertex gives an upper bound,
    /// and the search only has to look within the tightest one. Without a usable bound this is the
    /// plain [`Vertices::measure`].
    fn measure_near(&self, reference: [f32; 3], known: &[u32]) -> f32 {
        let bound = known
            .iter()
            .map(|&vertex| {
                let vertex = vertex as usize;
                let apart = distance_squared_f32(self.reference[vertex], reference).sqrt();
                self.distance[vertex] + apart
            })
            .fold(f32::INFINITY, f32::min);
        // Room for the rounding of all those `f32`s, so the true nearest triangle is never cut off.
        let bound = bound * 1.001 + 1e-3;
        Some(bound)
            .filter(|bound| bound.is_finite())
            .and_then(|bound| self.surface.distance_below(reference, bound))
            .unwrap_or_else(|| self.measure(reference))
    }

    fn u(&self, vertex: u32) -> f32 {
        scale_u(self.distance[vertex as usize])
    }

    /// The vertex halfway between `a` and `b`, made if it doesn't exist yet.
    fn midpoint(&mut self, a: u32, b: u32) -> u32 {
        let key = (a.min(b), a.max(b));
        if let Some(&vertex) = self.midpoints.get(&key) {
            return vertex;
        }
        let (from, to) = (self.local[a as usize], self.local[b as usize]);
        let vertex = self.add([0, 1, 2].map(|axis| (from[axis] + to[axis]) * 0.5), &[a, b]);
        self.midpoints.insert(key, vertex);
        vertex
    }

    /// The point in the middle of the triangle, in local space.
    fn center(&self, triangle: [u32; 3]) -> [f32; 3] {
        let [a, b, c] = triangle.map(|vertex| self.local[vertex as usize]);
        [0, 1, 2].map(|axis| (a[axis] + b[axis] + c[axis]) / 3.0)
    }

    /// Splits the most pressing triangle in four ([`Wanted`]), again and again, until none needs it
    /// or the mesh has [`Detail::max_triangles`]. The triangles whose colors are measurably off go
    /// first, the biggest of them first, so the budget is spent where the colors are most wrong,
    /// down to the finest detail if it lasts. Triangles split only as a precaution get what's
    /// left: a big face that looks right everywhere can't take the budget from a small area that
    /// doesn't. Closing the cracks adds triangles, so [`Vertices::within_budget`] may then keep
    /// only the first splits.
    fn subdivide(&mut self, triangles: Vec<[u32; 3]>) -> Subdivision {
        let input = triangles.len();
        let mut subdivision = Subdivision {
            split_at: vec![usize::MAX; input],
            triangles,
            input,
            splits: 0,
        };
        let budget = self.detail.max_triangles;
        if input >= budget {
            return subdivision;
        }
        let mut wanted = BinaryHeap::new();
        for index in 0..input {
            self.consider(&subdivision, index, &mut wanted);
        }
        // Each split adds three triangles.
        let mut count = input;
        while count + 3 <= budget {
            let Some(most_pressing) = wanted.pop() else {
                break;
            };
            subdivision.split_at[most_pressing.index] = subdivision.splits;
            subdivision.splits += 1;
            count += 3;
            for quarter in self.split(subdivision.triangles[most_pressing.index]) {
                subdivision.triangles.push(quarter);
                subdivision.split_at.push(usize::MAX);
                self.consider(&subdivision, subdivision.triangles.len() - 1, &mut wanted);
            }
        }
        subdivision
    }

    /// Adds a subdivision's triangle to the `wanted` ones if it should be split.
    fn consider(
        &mut self,
        subdivision: &Subdivision,
        index: usize,
        wanted: &mut BinaryHeap<Wanted>,
    ) {
        if let Some((need, longest)) = self.wants_split(subdivision.triangles[index]) {
            wanted.push(Wanted {
                need,
                longest,
                index,
            });
        }
    }

    /// The mesh after as many of the subdivision's splits as keep it within
    /// [`Detail::max_triangles`] once its cracks are closed ([`Vertices::conform`]): the least
    /// pressing splits are left out first. Found by halving (closing the cracks nearly always
    /// adds more triangles after more splits), only counting the triangles, nothing is measured.
    fn within_budget(&self, subdivision: &Subdivision) -> Vec<[u32; 3]> {
        let max = self.detail.max_triangles;
        let all = subdivision.after(subdivision.splits);
        if self.conformed_count(&all) <= max {
            return all;
        }
        // No split at all is the fallback, even if the input alone has too many.
        let (mut fits, mut too_many) = (0, subdivision.splits);
        while too_many - fits > 1 {
            let middle = (fits + too_many) / 2;
            if self.conformed_count(&subdivision.after(middle)) <= max {
                fits = middle;
            } else {
                too_many = middle;
            }
        }
        subdivision.after(fits)
    }

    /// How many triangles [`Vertices::conform`] makes of `triangles`.
    fn conformed_count(&self, triangles: &[[u32; 3]]) -> usize {
        let used: HashSet<u32> = triangles.iter().flatten().copied().collect();
        triangles
            .iter()
            .map(|&triangle| match self.outline(triangle, &used).len() {
                3 => 1,
                fan => fan,
            })
            .sum()
    }

    /// The triangle in four, wound as it is.
    fn split(&mut self, [a, b, c]: [u32; 3]) -> [[u32; 3]; 4] {
        let (ab, bc, ca) = (
            self.midpoint(a, b),
            self.midpoint(b, c),
            self.midpoint(c, a),
        );
        [[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]]
    }

    /// Why the triangle should be subdivided and the length of its longest edge (in the
    /// reference's space), if it should be.
    fn wants_split(&mut self, triangle: [u32; 3]) -> Option<(Need, f32)> {
        let longest = [(0, 1), (1, 2), (2, 0)]
            .into_iter()
            .map(|(from, to)| {
                let (from, to) = (triangle[from] as usize, triangle[to] as usize);
                distance_squared_f32(self.reference[from], self.reference[to])
            })
            .fold(0.0f32, f32::max)
            .sqrt();
        if longest <= self.detail.min_edge {
            return None;
        }
        let distances = triangle.map(|vertex| self.distance[vertex as usize]);
        let max_u_error = self.detail.max_u_error;
        if interpolation_is_safe(distances, longest, max_u_error) {
            return None;
        }
        if self.interpolation_is_off(triangle) {
            return Some((Need::Evidence, longest));
        }
        // A long triangle near the reference: the distance can dip between the points measured.
        // Unless the nearest corner is already at the near end of the scale, where a dip changes
        // no color: then it's a face lying on the reference, right as it is however big.
        let nearest = distances.iter().copied().fold(f32::INFINITY, f32::min);
        let dip_shows = scale_u(nearest) - scale_u(0.0) > max_u_error;
        (longest > self.detail.max_edge && nearest < 2.0 * longest && dip_shows)
            .then_some((Need::Precaution, longest))
    }

    /// Whether linear interpolation of U between the corners is off in the middle of the triangle
    /// or of one of its edges, by more than [`Detail::max_u_error`]. Makes the edges' midpoints on
    /// the way.
    fn interpolation_is_off(&mut self, triangle: [u32; 3]) -> bool {
        let max_u_error = self.detail.max_u_error;
        let [a, b, c] = triangle;
        let (ua, ub, uc) = (self.u(a), self.u(b), self.u(c));
        let mut off = false;
        for (from, to, expected) in [
            (a, b, (ua + ub) / 2.0),
            (b, c, (ub + uc) / 2.0),
            (c, a, (uc + ua) / 2.0),
        ] {
            let middle = self.midpoint(from, to);
            off |= (self.u(middle) - expected).abs() > max_u_error;
        }
        if !off {
            let center = (self.to_reference)(self.center(triangle));
            let measured = scale_u(self.measure_near(center, &triangle));
            off = (measured - (ua + ub + uc) / 3.0).abs() > max_u_error;
        }
        off
    }

    /// The mesh made of `corners`, with only the vertices they use.
    fn finish(mut self, corners: Vec<[u32; 3]>) -> Sampled {
        let corners = self.conform(corners);
        let mut new_index = vec![u32::MAX; self.local.len()];
        let mut sampled = Sampled::default();
        for triangle in &corners {
            sampled.corners.push(triangle.map(|old| {
                let slot = &mut new_index[old as usize];
                if *slot == u32::MAX {
                    *slot = sampled.positions.len() as u32;
                    sampled.positions.push(self.local[old as usize]);
                    let index = sampled.uvs.len();
                    sampled
                        .uvs
                        .push([scale_u(self.distance[old as usize]), scattered_v(index)]);
                }
                *slot
            }));
        }
        sampled.normals = vertex_normals(&sampled.positions, &sampled.corners);
        sampled
    }

    /// Removes the T-junctions. A triangle next to a more subdivided one has the neighbour's
    /// vertices along its edge, which don't belong to it: the mesh has a crack there, and the
    /// triangle's straight color gradient doesn't match the neighbour's. Such a triangle becomes a
    /// fan from its center to the points around its outline, edge vertices included.
    fn conform(&mut self, triangles: Vec<[u32; 3]>) -> Vec<[u32; 3]> {
        let used: HashSet<u32> = triangles.iter().flatten().copied().collect();
        let mut conformed = Vec::with_capacity(triangles.len());
        for triangle in triangles {
            let outline = self.outline(triangle, &used);
            if outline.len() == 3 {
                conformed.push(triangle);
                continue;
            }
            let center = self.add(self.center(triangle), &triangle);
            for (i, &point) in outline.iter().enumerate() {
                let next = outline[(i + 1) % outline.len()];
                conformed.push([center, point, next]);
            }
        }
        conformed
    }

    /// The vertices around the triangle in its winding order: its corners, and between them the
    /// `used` vertices that lie along its edges.
    fn outline(&self, [a, b, c]: [u32; 3], used: &HashSet<u32>) -> Vec<u32> {
        let mut outline = Vec::new();
        for (from, to) in [(a, b), (b, c), (c, a)] {
            outline.push(from);
            self.push_edge_points(from, to, used, &mut outline);
        }
        outline
    }

    /// Pushes the `used` vertices strictly between `from` and `to`, in that order. They are
    /// midpoints of the edge, of its halves, and so on.
    fn push_edge_points(&self, from: u32, to: u32, used: &HashSet<u32>, points: &mut Vec<u32>) {
        let key = (from.min(to), from.max(to));
        let Some(&middle) = self.midpoints.get(&key) else {
            return;
        };
        // A midpoint nobody uses was only made to test a triangle, and its halves have none either.
        if !used.contains(&middle) {
            return;
        }
        self.push_edge_points(from, middle, used, points);
        points.push(middle);
        self.push_edge_points(middle, to, used, points);
    }
}

/// Whether the corners' distances alone show that linear interpolation of U is good enough
/// everywhere in a triangle, so nothing inside needs measuring.
///
/// Every point of the triangle is at most `longest` (its longest edge) from every corner, and a
/// distance to a surface changes by at most the way travelled. So the distance inside is at least
/// the farthest corner's minus `longest`, and at most the nearest corner's plus `longest`, and
/// [`scale_u`] being monotonic gives the same range for the true U. Interpolation stays between the
/// corners' smallest and largest U. If these two ranges are never more than `max_u_error` apart,
/// the interpolation can't be off. It also covers triangles whose range is clamped at the ends of
/// the scale, where U doesn't change at all.
fn interpolation_is_safe(distances: [f32; 3], longest: f32, max_u_error: f32) -> bool {
    let nearest = distances.iter().copied().fold(f32::INFINITY, f32::min);
    let farthest = distances.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let true_u = (scale_u(farthest - longest), scale_u(nearest + longest));
    let corner_us = distances.map(scale_u);
    let interpolated_u = (
        corner_us.iter().copied().fold(f32::INFINITY, f32::min),
        corner_us.iter().copied().fold(f32::NEG_INFINITY, f32::max),
    );
    let most_off = (true_u.1 - interpolated_u.0).max(interpolated_u.1 - true_u.0);
    most_off <= max_u_error
}

/// Unit normals for the vertices: the sum of the normals of the triangles around each, weighted by
/// area. A triangle's normal is `(c - a) x (b - a)`, which points out of the front of a triangle
/// that is clockwise seen from the front. Vertices without a direction get `[0, 0, 1]`.
fn vertex_normals(positions: &[[f32; 3]], corners: &[[u32; 3]]) -> Vec<[f32; 3]> {
    let mut sums = vec![[0.0f64; 3]; positions.len()];
    for triangle in corners {
        let [a, b, c] = triangle.map(|i| positions[i as usize].map(f64::from));
        // As long as twice the triangle's area, which weights it.
        let normal = cross(sub(c, a), sub(b, a));
        for &vertex in triangle {
            let sum = &mut sums[vertex as usize];
            *sum = add_scaled(*sum, normal, 1.0);
        }
    }
    sums.into_iter()
        .map(|sum| {
            let length = length(sum);
            if length > 0.0 && length.is_finite() {
                sum.map(|axis| (axis / length) as f32)
            } else {
                [0.0, 0.0, 1.0]
            }
        })
        .collect()
}

fn bounds_of(points: impl Iterator<Item = Vec3>) -> (Vec3, Vec3) {
    points.fold(
        ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]),
        |(min, max), point| {
            (
                [0, 1, 2].map(|axis| min[axis].min(point[axis])),
                [0, 1, 2].map(|axis| max[axis].max(point[axis])),
            )
        },
    )
}

fn centroid(triangle: &[Vec3; 3]) -> Vec3 {
    [0, 1, 2].map(|axis| (triangle[0][axis] + triangle[1][axis] + triangle[2][axis]) / 3.0)
}

/// The point of the triangle nearest to `p` (Ericson, Real-Time Collision Detection, 5.1.5): the
/// region of the triangle `p` is in (corner, edge or inside) tells where it is.
fn closest_point_on_triangle(p: Vec3, &[a, b, c]: &[Vec3; 3]) -> Vec3 {
    let ab = sub(b, a);
    let ac = sub(c, a);

    let ap = sub(p, a);
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }

    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }

    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return add_scaled(a, ab, d1 / (d1 - d3));
    }

    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }

    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return add_scaled(a, ac, d2 / (d2 - d6));
    }

    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        return add_scaled(b, sub(c, b), (d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }

    let denominator = 1.0 / (va + vb + vc);
    let (v, w) = (vb * denominator, vc * denominator);
    add_scaled(add_scaled(a, ab, v), ac, w)
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add_scaled(a: Vec3, b: Vec3, scale: f64) -> Vec3 {
    [
        a[0] + b[0] * scale,
        a[1] + b[1] * scale,
        a[2] + b[2] * scale,
    ]
}

fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}

fn distance_squared(a: Vec3, b: Vec3) -> f64 {
    let d = sub(a, b);
    dot(d, d)
}

fn distance_squared_f32(a: [f32; 3], b: [f32; 3]) -> f32 {
    [0, 1, 2]
        .map(|axis| (a[axis] - b[axis]).powi(2))
        .iter()
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The corners and triangles of the cube from the origin to `(size, size, size)`, welded:
    /// 8 vertices shared by 12 triangles.
    fn cube(size: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let positions = (0..8)
            .map(|i| [i & 1, (i >> 1) & 1, (i >> 2) & 1].map(|bit| bit as f32 * size))
            .collect();
        let quads: [[u32; 4]; 6] = [
            [0, 2, 3, 1],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 4, 6, 2],
            [1, 3, 7, 5],
        ];
        let corners = quads
            .iter()
            .flat_map(|&[a, b, c, d]| [[a, b, c], [a, c, d]])
            .collect();
        (positions, corners)
    }

    fn triangles_of(positions: &[[f32; 3]], corners: &[[u32; 3]]) -> Vec<[[f32; 3]; 3]> {
        corners
            .iter()
            .map(|triangle| triangle.map(|i| positions[i as usize]))
            .collect()
    }

    fn unit_cube_surface() -> Surface {
        let (positions, corners) = cube(1.0);
        Surface::new(&triangles_of(&positions, &corners))
    }

    fn assert_near(actual: Option<f32>, expected: f32) {
        let actual = actual.expect("a distance");
        assert!(
            (actual - expected).abs() < 1e-5,
            "{actual} is not {expected}"
        );
    }

    /// Small deterministic random numbers (a linear congruential generator).
    struct Random(u64);

    impl Random {
        /// In `[0, 1)`.
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }

        fn point(&mut self, low: f32, high: f32) -> [f32; 3] {
            [0, 1, 2].map(|_| low + (high - low) * self.next())
        }
    }

    #[test]
    fn distance_to_a_cube() {
        let cube = unit_cube_surface();
        assert_near(cube.distance([0.5, 0.5, 0.5]), 0.5);
        assert_near(cube.distance([0.5, 0.5, 0.2]), 0.2);
        assert_near(cube.distance([2.0, 0.5, 0.5]), 1.0);
        assert_near(cube.distance([1.01, 0.5, 0.5]), 0.01);
        assert_near(cube.distance([0.3, 0.7, 1.0]), 0.0);
        // Nearest to an edge, then to a corner.
        assert_near(cube.distance([2.0, 2.0, 0.5]), 2.0f32.sqrt());
        assert_near(cube.distance([2.0, 2.0, 2.0]), 3.0f32.sqrt());
        assert_near(cube.distance([-1.0, -1.0, -1.0]), 3.0f32.sqrt());
    }

    #[test]
    fn small_distances_keep_their_precision() {
        // Away from the origin, a hundredth of a cm still has to show.
        let surface =
            Surface::new(&[[[500.0, 0.0, 0.0], [500.0, 100.0, 0.0], [500.0, 0.0, 100.0]]]);
        let distance = surface.distance([500.01, 10.0, 10.0]).unwrap();
        assert!((distance - 0.01).abs() < 1e-4, "{distance}");
    }

    #[test]
    fn empty_surfaces_and_degenerate_triangles() {
        assert!(Surface::new(&[]).is_empty());
        assert_eq!(Surface::new(&[]).distance([0.0; 3]), None);
        let line = [[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]];
        let point = [[1.0; 3]; 3];
        let surface = Surface::new(&[line, point]);
        assert!(surface.is_empty());
        assert_eq!(surface.distance([0.0; 3]), None);
    }

    #[test]
    fn hierarchy_matches_brute_force() {
        let mut random = Random(42);
        let triangles: Vec<[[f32; 3]; 3]> = (0..2000)
            .map(|_| {
                let base = random.point(0.0, 100.0);
                [0, 1, 2].map(|_| {
                    let offset = random.point(-5.0, 5.0);
                    [0, 1, 2].map(|axis| base[axis] + offset[axis])
                })
            })
            .collect();
        let surface = Surface::new(&triangles);
        for _ in 0..500 {
            let point = random.point(-20.0, 120.0);
            let brute = triangles
                .iter()
                .map(|triangle| {
                    let triangle = triangle.map(|corner| corner.map(f64::from));
                    let point = point.map(f64::from);
                    distance_squared(point, closest_point_on_triangle(point, &triangle))
                })
                .fold(f64::INFINITY, f64::min)
                .sqrt() as f32;
            assert_near(surface.distance(point), brute);
        }
    }

    #[test]
    fn scale_u_is_monotonic_clamped_and_off_the_gray() {
        let first = 0.5 / GRADIENT_WIDTH as f32;
        let last = (GRADIENT_TEXELS as f32 - 0.5) / GRADIENT_WIDTH as f32;
        assert_eq!(scale_u(0.0), first);
        assert_eq!(scale_u(f32::NAN), first);
        assert_eq!(scale_u(0.001), first);
        assert_eq!(scale_u(0.01), first);
        assert_eq!(scale_u(100_000.0), last);
        assert_eq!(scale_u(1e9), last);
        assert_eq!(scale_u(f32::INFINITY), last);

        let mut previous = 0.0;
        for step in 0..=200 {
            let u = scale_u(10f32.powf(-2.5 + step as f32 * 0.04));
            assert!(u >= previous);
            assert!((first..=last).contains(&u), "{u}");
            previous = u;
        }
        assert!(scale_u(10.0) > scale_u(1.0));
        // A decade is a seventh of the scale.
        let decade = scale_u(1.0) - scale_u(0.1);
        assert!((decade - (last - first) / 7.0).abs() < 1e-4);

        let gray = no_reference_u();
        assert!(gray > last && gray < 1.0);
        let texel = (gray * GRADIENT_WIDTH as f32) as usize;
        assert!(texel >= GRADIENT_TEXELS);
    }

    #[test]
    fn neighbouring_vertices_get_far_apart_v() {
        for index in 0..10_000 {
            let v = scattered_v(index);
            assert!((0.0..=65_472.0).contains(&v) && v.fract() == 0.0);
        }
        // Among a fan's or a strip's vertices, no two neighbours share a V.
        for index in 0..10_000 {
            assert_ne!(scattered_v(index), scattered_v(index + 1));
            assert_ne!(scattered_v(index), scattered_v(index + 2));
        }
    }

    #[test]
    fn gradient_has_the_stop_colors() {
        let image = gradient();
        assert_eq!((image.width, image.height), (GRADIENT_WIDTH as u32, 1));
        assert_eq!(image.rgba.len(), GRADIENT_WIDTH * 4);
        let texel = |i: usize| image.rgba[i * 4..i * 4 + 4].to_vec();
        let opaque = |[r, g, b]: [u8; 3]| vec![r, g, b, 255];
        assert_eq!(texel(0), opaque(STOPS[0]));
        assert_eq!(texel(GRADIENT_TEXELS - 1), opaque(STOPS[STOPS.len() - 1]));
        for i in GRADIENT_TEXELS..GRADIENT_WIDTH {
            assert_eq!(texel(i), opaque(GRAY));
        }
        // The texel read for each stop's distance is close to the stop's color.
        let color_at = |distance: f32| {
            let i = (scale_u(distance) * GRADIENT_WIDTH as f32) as usize;
            texel(i)
        };
        for (distance, stop) in [
            (0.1, STOPS[1]),
            (1.0, STOPS[2]),
            (10.0, STOPS[3]),
            (1000.0, STOPS[5]),
        ] {
            let color = color_at(distance);
            for channel in 0..3 {
                let off = (i32::from(color[channel]) - i32::from(stop[channel])).abs();
                assert!(off < 12, "{distance} cm: {color:?} vs {stop:?}");
            }
        }
    }

    fn area(sampled: &Sampled) -> f64 {
        sampled
            .corners
            .iter()
            .map(|triangle| {
                let [a, b, c] = triangle.map(|i| sampled.positions[i as usize].map(f64::from));
                length(cross(sub(b, a), sub(c, a))) / 2.0
            })
            .sum()
    }

    #[test]
    fn a_square_touching_the_reference_is_subdivided() {
        let positions = vec![
            [0.0, 0.0, 0.0],
            [100.0, 0.0, 0.0],
            [100.0, 100.0, 0.0],
            [0.0, 100.0, 0.0],
        ];
        let corners = vec![[0, 1, 2], [0, 2, 3]];
        let surface = Surface::new(&[[[50.0, 50.0, 0.0], [52.0, 50.0, 0.0], [50.0, 52.0, 5.0]]]);
        let sampled = sample(
            &positions,
            &corners,
            &|point| point,
            &surface,
            &Detail::FINE,
        );

        assert!(sampled.corners.len() > 100, "{}", sampled.corners.len());
        assert!(sampled.corners.len() <= Detail::FINE.max_triangles);
        assert_eq!(sampled.positions.len(), sampled.uvs.len());
        assert!((area(&sampled) - 10_000.0).abs() < 10_000.0 * 1e-3);
        // Same winding as the input: facing +Z or -Z, as the first input triangle does.
        for triangle in &sampled.corners {
            let [a, b, c] = triangle.map(|i| sampled.positions[i as usize].map(f64::from));
            assert!(cross(sub(b, a), sub(c, a))[2] > 0.0);
        }
        // The U follows the distance, and every vertex is on the scale.
        for (position, uv) in sampled.positions.iter().zip(&sampled.uvs) {
            let distance = surface.distance(*position).unwrap();
            assert_eq!(uv[0], scale_u(distance));
        }
        // The vertex at the reference is the nearest end of the scale.
        let nearest = sampled.uvs.iter().map(|uv| uv[0]).fold(1.0, f32::min);
        assert!(nearest < scale_u(0.5));
        // The input is counter-clockwise seen from +Z, so its normals point down.
        assert_eq!(sampled.normals.len(), sampled.positions.len());
        for normal in &sampled.normals {
            assert!((normal[2] + 1.0).abs() < 1e-5, "{normal:?}");
        }
    }

    #[test]
    fn subdivision_leaves_far_away_flat_surfaces_alone() {
        let positions = vec![[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 4.0, 0.0]];
        let surface = Surface::new(&[[[0.0, 0.0, 1000.0], [1.0, 0.0, 1000.0], [0.0, 1.0, 1000.0]]]);
        let sampled = sample(
            &positions,
            &[[0, 1, 2]],
            &|point| point,
            &surface,
            &Detail::FINE,
        );
        assert_eq!(sampled.corners.len(), 1);
    }

    #[test]
    fn the_budget_is_kept() {
        // Fine detail everywhere (the reference is a dense sheet right at the surface) would want
        // far more than the budget.
        let positions = vec![[0.0, 0.0, 0.0], [1000.0, 0.0, 0.0], [0.0, 1000.0, 0.0]];
        let sheet: Vec<[[f32; 3]; 3]> = (0..40)
            .flat_map(|i| {
                (0..40).map(move |j| {
                    let (x, y) = (i as f32 * 25.0, j as f32 * 25.0);
                    [
                        [x, y, 3.0 + (i % 3) as f32 * 4.0],
                        [x + 10.0, y, 0.0],
                        [x, y + 10.0, 8.0],
                    ]
                })
            })
            .collect();
        let surface = Surface::new(&sheet);
        let sampled = sample(
            &positions,
            &[[0, 1, 2]],
            &|point| point,
            &surface,
            &Detail::FINE,
        );
        assert!(sampled.corners.len() > 1000);
        assert!(sampled.corners.len() <= Detail::FINE.max_triangles);
        assert!((area(&sampled) - 500_000.0).abs() < 500_000.0 * 1e-3);
    }

    #[test]
    fn no_reference_is_all_gray() {
        let (positions, corners) = cube(100.0);
        let sampled = sample(
            &positions,
            &corners,
            &|point| point,
            &Surface::default(),
            &Detail::FINE,
        );
        assert_eq!(sampled.corners.len(), 12);
        assert!(sampled.uvs.iter().all(|&uv| uv[0] == no_reference_u()));
    }

    #[test]
    fn hard_edges_get_their_own_vertices() {
        let (positions, corners) = cube(100.0);
        // Far from the reference, so nothing is subdivided: 4 vertices per face.
        let surface = Surface::new(&[[[0.0, 0.0, 1e6], [1.0, 0.0, 1e6], [0.0, 1.0, 1e6]]]);
        let sampled = sample(
            &positions,
            &corners,
            &|point| point,
            &surface,
            &Detail::FINE,
        );
        assert_eq!(sampled.corners.len(), 12);
        assert_eq!(sampled.positions.len(), 24);
        assert_eq!(
            sampled.positions.len(),
            sampled.uvs.len(),
            "one UV per position"
        );
    }

    #[test]
    fn subdivision_is_measured_in_the_reference_space() {
        // A 0.03 unit triangle is under the finest edge as it is, but 30 cm long in the
        // reference's space, where it touches the reference.
        let positions = vec![[0.0, 0.0, 0.0], [0.03, 0.0, 0.0], [0.0, 0.03, 0.0]];
        let surface = Surface::new(&[[[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 5.0]]]);
        let unscaled = sample(&positions, &[[0, 1, 2]], &|p| p, &surface, &Detail::FINE);
        assert_eq!(unscaled.corners.len(), 1);
        let scaled = sample(
            &positions,
            &[[0, 1, 2]],
            &|p| p.map(|axis| axis * 1000.0),
            &surface,
            &Detail::FINE,
        );
        assert!(scaled.corners.len() > 1);
    }

    /// A sphere of triangles, `rings` from pole to pole with `segments` around.
    fn sphere(center: [f32; 3], radius: f32, rings: usize, segments: usize) -> Vec<[[f32; 3]; 3]> {
        let point = |ring: usize, segment: usize| {
            let (polar, azimuth) = (
                std::f32::consts::PI * ring as f32 / rings as f32,
                std::f32::consts::TAU * (segment % segments) as f32 / segments as f32,
            );
            [
                center[0] + radius * polar.sin() * azimuth.cos(),
                center[1] + radius * polar.sin() * azimuth.sin(),
                center[2] - radius * polar.cos(),
            ]
        };
        let mut triangles = Vec::new();
        for ring in 0..rings {
            for segment in 0..segments {
                let (a, b) = (point(ring, segment), point(ring, segment + 1));
                let (c, d) = (point(ring + 1, segment + 1), point(ring + 1, segment));
                triangles.push([a, b, c]);
                triangles.push([a, c, d]);
            }
        }
        triangles
    }

    /// Whether `r` lies on the segment from `p` to `q`, not at its ends.
    fn is_inside_segment(p: [f32; 3], q: [f32; 3], r: [f32; 3]) -> bool {
        let [p, q, r] = [p, q, r].map(|point| point.map(f64::from));
        let (along, to_r) = (sub(q, p), sub(r, p));
        let t = dot(to_r, along) / dot(along, along);
        let off_the_line = length(cross(along, to_r)) / length(along);
        off_the_line < 1e-3 && t > 1e-4 && t < 1.0 - 1e-4
    }

    #[test]
    fn distance_below_agrees_with_distance_within_its_bound() {
        let mut random = Random(7);
        let triangles: Vec<[[f32; 3]; 3]> = (0..300)
            .map(|_| {
                let base = random.point(0.0, 100.0);
                [0, 1, 2].map(|_| {
                    let offset = random.point(-8.0, 8.0);
                    [0, 1, 2].map(|axis| base[axis] + offset[axis])
                })
            })
            .collect();
        let surface = Surface::new(&triangles);
        for _ in 0..500 {
            let point = random.point(-30.0, 130.0);
            let distance = surface.distance(point).unwrap();
            assert_eq!(
                surface.distance_below(point, distance * 1.01 + 0.01),
                Some(distance)
            );
            if distance > 0.1 {
                assert_eq!(surface.distance_below(point, distance / 2.0), None);
            }
        }
        assert_eq!(Surface::default().distance_below([0.0; 3], 1e6), None);
    }

    #[test]
    fn subdivision_leaves_no_t_junctions() {
        // A square (clockwise seen from +Z) over a coarse ball: only the part right above the ball
        // gets subdivided, so triangles of different sizes meet.
        let positions = vec![
            [0.0, 0.0, 0.0],
            [0.0, 100.0, 0.0],
            [100.0, 100.0, 0.0],
            [100.0, 0.0, 0.0],
        ];
        let corners = vec![[0, 1, 2], [0, 2, 3]];
        let surface = Surface::new(&sphere([50.0, 50.0, 33.0], 30.0, 8, 12));
        let sampled = sample(
            &positions,
            &corners,
            &|point| point,
            &surface,
            &Detail::FINE,
        );

        assert!(sampled.corners.len() > 100, "{}", sampled.corners.len());
        assert!(sampled.corners.len() <= Detail::FINE.max_triangles);
        assert!((area(&sampled) - 10_000.0).abs() < 10.0);
        for triangle in &sampled.corners {
            let points = triangle.map(|i| sampled.positions[i as usize]);
            for (p, q) in [(0, 1), (1, 2), (2, 0)] {
                for &r in &sampled.positions {
                    assert!(
                        !is_inside_segment(points[p], points[q], r),
                        "{r:?} is on the edge {:?} - {:?}",
                        points[p],
                        points[q]
                    );
                }
            }
        }
    }

    #[test]
    fn far_away_meshes_are_not_subdivided() {
        let positions = vec![
            [0.0, 0.0, 0.0],
            [0.0, 100.0, 0.0],
            [100.0, 100.0, 0.0],
            [100.0, 0.0, 0.0],
        ];
        let corners = vec![[0, 1, 2], [0, 2, 3]];
        let surface = Surface::new(&[[
            [50.0, 50.0, 1000.0],
            [60.0, 50.0, 1000.0],
            [50.0, 60.0, 1000.0],
        ]]);
        let sampled = sample(
            &positions,
            &corners,
            &|point| point,
            &surface,
            &Detail::FINE,
        );
        assert_eq!(sampled.corners, corners);
        assert_eq!(sampled.positions, positions);
    }

    #[test]
    fn interpolation_is_safe_only_when_the_corners_pin_it_down() {
        // Far away, a triangle's distances can't stray much in proportion.
        let error = Detail::FINE.max_u_error;
        assert!(interpolation_is_safe([1000.0, 1010.0, 1020.0], 20.0, error));
        // Close to the reference they can: it may even touch it in the middle.
        assert!(!interpolation_is_safe([1.0, 1.0, 1.0], 5.0, error));
        assert!(!interpolation_is_safe([0.001; 3], 10.0, error));
        // Past either end of the scale, U doesn't change.
        assert!(interpolation_is_safe([2e5, 3e5, 2e5], 1.0, error));
        assert!(interpolation_is_safe([0.001; 3], 0.001, error));
    }

    #[test]
    fn normals_point_out_of_clockwise_fronts() {
        // The usual outward winding is counter-clockwise; Unreal's is the reverse.
        let (positions, corners) = cube(100.0);
        let corners: Vec<[u32; 3]> = corners.iter().map(|&[a, b, c]| [a, c, b]).collect();
        let far = Surface::new(&[[[0.0, 0.0, 1e6], [1.0, 0.0, 1e6], [0.0, 1.0, 1e6]]]);
        for surface in [&far, &Surface::default()] {
            let sampled = sample(&positions, &corners, &|point| point, surface, &Detail::FINE);
            assert_eq!(sampled.normals.len(), sampled.positions.len());
            for (position, normal) in sampled.positions.iter().zip(&sampled.normals) {
                let length = length(normal.map(f64::from));
                assert!((length - 1.0).abs() < 1e-5, "{normal:?}");
                let outward = [0, 1, 2].map(|axis| position[axis] - 50.0);
                let along = dot(normal.map(f64::from), outward.map(f64::from));
                assert!(along > 0.0, "{position:?} has the normal {normal:?}");
            }
        }
    }

    /// A square from `(0, 0, z)` to `(size, size, z)`, two triangles.
    fn square(size: f32, z: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let positions = vec![
            [0.0, 0.0, z],
            [size, 0.0, z],
            [size, size, z],
            [0.0, size, z],
        ];
        (positions, vec![[0, 1, 2], [0, 2, 3]])
    }

    /// A flat reference at height 0, reaching past `size` on every side.
    fn floor(size: f32) -> Vec<[[f32; 3]; 3]> {
        let (low, high) = (-size, 2.0 * size);
        vec![
            [[low, low, 0.0], [high, low, 0.0], [high, high, 0.0]],
            [[low, low, 0.0], [high, high, 0.0], [low, high, 0.0]],
        ]
    }

    /// The share of the mesh's area whose interpolated U is off from the true one by more than
    /// `tolerance`, from seven points in every triangle.
    fn share_off(sampled: &Sampled, surface: &Surface, tolerance: f32) -> f64 {
        let weights: [[f32; 3]; 7] = [
            [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0],
            [2.0 / 3.0, 1.0 / 6.0, 1.0 / 6.0],
            [1.0 / 6.0, 2.0 / 3.0, 1.0 / 6.0],
            [1.0 / 6.0, 1.0 / 6.0, 2.0 / 3.0],
            [1.0 / 6.0, 5.0 / 12.0, 5.0 / 12.0],
            [5.0 / 12.0, 1.0 / 6.0, 5.0 / 12.0],
            [5.0 / 12.0, 5.0 / 12.0, 1.0 / 6.0],
        ];
        let (mut off, mut total) = (0.0, 0.0);
        for triangle in &sampled.corners {
            let points = triangle.map(|i| sampled.positions[i as usize]);
            let us = triangle.map(|i| sampled.uvs[i as usize][0]);
            let [a, b, c] = points.map(|point| point.map(f64::from));
            let area = length(cross(sub(b, a), sub(c, a))) / 2.0;
            for weight in weights {
                let point =
                    [0, 1, 2].map(|axis| (0..3).map(|k| weight[k] * points[k][axis]).sum::<f32>());
                let interpolated: f32 = (0..3).map(|k| weight[k] * us[k]).sum();
                let true_u = scale_u(surface.distance(point).unwrap());
                total += area;
                if (interpolated - true_u).abs() > tolerance {
                    off += area;
                }
            }
        }
        off / total
    }

    /// The longest edge of the finest triangle with a corner within `within` of the circle of
    /// `radius` around `center` (in X and Y).
    fn finest_near_circle(sampled: &Sampled, center: [f32; 2], radius: f32, within: f32) -> f32 {
        sampled
            .corners
            .iter()
            .map(|triangle| triangle.map(|i| sampled.positions[i as usize]))
            .filter(|points| {
                points.iter().any(|point| {
                    let (x, y) = (point[0] - center[0], point[1] - center[1]);
                    ((x * x + y * y).sqrt() - radius).abs() < within
                })
            })
            .map(|[a, b, c]| {
                [(a, b), (b, c), (c, a)]
                    .map(|(p, q)| distance_squared_f32(p, q).sqrt())
                    .into_iter()
                    .fold(0.0, f32::max)
            })
            .fold(f32::INFINITY, f32::min)
    }

    #[test]
    fn a_giant_face_lying_on_the_reference_stays_coarse() {
        // Ten meters wide, a tenth of a millimeter or less from the reference: right as it is.
        let surface = Surface::new(&floor(1000.0));
        for z in [0.0, 0.001, 0.005] {
            let (positions, corners) = square(1000.0, z);
            let sampled = sample(&positions, &corners, &|p| p, &surface, &Detail::FINE);
            assert!(sampled.corners.len() <= 8, "{z}: {}", sampled.corners.len());
            assert_eq!(share_off(&sampled, &surface, 0.01), 0.0);
        }
    }

    #[test]
    fn measured_errors_get_the_budget_before_precautions() {
        // A ten meter face half a centimeter above the reference, everywhere long enough to be
        // split as a precaution, with a small ball of the reference poking through it: the circle
        // where they meet still gets millimeter triangles.
        let mut triangles = floor(1000.0);
        // Through the middle of the face's diagonal, measured from the start: a ball between the
        // points measured could only be found by the precautions.
        triangles.extend(sphere([500.0, 503.0, 0.5], 3.0, 24, 32));
        let surface = Surface::new(&triangles);
        let (positions, corners) = square(1000.0, 0.5);
        let sampled = sample(&positions, &corners, &|p| p, &surface, &Detail::FINE);
        assert!(sampled.corners.len() <= Detail::FINE.max_triangles);
        let finest = finest_near_circle(&sampled, [500.0, 503.0], 3.0, 0.5);
        assert!(finest <= 0.1, "{finest} cm");
    }

    #[test]
    fn fine_colors_are_right_almost_everywhere() {
        // A 20 cm plate cutting through a ball: the colors change fast around the circle where
        // they meet, and slowly away from it.
        let surface = Surface::new(&sphere([10.0, 10.0, 1.0], 4.0, 32, 48));
        let (positions, corners) = square(20.0, 0.0);
        let fine = sample(&positions, &corners, &|p| p, &surface, &Detail::FINE);
        let preview = sample(&positions, &corners, &|p| p, &surface, &Detail::PREVIEW);
        assert!(fine.corners.len() <= Detail::FINE.max_triangles);
        assert!(preview.corners.len() <= Detail::PREVIEW.max_triangles);
        assert!(preview.corners.len() < fine.corners.len());

        // Off by more than a factor of 1.4 (0.15 of a decade) on under 2% of the area.
        let fine_off = share_off(&fine, &surface, 0.15 / 7.0);
        let preview_off = share_off(&preview, &surface, 0.15 / 7.0);
        assert!(fine_off < 0.02, "fine: {fine_off}");
        assert!(
            fine_off < preview_off,
            "fine {fine_off}, preview {preview_off}"
        );
        let finest = finest_near_circle(&fine, [10.0, 10.0], 15f32.sqrt(), 0.3);
        assert!(finest <= 0.1, "{finest} cm");
    }

    #[test]
    fn the_preview_keeps_its_budget() {
        // The dense sheet of `the_budget_is_kept`.
        let positions = vec![[0.0, 0.0, 0.0], [1000.0, 0.0, 0.0], [0.0, 1000.0, 0.0]];
        let sheet: Vec<[[f32; 3]; 3]> = (0..40)
            .flat_map(|i| {
                (0..40).map(move |j| {
                    let (x, y) = (i as f32 * 25.0, j as f32 * 25.0);
                    [[x, y, 3.0], [x + 10.0, y, 0.0], [x, y + 10.0, 8.0]]
                })
            })
            .collect();
        let surface = Surface::new(&sheet);
        let sampled = sample(&positions, &[[0, 1, 2]], &|p| p, &surface, &Detail::PREVIEW);
        assert!(sampled.corners.len() > 100);
        assert!(sampled.corners.len() <= Detail::PREVIEW.max_triangles);
    }

    /// How long each detail takes on a few bricks against a dense reference. Run with
    /// `cargo test --release -p model_io -- --ignored --nocapture timings`.
    #[test]
    #[ignore]
    fn timings() {
        let mut triangles = sphere([10.0, 10.0, 1.0], 4.0, 300, 400);
        triangles.extend(sphere([60.0, 30.0, -20.0], 25.0, 300, 400));
        let surface = Surface::new(&triangles);
        let cases = [
            ("plate through a small ball", square(20.0, 0.0)),
            ("plate over a big ball", square(100.0, 5.0)),
            ("plate far away", square(20.0, 500.0)),
        ];
        for (name, (positions, corners)) in &cases {
            for (detail_name, detail) in [("preview", Detail::PREVIEW), ("fine", Detail::FINE)] {
                let started = std::time::Instant::now();
                let sampled = sample(positions, corners, &|p| p, &surface, &detail);
                let took = started.elapsed();
                let off = share_off(&sampled, &surface, 0.15 / 7.0);
                println!(
                    "{name}, {detail_name}: {} triangles in {:.2} ms, {:.2}% off",
                    sampled.corners.len(),
                    took.as_secs_f64() * 1000.0,
                    off * 100.0
                );
            }
        }
    }

    #[test]
    fn vertices_without_a_direction_get_a_default_normal() {
        let positions = [[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]];
        let normals = vertex_normals(&positions, &[[0, 1, 2]]);
        assert_eq!(normals, vec![[0.0, 0.0, 1.0]; 3]);
    }
}
