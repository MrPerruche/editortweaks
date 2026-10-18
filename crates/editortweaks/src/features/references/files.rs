//! Choosing the files references show: the Windows file dialog, starting in the references'
//! folder (`SavedRemastered\EditorTweaks\References`, made when first opened), a handy place to keep
//! them.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use model_io::readers::Format;
use model_io::sidecar::Kind;

use crate::platform::file_dialog::{self, Filter};

/// Image extensions the game's image import reads (lowercase).
const IMAGE_EXTENSIONS: [&str; 3] = ["png", "jpg", "jpeg"];

/// The references' folder, made if it isn't there.
pub fn references_folder() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    let folder = PathBuf::from(local).join(r"BrickRigs\SavedRemastered\EditorTweaks\References");
    // Fails if it exists already: either way, it's there or can't be.
    let _ = std::fs::create_dir_all(&folder);
    Some(folder)
}

/// Whether `path` can be a reference of `kind`, by its extension.
pub fn is_kind(path: &Path, kind: Kind) -> bool {
    match kind {
        Kind::Image => path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                IMAGE_EXTENSIONS
                    .iter()
                    .any(|known| extension.eq_ignore_ascii_case(known))
            }),
        Kind::Model => Format::of(path).is_some(),
    }
}

/// Opens the file dialog for a file of `kind`, starting in the references' folder; `None` if one
/// is already open.
pub fn browse(kind: Kind) -> Option<Receiver<Option<PathBuf>>> {
    let filters: &[Filter] = match kind {
        Kind::Image => &[Filter {
            name: "Images (PNG, JPEG)",
            patterns: "*.png;*.jpg;*.jpeg",
        }],
        Kind::Model => &[Filter {
            name: "Models (glTF, GLB, OBJ, STL)",
            patterns: "*.gltf;*.glb;*.obj;*.stl",
        }],
    };
    let title = match kind {
        Kind::Image => "Add a reference image",
        Kind::Model => "Add a reference model",
    };
    file_dialog::open_file(title, filters, references_folder().as_deref())
}
