//! The mouse cursor's position, in desktop pixels: the space of Slate's screen positions (its
//! Windows cursor reads `GetCursorPos`).

use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetClipCursor, GetCursorPos, SetCursorPos,
};

/// A rectangle of the desktop, right and bottom edges exclusive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Area {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Area {
    /// What this and `other` share, if anything.
    pub fn within(self, other: Self) -> Option<Self> {
        let shared = Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        };
        (shared.left < shared.right && shared.top < shared.bottom).then_some(shared)
    }
}

impl From<RECT> for Area {
    fn from(rect: RECT) -> Self {
        Self {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        }
    }
}

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

/// Where the cursor can go while dragging in the calling thread's active window: the inside of the
/// window, within the rectangle the cursor is confined to (the whole desktop if it isn't, so
/// every monitor). Just that rectangle if there's no such window.
pub fn window_area() -> Option<Area> {
    let confined = confined_area()?;
    Some(
        client_area()
            .and_then(|client| client.within(confined))
            .unwrap_or(confined),
    )
}

/// The rectangle the cursor is confined to, or the whole desktop if it isn't.
fn confined_area() -> Option<Area> {
    let mut rect = empty_rect();
    // SAFETY: `rect` is a valid out pointer.
    (unsafe { GetClipCursor(&mut rect) } != 0).then_some(rect.into())
}

/// The inside of the calling thread's active window, on the desktop.
fn client_area() -> Option<Area> {
    let mut rect = empty_rect();
    let mut corner = POINT { x: 0, y: 0 };
    // SAFETY: no preconditions; a null window (none active) is refused by the two calls after it,
    // whose out pointers are valid.
    let found = unsafe {
        let window = GetActiveWindow();
        !window.is_null()
            && GetClientRect(window, &mut rect) != 0
            && ClientToScreen(window, &mut corner) != 0
    };
    found.then_some(Area {
        left: corner.x,
        top: corner.y,
        right: corner.x + rect.right,
        bottom: corner.y + rect.bottom,
    })
}

fn empty_rect() -> RECT {
    RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    }
}
