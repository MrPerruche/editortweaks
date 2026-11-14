//! Access to the game's main window.

use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible,
};
use windows_sys::core::BOOL;

/// Window class Unreal Engine 4 registers for its top-level windows.
const UNREAL_WINDOW_CLASS: &str = "UnrealWindow";

/// Returns this process's visible top-level Unreal window (the game window), if it exists yet.
pub fn find_game_window() -> Option<HWND> {
    struct Search {
        pid: u32,
        found: HWND,
    }

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `&mut Search` passed to EnumWindows below, alive for the call.
        let search = unsafe { &mut *(lparam as *mut Search) };
        let mut pid = 0;
        // SAFETY: `hwnd` comes from EnumWindows; `pid` is a valid out pointer.
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        // SAFETY: `hwnd` comes from EnumWindows.
        let visible = unsafe { IsWindowVisible(hwnd) } != 0;
        if pid == search.pid && visible && class_name(hwnd) == UNREAL_WINDOW_CLASS {
            search.found = hwnd;
            return 0; // stop enumerating
        }
        1
    }

    let mut search = Search {
        // SAFETY: no preconditions.
        pid: unsafe { GetCurrentProcessId() },
        found: core::ptr::null_mut(),
    };
    // SAFETY: `visit` only dereferences `lparam` as the `Search` we pass, which outlives the call.
    unsafe { EnumWindows(Some(visit), &raw mut search as LPARAM) };
    (!search.found.is_null()).then_some(search.found)
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    // SAFETY: `buf` holds 64 u16s, and that's the maximum we let the API write.
    let len = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) }.max(0);
    String::from_utf16_lossy(&buf[..len as usize])
}

/// Whether the window the user works in now (the foreground one) is this process's.
#[cfg_attr(not(feature = "vehicle-menu"), allow(dead_code))]
pub fn is_foreground() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    // SAFETY: no preconditions.
    let window = unsafe { GetForegroundWindow() };
    if window.is_null() {
        return false;
    }
    let mut pid = 0;
    // SAFETY: a window handle from Windows; `pid` is a valid out pointer.
    unsafe { GetWindowThreadProcessId(window, &mut pid) };
    // SAFETY: no preconditions.
    pid == unsafe { GetCurrentProcessId() }
}
