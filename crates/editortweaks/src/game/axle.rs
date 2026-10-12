//! `UAxleBrick`: axles, and the arrows the editor draws on a selected one. Found through BRMK
//! 1.11.6's exports and reflection data, then in game 1.11.7 (builds 25633865 and 25831956); see
//! `docs/re/axle-arrows.md`.
//!
//! [`UPDATE_EDITOR_VISUALIZATION`] draws, for each of the axle's wheel connectors, up to two
//! circular arrows through its own lambda ([`SetArrowFn`]):
//!
//! ```text
//! T = GetConnectorRelativeTransform(Connector);
//! if (|SteeringAngle| > SMALL_NUMBER)       // "Positive" color, around the axle's Z axis
//!     SetArrow(T.Location, MakeFromZX(Up, T.Rotation * Z), angle 2 * SteeringAngle, radius 60)
//! if (bDriven)                              // "Negative" color, around its Y axis (-Y if inverted)
//!     SetArrow(T.Location + T.Rotation * (0, 0, 30 * T.Scale.Z), ..., angle 270, radius 30)
//! ```
//!
//! The suspension isn't shown. It lets a wheel move `SuspensionLength` down along the axle's Z
//! axis from where it was built, the top of its travel (see the documentation).

use core::ffi::c_void;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UStruct;

use crate::framework::sig::FunctionSig;
use crate::game::actuator::enum_byte;
use crate::game::editor_object::Vector;
use crate::game::reflection;

/// `virtual void UAxleBrick::UpdateEditorVisualization()` (BRMK 0xF7D40, game RVA 0xD10180,
/// split over several unwind entries): the game's `UBrick` version, then the wheel connectors'
/// arrows (see the module documentation); arrows left over from the last time are removed.
pub const UPDATE_EDITOR_VISUALIZATION: FunctionSig = FunctionSig {
    name: "UAxleBrick::UpdateEditorVisualization",
    // The steering arrow: its angle doubled, its color and radius, then the call to the arrow
    // lambda (registers and stack offsets wildcarded).
    pattern: "f3 0f 58 db c6 44 24 28 ?? 48 8d 54 24 ?? f3 ?? 0f 11 ?? 24 20 48 8d 4d ?? e8",
};

/// Where [`UPDATE_EDITOR_VISUALIZATION`]'s pattern calls the arrow lambda.
pub const ARROW_LAMBDA_CALL: usize = 25;

pub type UpdateEditorVisualizationFn = unsafe extern "C" fn(axle: *mut UObjectBase);

/// [`UPDATE_EDITOR_VISUALIZATION`]'s lambda (BRMK 0x79EC0, game RVA 0xCC9D00), like the actuators'
/// (`game::actuator::SET_ARROW`, with the same captures) but for circular arrows only: it sets an
/// arrow's place, `CircularArrowAngle` = `angle`, `CircularArrowRadius` = `size` and `ColorStyle`,
/// and leaves its `ArrowType` as it was. `rotation` is an `FRotator` (pitch, yaw, roll).
pub type SetArrowFn = unsafe extern "C" fn(
    captures: *mut c_void,
    location: *const Vector,
    rotation: *const [f32; 3],
    angle: f32,
    size: f32,
    color_style: u8,
);

/// Axles' properties and arrows, through reflection (after engine init).
pub struct Axles {
    /// `AxleBrick`.
    class: *const UStruct,
    /// Offsets of `UAxleBrick::SuspensionLength` and `SteeringAngle` (floats).
    suspension_length: usize,
    steering_angle: usize,
    /// `EBrickEditorArrowType::Linear` and `Circular`.
    pub linear_arrow: u8,
    pub circular_arrow: u8,
    /// `EBrickUIColorStyle::Positive`: the steering arrow's color, and the suspension's.
    pub positive: u8,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for Axles {}
unsafe impl Sync for Axles {}

impl Axles {
    pub fn find() -> Result<Self, String> {
        let class = reflection::find_class("AxleBrick").ok_or("class AxleBrick not found")?;
        Ok(Self {
            class: class.cast(),
            suspension_length: reflection::field_offset(
                class,
                "SuspensionLength",
                "FloatProperty",
                4,
            )?,
            steering_angle: reflection::field_offset(class, "SteeringAngle", "FloatProperty", 4)?,
            linear_arrow: enum_byte("EBrickEditorArrowType", "Linear")?,
            circular_arrow: enum_byte("EBrickEditorArrowType", "Circular")?,
            positive: enum_byte("EBrickUIColorStyle", "Positive")?,
        })
    }

    /// Whether `brick` is an axle.
    ///
    /// # Safety
    /// `brick` must be a live object.
    pub unsafe fn is_axle(&self, brick: *const UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { reflection::with_parents(reflection::class_of(brick.cast())) }
            .any(|class| class == self.class)
    }

    /// `axle`'s `SuspensionLength`, in cm.
    ///
    /// # Safety
    /// `axle` must be a live axle.
    pub unsafe fn suspension_length(&self, axle: *const UObjectBase) -> f32 {
        // SAFETY: per the contract; the offset comes from reflection.
        unsafe { self.float(axle, self.suspension_length).read() }
    }

    /// `axle`'s `SteeringAngle`, in degrees.
    ///
    /// # Safety
    /// `axle` must be a live axle.
    pub unsafe fn steering_angle(&self, axle: *const UObjectBase) -> f32 {
        // SAFETY: per the contract; the offset comes from reflection.
        unsafe { self.float(axle, self.steering_angle).read() }
    }

    /// Sets `axle`'s `SteeringAngle`, without telling it.
    ///
    /// # Safety
    /// `axle` must be a live axle. Game thread only.
    pub unsafe fn set_steering_angle(&self, axle: *mut UObjectBase, angle: f32) {
        // SAFETY: per the contract; the offset comes from reflection.
        unsafe {
            self.float(axle, self.steering_angle)
                .cast_mut()
                .write(angle)
        }
    }

    /// The float at `offset` in `axle`.
    ///
    /// # Safety
    /// `offset` must be one of a float field of `axle`'s.
    unsafe fn float(&self, axle: *const UObjectBase, offset: usize) -> *const f32 {
        // SAFETY: per the contract.
        unsafe { axle.cast::<u8>().add(offset).cast() }
    }
}
