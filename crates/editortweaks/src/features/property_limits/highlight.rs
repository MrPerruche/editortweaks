//! Marks slider values outside of the game's range with the game's own warning color (`Highlight`,
//! as its menus mark warnings, see `game::ui_style`), and those outside of the rules too with
//! `Negative`: e.g. values edited into a saved vehicle, kept until edited (see `rules`). With
//! property limits inactive, the game's range is the rules' range, so values outside of it get
//! `Negative`. A slider back in range gets its own style back.
//!
//! The game's range is the slider's (`MinValue`/`MaxValue`): the panel shows each property's own
//! range on its sliders (`property`). The rules need the property and the edited object, which only
//! the row knows: a numeric row fills its sliders in `UNumericPropertyWidget::UpdateValue`
//! (`game::property_widget`), whenever the panel shows or refreshes the property. While it runs, it
//! asks the property for its range: [`on_panel_range`] (from `property`'s `GetValueRange` hook)
//! checks the value against the rules then. A slider shows the first edited object's value, so that
//! one is checked.
//!
//! The panel doesn't refresh during a slider drag (or arrow keys): [`on_slider_moved`] (from
//! `slider`) restyles the moved slider itself. Moved values go through the rules, so they're only
//! outside of the game's range.

use core::ffi::c_void;
use std::cell::{Cell, RefCell};
use std::sync::OnceLock;

use super::limits::{self, Edit};
use crate::framework::hook;
use crate::game::numeric_property::{self, Container, NumericRange};
use crate::game::property_widget::{self, PropertyWidgets, UpdateValueFn};
use crate::game::slider::{self, RangeFields, ValueFields};
use crate::game::ui_style::{self, ButtonColorStyle, ColorStyleValues};
use crate::game::weak_object::WeakObject;
use crate::prelude::*;

static UPDATE_VALUE: OnceLock<UpdateValueFn> = OnceLock::new();
/// Set once reflection is available; until then (or if it fails) sliders keep their colors.
static SLIDERS: OnceLock<Sliders> = OnceLock::new();

/// Values this close to the slider's range (relative to the bound, at least 1e-4) count as in it:
/// distances in brick units snap to a grid the range isn't rounded to.
const TOLERANCE: f32 = 1.0e-4;

struct Sliders {
    widgets: PropertyWidgets,
    range: RangeFields,
    value: ValueFields,
    style: ButtonColorStyle,
    styles: ColorStyleValues,
}

/// A numeric row's `UpdateValue` in progress.
#[derive(Clone, Copy, Default)]
struct RowUpdate {
    /// Set once the value was checked against the rules.
    checked: bool,
    /// Per component: outside of the rules.
    outside_rules: [bool; 3],
}

/// A slider restyled here, and the style it had before.
struct Restyled {
    slider: WeakObject,
    original: u8,
}

thread_local! {
    /// The numeric row filling its sliders on this thread, if any.
    static ROW_UPDATE: Cell<Option<RowUpdate>> = const { Cell::new(None) };
    /// Sliders currently showing a warning style.
    static RESTYLED: RefCell<Vec<Restyled>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn resolve() -> Result<*const u8, String> {
    property_widget::UPDATE_VALUE.resolve()
}

/// # Safety
/// `target` must be `UpdateValue`, resolved by [`resolve`].
pub(super) unsafe fn install(target: *const u8) -> Result<(), String> {
    // SAFETY: `update_value` matches `UpdateValueFn`, the hooked function's signature.
    unsafe {
        hook::install_typed(
            property_widget::UPDATE_VALUE.name,
            target,
            update_value as UpdateValueFn,
            &UPDATE_VALUE,
        )
    }
}

pub(super) fn on_engine_init() {
    if UPDATE_VALUE.get().is_none() {
        return; // not hooked (pattern missing), already logged
    }
    let sliders = || -> Result<Sliders, String> {
        Ok(Sliders {
            widgets: property_widget::property_widgets()?,
            range: slider::range_fields()?,
            value: slider::value_fields()?,
            style: ui_style::button_color_style()?,
            styles: ui_style::color_style_values()?,
        })
    };
    match sliders() {
        Ok(sliders) => {
            log!("values outside of the game's range are highlighted");
            let _ = SLIDERS.set(sliders);
        }
        Err(reason) => log!("values outside of the game's range aren't highlighted: {reason}"),
    }
}

/// Called with the range the property answered, when the panel asks for it outside of storing a
/// value: during a row's `UpdateValue`, checks the value against the rules.
///
/// # Safety
/// `property` must be a live numeric property, `container` a live container for it, and `vanilla`
/// the property's own range for it.
pub(super) unsafe fn on_panel_range(
    property: *const c_void,
    container: *const c_void,
    vanilla: &NumericRange,
) {
    let Some(mut row) = ROW_UPDATE.get() else {
        return;
    };
    if row.checked {
        return;
    }
    row.checked = true;
    // SAFETY: per the contract.
    row.outside_rules = unsafe { outside_rules(property, container, vanilla) };
    ROW_UPDATE.set(Some(row));
}

/// Restyles a slider whose value a drag or an arrow key just set.
///
/// # Safety
/// `slider` must be a live `UBrickSliderWidget`. Game thread only.
pub(super) unsafe fn on_slider_moved(slider: *mut c_void) {
    if let Some(sliders) = SLIDERS.get() {
        // SAFETY: per the contract.
        unsafe { sliders.restyle(slider, !super::slider::active()) };
    }
}

unsafe extern "C" fn update_value(this: *mut c_void, event: *const c_void) {
    let original = *UPDATE_VALUE.get().expect("hook recorded before use");
    let previous = ROW_UPDATE.replace(Some(RowUpdate::default()));
    // SAFETY: forwards the caller's arguments unchanged.
    unsafe { original(this, event) };
    let row = ROW_UPDATE.replace(previous).unwrap_or_default();

    let Some(sliders) = SLIDERS.get() else {
        return;
    };
    // Without our limits, the game's range is the only one.
    let rules_apply = super::slider::active();
    // SAFETY: UpdateValue is a UNumericPropertyWidget member, so `this` is one; its sliders are
    // live UBrickSliderWidgets. UI code only runs on the game thread.
    unsafe {
        let row_sliders = sliders.widgets.sliders(this.cast()).to_vec();
        for (index, slider) in row_sliders.into_iter().enumerate() {
            // Slider `index` shows component `index` (all of them alike when the axes are locked).
            let outside_rules = !rules_apply || (row.checked && row.outside_rules[index.min(2)]);
            sliders.restyle(slider.cast_mut(), outside_rules);
        }
    }
}

/// Per component of the value `property` holds for `container`: whether the rules would change it.
///
/// # Safety
/// As for [`on_panel_range`].
unsafe fn outside_rules(
    property: *const c_void,
    container: *const c_void,
    vanilla: &NumericRange,
) -> [bool; 3] {
    // SAFETY: per the contract.
    let Some(value) = (unsafe { numeric_property::current_value(property, container) }) else {
        return [false; 3];
    };
    let used = usize::from(value.num_used).min(3);
    // SAFETY: per the contract; the target lives as long as the container.
    let edit = unsafe {
        let field = numeric_property::reflected_field(property);
        let target = (*container.cast::<Container>()).target();
        Edit::new(field, target, vanilla, used)
    };
    let Some(edit) = edit else {
        return [false; 3];
    };
    let allowed = limits::limited(&edit, value.data);
    core::array::from_fn(|i| i < used && value.data[i].is_finite() && allowed[i] != value.data[i])
}

impl Sliders {
    /// Gives `slider` the warning style its value calls for, or its own style back.
    ///
    /// # Safety
    /// `slider` must be a live `UBrickSliderWidget`. Game thread only.
    unsafe fn restyle(&self, slider: *mut c_void, outside_rules: bool) {
        // SAFETY: per the contract.
        let (value, (min, max)) = unsafe { (self.value.value(slider), self.range.get(slider)) };
        let warning = outside(value, min, max).then_some(if outside_rules {
            self.styles.negative
        } else {
            self.styles.highlight
        });

        // Decide while borrowing the list, restyle after: restyling runs game code.
        let style = RESTYLED.with_borrow_mut(|restyled| {
            restyled.retain(|entry| entry.slider.get().is_some());
            let entry = restyled
                .iter()
                .position(|entry| entry.slider.is(slider.cast()));
            match (warning, entry) {
                (Some(style), Some(_)) => Some(style),
                (Some(style), None) => {
                    // SAFETY: per the contract.
                    let original = unsafe { self.style.get(slider) };
                    restyled.push(Restyled {
                        // SAFETY: per the contract.
                        slider: unsafe { WeakObject::new(slider.cast()) },
                        original,
                    });
                    Some(style)
                }
                (None, Some(index)) => Some(restyled.swap_remove(index).original),
                (None, None) => None,
            }
        });
        if let Some(style) = style {
            // SAFETY: per the contract; sliders are buttons (UButtonWidgetBase).
            unsafe { self.style.set(slider, style) };
        }
    }
}

/// Whether `value` is outside of `min..=max` (beyond [`TOLERANCE`]).
fn outside(value: f32, min: f32, max: f32) -> bool {
    let margin = |bound: f32| TOLERANCE * bound.abs().max(1.0);
    value.is_finite() && (value < min - margin(min) || value > max + margin(max))
}
