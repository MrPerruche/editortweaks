//! Typing an exact length, as in CAD: a press of [`keybinds::TYPE_LENGTH`] (Enter by default; Alt
//! let go, Alt + Enter is the engine's full screen key) opens a text box on a length showing
//! (`super::line_label`, a real text box of the game's: cursor, selection, Ctrl + C / V / Z). It
//! holds the length at full precision and its unit, all selected: typing replaces it, Left or
//! Right moves into it. What's typed is read in the editor's unit (the unit may be left in or out),
//! and may be a formula like the sliders' (`+ - * / % ^`, `sqrt`, parentheses, `x` the length now);
//! a leading `+` or `-` adds to the length or takes from it (see `typed_formula`). Enter (or a
//! click elsewhere) commits it, for the editor's tick to apply (see `brick_resize`); Escape
//! cancels, and so does committing it unchanged. Anything ending what's typed for also closes the
//! box, or a drag starting.
//!
//! What's typed for is up to the tick ([`set_ready`]): while moving bricks, the angle of the turn
//! about the line just dragged (for a few seconds, see `super::hinge`; in degrees, `°` optional; a
//! leading `+` or `-` is its sign, a negative one turns the other way, `+=` and `-=` add and take
//! away, see `typed_formula::typed_angle`), else the length of the line (see [`super::line_tool`])
//! when one of its points is on the moved bricks; in the default mode,
//! the size of a brick's face, the one hovered or just dragged. When the key can't type anything
//! there, a press tells that (see [`take_refused`]) and opens nothing.
//!
//! How: keys reach this before the game and the keybinds do (`UPlayerInput::InputKey`, hooked by
//! `click_swap`, which calls [`key`] through `brick_resize::typed_key`). The key's press opens the
//! box and is taken, with its release, so the game doesn't act on it (Enter opens the chat in a
//! hosted game). While the box is open, the text box gets the keys first; any key it lets through
//! to the game is taken too (digits may be bound), except the mouse buttons'.

use std::sync::{Mutex, PoisonError};

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    VIRTUAL_KEY, VK_ESCAPE, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON, VK_XBUTTON1, VK_XBUTTON2,
};

use crate::features::keybinds;
use crate::game::player_input;
use crate::platform::keyboard;

/// What a press of the key does now (told every frame by the editor's tick).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ready {
    /// Nothing: there's no length to type (or something is dragged), the game gets the key.
    No,
    /// Tells why a length can't be typed there (see [`take_refused`]): a line neither or both of
    /// whose points are on the moved bricks, a brick's edge or corner hovered.
    Refused,
    /// Opens the box.
    Yes,
}

/// How the text box closed (told by `super::line_label`, which shows it).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Closed {
    /// With this text, changed from what it opened with.
    Committed(String),
    /// Escape, committed unchanged, or the box couldn't show.
    Cancelled,
}

struct Typing {
    ready: Ready,
    /// Whether the box is open.
    open: bool,
    /// A text committed, for the tick to apply.
    committed: Option<String>,
    /// The key was pressed where a length can't be typed, for the tick to tell.
    refused: bool,
    /// The box was cancelled, for the tick to forget what it was for.
    cancelled: bool,
    /// Escape reached the game while the box was open, for the box to close.
    escaped: bool,
    /// For each virtual key: whether its press was taken, so that its release is too.
    taken: [bool; 256],
}

static TYPING: Mutex<Typing> = Mutex::new(Typing {
    ready: Ready::No,
    open: false,
    committed: None,
    refused: false,
    cancelled: false,
    escaped: false,
    taken: [false; 256],
});

fn typing() -> std::sync::MutexGuard<'static, Typing> {
    TYPING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Tells what a press of the key does now: unless it opens the box ([`Ready::Yes`]), the box
/// closes.
pub fn set_ready(ready: Ready) {
    let mut typing = typing();
    typing.ready = ready;
    if ready != Ready::Yes {
        typing.open = false;
    }
}

/// Whether keys may be taken: a press of the key does something, the box is open, or a key's
/// release is awaited.
pub fn is_listening() -> bool {
    let typing = typing();
    typing.ready != Ready::No || typing.open || typing.taken.contains(&true)
}

/// Whether the box is open.
pub fn is_open() -> bool {
    typing().open
}

/// Closes the box as the text box did (see [`Closed`]).
pub fn close(closed: Closed) {
    let mut typing = typing();
    if !typing.open {
        return;
    }
    typing.open = false;
    match closed {
        Closed::Committed(text) => typing.committed = Some(text),
        Closed::Cancelled => typing.cancelled = true,
    }
}

/// Whether a text was committed and not taken yet.
pub fn has_committed() -> bool {
    typing().committed.is_some()
}

/// The text committed since last asked.
pub fn take_committed() -> Option<String> {
    typing().committed.take()
}

/// Whether the key was pressed where a length can't be typed, since last asked.
pub fn take_refused() -> bool {
    core::mem::take(&mut typing().refused)
}

/// Whether the box was cancelled since last asked.
pub fn take_cancelled() -> bool {
    core::mem::take(&mut typing().cancelled)
}

/// Whether Escape was pressed while the box was open (and the text box let it through), since
/// last asked.
pub fn take_escaped() -> bool {
    core::mem::take(&mut typing().escaped)
}

/// Whether `key` is a mouse button's.
fn is_mouse_button(key: VIRTUAL_KEY) -> bool {
    matches!(
        key,
        VK_LBUTTON | VK_RBUTTON | VK_MBUTTON | VK_XBUTTON1 | VK_XBUTTON2
    )
}

/// A key the player's input got: `name` (as Unreal names it), and the event (`EInputEvent`).
/// Whether it's taken: the game must not see it.
pub fn key(name: &str, event: i32) -> bool {
    let Some(virtual_key) = keyboard::virtual_key(name) else {
        return false;
    };
    let mut typing = typing();
    let index = usize::from(virtual_key).min(255);
    let taken = match event {
        player_input::RELEASED => return core::mem::take(&mut typing.taken[index]),
        player_input::PRESSED | player_input::REPEAT if typing.open => {
            if virtual_key == VK_ESCAPE {
                typing.escaped = true;
            }
            !is_mouse_button(virtual_key)
        }
        player_input::PRESSED => {
            let opens = typing.ready != Ready::No && keybinds::TYPE_LENGTH.is_pressed_by(name);
            if opens {
                match typing.ready {
                    Ready::Yes => typing.open = true,
                    _ => typing.refused = true,
                }
            }
            opens
        }
        _ => false,
    };
    if taken {
        typing.taken[index] = true;
    }
    taken
}
