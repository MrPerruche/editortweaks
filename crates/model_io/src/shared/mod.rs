//! The cooked reference format coop sends (`docs/coop-references.md`): a reference image or model
//! made small enough for a member's quality level, and checked by the member before it is shown.
//!
//! A file is `"ETRF"`, a version byte, a kind byte (0 image, 1 model), then the kind's data. An
//! image is a PNG or JPEG file as it is. A model is [`format`]'s zlib-packed body, already in the
//! editor's space.
//!
//! [`cook_image`] and [`cook_model`] are the host's side, [`check`] and [`read_model`] the
//! member's: what arrives comes from a host nobody vouches for, so they never panic on any input
//! (the game aborts on a panic) and never allocate for what the bytes don't hold. The fuzz test
//! in `tests/shared_fuzz.rs` holds that.

mod cook;
mod format;
mod simplify;
mod textures;

#[cfg(test)]
mod tests;

use crate::image;
use crate::model::{MAX_TRIANGLES, Model};

/// The file extension of a cooked model, which the model readers know (`readers::Format::Shared`).
pub const EXTENSION: &str = "etmodel";

const MAGIC: [u8; 4] = *b"ETRF";
const VERSION: u8 = 1;
const KIND_IMAGE: u8 = 0;
const KIND_MODEL: u8 = 1;

/// The JPEG quality of the copy of an image a member makes of what arrived.
const CANONICAL_QUALITY: u8 = 92;

/// The pixels the textures of a model read from a file may have together.
const FILE_PIXELS: u64 = 8192 * 8192 * 4;

/// What a cooked reference should fit in.
#[derive(Clone, Copy, Debug)]
pub struct Aim {
    /// Triangles of a model.
    pub triangles: u64,
    /// The longest side of an image or a texture.
    pub longest_side: u32,
    /// Pixels of an image, or of all of a model's textures.
    pub pixels: u64,
    /// The size of the cooked file.
    pub bytes: u64,
}

/// A cooked reference: a file to send, and what it holds.
pub struct Cooked {
    pub bytes: Vec<u8>,
    pub triangles: u32,
    pub pixels: u32,
    /// Whether it is less than the original: fewer triangles, a smaller or worse picture.
    pub simplified: bool,
    hash: u64,
}

impl Cooked {
    fn new(bytes: Vec<u8>, triangles: u32, pixels: u32, simplified: bool) -> Self {
        let hash = content_hash(&bytes);
        Self {
            bytes,
            triangles,
            pixels,
            simplified,
            hash,
        }
    }

    /// [`content_hash`] of the bytes, made once.
    pub fn content(&self) -> u64 {
        self.hash
    }
}

/// What a receiver allows, in total over one reference.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub triangles: u64,
    pub longest_side: u32,
    pub pixels: u64,
}

/// What a checked reference is, for the receiver to keep.
pub enum Checked {
    /// An image, as a file made here, never the sender's: the engine reads this one.
    Image {
        file: Vec<u8>,
        extension: &'static str,
    },
    /// A model: the cooked bytes it was checked from are fine to keep and read.
    Model,
}

/// A hash of `bytes` that stays the same from one build to the next, to tell cooked files apart.
pub fn content_hash(bytes: &[u8]) -> u64 {
    crate::sidecar::fnv1a(bytes)
}

/// The pixels of all the textures of `model`.
pub fn model_pixels(model: &Model) -> u64 {
    model
        .materials
        .iter()
        .filter_map(|material| material.texture.as_ref())
        .map(|texture| u64::from(texture.width) * u64::from(texture.height))
        .sum()
}

/// The image file `file` (PNG or JPEG) as a cooked image that fits `aim`: as it is if it already
/// does, else smaller and, if opaque, as a JPEG.
pub fn cook_image(file: &[u8], aim: &Aim) -> Result<Cooked, String> {
    cook::cook_image(file, aim)
}

/// `model` (in the editor's space) as a cooked model that fits `aim`.
pub fn cook_model(model: &Model, aim: &Aim) -> Result<Cooked, String> {
    cook::cook_model(model, aim)
}

/// Checks what a member received. `triangles` and `pixels` are what the sender declared, and must
/// be what is in the bytes, which must be within `limits`. For an image the result carries the
/// copy to use; for a model, the bytes were read whole and found good.
pub fn check(
    bytes: &[u8],
    limits: &Limits,
    triangles: u32,
    pixels: u32,
) -> Result<Checked, String> {
    let (kind, rest) = header(bytes)?;
    if kind == KIND_MODEL {
        let model = format::read(bytes, limits)?;
        declared(triangles, model.triangle_count() as u64, "triangles")?;
        declared(pixels, model_pixels(&model), "pixels")?;
        return Ok(Checked::Model);
    }

    // The header first: a small file can declare a huge picture.
    let (width, height) = image::dimensions(rest)?;
    if width.max(height) > limits.longest_side {
        return Err(format!("an image of {width} x {height} is too big"));
    }
    if u64::from(width) * u64::from(height) > limits.pixels {
        return Err("the image has too many pixels".into());
    }
    declared(triangles, 0, "triangles")?;
    declared(pixels, u64::from(width) * u64::from(height), "pixels")?;
    let picture = image::decode(rest)?;
    if (picture.width, picture.height) != (width, height) {
        return Err("the image isn't the size it says".into());
    }
    if picture.is_opaque() {
        let file = picture.to_jpeg(CANONICAL_QUALITY)?;
        Ok(Checked::Image {
            file,
            extension: "jpg",
        })
    } else {
        let file = picture.to_small_png()?;
        Ok(Checked::Image {
            file,
            extension: "png",
        })
    }
}

/// A cooked model (a whole `.etmodel` file) as a model, in the editor's space.
pub fn read_model(bytes: &[u8]) -> Result<Model, String> {
    let limits = Limits {
        triangles: MAX_TRIANGLES as u64,
        longest_side: image::MAX_SIDE,
        pixels: FILE_PIXELS,
    };
    format::read(bytes, &limits)
}

/// `bytes` of an image as a cooked image.
fn image_file(bytes: &[u8]) -> Vec<u8> {
    let mut file = Vec::with_capacity(bytes.len() + 6);
    file.extend_from_slice(&MAGIC);
    file.extend_from_slice(&[VERSION, KIND_IMAGE]);
    file.extend_from_slice(bytes);
    file
}

/// The kind of a cooked file and what comes after its header.
fn header(bytes: &[u8]) -> Result<(u8, &[u8]), String> {
    let Some((head, rest)) = bytes.split_first_chunk::<6>() else {
        return Err("not a cooked reference: too short".into());
    };
    if head[..4] != MAGIC {
        return Err("not a cooked reference".into());
    }
    if head[4] != VERSION {
        return Err(format!("a cooked reference of version {}", head[4]));
    }
    match head[5] {
        kind @ (KIND_IMAGE | KIND_MODEL) => Ok((kind, rest)),
        kind => Err(format!("a cooked reference of kind {kind}")),
    }
}

/// What comes after the header of a cooked file that must be of `kind`.
fn split_header(bytes: &[u8], kind: u8) -> Result<&[u8], String> {
    match header(bytes)? {
        (found, rest) if found == kind => Ok(rest),
        _ => Err("not the kind of reference expected".into()),
    }
}

/// Fails if what the sender said isn't what is there.
fn declared(said: u32, found: u64, what: &str) -> Result<(), String> {
    if u64::from(said) != found {
        return Err(format!("it said {said} {what} and has {found}"));
    }
    Ok(())
}
