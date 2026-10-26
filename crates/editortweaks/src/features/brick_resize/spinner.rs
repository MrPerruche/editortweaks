//! Spinners in fast editing (`docs/spinner-editing.md`): a spinner shows its real shape (its
//! cross-section swept along its arc, `model_io::spinner`), not its box, and dragging it changes
//! its own values instead of a size:
//!
//! - its sides (see [`Part`]: the inner or outer surface, the top or bottom face, each whole along
//!   the arc, and the rims between them) change the thickness and height like a box's faces and
//!   edges change its size: the dragged side follows the mouse, the opposite one stays. A curved
//!   stretch of the cross-section is split into sides where it turns from facing up or down to
//!   facing outward or inward, by separators (shown dashed) that drag like the rims at corners;
//! - its end cap (its face, edges and corners) changes the angle: the cap goes round the hub after
//!   the mouse, through zero to the other side, up to 360 degrees either way. The start cap does
//!   the same keeping the end in place (the brick turns about the hub), on round spinners only.
//!
//! With the snap key held, the dragged side, rim or cap goes through the point snapped to.

use std::collections::HashMap;
use std::sync::Arc;

pub use model_io::spinner::Edit;
use model_io::spinner::{self as shape, Outline, Place, Spinner};

use super::draw::{self, Drawing, PixelSize};
use super::geometry::{self, Frame, Ray, Vec3};
use super::model::{BrickModel, Element, Model};
use super::parts::{EDGE_PIXELS, PART_SHARE};
use super::resize::Target;
use super::scene::View;
use crate::game::brick_model::Triangles;
use crate::game::spinner::{Property, Values};
use crate::game::spinner_profiles::Profiles;
use crate::math;

/// How far an element may be from a cap's angle to be on it (radians).
const CAP_TOLERANCE: f64 = 1e-4;
/// How far off a model's points may be (as a share of the spinner's radius): they're `f32`.
const F32_NOISE: f64 = 1e-6;
/// How far from a corner of the cross-section (as a share of its half size) an edge or vertex may
/// be to be its rim, and how far off the middle a corner must be to drag that way.
const SIDE_TOLERANCE: f64 = 1e-3;
/// How much of a side's normal must lie along an axis to drag that way (a round side's ends may
/// not be quite even).
const NORMAL_TOLERANCE: f64 = 0.02;
/// How much closer to edge-on than this (the cosine between the mouse ray and the plane) a rim
/// stops following the mouse.
const MIN_PLANE_COSINE: f64 = 0.08;
/// How many points along a turn the mouse is looked for on a cap's circle, before refining.
const CIRCLE_SAMPLES: usize = 360;
/// How many segments a whole turn of a drawn arc has (at least 8 per arc).
const ARC_SEGMENTS: f64 = 96.0;
/// How far a corner of the cross-section turns (degrees) for its rim to be drawn.
const RIM_TURN: f64 = 20.0;

/// The separator under the mouse may be this many times [`EDGE_PIXELS`] from the point the ray
/// hits (pixels at its distance) and still be the one on that surface, not one behind it.
const SEPARATOR_DEPTH: f64 = 3.0;

/// A cross-section's own data, shared by the spinners of its shape.
pub struct Look {
    /// Its outline as the game sweeps it, from -1 to 1 (see `model_io::spinner::profile`).
    profile: Vec<[f64; 2]>,
    /// Split into sides (at corners turning at least [`RIM_TURN`], and separators): `Part::Rim`'s
    /// and `Part::Side`'s indices are its.
    outline: Outline,
    /// The average of its points: inside it (every spinner shape is convex).
    inside: [f64; 2],
    /// The side each segment of the outline (from the point of the same index) is on.
    segment_sides: Vec<usize>,
}

impl Look {
    fn new(profile: Vec<[f64; 2]>) -> Self {
        let count = profile.len();
        let inside =
            [0, 1].map(|axis| profile.iter().map(|point| point[axis]).sum::<f64>() / count as f64);
        let outline = Outline::new(&profile, RIM_TURN);
        let points = outline.points.len();
        let mut segment_sides = vec![0; points];
        for (index, side) in outline.sides.iter().enumerate() {
            for segment in 0..side.segments {
                segment_sides[(side.first + segment) % points] = index;
            }
        }
        Self {
            profile,
            outline,
            inside,
            segment_sides,
        }
    }

    /// The points of `side`, in the outline's order.
    fn side_points(&self, side: usize) -> impl Iterator<Item = [f64; 2]> + '_ {
        let shape::Side { first, segments } = self.outline.sides[side];
        let points = &self.outline.points;
        (0..=segments).map(move |index| points[(first + index) % points.len()])
    }

    /// The side `point` (from -1 to 1) is nearest.
    fn side_at(&self, point: [f64; 2]) -> usize {
        self.segment_sides[shape::nearest_segment(&self.outline.points, point)]
    }

    /// The sides before and after the outline's point `index` (a corner or separator).
    fn sides_by(&self, index: usize) -> [usize; 2] {
        let count = self.outline.points.len();
        [(index + count - 1) % count, index].map(|segment| self.segment_sides[segment])
    }
}

/// A spinner in the scene: its values, its cross-section, and the faces of its model that are its
/// caps.
#[derive(Clone)]
pub struct SpinnerBrick {
    /// As the brick stores them.
    pub stored: Values,
    pub spinner: Spinner,
    look: Arc<Look>,
    /// The model's start and end cap faces (none for a full turn).
    caps: [Option<u32>; 2],
}

impl SpinnerBrick {
    /// The points snapping knows it by beside its model's: its hub, and its caps' middles (where
    /// its connectors are), with their faces.
    pub fn snap_points(&self) -> Vec<(Option<Element>, Vec3)> {
        let mut points = vec![(None, [0.0; 3])];
        let ends = [0.0, self.spinner.radians()];
        for (cap, t) in self.caps.iter().zip(ends) {
            if let Some(face) = cap {
                points.push((Some(Element::Face(*face)), self.spinner.point(t, 0.0, 0.0)));
            }
        }
        points
    }
}

/// Spinners' cross-sections (by shape) and models (by values) made so far, kept across scenes.
#[derive(Default)]
pub struct SpinnerModels {
    looks: HashMap<u8, Arc<Look>>,
    models: HashMap<(u8, [u32; 5]), Option<Arc<Model>>>,
}

impl SpinnerModels {
    /// The spinner with `values`, and its model: its cross-section from `profiles` the first time
    /// its shape is seen. `None` without a cross-section, or if the model can't be made.
    pub fn brick(
        &mut self,
        values: Values,
        profiles: impl FnOnce() -> Option<Profiles>,
    ) -> Option<(SpinnerBrick, BrickModel)> {
        // Read again next time if it couldn't be.
        let look = match self.looks.get(&values.shape) {
            Some(look) => look.clone(),
            None => {
                let profiles = profiles()?;
                let profile = shape::profile(&profiles.visible, profiles.quarter_turns)?;
                let look = Arc::new(Look::new(profile));
                self.looks.insert(values.shape, look.clone());
                look
            }
        };
        let spinner = spinner_of(&values);
        let key = (
            values.shape,
            [
                values.size[0].to_bits(),
                values.size[1].to_bits(),
                values.radius[0].to_bits(),
                values.radius[1].to_bits(),
                values.angle.to_bits(),
            ],
        );
        let model = self
            .models
            .entry(key)
            .or_insert_with(|| {
                let (vertices, triangles) = spinner.sweep(&look.profile);
                Model::new(&Triangles {
                    vertices,
                    triangles,
                    ..Triangles::default()
                })
                .map(Arc::new)
            })
            .clone()?;
        let model = BrickModel::unscaled(model);
        let caps = if spinner.is_full_turn() {
            [None, None]
        } else {
            [0.0, spinner.radians()].map(|t| cap_face(&spinner, &model, t))
        };
        let brick = SpinnerBrick {
            stored: values,
            spinner,
            look,
            caps,
        };
        Some((brick, model))
    }
}

/// The spinner of stored values.
pub fn spinner_of(values: &Values) -> Spinner {
    Spinner {
        radius: values.radius.map(f64::from),
        size: values.size.map(f64::from),
        angle: f64::from(values.angle),
    }
}

/// The values to store for `edit` of a spinner stored as `stored`.
pub fn values_of(stored: &Values, edit: &Edit) -> Values {
    Values {
        shape: stored.shape,
        size: edit.spinner.size.map(|value| value as f32),
        radius: edit.spinner.radius.map(|value| value as f32),
        angle: edit.spinner.angle as f32,
    }
}

/// The face of `model` all at angle `t` along the arc: a cap.
fn cap_face(spinner: &Spinner, model: &BrickModel, t: f64) -> Option<u32> {
    model.faces().find(|&face| {
        let points: Vec<Vec3> = model.face_edges(face).flatten().collect();
        all_at_angle(spinner, &points, t)
    })
}

/// Whether `points` are all at angle `t` along the arc, some of them off the hub.
fn all_at_angle(spinner: &Spinner, points: &[Vec3], t: f64) -> bool {
    points.iter().any(|&point| spinner.place(point).is_some())
        && points.iter().all(|&point| at_angle(spinner, point, t))
}

/// Whether `point` is at angle `t` along the arc (or at the hub).
fn at_angle(spinner: &Spinner, point: Vec3, t: f64) -> bool {
    spinner.place(point).is_none_or(|place| {
        (spinner.along_arc(place.angle) - t).abs() <= CAP_TOLERANCE
            || (place.angle - t).abs() <= CAP_TOLERANCE
    })
}

/// A piece of a spinner as fast editing picks it, like a box's faces and edges: a whole side of
/// its cross-section along the arc, curved or not (see [`Outline`]), a rim (along a corner or a
/// separator of the cross-section), or a cap. The facets of its model are never picked alone.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Part {
    /// The side of that index in its [`Look`]'s outline.
    Side(usize),
    /// The rim through the outline's point of that index.
    Rim(usize),
    Cap {
        end: bool,
    },
}

impl Part {
    /// The piece of `brick` (with `model`) that `element` of its model is on, pointed at `hit` (in
    /// the brick): its cap if the element is all on one, its rim if it's an edge or vertex along a
    /// corner, else the side under `hit`.
    pub fn of(brick: &SpinnerBrick, model: &BrickModel, element: Element, hit: Vec3) -> Self {
        let spinner = &brick.spinner;
        let look = &brick.look;
        let points: Vec<Vec3> = match element {
            Element::Vertex(vertex) => vec![model.point(vertex)],
            Element::Edge(edge) => model.edge(edge).to_vec(),
            Element::Face(face) => model.face_edges(face).flatten().collect(),
        };
        for (cap, end) in brick.caps.iter().zip([false, true]) {
            let t = if end { spinner.radians() } else { 0.0 };
            let on_cap = match (cap, element) {
                (Some(cap), Element::Face(face)) => *cap == face,
                (Some(_), _) => all_at_angle(spinner, &points, t),
                (None, _) => false,
            };
            if on_cap {
                return Part::Cap { end };
            }
        }
        let half = [spinner.thickness() / 2.0, spinner.height() / 2.0];
        // Where a point is in the cross-section, from -1 to 1. A point at the hub is on the inner
        // side (a ring as thick as it can be).
        let section = |point: Vec3| -> [f64; 2] {
            match spinner.place(point) {
                Some(place) => [place.across / half[0], place.up / half[1]],
                None => [-1.0, point[2] / half[1]],
            }
        };
        if !matches!(element, Element::Face(_)) {
            // The model's points are `f32`: off by more on a large ring.
            let noise = F32_NOISE * spinner.radius[0].abs().max(spinner.radius[1].abs());
            let tolerance = SIDE_TOLERANCE + noise / half[0].min(half[1]);
            let places: Vec<[f64; 2]> = points.iter().map(|&point| section(point)).collect();
            let corner = look.outline.corners.iter().copied().find(|&corner| {
                let at = look.outline.points[corner];
                places.iter().all(|&place| close2(place, at, tolerance))
            });
            if let Some(corner) = corner {
                return Part::Rim(corner);
            }
        }
        Part::Side(look.side_at(section(hit)))
    }

    /// How dragging it changes `brick`, if it does.
    pub fn grip(&self, brick: &SpinnerBrick) -> Option<Grip> {
        let spinner = &brick.spinner;
        let look = &brick.look;
        let half = [spinner.thickness() / 2.0, spinner.height() / 2.0];
        let in_cm = |point: [f64; 2]| [point[0] * half[0], point[1] * half[1]];
        let grip = match *self {
            // Only a round spinner's start can turn about the hub with its end in place.
            Part::Cap { end } => return (end || spinner.is_round()).then_some(Grip::Cap { end }),
            Part::Rim(corner) => {
                let at = look.outline.points[corner];
                let sides = if look.outline.separators.contains(&corner) {
                    // Between two sides each facing along an axis: both of theirs.
                    let mut sides = [0; 2];
                    for side in look.sides_by(corner) {
                        let way = look.outline.facing(look.outline.sides[side])?;
                        for axis in 0..2 {
                            if way[axis].abs() > 0.5 {
                                sides[axis] = way[axis].signum() as i8;
                            }
                        }
                    }
                    sides
                } else {
                    at.map(|value| {
                        if value.abs() <= SIDE_TOLERANCE {
                            0
                        } else {
                            value.signum() as i8
                        }
                    })
                };
                Grip::Section {
                    sides,
                    shape: Section::Point {
                        corner,
                        point: in_cm(at),
                    },
                }
            }
            Part::Side(side) => {
                // The way it faces as a whole (see `Outline::facing`); along an axis if it nearly
                // is.
                let points: Vec<[f64; 2]> = look.side_points(side).collect();
                let normal = look.outline.facing(look.outline.sides[side])?;
                let normal = normal.map(|value| {
                    if value.abs() <= NORMAL_TOLERANCE {
                        0.0
                    } else {
                        value
                    }
                });
                // In cm, the cross-section stretched: normals stretch the other way.
                let normal = normalized2([normal[0] / half[0], normal[1] / half[1]])?;
                // Its furthest point that way: what a snap point puts it against.
                let point = points
                    .iter()
                    .map(|&point| in_cm(point))
                    .max_by(|a, b| dot2(normal, *a).total_cmp(&dot2(normal, *b)))?;
                Grip::Section {
                    sides: sides_along(normal),
                    shape: Section::Plane {
                        side,
                        point,
                        normal,
                    },
                }
            }
        };
        match grip {
            Grip::Section { sides: [0, 0], .. } => None,
            grip => Some(grip),
        }
    }

    /// The separator of `brick` (at `frame`) the mouse is on, if any, as a rim: within
    /// [`EDGE_PIXELS`] of it like a box's edge (less between sides that look narrow), near `hit`
    /// (where the mouse's ray meets the spinner, in the vehicle). Separators aren't edges of the
    /// model, so they're found on the screen.
    pub fn separator_under(
        brick: &SpinnerBrick,
        frame: &Frame,
        view: &View,
        hit: Vec3,
    ) -> Option<Self> {
        let spinner = &brick.spinner;
        let look = &brick.look;
        let pixels = &view.pixels;
        let place = spinner.place(frame.local(hit))?;
        let t = spinner.along_arc(place.angle);
        let arc = spinner.radians();
        if (arc >= 0.0 && !(0.0..=arc).contains(&t)) || (arc < 0.0 && !(arc..=0.0).contains(&t)) {
            return None;
        }
        let at = |point: [f64; 2]| frame.point(spinner.section_point(t, point));
        let near = pixels.at(hit);
        look.outline
            .separators
            .iter()
            .filter_map(|&separator| {
                let point = at(look.outline.points[separator]);
                // How wide the sides either side of it look there.
                let width = look
                    .sides_by(separator)
                    .map(|side| {
                        let points: Vec<[f64; 2]> = look.side_points(side).collect();
                        let [start, end] = [points[0], points[points.len() - 1]].map(at);
                        screen_length(&view.ray, pixels, start, geometry::sub(end, start))
                    })
                    .into_iter()
                    .fold(f32::INFINITY, f32::min);
                let reach = EDGE_PIXELS.min(PART_SHARE * width);
                let across = ray_distance(&view.ray, point) / pixels.at(point);
                let depth = geometry::length(geometry::sub(point, hit)) / near;
                let shows = depth <= SEPARATOR_DEPTH * f64::from(EDGE_PIXELS);
                (shows && across <= f64::from(reach)).then_some((across, separator))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, separator)| Part::Rim(separator))
    }
}

/// How dragging a part of a spinner changes it.
#[derive(Clone, Copy, Debug)]
pub enum Grip {
    /// Its sides across the cross-section: per value (thickness, height), -1 the inner side or the
    /// bottom, 1 the outer side or the top, 0 not dragged. The opposite sides stay.
    Section { sides: [i8; 2], shape: Section },
    /// A cap: the angle (the end's, or the start's turning the brick).
    Cap { end: bool },
}

/// What's grabbed across the cross-section, in it (across the ring and up, cm).
#[derive(Clone, Copy, Debug)]
pub enum Section {
    /// A rim (through the outline's point `corner`, a corner or a separator, at `point`): moves
    /// freely in the cross-section's plane (along the dragged axis only, for one).
    Point { corner: usize, point: [f64; 2] },
    /// A whole side (see [`Part::Side`]; its furthest point out, the way it faces): moves along
    /// that way, keeping its shape.
    Plane {
        side: usize,
        point: [f64; 2],
        normal: [f64; 2],
    },
}

impl Grip {
    /// The piece it drags.
    pub fn part(&self) -> Part {
        match *self {
            Grip::Section {
                shape: Section::Point { corner, .. },
                ..
            } => Part::Rim(corner),
            Grip::Section {
                shape: Section::Plane { side, .. },
                ..
            } => Part::Side(side),
            Grip::Cap { end } => Part::Cap { end },
        }
    }

    /// The values a drag of it changes.
    pub fn changes(&self) -> &'static [Property] {
        match self {
            Grip::Section { sides, .. } if sides[0] != 0 => &[Property::Size, Property::Radius],
            Grip::Section { .. } => &[Property::Size],
            Grip::Cap { .. } => &[Property::Angle],
        }
    }

    /// What it is, for the log.
    pub fn describe(&self) -> &'static str {
        match self {
            Grip::Section { sides: [0, _], .. } => "its top or bottom (height)",
            Grip::Section { sides: [_, 0], .. } => "its inner or outer side (thickness)",
            Grip::Section { .. } => "a rim (thickness and height)",
            Grip::Cap { end: true } => "its end cap (angle)",
            Grip::Cap { end: false } => "its start cap (angle, turning it)",
        }
    }
}

/// A spinner being dragged, as it was grabbed.
pub struct Dragging {
    pub grip: Grip,
    /// Its axes in the vehicle.
    pub frame: Frame,
    pub spinner: Spinner,
    /// Where it was grabbed, in its frame, and where that is in it.
    grab: Vec3,
    place: Place,
    /// The angle the mouse asked for last (degrees, not stepped): the end's, or how far the start
    /// turned. Angles go on from it.
    turned: f64,
    /// How far along the arc the cap is from the grabbed point (degrees): a cap grabbed by an edge
    /// a little off it doesn't jump to the mouse.
    behind: f64,
}

/// How much further from the mouse (pixels) a point of a cap's circle may be than the nearest and
/// still win by being nearer where the cap was: seen edge-on, the front and back of the circle
/// are as near the mouse.
const CIRCLE_TIE_PIXELS: f64 = 2.0;

impl Dragging {
    /// Starts dragging `grip` of `spinner` (at `frame`), grabbed at `grab` (in the vehicle).
    pub fn new(grip: Grip, frame: Frame, spinner: Spinner, grab: Vec3) -> Option<Self> {
        let grab = frame.local(grab);
        let place = spinner.place(grab)?;
        let at = spinner.along_arc(place.angle).to_degrees();
        let (turned, behind) = match grip {
            Grip::Cap { end: true } => (spinner.angle, spinner.angle - at),
            Grip::Cap { end: false } => (0.0, -at),
            _ => (0.0, 0.0),
        };
        Some(Self {
            grip,
            frame,
            spinner,
            grab,
            place,
            turned,
            behind,
        })
    }

    /// Where it was grabbed, in the vehicle (as grabbed).
    pub fn grabbed(&self) -> Vec3 {
        self.frame.point(self.grab)
    }

    /// No change: the spinner as grabbed.
    pub fn unchanged(&self) -> Edit {
        Edit {
            spinner: self.spinner,
            shift: [0.0; 3],
            turn: 0.0,
        }
    }

    /// The change following the mouse `view`, `None` while it gives no steady answer.
    pub fn follow(&mut self, view: &View) -> Option<Edit> {
        match self.grip {
            Grip::Section { sides, shape } => {
                let size = self.section_follow(sides, shape, &view.ray)?;
                Some(self.spinner.resized(sides, self.limited(sides, size)))
            }
            Grip::Cap { end } => {
                let t = self.on_circle(view)?;
                Some(self.angle_to(end, t.to_degrees() + self.behind))
            }
        }
    }

    /// The change taking what's grabbed to `target` (see the module's description).
    pub fn snap(&mut self, target: &Target) -> Option<Edit> {
        let point = self.frame.local(target.point);
        match self.grip {
            Grip::Section { sides, shape } => {
                let place = self.spinner.place(point)?;
                let size = self.section_to(sides, shape, [place.across, place.up])?;
                Some(self.spinner.resized(sides, self.limited(sides, size)))
            }
            Grip::Cap { end } => {
                let place = self.spinner.place(point)?;
                Some(self.angle_to(end, place.angle.to_degrees()))
            }
        }
    }

    /// The new angle for the mouse at `degrees` round the hub, going on from the last one: the
    /// end's, or the start's.
    fn angle_to(&mut self, end: bool, degrees: f64) -> Edit {
        let turned = shape::unwrapped(self.turned, degrees);
        if end {
            self.turned = turned.clamp(-shape::MAX_ANGLE, shape::MAX_ANGLE);
            self.spinner.with_end(shape::kept_angle(self.turned))
        } else {
            let from = self.spinner.angle;
            self.turned = turned.clamp(from - shape::MAX_ANGLE, from + shape::MAX_ANGLE);
            let angle = shape::kept_angle(from - self.turned);
            self.spinner.with_start(from - angle)
        }
    }

    /// The angle (radians) of the point of the grabbed point's circle about the hub nearest the
    /// mouse on the screen.
    fn on_circle(&self, view: &View) -> Option<f64> {
        let at = |t: f64| {
            let point = self
                .frame
                .point(self.spinner.point(t, self.place.across, self.place.up));
            let ahead = geometry::dot(geometry::sub(point, view.ray.origin), view.ray.direction);
            (ahead > 0.0).then(|| ray_distance(&view.ray, point) / view.pixels.at(point))
        };
        use core::f64::consts::{PI, TAU};
        let step = TAU / CIRCLE_SAMPLES as f64;
        let samples: Vec<(f64, f64)> = (0..CIRCLE_SAMPLES)
            .map(|sample| {
                let t = -PI + step * sample as f64;
                (t, at(t).unwrap_or(f64::INFINITY))
            })
            .collect();
        let nearest = samples
            .iter()
            .map(|&(_, pixels)| pixels)
            .fold(f64::INFINITY, f64::min);
        if !nearest.is_finite() {
            return None;
        }
        // Of the circle's points about as near the mouse as the nearest (each the nearest of its
        // stretch), the one nearest where the grabbed point was: it doesn't flip to the back.
        let was = (self.turned - self.behind).to_radians();
        let apart = |t: f64| ((t - was + PI).rem_euclid(TAU) - PI).abs();
        let count = samples.len();
        let best = (0..count)
            .filter(|&index| {
                let pixels = samples[index].1;
                pixels <= nearest + CIRCLE_TIE_PIXELS
                    && pixels <= samples[(index + count - 1) % count].1
                    && pixels <= samples[(index + 1) % count].1
            })
            .map(|index| samples[index].0)
            .min_by(|a, b| apart(*a).total_cmp(&apart(*b)))?;
        // Then down to a fraction of a pixel, between the samples next to it.
        let (mut low, mut high) = (best - step, best + step);
        for _ in 0..40 {
            let (first, second) = (low + (high - low) / 3.0, high - (high - low) / 3.0);
            let far = |t: f64| at(t).unwrap_or(f64::INFINITY);
            if far(first) < far(second) {
                high = second;
            } else {
                low = first;
            }
        }
        Some((low + high) / 2.0)
    }

    /// The thickness and height (effective, cm) putting the dragged sides where the mouse `ray`
    /// takes them, like a box's (`super::resize::Resize::follow`), in the cross-section where it
    /// was grabbed.
    fn section_follow(&self, sides: [i8; 2], section: Section, ray: &Ray) -> Option<[f64; 2]> {
        let grab = self.frame.point(self.grab);
        let directions = [Spinner::outward(self.place.angle), [0.0, 0.0, 1.0]];
        let anchor = self.anchor(sides);
        let dragged: Vec<usize> = (0..2).filter(|&axis| sides[axis] != 0).collect();
        match (section, dragged.as_slice()) {
            (Section::Plane { normal, .. }, _) => {
                // From where it was grabbed: that point follows the mouse.
                let grabbed = [self.place.across, self.place.up];
                let offset = dot2(normal, sub2(grabbed, anchor));
                if offset.abs() < 1e-3 {
                    return None;
                }
                let direction = geometry::add(
                    geometry::scale(directions[0], normal[0]),
                    geometry::scale(directions[1], normal[1]),
                );
                let moved = geometry::closest_on_line(ray, grab, self.frame.direction(direction))?;
                Some(self.scaled(sides, [(offset + moved) / offset; 2]))
            }
            (Section::Point { point: handle, .. }, &[axis]) => {
                let direction = self.frame.direction(directions[axis]);
                let moved = geometry::closest_on_line(ray, grab, direction)?;
                let from = handle[axis] - anchor[axis];
                let mut ratios = [1.0; 2];
                if from.abs() >= 1e-3 {
                    ratios[axis] = (from + moved) / from;
                }
                Some(self.scaled(sides, ratios))
            }
            (Section::Point { .. }, _) => {
                // Free in the cross-section's plane: across the ring and up.
                let normal = self
                    .frame
                    .direction(geometry::cross(directions[0], directions[1]));
                let distance = geometry::ray_plane(ray, grab, normal, MIN_PLANE_COSINE)?;
                let offset =
                    geometry::sub(self.frame.local(geometry::along(ray, distance)), self.grab);
                let moved = [
                    self.place.across + geometry::dot(offset, directions[0]),
                    self.place.up + offset[2],
                ];
                self.section_to(sides, section, moved)
            }
        }
    }

    /// The thickness and height putting the dragged sides through `point` (across the ring and up,
    /// cm): a side's furthest point out to it, keeping its shape, else each dragged axis to it.
    fn section_to(&self, sides: [i8; 2], section: Section, point: [f64; 2]) -> Option<[f64; 2]> {
        let anchor = self.anchor(sides);
        match section {
            Section::Plane {
                point: on, normal, ..
            } => {
                let offset = dot2(normal, sub2(on, anchor));
                if offset.abs() < 1e-3 {
                    return None;
                }
                let reach = dot2(normal, sub2(point, anchor));
                Some(self.scaled(sides, [reach / offset; 2]))
            }
            Section::Point { point: handle, .. } => {
                let ratios = [0, 1].map(|axis| {
                    let from = handle[axis] - anchor[axis];
                    if from.abs() >= 1e-3 {
                        (point[axis] - anchor[axis]) / from
                    } else {
                        1.0
                    }
                });
                Some(self.scaled(sides, ratios))
            }
        }
    }

    /// The cross-section's sides that stay (across the ring and up, cm): opposite the dragged
    /// ones, the middle along a value not dragged.
    fn anchor(&self, sides: [i8; 2]) -> [f64; 2] {
        let half = [self.spinner.thickness() / 2.0, self.spinner.height() / 2.0];
        [0, 1].map(|axis| -f64::from(sides[axis]) * half[axis])
    }

    /// The thickness and height scaled by `ratios` along the dragged values.
    fn scaled(&self, sides: [i8; 2], ratios: [f64; 2]) -> [f64; 2] {
        let now = [self.spinner.thickness(), self.spinner.height()];
        [0, 1].map(|axis| {
            if sides[axis] != 0 {
                now[axis] * ratios[axis]
            } else {
                now[axis]
            }
        })
    }

    /// `size` (thickness, height) kept within what the spinner can show: at least
    /// [`shape::MIN_LENGTH`], at most [`shape::MAX_SIZE`], the inner side no further in than the
    /// hub, and the radii (moving by half the thickness's change) at least [`shape::MIN_LENGTH`].
    fn limited(&self, sides: [i8; 2], size: [f64; 2]) -> [f64; 2] {
        let mut least = [shape::MIN_LENGTH; 2];
        let mut most = [shape::MAX_SIZE; 2];
        let radius = self.spinner.radius[0].min(self.spinner.radius[1]);
        let thickness = self.spinner.thickness();
        // How much the thickness may change before the smaller radius gets too small.
        let spare = 2.0 * (radius - shape::MIN_LENGTH).max(0.0);
        if sides[0] < 0 {
            // The outer side stays: the inner one reaches the hub at the smaller outer semi-axis.
            most[0] = most[0].min(radius + thickness / 2.0).min(thickness + spare);
        } else if sides[0] > 0 {
            // The inner side stays: the radii shrink as the thickness does.
            least[0] = least[0].max(thickness - spare);
        }
        [0, 1].map(|axis| size[axis].clamp(least[axis], most[axis].max(least[axis])))
    }

    /// What's dragged, with the spinner as `edit` made it: the side, rim or cap, in the brick as
    /// grabbed (see [`Self::frame`]).
    pub fn draw(&self, drawing: &mut Drawing, edit: &Edit, brick: &SpinnerBrick, pen: draw::Pen) {
        let frame = moved_frame(&self.frame, edit);
        draw_part(drawing, &frame, &edit.spinner, brick, self.grip.part(), pen);
    }
}

/// The brick's frame once `edit` moved and turned it, from `frame` (as grabbed).
pub fn moved_frame(frame: &Frame, edit: &Edit) -> Frame {
    let origin = frame.point(edit.shift);
    let rotation = frame.rotation * math::Quat::from_axis_angle([0.0, 0.0, 1.0], edit.turn);
    Frame::new(origin, rotation)
}

/// A spinner's outline: its caps, its rims along corners, its separators (dashed); while
/// `editing`, its hub.
pub fn draw_outline(
    drawing: &mut Drawing,
    frame: &Frame,
    spinner: &Spinner,
    brick: &SpinnerBrick,
    pen: draw::Pen,
    editing: bool,
) {
    let look = &brick.look;
    if !spinner.is_full_turn() {
        for t in [0.0, spinner.radians()] {
            draw_section(drawing, frame, spinner, look, t, pen);
        }
    }
    let outline = &look.outline;
    for &corner in &outline.corners {
        draw_arc(drawing, frame, spinner, outline.points[corner], pen, false);
    }
    for &separator in &outline.separators {
        draw_arc(
            drawing,
            frame,
            spinner,
            outline.points[separator],
            pen,
            true,
        );
    }
    if editing {
        drawing.mark(
            draw::Mark::Cross,
            frame.point([0.0; 3]),
            draw::END_MARK_PIXELS,
            draw::HUB,
        );
    }
}

/// `part` of `brick`, as `spinner` (its values now) shows it: a side's outline (its edges along
/// the arc, and across it at the caps), a rim, or a cap's cross-section.
pub fn draw_part(
    drawing: &mut Drawing,
    frame: &Frame,
    spinner: &Spinner,
    brick: &SpinnerBrick,
    part: Part,
    pen: draw::Pen,
) {
    let look = &brick.look;
    match part {
        Part::Side(side) => {
            let points: Vec<[f64; 2]> = look.side_points(side).collect();
            if !spinner.is_full_turn() {
                for t in [0.0, spinner.radians()] {
                    let across: Vec<Vec3> = points
                        .iter()
                        .map(|&point| outside(drawing, frame, spinner, look, t, point))
                        .collect();
                    for pair in across.windows(2) {
                        drawing.segment(pair[0], pair[1], pen);
                    }
                }
            }
            if let (Some(&first), Some(&last)) = (points.first(), points.last()) {
                draw_arc(drawing, frame, spinner, first, pen, false);
                if last != first {
                    draw_arc(drawing, frame, spinner, last, pen, false);
                }
            }
        }
        Part::Rim(corner) => draw_arc(
            drawing,
            frame,
            spinner,
            look.outline.points[corner],
            pen,
            false,
        ),
        Part::Cap { end } => {
            let t = if end { spinner.radians() } else { 0.0 };
            draw_section(drawing, frame, spinner, look, t, pen);
        }
    }
}

/// The cross-section's outline at angle `t`, just outside the surface.
fn draw_section(
    drawing: &mut Drawing,
    frame: &Frame,
    spinner: &Spinner,
    look: &Look,
    t: f64,
    pen: draw::Pen,
) {
    let points: Vec<Vec3> = look
        .profile
        .iter()
        .map(|&point| outside(drawing, frame, spinner, look, t, point))
        .collect();
    for index in 0..points.len() {
        drawing.segment(points[index], points[(index + 1) % points.len()], pen);
    }
}

/// The line along the whole arc through the cross-section's `point`, just outside the surface;
/// dashed or not.
fn draw_arc(
    drawing: &mut Drawing,
    frame: &Frame,
    spinner: &Spinner,
    point: [f64; 2],
    pen: draw::Pen,
    dashed: bool,
) {
    let arc = spinner.radians();
    let segments = (arc.abs() / core::f64::consts::TAU * ARC_SEGMENTS)
        .ceil()
        .max(8.0) as usize;
    let inside = [0.0; 2];
    let lift = |t: f64| {
        let on = frame.point(spinner.section_point(t, point));
        let middle = frame.point(spinner.section_point(t, inside));
        lifted(drawing, on, middle)
    };
    let points: Vec<Vec3> = (0..=segments)
        .map(|segment| lift(arc * segment as f64 / segments as f64))
        .collect();
    for pair in points.windows(2) {
        if dashed {
            drawing.dashed(pair[0], pair[1], pen);
        } else {
            drawing.segment(pair[0], pair[1], pen);
        }
    }
}

/// The cross-section's `point` at angle `t`, in the vehicle, just outside the surface.
fn outside(
    drawing: &Drawing,
    frame: &Frame,
    spinner: &Spinner,
    look: &Look,
    t: f64,
    point: [f64; 2],
) -> Vec3 {
    let on = frame.point(spinner.section_point(t, point));
    let middle = frame.point(spinner.section_point(t, look.inside));
    lifted(drawing, on, middle)
}

/// `on` moved a pixel away from `middle` (inside the surface it's on), so the surface doesn't hide
/// a line through it.
fn lifted(drawing: &Drawing, on: Vec3, middle: Vec3) -> Vec3 {
    match geometry::normalized(geometry::sub(on, middle)) {
        Some(away) => geometry::add(on, geometry::scale(away, drawing.pixel_at(on))),
        None => on,
    }
}

/// How far `point` is from the ray (cm).
fn ray_distance(ray: &Ray, point: Vec3) -> f64 {
    let offset = geometry::sub(point, ray.origin);
    let along = geometry::dot(offset, ray.direction);
    geometry::length(geometry::sub(offset, geometry::scale(ray.direction, along)))
}

/// How long `offset` from `point` looks on the screen (pixels).
fn screen_length(ray: &Ray, pixels: &PixelSize, point: Vec3, offset: Vec3) -> f32 {
    let along = geometry::dot(offset, ray.direction);
    let across = geometry::length(geometry::sub(offset, geometry::scale(ray.direction, along)));
    (across / pixels.at(point)) as f32
}

fn dot2(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn sub2(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn close2(a: [f64; 2], b: [f64; 2], tolerance: f64) -> bool {
    (a[0] - b[0]).hypot(a[1] - b[1]) <= tolerance
}

/// The sides a cross-section's normal points to (see [`Grip::Section`]).
fn sides_along(normal: [f64; 2]) -> [i8; 2] {
    normal.map(|value| {
        if value.abs() <= NORMAL_TOLERANCE {
            0
        } else {
            value.signum() as i8
        }
    })
}

fn normalized2(a: [f64; 2]) -> Option<[f64; 2]> {
    let length = a[0].hypot(a[1]);
    (length > 1e-12).then(|| a.map(|value| value / length))
}
