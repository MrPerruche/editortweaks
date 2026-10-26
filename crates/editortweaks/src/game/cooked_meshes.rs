//! Static meshes' triangles read from the pak files the game loaded them from
//! (`model_io::cooked`), for meshes without complex collision to read in memory
//! (`game::brick_model`): mods' bricks often have only simple collision; and for meshes' less
//! detailed LODs, which aren't in memory either. The game's own paks and
//! every mod's are opened the first time one is needed, once per game. See
//! `docs/re/cooked-content.md`.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

use brickrust::ue::coreuobject::UObjectBase;

use model_io::cooked::{Content, Triangles};

use crate::game::reflection;
use crate::prelude::*;

/// The game's content, or why it can't be read.
static CONTENT: OnceLock<Result<Content, String>> = OnceLock::new();

/// The triangles of `mesh` (a static mesh) as its package file holds them: its LOD `level`, as
/// drawn, or its least detailed one if it has fewer; and which LOD that is.
///
/// # Safety
/// `mesh` must be a live static mesh. Game thread only.
pub unsafe fn triangles(
    mesh: *mut UObjectBase,
    level: usize,
) -> Result<(usize, Triangles), String> {
    // SAFETY: per the contract; a live object's outers are live.
    let (package, name) = unsafe {
        let mut outermost = mesh;
        while !(*outermost).outer_private.is_null() {
            outermost = (*outermost).outer_private.cast_mut();
        }
        (
            reflection::name_text(&(*outermost).name_private),
            reflection::name_text(&(*mesh).name_private),
        )
    };
    let content = CONTENT.get_or_init(open).as_ref()?;
    content.static_mesh(&package, &name, level)
}

/// Opens the paks of the game's project (the folder above `Binaries/Win64`, where the game runs
/// from).
fn open() -> Result<Content, String> {
    let project = project_folder()?;
    let start = Instant::now();
    let (content, failed) = Content::open(&project);
    for (pak, reason) in failed {
        log!("{} can't be read: {reason}", pak.display());
    }
    log!(
        "meshes without complex collision and LODs are read from {} paks ({} packages, opened in {} ms)",
        content.pak_count(),
        content.package_count(),
        start.elapsed().as_millis()
    );
    Ok(content)
}

fn project_folder() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|error| format!("the game's folder: {error}"))?;
    exe.ancestors()
        .nth(3)
        .filter(|project| project.join("Content").is_dir())
        .map(PathBuf::from)
        .ok_or_else(|| format!("no project folder above {}", exe.display()))
}
