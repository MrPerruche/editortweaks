//! What the vehicle editor knows of its objects (bricks, ...) through reflection: each one's type,
//! place, rotation and size, as the editor places and pairs them; and (by signature) whether it's
//! selected. Found through BRMK 1.11.6's exports and reflection data; see
//! `docs/re/editor-view-modes.md` and `docs/re/editor-operations.md`.

use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::{UFunction, UStruct};

use crate::framework::sig::FunctionSig;
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

/// An `FTransform` (UE 4.27 with SIMD): a rotation quaternion (x, y, z, w), then the translation
/// and the 3D scale, each padded to four floats.
#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
pub struct Transform {
    pub rotation: [f32; 4],
    pub translation: [f32; 4],
    pub scale: [f32; 4],
}

/// Where editor objects are in the world: `FTransform UBrickEditorObject::GetWorldTransform()
/// const`, its root component's (the editor's own place, times the object's spawn transform).
pub struct WorldTransforms(Getter<Transform>);

// SAFETY: a reflection record living as long as the game; only used on the game thread.
unsafe impl Send for WorldTransforms {}
unsafe impl Sync for WorldTransforms {}

impl WorldTransforms {
    pub fn find() -> Result<Self, String> {
        let object = reflection::find_class("BrickEditorObject")
            .ok_or("class BrickEditorObject not found")?;
        Getter::find(object, "GetWorldTransform", "StructProperty").map(Self)
    }

    /// # Safety
    /// `object` must be a live editor object. Game thread only.
    pub unsafe fn of(&self, object: *mut UObjectBase) -> Transform {
        // SAFETY: per the contract.
        unsafe { self.0.call(object) }
    }
}

/// Moving editor objects like the move mode does when it commits a move
/// (`UBrickEditorMoveMode::EndMode`, see `docs/re/editor-operations.md`): it writes an object's
/// spawn location and rotation, then applies them (`UBrickEditorObject::ApplySpawnTransform`:
/// `RootComponent->SetRelativeLocationAndRotation(SpawnLocation, SpawnRotation)`).
///
/// The spawn location and rotation are plain fields (the rotation right after the location), only
/// readable through reflected getters. Where they are is found by looking for the getters' values
/// in objects ([`SpawnTransforms::learn`]).
pub struct SpawnTransforms {
    places: Places,
    /// `UPrimitiveComponent* UBrickEditorObject::GetRootComponent()`.
    root: Getter<*mut UObjectBase>,
    /// `void USceneComponent::K2_SetRelativeLocationAndRotation(FVector NewLocation, FRotator
    /// NewRotation, bool bSweep, FHitResult& SweepHitResult, bool bTeleport)`.
    set_relative: *const UFunction,
    new_location: usize,
    new_rotation: usize,
    /// Offset of the spawn location, once found.
    location: OnceLock<usize>,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for SpawnTransforms {}
unsafe impl Sync for SpawnTransforms {}

/// Where to look for the spawn location in an object: after `UObject`'s fields, within the
/// first bytes (BRMK has it at +0x54, the game at +0x4C).
const SPAWN_SEARCH: core::ops::Range<usize> = 0x28..0x200;

impl SpawnTransforms {
    pub fn find() -> Result<Self, String> {
        const SET: &str = "K2_SetRelativeLocationAndRotation";
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let object = class("BrickEditorObject")?;
        let set_relative = reflection::find_function(class("SceneComponent")?, SET)?;
        // SAFETY: a live function record.
        if !unsafe { Parameters::fit(set_relative) } {
            return Err(format!("{SET}: unexpected parameters"));
        }
        let parameter = |name, size| {
            reflection::parameter_offset(set_relative, SET, name, "StructProperty", size)
        };
        Ok(Self {
            places: Places::find()?,
            root: Getter::find(object, "GetRootComponent", "ObjectProperty")?,
            set_relative,
            new_location: parameter("NewLocation", 12)?,
            new_rotation: parameter("NewRotation", 12)?,
            location: OnceLock::new(),
        })
    }

    /// Where `object` is, as the editor places it.
    ///
    /// # Safety
    /// `object` must be a live editor object. Game thread only.
    pub unsafe fn of(&self, object: *mut UObjectBase) -> Option<Place> {
        // SAFETY: per the contract.
        unsafe { self.places.of(object) }
    }

    /// Finds where objects keep their spawn location, from the first of `objects` whose location
    /// and rotation show up at exactly one place in it (one at the origin, unrotated, shows up at
    /// many). Does nothing once known.
    ///
    /// # Safety
    /// `objects` must be live editor objects. Game thread only.
    pub unsafe fn learn(&self, objects: impl IntoIterator<Item = *mut UObjectBase>) {
        if self.location.get().is_some() {
            return;
        }
        for object in objects {
            // SAFETY: per the contract.
            if let Some(offset) = unsafe { self.search(object) } {
                let _ = self.location.set(offset);
                return;
            }
        }
    }

    /// Whether [`Self::learn`] found where objects keep their spawn location.
    pub fn known(&self) -> bool {
        self.location.get().is_some()
    }

    /// Where `object` is (location, rotation): read from its fields once [`Self::learn`] found
    /// them, else through the reflected getters (`None` without a type, see [`Places::of`]).
    ///
    /// # Safety
    /// `object` must be a live editor object. Game thread only.
    pub unsafe fn read(&self, object: *mut UObjectBase) -> Option<(Vector, Vector)> {
        // SAFETY: per the contract; the offset was found holding objects' places.
        unsafe {
            match self.location.get() {
                Some(&offset) => {
                    let [location, rotation] = object
                        .cast::<u8>()
                        .add(offset)
                        .cast::<[Vector; 2]>()
                        .read_unaligned();
                    Some((location, rotation))
                }
                None => self
                    .places
                    .of(object)
                    .map(|place| (place.location, place.rotation)),
            }
        }
    }

    /// # Safety
    /// `object` must be a live editor object. Game thread only.
    unsafe fn search(&self, object: *mut UObjectBase) -> Option<usize> {
        // SAFETY: per the contract; only bytes within the object (its class's size) are read.
        unsafe {
            let place = self.places.of(object)?;
            let class = reflection::class_of(object.cast());
            let size = usize::try_from((*class).properties_size).ok()?;
            let wanted: Vec<u8> = place
                .location
                .iter()
                .chain(&place.rotation)
                .flat_map(|value| value.to_ne_bytes())
                .collect();
            let last = SPAWN_SEARCH.end.min(size.checked_sub(wanted.len())?);
            let bytes = core::slice::from_raw_parts(object.cast::<u8>(), last + wanted.len());
            let mut found = (SPAWN_SEARCH.start..=last)
                .step_by(4)
                .filter(|&offset| bytes[offset..offset + wanted.len()] == wanted[..]);
            let first = found.next()?;
            found.next().is_none().then_some(first)
        }
    }

    /// Moves `object` to `location` with `rotation` (pitch, yaw, roll), like a committed move.
    /// Does nothing (returns false) until [`Self::learn`] found where objects keep their place.
    ///
    /// # Safety
    /// `object` must be a live editor object. Game thread only.
    pub unsafe fn set(&self, object: *mut UObjectBase, location: Vector, rotation: Vector) -> bool {
        let Some(&offset) = self.location.get() else {
            return false;
        };
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the offset was found holding objects' places, and the root
        // component is a live scene component if not null. The parameters lie within the checked
        // size; the others (sweep, teleport, the hit result) stay zero, as `ApplySpawnTransform`
        // passes them.
        unsafe {
            let fields = object.cast::<u8>().add(offset).cast::<[Vector; 2]>();
            fields.write_unaligned([location, rotation]);
            let root = self.root.call(object);
            if !root.is_null() {
                let base = parameters.as_mut_ptr();
                base.add(self.new_location)
                    .cast::<Vector>()
                    .write_unaligned(location);
                base.add(self.new_rotation)
                    .cast::<Vector>()
                    .write_unaligned(rotation);
                reflection::call(root, self.set_relative, base);
            }
        }
        true
    }
}

/// The editor's selection update (BRMK 0x4F700, called by `ABrickEditor::SelectObjects`; game RVA
/// 0xD04A90): gives each editor object its `EBrickSelectionState`, a byte in its `EditorParams`
/// (`UBrickEditorObject::SetSelectionState`, BRMK 0xE9990, inlined; BRMK +0x12, game +0xA):
///
/// ```text
/// State = IsSelected ? (Object == Primary ? 2 : 1) : 0;
/// if (Object->EditorParams && Object->EditorParams->SelectionState != State)
///     Object->EditorParams->SelectionState = State, Changed = true;
/// ```
///
/// Neither the state nor `EditorParams` is reflected.
pub const SELECTION_UPDATE: FunctionSig = FunctionSig {
    name: "ABrickEditor selection update",
    // The state (`sete al; inc al`, or 0), then `EditorParams` (its offset at +12), the state
    // compared and written (its offset at +20 and +25).
    pattern: "0f 94 c0 fe c0 eb 02 32 c0 49 8b 48 ?? 48 85 c9 74 ?? 3a 41 ?? 74 ?? 88 41 ?? b0 01",
};

/// Which editor objects are selected (after engine init).
pub struct Selection {
    /// Offset of an editor object's `EditorParams` pointer.
    editor_params: usize,
    /// Offset of the selection state in `EditorParams`.
    state: usize,
}

pub fn selection() -> Result<Selection, String> {
    let found = SELECTION_UPDATE.locate()?;
    // SAFETY: displacement bytes inside the matched pattern.
    let byte = |offset| usize::from(unsafe { found.address.add(offset).read() });
    if byte(20) != byte(25) {
        return Err(format!(
            "{}: compares and writes different fields",
            SELECTION_UPDATE.name
        ));
    }
    Ok(Selection {
        editor_params: byte(12),
        state: byte(20),
    })
}

impl Selection {
    /// Whether `object` is selected in its editor.
    ///
    /// # Safety
    /// `object` must be a live editor object (`UBrickEditorObject`).
    pub unsafe fn is_selected(&self, object: *const UObjectBase) -> bool {
        // SAFETY: per the contract; `EditorParams` is null or points to the object's parameters,
        // and both offsets come from the game's own code.
        unsafe {
            let params = object
                .cast::<u8>()
                .add(self.editor_params)
                .cast::<*const u8>()
                .read();
            !params.is_null() && params.add(self.state).read() != 0
        }
    }
}
