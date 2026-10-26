//! Cooked Unreal Engine 4.27 content read from the game's pak files: enough to find a package and
//! read a static mesh's triangles. The shipping game keeps render meshes on the GPU only, and not
//! every mesh has complex collision to read instead (mods' often don't), but the files the game
//! loaded them from are on disk (see `docs/re/cooked-content.md`).
//!
//! [`Content`] finds the paks and the package; [`package`] reads its header, [`static_mesh`] the
//! mesh's data.

mod bytes;
pub mod content;
pub mod package;
pub mod pak;
pub mod static_mesh;

pub use content::Content;
pub use static_mesh::Triangles;
