//! Where the air comes from and how fast, for the lift and drag arrows: a handle outside the vehicle
//! (an arrow pointing at it from where the air comes) that the mouse drags around it, snapping to
//! the vehicle's axes when near one, and whose speed the mouse wheel steps over it. It starts
//! straight ahead (the air coming at the vehicle's nose, its +X) in every new editor.
//!
//! The handle sits on a ball around the vehicle's box ([`Ball`]): dragged, it follows the point of
//! the ball under the mouse, on the side it was grabbed on (the front or the back of the ball, as
//! the camera sees it), or the point of the ball's outline nearest the mouse when the mouse is off
//! the ball.

use super::arrows::Arrow;
use super::centers::FORWARD;
use crate::features::mouse_moves::Pointer;
use crate::math::{self, Vec3};

/// How far the mouse may be from the handle and still be on it, in cm per cm away from the camera:
/// a few pixels on a usual screen.
pub const SLACK: f64 = 0.004;

/// How close to one of the vehicle's axes (forward, back, left, right, up, down) the handle snaps to
/// it, in degrees.
const SNAP_ANGLE: f64 = 4.0;

/// The speeds the mouse wheel steps through, in km/h or mph.
const SPEEDS: [f64; 30] = [
    5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0, 120.0, 140.0,
    160.0, 180.0, 200.0, 250.0, 300.0, 350.0, 400.0, 500.0, 600.0, 700.0, 800.0, 1000.0, 1200.0,
    1500.0, 2000.0,
];

/// A ball around the vehicle (in the editor root's space), which the handle goes round.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ball {
    pub center: Vec3,
    pub radius: f64,
}

impl Ball {
    /// How far out the handle's tip is, its length and thickness: proportioned to the vehicle.
    fn handle(&self) -> (f64, f64, f64) {
        let radius = self.radius;
        let gap = (0.1 * radius).max(30.0);
        (
            radius + gap,
            (0.6 * radius).clamp(100.0, 600.0),
            (0.06 * radius).clamp(10.0, 36.0),
        )
    }

    /// The radius the handle's middle goes round at.
    fn handle_radius(&self) -> f64 {
        let (near, length, _) = self.handle();
        near + length / 2.0
    }
}

/// The airflow and its handle.
pub struct Airflow {
    /// The way the vehicle moves through the air (unit length, in the editor root's space): the air
    /// comes from there, against it.
    direction: Vec3,
    /// The speed set with the mouse wheel (cm/s); `None`: the default for the units shown.
    speed: Option<f64>,
    drag: Option<Drag>,
    /// Whether the mouse was on the handle at the last [`Airflow::update`], where it was, and
    /// whether it was on the back of the handle's ball then.
    hovered: bool,
    mouse: Option<[f32; 2]>,
    on_back: bool,
}

/// A drag of the handle.
struct Drag {
    pointer: Pointer,
    /// Whether it follows the back of the ball (as the camera sees it).
    on_back: bool,
}

impl Airflow {
    pub const fn new() -> Self {
        Self {
            direction: FORWARD,
            speed: None,
            drag: None,
            hovered: false,
            mouse: None,
            on_back: false,
        }
    }

    /// The way the vehicle moves through the air (unit length).
    pub fn direction(&self) -> Vec3 {
        self.direction
    }

    /// The speed (cm/s): `default` until the mouse wheel set one.
    pub fn speed(&self, default: f64) -> f64 {
        self.speed.unwrap_or(default)
    }

    /// A step of the mouse wheel over the handle: the next speed of [`SPEEDS`] up or down (in the
    /// units of `per_cm_s` per cm/s) from the current one, `default` if none was set. Whether it
    /// was over the handle (then it's taken).
    pub fn step_speed(&mut self, faster: bool, per_cm_s: f64, default: f64) -> bool {
        if !self.hovered() {
            return false;
        }
        let shown = self.speed(default) * per_cm_s;
        let next = if faster {
            SPEEDS.into_iter().find(|&speed| speed > shown * 1.001)
        } else {
            SPEEDS
                .into_iter()
                .rev()
                .find(|&speed| speed < shown * 0.999)
        };
        let next = next.unwrap_or(if faster {
            SPEEDS[SPEEDS.len() - 1]
        } else {
            SPEEDS[0]
        });
        self.speed = Some(next / per_cm_s);
        true
    }

    /// The handle around `ball`: pointing at the vehicle from where the air comes.
    pub fn handle(&self, ball: &Ball) -> Arrow {
        let (near, length, thickness) = ball.handle();
        let out = |distance| math::add(ball.center, math::scale(self.direction, distance));
        Arrow {
            tail: out(near + length),
            tip: out(near),
            thickness,
        }
    }

    /// Whether the mouse is on the handle (or dragging it).
    pub fn hovered(&self) -> bool {
        self.hovered || self.drag.is_some()
    }

    /// Where the mouse points (screen pixels), given where the cursor is: the drag's pointer while
    /// dragging (see [`Pointer::point`]).
    pub fn pointer(&mut self, cursor: Option<[f32; 2]>) -> Option<[f32; 2]> {
        match &mut self.drag {
            Some(drag) => Some(drag.pointer.point(cursor)),
            None => cursor,
        }
    }

    /// Follows the mouse (`ray` through it in the editor root's space, a unit direction; `None` off
    /// the view, `mouse` the screen point): while dragging (and `held`, the button), the handle
    /// follows; else whether it's under the mouse. Whether the direction changed.
    pub fn update(
        &mut self,
        ray: Option<(Vec3, Vec3)>,
        mouse: Option<[f32; 2]>,
        ball: &Ball,
        held: bool,
    ) -> bool {
        self.mouse = mouse;
        if self.drag.is_some() && !held {
            // The release went elsewhere (another window).
            self.drag = None;
        }
        let Some((origin, direction)) = ray else {
            self.hovered = false;
            return false;
        };
        if let Some(drag) = &self.drag {
            let point = on_ball(
                ball.center,
                ball.handle_radius(),
                origin,
                direction,
                drag.on_back,
            );
            let Some(followed) = math::normalized(math::sub(point, ball.center)).map(snapped)
            else {
                return false;
            };
            let changed = followed != self.direction;
            self.direction = followed;
            return changed;
        }
        let handle = self.handle(ball);
        self.hovered = handle.hit(origin, direction, SLACK).is_some();
        if self.hovered {
            // The side of the ball the handle's middle is on, as seen along the ray.
            let middle = math::scale(math::add(handle.tail, handle.tip), 0.5);
            let [front, back] = [false, true].map(|back| {
                let point = on_ball(ball.center, ball.handle_radius(), origin, direction, back);
                math::length(math::sub(point, middle))
            });
            self.on_back = back < front;
        }
        false
    }

    /// A press of the select button: starts dragging the handle if the mouse is on it.
    pub fn press(&mut self) -> bool {
        let Some(mouse) = self.mouse.filter(|_| self.hovered && self.drag.is_none()) else {
            return false;
        };
        self.drag = Some(Drag {
            pointer: Pointer::new(mouse),
            on_back: self.on_back,
        });
        true
    }

    /// A release of the select button: ends the drag, if dragging.
    pub fn release(&mut self) -> bool {
        self.drag.take().is_some()
    }

    /// The mouse moved (screen pixels): the drag follows it, if dragging.
    pub fn moved(&mut self, pixels: [f32; 2]) -> bool {
        let Some(drag) = &mut self.drag else {
            return false;
        };
        drag.pointer.moved(pixels);
        true
    }

    /// Stops dragging and hovering (the handle is hidden).
    pub fn leave(&mut self) {
        self.drag = None;
        self.hovered = false;
    }

    /// The angle of attack and the sideslip, in degrees: how far below the nose (above, negative)
    /// and right of it (left, negative) the air comes from.
    pub fn angles(&self) -> (f64, f64) {
        let [x, y, z] = self.direction;
        (
            -z.clamp(-1.0, 1.0).asin().to_degrees(),
            y.atan2(x).to_degrees(),
        )
    }

    /// Where the air comes from, e.g. "straight ahead" or "from 5° below, 2° right".
    pub fn from(&self) -> String {
        let (attack, sideslip) = self.angles();
        let part = |angle: f64, positive: &str, negative: &str| {
            let angle = angle.round();
            (angle != 0.0).then(|| {
                let side = if angle > 0.0 { positive } else { negative };
                format!("{}° {side}", angle.abs())
            })
        };
        let parts: Vec<String> = [
            part(attack, "below", "above"),
            part(sideslip, "right", "left"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if parts.is_empty() {
            "straight ahead".to_owned()
        } else {
            format!("from {}", parts.join(", "))
        }
    }
}

/// `direction` (unit length), or the vehicle's axis it's within [`SNAP_ANGLE`] of.
fn snapped(direction: Vec3) -> Vec3 {
    let near = SNAP_ANGLE.to_radians().cos();
    (0..3)
        .flat_map(|axis| {
            [1.0, -1.0].map(|sign| {
                let mut unit = [0.0; 3];
                unit[axis] = sign;
                unit
            })
        })
        .find(|&unit| math::dot(unit, direction) >= near)
        .unwrap_or(direction)
}

/// The point of the ball around `center` of `radius` the ray meets (on its back if `back`, or the
/// only side ahead of the ray's origin), else the point of its outline nearest the ray.
fn on_ball(center: Vec3, radius: f64, origin: Vec3, direction: Vec3, back: bool) -> Vec3 {
    let offset = math::sub(origin, center);
    let b = math::dot(direction, offset);
    let c = math::dot(offset, offset) - radius * radius;
    let discriminant = b * b - c;
    let point = if discriminant >= 0.0 {
        let root = discriminant.sqrt();
        let (near, far) = (-b - root, -b + root);
        let along = if back || near < 0.0 { far } else { near };
        math::add(origin, math::scale(direction, along.max(0.0)))
    } else {
        math::add(origin, math::scale(direction, -b))
    };
    let out = math::normalized(math::sub(point, center)).unwrap_or(FORWARD);
    math::add(center, math::scale(out, radius))
}
