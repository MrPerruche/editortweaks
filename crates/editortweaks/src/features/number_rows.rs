//! The numeric rows of property panels updating, shared by the features that look at or change a
//! row once it filled its sliders. Not a feature itself: the loader hooks a function only once.
//!
//! `UNumericPropertyWidget::UpdateValue` (`game::property_widget`) fills a row's sliders from the
//! first edited object whenever the panel shows or refreshes the property (not during a drag). Each
//! listener ([`listen`]) runs around it: it gets the row and the update to run, in which the next
//! listener runs, and so on to the game's own.

use core::ffi::c_void;
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::framework::hook;
use crate::game::property_widget::{self, UpdateValueFn};

/// Runs around a row's update: called with the row (a live `UNumericPropertyWidget`, on the game
/// thread) and the update, which it must run once.
pub type Around = unsafe fn(row: *mut c_void, update: &mut dyn FnMut());

static ORIGINAL: OnceLock<UpdateValueFn> = OnceLock::new();
/// Whether `UpdateValue` is hooked, or why not.
static HOOKED: OnceLock<Result<(), String>> = OnceLock::new();
static LISTENERS: Mutex<Vec<Around>> = Mutex::new(Vec::new());

/// Has `around` run around every numeric row's update, after the listeners added before (so
/// inside them). Hooks `UpdateValue` the first time; works before engine init.
pub fn listen(around: Around) -> Result<(), String> {
    HOOKED.get_or_init(hook_update).clone()?;
    LISTENERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(around);
    Ok(())
}

fn hook_update() -> Result<(), String> {
    let target = property_widget::UPDATE_VALUE.resolve()?;
    // SAFETY: `target` is `UpdateValue`, found by its pattern, and the detour has its signature.
    unsafe {
        hook::install_typed(
            property_widget::UPDATE_VALUE.name,
            target,
            update_value as UpdateValueFn,
            &ORIGINAL,
        )
    }
}

unsafe extern "C" fn update_value(this: *mut c_void, event: *const c_void) {
    let original = *ORIGINAL.get().expect("hook recorded before use");
    // Copied out, so that a listener may add another meanwhile without a deadlock.
    let listeners = LISTENERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    // SAFETY: forwards the caller's arguments unchanged.
    let mut update = || unsafe { original(this, event) };
    // SAFETY: UpdateValue is a UNumericPropertyWidget member, so `this` is a live row, and UI code
    // runs on the game thread.
    unsafe { run(&listeners, this, &mut update) };
}

/// Runs `update` inside `listeners`, the first one outermost.
///
/// # Safety
/// As for [`Around`].
unsafe fn run(listeners: &[Around], row: *mut c_void, update: &mut dyn FnMut()) {
    match listeners.split_first() {
        None => update(),
        // SAFETY: per the contract.
        Some((first, rest)) => unsafe { first(row, &mut || run(rest, row, update)) },
    }
}
