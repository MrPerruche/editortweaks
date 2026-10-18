//! The box around the vehicle in the editor: every editor object's local bounds, placed and turned
//! like the object, as the game's `CalcBrickEditorObjectsBounds` works it out.

use brickrust::ue::coreuobject::UObjectBase;

use crate::game::editor_object::{LocalBounds, Places};
use crate::game::editor_view::Bricks;
use crate::math::{self, Quat, Vec3};

/// The game's code the box is worked out with.
pub struct VehicleBoxes {
    pub bricks: Bricks,
    pub places: Places,
    pub bounds: LocalBounds,
}

impl VehicleBoxes {
    pub fn find() -> Result<Self, String> {
        Ok(Self {
            bricks: crate::game::editor_view::bricks()?,
            places: Places::find()?,
            bounds: LocalBounds::find()?,
        })
    }

    /// The smallest and largest corners of the editor's objects, in the vehicle's space; `None`
    /// with no objects.
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    pub unsafe fn of(&self, editor: *mut UObjectBase) -> Option<[Vec3; 2]> {
        // SAFETY: per the contract.
        unsafe { vehicle_box(&self.bricks, &self.places, &self.bounds, editor) }
    }
}

/// [`VehicleBoxes::of`] with the game's code passed one by one.
///
/// # Safety
/// As for [`VehicleBoxes::of`].
pub unsafe fn vehicle_box(
    bricks: &Bricks,
    places: &Places,
    bounds: &LocalBounds,
    editor: *mut UObjectBase,
) -> Option<[Vec3; 2]> {
    let mut found: Option<[Vec3; 2]> = None;
    // SAFETY: per the contract; the objects are the editor's live ones (null entries skipped).
    unsafe {
        for &object in bricks.editor_objects(editor) {
            if object.is_null() {
                continue;
            }
            let (Some(place), Some([low, high])) = (places.of(object), bounds.of(object)) else {
                continue;
            };
            let rotation = Quat::from_rotator(math::widen(place.rotation));
            let location = math::widen(place.location);
            for corner in 0..8 {
                let pick = |axis: usize| {
                    if corner >> axis & 1 == 0 {
                        f64::from(low[axis])
                    } else {
                        f64::from(high[axis])
                    }
                };
                let point = math::add(location, rotation.rotate([pick(0), pick(1), pick(2)]));
                let [min, max] = found.get_or_insert([point, point]);
                for axis in 0..3 {
                    min[axis] = min[axis].min(point[axis]);
                    max[axis] = max[axis].max(point[axis]);
                }
            }
        }
    }
    found
}
