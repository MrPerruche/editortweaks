//! Keyboard and mouse button state, by Windows virtual key or by Unreal key name.

use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SWAPBUTTON};

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
/// right now; `None` for keys Windows can't tell (gamepad buttons, the mouse wheel, ...).
pub fn is_key_down(name: &str) -> Option<bool> {
    virtual_key(name).map(is_down)
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
fn virtual_key(name: &str) -> Option<VIRTUAL_KEY> {
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
