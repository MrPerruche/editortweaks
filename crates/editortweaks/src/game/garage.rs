//! The map's garages (`AGarage`) and the editor's entry point, read through reflection. A garage
//! places the vehicles spawned at it from its reflected fields; `docs/re/map-in-editor.md` has how
//! (BRMK 1.11.6's `AGarage::GetVehicleSpawnBoxRelativeTransform`, 0x1CE2B0) and how this was found.
//!
//! The game state's `SpawnPointArray` lists every spawn point of the map (garages, spawn areas,
//! team bases, ...): the garages are the ones of class `Garage`. The array holds object pointers
//! or interface pointers (an object pointer then an interface pointer), whichever the property
//! says; both start with the object.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UStruct;

use crate::framework::log::log;
use crate::game::editor_object::Vector;
use crate::game::reflection;
use crate::game::world;

/// Where the vehicles spawned at a garage are put, as the garage's fields say.
#[derive(Clone, Copy, Debug)]
pub struct Garage {
    pub actor: *mut UObjectBase,
    /// Half the size of the spawn box (`VehicleSpawnExtent`).
    pub extent: Vector,
    /// Where the box is, past the building's front (`VehicleSpawnOffset`).
    pub offset: Vector,
    /// The box's rotation relative to the garage (`VehicleSpawnRotation`: pitch, yaw, roll).
    pub rotation: Vector,
    /// Where the box is for vehicles spawned pinned, or zero for the same as the others
    /// (`PinnedVehicleSpawnOrigin`).
    pub pinned_origin: Vector,
    /// The building's box (`BuildingBoundsOrigin`, `BuildingBoundsExtent`), which the spawn box
    /// starts after (along the garage's X).
    pub bounds_origin: Vector,
    pub bounds_extent: Vector,
}

pub struct Garages {
    /// `Garage`.
    class: *const UStruct,
    /// Offset of `UWorld::GameState`.
    game_state: usize,
    /// `BrickGameState`, and the offset and element size of its `SpawnPointArray`.
    state_class: *const UStruct,
    spawn_points: usize,
    spawn_point_size: usize,
    /// Offsets of the fields of `AGarage`.
    extent: usize,
    offset: usize,
    rotation: usize,
    pinned_origin: usize,
    /// Offsets of the fields of `ABrickBuilding`.
    bounds_origin: usize,
    bounds_extent: usize,
    /// Offset of `ABrickPlayerController::EditorEntryPoint`, if it's an object pointer.
    entry_point: Option<usize>,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for Garages {}
unsafe impl Sync for Garages {}

/// Finds the garages' fields. After engine init.
pub fn find() -> Result<Garages, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let garage = class("Garage")?;
    let building = class("BrickBuilding")?;
    let state = class("BrickGameState")?;
    let controller = class("BrickPlayerController")?;
    let spawn_points = reflection::field(state, "SpawnPointArray", "ArrayProperty", 0x10)?;
    // SAFETY: a live array property record; its inner property is live too, if not null.
    let spawn_point_size = unsafe {
        let inner = reflection::array_inner(spawn_points);
        if inner.is_null() {
            return Err("SpawnPointArray has no element type".into());
        }
        (*inner).element_size as usize
    };
    if !matches!(spawn_point_size, 8 | 16) {
        return Err(format!(
            "SpawnPointArray's elements are {spawn_point_size} bytes"
        ));
    }
    let vector = |class, name| reflection::field_offset(class, name, "StructProperty", 12);
    Ok(Garages {
        class: garage.cast(),
        game_state: reflection::field_offset(class("World")?, "GameState", "ObjectProperty", 8)?,
        state_class: state.cast(),
        // SAFETY: a live property record.
        spawn_points: unsafe { (*spawn_points).offset_internal as usize },
        spawn_point_size,
        extent: vector(garage, "VehicleSpawnExtent")?,
        offset: vector(garage, "VehicleSpawnOffset")?,
        rotation: vector(garage, "VehicleSpawnRotation")?,
        pinned_origin: vector(garage, "PinnedVehicleSpawnOrigin")?,
        bounds_origin: vector(building, "BuildingBoundsOrigin")?,
        bounds_extent: vector(building, "BuildingBoundsExtent")?,
        entry_point: reflection::field_offset(controller, "EditorEntryPoint", "ObjectProperty", 8)
            .map_err(|reason| {
                log!("the editor's entry garage isn't known (the first garage is the default): {reason}")
            })
            .ok(),
    })
}

impl Garages {
    /// Every garage of the current map, in the order of the game state's spawn points. Empty
    /// before there is a game state.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn all(&self) -> Vec<Garage> {
        // SAFETY: on the game thread; the world, its game state and the spawn point array are
        // read through reflected offsets, and every listed object is checked to be a live
        // garage before its fields are read.
        unsafe {
            let world = world::current();
            if world.is_null() {
                return Vec::new();
            }
            let state = world
                .cast::<u8>()
                .add(self.game_state)
                .cast::<*mut UObjectBase>()
                .read();
            if state.is_null() || !reflection::is_a(state, self.state_class) {
                return Vec::new();
            }
            let array = state.cast::<u8>().add(self.spawn_points);
            let data = array.cast::<*const u8>().read();
            let count = array.add(8).cast::<i32>().read();
            let Ok(count) = usize::try_from(count) else {
                return Vec::new();
            };
            if data.is_null() || count > 100_000 {
                return Vec::new();
            }
            (0..count)
                .filter_map(|index| {
                    let object = data
                        .add(index * self.spawn_point_size)
                        .cast::<*mut UObjectBase>()
                        .read();
                    self.read(object)
                })
                .collect()
        }
    }

    /// `object`'s garage fields, if it's a garage.
    ///
    /// # Safety
    /// `object` must be null or a live object. Game thread only.
    pub unsafe fn read(&self, object: *mut UObjectBase) -> Option<Garage> {
        // SAFETY: per the contract; the offsets come from reflection, on this class.
        unsafe {
            if object.is_null() || !reflection::is_a(object, self.class) {
                return None;
            }
            let field = |offset| {
                object
                    .cast::<u8>()
                    .add(offset)
                    .cast::<Vector>()
                    .read_unaligned()
            };
            Some(Garage {
                actor: object,
                extent: field(self.extent),
                offset: field(self.offset),
                rotation: field(self.rotation),
                pinned_origin: field(self.pinned_origin),
                bounds_origin: field(self.bounds_origin),
                bounds_extent: field(self.bounds_extent),
            })
        }
    }

    /// The actor the editor was entered through (`EditorEntryPoint`: a garage, a vehicle, ...), or
    /// null.
    ///
    /// # Safety
    /// `controller` must be a live `ABrickPlayerController`. Game thread only.
    pub unsafe fn entry_point(&self, controller: *const UObjectBase) -> *mut UObjectBase {
        let Some(offset) = self.entry_point else {
            return core::ptr::null_mut();
        };
        // SAFETY: per the contract; the offset comes from reflection.
        unsafe {
            controller
                .cast::<u8>()
                .add(offset)
                .cast::<*mut UObjectBase>()
                .read()
        }
    }
}
