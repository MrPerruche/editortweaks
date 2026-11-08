//! The session's references in coop (`docs/coop-references.md`): what crosses between this feature
//! and `features/coop/share`.
//!
//! A subhost's references are read from here to be shared ([`shared_now`]). A member's are handed
//! here ([`show`]): as long as a list is set they're shown instead of the open vehicle's sidecar,
//! and the sidecar isn't read or written, except when the game saves the vehicle: they're then kept
//! with it, copied into its folder ([`seal`]). Files of received references live in [`folder`]
//! until then, so they show through the same code as any other reference.
//!
//! A member may move, change the look of and remove the host's references (not add any: their files
//! are the host's). What changed since it was last sent goes to the host ([`edits_between`],
//! [`take_edits`]), which makes it as its own ([`apply_edits`]); its next lists tell everyone.
//!
//! Hiding stays local: the Hide key, the Show references row and opacity 0 never go through here.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use ::coop::references::{Edit, ImageLook, Look};
use model_io::plane::ImagePivot;
use model_io::sidecar::{Kind, Reference, Sidecar};

use super::vehicle::{self, Stamp};
use super::{STATE, edit};
use crate::prelude::*;

/// The session's references, while a member receives any.
static LIST: Mutex<Option<Vec<Reference>>> = Mutex::new(None);

/// What the panel says about them: "3 of 5 received, 1 simplified", say.
static STATUS: Mutex<String> = Mutex::new(String::new());

/// Member: changes of the host's references made here, not sent yet.
static OUTBOX: Mutex<Vec<Edit>> = Mutex::new(Vec::new());

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
    OUTBOX
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
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

/// Member: the changes of the host's references made here since this was last asked, oldest first.
pub fn take_edits() -> Vec<Edit> {
    core::mem::take(&mut *OUTBOX.lock().unwrap_or_else(PoisonError::into_inner))
}

/// Member: `edits` are to go to the host.
pub(super) fn send(edits: Vec<Edit>) {
    OUTBOX
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .extend(edits);
}

/// Where `reference` is and how it looks, as a member changes it.
fn look_of(reference: &Reference) -> Look {
    Look {
        position: reference.position,
        rotation: reference.rotation,
        scale: reference.scale,
        mirror: reference.mirror,
        opacity: reference.opacity,
        color: reference.color,
        image: reference.image.as_ref().map(|image| ImageLook {
            width: image.width,
            flip_x: image.flip_x,
            flip_y: image.flip_y,
            double_sided: image.double_sided,
            pixelated: image.pixelated,
            pivot: match image.pivot {
                ImagePivot::Center => 0,
                ImagePivot::Corner => 1,
            },
        }),
    }
}

/// Gives `reference` `look`. Not if `look` is an image's and `reference` isn't one, or the other way
/// round (false then).
fn set_look(reference: &mut Reference, look: &Look) -> bool {
    if look.image.is_some() != (reference.kind == Kind::Image) {
        return false;
    }
    reference.position = look.position;
    reference.rotation = look.rotation;
    reference.scale = look.scale;
    reference.mirror = look.mirror;
    reference.opacity = look.opacity;
    reference.color = look.color;
    if let (Some(image), Some(look)) = (reference.image.as_mut(), &look.image) {
        image.width = look.width;
        image.flip_x = look.flip_x;
        image.flip_y = look.flip_y;
        image.double_sided = look.double_sided;
        image.pixelated = look.pixelated;
        image.pivot = if look.pivot == 1 {
            ImagePivot::Corner
        } else {
            ImagePivot::Center
        };
    }
    true
}

/// Whether `changed` differs from `reference` only in what a member may change of a host's
/// reference: its look, and whether it's visible (local).
pub(super) fn only_look_changed(reference: &Reference, changed: &Reference) -> bool {
    let mut probe = reference.clone();
    probe.visible = changed.visible;
    set_look(&mut probe, &look_of(changed)) && probe == *changed
}

/// Member: the edits going from the references `sent` (as the host has them, as far as we know) to
/// those shown `now`; `sent` becomes what they make. A reference hidden here (opacity 0) keeps the
/// host's opacity: hiding is local. References only `now` has (none: a member adds nothing) are
/// left out.
pub(super) fn edits_between(sent: &mut Vec<Reference>, now: &[Reference]) -> Vec<Edit> {
    let mut edits = Vec::new();
    sent.retain(|known| {
        let kept = now.iter().any(|shown| shown.id == known.id);
        if !kept {
            edits.push(Edit::Remove {
                id: known.id.clone(),
            });
        }
        kept
    });
    for known in sent.iter_mut() {
        let Some(shown) = now.iter().find(|shown| shown.id == known.id) else {
            continue;
        };
        let was = look_of(known);
        let mut look = look_of(shown);
        if look.opacity <= 0.0 {
            look.opacity = was.opacity;
        }
        if look != was && set_look(known, &look) {
            edits.push(Edit::Set {
                id: known.id.clone(),
                look,
            });
        }
    }
    edits
}

/// Subhost: makes a member's `edits` of the open vehicle's references, as the player's own (one
/// step to undo with what follows). An edit naming no reference, or an image's look for a model, is
/// passed over; a reference hidden here keeps its opacity (hiding is local). Returns how many were
/// made (none while showing someone else's references); `None` while busy (ask again later). Game
/// thread only.
pub fn apply_edits(edits: &[Edit]) -> Option<usize> {
    let mut state = match STATE.try_lock() {
        Ok(state) => state,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return None,
    };
    let state = &mut *state;
    if state.received.is_some() {
        return Some(0);
    }
    let folder = state.folder.clone();
    let mut made = 0;
    for edit in edits {
        match edit {
            Edit::Set { id, look } => {
                let Some(reference) = state.sidecar.references.iter_mut().find(|r| r.id == *id)
                else {
                    continue;
                };
                let mut look = look.clone();
                let hidden = !reference.visible || reference.opacity <= 0.0;
                if hidden || look.opacity <= 0.0 {
                    look.opacity = reference.opacity;
                }
                let before = reference.clone();
                if set_look(reference, &look) && *reference != before {
                    made += 1;
                }
            }
            Edit::Remove { id } => {
                if !state.sidecar.references.iter().any(|r| r.id == *id) {
                    continue;
                }
                match edit::remove(&mut state.sidecar, folder.as_deref(), id) {
                    Ok(()) => made += 1,
                    Err(reason) => log!("removing {id} for a member: {reason}"),
                }
                state.selected.retain(|selected| selected != id);
                state.refresh_panel = true;
            }
        }
    }
    if made > 0 {
        state.save();
    }
    Some(made)
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
