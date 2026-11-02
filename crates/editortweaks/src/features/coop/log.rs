//! Coop's own log, one file per role next to `brickworks.txt`: `editortweaks-coop-host.txt` and
//! `editortweaks-coop-member.txt`. Two game instances on one PC share `brickworks.txt` (each
//! rewrites it when it starts), so each also writes its coop lines to its own file, with the
//! time of day so the two can be read side by side. `et.py log --coop host|member` prints one.

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

static FILE: Mutex<Option<File>> = Mutex::new(None);

/// Bytes of coop lines written this run, and the most there may be. Other players' traffic writes
/// lines (one per edit applied, one per unreadable message), so a hostile one could otherwise fill
/// the disk; a long session writes a few MB at most (about 100 bytes per edit).
static WRITTEN: AtomicUsize = AtomicUsize::new(0);
const MOST: usize = 32 << 20;

/// `coop_log!("format {}", args)`: like `log!`, and into this instance's coop log once it has one.
macro_rules! coop_log {
    ($($arg:tt)*) => {
        $crate::features::coop::log::write(&::std::format!($($arg)*))
    };
}
pub(crate) use coop_log;

/// The game's root folder (where `brickworks.txt` is): the exe is in `BrickRigs\Binaries\Win64`.
fn game_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().nth(4).map(PathBuf::from)
}

/// Starts (or restarts, emptied) the file for `role` ("host" or "member").
pub fn start(role: &str) {
    let Some(root) = game_root() else { return };
    let path = root.join(format!("editortweaks-coop-{role}.txt"));
    let file = File::create(&path).ok();
    if file.is_none() {
        crate::framework::log::write(
            "editortweaks::features::coop",
            &format!("couldn't write {}", path.display()),
        );
    }
    *FILE.lock().unwrap_or_else(PoisonError::into_inner) = file;
}

/// Writes one line to `brickworks.txt` and the coop log. Prefer [`coop_log!`].
pub fn write(line: &str) {
    let size = line.len() + 1;
    let before = WRITTEN.fetch_add(size, Ordering::Relaxed);
    let line = match before + size {
        total if total <= MOST => line,
        // The first line past it says so.
        _ if before <= MOST => "coop log full (32 MB this run): later coop lines are left out",
        _ => return,
    };
    crate::framework::log::write("editortweaks::features::coop", line);
    if let Some(file) = FILE.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
        let _ = writeln!(file, "{} {line}", time_of_day());
    }
}

/// `hh:mm:ss.mmm`, UTC.
fn time_of_day() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = now.as_secs() % 86_400;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60,
        now.subsec_millis()
    )
}
