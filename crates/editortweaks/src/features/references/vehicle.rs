//! Which folder the open vehicle is saved in, and noticing when its sidecar or a reference's file
//! changes on disk.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use model_io::sidecar::{self, Reference};

use crate::game::editor_file::FileInfo;

/// Where the game saves vehicles: `SavedRemastered\Vehicles` in its local data, one folder per
/// vehicle named after its local id.
fn vehicles_folder() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(local).join(r"BrickRigs\SavedRemastered\Vehicles"))
}

/// The folder of the vehicle `info` describes, if it's saved on this computer.
pub fn folder_of(info: &FileInfo) -> Option<PathBuf> {
    let id = info.local_id.as_str();
    if id.is_empty() {
        return None;
    }
    let path = Path::new(id);
    let folder = if path.is_absolute() {
        // A full path: the folder itself, or the folder of the vehicle's file.
        if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent()?.to_path_buf()
        }
    } else if id.contains(['/', '\\', ':']) || id == "." || id == ".." {
        return None;
    } else {
        vehicles_folder()?.join(id)
    };
    folder.is_dir().then_some(folder)
}

/// Whether `folder` was created after `time` (give or take a moment for clocks and rounding): the
/// game just saved a vehicle into it, rather than opening one saved before.
pub fn created_after(folder: &Path, time: SystemTime) -> bool {
    const SLACK: Duration = Duration::from_secs(2);
    std::fs::metadata(folder)
        .and_then(|metadata| metadata.created())
        .is_ok_and(|created| created + SLACK >= time)
}

/// A file's size and modification time: when either changes, the file changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    bytes: u64,
    modified: Option<SystemTime>,
}

/// The stamp of `path`, `None` if there's no file.
pub fn stamp(path: &Path) -> Option<Stamp> {
    let metadata = std::fs::metadata(path).ok()?;
    metadata.is_file().then(|| Stamp {
        bytes: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

/// The sidecar's path in `folder`.
pub fn sidecar_path(folder: &Path) -> PathBuf {
    folder.join(sidecar::FILE_NAME)
}

/// The file `reference` shows, best candidate first (relative paths are relative to the vehicle's
/// folder; an unsaved vehicle has none, so only full paths count), with its stamp; `None` if none
/// of them is there.
pub fn find_file(reference: &Reference, folder: Option<&Path>) -> Option<(PathBuf, Stamp)> {
    let candidates = match folder {
        Some(folder) => reference.file_candidates(folder),
        None => vec![PathBuf::from(&reference.source.path)],
    };
    candidates
        .into_iter()
        .filter(|candidate| !candidate.as_os_str().is_empty())
        .filter_map(|candidate| match folder {
            _ if candidate.is_absolute() => Some(candidate),
            Some(folder) => Some(folder.join(candidate)),
            None => None,
        })
        .find_map(|path| stamp(&path).map(|stamp| (path, stamp)))
}
