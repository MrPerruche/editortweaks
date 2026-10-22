//! The world's gravity, as `AWorldSettings::GetGravityZ` works it out (engine, UE 4.27): the
//! `WorldGravityZ` it keeps once set (`bWorldGravitySet`), else `GlobalGravityZ` if the world sets
//! one (`bGlobalGravitySet`), else the project's `UPhysicsSettings::DefaultGravityZ`. All reflected;
//! the world settings are the current world's persistent level's (`UWorld::PersistentLevel`,
//! `ULevel::WorldSettings`). Projectiles fall at it times their `GravityScale` (`game::guns`).

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UStruct;

use crate::game::reflection::{self, BoolField};
use crate::game::world;

/// The engine's default, if nothing can be read: cm/s².
pub const DEFAULT_GRAVITY_Z: f64 = -980.0;

/// Reads the world's gravity (after engine init).
pub struct Gravity {
    /// Offsets of `UWorld::PersistentLevel` and `ULevel::WorldSettings`.
    persistent_level: usize,
    world_settings: usize,
    /// `WorldSettings`, and its fields.
    settings_class: *const UStruct,
    world_gravity_set: Option<BoolField>,
    world_gravity: usize,
    global_gravity_set: BoolField,
    global_gravity: usize,
    /// `UPhysicsSettings::DefaultGravityZ` (its default object's).
    default_gravity: f64,
}

// SAFETY: reflection records and offsets living as long as the game; only used on the game thread.
unsafe impl Send for Gravity {}
unsafe impl Sync for Gravity {}

pub fn find() -> Result<Gravity, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let settings = class("WorldSettings")?;
    let physics = class("PhysicsSettings")?;
    let default_offset = reflection::field_offset(physics, "DefaultGravityZ", "FloatProperty", 4)?;
    // SAFETY: a live class; its default object's field is read at its reflected offset.
    let default_gravity = unsafe {
        reflection::default_object(physics).map(|defaults| {
            f64::from(
                defaults
                    .cast::<u8>()
                    .add(default_offset)
                    .cast::<f32>()
                    .read_unaligned(),
            )
        })
    }
    .filter(|gravity| gravity.is_finite())
    .unwrap_or(DEFAULT_GRAVITY_Z);
    // SAFETY: `find_class` returns live classes.
    let settings_struct = unsafe { &(*settings).ustruct } as *const UStruct;
    Ok(Gravity {
        persistent_level: reflection::field_offset(
            class("World")?,
            "PersistentLevel",
            "ObjectProperty",
            8,
        )?,
        world_settings: reflection::field_offset(
            class("Level")?,
            "WorldSettings",
            "ObjectProperty",
            8,
        )?,
        settings_class: settings_struct,
        world_gravity_set: reflection::bool_field(settings_struct, "bWorldGravitySet").ok(),
        world_gravity: reflection::field_offset(settings, "WorldGravityZ", "FloatProperty", 4)?,
        global_gravity_set: reflection::bool_field(settings_struct, "bGlobalGravitySet")?,
        global_gravity: reflection::field_offset(settings, "GlobalGravityZ", "FloatProperty", 4)?,
        default_gravity,
    })
}

impl Gravity {
    /// The current world's gravity along Z (negative: down), in cm/s².
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn z(&self) -> f64 {
        // SAFETY: per the contract; each pointer is read at its reflected offset and checked
        // before use.
        unsafe {
            let world = world::current();
            if world.is_null() {
                return self.default_gravity;
            }
            let level = read::<*const UObjectBase>(world.cast(), self.persistent_level);
            if level.is_null() {
                return self.default_gravity;
            }
            let settings = read::<*const UObjectBase>(level.cast(), self.world_settings);
            if settings.is_null() || !reflection::is_a(settings, self.settings_class) {
                return self.default_gravity;
            }
            let base = settings.cast::<u8>();
            let world_gravity = f64::from(read::<f32>(base, self.world_gravity));
            let kept = match &self.world_gravity_set {
                Some(set) => set.get(base),
                // Without the flag, a gravity of exactly 0 reads as unset.
                None => world_gravity != 0.0,
            };
            let gravity = if kept {
                world_gravity
            } else if self.global_gravity_set.get(base) {
                f64::from(read::<f32>(base, self.global_gravity))
            } else {
                self.default_gravity
            };
            if gravity.is_finite() {
                gravity
            } else {
                self.default_gravity
            }
        }
    }
}

/// The `T` at `offset` in the object at `base`.
///
/// # Safety
/// `base + offset` must hold a `T`.
unsafe fn read<T>(base: *const u8, offset: usize) -> T {
    // SAFETY: per the contract.
    unsafe { base.add(offset).cast::<T>().read_unaligned() }
}
