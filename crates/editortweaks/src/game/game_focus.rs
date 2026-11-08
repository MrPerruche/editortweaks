//! Giving the keyboard back to the game's 3D view: `UWidgetBlueprintLibrary::SetFocusToGameViewport()`
//! (UMG, static), which moves every user's keyboard focus to the game's viewport widget, through
//! reflection.
//!
//! A widget with the keyboard focus gets keys before the game: the game's buttons take it when
//! hovered, and then Enter presses the button last hovered instead of reaching the game (and the
//! mod's keys) until something else takes it.

use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;

use crate::game::reflection::{self, Function, Parameters};
use crate::prelude::*;

/// What [`give_to_view`] calls.
struct GameFocus {
    function: Function,
    /// `WidgetBlueprintLibrary`'s default object, to call the static function on.
    library: *mut UObjectBase,
}

// SAFETY: a reflection record and a default object living as long as the game; only used on the
// game thread.
unsafe impl Send for GameFocus {}
unsafe impl Sync for GameFocus {}

static FOCUS: OnceLock<Option<GameFocus>> = OnceLock::new();

fn find() -> Result<GameFocus, String> {
    let library_class = reflection::find_class("WidgetBlueprintLibrary")
        .ok_or("class WidgetBlueprintLibrary not found")?;
    Ok(GameFocus {
        function: Function::find(library_class, "SetFocusToGameViewport")?,
        library: reflection::find_object(
            "Default__WidgetBlueprintLibrary",
            "WidgetBlueprintLibrary",
        )
        .ok_or("WidgetBlueprintLibrary's default object not found")?,
    })
}

/// Gives the keyboard focus to the game's 3D view, taking it from any widget (looked for the first
/// time; if the game doesn't offer it, that's logged once and nothing happens).
///
/// # Safety
/// After engine init. Game thread only.
pub unsafe fn give_to_view() {
    let focus = FOCUS.get_or_init(|| {
        find()
            .map_err(|reason| log!("the keyboard can't be given back to the 3D view: {reason}"))
            .ok()
    });
    if let Some(focus) = focus {
        // SAFETY: per the contract; a static function without parameters, on its class's default
        // object.
        unsafe { focus.function.call(focus.library, &mut Parameters::new()) };
    }
}
