//! The air's density on the current map, as the game's fluid dynamics use it: the world's default
//! physics volume's `FluidDensity` (a Brick Rigs field of `APhysicsVolume`, kg/cm³; 1.225e-6 on
//! Earth maps, 0 in space). Both reflected: `UWorld::DefaultPhysicsVolume` (made when something
//! first asks for it, so null on a map nothing has asked yet). See `docs/re/physics-centers.md`.

use brickrust::ue::coreuobject::UObjectBase;

use crate::game::reflection;
use crate::game::world;

/// The engine's default (`APhysicsVolume`'s constructor), kg/cm³.
pub const DEFAULT_DENSITY: f64 = 1.225e-6;

/// Reads the air's density (after engine init).
pub struct AirDensity {
    /// Offsets of `UWorld::DefaultPhysicsVolume` and `APhysicsVolume::FluidDensity`.
    volume: usize,
    density: usize,
}

pub fn find() -> Result<AirDensity, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    Ok(AirDensity {
        volume: reflection::field_offset(
            class("World")?,
            "DefaultPhysicsVolume",
            "ObjectProperty",
            8,
        )?,
        density: reflection::field_offset(
            class("PhysicsVolume")?,
            "FluidDensity",
            "FloatProperty",
            4,
        )?,
    })
}

impl AirDensity {
    /// The current world's air density (kg/cm³), if it has its default physics volume yet.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn current(&self) -> Option<f64> {
        // SAFETY: per the contract; the world and its volume are live if not null, and the
        // offsets come from reflection.
        unsafe {
            let world = world::current();
            if world.is_null() {
                return None;
            }
            let volume = world
                .cast::<u8>()
                .add(self.volume)
                .cast::<*mut UObjectBase>()
                .read();
            if volume.is_null() {
                return None;
            }
            let density = volume.cast::<u8>().add(self.density).cast::<f32>().read();
            density.is_finite().then_some(f64::from(density))
        }
    }
}
