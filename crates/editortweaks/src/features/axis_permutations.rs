//! A button on rows of two or three values (a brick's size, position, rotation, ...), before the
//! lock: each click puts the values in their next order, and clicking on goes through every order
//! and back to the first (3 values: 6 orders, 2 values: swapped).
//!
//! The values have no identity: which value went where isn't kept. The next order comes from
//! comparing the values alone, as the next permutation in lexicographic order ([`next_order`]):
//! from the right, the first value smaller than the one after it is swapped with the last value
//! larger than it, and the values after it are reversed; once they're in decreasing order, the
//! increasing one comes next. Equal values give fewer orders (1, 1, 2: three) and are never
//! swapped with each other; all equal, a click changes nothing. With several objects edited, the
//! order is the row's (the first object's), and every object's values move alike: e.g. Y and Z
//! swapped on each.
//!
//! How: a row's Blueprint adds its lock button to the row's button panel when the row updates
//! (`UpdateNumericProperty`, hooked, see `game::property_widget`); ours goes in just before it. A
//! click (`menu_buttons`) sets the first component through the row, as its slider does
//! (`OnSliderValueChanged`, one step to undo, every edited object), and while the row writes, the
//! value stored into each object becomes that object's own values in the new order
//! ([`per_object_value`]: from `property_limits`' `ClampValue` hook for the game's properties,
//! from our rows' setter for ours). The values still go through every limit, as typed ones do.
//! Rows of ours whose values aren't alike (`compact_panel`'s time and atmosphere) get neither our
//! button nor the game's lock.

use core::ffi::c_void;
use std::cell::RefCell;
use std::sync::{Mutex, MutexGuard, Once, OnceLock, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;

use super::{menu_buttons, property_limits};
use crate::framework::hook;
use crate::framework::timings;
use crate::game::kismet;
use crate::game::menu::ButtonPanels;
use crate::game::numeric_property::{self, NumericValue};
use crate::game::property_reflection;
use crate::game::property_widget::{self, PropertyWidgets, RowParts, UpdateNumericPropertyFn};
use crate::game::reflection;
use crate::game::slider::{self, ValueFields};
use crate::game::text::Text;
use crate::game::ui_style::{self, SetColorStyle};
use crate::game::weak_object::WeakObject;
use crate::prelude::*;

pub struct AxisPermutations;

/// The button's icon (the units button has `T_SwitchMode`).
const ICON: &str = "/Game/BrickRigs/UI/Textures/Icons/T_Retry.T_Retry";

/// How many values a row may have for the button: as many as get a lock.
const COMPONENTS: core::ops::RangeInclusive<usize> = 2..=3;

/// The game code this feature uses (after engine init).
struct Game {
    panels: ButtonPanels,
    rows: RowParts,
    widgets: PropertyWidgets,
    values: ValueFields,
    /// `UMenuButtonWidget::SetColorStyle`, and the `Default` style: a reused button widget may still
    /// have the lock's style.
    set_style: SetColorStyle,
    default_style: u8,
    /// No text, like the lock's.
    text: Text,
    icon: FName,
}

// SAFETY: reflection records, names and a text kept for the whole game; only used on the game
// thread.
unsafe impl Send for Game {}
unsafe impl Sync for Game {}

static GAME: OnceLock<Game> = OnceLock::new();
static ORIGINAL: OnceLock<UpdateNumericPropertyFn> = OnceLock::new();

/// Our buttons, with the row each was made for, until that row updates again (it then makes a new
/// one, maybe reusing the same widget). Game thread only; never locked while calling the game.
static SHOWN: Mutex<Vec<Shown>> = Mutex::new(Vec::new());

struct Shown {
    button: WeakObject,
    row: WeakObject,
}

// SAFETY: weak object references, only used on the game thread.
unsafe impl Send for Shown {}

fn shown() -> MutexGuard<'static, Vec<Shown>> {
    SHOWN.lock().unwrap_or_else(PoisonError::into_inner)
}

thread_local! {
    /// The values being put in their next order on this thread, if any.
    static WRITING: RefCell<Option<Writing>> = const { RefCell::new(None) };
}

/// A row writing values in their next order.
struct Writing {
    /// Component `i` takes the value of component `order[i]`.
    order: Vec<usize>,
    /// The numeric property being written: the first one stored while writing.
    property: Option<*const c_void>,
}

impl Feature for AxisPermutations {
    fn id(&self) -> &'static str {
        "axis_permutations"
    }

    fn on_engine_init(&self) {
        match start() {
            Ok(()) => log!("active: rows of two or three values get a value order button"),
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

/// Finds the rows' parts and hooks them. After engine init (reflection, and the event's name).
fn start() -> Result<(), String> {
    // Without it, a click would write the first component only to the game's properties.
    if !property_limits::rewrites_stored_values() {
        return Err("property limits are inactive, so stored values can't be rewritten".into());
    }
    let button =
        reflection::find_class("MenuButtonWidget").ok_or("class MenuButtonWidget not found")?;
    let game = Game {
        panels: ButtonPanels::find()?,
        rows: property_widget::row_parts()?,
        widgets: property_widget::property_widgets()?,
        values: slider::value_fields()?,
        set_style: SetColorStyle::find(button)?,
        default_style: ui_style::color_style_values()?.default,
        text: kismet::string_to_text("")?,
        icon: kismet::string_to_name(ICON)?,
    };
    let target = property_widget::update_numeric_property()?;
    let _ = GAME.set(game);
    menu_buttons::listen(clicked)?;
    property_reflection::rewrite_written_numbers(per_object_value);
    // SAFETY: `target` is `UpdateNumericProperty` (checked by its shape and the event's name), and
    // the detour has its signature.
    unsafe {
        hook::install_typed(
            property_widget::UPDATE_NUMERIC_PROPERTY.name,
            target,
            update_numeric_property as UpdateNumericPropertyFn,
            &ORIGINAL,
        )
    }
}

/// A numeric row updates; its Blueprint is about to add its buttons. A row of ours whose values
/// aren't alike (a time and a percentage, `Number::alike`) gets no buttons for several values: the
/// Blueprint is told it has one, which only decides the lock.
unsafe extern "C" fn update_numeric_property(row: *mut c_void, value_type: u8, num_axes: i32) {
    let _span = timings::span("axis_permutations::update_numeric_property");
    let original = *ORIGINAL.get().expect("hook recorded before use");
    let unalike = property_reflection::number_shown().is_some_and(|number| !number.alike());
    let num_axes = if unalike { 1 } else { num_axes };
    if let Some(game) = GAME.get() {
        // SAFETY: the row this member function runs for, so alive; UI code runs on the game
        // thread.
        unsafe { game.row_updating(row.cast(), num_axes) };
    }
    // SAFETY: forwards the caller's arguments.
    unsafe {
        timings::game(property_widget::UPDATE_NUMERIC_PROPERTY.name, || {
            original(row, value_type, num_axes)
        })
    };
}

/// A menu button was clicked (`menu_buttons`): ours puts its row's values in their next order.
fn clicked(button: *mut UObjectBase) {
    let Some(game) = GAME.get() else {
        return;
    };
    let row = shown()
        .iter()
        .find(|shown| shown.button.is(button))
        .and_then(|shown| shown.row.get_live());
    let Some(row) = row else {
        return;
    };
    // SAFETY: a live row and a live button, on the game thread (button clicks are UI code).
    unsafe {
        // The widget may have been reused for another button since (by this panel or another).
        let ours = game
            .rows
            .button_panel(row)
            .is_some_and(|panel| game.panels.shows(panel, button));
        if ours {
            game.next_order(row);
        }
    }
}

impl Game {
    /// `row` updates, with `num_axes` values: it gets our button (again), first in its panel.
    ///
    /// # Safety
    /// `row` must be a live `UNumericPropertyWidget`. Game thread only.
    unsafe fn row_updating(&self, row: *mut UObjectBase, num_axes: i32) {
        shown().retain(|shown| {
            !shown.row.is(row) && shown.row.get().is_some() && shown.button.get().is_some()
        });
        let components = usize::try_from(num_axes).unwrap_or(0);
        if !COMPONENTS.contains(&components) {
            return;
        }
        // SAFETY: per the contract; the panel is live, and so is the button it makes.
        unsafe {
            let Some(panel) = self.rows.button_panel(row) else {
                return;
            };
            let Some(button) = self.panels.add_button(panel, &self.text, self.icon) else {
                static NO_BUTTON: Once = Once::new();
                NO_BUTTON.call_once(|| log!("a row's panel made no button"));
                return;
            };
            self.set_style.call(button.cast(), self.default_style);
            let mut shown = shown();
            shown.retain(|shown| !shown.button.is(button));
            shown.push(Shown {
                button: WeakObject::new(button),
                row: WeakObject::new(row),
            });
        }
    }

    /// Puts the values `row` edits in their next order, for every edited object.
    ///
    /// # Safety
    /// `row` must be a live `UNumericPropertyWidget`. Game thread only.
    unsafe fn next_order(&self, row: *mut UObjectBase) {
        // SAFETY: per the contract; a row's sliders are live `UBrickSliderWidget`s, showing the
        // first edited object's values.
        let values: Vec<f32> = unsafe {
            self.widgets
                .sliders(row)
                .iter()
                .map(|&slider| self.values.value(slider))
                .collect()
        };
        if !COMPONENTS.contains(&values.len()) {
            return;
        }
        let Some(order) = next_order(&values) else {
            return; // all the same: one order only
        };
        let first = values[order[0]];
        let previous = WRITING.replace(Some(Writing {
            order,
            property: None,
        }));
        // SAFETY: per the contract; the row has a first slider.
        unsafe { self.rows.set_component(row, 0, first, slider::COMMITTED) };
        WRITING.replace(previous);
    }
}

/// The value to store into `container`'s object instead of `written` (the same value for every
/// edited object) while a row puts its values in their next order: the object's own values in that
/// order. `None`: store `written`. Called by `property_limits` for every numeric value stored, and
/// by our rows' setter.
///
/// # Safety
/// `property` and `container` must be the arguments of a `ClampValue` call in progress, or of our
/// rows' setter.
pub(crate) unsafe fn per_object_value(
    property: *const c_void,
    container: *const c_void,
    written: &NumericValue,
) -> Option<NumericValue> {
    let order = WRITING.with_borrow_mut(|writing| {
        let writing = writing.as_mut()?;
        (*writing.property.get_or_insert(property) == property).then(|| writing.order.clone())
    })?;
    // SAFETY: per the contract.
    let current = unsafe { numeric_property::current_value(property, container) }?;
    if current.num_used != written.num_used || usize::from(current.num_used) != order.len() {
        return None;
    }
    let mut value = current;
    for (component, &from) in value.data.iter_mut().zip(&order) {
        *component = current.data[from];
    }
    Some(value)
}

/// The next order of `values` (see the module docs), as where each component takes its value from:
/// component `i` takes the value of component `order[i]`. Components keeping their value keep
/// their own, and equal values keep their order. `None` if the values have one order only (all
/// equal, or not comparable).
fn next_order(values: &[f32]) -> Option<Vec<usize>> {
    if values.iter().any(|value| value.is_nan()) {
        return None;
    }
    let mut next = values.to_vec();
    if !next_permutation(&mut next) {
        // The last order (decreasing): the first one (increasing) comes next.
        next.reverse();
    }
    if next == values {
        return None;
    }
    let mut taken = vec![false; values.len()];
    let mut order: Vec<Option<usize>> = vec![None; values.len()];
    for (i, (new, old)) in next.iter().zip(values).enumerate() {
        if new == old {
            order[i] = Some(i);
            taken[i] = true;
        }
    }
    for (i, new) in next.iter().enumerate() {
        if order[i].is_none() {
            let from = (0..values.len()).find(|&j| !taken[j] && values[j] == *new)?;
            order[i] = Some(from);
            taken[from] = true;
        }
    }
    order.into_iter().collect()
}

/// Puts `values` in their next order in lexicographic order; false (unchanged) if they're in the
/// last one, decreasing.
fn next_permutation(values: &mut [f32]) -> bool {
    // The last value smaller than the one after it.
    let Some(pivot) = (1..values.len())
        .rev()
        .find(|&i| values[i - 1] < values[i])
        .map(|i| i - 1)
    else {
        return false;
    };
    // The last value larger than it (the one after it is).
    let Some(larger) = (pivot + 1..values.len())
        .rev()
        .find(|&i| values[i] > values[pivot])
    else {
        return false;
    };
    values.swap(pivot, larger);
    values[pivot + 1..].reverse();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `values` after following `next_order` once.
    fn step(values: &[f32]) -> Option<Vec<f32>> {
        let order = next_order(values)?;
        Some(order.iter().map(|&from| values[from]).collect())
    }

    /// Every order `start` goes through until it comes back.
    fn cycle(start: &[f32]) -> Vec<Vec<f32>> {
        let mut seen = vec![start.to_vec()];
        let mut values = start.to_vec();
        while let Some(next) = step(&values) {
            if next == start {
                break;
            }
            assert!(!seen.contains(&next), "{next:?} came twice from {start:?}");
            seen.push(next.clone());
            values = next;
        }
        seen
    }

    #[test]
    fn three_different_values_go_through_six_orders() {
        for start in [[1.0, 2.0, 3.0], [3.0, 1.0, 2.0], [2.0, 3.0, 1.0]] {
            assert_eq!(cycle(&start).len(), 6, "from {start:?}");
        }
    }

    #[test]
    fn equal_values_give_fewer_orders() {
        assert_eq!(cycle(&[1.0, 1.0, 2.0]).len(), 3);
        assert_eq!(cycle(&[5.0, 2.0, 5.0]).len(), 3);
        assert_eq!(next_order(&[4.0, 4.0, 4.0]), None);
        assert_eq!(next_order(&[0.0, -0.0]), None);
    }

    #[test]
    fn two_values_swap() {
        assert_eq!(next_order(&[1.0, 2.0]), Some(vec![1, 0]));
        assert_eq!(next_order(&[2.0, 1.0]), Some(vec![1, 0]));
    }

    #[test]
    fn components_keeping_their_value_keep_their_place() {
        // 1, 2, 3 -> 1, 3, 2: Y and Z swap, X stays.
        assert_eq!(next_order(&[1.0, 2.0, 3.0]), Some(vec![0, 2, 1]));
        // 1, 1, 2 -> 1, 2, 1: the first 1 stays.
        assert_eq!(next_order(&[1.0, 1.0, 2.0]), Some(vec![0, 2, 1]));
        // 3, 2, 1 (last) -> 1, 2, 3: X and Z swap, Y stays.
        assert_eq!(next_order(&[3.0, 2.0, 1.0]), Some(vec![2, 1, 0]));
    }

    #[test]
    fn not_comparable_values_stay() {
        assert_eq!(next_order(&[f32::NAN, 1.0]), None);
    }
}
