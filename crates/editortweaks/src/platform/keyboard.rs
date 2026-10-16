//! Keyboard and mouse button state, by Windows virtual key or by Unreal key name.
//!
//! Keys are polled (`GetAsyncKeyState`), which alone knows nothing of what the game does with
//! them. [`is_key_down`] therefore counts a key only while
//! - the game's window is the foreground one ([`is_game_focused`]), and
//! - for keys that type text or act in menus (all but modifier keys and mouse buttons): the game's
//!   viewport was given the press ([`key_delivered`], from the hooked `UPlayerInput::InputKey`),
//!   which a focused text box such as a slider's value keeps from happening. Modifier keys and
//!   mouse buttons are exempt: they type nothing, and dragged sliders and widgets keep them from the
//!   viewport too. Without the hook ([`track_delivery`] never called) every key counts.

use core::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetSystemMetrics, GetWindowThreadProcessId, SM_SWAPBUTTON,
};

/// For each virtual key: whether the game's viewport was given its press and not its release yet.
static DELIVERED: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];
/// Whether [`DELIVERED`] is kept up to date (the key hook is in place).
static TRACKING: AtomicBool = AtomicBool::new(false);

/// Starts requiring that keys be delivered to the game's viewport (see the module's doc): called
/// once the hook calling [`key_delivered`] is in place.
pub fn track_delivery() {
    TRACKING.store(true, Ordering::Relaxed);
}

/// Records that the game's viewport got the press (`down`) or release of virtual key `key`.
pub fn key_delivered(key: VIRTUAL_KEY, down: bool) {
    if let Some(flag) = DELIVERED.get(usize::from(key)) {
        flag.store(down, Ordering::Relaxed);
    }
}

/// Whether the game's window is the one the user is typing into (a window of this process has the
/// keyboard focus), as opposed to another application or a background game.
pub fn is_game_focused() -> bool {
    // SAFETY: no preconditions; a null window (none focused) has no process, which doesn't match.
    unsafe {
        let mut process = 0;
        GetWindowThreadProcessId(GetForegroundWindow(), &mut process);
        process != 0 && process == GetCurrentProcessId()
    }
}

/// Whether either Alt key is held right now (read like Unreal reads modifier keys).
pub fn is_alt_down() -> bool {
    is_down(VK_MENU)
}

/// Whether either Shift key is held right now.
pub fn is_shift_down() -> bool {
    is_down(VK_SHIFT)
}

/// Whether either Control key is held right now (also while AltGr is).
pub fn is_control_down() -> bool {
    is_down(VK_CONTROL)
}

/// Whether either Windows key is held right now (Unreal's "Cmd" modifier).
pub fn is_command_down() -> bool {
    is_down(VK_LWIN) || is_down(VK_RWIN)
}

/// Whether a mouse button (the left or the right one) is held right now. Windows reports the
/// physical buttons, which swapped buttons exchange, so both are checked.
pub fn is_mouse_button_down() -> bool {
    is_down(VK_LBUTTON) || is_down(VK_RBUTTON)
}

/// Whether the key Unreal calls `name` (an `FKey` name, e.g. `LeftAlt`, `G`, `Ampersand`) is held
/// right now, in the sense of the module's doc; `None` for keys Windows can't tell (gamepad
/// buttons, the mouse wheel, ...).
pub fn is_key_down(name: &str) -> Option<bool> {
    let key = virtual_key(name)?;
    if !is_game_focused() || !is_down(key) {
        // Let go (or the game isn't looking): whatever was delivered is stale, say a release a
        // text box took.
        key_delivered(key, false);
        return Some(false);
    }
    Some(!TRACKING.load(Ordering::Relaxed) || !needs_delivery(key) || delivered(key))
}

/// Whether the game's viewport got the press of `key` (and not its release).
fn delivered(key: VIRTUAL_KEY) -> bool {
    DELIVERED
        .get(usize::from(key))
        .is_some_and(|flag| flag.load(Ordering::Relaxed))
}

/// Whether `key` only counts once the viewport got it: it isn't a modifier key or a mouse button.
fn needs_delivery(key: VIRTUAL_KEY) -> bool {
    !matches!(
        key,
        VK_SHIFT
            | VK_LSHIFT
            | VK_RSHIFT
            | VK_CONTROL
            | VK_LCONTROL
            | VK_RCONTROL
            | VK_MENU
            | VK_LMENU
            | VK_RMENU
            | VK_LWIN
            | VK_RWIN
            | VK_LBUTTON
            | VK_RBUTTON
            | VK_MBUTTON
            | VK_XBUTTON1
            | VK_XBUTTON2
    )
}

fn is_down(key: VIRTUAL_KEY) -> bool {
    // SAFETY: no preconditions.
    let state = unsafe { GetAsyncKeyState(i32::from(key)) };
    // The high bit is set while the key is down.
    state < 0
}

/// Keys Unreal names by their virtual key (UE 4.27 `FWindowsPlatformInput::GetKeyMap`).
const NAMED_KEYS: &[(&str, VIRTUAL_KEY)] = &[
    ("MiddleMouseButton", VK_MBUTTON),
    ("ThumbMouseButton", VK_XBUTTON1),
    ("ThumbMouseButton2", VK_XBUTTON2),
    ("BackSpace", VK_BACK),
    ("Tab", VK_TAB),
    ("Enter", VK_RETURN),
    ("Pause", VK_PAUSE),
    ("CapsLock", VK_CAPITAL),
    ("Escape", VK_ESCAPE),
    ("SpaceBar", VK_SPACE),
    ("PageUp", VK_PRIOR),
    ("PageDown", VK_NEXT),
    ("End", VK_END),
    ("Home", VK_HOME),
    ("Left", VK_LEFT),
    ("Up", VK_UP),
    ("Right", VK_RIGHT),
    ("Down", VK_DOWN),
    ("Insert", VK_INSERT),
    ("Delete", VK_DELETE),
    ("NumPadZero", VK_NUMPAD0),
    ("NumPadOne", VK_NUMPAD1),
    ("NumPadTwo", VK_NUMPAD2),
    ("NumPadThree", VK_NUMPAD3),
    ("NumPadFour", VK_NUMPAD4),
    ("NumPadFive", VK_NUMPAD5),
    ("NumPadSix", VK_NUMPAD6),
    ("NumPadSeven", VK_NUMPAD7),
    ("NumPadEight", VK_NUMPAD8),
    ("NumPadNine", VK_NUMPAD9),
    ("Multiply", VK_MULTIPLY),
    ("Add", VK_ADD),
    ("Subtract", VK_SUBTRACT),
    ("Decimal", VK_DECIMAL),
    ("Divide", VK_DIVIDE),
    ("NumLock", VK_NUMLOCK),
    ("ScrollLock", VK_SCROLL),
    ("LeftShift", VK_LSHIFT),
    ("RightShift", VK_RSHIFT),
    ("LeftControl", VK_LCONTROL),
    ("RightControl", VK_RCONTROL),
    ("LeftAlt", VK_LMENU),
    ("RightAlt", VK_RMENU),
    ("LeftCommand", VK_LWIN),
    ("RightCommand", VK_RWIN),
];

/// Keys Unreal names by the character they type (`FGenericPlatformInput::GetStandardPrintableKeyMap`);
/// single letters and other single characters are their own names.
const CHARACTER_KEYS: &[(&str, char)] = &[
    ("Zero", '0'),
    ("One", '1'),
    ("Two", '2'),
    ("Three", '3'),
    ("Four", '4'),
    ("Five", '5'),
    ("Six", '6'),
    ("Seven", '7'),
    ("Eight", '8'),
    ("Nine", '9'),
    ("Semicolon", ';'),
    ("Equals", '='),
    ("Comma", ','),
    ("Hyphen", '-'),
    ("Underscore", '_'),
    ("Period", '.'),
    ("Slash", '/'),
    ("Tilde", '`'),
    ("LeftBracket", '['),
    ("Backslash", '\\'),
    ("RightBracket", ']'),
    ("Apostrophe", '\''),
    ("Ampersand", '&'),
    ("Asterix", '*'),
    ("Caret", '^'),
    ("Colon", ':'),
    ("Dollar", '$'),
    ("Exclamation", '!'),
    ("LeftParantheses", '('),
    ("RightParantheses", ')'),
    ("Quote", '"'),
    ("A_AccentGrave", 'à'),
    ("C_Cedille", 'ç'),
    ("E_AccentAigu", 'é'),
    ("E_AccentGrave", 'è'),
    ("Section", '§'),
];

/// The virtual key of the key Unreal calls `name`: the reverse of how Unreal names Windows keys.
/// Keys that type a character are named after it in the current keyboard layout (on AZERTY, the
/// key typing `&` is `Ampersand`), so they're found through the layout too.
pub fn virtual_key(name: &str) -> Option<VIRTUAL_KEY> {
    // Unreal's left mouse button is the one acting as left (after swapping), Windows' the
    // physical one.
    // SAFETY: no preconditions.
    let swapped = unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0;
    match name {
        "LeftMouseButton" => return Some(if swapped { VK_RBUTTON } else { VK_LBUTTON }),
        "RightMouseButton" => return Some(if swapped { VK_LBUTTON } else { VK_RBUTTON }),
        _ => {}
    }
    if let Some(&(_, key)) = NAMED_KEYS.iter().find(|(known, _)| *known == name) {
        return Some(key);
    }
    if let Some(number) = name.strip_prefix('F').and_then(|n| n.parse::<u16>().ok())
        && (1..=24).contains(&number)
    {
        return Some(VK_F1 + number - 1);
    }
    let character = match CHARACTER_KEYS.iter().find(|(known, _)| *known == name) {
        Some(&(_, character)) => character,
        None => {
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(character), None) => character,
                _ => return None,
            }
        }
    };
    let unit = u16::try_from(u32::from(character)).ok()?;
    // SAFETY: no preconditions. The low byte is the virtual key, the high one the modifier keys
    // typing the character (ignored: Unreal names keys by their character without modifiers).
    let found = unsafe { VkKeyScanW(unit) };
    (found != -1).then_some(found as u16 & 0xFF)
}
