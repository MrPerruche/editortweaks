//! Numeric properties clamp values again when storing them (`ClampValue`, see
//! `game::numeric_property`). While it runs, `GetValueRange` answers with the limits from the rules
//! instead of the property's own range, so the rest of `ClampValue` (axis locks) still applies; then
//! the rule's adjustment, if any, runs on the result. The panel's sliders still show the property's
//! own range: the panel asks for it outside of `ClampValue`, which is when `highlight` checks the
//! shown value against the rules.
//!
//! A value from a slider drag also stops at its rule's `drag_min` here ([`stop_drag`]).

use core::ffi::c_void;
use std::cell::RefCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use super::limits::{self, Edit, Limits, Rule};
use crate::features::{axis_permutations, slider_drag, slider_formulas};
use crate::framework::{hook, timings};
use crate::game::numeric_property::{
    self, ClampValueFn, Container, GetValueRangeFn, NumericRange, NumericValue,
};
use crate::prelude::*;

static CLAMP_VALUE: OnceLock<ClampValueFn> = OnceLock::new();
static GET_VALUE_RANGE: OnceLock<GetValueRangeFn> = OnceLock::new();
/// Where `GetValueRange` lives, to spot property classes that override it.
static GET_VALUE_RANGE_TARGET: OnceLock<usize> = OnceLock::new();

thread_local! {
    /// The `ClampValue` call in progress on this thread, if any.
    static CLAMPING: RefCell<Option<Clamping>> = const { RefCell::new(None) };
}

/// A `ClampValue` call in progress.
struct Clamping {
    property: *const c_void,
    container: *const c_void,
    components: usize,
    /// Set once `GetValueRange` answered with a rule's limits.
    resolved: Option<Resolved>,
}

struct Resolved {
    edit: Edit,
    limits: Limits,
    rule: Option<&'static Rule>,
}

pub(super) struct Targets {
    clamp_value: *const u8,
    get_value_range: *const u8,
}

pub(super) fn resolve() -> Result<Targets, String> {
    Ok(Targets {
        clamp_value: numeric_property::CLAMP_VALUE.resolve()?,
        get_value_range: numeric_property::GET_VALUE_RANGE.resolve()?,
    })
}

/// # Safety
/// `targets` must come from [`resolve`].
pub(super) unsafe fn install(targets: Targets) -> Result<(), String> {
    let _ = GET_VALUE_RANGE_TARGET.set(targets.get_value_range as usize);
    // SAFETY: each detour matches the hooked function's signature (`ClampValueFn`,
    // `GetValueRangeFn`).
    unsafe {
        hook::install_typed(
            numeric_property::CLAMP_VALUE.name,
            targets.clamp_value,
            clamp_value_detour as ClampValueFn,
            &CLAMP_VALUE,
        )?;
        hook::install_typed(
            numeric_property::GET_VALUE_RANGE.name,
            targets.get_value_range,
            get_value_range_detour as GetValueRangeFn,
            &GET_VALUE_RANGE,
        )
    }
}

/// Whether `ClampValue` is hooked, so features can rewrite stored values from it.
pub(super) fn hooked() -> bool {
    CLAMP_VALUE.get().is_some()
}

unsafe extern "C" fn clamp_value_detour(
    property: *const c_void,
    out: *mut NumericValue,
    container: *const c_void,
    input: *const NumericValue,
) -> *mut NumericValue {
    let _span = timings::span("property_limits::clamp_value_detour");
    let original = *CLAMP_VALUE.get().expect("hook recorded before use");
    // SAFETY: `input` is the caller's value, valid for the whole call.
    let input = unsafe { &*input };
    // A typed formula using `x` gives each edited object its own value (`slider_formulas`), and so
    // does putting values in their next order (`axis_permutations`).
    // SAFETY: these are the arguments of this ClampValue call.
    let per_object = unsafe {
        slider_formulas::per_object_value(property, container, input)
            .or_else(|| axis_permutations::per_object_value(property, container, input))
    };
    let input = per_object.as_ref().unwrap_or(input);
    // SAFETY: forwards the caller's arguments, with only the input value possibly replaced.
    let call_original = || unsafe {
        timings::game(numeric_property::CLAMP_VALUE.name, || {
            original(property, out, container, input)
        })
    };

    if !super::slider::active() || !all_finite(input) {
        return call_original();
    }
    warn_if_range_overridden(property);

    let previous = CLAMPING.replace(Some(Clamping {
        property,
        container,
        components: usize::from(input.num_used),
        resolved: None,
    }));
    let result = call_original();
    let clamping = CLAMPING.replace(previous);

    // `GetValueRange` isn't called when the property doesn't clamp (its "don't clamp" attribute).
    if let Some(Resolved { edit, limits, rule }) = clamping.and_then(|c| c.resolved) {
        // SAFETY: `out` is the caller's value buffer, just filled in by the original.
        let value = unsafe { &mut (*out).data };
        // SAFETY: the arguments of this ClampValue call.
        unsafe { stop_drag(property, container, rule, &limits, input, value) };
        limits::adjust(&edit, &limits, rule, value);
    }
    // A drag stopped at the limits carries on from the value kept (`slider_drag`).
    // SAFETY: as above.
    slider_drag::stored(input, unsafe { &*out });
    result
}

unsafe extern "C" fn get_value_range_detour(
    property: *const c_void,
    out: *mut NumericRange,
    container: *const c_void,
) -> *mut NumericRange {
    let _span = timings::span("property_limits::get_value_range_detour");
    let original = *GET_VALUE_RANGE.get().expect("hook recorded before use");
    // SAFETY: forwards the caller's arguments unchanged.
    let result = unsafe {
        timings::game(numeric_property::GET_VALUE_RANGE.name, || {
            original(property, out, container)
        })
    };
    let panel = CLAMPING.with_borrow_mut(|clamping| {
        // Outside of `ClampValue` (the panel asking for the sliders' range), or for another
        // property: leave the range alone.
        let Some(clamping) = clamping.as_mut() else {
            return true;
        };
        if clamping.property != property || clamping.container != container {
            return false;
        }
        // SAFETY: `out` is the caller's range buffer, just filled in by the original; `property`
        // and `container` are the live arguments of the `ClampValue` call in progress.
        unsafe { answer(clamping, &mut *out) };
        false
    });
    if panel {
        // SAFETY: the arguments of this GetValueRange call, and the property's own range for them,
        // just filled in by the original.
        unsafe { super::highlight::on_panel_range(property, container, &*out) };
    }
    result
}

/// Replaces `range` (the property's own) with the limits for the value `clamping` is storing.
///
/// # Safety
/// `clamping` must describe a `ClampValue` call in progress.
unsafe fn answer(clamping: &mut Clamping, range: &mut NumericRange) {
    // SAFETY: per the contract, `property` is a numeric property and `container` its container.
    let edit = unsafe {
        let field = numeric_property::reflected_field(clamping.property);
        let target = (*clamping.container.cast::<Container>()).target();
        Edit::new(field, target, range, clamping.components)
    };
    let limits = match edit {
        Some(edit) => {
            let (limits, rule) = limits::resolve(&edit);
            clamping.resolved = Some(Resolved { edit, limits, rule });
            limits
        }
        None => Limits::unknown(),
    };
    range.min = NumericValue {
        data: limits.min,
        num_used: 3,
    };
    range.max = NumericValue {
        data: limits.max,
        num_used: 3,
    };
}

/// Stops a slider drag at the rule's `drag_min`: raises the components the drag moves that are
/// below it. Those are the ones holding the value the slider sends that this edit changes, so a
/// component the drag doesn't move keeps its value: another axis, or one the panel copied from the
/// first of several edited objects.
///
/// # Safety
/// `property` and `container` must be the arguments of the `ClampValue` call in progress, `input`
/// the value it's storing and `value` the clamped one.
unsafe fn stop_drag(
    property: *const c_void,
    container: *const c_void,
    rule: Option<&Rule>,
    limits: &Limits,
    input: &NumericValue,
    value: &mut [f32; 3],
) {
    let Some(drag_min) = rule.and_then(|rule| rule.drag_min) else {
        return;
    };
    let Some(dragged) = super::slider::dragged_value() else {
        return; // typed, or not from a slider
    };
    // SAFETY: per the contract; ClampValue runs before the value is stored, so this is the one
    // the object holds before the edit.
    let Some(current) = (unsafe { numeric_property::current_value(property, container) }) else {
        return;
    };
    let used = usize::from(input.num_used).min(3);
    let components = value.iter_mut().zip(input.data).zip(current.data);
    for (((component, requested), before), max) in components.zip(limits.max).take(used) {
        let moved = requested == dragged && requested != before;
        if moved && *component < drag_min {
            *component = drag_min.min(max);
        }
    }
}

fn all_finite(value: &NumericValue) -> bool {
    value
        .data
        .iter()
        .take(value.num_used.into())
        .all(|c| c.is_finite())
}

/// Logs once if a property class has its own `GetValueRange`: values of such properties would
/// still be clamped, and that override would need hooking too.
fn warn_if_range_overridden(property: *const c_void) {
    static WARNED: AtomicBool = AtomicBool::new(false);
    let Some(&hooked) = GET_VALUE_RANGE_TARGET.get() else {
        return;
    };
    // SAFETY: `property` is a live property object; its first field is its vtable pointer, and
    // its vtable has at least GET_VALUE_RANGE_SLOT + 1 entries (ClampValue calls that slot).
    let actual = unsafe {
        let vtable = *(property as *const *const usize);
        *vtable.add(numeric_property::GET_VALUE_RANGE_SLOT)
    };
    if actual != hooked && !WARNED.swap(true, Ordering::Relaxed) {
        log!(
            "a property class overrides GetValueRange (0x{actual:X}); its values keep the game's range"
        );
    }
}
