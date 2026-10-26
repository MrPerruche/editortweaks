//! Bricks' models as faces, edges and vertices, for fast editing on models
//! ([`super::keybinds::MODEL_EDITING`]): built once per mesh from its triangles
//! (`game::brick_model`), then scaled for each brick like the game scales its mesh.
//!
//! - A face is a set of connected triangles in one plane: the sides of a cylinder are one face per
//!   segment, its caps one face each.
//! - An edge is where two faces meet (or where the mesh ends), straight: segments in a line along
//!   the same faces make one edge.
//! - A vertex is where edges end.
//!
//! Scaling a model along its axes keeps planes planes and lines lines, so the faces, edges and
//! vertices found at the mesh's size hold for every brick showing it.

use std::collections::HashMap;
use std::sync::Arc;

use brickrust::ue::coreuobject::UObjectBase;

use super::draw::PixelSize;
use super::geometry::{self, Bounds, Ray, Vec3};
use super::parts::{CORNER_PIXELS, EDGE_PIXELS, PART_SHARE};
use crate::game::brick_model::{self, Triangles};
use crate::game::weak_object::WeakObject;

/// How far a point may be from a plane to be in it, as a share of the mesh's size (the length of
/// its box's diagonal): far above `f32` noise, far below any real step.
const PLANE_TOLERANCE: f64 = 1e-5;
/// How far from a straight line (as the sine of the angle) two segments may turn to make one edge.
const LINE_TOLERANCE: f64 = 1e-5;

/// A part of a model: one of its vertices (a point's index), edges or faces.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Element {
    Vertex(u32),
    Edge(u32),
    Face(u32),
}

pub struct Face {
    /// Its normal, outward (the triangles' winding), in the mesh.
    normal: Vec3,
    /// The center of its area, in the mesh.
    center: Vec3,
    /// Its triangles.
    triangles: Vec<u32>,
    /// The edges around it (holes included).
    edges: Vec<u32>,
    /// Its vertices.
    vertices: Vec<u32>,
}

pub struct Edge {
    /// Its two ends (points).
    ends: [u32; 2],
}

/// A mesh's faces, edges and vertices, in its own space.
pub struct Model {
    points: Vec<Vec3>,
    triangles: Vec<[u32; 3]>,
    /// The face each triangle belongs to.
    triangle_faces: Vec<u32>,
    faces: Vec<Face>,
    edges: Vec<Edge>,
    /// The points that are vertices.
    vertices: Vec<u32>,
    bounds: Bounds,
}

impl Model {
    /// The model of a mesh with these triangles, if it has any.
    pub fn new(mesh: &Triangles) -> Option<Self> {
        // Points at the same place are one (PhysX welds them already, but keep it sure).
        let mut index_of: HashMap<[u32; 3], u32> = HashMap::new();
        let mut points: Vec<Vec3> = Vec::new();
        let welded: Vec<u32> = mesh
            .vertices
            .iter()
            .map(|vertex| {
                // `+ 0.0` turns -0.0 into 0.0.
                let key = vertex.map(|coordinate| (coordinate + 0.0).to_bits());
                *index_of.entry(key).or_insert_with(|| {
                    points.push(vertex.map(f64::from));
                    (points.len() - 1) as u32
                })
            })
            .collect();
        let bounds = bounds_of(&points)?;
        let tolerance = PLANE_TOLERANCE * geometry::length(geometry::sub(bounds.max, bounds.min));

        let mut triangles = Vec::new();
        let mut normals = Vec::new();
        for triangle in &mesh.triangles {
            let triangle = triangle.map(|index| welded[index as usize]);
            let [a, b, c] = triangle.map(|index| points[index as usize]);
            let Some(normal) =
                geometry::normalized(geometry::cross(geometry::sub(b, a), geometry::sub(c, a)))
            else {
                continue;
            };
            triangles.push(triangle);
            normals.push(normal);
        }
        if triangles.is_empty() {
            return None;
        }

        // The triangles around each segment.
        let mut around: HashMap<[u32; 2], Vec<u32>> = HashMap::new();
        for (index, triangle) in triangles.iter().enumerate() {
            for corner in 0..3 {
                let (a, b) = (triangle[corner], triangle[(corner + 1) % 3]);
                around
                    .entry([a.min(b), a.max(b)])
                    .or_default()
                    .push(index as u32);
            }
        }

        // Faces: triangles joined across segments where both lie in one plane.
        let point = |index: u32| points[index as usize];
        let in_plane = |triangle: usize, other: usize| {
            let corner = point(triangles[triangle][0]);
            triangles[other].iter().all(|&index| {
                geometry::dot(normals[triangle], geometry::sub(point(index), corner)).abs()
                    <= tolerance
            })
        };
        let mut sets = Sets::new(triangles.len());
        for list in around.values() {
            for (position, &first) in list.iter().enumerate() {
                for &second in &list[position + 1..] {
                    let (first, second) = (first as usize, second as usize);
                    if geometry::dot(normals[first], normals[second]) > 0.0
                        && in_plane(first, second)
                        && in_plane(second, first)
                    {
                        sets.join(first, second);
                    }
                }
            }
        }
        let mut face_of_set: HashMap<usize, u32> = HashMap::new();
        let mut faces: Vec<Face> = Vec::new();
        let mut area_normals: Vec<Vec3> = Vec::new();
        // Per face, its triangles' centers weighted by their areas, and its area (both doubled).
        let mut moments: Vec<(Vec3, f64)> = Vec::new();
        let triangle_faces: Vec<u32> = (0..triangles.len())
            .map(|triangle| {
                let face = *face_of_set.entry(sets.find(triangle)).or_insert_with(|| {
                    faces.push(Face {
                        normal: [0.0; 3],
                        center: [0.0; 3],
                        triangles: Vec::new(),
                        edges: Vec::new(),
                        vertices: Vec::new(),
                    });
                    area_normals.push([0.0; 3]);
                    moments.push(([0.0; 3], 0.0));
                    (faces.len() - 1) as u32
                });
                let [a, b, c] = triangles[triangle].map(point);
                let area = geometry::cross(geometry::sub(b, a), geometry::sub(c, a));
                area_normals[face as usize] = geometry::add(area_normals[face as usize], area);
                let (weighted, total) = &mut moments[face as usize];
                let size = geometry::length(area);
                let middle = geometry::add(geometry::add(a, b), c);
                *weighted = geometry::add(*weighted, geometry::scale(middle, size / 3.0));
                *total += size;
                faces[face as usize].triangles.push(triangle as u32);
                face
            })
            .collect();
        for ((face, area), &(weighted, total)) in faces.iter_mut().zip(&area_normals).zip(&moments)
        {
            face.normal = geometry::normalized(*area).unwrap_or([0.0, 0.0, 1.0]);
            // Triangles without area were left out: every face has some.
            face.center = geometry::scale(weighted, 1.0 / total);
        }

        // Segments where faces end (a face going on across a segment has a triangle on both sides:
        // twice around it; a two-sided panel is two faces going on together), then edges: segments
        // in a line along the same faces, joined.
        let mut segments: Vec<([u32; 2], Vec<u32>)> = Vec::new();
        for (&ends, list) in &around {
            let mut faces: Vec<u32> = list
                .iter()
                .map(|&triangle| triangle_faces[triangle as usize])
                .collect();
            faces.sort_unstable();
            let ending: Vec<u32> = faces
                .iter()
                .copied()
                .filter(|&face| faces.iter().filter(|&&other| other == face).count() == 1)
                .collect();
            if !ending.is_empty() {
                segments.push((ends, ending));
            }
        }
        let mut at_point: HashMap<u32, Vec<usize>> = HashMap::new();
        for (index, (ends, _)) in segments.iter().enumerate() {
            for &end in ends {
                at_point.entry(end).or_default().push(index);
            }
        }
        let other_end = |segment: usize, end: u32| {
            let [a, b] = segments[segment].0;
            if a == end { b } else { a }
        };
        // The segment going on through `end` from `segment`, if `end` is no vertex.
        let through = |segment: usize, end: u32| -> Option<usize> {
            let [first, second] = at_point.get(&end)?.as_slice() else {
                return None;
            };
            let next = if *first == segment { *second } else { *first };
            if segments[next].1 != segments[segment].1 {
                return None;
            }
            let here = point(end);
            let back = geometry::normalized(geometry::sub(point(other_end(segment, end)), here))?;
            let ahead = geometry::normalized(geometry::sub(point(other_end(next, end)), here))?;
            let turned = geometry::length(geometry::cross(back, ahead));
            (geometry::dot(back, ahead) < 0.0 && turned <= LINE_TOLERANCE).then_some(next)
        };
        let mut used = vec![false; segments.len()];
        let mut edges: Vec<Edge> = Vec::new();
        // Edges by their ends: faces meeting at T-junctions (one split along an edge, its neighbor
        // not) give the same edge once per face.
        let mut edge_at: HashMap<[u32; 2], u32> = HashMap::new();
        for start in 0..segments.len() {
            if used[start] {
                continue;
            }
            used[start] = true;
            let ends = segments[start].0.map(|end| {
                let (mut segment, mut end) = (start, end);
                while let Some(next) = through(segment, end).filter(|&next| !used[next]) {
                    used[next] = true;
                    end = other_end(next, end);
                    segment = next;
                }
                end
            });
            let edge = *edge_at
                .entry([ends[0].min(ends[1]), ends[0].max(ends[1])])
                .or_insert_with(|| {
                    edges.push(Edge { ends });
                    (edges.len() - 1) as u32
                });
            for &face in &segments[start].1 {
                let around = &mut faces[face as usize].edges;
                if !around.contains(&edge) {
                    around.push(edge);
                }
            }
        }
        let mut vertices: Vec<u32> = edges.iter().flat_map(|edge| edge.ends).collect();
        vertices.sort_unstable();
        vertices.dedup();
        for face in &mut faces {
            face.vertices = face
                .edges
                .iter()
                .flat_map(|&edge| edges[edge as usize].ends)
                .collect();
            face.vertices.sort_unstable();
            face.vertices.dedup();
        }
        Some(Self {
            points,
            triangles,
            triangle_faces,
            faces,
            edges,
            vertices,
            bounds,
        })
    }
}

impl Model {
    /// How many triangles, faces, edges and vertices it has.
    pub fn counts(&self) -> [usize; 4] {
        [
            self.triangles.len(),
            self.faces.len(),
            self.edges.len(),
            self.vertices.len(),
        ]
    }

    /// Its box, in the mesh's space.
    pub fn bounds(&self) -> &Bounds {
        &self.bounds
    }
}

/// The smallest box around `points`, if any.
fn bounds_of(points: &[Vec3]) -> Option<Bounds> {
    let first = *points.first()?;
    Some(points.iter().fold(
        Bounds {
            min: first,
            max: first,
        },
        |bounds, point| Bounds {
            min: core::array::from_fn(|axis| bounds.min[axis].min(point[axis])),
            max: core::array::from_fn(|axis| bounds.max[axis].max(point[axis])),
        },
    ))
}

/// Disjoint sets of indices (union-find), to join triangles into faces.
struct Sets(Vec<usize>);

impl Sets {
    fn new(count: usize) -> Self {
        Self((0..count).collect())
    }

    fn find(&mut self, mut index: usize) -> usize {
        while self.0[index] != index {
            self.0[index] = self.0[self.0[index]];
            index = self.0[index];
        }
        index
    }

    fn join(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        self.0[a] = b;
    }
}

/// Models made so far, by mesh: each is made once, from the mesh's triangles (meshes stay loaded
/// while bricks show them; one that's gone is made again).
#[derive(Default)]
pub struct Models(HashMap<usize, Made>);

struct Made {
    mesh: WeakObject,
    /// The model, or why there's none.
    model: Result<Arc<Model>, String>,
}

impl Models {
    /// The model of `mesh`, made from the triangles `triangles` gives the first time.
    ///
    /// # Safety
    /// `mesh` must be a live object. Game thread only.
    pub unsafe fn get(
        &mut self,
        mesh: *mut UObjectBase,
        triangles: impl FnOnce() -> Result<Triangles, String>,
    ) -> Result<Arc<Model>, String> {
        let key = mesh as usize;
        if let Some(made) = self.0.get(&key)
            && made.mesh.get() == Some(mesh)
        {
            return made.model.clone();
        }
        let model = triangles().and_then(|triangles| {
            Model::new(&triangles)
                .map(Arc::new)
                .ok_or("no triangles".into())
        });
        // SAFETY: per the contract.
        let mesh = unsafe { WeakObject::new(mesh) };
        self.0.insert(
            key,
            Made {
                mesh,
                model: model.clone(),
            },
        );
        model
    }

    /// How many meshes have a model, and how many were tried.
    pub fn counts(&self) -> (usize, usize) {
        let made = self.0.values().filter(|made| made.model.is_ok()).count();
        (made, self.0.len())
    }

    /// The counts of the model with the most triangles (see [`Model::counts`]).
    pub fn largest(&self) -> Option<[usize; 4]> {
        self.0
            .values()
            .filter_map(|made| made.model.as_ref().ok())
            .map(|model| model.counts())
            .max()
    }
}

/// How the mouse points at a model.
pub enum Pointing {
    /// The ray hits it, `distance` along at `point`: on `element`, the face it hits or a vertex or
    /// edge of that face near the mouse.
    On {
        element: Element,
        point: Vec3,
        distance: f64,
    },
    /// The mouse is beside it, by a vertex or edge: `point` is that part's point nearest the ray.
    Beside { element: Element, point: Vec3 },
}

/// A model as a brick shows it: scaled along the brick's axes, around its origin.
#[derive(Clone)]
pub struct BrickModel {
    model: Arc<Model>,
    scale: Vec3,
}

impl BrickModel {
    /// `model` scaled to a brick's box `exact` (see `super::sized`), turned inside out along the
    /// axes the brick is `mirrored` along. `None` unless the box is the model's box scaled around
    /// the origin (e.g. a brick adding other meshes to its box).
    pub fn fit(model: Arc<Model>, mirrored: [bool; 3], exact: &Bounds) -> Option<Self> {
        let scale = brick_model::scale_to_fit(
            [model.bounds.min, model.bounds.max],
            mirrored,
            [exact.min, exact.max],
        )?;
        Some(Self { model, scale })
    }

    /// `model` as it is: made in the brick's own space (a spinner's, `super::spinner`).
    pub fn unscaled(model: Arc<Model>) -> Self {
        Self {
            model,
            scale: [1.0; 3],
        }
    }

    /// A point of the model (by index), in the brick.
    pub fn point(&self, index: u32) -> Vec3 {
        let point = self.model.points[index as usize];
        core::array::from_fn(|axis| point[axis] * self.scale[axis])
    }

    /// A face's normal, in the brick (length 1).
    pub fn normal(&self, face: u32) -> Vec3 {
        let normal = self.model.faces[face as usize].normal;
        let scaled = core::array::from_fn(|axis| normal[axis] / self.scale[axis]);
        geometry::normalized(scaled).unwrap_or(normal)
    }

    /// A face's center (of its area), in the brick: scaling along axes keeps it the center.
    pub fn center(&self, face: u32) -> Vec3 {
        let center = self.model.faces[face as usize].center;
        core::array::from_fn(|axis| center[axis] * self.scale[axis])
    }

    /// Its faces (by index).
    pub fn faces(&self) -> core::ops::Range<u32> {
        0..self.model.faces.len() as u32
    }

    /// Whether `point` (in the brick, in the face's plane) is on the face, its edges included.
    pub fn on_face(&self, face: u32, point: Vec3) -> bool {
        // In the mesh, where the triangles are.
        let point: Vec3 = core::array::from_fn(|axis| point[axis] / self.scale[axis]);
        let corner = |index: u32| self.model.points[index as usize];
        self.model.faces[face as usize]
            .triangles
            .iter()
            .any(|&triangle| {
                let [a, b, c] = self.model.triangles[triangle as usize].map(corner);
                in_triangle(point, a, b, c)
            })
    }

    /// Its vertices (by point index) and where they are, in the brick.
    pub fn vertices(&self) -> impl Iterator<Item = (u32, Vec3)> + '_ {
        self.model
            .vertices
            .iter()
            .map(|&vertex| (vertex, self.point(vertex)))
    }

    /// Its edges (by index) and their ends, in the brick.
    pub fn edges(&self) -> impl Iterator<Item = (u32, [Vec3; 2])> + '_ {
        (0..self.model.edges.len() as u32).map(|edge| (edge, self.edge(edge)))
    }

    /// An edge's ends, in the brick.
    pub fn edge(&self, edge: u32) -> [Vec3; 2] {
        self.model.edges[edge as usize]
            .ends
            .map(|end| self.point(end))
    }

    /// The edges around a face, in the brick.
    pub fn face_edges(&self, face: u32) -> impl Iterator<Item = [Vec3; 2]> + '_ {
        self.model.faces[face as usize]
            .edges
            .iter()
            .map(|&edge| self.edge(edge))
    }

    /// The other ends of the edges leaving a vertex, in the brick.
    pub fn neighbors(&self, vertex: u32) -> impl Iterator<Item = Vec3> + '_ {
        self.model
            .edges
            .iter()
            .filter_map(move |edge| match edge.ends {
                [a, b] if a == vertex => Some(self.point(b)),
                [a, b] if b == vertex => Some(self.point(a)),
                _ => None,
            })
    }

    /// What the mouse points at on the model: `ray` and `pixels` in the brick.
    pub fn pointed(&self, ray: &Ray, pixels: &PixelSize) -> Option<Pointing> {
        if let Some((distance, face)) = self.hit(ray) {
            let point = geometry::along(ray, distance);
            return Some(Pointing::On {
                element: self.under(face, point, ray, pixels),
                point,
                distance,
            });
        }
        self.beside(ray, pixels)
            .map(|(element, point)| Pointing::Beside { element, point })
    }

    /// Where `ray` (in the brick) first meets the model (either side of a triangle; facing it first
    /// where two are as far), and the face there.
    pub fn hit(&self, ray: &Ray) -> Option<(f64, u32)> {
        // In the mesh: points stay unscaled, and distances along the ray stay the same.
        let origin: Vec3 = core::array::from_fn(|axis| ray.origin[axis] / self.scale[axis]);
        let direction: Vec3 = core::array::from_fn(|axis| ray.direction[axis] / self.scale[axis]);
        let point = |index: u32| self.model.points[index as usize];
        let mut nearest: Option<(f64, bool, u32)> = None;
        for (index, triangle) in self.model.triangles.iter().enumerate() {
            let [a, b, c] = triangle.map(point);
            let Some((distance, facing)) = ray_triangle(origin, direction, a, b, c) else {
                continue;
            };
            let closer = nearest.is_none_or(|(best, best_facing, _)| {
                let tie = (distance - best).abs() <= 1e-9 * best.max(1.0);
                if tie {
                    facing && !best_facing
                } else {
                    distance < best
                }
            });
            if closer {
                nearest = Some((distance, facing, self.model.triangle_faces[index]));
            }
        }
        nearest.map(|(distance, _, face)| (distance, face))
    }

    /// The part of `face` under the mouse, the ray hitting it at `point`: one of its vertices within
    /// [`CORNER_PIXELS`], else one of its edges within [`EDGE_PIXELS`], else the face; less on a face
    /// that looks thin there (see [`PART_SHARE`]), like a box's parts (`super::parts::part_under`).
    fn under(&self, face: u32, point: Vec3, ray: &Ray, pixels: &PixelSize) -> Element {
        let normal = self.normal(face);
        let outline = &self.model.faces[face as usize];
        let edges: Vec<(u32, [Vec3; 2])> = outline
            .edges
            .iter()
            .map(|&edge| (edge, self.edge(edge)))
            .collect();
        // How far the mouse is from each edge (pixels).
        let near: Vec<f32> = edges
            .iter()
            .map(|&(_, [start, end])| {
                to_ray_pixels(ray, pixels, geometry::closest_on_segment(ray, start, end))
            })
            .collect();
        // How wide the face looks across edge `index`, through the mouse (pixels): only asked for
        // the few parts near the mouse.
        let width = |index: usize| {
            let (edge, [start, end]) = edges[index];
            let far = match exit(&edges, edge, start, end, point, normal) {
                Some((direction, distance)) => {
                    screen_pixels(ray, pixels, point, geometry::scale(direction, distance))
                }
                None => f32::INFINITY,
            };
            near[index] + far
        };
        let nearest_vertex = outline
            .vertices
            .iter()
            .filter_map(|&vertex| {
                let distance = to_ray_pixels(ray, pixels, self.point(vertex));
                if distance > CORNER_PIXELS {
                    return None;
                }
                let across = (0..edges.len())
                    .filter(|&index| {
                        self.model.edges[edges[index].0 as usize]
                            .ends
                            .contains(&vertex)
                    })
                    .map(width)
                    .fold(f32::INFINITY, f32::min);
                Some((distance / CORNER_PIXELS.min(PART_SHARE * across), vertex))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((share, vertex)) = nearest_vertex
            && share <= 1.0
        {
            return Element::Vertex(vertex);
        }
        let nearest_edge = (0..edges.len())
            .filter(|&index| near[index] <= EDGE_PIXELS)
            .map(|index| {
                let reach = EDGE_PIXELS.min(PART_SHARE * width(index));
                (near[index] / reach, edges[index].0)
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        match nearest_edge {
            Some((share, edge)) if share <= 1.0 => Element::Edge(edge),
            _ => Element::Face(face),
        }
    }

    /// The vertex within [`CORNER_PIXELS`] of the mouse, else the edge within [`EDGE_PIXELS`] (the
    /// nearest), with its point nearest the ray, when the mouse is beside the model.
    fn beside(&self, ray: &Ray, pixels: &PixelSize) -> Option<(Element, Vec3)> {
        let in_front =
            |point: Vec3| geometry::dot(geometry::sub(point, ray.origin), ray.direction) > 0.0;
        let nearest_vertex = self
            .model
            .vertices
            .iter()
            .map(|&vertex| (vertex, self.point(vertex)))
            .filter(|&(_, point)| in_front(point))
            .map(|(vertex, point)| (to_ray_pixels(ray, pixels, point), vertex, point))
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((distance, vertex, point)) = nearest_vertex
            && distance <= CORNER_PIXELS
        {
            return Some((Element::Vertex(vertex), point));
        }
        let (distance, edge, point) = (0..self.model.edges.len() as u32)
            .map(|edge| {
                let [start, end] = self.edge(edge);
                (edge, geometry::closest_on_segment(ray, start, end))
            })
            .filter(|&(_, point)| in_front(point))
            .map(|(edge, point)| (to_ray_pixels(ray, pixels, point), edge, point))
            .min_by(|a, b| a.0.total_cmp(&b.0))?;
        (distance <= EDGE_PIXELS).then_some((Element::Edge(edge), point))
    }
}

/// How far `point` is from the ray, in pixels.
fn to_ray_pixels(ray: &Ray, pixels: &PixelSize, point: Vec3) -> f32 {
    let offset = geometry::sub(point, ray.origin);
    let along = geometry::dot(offset, ray.direction);
    let across = geometry::length(geometry::sub(offset, geometry::scale(ray.direction, along)));
    (across / pixels.at(point)) as f32
}

/// How long `offset` from `point` looks on the screen (pixels), seen along the ray.
fn screen_pixels(ray: &Ray, pixels: &PixelSize, point: Vec3, offset: Vec3) -> f32 {
    let along = geometry::dot(offset, ray.direction);
    let across = geometry::length(geometry::sub(offset, geometry::scale(ray.direction, along)));
    (across / pixels.at(point)) as f32
}

/// From `point` on a face (with `normal`), going straight away from its edge `edge` (from `start`
/// to `end`): the direction, and how far the face goes that way (to its first other edge).
fn exit(
    edges: &[(u32, [Vec3; 2])],
    edge: u32,
    start: Vec3,
    end: Vec3,
    point: Vec3,
    normal: Vec3,
) -> Option<(Vec3, f64)> {
    let along = geometry::normalized(geometry::sub(end, start))?;
    let mut inward = geometry::normalized(geometry::cross(normal, along))?;
    if geometry::dot(inward, geometry::sub(point, start)) < 0.0 {
        inward = geometry::scale(inward, -1.0);
    }
    let side = geometry::cross(normal, inward);
    // In the face's plane, with `point` at the origin, `inward` along the first axis.
    let flat = |corner: Vec3| {
        let offset = geometry::sub(corner, point);
        (geometry::dot(offset, inward), geometry::dot(offset, side))
    };
    let distance = edges
        .iter()
        .filter(|&&(other, _)| other != edge)
        .filter_map(|&(_, [a, b])| {
            let ((ax, ay), (bx, by)) = (flat(a), flat(b));
            if (ay > 0.0 && by > 0.0) || (ay < 0.0 && by < 0.0) || ay == by {
                return None;
            }
            let x = ax + (bx - ax) * ay / (ay - by);
            (x > 1e-9).then_some(x)
        })
        .fold(f64::INFINITY, f64::min);
    distance.is_finite().then_some((inward, distance))
}

/// Whether `point` is in the triangle `a`, `b`, `c` (its edges included), once moved onto its plane.
fn in_triangle(point: Vec3, a: Vec3, b: Vec3, c: Vec3) -> bool {
    /// How far outside an edge a point may be, as a share of the triangle (rounding).
    const TOLERANCE: f64 = 1e-7;
    let (ab, ac, to_point) = (
        geometry::sub(b, a),
        geometry::sub(c, a),
        geometry::sub(point, a),
    );
    let (d00, d01, d11) = (
        geometry::dot(ab, ab),
        geometry::dot(ab, ac),
        geometry::dot(ac, ac),
    );
    let (d20, d21) = (geometry::dot(to_point, ab), geometry::dot(to_point, ac));
    let denominator = d00 * d11 - d01 * d01;
    if denominator <= 0.0 {
        return false;
    }
    // The point's barycentric coordinates: toward `b`, toward `c`, and `a`'s.
    let v = (d11 * d20 - d01 * d21) / denominator;
    let w = (d00 * d21 - d01 * d20) / denominator;
    v >= -TOLERANCE && w >= -TOLERANCE && v + w <= 1.0 + TOLERANCE
}

/// How far along a ray (any length of `direction`) it meets the triangle `a`, `b`, `c`, from
/// either side (Möller-Trumbore), and whether the triangle faces it (wound counterclockwise as
/// seen from the ray).
fn ray_triangle(origin: Vec3, direction: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<(f64, bool)> {
    let ab = geometry::sub(b, a);
    let ac = geometry::sub(c, a);
    let p = geometry::cross(direction, ac);
    let determinant = geometry::dot(ab, p);
    if determinant.abs() < 1e-18 {
        return None;
    }
    let inverse = 1.0 / determinant;
    let to_origin = geometry::sub(origin, a);
    let u = geometry::dot(to_origin, p) * inverse;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = geometry::cross(to_origin, ab);
    let v = geometry::dot(direction, q) * inverse;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let distance = geometry::dot(ac, q) * inverse;
    (distance > 0.0).then_some((distance, determinant > 0.0))
}
