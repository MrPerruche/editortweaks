//! Thrusters' thrust: how hard and where each thruster brick pushes at full throttle, as
//! `UThrusterBrick::TickBrick` pushes (BRMK 0x18B640). All through reflection; found through BRMK
//! 1.11.6's exports, see `docs/re/physics-centers.md`.
//!
//! ```text
//! Thrust = Abs(InputScale) * StaticInfo->Thrust * BrickSize.X * BrickSize.Y / 900   // GetThrustForce
//! Body->AddForceAtLocation(Body.Rotate((0, 0, 1)) * Thrust * Throttle,
//!                          Body.TransformPosition((0, 0, -BrickSize.Z / 2)))       // the nozzle
//! ```
//!
//! `Thrust` is the static info's "thrust produced at a nozzle size of 1x1 brick units" (30 cm
//! bricks: `/ 900`), `InputScale` the brick's "Thrust force" property (`SetThrustForce` stores the
//! force over the nozzle's thrust). The nozzle is the brick's bottom face
//! (`GetNozzleRelativeLocation`), and it pushes toward the brick's top.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::game::editor_object::Vector;
use crate::game::reflection;

/// Square centimeters in a nozzle of 1x1 brick units (`GetThrustFactor`'s `1 / 900`).
const NOZZLE_UNIT: f32 = 900.0;

/// A thruster's push at full throttle, in the brick's own space. A mirrored thruster (one negative
/// size) gets a negative force: it pushes the other way, as in the game.
#[derive(Clone, Copy, Debug)]
pub struct Push {
    /// Where it pushes: the nozzle.
    pub location: Vector,
    /// Which way it pushes (unit length).
    pub direction: Vector,
    /// How hard.
    pub force: f32,
}

/// Reads thrusters (after engine init).
pub struct Thrusters {
    /// `ThrusterBrick`.
    class: *const UStruct,
    /// Offsets of `UThrusterBrick::InputScale`, `UScalableBrick::BrickSize` and
    /// `UThrusterBrickStaticInfo::Thrust`.
    input_scale: usize,
    size: usize,
    thrust: usize,
    /// `ThrusterBrickStaticInfo`.
    info_class: *const UStruct,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for Thrusters {}
unsafe impl Sync for Thrusters {}

pub fn find() -> Result<Thrusters, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let thruster = class("ThrusterBrick")?;
    let info = class("ThrusterBrickStaticInfo")?;
    Ok(Thrusters {
        class: thruster.cast(),
        input_scale: reflection::field_offset(thruster, "InputScale", "FloatProperty", 4)?,
        size: reflection::field_offset(thruster, "BrickSize", "StructProperty", 12)?,
        thrust: reflection::field_offset(info, "Thrust", "FloatProperty", 4)?,
        info_class: info.cast(),
    })
}

impl Thrusters {
    /// How `brick` pushes at full throttle, if it's a thruster; `info_class` is its static info
    /// class (`UBrickEditorObject::StaticInfoClass`).
    ///
    /// # Safety
    /// `brick` must be a live object and `info_class` its live static info class. Game thread only.
    pub unsafe fn push(
        &self,
        brick: *const UObjectBase,
        info_class: *const UObjectBase,
    ) -> Option<Push> {
        // SAFETY: per the contract; the offsets come from reflection, checked against the classes
        // first.
        unsafe {
            let is_thruster = reflection::with_parents(reflection::class_of(brick.cast()))
                .any(|class| class == self.class);
            let has_thrust =
                reflection::with_parents(info_class.cast()).any(|class| class == self.info_class);
            if !is_thruster || !has_thrust {
                return None;
            }
            let info = reflection::default_object(info_class.cast::<UClass>())?;
            let base = brick.cast::<u8>();
            let input_scale = base.add(self.input_scale).cast::<f32>().read_unaligned();
            let [x, y, z] = base.add(self.size).cast::<Vector>().read_unaligned();
            let thrust = info
                .cast::<u8>()
                .add(self.thrust)
                .cast::<f32>()
                .read_unaligned();
            Some(Push {
                location: [0.0, 0.0, -z / 2.0],
                direction: [0.0, 0.0, 1.0],
                force: input_scale.abs() * thrust * x * y / NOZZLE_UNIT,
            })
        }
    }
}
