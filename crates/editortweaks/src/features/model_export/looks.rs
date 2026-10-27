//! Bricks' looks as glTF materials, as the game's brick material (`ML_Brick_Master`, see
//! `docs/re/model-export.md`) makes them from the brick's colour `C`
//! (its alpha is how much paint covers the brick) and its brick material's textures:
//!
//! ```text
//! base colour = lerp(Diffuse.rgb, C.rgb, Diffuse.a * C.a)
//! metallic    = MRAO.r * Metallic * (1 - C.a * MetallicAlpha)   // MetallicAlpha = 1 - PaintMetallicAlpha if > 0, else 1
//! roughness   = MRAO.g * Roughness                              // with an MRAO texture, Metallic = Roughness = 1
//! emissive    = base colour * Emissive * Brightness
//! normal      = the Normal texture
//! ```
//!
//! All textures repeat every `1 / BaseTiling` cm of a box projection ([`export::box_uvs`]). Fully
//! painted bricks (most) show their colour only, so the diffuse texture is only used (baked with
//! the colour, one image per colour) where paint doesn't cover it.
//!
//! A pattern `P` multiplies the paint (`C.rgb * P.rgb` above, and the emissive once more), on a
//! projection of its own ([`export::pattern_uvs`]). A part has one set of UVs, so a patterned
//! brick's are the pattern's: its base colour is the pattern painted (one image per pattern,
//! colour and diffuse texture), and the textures made for the material's projection are left out
//! or averaged instead: the diffuse texture under the paint, MRAO, emissive; the normal map is left
//! out.
//!
//! Textures are read back from the GPU ([`Readback`]) once per game, at most [`LARGEST`] pixels.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;

use model_io::export::{self, Export, Look};
use model_io::image::{Image, linear_to_srgb, srgb_to_linear};
use model_io::model::Part;

use crate::game::brick_looks::{Looks, Pattern};
use crate::game::texture_readback::{Read, Readback};
use crate::game::tyres::Tyre;
use crate::game::weak_object::WeakObject;
use crate::prelude::*;

/// The largest texture side exported (pixels): the game's brick textures are 512 to 2048.
pub const LARGEST: u32 = 1024;

/// The alpha below which a glyph is cut out: the middle of its distance field (the text material's
/// `FontThreshold`).
const GLYPH_CUTOFF: f32 = 0.5;

/// Tyres' roughness (`M_Tire_Master`'s `Roughness`, times the rubber's own roughness, left out).
const TYRE_ROUGHNESS: f32 = 0.9;

/// Tyres' colour without their texture (sRGB, 0 to 1): dark rubber, a guess.
const TYRE_SRGB: f32 = 0.16;

/// How often the rubber texture repeats on tyres whose mesh UVs aren't known (per cm of the box
/// projection: once a metre, as bricks' textures). In game it follows the mesh's UVs.
const TYRE_TILING: f32 = 0.01;

/// Textures read so far this game, by object: they don't change.
static READ: Mutex<Option<HashMap<usize, Known>>> = Mutex::new(None);

/// A texture read: the texture (weakly), and what it gave.
struct Known(WeakObject, Result<Arc<Read>, String>);

// SAFETY: a weak pointer and plain data; the pointer is only used on the game thread.
unsafe impl Send for Known {}

/// What an image of the export is made from.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Source {
    /// A diffuse texture with a colour painted over it.
    BaseColor(usize, [u8; 4]),
    MetallicRoughness(usize),
    Normal(usize),
    /// Ambient occlusion in red.
    Occlusion(usize),
    Emissive(usize),
    /// A font's page, its distance field as alpha.
    Glyphs(usize),
    /// A pattern painted a colour over a diffuse texture's average (0 for none).
    Pattern(usize, usize, [u8; 4]),
    /// A pattern times itself: what it does to the emissive.
    PatternGlow(usize),
}

/// How a look's textures lie on a brick: the UVs its parts need.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Projection {
    /// The brick material's, repeating this often per cm ([`export::box_uvs`]).
    Material(f32),
    /// The brick's pattern's ([`export::pattern_uvs`]).
    Pattern {
        scale: [f32; 2],
        offset: [f32; 2],
        /// Radians.
        rotation: f32,
    },
}

impl Projection {
    /// Gives `part` the UVs of this projection.
    pub fn apply(self, part: &mut Part) {
        match self {
            Self::Material(tiling) => export::box_uvs(part, tiling),
            Self::Pattern {
                scale,
                offset,
                rotation,
            } => export::pattern_uvs(part, scale, offset, rotation),
        }
    }

    /// The projection of `pattern`, as `UBrick::UpdatePrimitiveData` hands it to the material:
    /// `1 / (Tiling * PatternScale)` repeats per cm; `None` if that isn't a number (a scale or
    /// tiling of 0).
    fn of(pattern: &Pattern) -> Option<Self> {
        let scale = pattern.scale.map(|scale| 1.0 / (pattern.tiling * scale));
        scale
            .iter()
            .all(|scale| scale.is_finite())
            .then(|| Self::Pattern {
                scale,
                offset: pattern.offset,
                rotation: pattern.rotation.to_radians(),
            })
    }
}

/// Makes the looks of one export, adding the images they need to it.
pub struct Materials<'a> {
    readback: Option<&'a Readback>,
    /// A live actor in the editor's world, for the engine's drawing.
    context: *mut UObjectBase,
    images: HashMap<Source, Option<usize>>,
    /// Textures' average linear colours, by texture; `None` if one can't be read.
    averages: HashMap<usize, Option<[f32; 4]>>,
    /// How many textures couldn't be read.
    pub unread: usize,
}

impl<'a> Materials<'a> {
    pub fn new(readback: Option<&'a Readback>, context: *mut UObjectBase) -> Self {
        Self {
            readback,
            context,
            images: HashMap::new(),
            averages: HashMap::new(),
            unread: 0,
        }
    }

    /// The look of a brick with `looks` and `pattern`, and the UVs it needs if it has textures.
    ///
    /// # Safety
    /// The textures in `looks` and `pattern` must be live (or null). Game thread only.
    pub unsafe fn brick(
        &mut self,
        export: &mut Export,
        looks: &Looks,
        pattern: Option<&Pattern>,
    ) -> (Look, Option<Projection>) {
        if let Some(pattern) = pattern
            && let Some(projection) = Projection::of(pattern)
            // SAFETY: per the contract.
            && let Some(look) = unsafe { self.patterned(export, looks, pattern) }
        {
            return (look, Some(projection));
        }
        // SAFETY: per the contract.
        let (look, tiling) = unsafe { self.plain(export, looks) };
        (look, tiling.map(Projection::Material))
    }

    /// The look of a brick with `looks` and no pattern, and the tiling its UVs need if it has
    /// textures.
    ///
    /// # Safety
    /// As for [`Self::brick`].
    unsafe fn plain(&mut self, export: &mut Export, looks: &Looks) -> (Look, Option<f32>) {
        let textures = looks.textures;
        let with_mrao = !textures.mrao.is_null();
        let glow = glow(looks);
        let mut look = untextured(looks);
        // SAFETY: per the contract.
        unsafe {
            if !textures.diffuse.is_null()
                && let Some(image) = self.image(
                    export,
                    Source::BaseColor(textures.diffuse as usize, looks.color),
                    textures.diffuse,
                    |read| painted(read, looks.color),
                )
            {
                look.textures.base_color = Some(image);
                look.color[..3].fill(1.0);
            }
            if with_mrao {
                look.textures.metallic_roughness = self.image(
                    export,
                    Source::MetallicRoughness(textures.mrao as usize),
                    textures.mrao,
                    |read| Some(metallic_roughness(read)),
                );
            }
            if !textures.normal.is_null() {
                look.textures.normal = self.image(
                    export,
                    Source::Normal(textures.normal as usize),
                    textures.normal,
                    |read| Some(normals(read)),
                );
            }
            if glow > 0.0 && !textures.emissive.is_null() {
                look.textures.emissive = self.image(
                    export,
                    Source::Emissive(textures.emissive as usize),
                    textures.emissive,
                    |read| Some(read.image.clone()),
                );
            }
        }
        let textured = look.textures.used().next().is_some();
        (look, textured.then_some(looks.base_tiling))
    }

    /// The look of a brick with `looks` and `pattern`: the pattern painted as its base colour, the
    /// material's textures averaged (see the module's description); `None` if the pattern can't
    /// be read.
    ///
    /// # Safety
    /// As for [`Self::brick`].
    unsafe fn patterned(
        &mut self,
        export: &mut Export,
        looks: &Looks,
        pattern: &Pattern,
    ) -> Option<Look> {
        self.readback?;
        let textures = looks.textures;
        // SAFETY: per the contract.
        let [under, mrao, emissive] = [textures.diffuse, textures.mrao, textures.emissive]
            .map(|texture| unsafe { self.average(texture) });
        let source = Source::Pattern(
            pattern.texture as usize,
            textures.diffuse as usize,
            looks.color,
        );
        // SAFETY: per the contract.
        let base_color = unsafe {
            self.image(export, source, pattern.texture, |read| {
                Some(patterned(read, looks.color, under.unwrap_or([1.0; 4])))
            })?
        };
        let mut look = untextured(looks);
        look.name = format!("{} {}", look.name, pattern_name(&pattern.name));
        look.color[..3].fill(1.0);
        look.textures.base_color = Some(base_color);
        if let Some([metallic, roughness, ..]) = mrao {
            look.metallic = (look.metallic * metallic).clamp(0.0, 1.0);
            look.roughness = (look.roughness * roughness).clamp(0.0, 1.0);
        }
        if glow(looks) > 0.0 {
            if let Some(average) = emissive {
                for (channel, average) in look.emissive.iter_mut().zip(average) {
                    *channel *= average;
                }
            }
            // SAFETY: per the contract.
            look.textures.emissive = unsafe {
                self.image(
                    export,
                    Source::PatternGlow(pattern.texture as usize),
                    pattern.texture,
                    |read| Some(squared(read)),
                )
            };
        }
        Some(look)
    }

    /// The average linear colour of `texture` (sRGB ones made linear); `None` if it's null or
    /// can't be read.
    ///
    /// # Safety
    /// `texture` must be null or a live texture. Game thread only.
    unsafe fn average(&mut self, texture: *mut UObjectBase) -> Option<[f32; 4]> {
        if texture.is_null() {
            return None;
        }
        if let Some(known) = self.averages.get(&(texture as usize)) {
            return *known;
        }
        // SAFETY: per the contract.
        let average = match unsafe { self.read(texture) } {
            Ok(read) => Some(average(&read)),
            Err(_) => {
                self.unread += 1;
                None
            }
        };
        self.averages.insert(texture as usize, average);
        average
    }

    /// The image of a font's page `texture`: white, its distance field as alpha; `None` if it
    /// can't be read. Its size is the texture's.
    ///
    /// # Safety
    /// `texture` must be a live texture. Game thread only.
    pub unsafe fn glyphs(
        &mut self,
        export: &mut Export,
        texture: *mut UObjectBase,
    ) -> Option<(usize, [u32; 2])> {
        // SAFETY: per the contract.
        let index = unsafe {
            self.image(export, Source::Glyphs(texture as usize), texture, |read| {
                Some(distance_field(read))
            })?
        };
        let image = &export.images[index];
        Some((index, [image.width, image.height]))
    }

    /// The look of a wheel's tyre (`M_Tire_Master`: rubber whatever the brick's colour and
    /// material): the texture `rubber` if it reads, and with the mesh's UVs (`mesh_uvs`) the tyre's
    /// own normal map (its tread) and occlusion map; the tiling a box projection needs if it's
    /// textured without the mesh's UVs.
    ///
    /// # Safety
    /// `rubber` and the tyre's textures must be null or live textures. Game thread only.
    pub unsafe fn tyre(
        &mut self,
        export: &mut Export,
        rubber: *mut UObjectBase,
        tyre: Tyre,
        mesh_uvs: bool,
    ) -> (Look, Option<f32>) {
        let rubber_color = srgb_to_linear(TYRE_SRGB);
        let mut look = Look {
            name: "Tyre".into(),
            color: [rubber_color, rubber_color, rubber_color, 1.0],
            metallic: 0.0,
            roughness: TYRE_ROUGHNESS,
            ..Look::default()
        };
        if self.readback.is_some() && !rubber.is_null() {
            // Unpainted: the texture as it is.
            let unpainted = [0; 4];
            // SAFETY: per the contract.
            let image = unsafe {
                self.image(
                    export,
                    Source::BaseColor(rubber as usize, unpainted),
                    rubber,
                    |read| painted(read, unpainted),
                )
            };
            if let Some(image) = image {
                look.textures.base_color = Some(image);
                look.color[..3].fill(1.0);
            }
        }
        if !mesh_uvs {
            let textured = look.textures.base_color.is_some();
            return (look, textured.then_some(TYRE_TILING));
        }
        // SAFETY: per the contract.
        unsafe {
            if !tyre.normal.is_null() {
                look.textures.normal = self.image(
                    export,
                    Source::Normal(tyre.normal as usize),
                    tyre.normal,
                    |read| Some(normals(read)),
                );
            }
            // The game multiplies the colour by it; glTF's occlusion darkens the indirect light
            // only.
            if !tyre.occlusion.is_null() {
                look.textures.occlusion = self.image(
                    export,
                    Source::Occlusion(tyre.occlusion as usize),
                    tyre.occlusion,
                    |read| Some(read.image.clone()),
                );
            }
        }
        (look, None)
    }

    /// The look of text in `color` (linear bytes) on the page `image`.
    pub fn text(color: [u8; 4], image: usize) -> Look {
        let [red, green, blue, _] = color;
        let [r, g, b] = [red, green, blue].map(|channel| f32::from(channel) / 255.0);
        let mut look = Look {
            name: format!("Text #{red:02X}{green:02X}{blue:02X}"),
            color: [r, g, b, 1.0],
            alpha_cutoff: Some(GLYPH_CUTOFF),
            double_sided: true,
            ..Look::default()
        };
        look.textures.base_color = Some(image);
        look
    }

    /// The index in `export.images` of the image made by `make` from `texture` read back, added the
    /// first time; `None` if it can't be read or made.
    ///
    /// # Safety
    /// `texture` must be a live texture. Game thread only.
    unsafe fn image(
        &mut self,
        export: &mut Export,
        source: Source,
        texture: *mut UObjectBase,
        make: impl FnOnce(&Read) -> Option<Image>,
    ) -> Option<usize> {
        if let Some(known) = self.images.get(&source) {
            return *known;
        }
        // SAFETY: per the contract.
        let read = unsafe { self.read(texture) };
        let index = match read {
            Ok(read) => make(&read).map(|image| {
                export.images.push(image);
                export.images.len() - 1
            }),
            Err(_) => {
                self.unread += 1;
                None
            }
        };
        self.images.insert(source, index);
        index
    }

    /// `texture` read back, or how it couldn't be (logged the first time).
    ///
    /// # Safety
    /// As for [`Self::image`].
    unsafe fn read(&self, texture: *mut UObjectBase) -> Result<Arc<Read>, String> {
        let readback = self.readback.ok_or("textures can't be read")?;
        let mut read = READ.lock().unwrap_or_else(PoisonError::into_inner);
        let read = read.get_or_insert_with(HashMap::new);
        let key = texture as usize;
        if let Some(Known(known, result)) = read.get(&key)
            && known.get() == Some(texture)
        {
            return result.clone();
        }
        // SAFETY: per the contract; the context is the live editor.
        let result = unsafe { readback.read(self.context, texture, LARGEST) }.map(Arc::new);
        if let Err(reason) = &result {
            // SAFETY: a live object.
            let name = unsafe { crate::game::reflection::name_text(&(*texture).name_private) };
            log!("texture {name} not exported: {reason}");
        }
        // SAFETY: a live object.
        read.insert(
            key,
            Known(unsafe { WeakObject::new(texture) }, result.clone()),
        );
        result
    }
}

/// The look of a brick with `looks` before textures: named after its material and colour, with
/// the material's numbers (1 for metallic and roughness with an MRAO texture, which scales them).
fn untextured(looks: &Looks) -> Look {
    let [red, green, blue, alpha] = looks.color;
    let rgb = [red, green, blue].map(|channel| srgb_to_linear(f32::from(channel) / 255.0));
    let paint = f32::from(alpha) / 255.0;
    let with_mrao = !looks.textures.mrao.is_null();
    let metallic_alpha = if looks.paint_metallic_alpha > 0.0 {
        1.0 - looks.paint_metallic_alpha
    } else {
        1.0
    };
    let metallic = if with_mrao { 1.0 } else { looks.metallic };
    let roughness = if with_mrao { 1.0 } else { looks.roughness };
    let glow = glow(looks);
    Look {
        name: format!(
            "{} #{red:02X}{green:02X}{blue:02X}{}",
            if looks.material.is_empty() {
                "Brick"
            } else {
                &looks.material
            },
            if alpha < 255 {
                format!(" {}%", (paint * 100.0).round())
            } else {
                String::new()
            }
        ),
        color: [rgb[0], rgb[1], rgb[2], looks.opacity.clamp(0.0, 1.0)],
        metallic: (metallic * (1.0 - paint * metallic_alpha)).clamp(0.0, 1.0),
        roughness: roughness.clamp(0.0, 1.0),
        emissive: rgb.map(|channel| channel * glow),
        ..Look::default()
    }
}

/// How much a brick with `looks` glows, 0 to 1.
fn glow(looks: &Looks) -> f32 {
    looks.brightness.clamp(0.0, 1.0)
}

/// A pattern's name for its look's: its class's without the Blueprint suffix (`C_Army_C` is
/// `C_Army`).
fn pattern_name(class: &str) -> &str {
    class.strip_suffix("_C").unwrap_or(class)
}

/// A pattern with `color` painted over it as the brick material blends them (see the module's
/// description), over `under` (a linear colour: the diffuse texture's average, white without one)
/// where the paint doesn't cover.
fn patterned(read: &Read, color: [u8; 4], under: [f32; 4]) -> Image {
    let share = under[3] * f32::from(color[3]) / 255.0;
    let paint_rgb =
        [color[0], color[1], color[2]].map(|channel| srgb_to_linear(f32::from(channel) / 255.0));
    let mut image = read.image.clone();
    for pixel in image.rgba.as_chunks_mut::<4>().0 {
        for ((channel, painted), below) in pixel[..3].iter_mut().zip(paint_rgb).zip(under) {
            let pattern = linear(*channel, read.srgb);
            let above = painted * pattern;
            *channel = to_byte(linear_to_srgb(below + (above - below) * share));
        }
        pixel[3] = 255;
    }
    image
}

/// A pattern times itself, as sRGB: the emissive takes the pattern twice (once in the base
/// colour).
fn squared(read: &Read) -> Image {
    let mut image = read.image.clone();
    for pixel in image.rgba.as_chunks_mut::<4>().0 {
        for channel in &mut pixel[..3] {
            let value = linear(*channel, read.srgb);
            *channel = to_byte(linear_to_srgb(value * value));
        }
        pixel[3] = 255;
    }
    image
}

/// The average linear colour and alpha of a texture read back.
fn average(read: &Read) -> [f32; 4] {
    let pixels = read.image.rgba.as_chunks::<4>().0;
    if pixels.is_empty() {
        return [1.0; 4];
    }
    let mut sums = [0.0f64; 4];
    for pixel in pixels {
        for (channel, sum) in sums.iter_mut().enumerate() {
            let srgb = read.srgb && channel < 3;
            *sum += f64::from(linear(pixel[channel], srgb));
        }
    }
    sums.map(|sum| (sum / pixels.len() as f64) as f32)
}

/// A texture's byte as a linear value, 0 to 1 (made linear if the texture is sRGB).
fn linear(byte: u8, srgb: bool) -> f32 {
    let value = f32::from(byte) / 255.0;
    if srgb { srgb_to_linear(value) } else { value }
}

/// A diffuse texture with `color` painted over it, or `None` if the paint covers it all (the
/// colour alone shows).
fn painted(read: &Read, color: [u8; 4]) -> Option<Image> {
    let paint = f32::from(color[3]) / 255.0;
    let covered = |alpha: u8| f32::from(alpha) / 255.0 * paint >= 1.0;
    let pixels = read.image.rgba.as_chunks::<4>().0;
    if pixels.iter().all(|pixel| covered(pixel[3])) {
        return None;
    }
    let paint_rgb = [color[0], color[1], color[2]].map(|channel| linear(channel, true));
    let mut image = read.image.clone();
    for pixel in image.rgba.as_chunks_mut::<4>().0 {
        let share = f32::from(pixel[3]) / 255.0 * paint;
        for (channel, painted) in pixel[..3].iter_mut().zip(paint_rgb) {
            let below = linear(*channel, read.srgb);
            *channel = to_byte(linear_to_srgb(below + (painted - below) * share));
        }
        pixel[3] = 255;
    }
    Some(image)
}

/// glTF's metallic-roughness image from an MRAO texture (metallic, roughness, occlusion): roughness
/// in green, metallic in blue.
fn metallic_roughness(read: &Read) -> Image {
    let mut image = read.image.clone();
    for pixel in image.rgba.as_chunks_mut::<4>().0 {
        let [metallic, roughness, _, _] = [pixel[0], pixel[1], pixel[2], pixel[3]];
        *pixel = [255, roughness, metallic, 255];
    }
    image
}

/// glTF's normal image from an Unreal normal map (two channels, `TC_Normalmap`): the third worked
/// out, green turned around (Unreal's normal maps have Y down, glTF's up).
fn normals(read: &Read) -> Image {
    let mut image = read.image.clone();
    for pixel in image.rgba.as_chunks_mut::<4>().0 {
        let [x, y] = [pixel[0], pixel[1]].map(|byte| f32::from(byte) / 255.0 * 2.0 - 1.0);
        let z = (1.0 - x * x - y * y).max(0.0).sqrt();
        let byte = |value: f32| to_byte((value + 1.0) / 2.0);
        *pixel = [pixel[0], byte(-y), byte(z), 255];
    }
    image
}

/// A font page as white with its distance field as alpha: the alpha it came with, or its red if
/// alpha didn't come through (single-channel pages).
fn distance_field(read: &Read) -> Image {
    let in_alpha = read
        .image
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .any(|pixel| pixel[3] < 255);
    let mut image = read.image.clone();
    for pixel in image.rgba.as_chunks_mut::<4>().0 {
        let field = if in_alpha { pixel[3] } else { pixel[0] };
        *pixel = [255, 255, 255, field];
    }
    image
}

/// `value` (0 to 1) as a byte.
fn to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}
