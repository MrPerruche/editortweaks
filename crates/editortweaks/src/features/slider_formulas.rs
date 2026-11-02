//! `x` and `X` in typed slider values: `x*2`, `x+0.5`, `(x+X)/2`, `2x` (= `2*x`), ...
//!
//! - `X` is the value the slider shows (with several objects selected: the first one's).
//! - `x` is each edited object's own value: `x*2` typed into a brick size's X doubles every
//!   selected brick's X, and their other axes stay as they are. With one object, `x` = `X`.
//! - Both are in the shown unit (50 for 50 %), unlike the game's own `+=`, `*=`, ... operators,
//!   which use the stored value (see `game::slider_text`).
//!
//! How: while a slider handles typed text (`OnTextChanged`, hooked through reflection), its parser
//! (`FromString`) gets the text with `x` and `X` replaced by the slider's value. If the text uses
//! `x`, the value then stored into each object is evaluated again with that object's value
//! ([`per_object_value`], called by `property_limits` when values are stored).
//!
//! The game also removes letters from the text box while typing (`game::slider`), so its filter
//! is skipped: any text can be typed, and what isn't a number or formula is still rejected when
//! committed, as before.

use core::ffi::c_void;
use std::cell::RefCell;
use std::sync::OnceLock;

use crate::framework::{hook, timings};
use crate::game::numeric_property::{self, NumericValue};
use crate::game::property_widget::{self, PropertyWidgets, SliderComponent};
use crate::game::reflection::NativeThunk;
use crate::game::slider::{self, TextBoxes, TextChanged, ValueFields};
use crate::game::slider_text::{self, FString, FromStringFn, OptionalF32, OwnedFString};
use crate::game::ui_library::{self, UnitConversion};
use crate::prelude::*;

pub struct SliderFormulas;

static FROM_STRING: OnceLock<FromStringFn> = OnceLock::new();
static ON_TEXT_CHANGED: OnceLock<NativeThunk> = OnceLock::new();
/// Set once reflection is available; until then (or if it fails) typed text stays as it is.
static SLIDERS: OnceLock<Sliders> = OnceLock::new();

struct Sliders {
    fields: ValueFields,
    units: UnitConversion,
    text_changed: TextChanged,
    text_boxes: TextBoxes,
    /// To find which component a slider edits; without it, `x` = `X`.
    widgets: Option<PropertyWidgets>,
}

thread_local! {
    /// The slider handling typed text on this thread, if any.
    static TYPING: RefCell<Option<Typing>> = const { RefCell::new(None) };
}

struct Typing {
    slider: *mut c_void,
    per_object: Option<PerObject>,
}

/// A typed formula using `x`, to evaluate again for each edited object.
#[derive(Clone)]
struct PerObject {
    /// The text as typed.
    formula: String,
    /// The slider's value (`X`), written out.
    shown: String,
    factor: f32,
    component: SliderComponent,
    /// The parser's object (`FromString`'s `this`).
    interface: *const c_void,
    /// The numeric property being written: the first one stored while typing.
    property: Option<*const c_void>,
}

impl Feature for SliderFormulas {
    fn id(&self) -> &'static str {
        "slider_formulas"
    }

    fn on_load(&self) -> LoadResult {
        let target = slider_text::FROM_STRING.resolve()?;
        // SAFETY: `target` is the start of FromString, and `from_string` has its signature.
        unsafe {
            hook::install_typed(
                slider_text::FROM_STRING.name,
                target,
                from_string as FromStringFn,
                &FROM_STRING,
            )
        }
    }

    fn on_engine_init(&self) {
        match engine_init() {
            Ok(()) => log!("active"),
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

/// The `OnTextChanged` thunk can only be found through reflection, so it's hooked here.
fn engine_init() -> Result<(), String> {
    let widgets = property_widget::property_widgets()
        .inspect_err(|reason| log!("x is the slider's value for every object: {reason}"))
        .ok();
    let sliders = Sliders {
        fields: slider::value_fields()?,
        units: ui_library::unit_conversion()?,
        text_changed: slider::on_text_changed()?,
        text_boxes: slider::text_boxes()?,
        widgets,
    };
    let target = sliders.text_changed.thunk;
    let _ = SLIDERS.set(sliders);
    // SAFETY: `target` is OnTextChanged's exec thunk, and `on_text_changed` is a NativeThunk.
    unsafe {
        hook::install_typed(
            "UBrickSliderWidget::execOnTextChanged",
            target,
            on_text_changed as NativeThunk,
            &ON_TEXT_CHANGED,
        )
    }
}

unsafe extern "C" fn on_text_changed(
    context: *mut c_void,
    stack: *mut c_void,
    result: *mut c_void,
) {
    let _span = timings::span("slider_formulas::on_text_changed");
    let original = *ON_TEXT_CHANGED.get().expect("hook recorded before use");
    if let Some(sliders) = SLIDERS.get() {
        // SAFETY: the thunk's own arguments: `context` is the slider, `stack` the call's frame.
        let keystroke = unsafe {
            sliders.text_changed.event_type(stack) == Some(slider::INTERACTIVE)
                && sliders.text_boxes.is_typing(context)
        };
        if keystroke {
            // All the game does now is remove disallowed characters: keep the text as typed.
            return;
        }
    }
    let previous = TYPING.replace(Some(Typing {
        slider: context,
        per_object: None,
    }));
    // SAFETY: forwards the caller's arguments unchanged.
    unsafe {
        timings::game("UBrickSliderWidget::execOnTextChanged", || {
            original(context, stack, result)
        })
    };
    TYPING.replace(previous);
}

unsafe extern "C" fn from_string(
    interface: *const c_void,
    out: *mut OptionalF32,
    text: *const FString,
    existing: *const f32,
) -> *mut OptionalF32 {
    let _span = timings::span("slider_formulas::from_string");
    let original = *FROM_STRING.get().expect("hook recorded before use");
    // SAFETY: forwards the caller's arguments, with only the text possibly replaced.
    let call = |text: *const FString| unsafe {
        timings::game(slider_text::FROM_STRING.name, || {
            original(interface, out, text, existing)
        })
    };

    let slider = TYPING.with_borrow(|typing| typing.as_ref().map(|typing| typing.slider));
    let (Some(slider), Some(sliders)) = (slider, SLIDERS.get()) else {
        return call(text);
    };
    // SAFETY: `text` is the caller's string; `slider` is the UBrickSliderWidget whose
    // OnTextChanged is running.
    let (typed, stored, value_type) = unsafe {
        (
            (*text).to_string(),
            sliders.fields.value(slider),
            sliders.fields.value_type(slider),
        )
    };
    if !typed.contains(['x', 'X']) || value_type == slider::TIME_OF_DAY {
        return call(text);
    }

    let factor = sliders.units.factor(value_type);
    let shown = slider_text::formula_number(stored, factor);
    let substituted = substitute(&typed, &shown, &shown);
    log!("'{typed}' = {substituted}");
    let native = OwnedFString::new(&substituted);
    let result = call(native.as_ptr());

    if typed.contains('x') {
        // Outside of property panels (e.g. settings), there's one value: `x` = `X`.
        let widgets = sliders.widgets.as_ref();
        let component = widgets.and_then(|widgets| widgets.slider_component(slider));
        if let Some(component) = component {
            let per_object = PerObject {
                formula: typed,
                shown,
                factor,
                component,
                interface,
                property: None,
            };
            TYPING.with_borrow_mut(|typing| {
                if let Some(typing) = typing {
                    typing.per_object = Some(per_object);
                }
            });
        }
    }
    result
}

/// The value to store into `container`'s object instead of `input` (the same value for every
/// edited object) while a formula using `x` is committed: the object's own value, with the
/// slider's component (all of them when the axes are locked) evaluated with the object's value.
/// `None`: store `input`. Called by `property_limits` for every numeric value stored.
///
/// # Safety
/// `property` and `container` must be the arguments of a `ClampValue` call in progress.
pub(crate) unsafe fn per_object_value(
    property: *const c_void,
    container: *const c_void,
    input: &NumericValue,
) -> Option<NumericValue> {
    let per_object = TYPING.with_borrow_mut(|typing| {
        let per_object = typing.as_mut()?.per_object.as_mut()?;
        (*per_object.property.get_or_insert(property) == property).then(|| per_object.clone())
    })?;
    // SAFETY: per the contract.
    let current = unsafe { numeric_property::current_value(property, container) }?;
    let used = usize::from(input.num_used).min(3);
    if current.num_used != input.num_used || per_object.component.index >= used {
        return None;
    }
    let index = per_object.component.index;
    let edited = if per_object.component.axes_locked {
        0..used
    } else {
        index..index + 1
    };

    let mut value = current;
    for i in edited {
        // An invalid result for this object (e.g. `1/x` with x = 0) leaves its value unchanged.
        if let Some(result) = per_object.evaluate(current.data[i]) {
            value.data[i] = result;
        }
    }
    Some(value)
}

impl PerObject {
    /// The stored value the formula gives with `x` = `stored`, like `OnTextChanged` computes it.
    fn evaluate(&self, stored: f32) -> Option<f32> {
        let original = *FROM_STRING.get()?;
        let x = slider_text::formula_number(stored, self.factor);
        let native = OwnedFString::new(&substitute(&self.formula, &x, &self.shown));
        let mut out = OptionalF32 {
            value: 0.0,
            is_set: false,
        };
        // SAFETY: the original parser with its own object, as OnTextChanged calls it; it only
        // reads the string and the existing value.
        unsafe {
            timings::game(slider_text::FROM_STRING.name, || {
                original(self.interface, &mut out, native.as_ptr(), &stored)
            })
        };
        (out.is_set && !out.value.is_nan()).then(|| out.value / self.factor)
    }
}

/// `text` with `x` replaced by `x_value` and `X` by `big_x_value`, each in parentheses, with `*`
/// added where a number or parenthesis touches them: `2x+X(1+x)` -> `2*(a)+(b)*(1+(a))`.
fn substitute(text: &str, x_value: &str, big_x_value: &str) -> String {
    let operand_end = |c: char| c.is_ascii_digit() || c == '.' || c == ')';
    let operand_start = |c: char| c.is_ascii_digit() || c == '.' || c == '(';
    let mut out = String::with_capacity(text.len() + 4 * x_value.len().max(big_x_value.len()));
    for (i, c) in text.char_indices() {
        let value = match c {
            'x' => x_value,
            'X' => big_x_value,
            _ => {
                out.push(c);
                continue;
            }
        };
        if out.trim_end().ends_with(operand_end) {
            out.push('*');
        }
        out.push('(');
        out.push_str(value);
        out.push(')');
        let next = text[i + c.len_utf8()..].trim_start().chars().next();
        if next.is_some_and(operand_start) {
            out.push('*');
        }
    }
    out
}
