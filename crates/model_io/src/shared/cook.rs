//! Fitting an image or a model under an [`Aim`]: smaller pictures, fewer triangles, until the
//! cooked file is small enough.

use super::format::{self, Body, EncodedMaterial, MAX_MATERIALS, MAX_PARTS};
use super::simplify;
use super::textures::{self, BEST_QUALITY, WORST_QUALITY};
use super::{Aim, Cooked, image_file};
use crate::image::{self, Image};
use crate::model::{Model, Part};

/// How often a picture is made smaller, to fit its bytes.
const IMAGE_ROUNDS: usize = 8;

/// A picture's side when it has to shrink to fit its bytes: this much of the last one.
const IMAGE_STEP: f64 = 0.8;

/// How often a model is made smaller, after the first try, to fit its bytes.
const MODEL_RETRIES: usize = 5;

/// What a model's bytes are lowered by, on top of what they are over: a margin, so that the next
/// try fits.
const BYTES_MARGIN: f64 = 0.9;

/// The JPEG qualities an opaque image is tried at, best first.
const IMAGE_QUALITIES: [u8; 4] = [BEST_QUALITY, 80, 70, WORST_QUALITY];

/// Pictures a model past [`MAX_MATERIALS`] keeps: the other materials need some room for their
/// colours.
const PICTURES_WHEN_COMBINING: usize = MAX_MATERIALS * 3 / 4;

/// About how many pixels of a picture tell its average colour.
const AVERAGED_PIXELS: usize = 4096;

pub(super) fn cook_image(file: &[u8], aim: &Aim) -> Result<Cooked, String> {
    if aim.pixels == 0 || aim.longest_side == 0 || aim.bytes == 0 {
        return Err("the aim has no room for an image".into());
    }
    let picture = image::decode(file)?;
    let side = aim.longest_side.min(image::MAX_SIDE);
    let (width, height) = (picture.width, picture.height);
    let pixels = u32::try_from(u64::from(width) * u64::from(height)).map_err(|_| "too big")?;
    let within = width.max(height) <= side && u64::from(pixels) <= aim.pixels;
    let kept = image_file(file);
    if within && kept.len() as u64 <= aim.bytes {
        return Ok(Cooked::new(kept, 0, pixels, false));
    }

    let (mut width, mut height) = textures::sizes_within(&[(width, height)], 1.0, side, aim.pixels)
        .and_then(|sizes| sizes.first().copied())
        .ok_or("the image doesn't fit the pixels")?;
    let qualities: &[u8] = if picture.is_opaque() {
        &IMAGE_QUALITIES
    } else {
        &[BEST_QUALITY]
    };
    for _ in 0..IMAGE_ROUNDS {
        let resized = (width, height) != (picture.width, picture.height);
        let scaled = textures::shrunk(&picture, width, height);
        for &quality in qualities {
            let encoded = textures::encode(&scaled, quality, resized)?;
            let cooked = image_file(&encoded.file);
            if cooked.len() as u64 <= aim.bytes {
                let pixels = encoded.width * encoded.height;
                return Ok(Cooked::new(cooked, 0, pixels, encoded.lowered));
            }
        }
        if (width, height) == (1, 1) {
            break;
        }
        let smaller = |side: u32| ((f64::from(side) * IMAGE_STEP) as u32).max(1);
        (width, height) = (smaller(width), smaller(height));
    }
    Err(format!("the image doesn't fit in {} bytes", aim.bytes))
}

/// A material as it is cooked: its colour and the picture it keeps.
type Look<'a> = ([f32; 4], Option<&'a Image>);

/// The model as it is cooked: only what is used, with the pictures it keeps.
struct Prepared<'a> {
    /// The materials that parts use.
    materials: Vec<Look<'a>>,
    parts: Vec<Part>,
    /// Whether materials were combined, or a picture left out for want of pixels.
    looks_lowered: bool,
}

/// How the cooking of a model goes on.
struct Plan {
    /// The triangles all parts may have together.
    triangles: usize,
    /// How much of its own size a texture may keep.
    texture_scale: f64,
    /// The JPEG quality opaque textures are encoded at.
    quality: u8,
}

pub(super) fn cook_model(model: &Model, aim: &Aim) -> Result<Cooked, String> {
    model.validate()?;
    let side = aim.longest_side.min(image::MAX_SIDE);
    let Prepared {
        materials,
        parts,
        looks_lowered,
    } = prepare(model, aim)?;
    let original: usize = parts.iter().map(|part| part.corners.len()).sum();
    if original > 0 && aim.triangles == 0 {
        return Err("the aim has no room for triangles".into());
    }
    let mut plan = Plan {
        triangles: usize::try_from(aim.triangles)
            .unwrap_or(usize::MAX)
            .min(original),
        texture_scale: 1.0,
        quality: BEST_QUALITY,
    };

    let pictures: Vec<&Image> = materials
        .iter()
        .filter_map(|&(_, picture)| picture)
        .collect();
    let mut parts = parts;
    for _ in 0..=MODEL_RETRIES {
        let sizes: Vec<(u32, u32)> = pictures.iter().map(|p| (p.width, p.height)).collect();
        let fitted = textures::sizes_within(&sizes, plan.texture_scale, side, aim.pixels)
            .ok_or("the textures don't fit the pixels")?;
        let mut files = Vec::with_capacity(pictures.len());
        for (picture, &(width, height)) in pictures.iter().zip(&fitted) {
            let resized = (width, height) != (picture.width, picture.height);
            let scaled = textures::shrunk(picture, width, height);
            files.push(textures::encode(&scaled, plan.quality, resized)?);
        }

        parts = fit(&parts, plan.triangles);
        let triangles: usize = parts.iter().map(|part| part.corners.len()).sum();
        let mut files_of_materials = files.iter();
        let body = Body {
            materials: materials
                .iter()
                .map(|&(color, picture)| EncodedMaterial {
                    color,
                    texture: picture
                        .and_then(|_| files_of_materials.next())
                        .map(|encoded| encoded.file.clone()),
                })
                .collect(),
            parts: parts.clone(),
        };
        let file = format::write(&body)?;
        if file.len() as u64 <= aim.bytes {
            let pixels: u64 = files
                .iter()
                .map(|f| u64::from(f.width) * u64::from(f.height))
                .sum();
            let simplified =
                triangles < original || looks_lowered || files.iter().any(|f| f.lowered);
            return Ok(Cooked::new(
                file,
                u32::try_from(triangles).map_err(|_| "too many triangles")?,
                u32::try_from(pixels).map_err(|_| "too many pixels")?,
                simplified,
            ));
        }

        // Too big: everything lowered by what it is over, and a margin.
        let factor = aim.bytes as f64 / file.len() as f64 * BYTES_MARGIN;
        plan.triangles = (triangles as f64 * factor) as usize;
        plan.texture_scale *= factor.sqrt();
        plan.quality = plan.quality.saturating_sub(10).max(WORST_QUALITY);
        if triangles > 0 && plan.triangles == 0 {
            break;
        }
    }
    Err(format!("the model doesn't fit in {} bytes", aim.bytes))
}

/// The parts with at most `target` triangles between them, in proportion to their own.
fn fit(parts: &[Part], target: usize) -> Vec<Part> {
    let counts: Vec<usize> = parts.iter().map(|part| part.corners.len()).collect();
    let shares = simplify::allocate(&counts, target);
    parts
        .iter()
        .zip(shares)
        .filter(|(_, share)| *share > 0)
        .map(|(part, share)| simplify::reduced(part, share))
        .collect()
}

/// What of the model is cooked: parts with triangles, the materials they use (past 64, combined:
/// [`combine_materials`]), and UVs only where a picture is shown. Refuses what couldn't be written
/// as numbers.
fn prepare<'a>(model: &'a Model, aim: &Aim) -> Result<Prepared<'a>, String> {
    let mut used: Vec<Option<usize>> = vec![None; model.materials.len()];
    for part in model.parts.iter().filter(|part| !part.corners.is_empty()) {
        if let Some(material) = part.material {
            used[material] = Some(0);
        }
    }
    let wanted_pictures = model
        .materials
        .iter()
        .zip(&used)
        .filter(|(material, used)| used.is_some() && has_picture(material.texture.as_ref()))
        .count();
    // One pixel each, at the very least.
    let pictures_fit = wanted_pictures as u64 <= aim.pixels;

    let mut materials: Vec<Look> = Vec::new();
    let mut looks_lowered = false;
    for (material, slot) in model.materials.iter().zip(&mut used) {
        if slot.is_none() {
            continue;
        }
        *slot = Some(materials.len());
        let picture = material
            .texture
            .as_ref()
            .filter(|picture| has_picture(Some(picture)));
        looks_lowered |= picture.is_some() && !pictures_fit;
        materials.push((material.color, picture.filter(|_| pictures_fit)));
    }
    if materials.len() > MAX_MATERIALS {
        let mut triangles = vec![0u64; materials.len()];
        for part in &model.parts {
            if let Some(material) = part.material.and_then(|material| used[material]) {
                triangles[material] += part.corners.len() as u64;
            }
        }
        let (kept, moved) = combine_materials(&materials, &triangles);
        // Materials that look the same becoming one changes nothing to tell a player about.
        looks_lowered |= materials.iter().zip(&moved).any(|(before, &after)| {
            let now = &kept[after];
            before.0 != now.0 || before.1.is_some() != now.1.is_some()
        });
        materials = kept;
        for slot in used.iter_mut().flatten() {
            *slot = moved[*slot];
        }
    }

    let finite = |value: f32| value.is_finite() && value.abs() <= 1e9;
    let mut parts = Vec::new();
    for part in model.parts.iter().filter(|part| !part.corners.is_empty()) {
        let material = part.material.and_then(|material| used[material]);
        let mut part = Part {
            name: String::new(),
            material,
            ..part.clone()
        };
        let textured = material.is_some_and(|material| materials[material].1.is_some());
        if !textured {
            part.uvs.clear();
        }
        if !part
            .positions
            .iter()
            .flatten()
            .chain(part.uvs.iter().flatten())
            .all(|&v| finite(v))
        {
            return Err("the model has numbers that aren't numbers, or past 1e9".into());
        }
        parts.push(part);
    }
    if materials
        .iter()
        .any(|(color, _)| !color.iter().all(|&value| value.is_finite()))
    {
        return Err("a material colour isn't a number".into());
    }
    if parts.len() > MAX_PARTS {
        parts = merged_by_material(parts);
    }
    Ok(Prepared {
        materials,
        parts,
        looks_lowered,
    })
}

fn has_picture(picture: Option<&Image>) -> bool {
    picture.is_some_and(|picture| picture.width > 0 && picture.height > 0)
}

/// `materials` brought down to [`MAX_MATERIALS`], with where each one went. The pictures covering
/// the most triangles stay, up to [`PICTURES_WHEN_COMBINING`]; every other material becomes a plain
/// colour, and those closest in colour share one. `triangles` is how much each material covers.
fn combine_materials<'a>(materials: &[Look<'a>], triangles: &[u64]) -> (Vec<Look<'a>>, Vec<usize>) {
    let mut pictured: Vec<usize> = (0..materials.len())
        .filter(|&material| materials[material].1.is_some())
        .collect();
    pictured.sort_by_key(|&material| std::cmp::Reverse(triangles[material]));
    pictured.truncate(PICTURES_WHEN_COMBINING);

    let plain: Vec<usize> = (0..materials.len())
        .filter(|material| !pictured.contains(material))
        .collect();
    let colors: Vec<[f32; 4]> = materials.iter().map(|&look| seen_color(look)).collect();

    let mut kept = Vec::with_capacity(MAX_MATERIALS);
    let mut moved = vec![0; materials.len()];
    for &material in &pictured {
        moved[material] = kept.len();
        kept.push(materials[material]);
    }
    for group in color_groups(plain, &colors, MAX_MATERIALS - pictured.len()) {
        for &material in &group {
            moved[material] = kept.len();
        }
        kept.push((average_color(&group, &colors, triangles), None));
    }
    (kept, moved)
}

/// How a material looks from afar: its colour, times its picture's average.
fn seen_color((color, picture): Look) -> [f32; 4] {
    let Some(picture) = picture else {
        return color;
    };
    // A few thousand pixels tell the average of any picture.
    let step = (picture.rgba.len() / 4 / AVERAGED_PIXELS).max(1);
    let mut sum = [0.0f64; 4];
    let mut count = 0.0f64;
    for pixel in picture.rgba.chunks_exact(4).step_by(step) {
        let linear = |value: u8| (f64::from(value) / 255.0).powf(2.2);
        sum[0] += linear(pixel[0]);
        sum[1] += linear(pixel[1]);
        sum[2] += linear(pixel[2]);
        sum[3] += f64::from(pixel[3]) / 255.0;
        count += 1.0;
    }
    if count == 0.0 {
        return color;
    }
    std::array::from_fn(|channel| color[channel] * (sum[channel] / count) as f32)
}

/// The largest gap between the colours of a group, on one channel.
struct Gap {
    size: f32,
    channel: usize,
    /// Colours from here up go on one side of it.
    at: f32,
}

/// A colour's channel as far as telling colours apart goes: the eye sees dark colours differ more.
fn tone(color: &[f32; 4], channel: usize) -> f32 {
    color[channel].max(0.0).sqrt()
}

fn widest_gap(group: &[usize], colors: &[[f32; 4]]) -> Option<Gap> {
    let mut widest: Option<Gap> = None;
    for channel in 0..4 {
        let mut tones: Vec<f32> = group
            .iter()
            .map(|&material| tone(&colors[material], channel))
            .collect();
        tones.sort_by(f32::total_cmp);
        for pair in tones.windows(2) {
            let size = pair[1] - pair[0];
            if size > widest.as_ref().map_or(0.0, |gap| gap.size) {
                widest = Some(Gap {
                    size,
                    channel,
                    at: pair[1],
                });
            }
        }
    }
    widest
}

/// `materials` in at most `most` groups of close colours: split at the largest gap between colours
/// until there are enough groups, so the ones left together are the closest.
fn color_groups(materials: Vec<usize>, colors: &[[f32; 4]], most: usize) -> Vec<Vec<usize>> {
    if materials.is_empty() {
        return Vec::new();
    }
    let mut groups = vec![materials];
    while groups.len() < most {
        let widest = groups
            .iter()
            .enumerate()
            .filter_map(|(index, group)| Some((index, widest_gap(group, colors)?)))
            .max_by(|(_, a), (_, b)| a.size.total_cmp(&b.size));
        let Some((index, gap)) = widest else {
            break;
        };
        let (below, above) = groups[index]
            .iter()
            .partition(|&&material| tone(&colors[material], gap.channel) < gap.at);
        groups[index] = below;
        groups.push(above);
    }
    groups
}

/// The colour of `group`, each material counting for the triangles it covers.
fn average_color(group: &[usize], colors: &[[f32; 4]], triangles: &[u64]) -> [f32; 4] {
    let mut sum = [0.0f64; 4];
    let mut total = 0.0f64;
    for &material in group {
        let weight = triangles[material].max(1) as f64;
        total += weight;
        for (sum, &value) in sum.iter_mut().zip(&colors[material]) {
            *sum += f64::from(value) * weight;
        }
    }
    sum.map(|value| (value / total) as f32)
}

/// One part for each material: what looks the same as one part. (Parts keep their vertices apart,
/// so nothing is shaded differently.)
fn merged_by_material(parts: Vec<Part>) -> Vec<Part> {
    let mut merged: Vec<Part> = Vec::new();
    for part in parts {
        let Some(target) = merged.iter_mut().find(|m| m.material == part.material) else {
            merged.push(part);
            continue;
        };
        let offset = target.positions.len() as u32;
        target.positions.extend(part.positions);
        target.uvs.extend(part.uvs);
        target.corners.extend(
            part.corners
                .iter()
                .map(|corner| corner.map(|index| index + offset)),
        );
    }
    merged
}
