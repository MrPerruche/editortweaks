//! How far projectiles go through bricks. Found in BRMK 1.11.6 (exports, reflected property
//! names); see `docs/re/brick-strength.md`.
//!
//! A projectile hitting a brick deals its whole damage to it, then, unless its range is used up,
//! may go on through it (`UBrickProjectileMovementComponent::HandleBlockingHit`, BRMK 0x32D420): it
//! can cross
//!
//! ```text
//! Reach = 100 * PenetrationCoefficient * Damage / Density   // cm of this material
//! ```
//!
//! where 100 is a global float (BRMK .data 0xBD12BC, 100 at build time), `PenetrationCoefficient`
//! the projectile's (reflected `ABrickProjectile` property),
//! `Damage` what it deals at this range, and `Density` the hit's physical material's (reflected
//! `UPhysicalMaterial::Density`; 1 unless it's a `UBrickPhysicalMaterial`). The brick's thickness
//! along its path (from where it entered to where it last leaves the brick) uses up `Thickness /
//! Reach` of the projectile; it stops in the brick that would use up more than what's left. So a
//! projectile goes through a line of bricks if `100 * PenetrationCoefficient * Damage` is more than
//! the sum of their `Thickness * Density`: the line's [armor](Densities::density).

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UStruct;

use crate::game::reflection;

/// Reads physical materials' densities (after engine init).
pub struct Densities {
    /// `BrickPhysicalMaterial`.
    brick_material: *const UStruct,
    /// Offset of `UPhysicalMaterial::Density`.
    density: usize,
}

// SAFETY: a reflection record and an offset, living as long as the game.
unsafe impl Send for Densities {}
unsafe impl Sync for Densities {}

pub fn find() -> Result<Densities, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    Ok(Densities {
        brick_material: class("BrickPhysicalMaterial")?.cast(),
        density: reflection::field_offset(
            class("PhysicalMaterial")?,
            "Density",
            "FloatProperty",
            4,
        )?,
    })
}

impl Densities {
    /// The density projectiles meet in `material` (a physical material, or null): 1 unless it's a
    /// brick's. A line's armor is the sum of each brick's thickness (cm) times its density.
    ///
    /// # Safety
    /// `material` must be null or a live object.
    pub unsafe fn density(&self, material: *const UObjectBase) -> f32 {
        // SAFETY: per the contract; a brick physical material has the reflected field.
        unsafe {
            if material.is_null() || !reflection::is_a(material, self.brick_material) {
                return 1.0;
            }
            material
                .cast::<u8>()
                .add(self.density)
                .cast::<f32>()
                .read_unaligned()
        }
    }
}
