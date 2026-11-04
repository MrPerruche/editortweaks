//! What the resize tool shows, as lines: boxes, the part the mouse is on, what it snaps to (marked
//! by its kind, see [`Mark`]). Drawn in the vehicle, shown in the world. Line widths are given in
//! pixels and turned into centimeters at the distance of each line's middle, so a line looks wider
//! where it's nearer; on a line long enough to go too far (the edge of a giant brick next to the
//! camera), the stretches past [`WIDTHS`]'s limits get a width of their own. What's drawn on top has
//! a dark halo so it stands out on light and dark bricks alike, and the snap point's mark pops up and
//! pulses.

use core::f64::consts::TAU;

use super::geometry::{self, Bounds, Frame, Vec3, WidthRule};
use super::model::{BrickModel, Element};
use super::parts::Part;
use crate::game::line_batcher::Line;
use crate::math;

/// How a line looks: its color (linear red, green, blue), its width in pixels (0: the thinnest),
/// whether it shows through what's in front of it, and whether it has a dark halo around it.
#[derive(Clone, Copy)]
pub struct Pen {
    pub color: [f32; 3],
    pub pixels: f32,
    pub on_top: bool,
    pub halo: bool,
}

/// The box of a brick that can be resized, under the mouse or being resized.
pub const BOX: Pen = Pen {
    color: [0.8, 0.8, 0.8],
    pixels: 0.0,
    on_top: false,
    halo: false,
};
/// The box of a brick that can't be resized (no `BrickSize`).
pub const FIXED_BOX: Pen = Pen {
    color: [0.15, 0.15, 0.15],
    pixels: 0.0,
    on_top: false,
    halo: false,
};
/// The part being pointed at or dragged.
pub const PART: Pen = Pen {
    color: [0.0, 0.55, 1.0],
    pixels: 3.0,
    on_top: true,
    halo: true,
};
/// A part of a brick's model that can't drag its box (its brick can't be resized, or it's in the
/// middle of the box).
pub const FIXED_PART: Pen = Pen {
    color: [0.15, 0.15, 0.15],
    pixels: 3.0,
    on_top: true,
    halo: false,
};
/// The box and part snapped to, and the point.
pub const TARGET_BOX: Pen = Pen {
    color: [1.0, 0.3, 0.0],
    pixels: 1.0,
    on_top: true,
    halo: true,
};
pub const TARGET: Pen = Pen {
    color: [1.0, 0.3, 0.0],
    pixels: 3.0,
    on_top: true,
    halo: true,
};
pub const SNAP_POINT: Pen = Pen {
    color: [1.0, 0.85, 0.0],
    pixels: 3.0,
    on_top: true,
    halo: true,
};
/// The stretch of an edge a snap point is halfway along, between two crossings.
pub const SPAN: Pen = Pen {
    color: [1.0, 0.85, 0.0],
    pixels: 2.0,
    on_top: true,
    halo: true,
};
/// From the dragged part to the point it snapped to, where they're apart (dashed): what it lines
/// up with.
pub const GUIDE: Pen = Pen {
    color: [1.0, 0.85, 0.0],
    pixels: 1.5,
    on_top: true,
    halo: true,
};
/// The selected spinners' curve (`spinner_curve`): its centre line, its nodes and handles' tips,
/// and its handles.
pub const CURVE: Pen = Pen {
    color: [0.0, 0.8, 0.55],
    pixels: 1.5,
    on_top: true,
    halo: true,
};
pub const CURVE_CONTROL: Pen = Pen {
    color: [0.0, 0.8, 0.55],
    pixels: 2.5,
    on_top: true,
    halo: true,
};
pub const CURVE_HANDLE: Pen = Pen {
    color: [0.0, 0.8, 0.55],
    pixels: 1.0,
    on_top: true,
    halo: false,
};
/// From a curve's control the spinners hold back to where the mouse asks for it (dashed).
pub const CURVE_HELD: Pen = Pen {
    color: [1.0, 0.35, 0.2],
    pixels: 1.5,
    on_top: true,
    halo: true,
};
/// A spinner's hub, while it's under the mouse with the resize key held.
pub const HUB: Pen = Pen {
    color: [0.55, 0.85, 1.0],
    pixels: 1.5,
    on_top: false,
    halo: false,
};
/// Where the mouse points while resizing, when the engine hides the cursor.
pub const POINTER: Pen = Pen {
    color: [1.0, 1.0, 1.0],
    pixels: 2.0,
    on_top: true,
    halo: true,
};

/// A line moved bricks turn about or slide along (see `hinge`), its points, and what a click would
/// place.
pub const LINE: Pen = Pen {
    color: [0.6, 0.1, 1.0],
    pixels: 2.5,
    on_top: true,
    halo: true,
};
/// The line past its points: the thinnest, hidden by bricks in front of it.
pub const LINE_PAST: Pen = Pen {
    color: [0.6, 0.1, 1.0],
    pixels: 0.0,
    on_top: false,
    halo: false,
};
/// The circle a grabbed point goes round the line on.
pub const CIRCLE: Pen = Pen {
    color: [0.6, 0.1, 1.0],
    pixels: 1.5,
    on_top: true,
    halo: true,
};

/// How long a corner's marks along its edges are (pixels), and the pointer's cross.
const CORNER_MARK_PIXELS: f32 = 22.0;
const POINTER_PIXELS: f32 = 16.0;
/// How wide a snap point's mark is (pixels), and the marks at the ends of a span.
pub const MARK_PIXELS: f32 = 20.0;
pub const END_MARK_PIXELS: f32 = 10.0;
/// A halo's color, how far it reaches past its line on each side (pixels), and how much further
/// from the camera than its line it is (share of the distance), so the line shows in front of it.
const HALO_COLOR: [f32; 3] = [0.01, 0.01, 0.01];
const HALO_PIXELS: f32 = 1.5;
const HALO_BEHIND: f64 = 0.002;
/// A new snap point's mark: how much bigger it starts (share of its size) and how long it takes to
/// shrink back (seconds); how often it pulses after (per second), and how close to white it gets.
const POP_GROWTH: f32 = 0.8;
const POP_SECONDS: f64 = 0.2;
const PULSE_HZ: f64 = 1.5;
const PULSE_WHITE: f64 = 0.65;
/// A dashed line's dashes and the gaps between them (pixels, about), and the most dashes it has.
const DASH_PIXELS: f32 = 6.0;
const GAP_PIXELS: f32 = 4.0;
const MAX_DASHES: f64 = 200.0;
/// How much wider or thinner than its pen a line may look along it, and so where it's cut when it
/// would go past that: into stretches between which the distance to the camera changes by 1.25
/// times (so about how much wider one is than the next), counting 1 cm as the nearest (a line right
/// through the camera gets no endless tiny stretches).
const WIDTHS: WidthRule = WidthRule {
    ratio: 1.25,
    nearest: 1.0,
    thinnest: 0.5,
    widest: 3.0,
};

/// The shape marking a snap point, facing the screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mark {
    Square,
    /// Pointing up.
    Triangle,
    /// Pointing down.
    TriangleDown,
    Diamond,
    Circle,
    /// An X.
    Cross,
    /// An X in a square.
    BoxedCross,
    Hourglass,
    /// A +.
    Plus,
}

/// The size of a pixel at a distance from the camera, and the screen's directions, in the vehicle.
#[derive(Clone, Copy)]
pub struct PixelSize {
    pub camera: Vec3,
    /// Centimeters per pixel per centimeter away.
    pub per_distance: f64,
    /// The directions going right and down on the screen (length 1).
    pub right: Vec3,
    pub down: Vec3,
}

impl PixelSize {
    /// The size of a pixel at `point`, in centimeters.
    pub fn at(&self, point: Vec3) -> f64 {
        geometry::length(geometry::sub(point, self.camera)) * self.per_distance
    }

    /// `pixels` pixels at `point`, in centimeters.
    fn of(&self, pixels: f32, point: Vec3) -> f64 {
        f64::from(pixels) * self.at(point)
    }
}

/// `pen` as it pulses, `seconds` after it started: white while its mark pops up, then back and
/// forth between nearly white and its color.
pub fn pulsed(pen: Pen, seconds: f64) -> Pen {
    let white = if seconds < POP_SECONDS {
        1.0
    } else {
        (0.5 + 0.5 * (TAU * PULSE_HZ * (seconds - POP_SECONDS)).cos()) * PULSE_WHITE
    } as f32;
    Pen {
        color: pen.color.map(|color| color + (1.0 - color) * white),
        ..pen
    }
}

/// How big a mark of `pixels` is `seconds` after it showed up: bigger at first, quickly shrinking
/// to its size.
pub fn popped(pixels: f32, seconds: f64) -> f32 {
    let left = (1.0 - seconds / POP_SECONDS).max(0.0) as f32;
    pixels * (1.0 + POP_GROWTH * left * left)
}

/// The lines to show this frame.
pub struct Drawing {
    lines: Vec<Line>,
    /// Drawn first, under all the lines.
    halos: Vec<Line>,
    pixels: PixelSize,
    /// Where the vehicle is in the world.
    vehicle: Frame,
}

impl Drawing {
    pub fn new(pixels: PixelSize, vehicle: Frame) -> Self {
        Self {
            lines: Vec::new(),
            halos: Vec::new(),
            pixels,
            vehicle,
        }
    }

    /// The lines drawn, halos first.
    pub fn into_lines(self) -> Vec<Line> {
        let mut lines = self.halos;
        lines.extend(self.lines);
        lines
    }

    /// A line from `start` to `end`: one, unless it's long enough to go past [`WIDTHS`]'s limits.
    /// The thinnest pen has no width to limit.
    pub fn segment(&mut self, start: Vec3, end: Vec3, pen: Pen) {
        if pen.pixels <= 0.0 {
            self.stroke(start, end, pen, 0.0);
            return;
        }
        let camera = self.pixels.camera;
        let stretches = geometry::stretches(start, end, camera, &WIDTHS);
        let at = |share| geometry::lerp([start, end], share);
        for stretch in geometry::merged(stretches) {
            self.stroke(
                at(stretch.from),
                at(stretch.to),
                pen,
                stretch.width_distance,
            );
        }
    }

    /// One line and its halo, as wide as `pen` says at `width_distance` from the camera.
    fn stroke(&mut self, start: Vec3, end: Vec3, pen: Pen, width_distance: f64) {
        if pen.halo {
            self.halo(start, end, pen, width_distance);
        }
        let line = self.line(start, end, pen, width_distance);
        self.lines.push(line);
    }

    fn line(&self, start: Vec3, end: Vec3, pen: Pen, width_distance: f64) -> Line {
        Line {
            start: math::narrow(self.vehicle.point(start)),
            end: math::narrow(self.vehicle.point(end)),
            color: pen.color,
            thickness: (f64::from(pen.pixels) * width_distance * self.pixels.per_distance) as f32,
            on_top: pen.on_top,
        }
    }

    /// A dark line under the segment from `start` to `end`: wider, a bit longer at both ends, and a
    /// bit further from the camera (where it looks the same), so the segment shows in front of it
    /// whether or not the engine tests the depth of lines on top.
    fn halo(&mut self, start: Vec3, end: Vec3, pen: Pen, width_distance: f64) {
        let pixels = self.pixels;
        let along = geometry::normalized(geometry::sub(end, start)).unwrap_or_default();
        let moved = |point: Vec3, outward: f64| {
            let point = geometry::add(
                point,
                geometry::scale(along, outward * pixels.of(HALO_PIXELS, point)),
            );
            let behind = geometry::scale(geometry::sub(point, pixels.camera), HALO_BEHIND);
            geometry::add(point, behind)
        };
        let halo = Pen {
            color: HALO_COLOR,
            pixels: pen.pixels + 2.0 * HALO_PIXELS,
            ..pen
        };
        let line = self.line(moved(start, -1.0), moved(end, 1.0), halo, width_distance);
        self.halos.push(line);
    }

    /// The twelve edges of a box, a pixel outside it all along (so it doesn't hide them, and on a
    /// giant brick they don't float away from it next to the camera).
    pub fn outline(&mut self, frame: &Frame, bounds: &Bounds, pen: Pen) {
        for along in 0..3 {
            let (a, b) = ((along + 1) % 3, (along + 2) % 3);
            for (side_a, side_b) in [(-1, -1), (-1, 1), (1, -1), (1, 1)] {
                let mut sides = [0; 3];
                sides[a] = side_a;
                sides[b] = side_b;
                sides[along] = -1;
                let start = bounds.point(sides);
                sides[along] = 1;
                let end = bounds.point(sides);
                let pixels = self.pixels;
                // A point along the edge at `share` of it, moved out by a pixel there; past the
                // edge's ends too, to meet the other edges at the corners.
                let out = |share: f64| {
                    let mut point = geometry::lerp([start, end], share);
                    let pixel = pixels.at(frame.point(point));
                    point[a] += f64::from(side_a) * pixel;
                    point[b] += f64::from(side_b) * pixel;
                    if share == 0.0 {
                        point[along] -= pixel;
                    } else if share == 1.0 {
                        point[along] += pixel;
                    }
                    frame.point(point)
                };
                let ends = (frame.point(start), frame.point(end));
                // Not merged: where it's moved out follows the distance, whatever its width does.
                for stretch in geometry::stretches(ends.0, ends.1, pixels.camera, &WIDTHS) {
                    let (from, to) = (out(stretch.from), out(stretch.to));
                    self.stroke(from, to, pen, stretch.width_distance);
                }
            }
        }
    }

    /// A part of a box: a face's outline, an edge, or marks along a corner's three edges.
    pub fn part(&mut self, frame: &Frame, bounds: &Bounds, part: Part, pen: Pen) {
        let corners = part.corners(bounds);
        match corners.len() {
            1 => {
                let corner = corners[0];
                let mark = self.pixels.of(CORNER_MARK_PIXELS, frame.point(corner));
                for axis in 0..3 {
                    let mut toward = part.sides;
                    toward[axis] = -toward[axis];
                    let edge = geometry::sub(bounds.point(toward), corner);
                    let length = geometry::length(edge);
                    let end = geometry::add(
                        corner,
                        geometry::scale(edge, (mark / length.max(1e-6)).min(0.4)),
                    );
                    self.segment(frame.point(corner), frame.point(end), pen);
                }
            }
            2 => self.segment(frame.point(corners[0]), frame.point(corners[1]), pen),
            count => {
                for index in 0..count {
                    let next = (index + 1) % count;
                    self.segment(frame.point(corners[index]), frame.point(corners[next]), pen);
                }
            }
        }
    }

    /// A part of a brick's model: a face's outline, an edge, or marks along a vertex's edges.
    /// `place` moves the model's points (in the brick) first, e.g. where a resize took them.
    pub fn element(
        &mut self,
        frame: &Frame,
        model: &BrickModel,
        element: Element,
        pen: Pen,
        place: impl Fn(Vec3) -> Vec3,
    ) {
        match element {
            Element::Face(face) => {
                for [start, end] in model.face_edges(face) {
                    self.segment(frame.point(place(start)), frame.point(place(end)), pen);
                }
            }
            Element::Edge(edge) => {
                let [start, end] = model.edge(edge);
                self.segment(frame.point(place(start)), frame.point(place(end)), pen);
            }
            Element::Vertex(vertex) => {
                let corner = place(model.point(vertex));
                let mark = self.pixels.of(CORNER_MARK_PIXELS, frame.point(corner));
                for neighbor in model.neighbors(vertex) {
                    let edge = geometry::sub(place(neighbor), corner);
                    let length = geometry::length(edge);
                    let end = geometry::add(
                        corner,
                        geometry::scale(edge, (mark / length.max(1e-6)).min(0.4)),
                    );
                    self.segment(frame.point(corner), frame.point(end), pen);
                }
            }
        }
    }

    /// A snap point's mark `size` pixels wide at `point`, facing the screen.
    pub fn mark(&mut self, mark: Mark, point: Vec3, size: f32, pen: Pen) {
        /// A point of a mark: x going right and y down on the screen, from -1 to 1.
        type Spot = (f64, f64);
        const X: [[Spot; 2]; 2] = [[(-0.8, -0.8), (0.8, 0.8)], [(-0.8, 0.8), (0.8, -0.8)]];
        const PLUS: [[Spot; 2]; 2] = [[(-1.0, 0.0), (1.0, 0.0)], [(0.0, -1.0), (0.0, 1.0)]];
        let square = vec![(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
        // A closed outline, and lines.
        let (outline, lines): (Vec<Spot>, &[[Spot; 2]]) = match mark {
            Mark::Square => (square, &[]),
            Mark::Triangle => (vec![(0.0, -1.0), (0.87, 0.5), (-0.87, 0.5)], &[]),
            Mark::TriangleDown => (vec![(0.0, 1.0), (0.87, -0.5), (-0.87, -0.5)], &[]),
            Mark::Diamond => (vec![(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)], &[]),
            Mark::Circle => (
                (0..12)
                    .map(|step| {
                        let angle = TAU * f64::from(step) / 12.0;
                        (angle.cos(), angle.sin())
                    })
                    .collect(),
                &[],
            ),
            Mark::Cross => (Vec::new(), &X),
            Mark::BoxedCross => (square, &X),
            Mark::Hourglass => (
                vec![(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)],
                &[],
            ),
            Mark::Plus => (Vec::new(), &PLUS),
        };
        let half = self.pixels.of(size, point) / 2.0;
        let (right, down) = (self.pixels.right, self.pixels.down);
        let at = |(x, y): Spot| {
            geometry::add(
                point,
                geometry::add(
                    geometry::scale(right, x * half),
                    geometry::scale(down, y * half),
                ),
            )
        };
        let around =
            (0..outline.len()).map(|index| [outline[index], outline[(index + 1) % outline.len()]]);
        let segments: Vec<[Vec3; 2]> = around
            .chain(lines.iter().copied())
            .map(|ends| ends.map(at))
            .collect();
        for [start, end] in segments {
            self.segment(start, end, pen);
        }
    }

    /// A snap point's mark on a face: a square `size` pixels wide lying in its plane (across
    /// `normal`, length 1) at `point`, and a short line out of it along the normal.
    pub fn flat_mark(&mut self, point: Vec3, normal: Vec3, size: f32, pen: Pen) {
        let half = self.pixels.of(size, point) / 2.0;
        // Two directions in the plane, the first one level on the screen if it can be.
        let Some(first) = geometry::normalized(geometry::cross(normal, self.pixels.down))
            .or_else(|| geometry::normalized(geometry::cross(normal, self.pixels.right)))
        else {
            return;
        };
        let second = geometry::cross(normal, first);
        let at = |x: f64, y: f64| {
            geometry::add(
                point,
                geometry::add(
                    geometry::scale(first, x * half),
                    geometry::scale(second, y * half),
                ),
            )
        };
        let corners = [at(-1.0, -1.0), at(1.0, -1.0), at(1.0, 1.0), at(-1.0, 1.0)];
        for index in 0..4 {
            self.segment(corners[index], corners[(index + 1) % 4], pen);
        }
        self.segment(
            point,
            geometry::add(point, geometry::scale(normal, half)),
            pen,
        );
    }

    /// The size of a pixel at `point`, in centimeters.
    pub fn pixel_at(&self, point: Vec3) -> f64 {
        self.pixels.at(point)
    }

    /// How long the segment from `start` to `end` looks (pixels, about: at its middle's distance).
    pub fn pixels_between(&self, start: Vec3, end: Vec3) -> f64 {
        let middle = geometry::scale(geometry::add(start, end), 0.5);
        geometry::length(geometry::sub(end, start)) / self.pixels.at(middle)
    }

    /// A dashed line from `start` to `end`.
    pub fn dashed(&mut self, start: Vec3, end: Vec3, pen: Pen) {
        let pixels = self.pixels_between(start, end);
        let period = f64::from(DASH_PIXELS + GAP_PIXELS);
        let count = (pixels / period).ceil().clamp(1.0, MAX_DASHES) as usize;
        let dash = f64::from(DASH_PIXELS) / period;
        let along = geometry::sub(end, start);
        let at = |share: f64| geometry::add(start, geometry::scale(along, share / count as f64));
        for index in 0..count {
            let from = index as f64;
            self.segment(at(from), at(from + dash), pen);
        }
    }

    /// A small upright cross at `point`, like the screen's cursor.
    pub fn pointer(&mut self, point: Vec3, pen: Pen) {
        let half = self.pixels.of(POINTER_PIXELS, point) / 2.0;
        for direction in [self.pixels.right, self.pixels.down] {
            let offset = geometry::scale(direction, half);
            self.segment(
                geometry::sub(point, offset),
                geometry::add(point, offset),
                pen,
            );
        }
    }
}
