//! One reader per model format. Each turns a file into a [`Model`] in the file's own space and
//! winding (see [`Part`](crate::model::Part)); [`read`] picks the reader and checks the result.

pub mod gltf;
pub mod obj;
pub mod stl;

use std::path::Path;

use crate::model::Model;
use crate::space::ImportSettings;

/// The model formats references can be made from (`docs/model-io.md`, "Formats").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// glTF 2.0: `.gltf` (with its `.bin` and image files next to it) or `.glb`.
    Gltf,
    /// Wavefront OBJ, with its `.mtl` if there is one.
    Obj,
    /// STL, binary or text.
    Stl,
}

impl Format {
    /// File extensions (lowercase) and their format.
    pub const EXTENSIONS: [(&'static str, Format); 4] = [
        ("gltf", Format::Gltf),
        ("glb", Format::Gltf),
        ("obj", Format::Obj),
        ("stl", Format::Stl),
    ];

    /// The format of `path`, by its extension.
    pub fn of(path: &Path) -> Option<Format> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        Self::EXTENSIONS
            .iter()
            .find(|(known, _)| *known == extension)
            .map(|(_, format)| *format)
    }

    /// The import settings a new reference of this format starts with.
    pub fn default_settings(self) -> ImportSettings {
        match self {
            Format::Gltf | Format::Obj => ImportSettings::default(),
            Format::Stl => ImportSettings::stl(),
        }
    }

    /// glTF's axes and units are part of the format: the panel doesn't offer them.
    pub fn has_fixed_space(self) -> bool {
        self == Format::Gltf
    }
}

/// Reads the model at `path` in its file's space. Slow for big files: call it off the game thread.
pub fn read(path: &Path) -> Result<Model, String> {
    let format = Format::of(path)
        .ok_or_else(|| format!("{}: not a model format we read", path.display()))?;
    let model = match format {
        Format::Gltf => gltf::read(path),
        Format::Obj => obj::read(path),
        Format::Stl => stl::read(path),
    }
    .map_err(|error| format!("{}: {error}", path.display()))?;
    model
        .validate()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_by_extension() {
        assert_eq!(Format::of(Path::new("a/b/Car.GLB")), Some(Format::Gltf));
        assert_eq!(Format::of(Path::new("car.obj")), Some(Format::Obj));
        assert_eq!(Format::of(Path::new("car.stl")), Some(Format::Stl));
        assert_eq!(Format::of(Path::new("car.fbx")), None);
        assert_eq!(Format::of(Path::new("car")), None);
    }
}
