//! What coop keeps of the vehicles it builds or replaces, as vehicle files (the game's own bytes,
//! as in a saved vehicle's `Vehicle.brv`):
//!
//! - The coop autosave ([`AutoSave`]), a clone of the game's own autosave: a recent copy of the
//!   session's vehicle in a vehicle folder of its own, `Vehicles\CoopAutoSave\Autosave.brv`,
//!   written as soon as the session's vehicle is in the editor, then every [`EVERY`] if it
//!   changed, and as the session ends. The game lists a vehicle folder holding only an
//!   `Autosave.brv` as it lists its own autosave folder (`Vehicles\AutoSave`): "Auto saved file"
//!   and the file's date (BRMK 0x333EAC, `docs/re/coop/README.md`). Each session replaces the
//!   last one's, as the game's autosave does.
//! - Backups of what coop replaces ([`now`]): the joiner's own vehicle before the session's
//!   replaces it, the host's before another is shared. In `SavedRemastered\EditorTweaks\
//!   CoopBackups`, named by the time (UTC) and what they are; the newest [`KEEP`] of each kind are
//!   kept, so a host sharing vehicle after vehicle can't push out the backup of a member's own
//!   vehicle from before joining. To restore one, copy it over the `Vehicle.brv` of a saved
//!   vehicle's folder.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::log::coop_log;
use super::{Game, Object, objects};
use crate::framework::clock;

/// How often the coop autosave is written again, if the vehicle changed.
pub const EVERY: Duration = Duration::from_secs(5 * 60);
/// Backups kept of each kind: older ones are deleted.
const KEEP: usize = 20;
/// The coop autosave's folder among the game's vehicles, and its file (named as the game names
/// its autosaves).
const AUTOSAVE_FOLDER: &str = "CoopAutoSave";
const AUTOSAVE_FILE: &str = "Autosave.brv";

/// The coop autosave of a session: when it's due, and what it holds.
pub struct AutoSave {
    due: Instant,
    last: Vec<u8>,
}

impl AutoSave {
    /// For a session starting: its first tick with the vehicle writes it.
    pub fn new() -> Self {
        Self {
            due: Instant::now(),
            last: Vec::new(),
        }
    }

    /// Each tick the editor holds the session's vehicle: writes it (`editor`'s, of `component`)
    /// when due, if it changed.
    pub fn tick(&mut self, game: &Game, editor: Object, component: Object) {
        if Instant::now() < self.due {
            return;
        }
        self.due = Instant::now() + EVERY;
        self.write(game, editor, component);
    }

    /// The session ends, `editor` holding its vehicle: writes it if it changed (not if it never
    /// was written: the session's vehicle never came).
    pub fn finish(&mut self, game: &Game, editor: Object, component: Object) {
        if !self.last.is_empty() {
            self.write(game, editor, component);
        }
    }

    fn write(&mut self, game: &Game, editor: Object, component: Object) {
        // SAFETY: the editor is live (it's ticking); game thread.
        let objects = unsafe { objects(game, editor) };
        // An empty editor isn't a vehicle to keep.
        if objects.is_empty() {
            return;
        }
        // SAFETY: the editor's live objects.
        match unsafe { game.archive.save(component, &objects) } {
            Ok(bytes) if bytes == self.last => {}
            Ok(bytes) => {
                if write_autosave(&bytes) {
                    self.last = bytes;
                }
            }
            Err(reason) => coop_log!("coop autosave not written: {reason}"),
        }
    }
}

/// Writes `bytes` as the coop autosave: beside it first, then in its place, so the game's vehicle
/// list never reads half a file. Whether it was written.
fn write_autosave(bytes: &[u8]) -> bool {
    let Some(local) = std::env::var_os("LOCALAPPDATA") else {
        return false;
    };
    let folder = PathBuf::from(local)
        .join(r"BrickRigs\SavedRemastered\Vehicles")
        .join(AUTOSAVE_FOLDER);
    let path = folder.join(AUTOSAVE_FILE);
    let part = folder.join(format!("{AUTOSAVE_FILE}.part"));
    let written = fs::create_dir_all(&folder)
        .and_then(|()| fs::write(&part, bytes))
        .and_then(|()| fs::rename(&part, &path));
    match written {
        Ok(()) => {
            coop_log!("coop autosave: {}", path.display());
            true
        }
        Err(error) => {
            coop_log!("coop autosave not written to {}: {error}", path.display());
            let _ = fs::remove_file(&part);
            false
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

/// Writes `bytes` as a new backup called `what`, then deletes the oldest of that kind past
/// [`KEEP`].
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
    // Names are the time, a space and the kind, so they sort oldest first.
    let kind = format!("{what}.brv");
    let mut backups: Vec<PathBuf> = fs::read_dir(&folder)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            name.get(TIMESTAMP_LEN + 1..) == Some(kind.as_str())
        })
        .collect();
    backups.sort();
    let excess = backups.len().saturating_sub(KEEP);
    for old in &backups[..excess] {
        let _ = fs::remove_file(old);
    }
}

/// The length of a [`timestamp`].
const TIMESTAMP_LEN: usize = "YYYY-MM-DD hh-mm-ss".len();

/// `YYYY-MM-DD hh-mm-ss`, UTC.
fn timestamp() -> String {
    let seconds = clock::now();
    let (year, month, day) = clock::civil_date(seconds / 86_400);
    let time = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}-{:02}-{:02}",
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}
