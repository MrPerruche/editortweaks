//! Swapping the left and right mouse buttons in the editor, a setting ([`SWAP_CLICKS`], off by
//! default): the left button selects, the right one moves the camera, or the other way around.
//! Everything bound to one button reacts to the other in the editor: the game's actions (select,
//! add to selection, move the camera) and the mod's keys (`keybinds`, see [`pressed_for`]). Menus
//! keep their clicks, and nothing changes outside the editor (where the right button aims, say).
//!
//! Rebinding the game's actions can't do this: moving the camera is one action in the editor, the
//! free camera and vehicles alike, so binding it to the left button would fire weapons with it too.
//!
//! How: every key the player's controller gets goes through `UPlayerInput::InputKey`
//! (`game::player_input::INPUT_KEY`, hooked) before its bindings see it. A mouse button pressed
//! while the player looks through the editor ([`ViewTargets::in_editor`]) with the setting on comes
//! in as the other button, and its release the same way as its press, so no button stays down when
//! the setting or the view changes in between.
//!
//! The same hook tells `keybinds` which keys reach the viewport, as they come in (before the swap).

use std::sync::{Mutex, OnceLock, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;

use super::keybinds;
use crate::framework::hook;
use crate::framework::settings::Toggle;
use crate::framework::timings;
use crate::game::kismet;
use crate::game::player_input::{self, InputKeyFn, Key, ViewTargets};
use crate::platform::keyboard;
use crate::prelude::*;

/// Whether the mouse buttons are swapped in the editor.
pub static SWAP_CLICKS: Toggle = Toggle::new(
    "SwapClicks",
    "Swap left and right click",
    "In the vehicle editor, the left mouse button does what the right one does, and the other way \
     around. Menus keep their clicks.",
    false,
);

/// Unreal's names of the buttons.
const LEFT: &str = "LeftMouseButton";
const RIGHT: &str = "RightMouseButton";

pub struct ClickSwap;

static ORIGINAL: OnceLock<InputKeyFn> = OnceLock::new();
/// Set once the hook is in place and everything is found.
static GAME: OnceLock<Game> = OnceLock::new();
/// For each button (left, right, as they come in): whether its press went in as the other one,
/// which its release follows.
static PRESSES: Mutex<[bool; 2]> = Mutex::new([false; 2]);

struct Game {
    view_targets: ViewTargets,
    /// The buttons' names: left, right.
    buttons: [FName; 2],
}

impl Feature for ClickSwap {
    fn id(&self) -> &'static str {
        "click_swap"
    }

    fn on_engine_init(&self) {
        match engine_init() {
            Ok(()) => log!(
                "active: left and right click swapped in the editor while the setting is on (now {})",
                if SWAP_CLICKS.get() { "on" } else { "off" }
            ),
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

fn engine_init() -> Result<(), String> {
    let game = Game {
        view_targets: player_input::view_targets()?,
        buttons: [
            kismet::string_to_name(LEFT)?,
            kismet::string_to_name(RIGHT)?,
        ],
    };
    let target = player_input::INPUT_KEY.resolve()?;
    // SAFETY: `target` is the start of UPlayerInput::InputKey, which `input_key` matches.
    unsafe {
        hook::install_typed(
            player_input::INPUT_KEY.name,
            target,
            input_key as InputKeyFn,
            &ORIGINAL,
        )?;
    }
    let _ = GAME.set(game);
    // The hook also tells the keybinds which keys the viewport gets (`keybinds::key_delivered`).
    keyboard::track_delivery();
    Ok(())
}

unsafe extern "C" fn input_key(
    input: *mut UObjectBase,
    key: *mut Key,
    event: i32,
    amount: f32,
    gamepad: bool,
) -> bool {
    let _span = timings::span("click_swap::input_key");
    let original = *ORIGINAL.get().expect("hook recorded before use");
    // SAFETY: the function's own argument: the caller's live copy of the key. Before the swap
    // below, which only touches mouse buttons: the keybinds want the physical key.
    unsafe { keybinds::key_delivered(&(*key).name, event) };
    if let Some(game) = GAME.get() {
        // SAFETY: the function's own arguments: a live player input and the caller's copy of the
        // key, on the game thread.
        unsafe {
            let key = &mut *key;
            if let Some(button) = game.buttons.iter().position(|name| same(name, &key.name)) {
                let mut presses = PRESSES.lock().unwrap_or_else(PoisonError::into_inner);
                let swap = match event {
                    player_input::PRESSED | player_input::DOUBLE_CLICK => {
                        presses[button] = SWAP_CLICKS.get() && game.view_targets.in_editor(input);
                        presses[button]
                    }
                    player_input::RELEASED => core::mem::take(&mut presses[button]),
                    _ => presses[button],
                };
                if swap {
                    key.rename(game.buttons[1 - button]);
                }
            }
        }
    }
    // SAFETY: the caller's arguments, the key maybe renamed.
    unsafe {
        timings::game(player_input::INPUT_KEY.name, || {
            original(input, key, event, amount, gamepad)
        })
    }
}

fn same(a: &FName, b: &FName) -> bool {
    a.comparison_index == b.comparison_index && a.number == b.number
}

/// The key to hold for a binding to the key Unreal calls `name`: with the buttons swapped (the
/// setting on, and the swap working), a mouse button's binding is the other button's. For the
/// mod's keys, which only work in the editor.
pub fn pressed_for(name: &str) -> &str {
    if !(SWAP_CLICKS.get() && GAME.get().is_some()) {
        return name;
    }
    match name {
        LEFT => RIGHT,
        RIGHT => LEFT,
        other => other,
    }
}
