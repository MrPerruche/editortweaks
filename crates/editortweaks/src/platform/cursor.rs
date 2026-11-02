//! The mouse cursor's position, in desktop pixels: the space of Slate's screen positions (its
//! Windows cursor reads `GetCursorPos`).

use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClipCursor, GetCursorPos, SetCursorPos};

/// The cursor's position, if Windows tells.
pub fn position() -> Option<(i32, i32)> {
    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: `point` is a valid out pointer.
    (unsafe { GetCursorPos(&mut point) } != 0).then_some((point.x, point.y))
}

/// Moves the cursor. Returns false if Windows refused.
pub fn set_position(x: i32, y: i32) -> bool {
    // SAFETY: no preconditions.
    unsafe { SetCursorPos(x, y) != 0 }
}

/// Where the cursor can go, from the left to the right edge (exclusive): the rectangle it's
/// confined to, or the whole desktop if it isn't.
pub fn horizontal_bounds() -> Option<(i32, i32)> {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: `rect` is a valid out pointer.
    (unsafe { GetClipCursor(&mut rect) } != 0).then_some((rect.left, rect.right))
}
