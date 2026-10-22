//! Bricks' lift surfaces: where a brick generating lift has it, as the game's fluid dynamics see it.
//! Found through BRMK 1.11.6's exports, then in game 1.11.7 (build 25831956); see
//! `docs/re/physics-centers.md`.
//!
//! Each part of a vehicle gathers the lift surfaces of its bricks every physics tick
//! (`UBrick::TickPartRootFluidDynamics`, through the virtual `GetFluidDynamicElements`): one per
//! brick whose lift is on, from the brick's virtual `GetFluidDynamicSurface`, which scalable bricks
//! (their size) and flaps (their angle) override. Lift then pushes each surface across the air
//! flowing past it.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UClass;

use crate::framework::sig::{self, CodeShape};
use crate::game::editor_object::{Transform, Vector};
use crate::game::reflection::{self, BoolField};

/// `virtual void UBrick::GetFluidDynamicElements(FFluidDynamicElements& Elements, const
/// FTransform& Transform) const` (BRMK 0xAE740, game RVA 0xCE39C0): adds the brick's lift surface
/// to its part's, if its lift is on (`ShouldUseFluidDynamicLiftSurface`, inlined):
///
/// ```text
/// if (Flags & bHasFluidDynamicLiftSurface && bGenerateLift)
///     GetFluidDynamicSurface(Elements.Surfaces[--Elements.NumSurfaces], Transform);  // virtual
/// ```
///
/// `bHasFluidDynamicLiftSurface` is cached when the brick is set up (game 0xCF32A0: its static
/// info's `LiftSurfaceRadius > 0`) and by `UScalableBrick::UpdateAerodynamicLiftSurface`.
/// `UFlapBrick`'s override (game 0xD365B0) also notes the surface's index, so it has another shape.
/// A leaf function: found among the virtual functions of `UBrick`'s default object.
pub const ELEMENTS: CodeShape = CodeShape {
    name: "UBrick::GetFluidDynamicElements",
    // The flag test (offset at +2, bit at +6), `bGenerateLift` (offset at +14), then the surface
    // taken from the end of the elements and the tail call (vtable offset at +48).
    shape: "f6 81 ?? ?? 00 00 ?? 4c 8b ca 74 ?? 80 b9 ?? ?? 00 00 00 74 ?? 48 8b 01 ba ff ff 00 00 66 41 01 11 41 0f b7 11 48 c1 e2 05 49 03 51 08 48 ff a0 ?? ?? 00 00 c3",
};

/// Where [`ELEMENTS`] holds the flags' offset, the flag's bit, `bGenerateLift`'s offset and
/// `GetFluidDynamicSurface`'s vtable offset.
const FLAGS_OFFSET: usize = 2;
const FLAG_BIT: usize = 6;
const GENERATE_LIFT_OFFSET: usize = 14;
const SURFACE_SLOT: usize = 48;

/// How many entries of `UBrick`'s vtable to look through for [`ELEMENTS`] (game 1.11.7: entry
/// 0x3B0 / 8 = 118).
const MAX_VIRTUALS: usize = 400;

/// The offsets [`ELEMENTS`] holds, as read from it.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Offsets {
    flags: usize,
    flag: u8,
    generate_lift: usize,
    slot: usize,
}

/// `FFluidDynamicSurface` (0x20 bytes, game and BRMK): a lift surface in the space of the transform
/// it was asked for. `UBrick`'s version (game 0xCE3A00) copies its static info's `LiftSurface*`
/// fields, the location and normal turned and moved by the transform (not scaled).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Surface {
    /// Where lift pushes it.
    pub location: Vector,
    /// Across the surface (unit length).
    pub normal: Vector,
    /// `LiftSurfaceRadius`: lift grows with `PI * Radius^2`.
    pub radius: f32,
    /// `LiftSurfaceCrossSectionArea`: for drag.
    pub area: f32,
}

/// `void GetFluidDynamicSurface(FFluidDynamicSurface& Out, const FTransform& Transform) const`.
type SurfaceFn =
    unsafe extern "C" fn(brick: *const UObjectBase, out: *mut Surface, transform: *const Transform);

/// Reads bricks' lift surfaces (after engine init).
pub struct LiftSurfaces {
    /// Offset of the brick's flags byte holding `bHasFluidDynamicLiftSurface`, and its bit.
    flags: usize,
    flag: u8,
    /// `UBrick::bGenerateLift`, the brick's "Generate lift" property.
    generate_lift: BoolField,
    /// Offset of `GetFluidDynamicSurface` in a brick's vtable.
    slot: usize,
}

// SAFETY: offsets from the game's code and reflection records living as long as the game; only
// used on the game thread.
unsafe impl Send for LiftSurfaces {}
unsafe impl Sync for LiftSurfaces {}

/// Finds it all (after engine init).
pub fn find() -> Result<LiftSurfaces, String> {
    let brick = reflection::find_class("Brick").ok_or("class Brick not found")?;
    // SAFETY: a live class.
    let Offsets {
        flags,
        flag,
        generate_lift: code_generate_lift,
        slot,
    } = unsafe { offsets(brick) }.ok_or_else(|| {
        format!(
            "{} not found among Brick's virtual functions",
            ELEMENTS.name
        )
    })?;
    // SAFETY: `find_class` returns a live class.
    let generate_lift = reflection::bool_field(unsafe { &(*brick).ustruct }, "bGenerateLift")?;
    let reflected = reflection::field_offset(brick, "bGenerateLift", "BoolProperty", 1)?;
    if reflected != code_generate_lift {
        return Err(format!(
            "{}: bGenerateLift at {code_generate_lift:#x}, reflected at {reflected:#x}",
            ELEMENTS.name
        ));
    }
    if flag.count_ones() != 1 || flags >= 0x400 || !slot.is_multiple_of(8) || slot >= 0x1000 {
        return Err(format!(
            "{}: unexpected flag {flags:#x}/{flag:#x} or vtable offset {slot:#x}",
            ELEMENTS.name
        ));
    }
    Ok(LiftSurfaces {
        flags,
        flag,
        generate_lift,
        slot,
    })
}

/// The offsets in [`ELEMENTS`], found among the virtual functions of `class`'s default object;
/// `None` if it isn't there, or there with different offsets.
///
/// # Safety
/// `class` must be the live class `Brick`.
unsafe fn offsets(class: *const UClass) -> Option<Offsets> {
    // SAFETY: per the contract. The vtable is read for [`MAX_VIRTUALS`] entries at most, up to the
    // first that isn't game code; past `UBrick`'s own, the scan reads other classes' (still
    // mapped), which only matters if one has the shape with other offsets (then nothing is found).
    // The bytes read from a function lie in its checked shape.
    unsafe {
        let defaults = reflection::default_object(class)?;
        let vtable = (*defaults).vtable.cast::<*const u8>().cast_const();
        let mut found = None;
        for index in 0..MAX_VIRTUALS {
            let function = vtable.add(index).read();
            if !sig::is_code(function) {
                break;
            }
            if !ELEMENTS.matches(function) {
                continue;
            }
            let u32_at = |offset| function.add(offset).cast::<u32>().read_unaligned() as usize;
            let offsets = Offsets {
                flags: u32_at(FLAGS_OFFSET),
                flag: function.add(FLAG_BIT).read(),
                generate_lift: u32_at(GENERATE_LIFT_OFFSET),
                slot: u32_at(SURFACE_SLOT),
            };
            if found.is_some_and(|known| known != offsets) {
                return None;
            }
            found = Some(offsets);
        }
        found
    }
}

impl LiftSurfaces {
    /// `brick`'s lift surface in the space `transform` places the brick in, if its lift is on.
    ///
    /// # Safety
    /// `brick` must be a live brick (`UBrick`). Game thread only.
    pub unsafe fn of(&self, brick: *const UObjectBase, transform: &Transform) -> Option<Surface> {
        // SAFETY: per the contract; the offsets and the slot come from the game's own check and
        // call on a brick, and the function is called the same way, into a surface of its size.
        unsafe {
            let base = brick.cast::<u8>();
            let lifts =
                base.add(self.flags).read() & self.flag != 0 && self.generate_lift.get(base);
            if !lifts {
                return None;
            }
            let function = (*brick)
                .vtable
                .cast::<u8>()
                .add(self.slot)
                .cast::<*const u8>()
                .read();
            if !sig::is_code(function) {
                return None;
            }
            let mut surface = Surface::default();
            core::mem::transmute::<*const u8, SurfaceFn>(function)(brick, &mut surface, transform);
            Some(surface)
        }
    }
}
