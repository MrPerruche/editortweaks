//! What a resize snaps to with the snap key held ([`super::keybinds::SNAP_RESIZE`]): points of
//! other bricks near the mouse, like a CAD program's object snaps. Each kind can be turned off in
//! [`SNAPPING`]:
//!
//! - boxes' corners and models' vertices;
//! - halfway along edges (and thirds and quarters of them);
//! - faces' centers;
//! - where two bricks' edges cross, and where an edge goes through another brick's face;
//! - halfway between those crossings along an edge (or between one and the edge's end);
//! - where two bricks go into each other, the lines their faces meet along (seams, see [`seams`]):
//!   halfway along them (and thirds and quarters), or anywhere along them;
//! - else anywhere along the edge or seam the mouse is near, or on the face under it (its plane).
//!
//! The point nearest the mouse within [`POINT_PIXELS`] wins if it shows (no brick in front of it),
//! corners and crossings first, then middles (a few pixels ahead, see [`Kind::bias`]); else the
//! nearest edge or seam within [`EDGE_PIXELS`] that shows; else the face under the mouse. Of points
//! about as near the mouse (seen one behind the other), the one in front wins ([`TIE_PIXELS`]).
//! What it snapped to last stays a little ahead ([`STICKY_PIXELS`]), so it doesn't flicker between
//! close points.
//!
//! Bricks are seen as for pointing at them: their models with the model key held (those that have
//! one), else their boxes. Each right click sets one more of the bricks the mouse goes through
//! aside, front first (back to none after the last), to reach bricks behind them.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::draw::Mark;
use super::geometry::{self, Ray, Vec3};
use super::model::{BrickModel, Element};
use super::parts::{self, Part};
use super::resize::Target;
use super::scene::{Piece, Placed, Scene, View};

/// What snapping snaps to: each kind on (`true`) or off. Edit here; a settings menu may set them
/// later.
pub const SNAPPING: Snapping = Snapping {
    corners: true,
    midpoints: true,
    thirds: false,
    quarters: false,
    face_centers: true,
    edge_intersections: true,
    face_intersections: true,
    between_intersections: true,
    seams: true,
    edges: true,
    faces: true,
};

/// The kinds of points snapping snaps to (see [`SNAPPING`]).
#[derive(Clone, Copy, Debug)]
pub struct Snapping {
    /// Boxes' corners and models' vertices.
    pub corners: bool,
    /// Halfway along edges.
    pub midpoints: bool,
    /// A third of the way along edges, from either end.
    pub thirds: bool,
    /// A quarter of the way along edges, from either end.
    pub quarters: bool,
    /// Faces' centers (a model's face: the center of its area, like a cylinder cap's center).
    pub face_centers: bool,
    /// Where two bricks' edges cross.
    pub edge_intersections: bool,
    /// Where an edge goes through another brick's face.
    pub face_intersections: bool,
    /// Halfway between the points where other bricks cross an edge (with the kinds of crossings
    /// above that are on), or between one and the edge's end.
    pub between_intersections: bool,
    /// Where two bricks go into each other: along the lines their faces meet (anywhere, and with
    /// the kinds above that are on, halfway, a third or a quarter along them).
    pub seams: bool,
    /// Anywhere along an edge: its point nearest the mouse.
    pub edges: bool,
    /// Anywhere on a face: its plane, through the point under the mouse.
    pub faces: bool,
}

/// How near the mouse a point must be to snap to it, and an edge (pixels). Tweakable.
const POINT_PIXELS: f32 = parts::CORNER_PIXELS;
const EDGE_PIXELS: f32 = parts::EDGE_PIXELS;

/// How much nearer the mouse what was snapped to last counts (pixels): it holds against points
/// about as near.
const STICKY_PIXELS: f32 = 3.0;

/// How far apart two edges may pass and still cross, and how far from a face's plane an edge's end
/// may be and still end on it (cm): bricks placed to touch are a hair apart at most (their places
/// are `f32`), bricks placed apart are far more.
const CROSSING_TOLERANCE: f64 = 0.01;

/// Below this (the squared sine of their angle), two edges are taken as parallel: they don't cross
/// at one point.
const PARALLEL: f64 = 1e-10;

/// How much further from the mouse (pixels) a point may be than the best one and still win by
/// being in front of it: points seen one behind the other look as near the mouse.
const TIE_PIXELS: f32 = 1.5;
/// How much nearer the camera (cm) a point must be to be in front of another.
const SAME_DEPTH: f64 = 1e-6;

/// How far behind a brick's surface a point may be and still show (pixels at its distance): points
/// on the surface are only off it by rounding.
const HIDDEN_PIXELS: f64 = 1.0;

/// What a snap point is. The first kinds win ties.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Kind {
    /// A box's corner or a model's vertex.
    Corner,
    /// Where two bricks' edges cross.
    Intersection,
    /// Where an edge goes through another brick's face.
    Piercing,
    /// Halfway along an edge.
    Midpoint,
    /// A face's center.
    FaceCenter,
    /// Halfway between crossings along an edge.
    Between,
    /// A third or a quarter along an edge.
    Fraction,
    /// Along an edge: its point nearest the mouse.
    Edge,
    /// Along a seam (where two bricks' faces meet): its point nearest the mouse.
    Seam,
    /// On a face: the point under the mouse.
    Face,
}

impl Kind {
    /// How many pixels further from the mouse than other points a point of this kind may be and
    /// still win. Tweakable.
    fn bias(self) -> f32 {
        match self {
            Kind::Corner | Kind::Intersection | Kind::Piercing => 3.0,
            Kind::Midpoint | Kind::FaceCenter => 2.0,
            Kind::Between | Kind::Fraction => 1.0,
            Kind::Edge | Kind::Seam | Kind::Face => 0.0,
        }
    }

    /// The mark showing a snap point of this kind; a point on a face is marked lying on it.
    pub fn mark(self) -> Option<Mark> {
        Some(match self {
            Kind::Corner => Mark::Square,
            Kind::Intersection => Mark::Cross,
            Kind::Piercing => Mark::BoxedCross,
            Kind::Midpoint => Mark::Triangle,
            Kind::FaceCenter => Mark::Circle,
            Kind::Between => Mark::TriangleDown,
            Kind::Fraction => Mark::Diamond,
            Kind::Edge | Kind::Seam => Mark::Hourglass,
            Kind::Face => return None,
        })
    }

    /// What it is, for the log.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Corner => "a corner",
            Kind::Intersection => "where edges cross",
            Kind::Piercing => "where an edge goes through a face",
            Kind::Midpoint => "halfway along an edge",
            Kind::FaceCenter => "a face's center",
            Kind::Between => "halfway between crossings",
            Kind::Fraction => "a third or quarter of an edge",
            Kind::Edge => "an edge",
            Kind::Seam => "where two bricks' faces meet",
            Kind::Face => "a face",
        }
    }
}

/// What the dragged part snaps to.
pub struct Snap {
    pub kind: Kind,
    pub target: Target,
    /// The pieces of bricks it's on (by brick, see [`Scene::get`]): one, or two where bricks cross
    /// (a seam's: the two faces).
    pub on: Vec<(usize, Piece)>,
    /// For a point halfway between crossings: the stretch of the edge it's the middle of; on a
    /// seam: the seam.
    pub span: Option<[Vec3; 2]>,
}

impl Snap {
    /// Whether this is a point of `kind` on `on` at `point` (along an edge or seam or on a face,
    /// anywhere).
    fn is(&self, kind: Kind, on: &[(usize, Piece)], point: Vec3) -> bool {
        self.kind == kind
            && self.on == on
            && (matches!(kind, Kind::Edge | Kind::Seam | Kind::Face)
                || geometry::length(geometry::sub(self.target.point, point)) < 1e-6)
    }
}

/// Snapping while a brick is resized.
#[derive(Default)]
pub struct Snapper {
    /// What it snaps to now, and since when (for its mark's animation).
    snapped: Option<Snap>,
    since: Option<Instant>,
    /// How many of the bricks the mouse goes through to set aside (right clicks so far).
    depth: usize,
    /// Where other bricks cross edges, by brick, edge and whether models were seen: found once per
    /// edge (bricks don't change while a resize lasts, but the resized ones, left out).
    crossings: HashMap<(usize, Piece, bool), Vec<f64>>,
    /// The seams between two bricks, by brick (the first one first) and whether models were seen:
    /// found once per pair, like crossings.
    seams: HashMap<(usize, usize, bool), Vec<Seam>>,
}

impl Snapper {
    /// What it snaps to now.
    pub fn snapped(&self) -> Option<&Snap> {
        self.snapped.as_ref()
    }

    /// How long it has snapped to the same point (or along the same edge or face).
    pub fn snapped_for(&self) -> Duration {
        self.since.map_or(Duration::ZERO, |since| since.elapsed())
    }

    /// Sets one more of the bricks the mouse goes through aside.
    pub fn deeper(&mut self) {
        self.depth += 1;
    }

    /// Snaps to nothing (until the next [`Self::snap`]).
    pub fn stop(&mut self) {
        self.snapped = None;
        self.since = None;
    }

    /// Finds what to snap to with the mouse at `view` (see the module's description), among the
    /// bricks of `scene` but those `skip` says (the resized ones); their models with `models`.
    pub fn snap(
        &mut self,
        settings: &Snapping,
        scene: &Scene,
        view: &View,
        models: bool,
        skip: impl Fn(&Placed) -> bool,
    ) -> Option<&Snap> {
        let last = self.snapped.take();
        let seen = near(scene, view, models, &skip, self.depth);
        let edges = near_edges(&seen, view);
        let seams = if settings.seams {
            near_seams(&mut self.seams, &seen, view, models)
        } else {
            Vec::new()
        };
        let mut points = Points {
            view,
            seen: &seen,
            last: last.as_ref(),
            found: Vec::new(),
        };
        points.of_bricks(settings);
        points.along(&edges, &fractions(settings));
        points.along_seams(&seams, &fractions(settings));
        if settings.edge_intersections {
            points.intersections(&edges);
        }
        if settings.face_intersections {
            points.piercings(&edges);
        }
        if settings.between_intersections
            && (settings.edge_intersections || settings.face_intersections)
        {
            for edge in &edges {
                let index = seen[edge.brick].index;
                let shares = self
                    .crossings
                    .entry((index, edge.piece, models))
                    .or_insert_with(|| crossings(settings, scene, models, &skip, index, edge.ends));
                points.between(edge, shares);
            }
        }
        self.snapped = choose(
            settings,
            &seen,
            view,
            &edges,
            &seams,
            points.found,
            last.as_ref(),
        );
        let same = match (&self.snapped, &last) {
            (Some(now), Some(last)) => now.is(last.kind, &last.on, last.target.point),
            _ => false,
        };
        if !same {
            self.since = self.snapped.as_ref().map(|_| Instant::now());
        }
        self.snapped.as_ref()
    }
}

/// How snapping sees a brick, in its frame: its model, or its box.
#[derive(Clone, Copy)]
enum Look<'a> {
    Box(&'a geometry::Bounds),
    Model(&'a BrickModel),
}

impl<'a> Look<'a> {
    fn of(placed: &'a Placed, models: bool) -> Self {
        match &placed.model {
            Some(model) if models => Look::Model(model),
            _ => Look::Box(&placed.bounds),
        }
    }

    /// Its corners (a box's) or vertices (a model's).
    fn corners(self) -> Vec<(Piece, Vec3)> {
        match self {
            Look::Box(bounds) => Part::all(3)
                .map(|part| (Piece::Box(part), bounds.point(part.sides)))
                .collect(),
            Look::Model(model) => model
                .vertices()
                .map(|(vertex, point)| (Piece::Model(Element::Vertex(vertex)), point))
                .collect(),
        }
    }

    /// Its edges and their ends.
    fn edges(self) -> Vec<(Piece, [Vec3; 2])> {
        match self {
            Look::Box(bounds) => Part::all(2)
                .map(|part| {
                    let corners = part.corners(bounds);
                    (Piece::Box(part), [corners[0], corners[1]])
                })
                .collect(),
            Look::Model(model) => model
                .edges()
                .map(|(edge, ends)| (Piece::Model(Element::Edge(edge)), ends))
                .collect(),
        }
    }

    /// The edges around its face `face`.
    fn face_edges(self, face: Piece) -> Vec<[Vec3; 2]> {
        match (self, face) {
            (Look::Box(bounds), Piece::Box(part)) => {
                let corners = part.corners(bounds);
                let next = |index: usize| corners[(index + 1) % corners.len()];
                (0..corners.len())
                    .map(|index| [corners[index], next(index)])
                    .collect()
            }
            (Look::Model(model), Piece::Model(Element::Face(face))) => {
                model.face_edges(face).collect()
            }
            _ => Vec::new(),
        }
    }

    /// Its faces.
    fn faces(self) -> Vec<Face> {
        match self {
            Look::Box(bounds) => Part::all(1)
                .map(|part| Face {
                    piece: Piece::Box(part),
                    center: bounds.point(part.sides),
                    normal: part.sides.map(f64::from),
                })
                .collect(),
            Look::Model(model) => model
                .faces()
                .map(|face| Face {
                    piece: Piece::Model(Element::Face(face)),
                    center: model.center(face),
                    normal: model.normal(face),
                })
                .collect(),
        }
    }

    /// Whether `point` (in the plane of its face `face`) is on that face.
    fn on_face(self, face: Piece, point: Vec3) -> bool {
        match (self, face) {
            (Look::Box(bounds), Piece::Box(part)) => {
                (0..3).filter(|&axis| part.sides[axis] == 0).all(|axis| {
                    let tolerance = 1e-9 * bounds.extent(axis).max(1.0);
                    point[axis] >= bounds.min[axis] - tolerance
                        && point[axis] <= bounds.max[axis] + tolerance
                })
            }
            (Look::Model(model), Piece::Model(Element::Face(face))) => model.on_face(face, point),
            _ => false,
        }
    }

    /// Where `ray` (in the brick) first meets it, the face there and that face's outward normal.
    fn hit(self, ray: &Ray) -> Option<(f64, Piece, Vec3)> {
        match self {
            Look::Box(bounds) => {
                geometry::ray_box(ray, bounds).map(|(distance, axis, positive)| {
                    let face = Part::face(axis, positive);
                    (distance, Piece::Box(face), face.sides.map(f64::from))
                })
            }
            Look::Model(model) => model.hit(ray).map(|(distance, face)| {
                let piece = Piece::Model(Element::Face(face));
                (distance, piece, model.normal(face))
            }),
        }
    }
}

/// A face of a brick, in its frame.
#[derive(Clone, Copy)]
struct Face {
    piece: Piece,
    center: Vec3,
    /// Outward, length 1.
    normal: Vec3,
}

/// A brick near the mouse.
struct Seen<'a> {
    /// Which (see [`Scene::get`]).
    index: usize,
    placed: &'a Placed,
    look: Look<'a>,
}

/// The bricks near the mouse (their boxes within [`POINT_PIXELS`] of it) but those `skip` says,
/// without the first `depth` the mouse goes through (counting again from the front past the last).
fn near<'a>(
    scene: &'a Scene,
    view: &View,
    models: bool,
    skip: impl Fn(&Placed) -> bool,
    depth: usize,
) -> Vec<Seen<'a>> {
    let ray = &view.ray;
    let reach = f64::from(POINT_PIXELS) * view.pixels.per_distance;
    // Each one, and where the mouse's ray meets it if it does.
    let mut near: Vec<(Option<f64>, Seen)> = Vec::new();
    for (index, placed) in scene.bricks().iter().enumerate() {
        if skip(placed) {
            continue;
        }
        let local = placed.frame.local_ray(ray);
        // Its box grown by the reach at its far end.
        let (center, radius) = sphere(placed);
        let far = geometry::length(geometry::sub(center, ray.origin)) + radius;
        if geometry::ray_box(&local, &placed.bounds.grown(reach * far)).is_none() {
            continue;
        }
        let look = Look::of(placed, models);
        let hit = look.hit(&local).map(|(distance, ..)| distance);
        near.push((
            hit,
            Seen {
                index,
                placed,
                look,
            },
        ));
    }
    let mut through: Vec<(f64, usize)> = near
        .iter()
        .filter_map(|(hit, seen)| Some(((*hit)?, seen.index)))
        .collect();
    through.sort_by(|a, b| a.0.total_cmp(&b.0));
    let aside: Vec<usize> = match through.len() {
        0 => Vec::new(),
        count => through[..depth % count]
            .iter()
            .map(|&(_, index)| index)
            .collect(),
    };
    near.into_iter()
        .map(|(_, seen)| seen)
        .filter(|seen| !aside.contains(&seen.index))
        .collect()
}

/// The ball around a brick's box: its center, in the vehicle, and its radius.
fn sphere(placed: &Placed) -> (Vec3, f64) {
    let center = placed.frame.point(placed.bounds.point([0; 3]));
    let radius = geometry::length(geometry::sub(placed.bounds.max, placed.bounds.min)) / 2.0;
    (center, radius)
}

/// An edge near the mouse.
struct NearEdge {
    /// Its brick (among those seen) and which edge.
    brick: usize,
    piece: Piece,
    /// Its ends, in the vehicle.
    ends: [Vec3; 2],
    /// Its point nearest the mouse's ray, and how far from the mouse that is (pixels).
    nearest: Vec3,
    pixels: f32,
}

/// The edges of the bricks `seen` passing within [`POINT_PIXELS`] of the mouse.
fn near_edges(seen: &[Seen], view: &View) -> Vec<NearEdge> {
    let mut edges = Vec::new();
    for (brick, seen) in seen.iter().enumerate() {
        let frame = &seen.placed.frame;
        for (piece, ends) in seen.look.edges() {
            let ends = ends.map(|end| frame.point(end));
            let nearest = geometry::closest_on_segment(&view.ray, ends[0], ends[1]);
            if let Some(pixels) = from_mouse(view, nearest)
                && pixels <= POINT_PIXELS
            {
                edges.push(NearEdge {
                    brick,
                    piece,
                    ends,
                    nearest,
                    pixels,
                });
            }
        }
    }
    edges
}

/// A seam: a line where a face of one brick crosses a face of another (each goes through the
/// other's plane), where the bricks go into each other.
#[derive(Clone)]
struct Seam {
    /// The two faces (by brick, see [`Scene::get`]).
    on: [(usize, Piece); 2],
    /// Its ends, in the vehicle.
    ends: [Vec3; 2],
}

/// A seam near the mouse.
struct NearSeam {
    seam: Seam,
    /// Its point nearest the mouse's ray, and how far from the mouse that is (pixels).
    nearest: Vec3,
    pixels: f32,
}

/// The seams between the bricks `seen` passing within [`POINT_PIXELS`] of the mouse. Those between
/// two bricks are found once, then kept in `found` (see [`Snapper::seams`]).
fn near_seams(
    found: &mut HashMap<(usize, usize, bool), Vec<Seam>>,
    seen: &[Seen],
    view: &View,
    models: bool,
) -> Vec<NearSeam> {
    let mut near = Vec::new();
    for (position, first) in seen.iter().enumerate() {
        for second in &seen[position + 1..] {
            let seams = found
                .entry((first.index, second.index, models))
                .or_insert_with(|| seams(first, second));
            for seam in seams.iter() {
                let nearest = geometry::closest_on_segment(&view.ray, seam.ends[0], seam.ends[1]);
                if let Some(pixels) = from_mouse(view, nearest)
                    && pixels <= POINT_PIXELS
                {
                    near.push(NearSeam {
                        seam: seam.clone(),
                        nearest,
                        pixels,
                    });
                }
            }
        }
    }
    near
}

/// A face of a brick in the vehicle, with the edges around it.
struct Outline {
    piece: Piece,
    center: Vec3,
    /// Outward, length 1.
    normal: Vec3,
    edges: Vec<[Vec3; 2]>,
}

impl Outline {
    /// How far `point` is above its plane (below: negative).
    fn height(&self, point: Vec3) -> f64 {
        geometry::dot(self.normal, geometry::sub(point, self.center))
    }

    /// Whether it goes through `plane`'s plane: it has corners on both sides.
    fn goes_through(&self, plane: &Outline) -> bool {
        let heights = self
            .edges
            .iter()
            .flatten()
            .map(|&point| plane.height(point));
        let (mut above, mut below) = (false, false);
        for height in heights {
            above |= height > CROSSING_TOLERANCE;
            below |= height < -CROSSING_TOLERANCE;
        }
        above && below
    }
}

/// The seams between the bricks `first` and `second`.
fn seams(first: &Seen, second: &Seen) -> Vec<Seam> {
    let ((first_center, first_radius), (second_center, second_radius)) =
        (sphere(first.placed), sphere(second.placed));
    let apart = geometry::length(geometry::sub(first_center, second_center));
    if apart > first_radius + second_radius + CROSSING_TOLERANCE {
        return Vec::new();
    }
    let second_faces = outlines(second, first);
    let mut seams = Vec::new();
    for first_face in outlines(first, second) {
        for second_face in &second_faces {
            for ends in face_seams((first, &first_face), (second, second_face)) {
                seams.push(Seam {
                    on: [
                        (first.index, first_face.piece),
                        (second.index, second_face.piece),
                    ],
                    ends,
                });
            }
        }
    }
    seams
}

/// The faces of the brick `seen` (in the vehicle) that reach into the box of `other`.
fn outlines(seen: &Seen, other: &Seen) -> Vec<Outline> {
    let frame = &seen.placed.frame;
    let reached = other.placed.bounds.grown(CROSSING_TOLERANCE);
    seen.look
        .faces()
        .into_iter()
        .filter_map(|face| {
            let edges: Vec<[Vec3; 2]> = seen
                .look
                .face_edges(face.piece)
                .into_iter()
                .map(|ends| ends.map(|end| frame.point(end)))
                .collect();
            // Its own box, in the other brick.
            let (mut min, mut max) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
            for &point in edges.iter().flatten() {
                let local = other.placed.frame.local(point);
                for axis in 0..3 {
                    min[axis] = min[axis].min(local[axis]);
                    max[axis] = max[axis].max(local[axis]);
                }
            }
            let reaches =
                (0..3).all(|axis| min[axis] <= reached.max[axis] && max[axis] >= reached.min[axis]);
            reaches.then(|| Outline {
                piece: face.piece,
                center: frame.point(face.center),
                normal: frame.direction(face.normal),
                edges,
            })
        })
        .collect()
}

/// Where the faces `first` and `second` (each with the brick it's of) cross, if each goes through
/// the other's plane: the stretches of the line their planes meet along that are on both faces.
/// That line goes on or off a face where an edge of one face meets the other's plane, so the
/// stretches between those points are each wholly on both faces or not.
fn face_seams(first: (&Seen, &Outline), second: (&Seen, &Outline)) -> Vec<[Vec3; 2]> {
    let ((first_seen, first), (second_seen, second)) = (first, second);
    let direction = geometry::cross(first.normal, second.normal);
    if geometry::dot(direction, direction) <= PARALLEL
        || !first.goes_through(second)
        || !second.goes_through(first)
    {
        return Vec::new();
    }
    // Where the edges of each face meet (or lie in) the other's plane, in order along the line.
    let mut stations: Vec<(f64, Vec3)> = Vec::new();
    for (face, plane) in [(first, second), (second, first)] {
        for &ends in &face.edges {
            let [start, end] = ends.map(|end| plane.height(end));
            let mut add = |point: Vec3| stations.push((geometry::dot(direction, point), point));
            if start.abs() <= CROSSING_TOLERANCE {
                add(ends[0]);
            }
            if end.abs() <= CROSSING_TOLERANCE {
                add(ends[1]);
            }
            let across = (start > CROSSING_TOLERANCE && end < -CROSSING_TOLERANCE)
                || (start < -CROSSING_TOLERANCE && end > CROSSING_TOLERANCE);
            if across {
                add(geometry::lerp(ends, start / (start - end)));
            }
        }
    }
    stations.sort_by(|a, b| a.0.total_cmp(&b.0));
    let close = CROSSING_TOLERANCE * geometry::length(direction);
    stations.dedup_by(|later, earlier| later.0 - earlier.0 <= close);
    let on = |seen: &Seen, face: &Outline, point: Vec3| {
        seen.look
            .on_face(face.piece, seen.placed.frame.local(point))
    };
    // The stretches between them on both faces, those next to each other joined.
    let mut seams: Vec<[Vec3; 2]> = Vec::new();
    for pair in stations.windows(2) {
        let [(_, start), (_, end)] = [pair[0], pair[1]];
        let middle = geometry::lerp([start, end], 0.5);
        if !on(first_seen, first, middle) || !on(second_seen, second, middle) {
            continue;
        }
        match seams.last_mut() {
            Some(last) if last[1] == start => last[1] = end,
            _ => seams.push([start, end]),
        }
    }
    seams
}

/// A point to snap to, near the mouse.
struct Point {
    kind: Kind,
    /// Where, in the vehicle.
    point: Vec3,
    on: Vec<(usize, Piece)>,
    span: Option<[Vec3; 2]>,
    /// How far from the mouse it is (pixels), less its kind's bias and what it keeps from being
    /// snapped to last: the lowest wins.
    score: f32,
}

impl Point {
    fn into_snap(self) -> Snap {
        Snap {
            kind: self.kind,
            target: Target {
                point: self.point,
                normal: None,
            },
            on: self.on,
            span: self.span,
        }
    }
}

/// The points to snap to found so far, near the mouse.
struct Points<'a> {
    view: &'a View,
    /// The bricks near the mouse.
    seen: &'a [Seen<'a>],
    /// What was snapped to last.
    last: Option<&'a Snap>,
    found: Vec<Point>,
}

impl Points<'_> {
    /// Adds a point (in the vehicle), if it's near the mouse.
    fn add(&mut self, kind: Kind, point: Vec3, on: &[(usize, Piece)], span: Option<[Vec3; 2]>) {
        let Some(pixels) = from_mouse(self.view, point) else {
            return;
        };
        if pixels > POINT_PIXELS {
            return;
        }
        self.found.push(Point {
            kind,
            point,
            on: on.to_vec(),
            span,
            score: pixels - kind.bias() - sticky(self.last, kind, on, point),
        });
    }

    /// The bricks' corners and faces' centers (with those kinds on).
    fn of_bricks(&mut self, settings: &Snapping) {
        for brick in self.seen {
            let frame = &brick.placed.frame;
            if settings.corners {
                for (piece, corner) in brick.look.corners() {
                    let on = [(brick.index, piece)];
                    self.add(Kind::Corner, frame.point(corner), &on, None);
                }
            }
            if settings.face_centers {
                for face in brick.look.faces() {
                    let on = [(brick.index, face.piece)];
                    self.add(Kind::FaceCenter, frame.point(face.center), &on, None);
                }
            }
        }
    }

    /// The points `fractions` of the way along `edges` (see [`fractions`]).
    fn along(&mut self, edges: &[NearEdge], fractions: &[(f64, Kind)]) {
        for edge in edges {
            let on = [(self.seen[edge.brick].index, edge.piece)];
            for &(share, kind) in fractions {
                self.add(kind, geometry::lerp(edge.ends, share), &on, None);
            }
        }
    }

    /// The points `fractions` of the way along `seams` (see [`fractions`]), with the seam they're on.
    fn along_seams(&mut self, seams: &[NearSeam], fractions: &[(f64, Kind)]) {
        for near in seams {
            let seam = &near.seam;
            for &(share, kind) in fractions {
                let point = geometry::lerp(seam.ends, share);
                self.add(kind, point, &seam.on, Some(seam.ends));
            }
        }
    }

    /// Where two of `edges` of different bricks cross.
    fn intersections(&mut self, edges: &[NearEdge]) {
        for (position, first) in edges.iter().enumerate() {
            for second in &edges[position + 1..] {
                if first.brick == second.brick {
                    continue;
                }
                if let Some((_, point)) = crossing(first.ends, second.ends) {
                    let on = [
                        (self.seen[first.brick].index, first.piece),
                        (self.seen[second.brick].index, second.piece),
                    ];
                    self.add(Kind::Intersection, point, &on, None);
                }
            }
        }
    }

    /// Where `edges` go through other bricks' faces.
    fn piercings(&mut self, edges: &[NearEdge]) {
        let faces: Vec<Vec<Face>> = self.seen.iter().map(|brick| brick.look.faces()).collect();
        for edge in edges {
            for (position, brick) in self.seen.iter().enumerate() {
                if position == edge.brick {
                    continue;
                }
                let frame = &brick.placed.frame;
                let ends = edge.ends.map(|end| frame.local(end));
                for (face, _, point) in piercings(brick.look, &faces[position], ends) {
                    let on = [
                        (self.seen[edge.brick].index, edge.piece),
                        (brick.index, face),
                    ];
                    self.add(Kind::Piercing, frame.point(point), &on, None);
                }
            }
        }
    }

    /// Halfway between the points where other bricks cross `edge` (at `shares` of its length, see
    /// [`crossings`]) and its ends, each next to the other.
    fn between(&mut self, edge: &NearEdge, shares: &[f64]) {
        if shares.is_empty() {
            // Halfway between its ends: its midpoint.
            return;
        }
        let on = [(self.seen[edge.brick].index, edge.piece)];
        let stations: Vec<f64> = core::iter::once(0.0)
            .chain(shares.iter().copied())
            .chain(core::iter::once(1.0))
            .collect();
        for pair in stations.windows(2) {
            let span = [pair[0], pair[1]].map(|share| geometry::lerp(edge.ends, share));
            let middle = geometry::lerp(edge.ends, (pair[0] + pair[1]) / 2.0);
            self.add(Kind::Between, middle, &on, Some(span));
        }
    }
}

/// How much nearer the mouse a point of `kind` on `on` at `point` counts for having been snapped to
/// last.
fn sticky(last: Option<&Snap>, kind: Kind, on: &[(usize, Piece)], point: Vec3) -> f32 {
    if last.is_some_and(|last| last.is(kind, on, point)) {
        STICKY_PIXELS
    } else {
        0.0
    }
}

/// What to snap to: the best of `points` that shows, else the nearest of `edges` and `seams` within
/// [`EDGE_PIXELS`] that shows, else the face under the mouse (with those kinds on).
fn choose(
    settings: &Snapping,
    seen: &[Seen],
    view: &View,
    edges: &[NearEdge],
    seams: &[NearSeam],
    points: Vec<Point>,
    last: Option<&Snap>,
) -> Option<Snap> {
    if let Some(point) = shown_best(points, seen, view) {
        return Some(point.into_snap());
    }
    // Along an edge or a seam: its point nearest the mouse.
    let along = |kind: Kind, on: Vec<(usize, Piece)>, point: Vec3, pixels: f32, span| Point {
        kind,
        point,
        score: pixels - sticky(last, kind, &on, point),
        on,
        span,
    };
    let mut lines = Vec::new();
    if settings.edges {
        lines.extend(
            edges
                .iter()
                .filter(|edge| edge.pixels <= EDGE_PIXELS)
                .map(|edge| {
                    let on = vec![(seen[edge.brick].index, edge.piece)];
                    along(Kind::Edge, on, edge.nearest, edge.pixels, None)
                }),
        );
    }
    lines.extend(
        seams
            .iter()
            .filter(|near| near.pixels <= EDGE_PIXELS)
            .map(|near| {
                let (on, ends) = (near.seam.on.to_vec(), near.seam.ends);
                along(Kind::Seam, on, near.nearest, near.pixels, Some(ends))
            }),
    );
    if let Some(point) = shown_best(lines, seen, view) {
        return Some(point.into_snap());
    }
    if !settings.faces {
        return None;
    }
    let (distance, brick, face, normal) = seen
        .iter()
        .filter_map(|brick| {
            let (distance, face, normal) =
                brick.look.hit(&brick.placed.frame.local_ray(&view.ray))?;
            Some((distance, brick, face, normal))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))?;
    Some(Snap {
        kind: Kind::Face,
        target: Target {
            point: geometry::along(&view.ray, distance),
            normal: Some(brick.placed.frame.direction(normal)),
        },
        on: vec![(brick.index, face)],
        span: None,
    })
}

/// The best of `points` (lowest score, then kind) that shows, or one that shows scoring at most
/// [`TIE_PIXELS`] worse but in front of it: of two points about as near the mouse (seen one behind
/// the other), the one in front.
fn shown_best(mut points: Vec<Point>, seen: &[Seen], view: &View) -> Option<Point> {
    points.sort_by(|a, b| a.score.total_cmp(&b.score).then(a.kind.cmp(&b.kind)));
    // The best so far, its distance from the camera, and the worst score that can still beat it.
    let mut best: Option<(Point, f64)> = None;
    let mut limit = f32::INFINITY;
    for point in points {
        if point.score > limit {
            break;
        }
        if !visible(seen, view, point.point) {
            continue;
        }
        let distance = geometry::length(geometry::sub(point.point, view.ray.origin));
        match &best {
            None => {
                limit = point.score + TIE_PIXELS;
                best = Some((point, distance));
            }
            Some((_, nearest)) if distance < nearest - SAME_DEPTH => best = Some((point, distance)),
            Some(_) => {}
        }
    }
    best.map(|(point, _)| point)
}

/// The shares of an edge's length snapped to along it, and what they are.
fn fractions(settings: &Snapping) -> Vec<(f64, Kind)> {
    let mut fractions = Vec::new();
    if settings.midpoints {
        fractions.push((0.5, Kind::Midpoint));
    }
    if settings.thirds {
        fractions.extend([(1.0 / 3.0, Kind::Fraction), (2.0 / 3.0, Kind::Fraction)]);
    }
    if settings.quarters {
        fractions.extend([(0.25, Kind::Fraction), (0.75, Kind::Fraction)]);
    }
    fractions
}

/// How far `point` (in the vehicle) is from the mouse on the screen (pixels), if it's in front of
/// the camera.
fn from_mouse(view: &View, point: Vec3) -> Option<f32> {
    let ray = &view.ray;
    let offset = geometry::sub(point, ray.origin);
    let along = geometry::dot(offset, ray.direction);
    if along <= 0.0 {
        return None;
    }
    let across = geometry::length(geometry::sub(offset, geometry::scale(ray.direction, along)));
    Some((across / view.pixels.at(point)) as f32)
}

/// Whether `point` (in the vehicle) shows: no brick seen is in front of it.
fn visible(seen: &[Seen], view: &View, point: Vec3) -> bool {
    let camera = view.ray.origin;
    let offset = geometry::sub(point, camera);
    let Some(direction) = geometry::normalized(offset) else {
        return false;
    };
    let ray = Ray {
        origin: camera,
        direction,
    };
    let reach = geometry::length(offset) - HIDDEN_PIXELS * view.pixels.at(point);
    seen.iter().all(|brick| {
        let local = brick.placed.frame.local_ray(&ray);
        brick
            .look
            .hit(&local)
            .is_none_or(|(distance, ..)| distance >= reach)
    })
}

/// Where the segments `first` and `second` cross (pass within [`CROSSING_TOLERANCE`] of each
/// other), if they do at one point (not along each other): the share of `first`'s length there,
/// and the point.
fn crossing(first: [Vec3; 2], second: [Vec3; 2]) -> Option<(f64, Vec3)> {
    // The closest points of two segments (Ericson, Real-Time Collision Detection, 5.1.9).
    let along_first = geometry::sub(first[1], first[0]);
    let along_second = geometry::sub(second[1], second[0]);
    let between = geometry::sub(first[0], second[0]);
    let a = geometry::dot(along_first, along_first);
    let b = geometry::dot(along_first, along_second);
    let e = geometry::dot(along_second, along_second);
    let denominator = a * e - b * b;
    if a <= 0.0 || e <= 0.0 || denominator <= PARALLEL * a * e {
        return None;
    }
    let c = geometry::dot(along_first, between);
    let f = geometry::dot(along_second, between);
    let mut share = ((b * f - c * e) / denominator).clamp(0.0, 1.0);
    let mut other = (b * share + f) / e;
    if other < 0.0 {
        other = 0.0;
        share = (-c / a).clamp(0.0, 1.0);
    } else if other > 1.0 {
        other = 1.0;
        share = ((b - c) / a).clamp(0.0, 1.0);
    }
    let on_first = geometry::lerp(first, share);
    let on_second = geometry::lerp(second, other);
    (geometry::length(geometry::sub(on_first, on_second)) <= CROSSING_TOLERANCE).then(|| {
        let point = geometry::scale(geometry::add(on_first, on_second), 0.5);
        (share, point)
    })
}

/// Where the segment `ends` (in a brick, seen as `look`, with faces `faces`) goes through the
/// brick's faces: each face, the share of the segment's length there, and the point (in the brick).
/// An end on a face's plane isn't going through it, nor a segment in it.
fn piercings(look: Look, faces: &[Face], ends: [Vec3; 2]) -> Vec<(Piece, f64, Vec3)> {
    faces
        .iter()
        .filter_map(|face| {
            let [start, end] =
                ends.map(|end| geometry::dot(face.normal, geometry::sub(end, face.center)));
            let across = (start > CROSSING_TOLERANCE && end < -CROSSING_TOLERANCE)
                || (start < -CROSSING_TOLERANCE && end > CROSSING_TOLERANCE);
            if !across {
                return None;
            }
            let share = start / (start - end);
            let point = geometry::lerp(ends, share);
            look.on_face(face.piece, point)
                .then_some((face.piece, share, point))
        })
        .collect()
}

/// Where other bricks' edges and faces (with those kinds of crossings on) cross the edge `ends` (in
/// the vehicle) of brick `index`: as shares of its length from its first end, in order, without its
/// ends. Bricks `skip` says are left out.
fn crossings(
    settings: &Snapping,
    scene: &Scene,
    models: bool,
    skip: impl Fn(&Placed) -> bool,
    index: usize,
    ends: [Vec3; 2],
) -> Vec<f64> {
    let length = geometry::length(geometry::sub(ends[1], ends[0]));
    if length <= 0.0 {
        return Vec::new();
    }
    let mut shares = Vec::new();
    for (other, placed) in scene.bricks().iter().enumerate() {
        if other == index || skip(placed) {
            continue;
        }
        let ends = ends.map(|end| placed.frame.local(end));
        if !geometry::segment_box(ends, &placed.bounds.grown(CROSSING_TOLERANCE)) {
            continue;
        }
        let look = Look::of(placed, models);
        if settings.edge_intersections {
            for (_, edge) in look.edges() {
                if let Some((share, _)) = crossing(ends, edge) {
                    shares.push(share);
                }
            }
        }
        if settings.face_intersections {
            for (_, share, _) in piercings(look, &look.faces(), ends) {
                shares.push(share);
            }
        }
    }
    // Crossings as close as the tolerance are one, and at an end are that end.
    let close = CROSSING_TOLERANCE / length;
    shares.retain(|&share| share > close && share < 1.0 - close);
    shares.sort_by(f64::total_cmp);
    shares.dedup_by(|later, earlier| *later - *earlier <= close);
    shares
}
