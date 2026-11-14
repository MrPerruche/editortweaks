//! Clicks on the game's menu buttons (`UMenuButtonWidget::OnClicked`, see `game::menu`), shared by
//! the features adding buttons of their own: the options' "Editor tweaks" button, property rows'
//! permutation button. Not a feature itself: the loader hooks a function only once. The buttons we
//! add have no delegate, so the game's handler does nothing for them; every click goes to the
//! listeners after it, which tell their buttons apart.

use std::sync::{Mutex, OnceLock, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;

use crate::framework::{hook, timings};
use crate::game::menu::{ButtonPanels, ClickedFn};

/// Called with the clicked button, on the game thread.
pub type Listener = fn(*mut UObjectBase);

/// Called with the clicked button before the game handles it, on the game thread: whether it
/// handled the click itself (then the game and the other listeners don't see it).
pub type Interceptor = fn(*mut UObjectBase) -> bool;

static ORIGINAL_CLICKED: OnceLock<ClickedFn> = OnceLock::new();
/// Whether the hook is in place, or why not.
static HOOKED: OnceLock<Result<(), String>> = OnceLock::new();
static LISTENERS: Mutex<Vec<Listener>> = Mutex::new(Vec::new());
static INTERCEPTORS: Mutex<Vec<Interceptor>> = Mutex::new(Vec::new());

/// Called with a button panel that made its buttons and is about to pool the rest
/// (`UMenuButtonPanelWidget::RemoveUnusedButtons`), on the game thread: buttons it adds now stay.
pub type Maker = fn(*mut UObjectBase);

static MAKERS: Mutex<Vec<Maker>> = Mutex::new(Vec::new());

/// Called with a button panel that just pooled its unused buttons, on the game thread: its buttons
/// are the ones it shows, e.g. to reorder them.
pub type Arranger = fn(*mut UObjectBase);

static ARRANGERS: Mutex<Vec<Arranger>> = Mutex::new(Vec::new());

/// Gives `maker` every button panel about to pool its unused buttons (see [`Maker`]). Run from
/// `settings_menu`'s hook of `RemoveUnusedButtons` (the loader hooks a function only once), so only
/// while that feature runs.
#[cfg_attr(not(feature = "vehicle-menu"), allow(dead_code))]
pub fn before_pooling(maker: Maker) {
    MAKERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(maker);
}

/// Runs the [`Maker`]s for `panel` (see [`before_pooling`]).
pub fn run_makers(panel: *mut UObjectBase) {
    // Copied out, so that a maker may register too without a deadlock.
    let makers = MAKERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    for maker in makers {
        maker(panel);
    }
}

/// Gives `arranger` every button panel that just pooled its unused buttons (see [`Arranger`]). Run
/// from `settings_menu`'s hook, as [`before_pooling`].
#[cfg_attr(not(feature = "vehicle-menu"), allow(dead_code))]
pub fn after_pooling(arranger: Arranger) {
    ARRANGERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(arranger);
}

/// Runs the [`Arranger`]s for `panel` (see [`after_pooling`]).
pub fn run_arrangers(panel: *mut UObjectBase) {
    // Copied out, as the makers.
    let arrangers = ARRANGERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    for arranger in arrangers {
        arranger(panel);
    }
}

/// Calls `listener` with every menu button clicked from now on (after engine init: the handler is
/// found through reflection).
pub fn listen(listener: Listener) -> Result<(), String> {
    HOOKED.get_or_init(hook_clicked).clone()?;
    LISTENERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(listener);
    Ok(())
}

/// Gives `interceptor` every menu button clicked from now on, before the game (see [`listen`]).
pub fn intercept(interceptor: Interceptor) -> Result<(), String> {
    HOOKED.get_or_init(hook_clicked).clone()?;
    INTERCEPTORS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(interceptor);
    Ok(())
}

fn hook_clicked() -> Result<(), String> {
    let target = ButtonPanels::find()?.on_clicked;
    // SAFETY: `target` is `UMenuButtonWidget::OnClicked`, whose signature `ClickedFn` is.
    unsafe {
        hook::install_typed(
            "UMenuButtonWidget::OnClicked",
            target,
            clicked as ClickedFn,
            &ORIGINAL_CLICKED,
        )
    }
}

unsafe extern "C" fn clicked(button: *mut UObjectBase) {
    let _span = timings::span("menu_buttons::clicked");
    let Some(original) = ORIGINAL_CLICKED.get() else {
        return;
    };
    let interceptors = INTERCEPTORS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if interceptors.into_iter().any(|intercept| intercept(button)) {
        return;
    }
    // SAFETY: forwards the caller's argument.
    unsafe { timings::game("UMenuButtonWidget::OnClicked", || original(button)) };
    // Copied out, so that a listener may listen too without a deadlock.
    let listeners = LISTENERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    for listener in listeners {
        listener(button);
    }
}
