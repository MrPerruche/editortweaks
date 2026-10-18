//! The files a model file points to, so that a copy of the model can take them along: the `.bin`
//! and images of a `.gltf`, the `.mtl` of an `.obj` and the textures of those materials. Formats
//! that are one file (`.glb`, `.stl`, images) have none.
//!
//! The readers resolve these names the same way (relative to the model's folder), so only files
//! inside that folder are listed: the model comes from a file the user may have received, and a
//! copy must not pull in `..\..\Documents\secret.png`. Names that point outside are reported
//! apart, so the caller can tell the user what the copy lacks.

use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::readers::gltf::percent_decode;

/// What a model file points to.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Companions {
    /// The files that exist, relative to the model's folder with forward slashes, in the order
    /// they are mentioned, each once.
    pub found: Vec<String>,
    /// Names that point outside the model's folder (absolute, a drive, `..`), as written.
    pub skipped: Vec<String>,
}

/// The companions of the model at `model`, by its extension. Anything unreadable has none: the
/// reader reports it when the model is loaded.
pub fn of(model: &Path) -> Companions {
    let folder = model.parent().unwrap_or(Path::new(""));
    let main = model
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut finder = Finder {
        folder,
        main,
        companions: Companions::default(),
    };
    let extension = model.extension().map(|e| e.to_ascii_lowercase());
    match extension.as_ref().and_then(|e| e.to_str()) {
        Some("gltf") => finder.gltf(model),
        Some("obj") => finder.obj(model),
        _ => {}
    }
    finder.companions
}

/// Whether a name points to an existing file inside the model's folder.
enum Outcome {
    Found,
    Missing,
    Outside,
}

struct Finder<'a> {
    folder: &'a Path,
    /// The model's own file name, never its own companion.
    main: String,
    companions: Companions,
}

impl Finder<'_> {
    /// Looks for the file `name` and, if it is there, adds it.
    fn consider(&mut self, name: &str) -> Outcome {
        if name.trim().is_empty() {
            return Outcome::Missing;
        }
        let Some(relative) = inside_folder(name) else {
            return Outcome::Outside;
        };
        if relative.eq_ignore_ascii_case(&self.main) || !self.folder.join(&relative).is_file() {
            return Outcome::Missing;
        }
        if !self.companions.found.contains(&relative) {
            self.companions.found.push(relative);
        }
        Outcome::Found
    }

    fn skip(&mut self, name: &str) {
        if !self
            .companions
            .skipped
            .iter()
            .any(|skipped| skipped == name)
        {
            self.companions.skipped.push(name.to_string());
        }
    }

    /// Looks for the one file the `candidates` (best first) can stand for and adds the first that
    /// exists. If none does, the last that points outside is skipped.
    fn consider_one_of(&mut self, candidates: &[&str]) {
        let mut outside = None;
        for &candidate in candidates {
            match self.consider(candidate) {
                Outcome::Found => return,
                Outcome::Outside => outside = Some(candidate),
                Outcome::Missing => {}
            }
        }
        if let Some(name) = outside {
            self.skip(name);
        }
    }

    // --- glTF ---

    /// `buffers[].uri` and `images[].uri`, except `data:` URIs.
    fn gltf(&mut self, path: &Path) {
        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        let Ok(document) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return;
        };
        for list in ["buffers", "images"] {
            let uris = document[list]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| entry["uri"].as_str())
                .filter(|uri| !uri.starts_with("data:"));
            for uri in uris {
                // A name that isn't valid percent-encoding is one the reader refuses too.
                if let Ok(name) = percent_decode(uri) {
                    self.consider_one_of(&[&name]);
                }
            }
        }
    }

    // --- OBJ ---

    /// The `.mtl` files of `mtllib` lines, then the texture maps those name.
    fn obj(&mut self, path: &Path) {
        let mut libraries = Vec::new();
        for line in lines(path) {
            let Some(("mtllib", names)) = keyword_and_rest(&line) else {
                continue;
            };
            libraries.extend(self.material_libraries(names));
        }
        for library in libraries {
            for line in lines(&self.folder.join(&library)) {
                if let Some((keyword, rest)) = keyword_and_rest(&line)
                    && is_texture_map(keyword)
                {
                    self.consider_one_of(&texture_candidates(rest));
                }
            }
        }
    }

    /// The `.mtl` files a `mtllib` line names: the rest of the line is one file name (tobj reads
    /// it so, and names with spaces are common), else each word is a file.
    fn material_libraries(&mut self, names: &str) -> Vec<String> {
        if let Outcome::Found = self.consider(names) {
            return inside_folder(names).into_iter().collect();
        }
        let mut libraries = Vec::new();
        for name in names.split_whitespace() {
            match self.consider(name) {
                Outcome::Found => libraries.extend(inside_folder(name)),
                Outcome::Outside => self.skip(name),
                Outcome::Missing => {}
            }
        }
        libraries
    }
}

/// `name` as a path relative to the model's folder with forward slashes, or `None` if it leaves
/// that folder: absolute, with a drive or scheme (`:`), or with `..`. Backslashes separate folders
/// (OBJ files written on Windows use them) and `.` parts and empty ones are dropped.
fn inside_folder(name: &str) -> Option<String> {
    let name = name.replace('\\', "/");
    if name.starts_with('/') || name.contains([':', '\0']) {
        return None;
    }
    let mut parts = Vec::new();
    for part in name.split('/') {
        match part {
            ".." => return None,
            "" | "." => {}
            _ => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The lines of a text file, leniently: invalid UTF-8 is replaced and a read error ends the file.
fn lines(path: &Path) -> impl Iterator<Item = String> {
    let reader = std::fs::File::open(path).ok().map(BufReader::new);
    reader
        .into_iter()
        .flat_map(|reader| reader.split(b'\n').map_while(Result::ok))
        .map(|line| {
            String::from_utf8_lossy(&line)
                .trim_start_matches('\u{feff}')
                .to_string()
        })
}

/// The first word of an OBJ/MTL line and the rest of it, trimmed. `None` for blank lines.
fn keyword_and_rest(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    let (keyword, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    (!keyword.is_empty()).then_some((keyword, rest.trim()))
}

/// MTL statements that name an image: `map_Kd`, `map_Bump`, ... and the older `bump`, `disp`,
/// `decal`, `refl` and `norm`.
fn is_texture_map(keyword: &str) -> bool {
    let keyword = keyword.to_ascii_lowercase();
    keyword.starts_with("map_")
        || matches!(
            keyword.as_str(),
            "bump" | "disp" | "decal" | "refl" | "norm"
        )
}

/// What a texture statement's argument can name, best first. The reader takes all of it as the
/// file name (spaces included); options before the name (`-bm 1.0 -s 1 1 1 wood.png`) are dropped
/// word by word otherwise.
fn texture_candidates(rest: &str) -> Vec<&str> {
    let starts = rest
        .char_indices()
        .filter(|&(at, c)| {
            !c.is_whitespace() && (at == 0 || rest[..at].ends_with(char::is_whitespace))
        })
        .map(|(at, _)| at);
    starts.map(|at| &rest[at..]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// An empty folder of our own, deleted when dropped (even if the test fails).
    struct TempFolder(PathBuf);

    impl TempFolder {
        fn new(test: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("model_io_companions_{test}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        /// Writes `contents` to `name` (folders in it are created) and returns its path.
        fn write(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, contents).unwrap();
            path
        }
    }

    impl Drop for TempFolder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn found(model: &Path) -> Vec<String> {
        of(model).found
    }

    #[test]
    fn gltf_buffers_and_images() {
        let folder = TempFolder::new("gltf");
        let model = folder.write(
            "scene.gltf",
            r#"{
                "buffers": [
                    {"byteLength": 4, "uri": "scene.bin"},
                    {"byteLength": 4, "uri": "data:application/octet-stream;base64,AAAAAA=="}
                ],
                "images": [
                    {"uri": "textures/base%20color.png"},
                    {"bufferView": 0},
                    {"uri": "textures/base%20color.png"},
                    {"uri": "missing.png"}
                ]
            }"#,
        );
        folder.write("scene.bin", "bin");
        folder.write("textures/base color.png", "png");
        folder.write("unrelated.txt", "no");

        let companions = of(&model);
        assert_eq!(companions.found, ["scene.bin", "textures/base color.png"]);
        assert!(companions.skipped.is_empty());
    }

    #[test]
    fn gltf_uris_outside_the_folder_are_skipped() {
        let folder = TempFolder::new("gltf_outside");
        let model = folder.write(
            "sub/scene.gltf",
            r#"{"buffers": [{"uri": "../secret.bin"}, {"uri": "C:/x.bin"}, {"uri": "ok.bin"}]}"#,
        );
        folder.write("secret.bin", "secret");
        folder.write("sub/ok.bin", "ok");

        let companions = of(&model);
        assert_eq!(companions.found, ["ok.bin"]);
        assert_eq!(companions.skipped, ["../secret.bin", "C:/x.bin"]);
    }

    #[test]
    fn broken_or_self_contained_models_have_none() {
        let folder = TempFolder::new("none");
        assert_eq!(
            of(&folder.write("a.gltf", "{ not json")),
            Companions::default()
        );
        assert_eq!(of(&folder.write("b.glb", "glTF")), Companions::default());
        assert_eq!(of(&folder.write("c.stl", "solid")), Companions::default());
        assert_eq!(of(&folder.write("d.png", "png")), Companions::default());
        assert_eq!(of(&folder.0.join("missing.obj")), Companions::default());
    }

    #[test]
    fn obj_materials_and_textures() {
        let folder = TempFolder::new("obj");
        let model = folder.write("car.obj", "mtllib car.mtl\nv 0 0 0\nusemtl paint\n");
        folder.write(
            "car.mtl",
            "newmtl paint\nKd 1 1 1\nmap_Kd paint.png\nmap_Bump -bm 0.5 normal maps\\n.png\n\
             bump -s 1 1 1 relief.jpg\nmap_d paint.png\nMAP_Ks spec.png\nnorm n.png\n\
             map_Ka missing.png\nillum 2\n",
        );
        for name in [
            "paint.png",
            "normal maps/n.png",
            "relief.jpg",
            "spec.png",
            "n.png",
        ] {
            folder.write(name, "img");
        }

        let companions = of(&model);
        assert_eq!(
            companions.found,
            [
                "car.mtl",
                "paint.png",
                "normal maps/n.png",
                "relief.jpg",
                "spec.png",
                "n.png"
            ]
        );
        assert!(companions.skipped.is_empty());
    }

    #[test]
    fn obj_material_library_names() {
        let folder = TempFolder::new("obj_mtllib");
        // The whole line is one name with spaces.
        let spaced = folder.write("a.obj", "mtllib my materials.mtl\n");
        folder.write("my materials.mtl", "newmtl x\nmap_Kd x.png\n");
        folder.write("x.png", "img");
        assert_eq!(found(&spaced), ["my materials.mtl", "x.png"]);

        // Several libraries on one line.
        let several = folder.write("b.obj", "mtllib one.mtl two.mtl gone.mtl\n");
        folder.write("one.mtl", "newmtl x\n");
        folder.write("two.mtl", "newmtl y\nmap_Kd y.png\n");
        folder.write("y.png", "img");
        assert_eq!(found(&several), ["one.mtl", "two.mtl", "y.png"]);

        // Missing library: nothing, and no error.
        let missing = folder.write("c.obj", "mtllib nope.mtl\n");
        assert_eq!(of(&missing), Companions::default());
    }

    #[test]
    fn obj_names_outside_the_folder_are_skipped() {
        let folder = TempFolder::new("obj_outside");
        let model = folder.write("sub/car.obj", "mtllib ../shared.mtl\nmtllib car.mtl\n");
        folder.write("shared.mtl", "newmtl x\nmap_Kd shared.png\n");
        folder.write("shared.png", "img");
        folder.write(
            "sub/car.mtl",
            "newmtl x\nmap_Kd ../shared.png\nmap_Bump -bm 1 C:/tex/n.png\nmap_Ks ok.png\n",
        );
        folder.write("sub/ok.png", "img");

        let companions = of(&model);
        assert_eq!(companions.found, ["car.mtl", "ok.png"]);
        assert_eq!(
            companions.skipped,
            ["../shared.mtl", "../shared.png", "C:/tex/n.png"]
        );
    }

    #[test]
    fn names_inside_the_folder() {
        assert_eq!(inside_folder("a.png").as_deref(), Some("a.png"));
        assert_eq!(
            inside_folder("./a/./b\\c.png").as_deref(),
            Some("a/b/c.png")
        );
        for outside in [
            "../a",
            "a/../b",
            "/a",
            "\\a",
            "C:\\a",
            "C:a",
            "file:///a",
            "..",
        ] {
            assert_eq!(inside_folder(outside), None, "{outside}");
        }
        assert_eq!(inside_folder("."), None);
    }
}
