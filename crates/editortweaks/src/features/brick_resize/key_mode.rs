//! Whether fast editing is on: while the fast editing key ([`keybinds::RESIZE_BRICKS`]) is held,
//! or, as the "Fast editing key" setting ([`KEY_MODE`]) can have it, also while a tap of the key
//! has toggled it on.
//!
//! A tap: the key goes down and up again within [`TAP_MS`], with no mouse button pressed meanwhile
//! and the game focused when it comes up. So Alt + Tab (which takes the focus away) and Alt + a
//! click or a drag (Alt is the key by default) don't toggle. Holding the key longer still fast
//! edits while it's held, without changing the toggle.
//!
//! While fast editing is on, the middle button's camera pivot stays taken, as while the key is held
//! (see [`super::State::pivot_click`]): with the toggle on, the middle button (the model key and the
//! copy keys by default) doesn't pivot the camera until it's toggled off.
//!
//! [`update`] follows the key once per editor frame; [`is_on`] tells the result to everything that
//! used to ask whether the key was held.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::features::keybinds;
use crate::framework::settings::{Alternative, Choice};
use crate::platform::keyboard;

/// How long the key may be held for it to be a tap (milliseconds).
const TAP_MS: u64 = 300;

/// The index of the toggle option in [`KEY_MODE`].
const TOGGLE: u8 = 1;

/// How the fast editing key works, a setting (see `super::SETTINGS`): the options of [`is_on`], in
/// order.
///
/// With "Tap to toggle", the middle button's camera pivot stays taken for as long as fast editing is
/// on (see the module's doc).
pub(super) static KEY_MODE: Choice = Choice::new(
    "FastEditingKey",
    "Fast editing key",
    "Hold: fast editing while the key is held. Tap to toggle: a quick tap of the key turns fast \
     editing on or off; holding it still fast edits while held. While it's on, the middle mouse \
     button doesn't pivot the camera.",
    &[
        Alternative {
            key: "Hold",
            name: "Hold",
        },
        Alternative {
            key: "Toggle",
            name: "Tap to toggle",
        },
    ],
    0,
);

static TAP: Mutex<Tap> = Mutex::new(Tap::new());

/// Whether fast editing is on now: the key is held, or (with the key a toggle) it was tapped on.
/// Game thread only, like [`keybinds::Keybind::is_held`].
pub fn is_on() -> bool {
    if keybinds::RESIZE_BRICKS.is_held() {
        return true;
    }
    KEY_MODE.get() == TOGGLE && TAP.lock().unwrap_or_else(PoisonError::into_inner).toggled
}

/// Follows the key for one editor frame: flips the toggle on a tap, and clears it while the key is
/// a hold. Called once per frame, before anything asks [`is_on`].
pub(super) fn update() {
    let mut tap = TAP.lock().unwrap_or_else(PoisonError::into_inner);
    if KEY_MODE.get() != TOGGLE {
        *tap = Tap::new();
        return;
    }
    let interrupted = keyboard::is_mouse_button_down()
        || keyboard::is_middle_button_down()
        || !keyboard::is_game_focused();
    tap.step(
        Instant::now(),
        keybinds::RESIZE_BRICKS.is_held(),
        interrupted,
    );
}

/// The key between frames.
struct Tap {
    /// Whether a tap has turned fast editing on.
    toggled: bool,
    /// While the key is held: when it went down, and whether it can still end as a tap.
    down: Option<Down>,
}

struct Down {
    since: Instant,
    tapping: bool,
}

impl Tap {
    const fn new() -> Self {
        Self {
            toggled: false,
            down: None,
        }
    }

    /// One frame: the key is `held` (or not) at `now`, and `interrupted` says a mouse button is down
    /// or the game isn't focused. A key that comes up as a tap flips the toggle.
    fn step(&mut self, now: Instant, held: bool, interrupted: bool) {
        match (&mut self.down, held) {
            (None, true) => {
                self.down = Some(Down {
                    since: now,
                    tapping: !interrupted,
                });
            }
            (Some(down), true) => down.tapping &= !interrupted,
            (Some(down), false) => {
                let quick = now.duration_since(down.since) <= Duration::from_millis(TAP_MS);
                if down.tapping && quick && !interrupted {
                    self.toggled = !self.toggled;
                }
                self.down = None;
            }
            (None, false) => {}
        }
    }
}
