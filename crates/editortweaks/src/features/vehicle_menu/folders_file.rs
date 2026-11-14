//! The folders file, `SavedRemastered\Vehicles\EditorTweaks-Folders.toml`: beside the vehicles, an
//! unknown file to the game (its local query only walks folders). Read once at start, written on
//! every change (a small file: written whole, through a temporary file so a crash never leaves half
//! of it). The first write of a session first copies the file as it was to
//! `EditorTweaks-Folders.toml.bak`, so a bad session can be undone by hand.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use vehicle_folders::Store;

use crate::prelude::*;

const FILE_NAME: &str = "EditorTweaks-Folders.toml";

/// The store read from the file, and whether it may be written back.
pub struct Loaded {
    pub store: Store,
    /// False when the file exists but couldn't be read as TOML: it's left alone (never replaced by
    /// an empty store) until the player fixes or removes it.
    pub writable: bool,
}

/// The game's vehicles folder in its local data, where the file goes.
fn vehicles_dir() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(local).join(r"BrickRigs\SavedRemastered\Vehicles"))
}

/// Where the file is.
fn path() -> Option<PathBuf> {
    Some(vehicles_dir()?.join(FILE_NAME))
}

/// Reads the file: an empty store if there is none yet. What couldn't be read is logged.
pub fn load() -> Loaded {
    let Some(path) = path() else {
        log!("vehicle folders: no LOCALAPPDATA, folders aren't kept");
        return Loaded {
            store: Store::default(),
            writable: false,
        };
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Loaded {
                store: Store::default(),
                writable: true,
            };
        }
        Err(error) => {
            log!(
                "vehicle folders: {} not read ({error}), left alone",
                path.display()
            );
            return Loaded {
                store: Store::default(),
                writable: false,
            };
        }
    };
    match Store::parse(&text) {
        Ok(parsed) => {
            for warning in &parsed.warnings {
                log!("vehicle folders: {warning}");
            }
            Loaded {
                store: parsed.store,
                writable: true,
            }
        }
        Err(error) => {
            log!(
                "vehicle folders: {} isn't valid ({error}), left alone: fix or remove it",
                path.display()
            );
            Loaded {
                store: Store::default(),
                writable: false,
            }
        }
    }
}

/// Whether this session's first write made its backup (or found no file to back up).
static BACKED_UP: AtomicBool = AtomicBool::new(false);

/// Writes `store` to the file.
pub fn save(store: &Store) -> Result<(), String> {
    let path = path().ok_or("no LOCALAPPDATA")?;
    if !BACKED_UP.load(Ordering::Relaxed) {
        let backup = path.with_extension("toml.bak");
        match std::fs::copy(&path, &backup) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            // Without the backup, nothing is written: the file stays as the last session left it.
            Err(error) => return Err(format!("{} not written: {error}", backup.display())),
        }
        BACKED_UP.store(true, Ordering::Relaxed);
    }
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, store.to_toml())
        .map_err(|error| format!("{} not written: {error}", temporary.display()))?;
    // Replaces the old file in one step (`MoveFileEx` with replace).
    std::fs::rename(&temporary, &path)
        .map_err(|error| format!("{} not replaced: {error}", path.display()))
}
