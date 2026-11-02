//! Sliders show their value exactly while you edit it, and show when their label hides digits.
//!
//! - Editing (typing in a slider's text box): the game shows the value with at most 3 decimals and
//!   with digit grouping ("1,234.568"), which loses precision, and grouped text can't even be typed
//!   back. This shows the shortest text that reads back as the stored value, ungrouped
//!   (`game::slider_text::ExactText`).
//! - Otherwise the label keeps the game's decimals and rounding, but keeps trailing zeros when the
//!   value has more digits than shown: 90.10004 with 3 decimals shows "90.100", 90.1 stays "90.1".
//!
//! How: while `UpdateSliderValue` builds a slider's text (`game::slider`), the number formatting
//! (`FText::AsNumber`, `game::text`) gets adjusted options. `NumberToText` (`game::ui_library`)
//! builds the label; a number formatted outside of it is the edit text.

use core::ffi::c_void;
use std::cell::Cell;
use std::sync::OnceLock;

use crate::framework::{hook, timings};
use crate::game::slider::{self, UpdateSliderValueFn, ValueFields};
use crate::game::slider_text::ExactText;
use crate::game::text::{self, AsNumberFn, NumberFormattingOptions};
use crate::game::ui_library::{self, NumberToTextFn, UnitConversion};
use crate::prelude::*;

pub struct PreciseSliders;

static UPDATE_SLIDER_VALUE: OnceLock<UpdateSliderValueFn> = OnceLock::new();
static NUMBER_TO_TEXT: OnceLock<NumberToTextFn> = OnceLock::new();
static AS_NUMBER: OnceLock<AsNumberFn> = OnceLock::new();
/// Set once reflection is available; until then (or if it fails) texts stay as the game makes them.
static SLIDERS: OnceLock<Sliders> = OnceLock::new();

struct Sliders {
    fields: ValueFields,
    units: UnitConversion,
}

thread_local! {
    /// The slider whose text is being built on this thread, if any.
    static SHOWING: Cell<Option<Showing>> = const { Cell::new(None) };
}

#[derive(Clone, Copy)]
struct Showing {
    slider: *mut c_void,
    /// Inside `NumberToText`: building the label, not the edit text.
    label: bool,
}

impl Feature for PreciseSliders {
    fn id(&self) -> &'static str {
        "precise_sliders"
    }

    fn on_load(&self) -> LoadResult {
        let update = slider::UPDATE_SLIDER_VALUE.resolve()?;
        let number_to_text = ui_library::NUMBER_TO_TEXT.resolve()?;
        let as_number = text::AS_NUMBER_FLOAT.resolve()?;
        // SAFETY: each target is the start of the function its pattern identifies, and each
        // detour has exactly the signature declared next to that pattern.
        unsafe {
            hook::install_typed(
                slider::UPDATE_SLIDER_VALUE.name,
                update,
                update_slider_value as UpdateSliderValueFn,
                &UPDATE_SLIDER_VALUE,
            )?;
            hook::install_typed(
                ui_library::NUMBER_TO_TEXT.name,
                number_to_text,
                number_to_text_detour as NumberToTextFn,
                &NUMBER_TO_TEXT,
            )?;
            hook::install_typed(
                text::AS_NUMBER_FLOAT.name,
                as_number,
                as_number_detour as AsNumberFn,
                &AS_NUMBER,
            )
        }
    }

    fn on_engine_init(&self) {
        let sliders = slider::value_fields().and_then(|fields| {
            Ok(Sliders {
                fields,
                units: ui_library::unit_conversion()?,
            })
        });
        match sliders {
            Ok(sliders) => {
                log!("active");
                let _ = SLIDERS.set(sliders);
            }
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

unsafe extern "C" fn update_slider_value(
    this: *mut c_void,
    use_given_focus: usize,
    focused: usize,
) {
    let _span = timings::span("precise_sliders::update_slider_value");
    let original = *UPDATE_SLIDER_VALUE.get().expect("hook recorded before use");
    let previous = SHOWING.replace(Some(Showing {
        slider: this,
        label: false,
    }));
    // SAFETY: forwards the caller's arguments unchanged.
    unsafe {
        timings::game(slider::UPDATE_SLIDER_VALUE.name, || {
            original(this, use_given_focus, focused)
        })
    };
    SHOWING.set(previous);
}

unsafe extern "C" fn number_to_text_detour(
    out: *mut c_void,
    value: f32,
    value_type: usize,
    max_fractional_digits: usize,
    show_unit: usize,
    always_sign: usize,
    zero_as_unlimited: usize,
) -> *mut c_void {
    let _span = timings::span("precise_sliders::number_to_text_detour");
    let original = *NUMBER_TO_TEXT.get().expect("hook recorded before use");
    let showing = SHOWING.get();
    if let Some(showing) = showing {
        SHOWING.set(Some(Showing {
            label: true,
            ..showing
        }));
    }
    // SAFETY: forwards the caller's arguments unchanged.
    let result = unsafe {
        timings::game(ui_library::NUMBER_TO_TEXT.name, || {
            original(
                out,
                value,
                value_type,
                max_fractional_digits,
                show_unit,
                always_sign,
                zero_as_unlimited,
            )
        })
    };
    SHOWING.set(showing);
    result
}

unsafe extern "C" fn as_number_detour(
    out: *mut c_void,
    value: f32,
    options: *const NumberFormattingOptions,
    culture: *const c_void,
) -> *mut c_void {
    let _span = timings::span("precise_sliders::as_number_detour");
    let original = *AS_NUMBER.get().expect("hook recorded before use");
    // SAFETY: forwards the caller's arguments, with only the value and options possibly replaced.
    let call = |value: f32, options: *const NumberFormattingOptions| unsafe {
        timings::game(text::AS_NUMBER_FLOAT.name, || {
            original(out, value, options, culture)
        })
    };

    let (Some(showing), Some(sliders)) = (SHOWING.get(), SLIDERS.get()) else {
        return call(value, options);
    };
    if options.is_null() {
        return call(value, options);
    }
    // SAFETY: `showing.slider` is the UBrickSliderWidget whose UpdateSliderValue is running, and
    // `options` the caller's options.
    let (stored, factor, mut adjusted) = unsafe {
        let value_type = sliders.fields.value_type(showing.slider);
        (
            sliders.fields.value(showing.slider),
            sliders.units.factor(value_type),
            *options,
        )
    };
    // Only the slider's own number, which the game computes exactly like this (not e.g. a part of
    // a brick-unit fraction, or a value it rounded itself).
    if value != stored * factor {
        return call(value, options);
    }

    if showing.label {
        // Keep the game's decimals and rounding; if digits are hidden, keep the trailing zeros
        // (with 0 decimals there are none to keep).
        let decimals = adjusted.maximum_fractional_digits;
        let hidden = decimals > 0
            && ExactText::find(stored, factor)
                .is_none_or(|exact| exact.digits > decimals.unsigned_abs() as usize);
        if !hidden {
            return call(value, options);
        }
        adjusted.minimum_fractional_digits = adjusted.minimum_fractional_digits.max(decimals);
        call(value, &adjusted)
    } else {
        // Typing the text back must give the same value: all digits needed, no grouping.
        let Some(exact) = ExactText::find(stored, factor) else {
            return call(value, options);
        };
        adjusted.minimum_fractional_digits = 0;
        adjusted.maximum_fractional_digits = exact.digits as i32;
        adjusted.use_grouping = false;
        call(exact.shown, &adjusted)
    }
}
