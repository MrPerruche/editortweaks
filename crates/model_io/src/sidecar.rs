//! The per-vehicle file that remembers the references (`<vehicle folder>/editortweaks.toml`), and
//! the copies of their files kept next to it (`<vehicle folder>/editortweaks/`).
//!
//! A reference is either a link (only the original path is stored) or a copy (the file is copied
//! into the vehicle's folder). The file is TOML so it can be edited by hand, and every field has a
//! default: a newer mod reads an older file, an older one ignores keys it doesn't know.
//!
//! Everything read from disk is untrusted: relative paths of copies are checked before they are
//! joined to the vehicle's folder.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::companions::{self, Companions};
pub use crate::plane::ImagePivot;
use crate::space::ImportSettings;

/// The sidecar's name inside the vehicle's folder.
pub const FILE_NAME: &str = "editortweaks.toml";
/// The folder (inside the vehicle's folder) holding copies of the referenced files.
pub const COPIES_FOLDER: &str = "editortweaks";
/// The format version this build writes.
pub const CURRENT_VERSION: u32 = 1;
/// The default size cap of a copied file: 64 MB.
pub const DEFAULT_COPY_LIMIT: u64 = 64 * 1024 * 1024;

/// What the written file starts with (the toml crate can't emit comments).
const HEADER: &str = "# Belongs to the editortweaks mod for Brick Rigs: lists this vehicle's reference images and \
                      models. Safe to delete.\n";

/// Everything in the sidecar file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sidecar {
    /// The format version of the file.
    pub version: u32,
    /// Only in a folder the game reuses for other vehicles (its autosave of unsaved vehicles): the
    /// vehicle file this sidecar was written for. It belongs to that vehicle only while the file
    /// is unchanged ([`Mark::matches`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vehicle: Option<Mark>,
    /// The references, in the order they were added (`[[reference]]` in the file).
    #[serde(rename = "reference")]
    pub references: Vec<Reference>,
}

impl Default for Sidecar {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            vehicle: None,
            references: Vec::new(),
        }
    }
}

/// A vehicle file as it was when the sidecar was written: its name in the vehicle's folder and a
/// hash of its contents.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mark {
    pub file: String,
    /// FNV-1a (64 bits) of the file's bytes, in hexadecimal.
    pub hash: String,
}

impl Mark {
    /// The mark of the file `file` in `folder` as it is now; `None` if it can't be read (or `file`
    /// isn't a plain file name).
    pub fn of(folder: &Path, file: &str) -> Option<Mark> {
        // Read from disk: untrusted, it mustn't lead out of the folder.
        if Path::new(file).file_name() != Some(file.as_ref()) {
            return None;
        }
        let bytes = std::fs::read(folder.join(file)).ok()?;
        Some(Mark {
            file: file.to_owned(),
            hash: format!("{:016x}", fnv1a(&bytes)),
        })
    }

    /// Whether the marked file in `folder` is still the one marked.
    pub fn matches(&self, folder: &Path) -> bool {
        Mark::of(folder, &self.file).as_ref() == Some(self)
    }
}

/// The 64-bit FNV-1a hash of `bytes`: stable from one build to the next (unlike the standard
/// library's hasher), and enough to tell one vehicle file from another.
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(PRIME)
    })
}

/// What a reference shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Image,
    Model,
}

/// In which views a reference is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShowIn {
    /// Every view.
    #[default]
    All,
    /// Only orthographic views (top, side, ...).
    Ortho,
    /// Only the perspective view.
    Perspective,
}

/// Whether the file is only linked or also copied next to the vehicle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceMode {
    #[default]
    Link,
    Copy,
}

/// Where a reference's file is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Source {
    pub mode: SourceMode,
    /// The original file, with forward slashes.
    pub path: String,
    /// The copy, relative to the vehicle's folder, with forward slashes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy: Option<String>,
    /// The size of the file when last loaded, to notice it changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// The modification time of the file when last loaded, in unix seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<u64>,
}

/// How a reference image is drawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageSettings {
    /// The width in the editor, in cm (the height follows the picture's proportions).
    pub width: f64,
    pub flip_x: bool,
    pub flip_y: bool,
    /// Visible from both sides.
    pub double_sided: bool,
    /// Sharp pixels instead of smoothed ones.
    pub pixelated: bool,
    pub pivot: ImagePivot,
}

impl Default for ImageSettings {
    fn default() -> Self {
        Self {
            width: 100.0,
            flip_x: false,
            flip_y: false,
            double_sided: true,
            pixelated: false,
            pivot: ImagePivot::Center,
        }
    }
}

/// One reference image or model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reference {
    /// Stable and unique in the sidecar (`r1`, `r2`, ...).
    pub id: String,
    pub kind: Kind,
    /// Shown in the panel; the file's name to begin with.
    pub name: String,
    pub visible: bool,
    pub selectable: bool,
    pub locked: bool,
    pub source: Source,
    /// In cm, in the vehicle's space.
    pub position: [f64; 3],
    /// Pitch, yaw and roll in degrees.
    pub rotation: [f64; 3],
    pub scale: [f64; 3],
    /// Mirrored along X, Y, Z.
    pub mirror: [bool; 3],
    pub opacity: f32,
    /// Red, green and blue (0 to 1, as color pickers show them: sRGB) that multiply the colors of
    /// the picture or the model's parts. White leaves them as they are.
    pub color: [f32; 3],
    pub show_in: ShowIn,
    /// Only for images (`Some` after [`Reference::new_image`] and [`Sidecar::read`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageSettings>,
    /// Only for models (`Some` after [`Reference::new_model`] and [`Sidecar::read`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub import: Option<ImportSettings>,
}

impl Default for Reference {
    /// The settings and neither of `image` / `import`: see [`Reference::fill_kind_settings`].
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: Kind::Image,
            name: String::new(),
            visible: true,
            selectable: true,
            locked: false,
            source: Source::default(),
            position: [0.0; 3],
            rotation: [0.0; 3],
            scale: [1.0; 3],
            mirror: [false; 3],
            opacity: 0.5,
            color: [1.0; 3],
            show_in: ShowIn::All,
            image: None,
            import: None,
        }
    }
}

impl Reference {
    /// A reference image linked to `path`, named after the file.
    pub fn new_image(id: impl Into<String>, path: &Path) -> Self {
        let mut reference = Self::linked(id, Kind::Image, path);
        reference.fill_kind_settings();
        reference
    }

    /// A reference model linked to `path`, named after the file.
    pub fn new_model(id: impl Into<String>, path: &Path, import: ImportSettings) -> Self {
        let mut reference = Self::linked(id, Kind::Model, path);
        reference.import = Some(import);
        reference
    }

    fn linked(id: impl Into<String>, kind: Kind, path: &Path) -> Self {
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            id: id.into(),
            kind,
            name,
            source: Source {
                path: slashes(path),
                ..Source::default()
            },
            ..Self::default()
        }
    }

    /// Makes `image` / `import` match `kind`: present (with defaults) for the kind that uses it,
    /// absent for the other. Files written by hand or by other versions may disagree.
    pub fn fill_kind_settings(&mut self) {
        match self.kind {
            Kind::Image => {
                self.image.get_or_insert_with(ImageSettings::default);
                self.import = None;
            }
            Kind::Model => {
                self.import.get_or_insert_with(ImportSettings::default);
                self.image = None;
            }
        }
    }

    /// Where to look for the reference's file, best first: the copy then the original for a copy,
    /// the original then the copy (if there is one) for a link. A copy path that isn't safe (see
    /// [`copy_path`]) is left out.
    pub fn file_candidates(&self, folder: &Path) -> Vec<PathBuf> {
        let original = PathBuf::from(&self.source.path);
        let copy = self
            .source
            .copy
            .as_deref()
            .and_then(|relative| copy_path(folder, relative).ok());
        match self.source.mode {
            SourceMode::Copy => copy.into_iter().chain([original]).collect(),
            SourceMode::Link => [original].into_iter().chain(copy).collect(),
        }
    }
}

impl Sidecar {
    /// Reads the sidecar in `folder`: `Ok(None)` if there is none.
    ///
    /// On `Err` (unreadable or not valid TOML) the caller must not write a sidecar there: it would
    /// overwrite a file the user may be able to fix.
    pub fn read(folder: &Path) -> Result<Option<Sidecar>, String> {
        let path = folder.join(FILE_NAME);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("can't read {}: {error}", path.display())),
        };
        let mut sidecar: Sidecar = toml::from_str(&text)
            .map_err(|error| format!("can't parse {}: {error}", path.display()))?;
        for reference in &mut sidecar.references {
            reference.fill_kind_settings();
        }
        Ok(Some(sidecar))
    }

    /// Writes the sidecar into `folder`, atomically (a temporary file renamed over the real one).
    /// With no references the file is deleted instead. Refuses if the version is newer than
    /// [`CURRENT_VERSION`]: a newer mod wrote it and this one would drop what it doesn't know.
    pub fn write(&self, folder: &Path) -> Result<(), String> {
        if self.is_newer() {
            return Err(format!(
                "{FILE_NAME} was written by a newer version of the mod (format {}, this one writes {CURRENT_VERSION})",
                self.version
            ));
        }
        let path = folder.join(FILE_NAME);
        if self.references.is_empty() {
            return match std::fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    Err(format!("can't delete {}: {error}", path.display()))
                }
                _ => Ok(()),
            };
        }

        let body =
            toml::to_string(self).map_err(|error| format!("can't write {FILE_NAME}: {error}"))?;
        let temp = folder.join(format!("{FILE_NAME}.tmp"));
        std::fs::write(&temp, format!("{HEADER}{body}"))
            .map_err(|error| format!("can't write {}: {error}", temp.display()))?;
        std::fs::rename(&temp, &path).map_err(|error| {
            // Don't leave the temporary file behind.
            let _ = std::fs::remove_file(&temp);
            format!("can't replace {}: {error}", path.display())
        })
    }

    /// This sidecar as it belongs in `to` when the vehicle in `from` is saved there under a new
    /// name (Save As): the copies are copied along, and links relative to `from` made absolute so
    /// they still find their files. Write the result with [`Sidecar::write`].
    ///
    /// A copy that's missing (or whose path isn't safe) is skipped: the reference falls back to
    /// its original file, as it did in `from`.
    pub fn carried_over(&self, from: &Path, to: &Path) -> Result<Sidecar, String> {
        let mut carried = self.clone();
        for reference in &mut carried.references {
            let original = Path::new(&reference.source.path);
            if !reference.source.path.is_empty() && original.is_relative() {
                reference.source.path = slashes(&from.join(original));
            }
            let Some(relative) = &reference.source.copy else {
                continue;
            };
            let (Ok(source), Ok(target)) = (copy_path(from, relative), copy_path(to, relative))
            else {
                continue;
            };
            if !source.is_file() {
                continue;
            }
            std::fs::create_dir_all(to.join(COPIES_FOLDER))
                .map_err(|error| format!("can't create the {COPIES_FOLDER} folder: {error}"))?;
            // A copy with companions is a whole folder.
            let result = match own_folder(relative) {
                Some(model_folder) => copy_folder(&from.join(model_folder), &to.join(model_folder)),
                None => std::fs::copy(&source, &target).map(|_| ()),
            };
            result.map_err(|error| format!("can't copy to {}: {error}", target.display()))?;
        }
        Ok(carried)
    }

    /// Whether the file comes from a newer format than this build knows.
    pub fn is_newer(&self) -> bool {
        self.version > CURRENT_VERSION
    }

    /// A short id (`r1`, `r2`, ...) no reference uses yet: the first free number.
    pub fn new_id(&self) -> String {
        (1u32..)
            .map(|number| format!("r{number}"))
            .find(|id| self.references.iter().all(|reference| reference.id != *id))
            .unwrap_or_default()
    }
}

/// What [`make_copy`] made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copied {
    /// The copy, relative to the vehicle's folder, with forward slashes.
    pub relative: String,
    /// Files the original points to that were left out because they aren't inside its folder
    /// (see [`companions`]), as written in it.
    pub skipped: Vec<String>,
}

/// Copies `original` into the vehicle's `folder` as `editortweaks/<id>.<extension>` (extension in
/// lowercase). A model that points to other files (a `.gltf` with its `.bin` and images, an `.obj`
/// with its `.mtl` and textures) is copied with them into `editortweaks/<id>/`, under its own file
/// name and the companions' relative paths, replacing an older copy there. Refuses files bigger
/// than `limit` bytes (all of them together, for a model with companions).
pub fn make_copy(folder: &Path, id: &str, original: &Path, limit: u64) -> Result<Copied, String> {
    if !is_valid_id(id) {
        return Err(format!("\"{id}\" is not a valid reference id"));
    }
    // Checked before the file is read for its companions.
    let mut total = file_size(original)?;
    check_limit(&original.display().to_string(), total, limit)?;

    let Companions { found, skipped } = companions::of(original);
    let source_folder = original.parent().unwrap_or(Path::new(""));
    for name in &found {
        total += file_size(&source_folder.join(name))?;
    }
    check_limit(
        &format!("{} and the files it uses", original.display()),
        total,
        limit,
    )?;

    let relative = if found.is_empty() {
        copy_single(folder, id, original)?
    } else {
        copy_with_companions(folder, id, original, &found)?
    };
    Ok(Copied { relative, skipped })
}

/// Whether `id` is a name a copy can safely be named after.
fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The copy of a file that stands alone: `editortweaks/<id>.<extension>`.
fn copy_single(folder: &Path, id: &str, original: &Path) -> Result<String, String> {
    let file_name = match original.extension() {
        Some(extension) => format!("{id}.{}", extension.to_string_lossy().to_lowercase()),
        None => id.to_string(),
    };
    let relative = format!("{COPIES_FOLDER}/{file_name}");
    copy_file(folder, &relative, original)?;
    Ok(relative)
}

/// The copy of a model and its `companions` (relative to its folder): `editortweaks/<id>/`
/// holding the model under its file name and the companions under their relative paths. A
/// half-made copy is removed again.
fn copy_with_companions(
    folder: &Path,
    id: &str,
    original: &Path,
    companions: &[String],
) -> Result<String, String> {
    let file_name = original
        .file_name()
        .ok_or_else(|| format!("{} has no file name", original.display()))?
        .to_string_lossy();
    let source_folder = original.parent().unwrap_or(Path::new(""));
    let model_folder = format!("{COPIES_FOLDER}/{id}");
    let model_path = copy_path(folder, &model_folder)?;
    remove_folder(&model_path)?;

    let files = std::iter::once(file_name.as_ref()).chain(companions.iter().map(String::as_str));
    for name in files {
        let result = copy_file(
            folder,
            &format!("{model_folder}/{name}"),
            &source_folder.join(name),
        );
        if let Err(error) = result {
            let _ = std::fs::remove_dir_all(&model_path);
            return Err(error);
        }
    }
    Ok(format!("{model_folder}/{file_name}"))
}

/// Copies `source` to `relative` inside `folder` (see [`copy_path`]), creating the folders.
fn copy_file(folder: &Path, relative: &str, source: &Path) -> Result<(), String> {
    let target = copy_path(folder, relative)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("can't create {}: {error}", parent.display()))?;
    }
    std::fs::copy(source, &target).map(|_| ()).map_err(|error| {
        format!(
            "can't copy {} to {}: {error}",
            source.display(),
            target.display()
        )
    })
}

fn file_size(path: &Path) -> Result<u64, String> {
    std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|error| format!("can't read {}: {error}", path.display()))
}

fn check_limit(what: &str, size: u64, limit: u64) -> Result<(), String> {
    if size > limit {
        return Err(format!(
            "{what} is {:.1} MB, over the {:.1} MB limit for copies",
            megabytes(size),
            megabytes(limit)
        ));
    }
    Ok(())
}

/// Copies a folder and everything in it to `to`.
fn copy_folder(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_folder(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Deletes a folder and what's in it. One that's already gone is fine.
fn remove_folder(path: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("can't delete {}: {error}", path.display()))
        }
        _ => Ok(()),
    }
}

/// Deletes the copy at `relative` (as returned by [`make_copy`]), then the copies folder if that
/// left it empty. A copy in its own folder (`editortweaks/<id>/<name>`) goes with all its files.
/// A copy that's already gone is fine.
pub fn remove_copy(folder: &Path, relative: &str) -> Result<(), String> {
    let path = copy_path(folder, relative)?;
    match own_folder(relative) {
        Some(model_folder) => remove_folder(&copy_path(folder, model_folder)?)?,
        None => match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(format!("can't delete {}: {error}", path.display()));
            }
            _ => {}
        },
    }
    // Fails if the folder still holds something (or is gone): either way, nothing to do.
    let _ = std::fs::remove_dir(folder.join(COPIES_FOLDER));
    Ok(())
}

/// `editortweaks/<id>` for a copy that lives in its own folder (`editortweaks/<id>/<name>`, made
/// by [`make_copy`]), `None` for a copy that is a file directly in `editortweaks`.
fn own_folder(relative: &str) -> Option<&str> {
    let rest = relative.strip_prefix(COPIES_FOLDER)?.strip_prefix('/')?;
    let (id, _) = rest.split_once('/')?;
    is_valid_id(id).then(|| &relative[..COPIES_FOLDER.len() + 1 + id.len()])
}

/// Where the copy at `relative` is inside `folder`, if `relative` is safe: it must be
/// `editortweaks/<name>` with plain names only (no `..`, no absolute path, no drive, no
/// backslashes), since it comes from a file the user may have edited or received.
pub fn copy_path(folder: &Path, relative: &str) -> Result<PathBuf, String> {
    let unsafe_path = || format!("\"{relative}\" is not a path inside the {COPIES_FOLDER} folder");
    let rest = relative
        .strip_prefix(COPIES_FOLDER)
        .and_then(|rest| rest.strip_prefix('/'))
        .ok_or_else(unsafe_path)?;
    let plain = |part: &str| !matches!(part, "" | "." | "..") && !part.contains(['\\', ':', '\0']);
    if !rest.split('/').all(plain) {
        return Err(unsafe_path());
    }
    Ok(folder.join(COPIES_FOLDER).join(rest))
}

/// The size in bytes and modification time in unix seconds of `path`, to tell whether a file
/// changed since it was loaded. `None` if it can't be read.
pub fn file_stamp(path: &Path) -> Option<(u64, u64)> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some((metadata.len(), modified))
}

/// A path as stored in the file: forward slashes.
fn slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn megabytes(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space::{Axis, Pivot, Units};

    /// An empty folder of our own, deleted when dropped (even if the test fails).
    struct TempFolder(PathBuf);

    impl TempFolder {
        fn new(test: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("model_io_sidecar_{test}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempFolder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sidecar_with(references: Vec<Reference>) -> Sidecar {
        Sidecar {
            references,
            ..Sidecar::default()
        }
    }

    #[test]
    fn round_trip() {
        let folder = TempFolder::new("round_trip");
        let mut image = Reference::new_image("r1", Path::new("D:\\Pictures\\blueprint side.png"));
        image.position = [10.5, -20.0, 30.25];
        image.rotation = [0.0, 90.0, 0.0];
        image.mirror = [false, true, false];
        image.opacity = 0.75;
        image.color = [1.0, 0.5, 0.25];
        image.show_in = ShowIn::Ortho;
        image.source.bytes = Some(1234);
        image.source.modified = Some(1_760_000_000);
        image.image = Some(ImageSettings {
            width: 250.0,
            flip_y: true,
            pivot: ImagePivot::Corner,
            ..Default::default()
        });
        let mut model = Reference::new_model(
            "r2",
            Path::new("D:/Models/hilux.glb"),
            ImportSettings::stl(),
        );
        model.source.mode = SourceMode::Copy;
        model.source.copy = Some("editortweaks/r2.glb".to_string());
        model.locked = true;
        model.visible = false;
        model.scale = [2.0, 2.0, 2.0];
        let sidecar = sidecar_with(vec![image, model]);

        sidecar.write(folder.0.as_path()).unwrap();
        let text = std::fs::read_to_string(folder.0.join(FILE_NAME)).unwrap();
        assert!(text.starts_with('#'), "{text}");
        assert!(text.contains("[[reference]]"), "{text}");
        assert!(text.contains("D:/Pictures/blueprint side.png"), "{text}");
        assert!(!folder.0.join(format!("{FILE_NAME}.tmp")).exists());

        let read = Sidecar::read(&folder.0).unwrap().unwrap();
        assert_eq!(read, sidecar);
        // Each kind only has its own settings.
        assert!(read.references[0].import.is_none());
        assert!(read.references[1].image.is_none());
    }

    #[test]
    fn missing_keys_use_defaults() {
        let folder = TempFolder::new("defaults");
        std::fs::write(
            folder.0.join(FILE_NAME),
            "version = 1\n\n[[reference]]\nid = \"r1\"\nkind = \"model\"\n",
        )
        .unwrap();
        let sidecar = Sidecar::read(&folder.0).unwrap().unwrap();
        assert_eq!(sidecar.references.len(), 1);
        let expected = Reference {
            id: "r1".to_string(),
            kind: Kind::Model,
            import: Some(ImportSettings::default()),
            ..Reference::default()
        };
        assert_eq!(sidecar.references[0], expected);
        assert!(expected.visible && expected.selectable && !expected.locked);
        assert_eq!(expected.opacity, 0.5);
        assert_eq!(expected.color, [1.0; 3]);
        assert_eq!(expected.scale, [1.0; 3]);
        assert_eq!(expected.source.mode, SourceMode::Link);
    }

    #[test]
    fn no_file_is_none_and_garbage_is_an_error() {
        let folder = TempFolder::new("missing");
        assert_eq!(Sidecar::read(&folder.0), Ok(None));
        std::fs::write(folder.0.join(FILE_NAME), "this is [not toml").unwrap();
        assert!(Sidecar::read(&folder.0).is_err());
    }

    #[test]
    fn hand_written_file() {
        let folder = TempFolder::new("by_hand");
        let text = r#"
version = 1

[[reference]]
id = "r1"
kind = "image"
position = [300.0, 0.0, 100.0]
opacity = 1.0

[reference.source]
path = "C:/Pictures/front.png"

[reference.image]
width = 200.0

[[reference]]
id = "r2"
kind = "model"
mirror = [false, true, false]

[reference.source]
path = "car.obj"

[reference.import]
pivot = "bottom"
"#;
        std::fs::write(folder.0.join(FILE_NAME), text).unwrap();
        let sidecar = Sidecar::read(&folder.0).unwrap().unwrap();
        let [image, model] = &sidecar.references[..] else {
            panic!("{sidecar:?}");
        };
        assert_eq!(image.position, [300.0, 0.0, 100.0]);
        assert_eq!(image.opacity, 1.0);
        let settings = image.image.as_ref().unwrap();
        assert_eq!(settings.width, 200.0);
        assert!(settings.double_sided);
        assert_eq!(model.mirror, [false, true, false]);
        assert_eq!(model.source.path, "car.obj");
        assert_eq!(
            model.import,
            Some(ImportSettings {
                pivot: Pivot::Bottom,
                ..ImportSettings::default()
            })
        );
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let folder = TempFolder::new("unknown");
        let text = "version = 1\nfuture = \"yes\"\n\n[[reference]]\nid = \"r1\"\nkind = \"image\"\nsparkle = 3\n\n\
                    [reference.source]\npath = \"a/b.png\"\nhash = \"abc\"\n";
        std::fs::write(folder.0.join(FILE_NAME), text).unwrap();
        let sidecar = Sidecar::read(&folder.0).unwrap().unwrap();
        assert_eq!(sidecar.references[0].source.path, "a/b.png");
        assert_eq!(sidecar.references[0].image, Some(ImageSettings::default()));
    }

    #[test]
    fn newer_version_refuses_to_write() {
        let folder = TempFolder::new("newer");
        let original = "version = 99\n";
        std::fs::write(folder.0.join(FILE_NAME), original).unwrap();
        let sidecar = Sidecar::read(&folder.0).unwrap().unwrap();
        assert!(sidecar.is_newer());
        assert!(sidecar.write(&folder.0).is_err());
        let with_reference = Sidecar {
            references: vec![Reference::default()],
            ..sidecar
        };
        assert!(with_reference.write(&folder.0).is_err());
        assert_eq!(
            std::fs::read_to_string(folder.0.join(FILE_NAME)).unwrap(),
            original
        );
        assert!(!Sidecar::default().is_newer());
    }

    #[test]
    fn vehicle_mark() {
        let folder = TempFolder::new("mark");
        assert_eq!(Mark::of(&folder.0, "Autosave.brv"), None);
        std::fs::write(folder.0.join("Autosave.brv"), "one vehicle").unwrap();
        let mark = Mark::of(&folder.0, "Autosave.brv").unwrap();
        assert!(mark.matches(&folder.0));
        // FNV-1a's known value for an empty input: the offset basis.
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);

        let sidecar = Sidecar {
            vehicle: Some(mark.clone()),
            ..sidecar_with(vec![Reference::new_image("r1", Path::new("C:/a.png"))])
        };
        sidecar.write(&folder.0).unwrap();
        let text = std::fs::read_to_string(folder.0.join(FILE_NAME)).unwrap();
        assert!(text.contains("[vehicle]"), "{text}");
        assert_eq!(Sidecar::read(&folder.0).unwrap().unwrap(), sidecar);

        std::fs::write(folder.0.join("Autosave.brv"), "another vehicle").unwrap();
        assert!(!mark.matches(&folder.0));
        std::fs::remove_file(folder.0.join("Autosave.brv")).unwrap();
        assert!(!mark.matches(&folder.0));

        // Files outside the folder are never read.
        for file in ["../Autosave.brv", "a/b.brv", "C:\\x.brv", "..", ""] {
            assert_eq!(Mark::of(&folder.0, file), None, "{file}");
        }
    }

    #[test]
    fn no_references_deletes_the_file() {
        let folder = TempFolder::new("empty");
        let one = sidecar_with(vec![Reference::new_image("r1", Path::new("a.png"))]);
        one.write(&folder.0).unwrap();
        assert!(folder.0.join(FILE_NAME).exists());
        Sidecar::default().write(&folder.0).unwrap();
        assert!(!folder.0.join(FILE_NAME).exists());
        // Nothing to delete is fine too.
        Sidecar::default().write(&folder.0).unwrap();
    }

    #[test]
    fn new_id_skips_used_ids() {
        let mut sidecar = Sidecar::default();
        assert_eq!(sidecar.new_id(), "r1");
        sidecar.references.push(Reference {
            id: "r1".to_string(),
            ..Reference::default()
        });
        sidecar.references.push(Reference {
            id: "r3".to_string(),
            ..Reference::default()
        });
        assert_eq!(sidecar.new_id(), "r2");
        sidecar.references.push(Reference {
            id: "r2".to_string(),
            ..Reference::default()
        });
        assert_eq!(sidecar.new_id(), "r4");
    }

    #[test]
    fn copies_respect_the_limit() {
        let folder = TempFolder::new("copy");
        let original = folder.0.join("Picture.PNG");
        std::fs::write(&original, [0u8; 2048]).unwrap();

        let error = make_copy(&folder.0, "r1", &original, 1024).unwrap_err();
        assert!(error.contains("MB"), "{error}");
        assert!(!folder.0.join(COPIES_FOLDER).exists());

        let copied = make_copy(&folder.0, "r1", &original, 4096).unwrap();
        assert_eq!(copied.relative, "editortweaks/r1.png");
        assert!(copied.skipped.is_empty());
        assert_eq!(
            std::fs::read(folder.0.join("editortweaks").join("r1.png"))
                .unwrap()
                .len(),
            2048
        );

        assert!(make_copy(&folder.0, "../r1", &original, 4096).is_err());
    }

    /// A `.gltf` in `folder/models` with an external buffer, a texture in a subfolder and a buffer
    /// in a data URI; returns its path.
    fn write_gltf_with_companions(folder: &Path) -> PathBuf {
        let models = folder.join("models");
        std::fs::create_dir_all(models.join("textures")).unwrap();
        std::fs::write(models.join("scene.bin"), [1u8; 100]).unwrap();
        std::fs::write(models.join("textures").join("wood.png"), [2u8; 50]).unwrap();
        std::fs::write(models.join("unrelated.txt"), "not copied").unwrap();
        let gltf = models.join("Scene.gltf");
        let json = r#"{"buffers": [{"uri": "scene.bin"}, {"uri": "data:;base64,AAAA"}],
                       "images": [{"uri": "textures/wood.png"}]}"#;
        std::fs::write(&gltf, json).unwrap();
        gltf
    }

    #[test]
    fn gltf_is_copied_with_its_files() {
        let folder = TempFolder::new("copy_gltf");
        let gltf = write_gltf_with_companions(&folder.0);

        let copied = make_copy(&folder.0, "r3", &gltf, 4096).unwrap();
        assert_eq!(copied.relative, "editortweaks/r3/Scene.gltf");
        assert!(copied.skipped.is_empty());
        let copy = folder.0.join(COPIES_FOLDER).join("r3");
        assert!(copy.join("Scene.gltf").is_file());
        assert_eq!(std::fs::read(copy.join("scene.bin")).unwrap(), [1u8; 100]);
        assert_eq!(
            std::fs::read(copy.join("textures").join("wood.png")).unwrap(),
            [2u8; 50]
        );
        assert!(!copy.join("unrelated.txt").exists());
        // The copy is where the reference looks for it.
        assert!(copy_path(&folder.0, &copied.relative).unwrap().is_file());

        // Copying again replaces what was there.
        std::fs::write(copy.join("stale.bin"), "old").unwrap();
        make_copy(&folder.0, "r3", &gltf, 4096).unwrap();
        assert!(!copy.join("stale.bin").exists());
        assert!(copy.join("scene.bin").is_file());
    }

    #[test]
    fn the_limit_counts_the_companions() {
        let folder = TempFolder::new("copy_gltf_limit");
        let gltf = write_gltf_with_companions(&folder.0);

        // The .gltf alone is small enough; with the files it uses it isn't.
        let error = make_copy(&folder.0, "r1", &gltf, 250).unwrap_err();
        assert!(error.contains("MB"), "{error}");
        assert!(!folder.0.join(COPIES_FOLDER).exists());
        assert!(make_copy(&folder.0, "r1", &gltf, 1000).is_ok());
    }

    #[test]
    fn obj_is_copied_with_its_material_and_texture() {
        let folder = TempFolder::new("copy_obj");
        let models = folder.0.join("models");
        std::fs::create_dir_all(&models).unwrap();
        std::fs::write(models.join("car.OBJ"), "mtllib car.mtl\nv 0 0 0\n").unwrap();
        std::fs::write(models.join("car.mtl"), "newmtl a\nmap_Kd paint.png\n").unwrap();
        std::fs::write(models.join("paint.png"), [3u8; 10]).unwrap();

        let copied = make_copy(&folder.0, "r2", &models.join("car.OBJ"), 4096).unwrap();
        assert_eq!(copied.relative, "editortweaks/r2/car.OBJ");
        let copy = folder.0.join(COPIES_FOLDER).join("r2");
        for name in ["car.OBJ", "car.mtl", "paint.png"] {
            assert!(copy.join(name).is_file(), "{name}");
        }
    }

    #[test]
    fn companions_outside_the_folder_are_skipped() {
        let folder = TempFolder::new("copy_outside");
        let models = folder.0.join("models");
        std::fs::create_dir_all(&models).unwrap();
        std::fs::write(folder.0.join("secret.bin"), "secret").unwrap();
        std::fs::write(models.join("ok.bin"), "ok").unwrap();
        let gltf = models.join("a.gltf");
        let json = r#"{"buffers": [{"uri": "../secret.bin"}, {"uri": "ok.bin"}]}"#;
        std::fs::write(&gltf, json).unwrap();

        let copied = make_copy(&folder.0, "r1", &gltf, 4096).unwrap();
        assert_eq!(copied.skipped, ["../secret.bin"]);
        let copy = folder.0.join(COPIES_FOLDER).join("r1");
        assert!(copy.join("ok.bin").is_file());
        assert!(!copy.join("secret.bin").exists());
        assert!(!folder.0.join(COPIES_FOLDER).join("secret.bin").exists());
    }

    #[test]
    fn files_that_stand_alone_keep_a_flat_copy() {
        let folder = TempFolder::new("copy_flat");
        let glb = folder.0.join("Car.GLB");
        std::fs::write(&glb, b"glTF").unwrap();
        let stl = folder.0.join("part.stl");
        std::fs::write(&stl, b"solid").unwrap();
        // A .gltf that points to no file has no companions either.
        let gltf = folder.0.join("empty.gltf");
        std::fs::write(&gltf, r#"{"buffers": [{"uri": "data:;base64,AAAA"}]}"#).unwrap();

        let relatives: Vec<String> = [("r1", &glb), ("r2", &stl), ("r3", &gltf)]
            .iter()
            .map(|(id, path)| make_copy(&folder.0, id, path, 4096).unwrap().relative)
            .collect();
        assert_eq!(
            relatives,
            [
                "editortweaks/r1.glb",
                "editortweaks/r2.stl",
                "editortweaks/r3.gltf"
            ]
        );
    }

    #[test]
    fn remove_copy_removes_the_models_folder() {
        let folder = TempFolder::new("remove_folder");
        let gltf = write_gltf_with_companions(&folder.0);
        let flat = folder.0.join("a.glb");
        std::fs::write(&flat, b"glTF").unwrap();

        let relative = make_copy(&folder.0, "r1", &gltf, 4096).unwrap().relative;
        let other = make_copy(&folder.0, "r2", &gltf, 4096).unwrap().relative;
        let single = make_copy(&folder.0, "r3", &flat, 4096).unwrap().relative;

        remove_copy(&folder.0, &relative).unwrap();
        let copies = folder.0.join(COPIES_FOLDER);
        assert!(!copies.join("r1").exists());
        assert!(copies.join("r2").join("scene.bin").is_file());
        assert!(copies.join("r3.glb").is_file());
        // The original is untouched.
        assert!(gltf.is_file());

        remove_copy(&folder.0, &other).unwrap();
        remove_copy(&folder.0, &single).unwrap();
        assert!(!copies.exists());
        // Already gone: fine.
        remove_copy(&folder.0, &other).unwrap();

        // Nothing outside the copies folder.
        assert!(remove_copy(&folder.0, "editortweaks/../models/Scene.gltf").is_err());
        assert!(remove_copy(&folder.0, "editortweaks/r1/../../models/a").is_err());
        assert!(gltf.is_file());
    }

    #[test]
    fn save_as_carries_a_copy_with_its_files() {
        let from = TempFolder::new("carry_folder_from");
        let to = TempFolder::new("carry_folder_to");
        let gltf = write_gltf_with_companions(&from.0);
        let mut model = Reference::new_model("r1", &gltf, ImportSettings::default());
        model.source.mode = SourceMode::Copy;
        model.source.copy = Some(make_copy(&from.0, "r1", &gltf, 4096).unwrap().relative);

        let carried = sidecar_with(vec![model])
            .carried_over(&from.0, &to.0)
            .unwrap();
        let copy = to.0.join(COPIES_FOLDER).join("r1");
        assert!(copy.join("Scene.gltf").is_file());
        assert!(copy.join("scene.bin").is_file());
        assert!(copy.join("textures").join("wood.png").is_file());
        assert_eq!(
            carried.references[0].source.copy.as_deref(),
            Some("editortweaks/r1/Scene.gltf")
        );
    }

    #[test]
    fn remove_copy_is_safe_and_tidy() {
        let folder = TempFolder::new("remove");
        let original = folder.0.join("a.glb");
        std::fs::write(&original, b"glTF").unwrap();
        let outside = folder.0.join("x");
        std::fs::write(&outside, b"keep me").unwrap();

        assert!(remove_copy(&folder.0, "../x").is_err());
        assert!(remove_copy(&folder.0, "editortweaks/../x").is_err());
        assert!(remove_copy(&folder.0, "x").is_err());
        assert!(remove_copy(&folder.0, "editortweaks/").is_err());
        assert!(remove_copy(&folder.0, "editortweaks\\r1.glb").is_err());
        assert!(remove_copy(&folder.0, "/etc/passwd").is_err());
        assert!(remove_copy(&folder.0, "editortweaks/C:/x").is_err());
        assert!(outside.exists());

        let relative = make_copy(&folder.0, "r1", &original, 4096)
            .unwrap()
            .relative;
        let other = make_copy(&folder.0, "r2", &original, 4096)
            .unwrap()
            .relative;
        remove_copy(&folder.0, &relative).unwrap();
        // Another copy is still in there: the folder stays.
        assert!(folder.0.join(COPIES_FOLDER).exists());
        remove_copy(&folder.0, &other).unwrap();
        assert!(!folder.0.join(COPIES_FOLDER).exists());
        // Already gone: fine.
        remove_copy(&folder.0, &other).unwrap();
    }

    #[test]
    fn save_as_carries_copies_and_links() {
        let from = TempFolder::new("carry_from");
        let to = TempFolder::new("carry_to");
        let absolute = Reference::new_image("r1", Path::new("D:/Pictures/front.png"));
        let relative = Reference::new_image("r2", Path::new("pictures/side.png"));
        let original = from.0.join("car.glb");
        std::fs::write(&original, b"glTF").unwrap();
        let mut copied = Reference::new_model("r3", &original, ImportSettings::default());
        copied.source.mode = SourceMode::Copy;
        copied.source.copy = Some(make_copy(&from.0, "r3", &original, 4096).unwrap().relative);
        let mut gone = Reference::new_model("r4", Path::new("D:/x.obj"), ImportSettings::default());
        gone.source.copy = Some("editortweaks/r4.obj".to_string());
        let sidecar = sidecar_with(vec![absolute, relative, copied, gone]);

        let carried = sidecar.carried_over(&from.0, &to.0).unwrap();
        let paths: Vec<&str> = carried
            .references
            .iter()
            .map(|reference| reference.source.path.as_str())
            .collect();
        assert_eq!(paths[0], "D:/Pictures/front.png");
        assert_eq!(paths[1], slashes(&from.0.join("pictures/side.png")));
        assert_eq!(
            std::fs::read(to.0.join("editortweaks").join("r3.glb")).unwrap(),
            b"glTF"
        );
        // The missing copy is left as it was: the reference falls back to its original.
        assert_eq!(carried.references[3], sidecar.references[3]);
        assert!(!to.0.join("editortweaks").join("r4.obj").exists());
    }

    #[test]
    fn candidates_order() {
        let folder = Path::new("V");
        let mut reference = Reference::new_model(
            "r1",
            Path::new("D:/Models/a.glb"),
            ImportSettings::default(),
        );
        let original = PathBuf::from("D:/Models/a.glb");
        let copy = folder.join("editortweaks").join("r1.glb");

        // No copy yet.
        assert_eq!(reference.file_candidates(folder), vec![original.clone()]);
        reference.source.copy = Some("editortweaks/r1.glb".to_string());
        assert_eq!(
            reference.file_candidates(folder),
            vec![original.clone(), copy.clone()]
        );
        reference.source.mode = SourceMode::Copy;
        assert_eq!(
            reference.file_candidates(folder),
            vec![copy, original.clone()]
        );
        // An unsafe copy path from the file is never offered.
        reference.source.copy = Some("../../secret.txt".to_string());
        assert_eq!(reference.file_candidates(folder), vec![original]);
    }

    #[test]
    fn constructors_name_and_link() {
        let image = Reference::new_image("r1", Path::new("D:\\Pics\\side view.png"));
        assert_eq!(image.name, "side view");
        assert_eq!(image.kind, Kind::Image);
        assert_eq!(image.source.mode, SourceMode::Link);
        assert_eq!(image.source.path, "D:/Pics/side view.png");
        assert!(image.image.is_some() && image.import.is_none());

        let settings = ImportSettings {
            up: Axis::PosZ,
            units: Units::Feet,
            pivot: Pivot::Bottom,
            ..Default::default()
        };
        let model = Reference::new_model("r2", Path::new("car.obj"), settings);
        assert_eq!(model.import, Some(settings));
        assert!(model.image.is_none());
    }

    #[test]
    fn stamp_of_a_file() {
        let folder = TempFolder::new("stamp");
        let path = folder.0.join("f.bin");
        std::fs::write(&path, [1u8, 2, 3]).unwrap();
        let (bytes, modified) = file_stamp(&path).unwrap();
        assert_eq!(bytes, 3);
        assert!(modified > 1_700_000_000);
        assert!(file_stamp(&folder.0.join("missing")).is_none());
    }
}
