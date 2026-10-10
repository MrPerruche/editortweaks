//! What the vehicle editor knows of its objects (bricks, ...) through reflection: each one's type,
//! place, rotation and size, as the editor places and pairs them. Found through BRMK 1.11.6's
//! exports and reflection data; see `docs/re/editor-view-modes.md`.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::{UFunction, UStruct};

use crate::game::reflection::{self, Getter, Parameters};

/// An `FVector`, or an `FRotator` (pitch, yaw, roll in degrees).
pub type Vector = [f32; 3];

/// Reads where editor objects are (after engine init).
pub struct Places {
    /// Offset of `UBrickEditorObject::StaticInfoClass`, the object's type.
    static_info_class: usize,
    /// `FVector UBrickEditorObject::GetBrickEditorObjectSpawnLocation()` (BRMK +0x54, game +0x4C).
    location: Getter<Vector>,
    /// `FRotator UBrickEditorObject::GetBrickEditorObjectSpawnRotation()` (next to it).
    rotation: Getter<Vector>,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for Places {}
unsafe impl Sync for Places {}

/// An editor object, its type and where the editor sees it.
pub struct Place {
    pub object: *mut UObjectBase,
    /// Its static info class: same class, same type of brick.
    pub class: *const UObjectBase,
    pub location: Vector,
    pub rotation: Vector,
}

impl Places {
    pub fn find() -> Result<Self, String> {
        let object = reflection::find_class("BrickEditorObject")
            .ok_or("class BrickEditorObject not found")?;
        Ok(Self {
            static_info_class: reflection::field_offset(
                object,
                "StaticInfoClass",
                "ClassProperty",
                8,
            )?,
            location: Getter::find(
                object,
                "GetBrickEditorObjectSpawnLocation",
                "StructProperty",
            )?,
            rotation: Getter::find(
                object,
                "GetBrickEditorObjectSpawnRotation",
                "StructProperty",
            )?,
        })
    }

    /// Where `object` is, if it has a type.
    ///
    /// # Safety
    /// `object` must be a live editor object. Game thread only.
    pub unsafe fn of(&self, object: *mut UObjectBase) -> Option<Place> {
        // SAFETY: per the contract; the offset comes from reflection.
        unsafe {
            let class = object
                .cast::<u8>()
                .add(self.static_info_class)
                .cast::<*const UObjectBase>()
                .read();
            if class.is_null() {
                return None;
            }
            Some(Place {
                object,
                class,
                location: self.location.call(object),
                rotation: self.rotation.call(object),
            })
        }
    }
}

/// `virtual bool UBrickEditorObject::GetBrickEditorObjectLocalBounds(FVector& OutBoundsMin,
/// FVector& OutBoundsMax) const`: the object's box in its own space (unrotated, around its
/// location), if it has one. Each brick type reports its own size this way: bricks as their mesh's
/// box times their scale (BRMK 0xA7B00; a scalable brick's scale follows its size), spinners,
/// thrusters and winches with their own versions. Its reflection thunk calls the virtual.
pub struct LocalBounds {
    function: *const UFunction,
    min: usize,
    max: usize,
    result: usize,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for LocalBounds {}
unsafe impl Sync for LocalBounds {}

impl LocalBounds {
    pub fn find() -> Result<Self, String> {
        const NAME: &str = "GetBrickEditorObjectLocalBounds";
        let object = reflection::find_class("BrickEditorObject")
            .ok_or("class BrickEditorObject not found")?;
        let function = reflection::find_function(object, NAME)?;
        let parameters = function.cast::<UStruct>();
        let field = |name, kind, size| {
            reflection::struct_field_offset(parameters, name, kind, size)
                .map_err(|reason| format!("{NAME}: {reason}"))
        };
        let found = Self {
            function,
            min: field("OutBoundsMin", "StructProperty", 12)?,
            max: field("OutBoundsMax", "StructProperty", 12)?,
            result: field("ReturnValue", "BoolProperty", 1)?,
        };
        // SAFETY: a live function record.
        if !unsafe { Parameters::fit(function) } {
            return Err(format!("{NAME}: unexpected parameters"));
        }
        Ok(found)
    }

    /// The smallest and largest corners of `object`'s box, if it has one.
    ///
    /// # Safety
    /// `object` must be a live editor object. Game thread only.
    pub unsafe fn of(&self, object: *mut UObjectBase) -> Option<[Vector; 2]> {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the out parameters are written in the buffer (ProcessEvent
        // points them there), and every field lies within the checked parameter size.
        unsafe {
            let base = parameters.as_mut_ptr();
            reflection::call(object, self.function, base);
            if base.add(self.result).read() == 0 {
                return None;
            }
            let corner = |offset| base.add(offset).cast::<Vector>().read_unaligned();
            Some([corner(self.min), corner(self.max)])
        }
    }
}
