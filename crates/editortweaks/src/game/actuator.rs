//! `UActuatorBrick`: actuators (cylinders, sliders, rotors, ...), and the arrows the editor draws on
//! one to show how far it moves. Found through BRMK 1.11.6's exports and reflection data, then in
//! game 1.11.7 (build 25831956); see `docs/re/actuator-arrows.md`.
//!
//! [`UPDATE_EDITOR_VISUALIZATION`] draws a linear actuator's two limits as straight arrows along its
//! actuation axis, each only when its limit is on its usual side of zero (the game's sliders keep
//! them there: `MaxLimit` -20 to 0, `MinLimit` 0 to 20):
//!
//! ```text
//! Axis = unit vector of StaticInfo->ActuationAxis;
//! if (MinLimit > Threshold)                       // "Negative" color, pointing along the axis
//!     SetArrow(Axis * Offset, MakeFromZ(Axis), length MinLimit);
//! if (-MaxLimit > Threshold)                      // "Positive" color, pointing back
//!     SetArrow(Axis * (Offset + Range), MakeFromZ(-Axis), length -MaxLimit);
//! ```
//!
//! with `Offset` and `Range` the static info's `LinearActuationOffset` and `LinearActuationRange`.
//! Angular actuators get circular arrows instead.

use core::ffi::c_void;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UStruct;

use crate::framework::sig::FunctionSig;
use crate::game::editor_object::Vector;
use crate::game::reflection::{self, BoolField};

/// `virtual void UActuatorBrick::UpdateEditorVisualization()` (BRMK 0xF73F0, game RVA 0xD0F970):
/// the game's `UBrick` version, then the actuator's arrows (see the module documentation), each
/// through [`SET_ARROW`]; arrows left over from the last time are removed.
pub const UPDATE_EDITOR_VISUALIZATION: FunctionSig = FunctionSig {
    name: "UActuatorBrick::UpdateEditorVisualization",
    // The second linear arrow: -MaxLimit compared with the threshold, then its start
    // (LinearActuationRange + LinearActuationOffset, field offset wildcarded) and the skip.
    pattern: "45 0f 57 d1 45 0f 2f d3 f3 0f 58 93 ?? ?? 00 00 76",
};

pub type UpdateEditorVisualizationFn = unsafe extern "C" fn(actuator: *mut UObjectBase);

/// A lambda of [`UPDATE_EDITOR_VISUALIZATION`] (BRMK 0x797B0, game RVA 0xCC9750), its only caller:
/// takes the actuator's next arrow (a `UBrickEditorArrowComponent`), or creates one, and sets it up:
///
/// ```text
/// Arrow->SetRelativeLocationAndRotation(Location, Rotation);
/// set ArrowType, LinearArrowLength = Size, CircularArrowAngle = Angle, CircularArrowRadius = Size,
///     ColorStyle (each updating the arrow's mesh or material when it changed)
/// ```
///
/// Location and rotation are the actuator's own space; a linear arrow starts at `Location` and points
/// along the rotation's Z axis.
pub const SET_ARROW: FunctionSig = FunctionSig {
    name: "UActuatorBrick::UpdateEditorVisualization arrow lambda",
    // The arrow type: compared with the arrow's, stored if different, then `UpdateMesh` (field
    // offsets wildcarded).
    pattern: "0f b6 8d ?? ?? 00 00 3a 88 ?? ?? 00 00 74 ?? 88 88 ?? ?? 00 00 48 8b c8 e8",
};

/// `arrow_type` is an `EBrickEditorArrowType`, `color_style` an `EBrickUIColorStyle`; `captures`
/// points to the lambda's captures (the actuator, its editor params, the arrow counter).
pub type SetArrowFn = unsafe extern "C" fn(
    captures: *mut c_void,
    location: *const Vector,
    rotation: *const [f32; 4],
    angle: f32,
    size: f32,
    arrow_type: u8,
    color_style: u8,
);

/// Actuators' limits and arrows, through reflection (after engine init).
pub struct Actuators {
    /// `ActuatorBrick`.
    class: *const UStruct,
    /// Offsets of `UActuatorBrick::MaxLimit` and `MinLimit` (floats).
    max_limit: usize,
    min_limit: usize,
    /// Offset of `UBrickEditorObject::StaticInfoClass`, the brick's type.
    static_info_class: usize,
    /// `ActuatorBrickStaticInfo`, the actuators' types.
    static_info: *const UStruct,
    /// `UActuatorBrickStaticInfo::bIsAngularActuator`.
    angular: BoolField,
    /// `EBrickEditorArrowType::Linear`.
    pub linear_arrow: u8,
    /// `EBrickUIColorStyle::Positive`, the color of the `MaxLimit` arrow, and `Negative`, the
    /// `MinLimit` arrow's.
    pub positive: u8,
    pub negative: u8,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for Actuators {}
unsafe impl Sync for Actuators {}

impl Actuators {
    pub fn find() -> Result<Self, String> {
        let class =
            reflection::find_class("ActuatorBrick").ok_or("class ActuatorBrick not found")?;
        let object = reflection::find_class("BrickEditorObject")
            .ok_or("class BrickEditorObject not found")?;
        let static_info = reflection::find_class("ActuatorBrickStaticInfo")
            .ok_or("class ActuatorBrickStaticInfo not found")?;
        Ok(Self {
            class: class.cast(),
            max_limit: reflection::field_offset(class, "MaxLimit", "FloatProperty", 4)?,
            min_limit: reflection::field_offset(class, "MinLimit", "FloatProperty", 4)?,
            static_info_class: reflection::field_offset(
                object,
                "StaticInfoClass",
                "ClassProperty",
                8,
            )?,
            static_info: static_info.cast(),
            angular: reflection::bool_field(static_info.cast(), "bIsAngularActuator")?,
            linear_arrow: enum_byte("EBrickEditorArrowType", "Linear")?,
            positive: enum_byte("EBrickUIColorStyle", "Positive")?,
            negative: enum_byte("EBrickUIColorStyle", "Negative")?,
        })
    }

    /// Whether `brick` is a linear actuator (not an angular one).
    ///
    /// # Safety
    /// `brick` must be a live object. Game thread only.
    pub unsafe fn is_linear(&self, brick: *const UObjectBase) -> bool {
        // SAFETY: per the contract; the offsets come from reflection, and the static info is a live
        // class (checked to be an actuator type) whose default object is live.
        unsafe {
            if !reflection::with_parents(reflection::class_of(brick.cast()))
                .any(|class| class == self.class)
            {
                return false;
            }
            let class = brick
                .cast::<u8>()
                .add(self.static_info_class)
                .cast::<*const UStruct>()
                .read();
            if !reflection::with_parents(class).any(|class| class == self.static_info) {
                return false;
            }
            reflection::default_object(class.cast())
                .is_some_and(|info| !self.angular.get(info.cast::<u8>()))
        }
    }

    /// `brick`'s `MaxLimit` and `MinLimit`.
    ///
    /// # Safety
    /// `brick` must be a live actuator.
    pub unsafe fn limits(&self, brick: *const UObjectBase) -> [f32; 2] {
        // SAFETY: per the contract; the offsets come from reflection.
        unsafe {
            let base = brick.cast::<u8>();
            [self.max_limit, self.min_limit].map(|offset| base.add(offset).cast::<f32>().read())
        }
    }

    /// Sets `brick`'s `MaxLimit` and `MinLimit`, without telling it.
    ///
    /// # Safety
    /// `brick` must be a live actuator. Game thread only.
    pub unsafe fn set_limits(&self, brick: *mut UObjectBase, [max, min]: [f32; 2]) {
        // SAFETY: per the contract; the offsets come from reflection.
        unsafe {
            let base = brick.cast::<u8>();
            base.add(self.max_limit).cast::<f32>().write(max);
            base.add(self.min_limit).cast::<f32>().write(min);
        }
    }
}

/// The value of the entry `entry` of the reflected enum `name`, as the byte the game stores.
pub fn enum_byte(name: &'static str, entry: &str) -> Result<u8, String> {
    let value = reflection::enum_value(name, &format!("{name}::{entry}"))?;
    u8::try_from(value).map_err(|_| format!("{name}::{entry} is {value}, not a byte"))
}
