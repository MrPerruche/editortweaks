//! Which sliders and values go past the game's limits: only those of what the vehicle editor edits.
//! Everything else keeps the game's limits, in the slider and in the property: the options, the
//! admin menu, the server browser, a color row's sliders, a Blueprint's own sliders.
//!
//! A value belongs to the editor if its container's root is a `UBrickEditorObject` (as the game's
//! own `CanModify` casts it, see `game::brick_property`: the bricks) or a `UBrickEditorMode` (the
//! tools' own rows). Other panels' roots are the user settings (also the editor's side panel with
//! nothing selected: time of day, sun, UI scale), the game state, menus and popups.
//!
//! A slider only learns what it edits from its row, so sliders start out at the game's limits and
//! a numeric row's update (`number_rows`) marks its sliders. Rows are pooled and reused, and every
//! update marks them anew. Sliders outside of numeric rows are never marked.
//!
//! Rows of ours aren't stored through `ClampValue`, so nothing would stop a drag at their limits:
//! their sliders are only marked if the row takes values past their ends
//! (`Number::component_limit`), and they stop at that limit themselves.

use core::ffi::c_void;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UStruct;

use super::limits::SAFE_MAX;
use crate::features::number_rows;
use crate::game::numeric_property::Container;
use crate::game::property_reflection::{self, Number};
use crate::game::property_widget::{self, PropertyWidgets};
use crate::game::reflection;
use crate::game::weak_object::WeakObject;
use crate::prelude::*;

/// How many lifted sliders may be remembered before those destroyed since are first forgotten.
const LIFTED_CLEANUP: usize = 1024;

/// Whether rows are watched as they update (`number_rows`).
static LISTENING: OnceLock<()> = OnceLock::new();
/// Set once reflection is available; until then (or if it fails) nothing is lifted.
static SCOPE: OnceLock<Scope> = OnceLock::new();

struct Scope {
    /// `BrickEditorObject` and `BrickEditorMode`: a container's root of one of these is lifted.
    editor_classes: [*const UStruct; 2],
    widgets: PropertyWidgets,
}

// SAFETY: class records, alive as long as the game; only used on the game thread.
unsafe impl Send for Scope {}
unsafe impl Sync for Scope {}

/// A slider past whose ends values may go.
struct Lifted {
    slider: WeakObject,
    /// How far they may go. Rules may stop them earlier (`property`).
    limit: (f32, f32),
}

thread_local! {
    /// The sliders lifted, by address.
    static LIFTED: RefCell<HashMap<usize, Lifted>> = RefCell::new(HashMap::new());
    /// While a numeric row updates on this thread: whether what it edits is lifted, once known.
    static ROW_UPDATE: Cell<Option<Option<bool>>> = const { Cell::new(None) };
    /// The classes of containers' roots logged so far.
    static LOGGED: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// Watches rows as they update. Before engine init.
pub(super) fn install() -> Result<(), String> {
    number_rows::listen(around_update)?;
    let _ = LISTENING.set(());
    Ok(())
}

pub(super) fn on_engine_init() {
    if LISTENING.get().is_none() {
        return; // not hooked (pattern missing), already logged
    }
    let scope = || -> Result<Scope, String> {
        let class = |name: &'static str| {
            reflection::find_class(name)
                .map(|class| class.cast::<UStruct>())
                .ok_or(format!("class {name} not found"))
        };
        Ok(Scope {
            editor_classes: [class("BrickEditorObject")?, class("BrickEditorMode")?],
            widgets: property_widget::property_widgets()?,
        })
    };
    match scope() {
        Ok(scope) => {
            let _ = SCOPE.set(scope);
        }
        Err(reason) => log!("every value keeps the game's limits: {reason}"),
    }
}

/// How far values of `slider` may go, if past its ends: `None` keeps it at the game's limits.
pub(super) fn limit_of(slider: *const c_void) -> Option<(f32, f32)> {
    LIFTED.with_borrow(|lifted| {
        lifted
            .get(&(slider as usize))
            .filter(|entry| entry.slider.get().is_some())
            .map(|entry| entry.limit)
    })
}

/// Whether values stored for `container` go past the game's limits.
///
/// # Safety
/// `container` must be null or a live container, as the game passes it. Game thread only.
pub(super) unsafe fn lifts(container: *const c_void) -> bool {
    let Some(scope) = SCOPE.get() else {
        return false;
    };
    if container.is_null() {
        return false;
    }
    // SAFETY: per the contract; a container's root is a live object or null.
    unsafe {
        let root = (*container.cast::<Container>()).root();
        if root.is_null() {
            return false;
        }
        let object: *const UObjectBase = root.cast_const().cast();
        let lifted = scope
            .editor_classes
            .iter()
            .any(|&class| reflection::is_a(object, class));
        log_first(root, lifted);
        lifted
    }
}

/// Called with the container the panel asks a property's range for: during a row's update, that
/// tells what the row edits.
///
/// # Safety
/// As for [`lifts`].
pub(super) unsafe fn on_panel_range(container: *const c_void) {
    if ROW_UPDATE.get() == Some(None) {
        // SAFETY: per the contract.
        ROW_UPDATE.set(Some(Some(unsafe { lifts(container) })));
    }
}

/// A numeric row `row` updates (`number_rows`): marks its sliders once it filled them.
unsafe fn around_update(row: *mut c_void, update: &mut dyn FnMut()) {
    let previous = ROW_UPDATE.replace(Some(None));
    let number = property_reflection::noting_number_shown(update);
    let edits_editor = ROW_UPDATE.replace(previous).flatten().unwrap_or(false);

    let Some(scope) = SCOPE.get() else {
        return;
    };
    // SAFETY: UpdateValue is a UNumericPropertyWidget member, so `row` is one; its sliders are
    // live UBrickSliderWidgets, copied out since the list may change. Game thread only.
    let sliders = unsafe { scope.widgets.sliders(row.cast_const().cast()).to_vec() };
    LIFTED.with_borrow_mut(|lifted| {
        if lifted.len() >= LIFTED_CLEANUP {
            lifted.retain(|_, entry| entry.slider.get().is_some());
        }
        for (index, slider) in sliders.into_iter().enumerate() {
            let limit = match number {
                Some(number) => own_row_limit(number, index),
                None => edits_editor.then_some((-SAFE_MAX, SAFE_MAX)),
            };
            match limit {
                Some(limit) => {
                    // SAFETY: a live slider, as above.
                    let slider_ref = unsafe { WeakObject::new(slider.cast_mut().cast()) };
                    lifted.insert(
                        slider as usize,
                        Lifted {
                            slider: slider_ref,
                            limit,
                        },
                    );
                }
                None => {
                    lifted.remove(&(slider as usize));
                }
            }
        }
    });
}

/// How far the value `index` of a row of ours goes, if past its slider's ends.
fn own_row_limit(number: &dyn Number, index: usize) -> Option<(f32, f32)> {
    let limit = number.component_limit(index);
    let (min, max) = number.component_range(index);
    (limit.0 < min || limit.1 > max).then_some(limit)
}

/// Logs what becomes of the values of `root`'s class, the first time one comes by: to see which
/// panels are lifted, and to spot a panel of the editor that should be.
///
/// # Safety
/// `root` must be a live object. Game thread only.
unsafe fn log_first(root: *mut c_void, lifted: bool) {
    // SAFETY: per the contract.
    let class = unsafe { reflection::class_of(root.cast_const()) };
    let first = LOGGED.with_borrow_mut(|logged| {
        let first = !logged.contains(&(class as usize));
        if first {
            logged.push(class as usize);
        }
        first
    });
    if first {
        // SAFETY: an object's class is live.
        let name = unsafe { reflection::struct_name(class) };
        let what = if lifted {
            "past the game's limits"
        } else {
            "within the game's limits"
        };
        log!("{name}: values stay {what}");
    }
}
