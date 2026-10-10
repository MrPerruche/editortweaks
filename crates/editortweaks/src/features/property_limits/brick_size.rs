//! Scalable bricks constrain their size to a per-type range whenever it's modified
//! (`ConstraintBrickSize`, see `game::scalable_brick`). This applies the `BrickSize` rule instead,
//! which can still use the game's limits (its connector budget shrinks sizes down to the game's
//! minimum at most). The rule's `drag_min` already applied when the panel stored the size
//! (`property`).

use core::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use brickrust::ue::fproperty::FProperty;

use super::limits::{self, Edit};
use crate::framework::hook;
use crate::game::numeric_property::{NumericRange, NumericValue};
use crate::game::scalable_brick::{self, ConstraintBrickSizeFn};
use crate::prelude::*;

static ORIGINAL: OnceLock<ConstraintBrickSizeFn> = OnceLock::new();
/// Address of the `BrickSize` property record, set once reflection is available.
static BRICK_SIZE: OnceLock<usize> = OnceLock::new();

pub(super) fn resolve() -> Result<*const u8, String> {
    scalable_brick::CONSTRAINT_BRICK_SIZE.resolve()
}

/// # Safety
/// `target` must be `ConstraintBrickSize`, resolved by [`resolve`].
pub(super) unsafe fn install(target: *const u8) -> Result<(), String> {
    // SAFETY: `detour` matches `ConstraintBrickSizeFn`, the hooked function's signature.
    unsafe {
        hook::install_typed(
            scalable_brick::CONSTRAINT_BRICK_SIZE.name,
            target,
            detour as ConstraintBrickSizeFn,
            &ORIGINAL,
        )
    }
}

pub(super) fn on_engine_init() {
    if ORIGINAL.get().is_none() {
        return; // not hooked (pattern missing), already logged
    }
    match scalable_brick::brick_size_field() {
        Ok(property) => {
            // SAFETY: property records live as long as their class.
            let offset = unsafe { (*property).offset_internal };
            log!("brick sizes follow the rules (BrickSize at 0x{offset:X})");
            let _ = BRICK_SIZE.set(property as usize);
        }
        Err(reason) => log!("brick sizes keep the game's limits: {reason}"),
    }
}

unsafe extern "C" fn detour(brick: *mut c_void) {
    let original = *ORIGINAL.get().expect("hook recorded before use");
    let Some(&property) = BRICK_SIZE.get() else {
        // SAFETY: forwards the caller's argument unchanged.
        return unsafe { original(brick) };
    };
    let property = property as *const FProperty;

    // SAFETY: ConstraintBrickSize is a UScalableBrick member, so `brick` is one, and `property` is
    // its BrickSize (three floats) per the class's reflection data.
    unsafe {
        let size = brick
            .byte_add((*property).offset_internal as usize)
            .cast::<[f32; 3]>();
        let wanted = *size;
        if !wanted.iter().all(|c| c.is_finite()) {
            return original(brick);
        }

        let vanilla = game_range(original, brick, size);
        let max = vanilla.max.data;
        let Some(edit) = Edit::new(property, brick, &vanilla, 3) else {
            return original(brick);
        };
        let (limits, rule) = limits::resolve(&edit);
        let mut value = wanted;
        limits.clamp(&mut value);
        limits::adjust(&edit, &limits, rule, &mut value);
        *size = value;

        if value.iter().zip(max).any(|(&kept, max)| kept > max) {
            static LOGGED: AtomicBool = AtomicBool::new(false);
            if !LOGGED.swap(true, Ordering::Relaxed) {
                log!("kept a brick size above the game's maximum: {value:?} (maximum {max:?})");
            }
        }
    }
}

/// The game's own range for `object`'s `property`, if that's a brick size (and brick sizes follow
/// the rules): see [`game_range`].
///
/// # Safety
/// `object` must be a live object and `property` a property of its class. Game thread only.
pub(super) unsafe fn game_range_of(
    object: *mut c_void,
    property: *const FProperty,
) -> Option<NumericRange> {
    let original = *ORIGINAL.get()?;
    if BRICK_SIZE.get() != Some(&(property as usize)) {
        return None;
    }
    // SAFETY: only scalable bricks have this BrickSize property, so `object` is one, and
    // `property` is its BrickSize (three floats).
    unsafe {
        let size = object
            .byte_add((*property).offset_internal as usize)
            .cast::<[f32; 3]>();
        Some(game_range(original, object, size))
    }
}

/// The brick's own size range (`GetMinBrickSize`..`GetMaxBrickSize`, per brick type): run on
/// extreme sizes, `ConstraintBrickSize` (`original`) clamps them to it. The size is left as it was.
///
/// # Safety
/// `brick` must be a live scalable brick and `size` its `BrickSize`. Game thread only.
unsafe fn game_range(
    original: ConstraintBrickSizeFn,
    brick: *mut c_void,
    size: *mut [f32; 3],
) -> NumericRange {
    // SAFETY: per the contract. The original only clamps the size to the brick's minimum and
    // maximum, so calling it more than once has no other effect.
    unsafe {
        let wanted = *size;
        *size = [f32::MAX; 3];
        original(brick);
        let max = *size;
        *size = [f32::MIN; 3];
        original(brick);
        let min = *size;
        *size = wanted;
        NumericRange {
            min: NumericValue {
                data: min,
                num_used: 3,
            },
            max: NumericValue {
                data: max,
                num_used: 3,
            },
        }
    }
}
