//! Backups of what coop replaces or changes: the joiner's own vehicle before the session's replaces
//! it, and the session's vehicle every few minutes while it changes. Vehicle files (the game's
//! own bytes, as in a saved vehicle's `Vehicle.brv`) in `SavedRemastered\EditorTweaks\
//! CoopBackups`, named by the time (UTC) and what they are; the newest [`KEEP`] are kept. To
//! restore one, copy it over the `Vehicle.brv` of a saved vehicle's folder.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::log::coop_log;
use super::{Game, Object, objects};

/// How often the session's vehicle is backed up, if it changed.
pub const EVERY: Duration = Duration::from_secs(5 * 60);
/// Backups kept: older ones are deleted.
const KEEP: usize = 20;

/// When the session's vehicle was last backed up, and what it was then.
pub struct Backups {
    due: Instant,
    last: Vec<u8>,
}

impl Backups {
    pub fn new() -> Self {
        Self {
            due: Instant::now() + EVERY,
            last: Vec::new(),
        }
    }

    /// Backs the session's vehicle (`editor`'s, of `component`) up if it's time and it changed.
    pub fn tick(&mut self, game: &Game, editor: Object, component: Object) {
        if Instant::now() < self.due {
            return;
        }
        self.due = Instant::now() + EVERY;
        // SAFETY: the editor is live (it's ticking); game thread.
        let objects = unsafe { objects(game, editor) };
        // SAFETY: the editor's live objects.
        match unsafe { game.archive.save(component, &objects) } {
            Ok(bytes) if bytes == self.last => {}
            Ok(bytes) => {
                write(&bytes, "session");
                self.last = bytes;
            }
            Err(reason) => coop_log!("couldn't back the session's vehicle up: {reason}"),
        }
    }
}

/// Backs the vehicle in `editor` (of `component`) up as `what`, before something replaces it.
pub fn now(game: &Game, editor: Object, component: Object, what: &str) {
    // SAFETY: the editor is live (it's ticking); game thread.
    let objects = unsafe { objects(game, editor) };
    if objects.is_empty() {
        return;
    }
    // SAFETY: the editor's live objects.
    match unsafe { game.archive.save(component, &objects) } {
        Ok(bytes) => write(&bytes, what),
        Err(reason) => coop_log!("couldn't back the {what} up: {reason}"),
    }
}

fn folder() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(local).join(r"BrickRigs\SavedRemastered\EditorTweaks\CoopBackups"))
}

/// Writes `bytes` as a new backup called `what`, then deletes the oldest past [`KEEP`].
fn write(bytes: &[u8], what: &str) {
    let Some(folder) = folder() else { return };
    let path = folder.join(format!("{} {what}.brv", timestamp()));
    let written = fs::create_dir_all(&folder).and_then(|()| fs::write(&path, bytes));
    match written {
        Ok(()) => coop_log!("backup of the {what}: {}", path.display()),
        Err(error) => {
            coop_log!("couldn't back the {what} up to {}: {error}", path.display());
            return;
        }
    }
    // Names start with the time, so they sort oldest first.
    let mut backups: Vec<PathBuf> = fs::read_dir(&folder)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "brv"))
        .collect();
    backups.sort();
    let excess = backups.len().saturating_sub(KEEP);
    for old in &backups[..excess] {
        let _ = fs::remove_file(old);
    }
}

/// `YYYY-MM-DD hh-mm-ss`, UTC.
fn timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (year, month, day) = civil_date(seconds / 86_400);
    let time = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}-{:02}-{:02}",
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}

/// The date `days` after 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil_date(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}
