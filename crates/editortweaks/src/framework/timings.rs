//! Timings, off unless asked for, to find what makes the game slow. With `LogTimings=True` in
//! `EditorTweaks.ini` (`et.py timings on`, then start the game), `brickworks.txt` gets:
//!
//! - every frame of [`LONG_FRAME`] or more (opening the editor, a hitch while dragging), with what
//!   the mod and the game code it measures took during it;
//! - every [`SUMMARY_EVERY`] while the mod measured anything: the frames (how many, the longest)
//!   and what it measured, longest first.
//!
//! Code measures itself with spans: [`span`] around work of ours (a hook's detour, a feature's
//! per-frame work), [`game`] around a call into the game's code from there (the hooked original).
//! A span's own time leaves out the spans inside it, so a detour's own time doesn't count the game
//! function it calls, which shows apart as `game:` its name. Only the game thread's spans are
//! told. Off, a span costs one atomic read.
//!
//! Frames: the editor's tick marks them while the editor is open ([`editor_frame`]); otherwise a
//! thread timer does, which Windows runs once per frame while the game reads its window messages.
//!
//! A frame that never ends (the game frozen) would never be logged: a watchdog thread tells a
//! frame lasting [`STUCK`] or more while it lasts, with the spans it's inside. `et.py hang`, while
//! it's frozen, shows the game thread's whole stack.

use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::framework::settings::Toggle;
use crate::platform::timer::ThreadTimer;
use crate::prelude::*;

/// Whether to measure, from the next start of the game. Only in `EditorTweaks.ini`: not a choice
/// for players, but something to switch on when looking for what's slow.
pub static LOG_TIMINGS: Toggle = Toggle::new(
    "LogTimings",
    "Log timings",
    "Writes how long frames and the mod's work take to brickworks.txt, to find what is slow.",
    false,
);

/// A frame at least this long is logged on its own.
const LONG_FRAME: Duration = Duration::from_millis(100);

/// How often the summary is logged (if anything was measured).
const SUMMARY_EVERY: Duration = Duration::from_secs(10);

/// The thread timer's interval: shorter than a frame, so it runs once per frame.
const TIMER_MS: u32 = 10;

/// The editor's tick marks frames while it ran this recently; the timer otherwise.
const EDITOR_TICKING: Duration = Duration::from_millis(500);

/// How many entries a line lists, longest first.
const LISTED: usize = 8;

/// A frame lasting this long is told while it lasts, then each time it lasted twice as long.
const STUCK: Duration = Duration::from_secs(5);

/// How often the watchdog looks at the game thread.
const WATCH_EVERY: Duration = Duration::from_secs(1);

/// Set once at engine init, from [`LOG_TIMINGS`].
static ON: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// Whether this is the game thread, whose spans the watchdog sees.
    static GAME_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// What the game thread is doing, for the watchdog.
struct Now {
    frame_started: Option<Instant>,
    /// The spans open, outermost first.
    open: Vec<(&'static str, Side)>,
}

static NOW: Mutex<Now> = Mutex::new(Now {
    frame_started: None,
    open: Vec::new(),
});

fn now() -> MutexGuard<'static, Now> {
    NOW.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts measuring if [`LOG_TIMINGS`] is on. On the game thread, at engine init.
pub fn start() {
    if !LOG_TIMINGS.get() {
        return;
    }
    let Some(timer) = ThreadTimer::start(TIMER_MS, timer_tick) else {
        log!("timings off: no timer");
        return;
    };
    with_thread(|thread| thread.timer = Some(timer));
    GAME_THREAD.set(true);
    let watchdog = std::thread::Builder::new()
        .name("editortweaks timings".into())
        .spawn(watch);
    if let Err(error) = watchdog {
        log!("timings won't tell frozen frames: no thread ({error})");
    }
    ON.store(true, Ordering::Relaxed);
    log!(
        "timings on: frames of {} ms or more, frames lasting {} s or more while they last, and a \
         summary every {} s",
        LONG_FRAME.as_millis(),
        STUCK.as_secs(),
        SUMMARY_EVERY.as_secs()
    );
}

/// The watchdog thread: tells a frame lasting [`STUCK`] or more, and what it's inside.
fn watch() {
    // The frame last told, and when to tell it again.
    let mut told: Option<(Instant, Duration)> = None;
    loop {
        std::thread::sleep(WATCH_EVERY);
        let (started, open) = {
            let now = now();
            (now.frame_started, now.open.clone())
        };
        let Some(started) = started else {
            continue;
        };
        let next = match told {
            Some((frame, next)) if frame == started => next,
            _ => STUCK,
        };
        let lasted = started.elapsed();
        if lasted < next {
            continue;
        }
        told = Some((started, lasted * 2));
        let inside = if open.is_empty() {
            "nothing the mod measures (the game's own code)".to_owned()
        } else {
            open.iter()
                .map(|&(name, side)| match side {
                    Side::Ours => name.to_owned(),
                    Side::Game => format!("game:{name}"),
                })
                .collect::<Vec<_>>()
                .join(" > ")
        };
        // While the game thread is stuck: it rarely logs at the same time.
        log!(
            "timings: a frame has lasted {} s so far, inside: {inside}",
            lasted.as_secs()
        );
    }
}

/// Measures work of ours until the returned guard is dropped: `let _span = timings::span("...");`
/// at the start of a detour.
#[must_use = "the span ends when this is dropped"]
pub fn span(name: &'static str) -> Span {
    Span::start(name, Side::Ours)
}

/// Calls `call`, a call into the game's code (a hooked original), measured as the game's.
pub fn game<T>(name: &'static str, call: impl FnOnce() -> T) -> T {
    let _span = Span::start(name, Side::Game);
    call()
}

/// A frame of the editor starts (its tick). Game thread only.
pub fn editor_frame() {
    if !ON.load(Ordering::Relaxed) {
        return;
    }
    let now = Instant::now();
    let restarted = with_thread(|thread| {
        let restarted = thread
            .editor_ticked
            .is_none_or(|ticked| now - ticked > EDITOR_TICKING);
        thread.editor_ticked = Some(now);
        restarted
    });
    if restarted == Some(true) {
        // The long frames just before were opening it.
        log!("timings: the editor ticks");
    }
    frame_ended(now);
}

/// Every frame (about), from the thread timer: marks a frame unless the editor's tick does.
fn timer_tick() {
    let now = Instant::now();
    let editor_ticking = with_thread(|thread| {
        let ticking = thread
            .editor_ticked
            .is_some_and(|ticked| now - ticked <= EDITOR_TICKING);
        if ticking {
            thread.timer_waited = true;
        } else if thread.timer_waited {
            // The editor stopped ticking a while ago: frames start anew rather than one as long
            // as the wait (a long frame closing the editor ends before the wait, and shows).
            thread.timer_waited = false;
            thread.last_frame = Some(now);
            thread.frame = Tally::default();
            self::now().frame_started = Some(now);
            return true;
        }
        ticking
    });
    if editor_ticking == Some(false) {
        frame_ended(now);
    }
}

fn frame_ended(now: Instant) {
    self::now().frame_started = Some(now);
    let lines = with_thread(|thread| thread.frame_ended(now)).unwrap_or_default();
    for line in lines {
        log!("timings: {line}");
    }
}

/// Whose code a span measures.
#[derive(Clone, Copy, PartialEq)]
enum Side {
    Ours,
    Game,
}

/// A measured part, until dropped (see [`span`]).
pub struct Span {
    /// When it started, what it is; none while timings are off.
    started: Option<(Instant, &'static str, Side)>,
}

impl Span {
    fn start(name: &'static str, side: Side) -> Self {
        if !ON.load(Ordering::Relaxed) {
            return Self { started: None };
        }
        with_thread(|thread| thread.open.push(Duration::ZERO));
        if GAME_THREAD.get() {
            now().open.push((name, side));
        }
        Self {
            started: Some((Instant::now(), name, side)),
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        let Some((started, name, side)) = self.started else {
            return;
        };
        let total = started.elapsed();
        if GAME_THREAD.get() {
            now().open.pop();
        }
        with_thread(|thread| {
            let inside = thread.open.pop().unwrap_or_default();
            if let Some(outer) = thread.open.last_mut() {
                *outer += total;
            }
            let own = total.saturating_sub(inside);
            thread.frame.add(name, side, own);
            thread.window.add(name, side, own);
        });
    }
}

/// What one name took over a while.
struct Entry {
    name: &'static str,
    side: Side,
    count: u32,
    own: Duration,
    longest: Duration,
}

/// What was measured over a while, by name.
#[derive(Default)]
struct Tally {
    entries: Vec<Entry>,
}

impl Tally {
    fn add(&mut self, name: &'static str, side: Side, own: Duration) {
        let found = self
            .entries
            .iter_mut()
            .find(|entry| entry.name == name && entry.side == side);
        match found {
            Some(entry) => {
                entry.count += 1;
                entry.own += own;
                entry.longest = entry.longest.max(own);
            }
            None => self.entries.push(Entry {
                name,
                side,
                count: 1,
                own,
                longest: own,
            }),
        }
    }

    fn total(&self, side: Side) -> Duration {
        self.entries
            .iter()
            .filter(|entry| entry.side == side)
            .map(|entry| entry.own)
            .sum()
    }

    /// `ours 12.3 ms, game 4.5 ms: name 3x 10.1 ms (longest 5.0 ms), ...`, the longest first.
    fn describe(&mut self) -> String {
        self.entries
            .sort_by_key(|entry| core::cmp::Reverse(entry.own));
        let mut text = format!(
            "ours {}, game {}",
            ms(self.total(Side::Ours)),
            ms(self.total(Side::Game))
        );
        for (index, entry) in self.entries.iter().take(LISTED).enumerate() {
            let prefix = match entry.side {
                Side::Ours => "",
                Side::Game => "game:",
            };
            let _ = write!(
                text,
                "{} {prefix}{} {}x {} (longest {})",
                if index == 0 { ":" } else { "," },
                entry.name,
                entry.count,
                ms(entry.own),
                ms(entry.longest)
            );
        }
        if self.entries.len() > LISTED {
            let _ = write!(text, ", {} more", self.entries.len() - LISTED);
        }
        text
    }
}

fn ms(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
}

/// The game thread's measures.
struct Thread {
    /// The spans open, innermost last: the time of the spans inside each so far.
    open: Vec<Duration>,
    /// Since the last frame, and since the last summary.
    frame: Tally,
    window: Tally,
    last_frame: Option<Instant>,
    /// The summary's: when it began, how many frames, the longest, how many long ones.
    window_start: Option<Instant>,
    frames: u32,
    longest_frame: Duration,
    long_frames: u32,
    editor_ticked: Option<Instant>,
    /// Whether the timer left frames to the editor's tick since it last marked one.
    timer_waited: bool,
    timer: Option<ThreadTimer>,
}

impl Thread {
    /// A frame ended `now`: the lines to log.
    fn frame_ended(&mut self, now: Instant) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(last) = self.last_frame {
            let frame = now - last;
            self.frames += 1;
            self.longest_frame = self.longest_frame.max(frame);
            if frame >= LONG_FRAME {
                self.long_frames += 1;
                lines.push(format!("{} frame; {}", ms(frame), self.frame.describe()));
            }
        }
        self.last_frame = Some(now);
        self.frame = Tally::default();
        let window_start = *self.window_start.get_or_insert(now);
        let over = now - window_start;
        if over >= SUMMARY_EVERY {
            if !self.window.entries.is_empty() || self.long_frames > 0 {
                lines.push(format!(
                    "last {:.1} s: {} frames, longest {}, {} of {} ms or more; {}",
                    over.as_secs_f64(),
                    self.frames,
                    ms(self.longest_frame),
                    self.long_frames,
                    LONG_FRAME.as_millis(),
                    self.window.describe()
                ));
            }
            self.window = Tally::default();
            self.window_start = Some(now);
            self.frames = 0;
            self.longest_frame = Duration::ZERO;
            self.long_frames = 0;
        }
        lines
    }
}

thread_local! {
    static THREAD: RefCell<Thread> = const {
        RefCell::new(Thread {
            open: Vec::new(),
            frame: Tally { entries: Vec::new() },
            window: Tally { entries: Vec::new() },
            last_frame: None,
            window_start: None,
            frames: 0,
            longest_frame: Duration::ZERO,
            long_frames: 0,
            editor_ticked: None,
            timer_waited: false,
            timer: None,
        })
    };
}

/// Runs `f` on this thread's measures, unless they're in use (a span ending while a line is
/// made) or gone (the thread ending).
fn with_thread<T>(f: impl FnOnce(&mut Thread) -> T) -> Option<T> {
    THREAD
        .try_with(|thread| {
            thread
                .try_borrow_mut()
                .ok()
                .map(|mut thread| f(&mut thread))
        })
        .ok()
        .flatten()
}
