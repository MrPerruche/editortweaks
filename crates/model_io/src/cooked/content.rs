//! The game's content as its pak files hold it: the base game's, and every mod's (mods are plugins
//! in `<project>/Mods/<Mod>/Content/Paks`), found by package name the way the engine mounts them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::package::Package;
use super::pak::Pak;
use super::static_mesh::{self, Triangles};

/// Where paks are, below the project's folder: its own, and plugins' (mods' and others').
const PAK_FOLDERS: [&str; 3] = ["Content/Paks", "Mods", "Plugins"];

/// How deep below those folders paks are looked for.
const MAX_DEPTH: usize = 6;

/// The paks of a game, and which holds each package.
pub struct Content {
    paks: Vec<Pak>,
    /// Package name (e.g. `/Game/Bricks/Foo`) -> the pak holding it and its file name without the
    /// extension.
    packages: HashMap<String, (usize, String)>,
}

impl Content {
    /// The paks of the project in `project` (the folder its `Content` and `Mods` are in, in the
    /// game's folder), and what couldn't be read (pak and reason).
    pub fn open(project: &Path) -> (Self, Vec<(PathBuf, String)>) {
        let mut paths = Vec::new();
        for folder in PAK_FOLDERS {
            find_paks(&project.join(folder), MAX_DEPTH, &mut paths);
        }
        // Patches (`_P`) override the paks they patch, like the engine mounts them.
        paths.sort_by_key(|path| is_patch(path));
        let mut content = Self {
            paks: Vec::new(),
            packages: HashMap::new(),
        };
        let mut failed = Vec::new();
        for path in paths {
            match Pak::open(&path) {
                Ok(pak) => content.add(pak),
                Err(reason) => failed.push((path, reason)),
            }
        }
        (content, failed)
    }

    fn add(&mut self, pak: Pak) {
        let index = self.paks.len();
        for file in pak.files() {
            if let Some(base) = file.strip_suffix(".uasset")
                && let Some(package) = package_name(base)
            {
                self.packages.insert(package, (index, base.to_owned()));
            }
        }
        self.paks.push(pak);
    }

    pub fn pak_count(&self) -> usize {
        self.paks.len()
    }

    pub fn package_count(&self) -> usize {
        self.packages.len()
    }

    /// The triangles of LOD `level` of the static mesh `object` in the package `package` (e.g.
    /// `/CombatAdvanced/Bricks/Clock/Clock` and `Clock`), or of its least detailed LOD if it has
    /// fewer, and which LOD that is (see [`static_mesh::level`]).
    pub fn static_mesh(
        &self,
        package: &str,
        object: &str,
        level: usize,
    ) -> Result<(usize, Triangles), String> {
        let (pak, base) = self
            .packages
            .get(package)
            .ok_or_else(|| format!("{package} isn't in the game's paks"))?;
        let pak = &self.paks[*pak];
        let asset = pak.read(&format!("{base}.uasset"))?;
        let exports_name = format!("{base}.uexp");
        let exports = if pak.contains(&exports_name) {
            Some(pak.read(&exports_name)?)
        } else {
            None
        };
        let header = Package::parse(&asset).map_err(|reason| format!("{package}: {reason}"))?;
        let export = header
            .export(object, "StaticMesh")
            .ok_or_else(|| format!("{package} has no static mesh {object}"))?;
        let data = header.data(export, &asset, exports.as_deref())?;
        static_mesh::level(&header, data, level).map_err(|reason| format!("{package}: {reason}"))
    }

    /// Every static mesh in the paks (package name, object name), for checking they all read.
    pub fn static_meshes(&self) -> Vec<(String, String)> {
        let mut meshes = Vec::new();
        for (package, (pak, base)) in &self.packages {
            let Ok(asset) = self.paks[*pak].read(&format!("{base}.uasset")) else {
                continue;
            };
            let Ok(header) = Package::parse(&asset) else {
                continue;
            };
            for export in &header.exports {
                if export.outer == 0 && header.class_name(export.class) == Some("StaticMesh") {
                    meshes.push((package.clone(), export.name.clone()));
                }
            }
        }
        meshes.sort_unstable();
        meshes
    }
}

/// The package a file is (its path in the paks, without the extension), by where the engine mounts
/// content: `<Project>/Content/X` is `/Game/X`, `Engine/Content/X` is `/Engine/X`, a plugin's
/// `.../<Plugin>/Content/X` is `/<Plugin>/X`.
pub fn package_name(file: &str) -> Option<String> {
    let (before, rest) = file.split_once("/Content/")?;
    if rest.is_empty() {
        return None;
    }
    let root = match before.rsplit_once('/') {
        Some((_, plugin)) => plugin,
        None if before == "Engine" => "Engine",
        None => "Game",
    };
    Some(format!("/{root}/{rest}"))
}

fn is_patch(path: &Path) -> bool {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.ends_with("_P"))
}

/// Adds the `.pak` files in `folder` and below (`depth` levels) to `paks`.
fn find_paks(folder: &Path, depth: usize, paks: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            if depth > 0 {
                find_paks(&path, depth - 1, paks);
            }
        } else if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pak"))
        {
            paks.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cooked::package::tests::header;
    use crate::cooked::pak::tests::version_3;
    use crate::cooked::static_mesh::tests::{NAMES, export};

    #[test]
    fn package_names() {
        let name = |file| package_name(file);
        assert_eq!(
            name("BrickRigs/Content/Bricks/Foo").as_deref(),
            Some("/Game/Bricks/Foo")
        );
        assert_eq!(
            name("BrickRigs/Mods/Combat/Content/Bricks/Clock/Clock").as_deref(),
            Some("/Combat/Bricks/Clock/Clock")
        );
        assert_eq!(
            name("Engine/Content/Basic/Cube").as_deref(),
            Some("/Engine/Basic/Cube")
        );
        assert_eq!(
            name("Engine/Plugins/Foo/Content/Bar").as_deref(),
            Some("/Foo/Bar")
        );
        assert_eq!(name("BrickRigs/AssetRegistry"), None);
    }

    #[test]
    fn reads_a_mod_mesh() {
        let positions = [[0.0, 0.0, 0.0], [5.0, 0.0, 0.0], [0.0, 5.0, 0.0]];
        let data = export(&positions, &[0, 1, 2], false);
        let asset = header(
            &NAMES,
            &[("Class", 0, "StaticMesh")],
            &[(-1, "Mesh", data.len() as u64)],
        );
        let project = std::env::temp_dir().join(format!("model_io-content-{}", std::process::id()));
        let paks = project.join("Mods/Foo/Content/Paks/WindowsNoEditor");
        std::fs::create_dir_all(&paks).unwrap();
        std::fs::write(
            paks.join("Foo.pak"),
            version_3(&[
                ("Game/Mods/Foo/Content/Meshes/Mesh.uasset", &asset, false),
                ("Game/Mods/Foo/Content/Meshes/Mesh.uexp", &data, true),
            ]),
        )
        .unwrap();
        let (content, failed) = Content::open(&project);
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(content.pak_count(), 1);
        let (level, mesh) = content.static_mesh("/Foo/Meshes/Mesh", "Mesh", 2).unwrap();
        assert_eq!((level, mesh.vertices), (0, positions.to_vec()));
        assert_eq!(
            content.static_meshes(),
            [("/Foo/Meshes/Mesh".to_owned(), "Mesh".to_owned())]
        );
        assert!(content.static_mesh("/Foo/Meshes/Mesh", "Other", 0).is_err());
        assert!(content.static_mesh("/Foo/Missing", "Mesh", 0).is_err());
        std::fs::remove_dir_all(&project).unwrap();
    }

    /// Every static mesh of an installed game reads, LOD 0 and its least detailed LOD:
    /// `EDITORTWEAKS_PROJECT` is its project folder
    /// (e.g. `.../Brick Rigs/BrickRigs`). `cargo test -p model_io -- --ignored installed`.
    #[test]
    #[ignore = "needs an installed game"]
    fn installed_game_meshes() {
        let project =
            PathBuf::from(std::env::var("EDITORTWEAKS_PROJECT").expect("EDITORTWEAKS_PROJECT"));
        let opening = std::time::Instant::now();
        let (content, failed) = Content::open(&project);
        let opened = opening.elapsed();
        assert!(failed.is_empty(), "{failed:?}");
        let meshes = content.static_meshes();
        let mut failures = Vec::new();
        let mut with_lods = 0;
        for (package, object) in &meshes {
            for level in [0, usize::MAX] {
                match content.static_mesh(package, object, level) {
                    // Maps' merged meshes can be hundreds of megabytes: no brick's is.
                    Err(reason) if reason.contains("too big") => {}
                    Err(reason) => failures.push(reason),
                    Ok((read, _)) => with_lods += usize::from(read > 0),
                }
            }
        }
        println!(
            "opened in {opened:?}: {} paks, {} packages, {} static meshes ({with_lods} with LODs),              {} failed",
            content.pak_count(),
            content.package_count(),
            meshes.len(),
            failures.len()
        );
        assert!(
            failures.is_empty(),
            "{:#?}",
            &failures[..failures.len().min(20)]
        );
    }
}
