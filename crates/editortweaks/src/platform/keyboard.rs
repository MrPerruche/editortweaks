//! Keyboard state.

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VIRTUAL_KEY, VK_CONTROL, VK_MENU, VK_SHIFT,
};

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

fn is_down(key: VIRTUAL_KEY) -> bool {
    // SAFETY: no preconditions.
    let state = unsafe { GetAsyncKeyState(i32::from(key)) };
    // The high bit is set while the key is down.
    state < 0
}
