//! Slider widgets clamp typed values to their range in `SetValueInternal` (`game::slider`). For a
//! value outside the range, the range is widened for that one call so the game's own clamp keeps
//! it (its snapping still applies), then restored, so the slider keeps showing its normal range.
//! Values only get the widest limit here (±`SAFE_MAX`): the property they're stored into applies
//! the rules (`property`). Dragging and arrow keys clamp to the range themselves before calling it
//! (`slider_drag` can remove that from dragging).
//!
//! While a drag (or an arrow key) sets a slider's value, [`dragged_value`] tells the property what
//! that value is, for rules stopping drags earlier than typed values (`Rule::drag_min`).

use core::ffi::c_void;
use std::cell::Cell;
use std::sync::OnceLock;

use super::limits::SAFE_MAX;
use crate::framework::hook;
use crate::game::slider::{self, RangeFields, SetValueInternalFn, ValueFields};
use crate::prelude::*;

static ORIGINAL: OnceLock<SetValueInternalFn> = OnceLock::new();
/// Set once reflection is available; until then (or if it fails) nothing changes.
static RANGE: OnceLock<RangeFields> = OnceLock::new();
/// Set once reflection is available; until then (or if it fails) drags go as far as typed values.
static VALUE: OnceLock<ValueFields> = OnceLock::new();

thread_local! {
    /// The slider whose value a drag or an arrow key is setting on this thread, if any.
    static DRAGGED: Cell<Option<*mut c_void>> = const { Cell::new(None) };
}

pub(super) fn resolve() -> Result<*const u8, String> {
    slider::SET_VALUE_INTERNAL.resolve()
}

/// # Safety
/// `target` must be `SetValueInternal`, resolved by [`resolve`].
pub(super) unsafe fn install(target: *const u8) -> Result<(), String> {
    // SAFETY: `detour` matches `SetValueInternalFn`, the hooked function's signature.
    unsafe {
        hook::install_typed(
            slider::SET_VALUE_INTERNAL.name,
            target,
            detour as SetValueInternalFn,
            &ORIGINAL,
        )
    }
}

pub(super) fn on_engine_init() {
    match slider::range_fields() {
        Ok(fields) => {
            log!(
                "active (slider MinValue/MaxValue at 0x{:X}/0x{:X})",
                fields.min,
                fields.max
            );
            let _ = RANGE.set(fields);
        }
        Err(reason) => log!("inactive: {reason}"),
    }
    match slider::value_fields() {
        Ok(fields) => {
            let _ = VALUE.set(fields);
        }
        Err(reason) => log!("drags go as far as typed values: {reason}"),
    }
}

/// Whether slider values are being unclamped (the other parts only matter if they are).
pub(super) fn active() -> bool {
    RANGE.get().is_some()
}

/// The value a slider drag or arrow key is setting right now (on this thread), as the slider sends
/// it on (snapped); `None` while setting a typed value, or anything not from a slider.
pub(super) fn dragged_value() -> Option<f32> {
    let slider = DRAGGED.get()?;
    let fields = VALUE.get()?;
    // SAFETY: the slider whose SetValueInternal runs on this thread, so alive; it stores the new
    // value before sending it on.
    Some(unsafe { fields.value(slider) })
}

unsafe extern "C" fn detour(
    this: *mut c_void,
    value: f32,
    clamp_and_snap: usize,
    broadcast: usize,
    event_type: usize,
) -> u8 {
    // Drags and arrow keys send interactive changes; typed values and mouse releases commit.
    let dragged = (event_type as u8 == slider::INTERACTIVE).then_some(this);
    let previous = DRAGGED.replace(dragged);
    // SAFETY: the caller's arguments, unchanged.
    let result = unsafe { set_value(this, value, clamp_and_snap, broadcast, event_type) };
    DRAGGED.set(previous);
    result
}

/// `SetValueInternal`, keeping values outside of the slider's range.
///
/// # Safety
/// The arguments of a `SetValueInternal` call.
unsafe fn set_value(
    this: *mut c_void,
    value: f32,
    clamp_and_snap: usize,
    broadcast: usize,
    event_type: usize,
) -> u8 {
    let original = *ORIGINAL.get().expect("hook recorded before use");
    // SAFETY: forwards the caller's arguments, with only the value possibly replaced.
    let call = |value: f32| unsafe { original(this, value, clamp_and_snap, broadcast, event_type) };

    let Some(range) = RANGE.get() else {
        return call(value);
    };
    // Not clamping, or not a number: leave it to the game, which clamps it to the range.
    if clamp_and_snap & 0xFF == 0 || value.is_nan() {
        return call(value);
    }
    // Infinity comes from typed values too large once converted from the shown unit.
    let value = value.clamp(-SAFE_MAX, SAFE_MAX);

    // SAFETY: SetValueInternal is a UBrickSliderWidget member, so `this` is one; the offsets come
    // from that class's reflection data. UI code only runs on the game thread.
    unsafe {
        let min_field = this.byte_add(range.min).cast::<f32>();
        let max_field = this.byte_add(range.max).cast::<f32>();
        let (min, max) = (*min_field, *max_field);
        if (min..=max).contains(&value) {
            return call(value);
        }

        let (wide_min, wide_max) = (min.min(value), max.max(value));
        *min_field = wide_min;
        *max_field = wide_max;
        let result = call(value);
        // Restore, unless the widget set a new range meanwhile (the value change can rebuild it).
        if *min_field == wide_min {
            *min_field = min;
        }
        if *max_field == wide_max {
            *max_field = max;
        }
        log_kept(value, (min, max), event_type);
        result
    }
}

/// Logs a value kept outside of the slider's range: every committed one, but during a drag (one
/// value per frame) only those of a larger order of magnitude than before, which still shows how
/// far it went.
fn log_kept(value: f32, (min, max): (f32, f32), event_type: usize) {
    thread_local! {
        /// The largest order of magnitude logged since the last committed value.
        static LARGEST: Cell<i32> = const { Cell::new(i32::MIN) };
    }
    // Saturates: zero gives i32::MIN.
    let magnitude = value.abs().log10().floor() as i32;
    let log = match event_type as u8 {
        slider::COMMITTED => {
            LARGEST.set(i32::MIN);
            true
        }
        slider::INTERACTIVE if magnitude > LARGEST.get() => {
            LARGEST.set(magnitude);
            true
        }
        _ => false,
    };
    if log {
        log!("kept {value:?} (slider range {min:?}..{max:?})");
    }
}
