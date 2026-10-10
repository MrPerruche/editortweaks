//! What the resize tool shows, as lines: boxes, the part the mouse is on, what it snaps to. Drawn
//! in the vehicle, shown in the world. Line widths are given in pixels and turned into centimeters
//! at each line's distance.

use super::geometry::{self, Bounds, Frame, Vec3};
use super::parts::Part;
use crate::game::line_batcher::Line;
use crate::math;

/// How a line looks: its color (linear red, green, blue), its width in pixels (0: the thinnest),
/// and whether it shows through what's in front of it.
#[derive(Clone, Copy)]
pub struct Pen {
    pub color: [f32; 3],
    pub pixels: f32,
    pub on_top: bool,
}

/// The box of a brick that can be resized, under the mouse or being resized.
pub const BOX: Pen = Pen {
    color: [0.8, 0.8, 0.8],
    pixels: 0.0,
    on_top: false,
};
/// The box of a brick that can't be resized (no `BrickSize`).
pub const FIXED_BOX: Pen = Pen {
    color: [0.15, 0.15, 0.15],
    pixels: 0.0,
    on_top: false,
};
/// The part being pointed at or dragged.
pub const PART: Pen = Pen {
    color: [0.0, 0.55, 1.0],
    pixels: 3.0,
    on_top: true,
};
/// The box and part snapped to, and the point.
pub const TARGET_BOX: Pen = Pen {
    color: [1.0, 0.3, 0.0],
    pixels: 0.0,
    on_top: true,
};
pub const TARGET: Pen = Pen {
    color: [1.0, 0.3, 0.0],
    pixels: 3.0,
    on_top: true,
};
pub const SNAP_POINT: Pen = Pen {
    color: [1.0, 0.85, 0.0],
    pixels: 2.0,
    on_top: true,
};
/// Where the mouse points while resizing, when the engine hides the cursor.
pub const POINTER: Pen = Pen {
    color: [1.0, 1.0, 1.0],
    pixels: 2.0,
    on_top: true,
};

/// How long a corner's marks along its edges are (pixels), a point's cross, and the pointer's.
const CORNER_MARK_PIXELS: f32 = 22.0;
const CROSS_PIXELS: f32 = 10.0;
const POINTER_PIXELS: f32 = 16.0;

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

/// The lines to show this frame.
pub struct Drawing {
    pub lines: Vec<Line>,
    pixels: PixelSize,
    /// Where the vehicle is in the world.
    vehicle: Frame,
}

impl Drawing {
    pub fn new(pixels: PixelSize, vehicle: Frame) -> Self {
        Self {
            lines: Vec::new(),
            pixels,
            vehicle,
        }
    }

    pub fn segment(&mut self, start: Vec3, end: Vec3, pen: Pen) {
        let middle = geometry::scale(geometry::add(start, end), 0.5);
        self.lines.push(Line {
            start: math::narrow(self.vehicle.point(start)),
            end: math::narrow(self.vehicle.point(end)),
            color: pen.color,
            thickness: self.pixels.of(pen.pixels, middle) as f32,
            on_top: pen.on_top,
        });
    }

    /// The twelve edges of a box (just outside it, so it doesn't hide them).
    pub fn outline(&mut self, frame: &Frame, bounds: &Bounds, pen: Pen) {
        let center = frame.point(bounds.point([0; 3]));
        let bounds = bounds.grown(self.pixels.at(center));
        for along in 0..3 {
            let (a, b) = ((along + 1) % 3, (along + 2) % 3);
            for (side_a, side_b) in [(-1, -1), (-1, 1), (1, -1), (1, 1)] {
                let mut sides = [0; 3];
                sides[a] = side_a;
                sides[b] = side_b;
                sides[along] = -1;
                let start = frame.point(bounds.point(sides));
                sides[along] = 1;
                let end = frame.point(bounds.point(sides));
                self.segment(start, end, pen);
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

    /// A small cross at `point`, along the vehicle's axes.
    pub fn cross(&mut self, point: Vec3, pen: Pen) {
        let half = self.pixels.of(CROSS_PIXELS, point) / 2.0;
        for axis in 0..3 {
            let mut offset = [0.0; 3];
            offset[axis] = half;
            self.segment(
                geometry::sub(point, offset),
                geometry::add(point, offset),
                pen,
            );
        }
    }
}
