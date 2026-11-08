//! Confirmed popups (`UPopupContainerWidget::ConfirmPopup`, see `game::popup`), shared by the
//! features opening popups of their own: coop's questions and ban dialog, the update reminder. Not
//! a feature itself: the loader hooks a function only once. Every way of confirming a popup goes
//! through it (the Confirm button, Enter); each interceptor tells its popups apart by their params.

use std::sync::{Mutex, OnceLock, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;

use crate::framework::{hook, timings};
use crate::game::popup::{ConfirmFn, Popups};

/// Called with the container confirming its popup, before the game closes it, on the game
/// thread: whether it closed the popup itself (then the game and the other interceptors don't see
/// the confirming).
pub type Interceptor = fn(*mut UObjectBase) -> bool;

static ORIGINAL_CONFIRM: OnceLock<ConfirmFn> = OnceLock::new();
/// Whether the hook is in place, or why not.
static HOOKED: OnceLock<Result<(), String>> = OnceLock::new();
static INTERCEPTORS: Mutex<Vec<Interceptor>> = Mutex::new(Vec::new());

/// Gives `interceptor` every popup confirmed from now on (after engine init: the function is found
/// through reflection).
pub fn intercept(interceptor: Interceptor) -> Result<(), String> {
    HOOKED.get_or_init(hook_confirm).clone()?;
    let mut interceptors = INTERCEPTORS.lock().unwrap_or_else(PoisonError::into_inner);
    if !interceptors.contains(&interceptor) {
        interceptors.push(interceptor);
    }
    Ok(())
}

fn hook_confirm() -> Result<(), String> {
    let target = Popups::find()?.confirm_popup;
    // SAFETY: `target` is `UPopupContainerWidget::ConfirmPopup`, whose signature `ConfirmFn` is.
    unsafe {
        hook::install_typed(
            "UPopupContainerWidget::ConfirmPopup",
            target,
            confirmed as ConfirmFn,
            &ORIGINAL_CONFIRM,
        )
    }
}

unsafe extern "C" fn confirmed(container: *mut UObjectBase) {
    let _span = timings::span("popup_answers::confirmed");
    let Some(original) = ORIGINAL_CONFIRM.get() else {
        return;
    };
    // Copied out, so that an interceptor may open popups or intercept too without a deadlock.
    let interceptors = INTERCEPTORS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if interceptors
        .into_iter()
        .any(|intercept| intercept(container))
    {
        return;
    }
    // SAFETY: forwards the caller's argument.
    unsafe {
        timings::game("UPopupContainerWidget::ConfirmPopup", || {
            original(container)
        })
    };
}
