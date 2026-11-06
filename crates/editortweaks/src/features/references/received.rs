//! The session's references in coop (`docs/coop-references.md`): what crosses between this feature
//! and `features/coop/share`.
//!
//! A subhost's references are read from here to be shared ([`shared_now`]). A member's are handed
//! here ([`show`]): as long as a list is set they're shown instead of the open vehicle's sidecar,
//! read-only, and the sidecar isn't read or written, except when the game saves the vehicle: they're
//! then kept with it, copied into its folder ([`seal`]). Files of received references live in
//! [`folder`] until then, so they show through the same code as any other reference.
//!
//! Hiding stays local: the Hide key and the Show references row never go through here.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use model_io::sidecar::{Reference, Sidecar};

use super::vehicle::{self, Stamp};
use super::{STATE, edit};
use crate::prelude::*;

/// The session's references, while a member receives any.
static LIST: Mutex<Option<Vec<Reference>>> = Mutex::new(None);

/// What the panel says about them: "3 of 5 received, 1 simplified", say.
static STATUS: Mutex<String> = Mutex::new(String::new());

/// Where received references' files are kept during a session.
pub fn folder() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(local).join(r"BrickRigs\SavedRemastered\EditorTweaks\CoopReferences"))
}

/// Member: shows `references` instead of the open vehicle's, from the next poll on. Their files
/// are in [`folder`]: `source.path` full, `source.mode` a copy, so a save copies them along.
pub fn show(references: Vec<Reference>, status: String) {
    *LIST.lock().unwrap_or_else(PoisonError::into_inner) = Some(references);
    *STATUS.lock().unwrap_or_else(PoisonError::into_inner) = status;
}

/// Member: back to the open vehicle's references (the session ended, or the player turned them off).
pub fn stop() {
    *LIST.lock().unwrap_or_else(PoisonError::into_inner) = None;
    STATUS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}

/// The list set by [`show`], if any.
pub(super) fn list() -> Option<Vec<Reference>> {
    LIST.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// What the panel says about the received references.
pub(super) fn status() -> String {
    STATUS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Whether the session's references are shown now.
pub(super) fn active() -> bool {
    LIST.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_some()
}

/// A subhost's reference to share: as the sidecar has it, with its file found.
pub struct Shared {
    pub reference: Reference,
    pub file: PathBuf,
    pub stamp: Stamp,
}

/// The references of the open vehicle whose files are there; `None` while busy (ask again later).
/// Game thread only.
pub fn shared_now() -> Option<Vec<Shared>> {
    let state = STATE.try_lock().ok()?;
    Some(
        state
            .sidecar
            .references
            .iter()
            .filter_map(|reference| {
                let (file, stamp) = vehicle::find_file(reference, state.folder.as_deref())?;
                Some(Shared {
                    reference: reference.clone(),
                    file,
                    stamp,
                })
            })
            .collect(),
    )
}

/// Whether `path` is a received reference's file.
pub(super) fn is_received(path: &str) -> bool {
    folder().is_some_and(|folder| Path::new(path).starts_with(folder))
}

/// `sidecar` as it's written with a vehicle: a received reference's copy is all it has, as its file
/// in [`folder`] doesn't last.
pub(super) fn seal(sidecar: &Sidecar) -> Sidecar {
    let mut sealed = sidecar.clone();
    for reference in &mut sealed.references {
        if is_received(&reference.source.path) {
            reference.source.path.clear();
        }
    }
    sealed
}

/// The session's vehicle was just written into the autosave folder `to` (by `coop::backup`): writes
/// the references shown along, marked with it. Game thread only.
pub fn write_along(to: &Path) {
    let Ok(state) = STATE.try_lock() else {
        crate::prelude::log!("autosaved: busy, references not written along");
        return;
    };
    let from = state.folder.clone().unwrap_or_else(|| to.to_path_buf());
    let sidecar = if from == to {
        Ok(state.sidecar.clone())
    } else {
        state.sidecar.carried_over(&from, to)
    };
    let result = sidecar.and_then(|mut sidecar| {
        for problem in edit::make_missing_copies(&mut sidecar, to) {
            log!("{problem}");
        }
        let mut sealed = seal(&sidecar);
        sealed.vehicle = vehicle::mark_for(to);
        sealed.write(to)?;
        Ok(sealed.references.len())
    });
    match result {
        Ok(count) => log!(
            "autosaved: {count} references written along to {}",
            to.display()
        ),
        Err(reason) => log!("autosaved: references not written along: {reason}"),
    }
}
