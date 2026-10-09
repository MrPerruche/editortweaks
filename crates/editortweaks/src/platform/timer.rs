//! Thread timers: a callback that Windows runs on the thread that started the timer, from that
//! thread's message loop. The game thread processes its window messages once per frame, so a short
//! timer there ticks about once per frame, between input events, on the game thread.

use core::marker::PhantomData;
use std::cell::RefCell;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

/// The callback of each running timer of a thread, by timer id.
type Callbacks = Vec<(usize, fn())>;

thread_local! {
    static CALLBACKS: RefCell<Callbacks> = const { RefCell::new(Vec::new()) };
}

/// A running timer; dropping it stops it.
pub struct ThreadTimer {
    id: usize,
    /// Timers belong to the thread that started them.
    _not_send: PhantomData<*const ()>,
}

impl ThreadTimer {
    /// Calls `callback` every `interval_ms` milliseconds (at least 10) on this thread, whenever it
    /// processes its messages. `None` if Windows refuses.
    pub fn start(interval_ms: u32, callback: fn()) -> Option<Self> {
        // SAFETY: a timer without a window, whose callback is `on_timer` below.
        let id = unsafe { SetTimer(core::ptr::null_mut(), 0, interval_ms, Some(on_timer)) };
        if id == 0 {
            return None;
        }
        CALLBACKS.with_borrow_mut(|callbacks| callbacks.push((id, callback)));
        Some(Self {
            id,
            _not_send: PhantomData,
        })
    }
}

impl Drop for ThreadTimer {
    fn drop(&mut self) {
        // SAFETY: `id` is a timer this thread started (a timer stays on its thread: not `Send`).
        unsafe { KillTimer(core::ptr::null_mut(), self.id) };
        CALLBACKS.with_borrow_mut(|callbacks| callbacks.retain(|&(id, _)| id != self.id));
    }
}

unsafe extern "system" fn on_timer(_window: HWND, _message: u32, id: usize, _time: u32) {
    let callback = CALLBACKS.with_borrow(|callbacks| {
        callbacks
            .iter()
            .find(|&&(timer, _)| timer == id)
            .map(|&(_, callback)| callback)
    });
    // Called outside of the borrow: the callback may stop its timer.
    if let Some(callback) = callback {
        callback();
    }
}
