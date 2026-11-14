//! How mirror mode sizes a scalable brick's counterpart: the same size, its axes in the order the
//! brick's static info's `MirrorMode` says. Found through BRMK 1.11.6's exports; see
//! `docs/re/editor-view-modes.md` ("Mirrored sizes").
//!
//! A brick whose mirror image is itself turned a quarter turn (a quarter pyramid,
//! `EMirrorMode::XYSymmetry`) has its counterpart's X and Y sizes swapped:
//!
//! ```text
//! // UBrickEditorObject::GetMirrorAxes(int& X, int& Y, int& Z) (BRMK 0xB0BE0), by
//! // GetStaticInfo()->MirrorMode (the default object of the reflected StaticInfoClass):
//! //   YForward (1): -1, 2, 3     ZForward (2): 1, 2, -3
//! //   XYSymmetry (3): 2, 1, 3    XYNegSymmetry (4): -2, -1, 3     else (None): 1, -2, 3
//! // UScalableBrick::IsBrickPropertyMirroredFrom(Other, "BrickSize") (BRMK 0x15E3F0), on the
//! // counterpart with its own axes (1-based, signs ignored):
//! //   Other->BrickSize == (BrickSize[|X|-1], BrickSize[|Y|-1], BrickSize[|Z|-1])
//! ```
//!
//! `UScalableBrick::GetMirroredBrickSize` (BRMK 0x148DF0) reorders the same way.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UClass;

use crate::game::editor_object::Vector;
use crate::game::reflection;

/// Reads the mirror mode of editor objects (after engine init).
pub struct MirroredSizes {
    /// Offset of `UBrickEditorObject::StaticInfoClass`.
    static_info_class: usize,
    /// Offset of `UBrickEditorObjectStaticInfo::MirrorMode` (one byte).
    mirror_mode: usize,
    /// The values of `EMirrorMode::XYSymmetry` and `XYNegSymmetry`: X and Y swap.
    swapped: [i64; 2],
}

// SAFETY: reflection offsets and enum values; only used on the game thread.
unsafe impl Send for MirroredSizes {}
unsafe impl Sync for MirroredSizes {}

impl MirroredSizes {
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let object = class("BrickEditorObject")?;
        let info = class("BrickEditorObjectStaticInfo")?;
        // An `EMirrorMode` field: a `TEnumAsByte` (ByteProperty) or an enum class (EnumProperty).
        let mirror_mode = reflection::field_offset(info, "MirrorMode", "ByteProperty", 1)
            .or_else(|_| reflection::field_offset(info, "MirrorMode", "EnumProperty", 1))?;
        let value = |entry| reflection::enum_value("EMirrorMode", entry);
        Ok(Self {
            static_info_class: reflection::field_offset(
                object,
                "StaticInfoClass",
                "ClassProperty",
                8,
            )?,
            mirror_mode,
            swapped: [
                value("EMirrorMode::XYSymmetry")?,
                value("EMirrorMode::XYNegSymmetry")?,
            ],
        })
    }

    /// The size the editor pairs `counterpart` with a brick of `size`: `size` with its axes in
    /// `counterpart`'s mirror order. `None` if `counterpart` has no static info.
    ///
    /// # Safety
    /// `counterpart` must be a live editor object. Game thread only.
    pub unsafe fn of_counterpart(
        &self,
        counterpart: *const UObjectBase,
        size: Vector,
    ) -> Option<Vector> {
        // SAFETY: per the contract; both offsets come from reflection, the class field holds null
        // or a live `BrickEditorObjectStaticInfo` class, whose default object is one too.
        let mode = unsafe {
            let class = counterpart
                .byte_add(self.static_info_class)
                .cast::<*const UClass>()
                .read();
            if class.is_null() {
                return None;
            }
            let info = reflection::default_object(class)?;
            info.cast::<u8>().add(self.mirror_mode).read()
        };
        Some(reordered(size, self.swapped.contains(&i64::from(mode))))
    }
}

/// `size` for a counterpart: X and Y `swapped` or as they are.
fn reordered(size: Vector, swapped: bool) -> Vector {
    let [x, y, z] = size;
    if swapped { [y, x, z] } else { size }
}
