//! Changing the references from the panel: adding one, linking or copying its file, removing it,
//! and writing the sidecar. An unsaved vehicle has no folder yet: its references stay in memory,
//! as links, until its first save writes them (with the copies asked for).

use std::path::{Path, PathBuf};

use model_io::readers::Format;
use model_io::sidecar::{self, Kind, Reference, Sidecar, SourceMode};

use super::{files, received};

/// A new reference of `kind` for the file at `path`, with a new id, a link or a copy as `mode`
/// asks (a copy only once there's a folder). Returns its id.
pub fn add(
    sidecar: &mut Sidecar,
    folder: Option<&Path>,
    kind: Kind,
    path: &Path,
    mode: SourceMode,
) -> Result<String, String> {
    if !files::is_kind(path, kind) {
        return Err(format!(
            "{}: not a file this kind of reference shows",
            path.display()
        ));
    }
    let id = sidecar.new_id();
    let mut reference = match kind {
        Kind::Image => {
            let mut image = Reference::new_image(id.clone(), path);
            // Opaque: the masked material, which the selection outline works with.
            image.opacity = 1.0;
            image
        }
        Kind::Model => {
            let format = Format::of(path).ok_or("not a model format")?;
            Reference::new_model(id.clone(), path, format.default_settings())
        }
    };
    if mode == SourceMode::Copy {
        reference.source.mode = SourceMode::Copy;
        if let Some(folder) = folder {
            reference.source.copy = Some(copy_of(folder, &reference)?);
        }
    }
    sidecar.references.push(reference);
    Ok(id)
}

/// Links `reference` to its original file or copies the file next to the vehicle. Linking deletes
/// the copy, unless the original is gone (the copy is all there is: it's kept as a fallback).
pub fn set_mode(
    reference: &mut Reference,
    folder: Option<&Path>,
    mode: SourceMode,
) -> Result<(), String> {
    match mode {
        SourceMode::Copy => {
            if let Some(folder) = folder {
                let has_copy = reference
                    .source
                    .copy
                    .as_deref()
                    .and_then(|relative| sidecar::copy_path(folder, relative).ok())
                    .is_some_and(|path| path.is_file());
                if !has_copy {
                    reference.source.copy = Some(copy_of(folder, reference)?);
                }
            }
        }
        SourceMode::Link => {
            if let (Some(folder), Some(relative)) = (folder, reference.source.copy.clone())
                && Path::new(&reference.source.path).is_file()
            {
                sidecar::remove_copy(folder, &relative)?;
                reference.source.copy = None;
            }
        }
    }
    reference.source.mode = mode;
    Ok(())
}

/// Removes the reference `id`, and its copy if the original file is still there.
pub fn remove(sidecar: &mut Sidecar, folder: Option<&Path>, id: &str) -> Result<(), String> {
    let Some(index) = sidecar.references.iter().position(|known| known.id == id) else {
        return Ok(());
    };
    let reference = sidecar.references.remove(index);
    if let (Some(folder), Some(relative)) = (folder, &reference.source.copy)
        && Path::new(&reference.source.path).is_file()
    {
        sidecar::remove_copy(folder, relative)?;
    }
    Ok(())
}

/// Going from the references `from` to `to` by an undo or a redo: removes the copies only `from`
/// uses, unless their original files are gone, as [`remove`] does. [`make_missing_copies`] makes
/// them again if the step is undone in turn. What couldn't be removed is told.
pub fn remove_copies_left(from: &[Reference], to: &[Reference], folder: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    for reference in from {
        let Some(relative) = &reference.source.copy else {
            continue;
        };
        if to
            .iter()
            .any(|kept| kept.source.copy.as_ref() == Some(relative))
            || !Path::new(&reference.source.path).is_file()
        {
            continue;
        }
        if let Err(problem) = sidecar::remove_copy(folder, relative) {
            problems.push(format!("{}: {problem}", reference.name));
        }
    }
    problems
}

/// Makes the copies asked for while there was no folder (or that went missing).
pub fn make_missing_copies(sidecar: &mut Sidecar, folder: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    for reference in &mut sidecar.references {
        if reference.source.mode != SourceMode::Copy {
            continue;
        }
        if let Err(problem) = set_mode(reference, Some(folder), SourceMode::Copy) {
            problems.push(format!("{}: {problem}", reference.name));
        }
    }
    problems
}

/// Copies the original file of `reference` into `folder`.
fn copy_of(folder: &Path, reference: &Reference) -> Result<String, String> {
    sidecar::make_copy(
        folder,
        &reference.id,
        &PathBuf::from(&reference.source.path),
        // A coop host's reference is under its level's limits, and has no other file to fall back on.
        if received::is_received(&reference.source.path) {
            u64::MAX
        } else {
            sidecar::DEFAULT_COPY_LIMIT
        },
    )
    // Companion files left out (outside the model's folder) aren't reported yet.
    .map(|copied| copied.relative)
}
