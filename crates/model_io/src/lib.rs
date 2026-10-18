//! Reference images and models, read from files into plain data, plus the per-vehicle file that
//! remembers them (`editortweaks.toml`). No game code here: the mod turns this data into textures
//! and meshes (`docs/model-io.md`).
//!
//! The flow for a model: a reader in [`readers`] parses the file into a [`Model`] in the file's own
//! space, then [`space::to_unreal`] turns it into the editor's space with the reference's
//! [`ImportSettings`].

pub mod companions;
pub mod image;
pub mod model;
pub mod pick;
pub mod plane;
pub mod readers;
pub mod sidecar;
pub mod space;

pub use image::Image;
pub use model::{Material, Model, Part};
pub use space::ImportSettings;
