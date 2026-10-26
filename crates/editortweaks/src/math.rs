//! Double-precision (`f64`) math for the editor's places, rotations and sizes.
//!
//! The game keeps these as `f32` and computes with them in `f32`: every step (a sine, a product)
//! rounds to `f32`, by about as much as `f32` can tell apart, so results come out a little off (a
//! turn of 90° gives 89.99999°) and drift further as edits pile up. Computed in `f64` from the
//! stored `f32` values, the steps' errors are a billion times smaller, and the only rounding left
//! is storing the result ([`narrow`]): the `f32` nearest to the exact result, so a result that
//! should be 90° is stored as 90°. (`f128` would store the same values.)
//!
//! Rotations follow Unreal's conventions: rotators are pitch, yaw, roll in degrees (`FRotator`),
//! quaternions x, y, z, w (`FQuat`), and `a * b` turns by `b`, then by `a`.

use core::ops::Mul;

pub mod hull;

/// A point or direction (`FVector`), or a rotator.
pub type Vec3 = [f64; 3];

pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: Vec3, factor: f64) -> Vec3 {
    a.map(|value| value * factor)
}

pub fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn length(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}

/// `a` scaled to length 1, if it has a length.
pub fn normalized(a: Vec3) -> Option<Vec3> {
    let length = length(a);
    (length > 1e-9).then(|| scale(a, 1.0 / length))
}

/// A game vector, exactly.
pub fn widen(value: [f32; 3]) -> Vec3 {
    value.map(f64::from)
}

/// The nearest `f32`s, to store.
pub fn narrow(value: Vec3) -> [f32; 3] {
    value.map(|value| value as f32)
}

/// How near zero a computed place or angle (cm, degrees) is taken as zero by [`narrow_settled`]:
/// far below anything placed on purpose, far above the `f64` noise of turning (about 1e-15).
const NOISE: f64 = 1e-9;

/// [`narrow`], with values within [`NOISE`] of zero stored as zero (and never -0). Elsewhere the
/// `f64` noise is far below what `f32` tells apart, but near zero `f32` keeps it: a turn that
/// leaves pitch at 0° may compute it as 2.6e-15°, which `f32` would store as is.
pub fn narrow_settled(value: Vec3) -> [f32; 3] {
    value.map(|value| {
        if value.abs() < NOISE {
            0.0
        } else {
            value as f32
        }
    })
}

/// A rotation, as a unit quaternion (`FQuat`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

impl Quat {
    pub const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    /// A game quaternion, scaled to length 1 (the game's are a little off).
    pub fn widen([x, y, z, w]: [f32; 4]) -> Self {
        let quat = Self {
            x: x.into(),
            y: y.into(),
            z: z.into(),
            w: w.into(),
        };
        let length = (quat.x * quat.x + quat.y * quat.y + quat.z * quat.z + quat.w * quat.w).sqrt();
        if length > 1e-9 {
            Self {
                x: quat.x / length,
                y: quat.y / length,
                z: quat.z / length,
                w: quat.w / length,
            }
        } else {
            Self::IDENTITY
        }
    }

    /// The turn by `degrees` about `axis` (length 1), like `FQuat(Axis, Angle)`.
    pub fn from_axis_angle(axis: Vec3, degrees: f64) -> Self {
        let (sin, cos) = (degrees.to_radians() / 2.0).sin_cos();
        Self {
            x: axis[0] * sin,
            y: axis[1] * sin,
            z: axis[2] * sin,
            w: cos,
        }
    }

    /// The rotation of a rotator (pitch, yaw, roll in degrees), like `FRotator::Quaternion`.
    pub fn from_rotator([pitch, yaw, roll]: Vec3) -> Self {
        let half = |angle: f64| (angle % 360.0).to_radians() / 2.0;
        let (sin_pitch, cos_pitch) = half(pitch).sin_cos();
        let (sin_yaw, cos_yaw) = half(yaw).sin_cos();
        let (sin_roll, cos_roll) = half(roll).sin_cos();
        Self {
            x: cos_roll * sin_pitch * sin_yaw - sin_roll * cos_pitch * cos_yaw,
            y: -cos_roll * sin_pitch * cos_yaw - sin_roll * cos_pitch * sin_yaw,
            z: cos_roll * cos_pitch * sin_yaw - sin_roll * sin_pitch * cos_yaw,
            w: cos_roll * cos_pitch * cos_yaw + sin_roll * sin_pitch * sin_yaw,
        }
    }

    /// The rotator the editor gives this rotation: `UFluMathStatics::QuatToNiceRotator` (game RVA
    /// 0xE387A0, BRMK 0x2CA470), in `f64`. Yaw and pitch point the brick's X axis where the
    /// rotation does (pitch ±90° when it points straight up or down, yaw then from its Z axis),
    /// roll turns it the rest of the way; each angle in (-180°, 180°].
    pub fn to_rotator(self) -> Vec3 {
        let Self { x, y, z, w } = self;
        let forward = [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + w * z),
            2.0 * (x * z - w * y),
        ];
        let (pitch, yaw) = if forward[0] * forward[0] + forward[1] * forward[1] > 1e-8 {
            (
                forward[2].clamp(-1.0, 1.0).asin().to_degrees(),
                forward[1].atan2(forward[0]).to_degrees(),
            )
        } else {
            let up = [2.0 * (w * y + x * z), 2.0 * (y * z - w * x)];
            let yaw = if up[0] * up[0] + up[1] * up[1] > 1e-8 {
                up[1].atan2(up[0]).to_degrees()
            } else {
                0.0
            };
            (if forward[2] > 0.0 { 90.0 } else { -90.0 }, yaw)
        };
        // What's left to turn about the brick's own X axis.
        let rest = Self::from_rotator([pitch, yaw, 0.0]).inverse() * self;
        let roll = -(2.0 * (rest.x * rest.w + rest.y * rest.z))
            .atan2(1.0 - 2.0 * (rest.x * rest.x + rest.z * rest.z))
            .to_degrees();
        [pitch, yaw, roll].map(normalize_angle)
    }

    /// [`Self::to_rotator`], or the other rotator of the same rotation (pitch `180° - p`, yaw and
    /// roll half a turn on) if that one is nearer `reference`. Pitch beyond ±90° (the camera upside
    /// down) is a rotator of its own: `to_rotator` always gives the one within ±90°, which would
    /// turn a view that keeps going over the top back the other way.
    pub fn to_rotator_near(self, reference: Vec3) -> Vec3 {
        let [pitch, yaw, roll] = self.to_rotator();
        let other = [180.0 - pitch, yaw + 180.0, roll + 180.0].map(normalize_angle);
        let gap = |rotator: Vec3| {
            (0..3)
                .map(|axis| normalize_angle(rotator[axis] - reference[axis]).abs())
                .sum::<f64>()
        };
        let first = [pitch, yaw, roll];
        if gap(other) < gap(first) {
            other
        } else {
            first
        }
    }

    /// The opposite rotation.
    pub fn inverse(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
            z: -self.z,
            w: self.w,
        }
    }

    /// `vector` turned by the rotation.
    pub fn rotate(self, vector: Vec3) -> Vec3 {
        let axis = [self.x, self.y, self.z];
        let twice = scale(cross(axis, vector), 2.0);
        add(add(vector, scale(twice, self.w)), cross(axis, twice))
    }

    /// How far apart two rotations are: their largest difference of components (`q` and `-q` are
    /// the same rotation).
    pub fn distance(self, other: Self) -> f64 {
        let a = [self.x, self.y, self.z, self.w];
        let b = [other.x, other.y, other.z, other.w];
        let apart = |sign: f64| {
            a.iter()
                .zip(b)
                .map(|(a, b)| (a - sign * b).abs())
                .fold(0.0, f64::max)
        };
        apart(1.0).min(apart(-1.0))
    }
}

impl Mul for Quat {
    type Output = Self;

    /// Turns by `other`, then by `self` (`FQuat::operator*`).
    fn mul(self, other: Self) -> Self {
        let (a, b) = (self, other);
        Self {
            x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
            y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
            z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
            w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
        }
    }
}

/// An angle in (-180°, 180°], like `FRotator::NormalizeAxis`.
fn normalize_angle(angle: f64) -> f64 {
    let mut angle = angle % 360.0;
    if angle < 0.0 {
        angle += 360.0;
    }
    if angle > 180.0 {
        angle -= 360.0;
    }
    angle
}
