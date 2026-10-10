//! The vehicle editor's mirror mode (automatic mirroring): its settings, and the game's own way to
//! mirror an editor object, to look at a brick's mirror image the way the editor pairs bricks. Found
//! through BRMK 1.11.6's exports, then in game 1.11.7 (build 25831956); see
//! `docs/re/editor-view-modes.md` ("Mirror pairing").
//!
//! The editor pairs a brick on one side of the mirror plane with a brick on the other side at its
//! mirror image when the type, place, rotation and every property match, the properties through a
//! check each brick type implements. Finding the bricks that pass everything but the properties
//! needs no knowledge of the properties: [`MirrorMode::images`] does the editor's other checks with
//! the editor's own functions, and whatever passes them without being paired failed on a property.
//! Bricks on the plane are never paired, so for them the properties stay unknown: checking them
//! would take the editor's internal property list (`FBrickPropertyReflection`, not reflected).

use std::collections::HashMap;

use brickrust::ue::coreuobject::UObjectBase;

use crate::framework::sig::FunctionSig;
use crate::game::editor_object::{Place, Places, Vector};
use crate::game::reflection::{self, Getter};

/// Our name for the loop of `ABrickEditor::UpdateMirroredObjects` (game RVA 0xCBC6D0, a lambda over
/// the editor's objects; BRMK 0x4F910) that looks for each object's counterpart:
///
/// ```text
/// // before: every object with Location[Axis] < 0 is hashed by its rounded location and class
/// for each object with Location[Axis] >= 0:     // on the mirror plane too
///     MirrorTransform(Location, Rotation.Quaternion(), MirroredLocation, MirroredRotation, Axis)
///     MirroredClass = Interface->GetBrickEditorObjectMirroredClass(StaticInfoClass)
///     for each offset in {0, -0.0001, 0.0001}^3:
///         // the candidate check (game 0xCB68F0): only objects with Location[Axis] < 0, so an
///         // object on the plane is never paired, not even with itself
///         for each candidate at RoundToInt(MirroredLocation + offset):
///             if class, location (0.0001 per coordinate) and rotation (q or -q, 0.0001 per
///             component) match, and candidate->IsBrickPropertyMirroredFrom(Object, Property,
///             Axis) for every property of the object (virtual, game vtable +0x2E8):
///                 pair them; take the candidate out of the hash
/// ```
const MATCHING: FunctionSig = FunctionSig {
    name: "ABrickEditor::UpdateMirroredObjects matching",
    // FRotator::Quaternion (call at +0xC), MirrorTransform (call at +0x2E), the editor's interface
    // component checked by class, then GetBrickEditorObjectMirroredClass (call at +0x74).
    pattern: "48 8d 4f ?? 66 0f 7f 44 24 ?? 8b 18 e8 ?? ?? ?? ?? 4c 8b c0 89 5c 24 ?? 48 8d 44 24 ?? 48 8b cf 48 8d 57 ?? 48 89 44 24 ?? 4c 8d 4c 24 ?? e8 ?? ?? ?? ?? 49 8b 47 ?? 48 8b 98 ?? ?? ?? ?? 48 85 db 74 ?? e8 ?? ?? ?? ?? 48 8b 53 10 4c 8d 40 ?? 48 63 40 ?? 3b 42 ?? 7f ?? 48 8b c8 48 8b 42 ?? 4c 39 04 c8 74 ?? 33 db 48 8b 54 24 ?? 48 8b cb 48 8b 52 ?? e8",
};

/// An `FQuat`, aligned like the engine's (for SIMD).
#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
struct Quat([f32; 4]);

/// `FQuat FRotator::Quaternion() const` (game RVA 0x10E4150): returned through `out`.
type QuaternionFn = unsafe extern "C" fn(rotator: *const Vector, out: *mut Quat) -> *mut Quat;

/// `void UBrickEditorObject::MirrorTransform(const FVector&, const FQuat&, FVector& OutLocation,
/// FQuat& OutRotation, EAxis::Type) const` (BRMK 0xBFD60, game RVA 0xCEDFE0): mirrors a transform
/// the way the object mirrors, which its static info sets (`MirrorMode`, `MirrorRotationOrigin`).
type MirrorTransformFn = unsafe extern "C" fn(
    object: *const UObjectBase,
    location: *const Vector,
    rotation: *const Quat,
    out_location: *mut Vector,
    out_rotation: *mut Quat,
    axis: u32,
);

/// `UClass* UBrickEditorInterfaceComponent::GetBrickEditorObjectMirroredClass(UClass*) const` (BRMK
/// 0xA7D10, game RVA 0xCDF500): the static info class of a mirrored object, e.g. a left wing's
/// right one; from the static info's `MirrorClassOverride`, else from its name.
type MirroredClassFn = unsafe extern "C" fn(
    interface: *const UObjectBase,
    static_info_class: *const UObjectBase,
) -> *const UObjectBase;

/// How far apart the editor still pairs objects, per coordinate of their locations and rotations
/// (`KINDA_SMALL_NUMBER`, the candidate check's constants).
const TOLERANCE: f32 = 1e-4;

/// Mirror mode's settings and the game's mirroring functions (after engine init).
pub struct MirrorMode {
    quaternion: QuaternionFn,
    mirror_transform: MirrorTransformFn,
    mirrored_class: MirroredClassFn,
    /// `UBrickUserSettings`' default object, to call its static functions on.
    settings_defaults: *mut UObjectBase,
    /// `static UBrickUserSettings* UBrickUserSettings::GetUserSettings()`.
    settings: Getter<*mut UObjectBase>,
    /// Offset of `UBrickUserSettings::bEditorMirrorAxisEnabled` (a native `bool`).
    enabled: usize,
    /// Offset of `UBrickUserSettings::EditorMirrorAxis` (`EAxis`: X 1, Y 2, Z 3).
    axis: usize,
    /// The type, location and rotation the editor pairs by.
    places: Places,
}

// SAFETY: game code and reflection records living as long as the game; only used on the game
// thread.
unsafe impl Send for MirrorMode {}
unsafe impl Sync for MirrorMode {}

pub fn find() -> Result<MirrorMode, String> {
    let matching = MATCHING.locate()?;
    let call = |offset, what: &str| {
        // SAFETY: each offset is a call inside the pattern (see its comment).
        unsafe { matching.call_target(offset) }
            .ok_or_else(|| format!("{}: no call to {what}", MATCHING.name))
    };
    let quaternion = call(0xC, "FRotator::Quaternion")?;
    let mirror_transform = call(0x2E, "MirrorTransform")?;
    let mirrored_class = call(0x74, "GetBrickEditorObjectMirroredClass")?;
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let settings = class("BrickUserSettings")?;
    // SAFETY: the call targets are game functions with these signatures (see MATCHING).
    let (quaternion, mirror_transform, mirrored_class) = unsafe {
        (
            core::mem::transmute::<*const u8, QuaternionFn>(quaternion),
            core::mem::transmute::<*const u8, MirrorTransformFn>(mirror_transform),
            core::mem::transmute::<*const u8, MirroredClassFn>(mirrored_class),
        )
    };
    Ok(MirrorMode {
        quaternion,
        mirror_transform,
        mirrored_class,
        settings_defaults: reflection::find_object(
            "Default__BrickUserSettings",
            "BrickUserSettings",
        )
        .ok_or("BrickUserSettings' default object not found")?,
        settings: Getter::find(settings, "GetUserSettings", "ObjectProperty")?,
        enabled: reflection::field_offset(settings, "bEditorMirrorAxisEnabled", "BoolProperty", 1)?,
        axis: reflection::field_offset(settings, "EditorMirrorAxis", "ByteProperty", 1)?,
        places: Places::find()?,
    })
}

/// An unpaired editor object, where the editor sees it, and its rotation as the editor compares it.
struct Placed {
    place: Place,
    rotation: Quat,
}

impl MirrorMode {
    /// The mirror axis (X 1, Y 2, Z 3), if mirror mode is on.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn axis(&self) -> Option<u8> {
        // SAFETY: per the contract; the settings are a live UBrickUserSettings if not null, and
        // both offsets come from reflection.
        unsafe {
            let settings = self.settings.call(self.settings_defaults).cast::<u8>();
            if settings.is_null() || settings.add(self.enabled).read() == 0 {
                return None;
            }
            let axis = settings.add(self.axis).read();
            (1..=3).contains(&axis).then_some(axis)
        }
    }

    /// The unpaired objects of `objects` (per `paired`), each with the unpaired object at its mirror
    /// image by type, place and rotation, checked like the editor does. Across the mirror plane the
    /// editor compared them and some property doesn't mirror. On the plane it never compares nor
    /// pairs objects: there an object can be its own mirror image (a symmetric brick), or another
    /// one's whatever their properties.
    ///
    /// # Safety
    /// `interface` must be the editor's live interface component, `objects` its editor objects
    /// (entries may be null), `axis` the one from [`Self::axis`]. Game thread only.
    pub unsafe fn images(
        &self,
        interface: *const UObjectBase,
        objects: &[*mut UObjectBase],
        axis: u8,
        paired: impl Fn(*mut UObjectBase) -> bool,
    ) -> Vec<(*mut UObjectBase, *mut UObjectBase)> {
        let placed: Vec<Placed> = objects
            .iter()
            .copied()
            .filter(|&object| !object.is_null() && !paired(object))
            // SAFETY: per the contract, a live editor object.
            .filter_map(|object| unsafe { self.placed(object) })
            .collect();
        // Like the editor's hash: by type and rounded location.
        let mut cells: HashMap<(usize, [i32; 3]), Vec<&Placed>> = HashMap::new();
        for object in &placed {
            let key = (object.place.class as usize, cell(object.place.location));
            cells.entry(key).or_default().push(object);
        }
        let mut mirrored_classes = HashMap::new();
        let mut images = Vec::new();
        // Every object, not only those on the side the editor looks from (`Location[Axis] >= 0`):
        // a brick a hair below the plane (a rounding error) can be its own mirror image too.
        for object in &placed {
            let class = *mirrored_classes
                .entry(object.place.class)
                // SAFETY: per the contract; the class is a live static info class.
                .or_insert_with(|| unsafe { (self.mirrored_class)(interface, object.place.class) });
            if class.is_null() {
                continue;
            }
            // SAFETY: per the contract.
            let (location, rotation) = unsafe { self.mirror(object, axis) };
            let [x, y, z] = cell(location);
            // Neighboring cells too: a location within the tolerance can round the other way.
            let around = (-1..=1).flat_map(|dx: i32| {
                (-1..=1).flat_map(move |dy: i32| {
                    (-1..=1).map(move |dz: i32| {
                        [
                            x.saturating_add(dx),
                            y.saturating_add(dy),
                            z.saturating_add(dz),
                        ]
                    })
                })
            });
            for key in around {
                for other in cells.get(&(class as usize, key)).into_iter().flatten() {
                    if close(&other.place.location, &location)
                        && same_rotation(&other.rotation, &rotation)
                    {
                        images.push((object.place.object, other.place.object));
                    }
                }
            }
        }
        images
    }

    /// Where the editor puts the counterpart of `object` if `object` is at `location` with
    /// `rotation` (pitch, yaw, roll): its mirror image across the plane of `axis`.
    ///
    /// # Safety
    /// `object` must be a live editor object, `axis` 1 to 3 (see [`Self::axis`]). Game thread only.
    pub unsafe fn mirrored_location(
        &self,
        object: *mut UObjectBase,
        location: Vector,
        rotation: Vector,
        axis: u8,
    ) -> Vector {
        let mut quaternion = Quat::default();
        // SAFETY: per the contract.
        unsafe {
            (self.quaternion)(&rotation, &mut quaternion);
            let place = Place {
                object,
                class: core::ptr::null(),
                location,
                rotation,
            };
            self.mirror(
                &Placed {
                    place,
                    rotation: quaternion,
                },
                axis,
            )
            .0
        }
    }

    /// # Safety
    /// `object` must be a live editor object. Game thread only.
    unsafe fn placed(&self, object: *mut UObjectBase) -> Option<Placed> {
        // SAFETY: per the contract.
        unsafe {
            let place = self.places.of(object)?;
            let mut rotation = Quat::default();
            (self.quaternion)(&place.rotation, &mut rotation);
            Some(Placed { place, rotation })
        }
    }

    /// Where the editor looks for the counterpart of `object`.
    ///
    /// # Safety
    /// `object.place.object` must be a live editor object, `axis` 1 to 3. Game thread only.
    unsafe fn mirror(&self, object: &Placed, axis: u8) -> (Vector, Quat) {
        let mut location = Vector::default();
        let mut rotation = Quat::default();
        // SAFETY: per the contract.
        unsafe {
            (self.mirror_transform)(
                object.place.object,
                &object.place.location,
                &object.rotation,
                &mut location,
                &mut rotation,
                u32::from(axis),
            );
        }
        (location, rotation)
    }
}

fn cell(location: Vector) -> [i32; 3] {
    location.map(|coordinate| coordinate.round() as i32)
}

fn close(a: &[f32], b: &[f32]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() <= TOLERANCE)
}

/// Whether two rotations are the same: `q` and `-q` are.
fn same_rotation(a: &Quat, b: &Quat) -> bool {
    close(&a.0, &b.0) || close(&a.0, &b.0.map(|component| -component))
}
