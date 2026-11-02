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
//!
//! Their size's axis lock: see [`size_axis_lock`].

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::framework::sig::{self, CodeShape};
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

/// `EFluAxisLock UScalableBrick::GetBrickSizeAxisLock() const` (virtual; BRMK 1.11.6 export
/// 0x143820, vtable +0x5A8): `None` (0). `UThrusterBrick` overrides it (BRMK 0x143830) with `XY`
/// (1): its nozzle stays square.
///
/// The lock only reaches the property panel: `BrickSize`'s row binds an axis lock delegate
/// (BRMK 0x13D9F0: `Cast<UScalableBrick>(Container.Root) ? Root->GetBrickSizeAxisLock() : None`),
/// which `UNumericPropertyWidget::UpdateValue` (BRMK 0x3FD440) turns into the sliders it shows,
/// one per distinct `RemapAxisLockIndex(Lock, i)` (BRMK 0x2D28C0; XY: Y shows X's), and
/// `ClampValue` copies X into Y when the panel stores a size. Its other callers (BRMK, every
/// `call`/`jmp` through +0x5A8) are editor ones too: a sibling lambda (0x13E5C0) and
/// `UScalableBrick::GetFocusedConnectorAxisFlags` (0x146330), plus wheel bricks' own tables. No
/// loading, simulation or `ConstraintBrickSize` code reads it, so the game takes any X and Y
/// (thrust is `X * Y`, see the module docs).
pub type SizeAxisLockFn = unsafe extern "C" fn(brick: *const UObjectBase) -> u8;

/// `UScalableBrick::GetBrickSizeAxisLock`: `xor al, al; ret`. Identical leaf functions may be
/// folded into one, so it's only checked on the slot found through the vtables.
const NO_LOCK: CodeShape = CodeShape {
    name: "UScalableBrick::GetBrickSizeAxisLock",
    shape: "32 c0 c3",
};

/// `UThrusterBrick::GetBrickSizeAxisLock`: `mov al, 1; ret` (`EFluAxisLock::XY`).
const XY_LOCK: CodeShape = CodeShape {
    name: "UThrusterBrick::GetBrickSizeAxisLock",
    shape: "b0 01 c3",
};

/// The most virtual functions compared between the two tables (BRMK's `UThrusterBrick` has 193;
/// the shipping game has fewer, as it lacks the editor's).
const MAX_SLOTS: usize = 400;

/// The entry of `UThrusterBrick`'s virtual function table holding its `GetBrickSizeAxisLock`
/// (after engine init): the one slot where thrusters say XY and scalable bricks say none, comparing
/// both classes' default objects' tables (found by the functions' code, so a game update moving the
/// slot doesn't matter).
pub fn size_axis_lock() -> Result<*mut usize, String> {
    let vtable_of = |name: &'static str| -> Result<*mut usize, String> {
        let class = reflection::find_class(name).ok_or(format!("class {name} not found"))?;
        // SAFETY: a live class, whose default object lives as long; an object's first field is its
        // virtual function table.
        unsafe {
            let object =
                reflection::default_object(class).ok_or(format!("{name} has no default object"))?;
            Ok(*object.cast::<*mut usize>())
        }
    };
    let thruster = vtable_of("ThrusterBrick")?;
    let scalable = vtable_of("ScalableBrick")?;
    let mut found = Vec::new();
    for index in 0..MAX_SLOTS {
        // SAFETY: both tables are in the exe's data; each entry is read only after the ones before
        // it led into code, so this stops at the first table's end at the latest.
        let (ours, theirs) = unsafe { (*thruster.add(index), *scalable.add(index)) };
        let (ours, theirs) = (ours as *const u8, theirs as *const u8);
        if !sig::is_code(ours) || !sig::is_code(theirs) {
            break;
        }
        if XY_LOCK.matches(ours) && NO_LOCK.matches(theirs) {
            // SAFETY: an entry of the table, read above.
            found.push(unsafe { thruster.add(index) });
        }
    }
    match found[..] {
        [slot] => Ok(slot),
        [] => Err("no virtual function of ThrusterBrick locks its size like it did".into()),
        _ => Err(format!(
            "{} virtual functions of ThrusterBrick could be its size's axis lock",
            found.len()
        )),
    }
}
