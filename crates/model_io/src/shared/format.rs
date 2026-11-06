//! The bytes of a cooked model: writing them, and reading them back from a source nobody trusts.
//!
//! After the header (`mod.rs`) comes the body's length and its zlib stream. The body holds the
//! model's bounds, its materials (colour, optional PNG or JPEG texture) and its parts: positions
//! quantized to 16 bits within the bounds, three planes (x, y, z) of wrapping deltas; UVs as
//! floats; corners as zigzag deltas in LEB128 varints. See `docs/coop-references.md`.
//!
//! The reader never panics (the game aborts on a panic): every count is checked against the bytes
//! left before anything is allocated for it, and every read goes through [`Reader`].

use miniz_oxide::deflate::compress_to_vec_zlib;
use miniz_oxide::inflate::stream::{InflateState, inflate};
use miniz_oxide::{DataFormat, MZFlush, MZStatus};

use super::{KIND_MODEL, Limits, MAGIC, VERSION};
use crate::image;
use crate::model::{Material, Model, Part};

/// Materials a model may have: a part names its material in a byte.
pub(super) const MAX_MATERIALS: usize = 64;

/// Parts a model may have.
pub(super) const MAX_PARTS: usize = 65_536;

/// A part's material byte when it has none.
const NO_MATERIAL: u8 = 255;

/// Positions and bounds stay within this, so that nothing overflows later.
const MAX_COORDINATE: f32 = 1e9;

/// zlib level of the body.
const COMPRESSION_LEVEL: u8 = 6;

/// What the body is made of, with the textures already encoded.
pub(super) struct Body {
    pub materials: Vec<EncodedMaterial>,
    pub parts: Vec<Part>,
}

pub(super) struct EncodedMaterial {
    pub color: [f32; 4],
    /// A PNG or JPEG file.
    pub texture: Option<Vec<u8>>,
}

/// The whole cooked file of `body`.
pub(super) fn write(body: &Body) -> Result<Vec<u8>, String> {
    pack(&write_body(body)?)
}

/// The cooked file of a body's bytes: header, length, zlib.
pub(super) fn pack(plain: &[u8]) -> Result<Vec<u8>, String> {
    let length = u32::try_from(plain.len()).map_err(|_| "the model is too big")?;
    let packed = compress_to_vec_zlib(plain, COMPRESSION_LEVEL);
    let mut file = Vec::with_capacity(packed.len() + 10);
    file.extend_from_slice(&MAGIC);
    file.extend_from_slice(&[VERSION, KIND_MODEL]);
    put_u32(&mut file, length);
    file.extend_from_slice(&packed);
    Ok(file)
}

pub(super) fn write_body(body: &Body) -> Result<Vec<u8>, String> {
    if body.materials.len() > MAX_MATERIALS {
        return Err(format!("more than {MAX_MATERIALS} materials"));
    }
    if body.parts.len() > MAX_PARTS {
        return Err(format!("more than {MAX_PARTS} parts"));
    }
    let (min, max) = bounds_of(&body.parts)?;
    let mut out = Vec::new();
    for value in min.into_iter().chain(max) {
        put_f32(&mut out, value);
    }

    out.push(body.materials.len() as u8);
    for material in &body.materials {
        for value in material.color {
            put_f32(&mut out, value);
        }
        let texture = material.texture.as_deref().unwrap_or_default();
        put_u32(&mut out, texture.len() as u32);
        out.extend_from_slice(texture);
    }

    put_u32(&mut out, body.parts.len() as u32);
    for part in &body.parts {
        out.push(part.material.map_or(NO_MATERIAL, |material| material as u8));
        put_u32(&mut out, part.positions.len() as u32);
        put_u32(&mut out, part.corners.len() as u32);
        out.push(u8::from(!part.uvs.is_empty()));
        for axis in 0..3 {
            let mut previous = 0u16;
            for position in &part.positions {
                let quantized = quantize(position[axis], min[axis], max[axis]);
                out.extend_from_slice(&quantized.wrapping_sub(previous).to_le_bytes());
                previous = quantized;
            }
        }
        for uv in &part.uvs {
            put_f32(&mut out, uv[0]);
            put_f32(&mut out, uv[1]);
        }
        let mut previous = 0i64;
        for &index in part.corners.iter().flatten() {
            let index = i64::from(index);
            put_varint(&mut out, zigzag(index - previous));
            previous = index;
        }
    }
    Ok(out)
}

/// The box around every position (zeros without any). Fails for what a reader would refuse.
fn bounds_of(parts: &[Part]) -> Result<([f32; 3], [f32; 3]), String> {
    let mut positions = parts.iter().flat_map(|part| part.positions.iter());
    let Some(&first) = positions.next() else {
        return Ok(([0.0; 3], [0.0; 3]));
    };
    let (mut min, mut max) = (first, first);
    for position in positions {
        for axis in 0..3 {
            min[axis] = min[axis].min(position[axis]);
            max[axis] = max[axis].max(position[axis]);
        }
    }
    let usable = |value: f32| value.is_finite() && value.abs() <= MAX_COORDINATE;
    if !min.into_iter().chain(max).all(usable) {
        return Err("the model has positions that are not numbers, or past 1e9".into());
    }
    Ok((min, max))
}

/// `value` as a step of 1/65535 of the way from `min` to `max`.
fn quantize(value: f32, min: f32, max: f32) -> u16 {
    let (min, max) = (f64::from(min), f64::from(max));
    if max <= min {
        return 0;
    }
    ((f64::from(value) - min) / (max - min) * 65535.0)
        .round()
        .clamp(0.0, 65535.0) as u16
}

/// What a step of `quantize` stands for.
fn dequantize(step: u16, min: f32, max: f32) -> f32 {
    let (min, max) = (f64::from(min), f64::from(max));
    (min + f64::from(step) / 65535.0 * (max - min)) as f32
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// A signed number as an unsigned one that is small when the signed one is.
fn zigzag(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

fn unzigzag(value: u64) -> i64 {
    (value >> 1) as i64 ^ -((value & 1) as i64)
}

/// LEB128: seven bits at a time, low first, the top bit saying more follows.
fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value & 0x7f) as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Reads a cooked model from `bytes` (the whole file, header included).
pub(super) fn read(bytes: &[u8], limits: &Limits) -> Result<Model, String> {
    let rest = super::split_header(bytes, KIND_MODEL)?;
    let mut reader = Reader(rest);
    let length = u64::from(reader.u32()?);
    // What a body within the limits could hold: a triangle with its vertices is far below 80 bytes,
    // a texture's pixels are at most 4 bytes each, plus the small rest.
    let most = limits
        .triangles
        .saturating_mul(80)
        .saturating_add(limits.pixels.saturating_mul(4))
        .saturating_add(64 << 20);
    if length > most {
        return Err(format!(
            "a body of {length} bytes is past what the limits allow"
        ));
    }
    let length = usize::try_from(length).map_err(|_| "the body is too big")?;
    let body = unpack(reader.0, length)?;
    read_body(&mut Reader(&body), limits)
}

/// What the zlib stream `packed` holds, which must be exactly `length` bytes and all of `packed`.
/// It is unpacked a piece at a time and given up as soon as it is past `length`, so a stream that
/// says little and unpacks to a lot costs next to nothing.
fn unpack(mut packed: &[u8], length: usize) -> Result<Vec<u8>, String> {
    let damaged = || "the model's data is damaged".to_string();
    let mut state = InflateState::new_boxed(DataFormat::Zlib);
    let mut piece = vec![0u8; 64 << 10];
    let mut plain = Vec::new();
    loop {
        let step = inflate(&mut state, packed, &mut piece, MZFlush::None);
        plain.extend_from_slice(piece.get(..step.bytes_written).ok_or_else(damaged)?);
        packed = packed.get(step.bytes_consumed..).ok_or_else(damaged)?;
        if plain.len() > length {
            return Err("the model's data is longer than it says".into());
        }
        match step.status {
            Ok(MZStatus::StreamEnd) => break,
            // More to come: but only if the last step got anywhere.
            Ok(_) if step.bytes_consumed + step.bytes_written > 0 => {}
            _ => return Err(damaged()),
        }
    }
    if plain.len() != length {
        return Err("the model's data isn't as long as it says".into());
    }
    if !packed.is_empty() {
        return Err("the model's data has more after its end".into());
    }
    Ok(plain)
}

fn read_body(reader: &mut Reader, limits: &Limits) -> Result<Model, String> {
    let mut bounds = [0.0f32; 6];
    for value in &mut bounds {
        *value = reader.finite_f32()?;
    }
    let min = [bounds[0], bounds[1], bounds[2]];
    let max = [bounds[3], bounds[4], bounds[5]];
    if bounds.iter().any(|value| value.abs() > MAX_COORDINATE) || (0..3).any(|a| min[a] > max[a]) {
        return Err("the model's bounds are not right".into());
    }

    let material_count = usize::from(reader.u8()?);
    if material_count > MAX_MATERIALS {
        return Err(format!("{material_count} materials"));
    }
    let mut materials = Vec::with_capacity(material_count);
    let mut pixels = 0u64;
    for _ in 0..material_count {
        let mut color = [0.0f32; 4];
        for value in &mut color {
            *value = reader.finite_f32()?;
        }
        let length = reader.u32()? as usize;
        let texture = match length {
            0 => None,
            _ => {
                let file = reader.take(length)?;
                // The header first: a small file can declare a huge picture.
                let (width, height) = image::dimensions(file)?;
                if width.max(height) > limits.longest_side {
                    return Err(format!("a texture of {width} x {height} is too big"));
                }
                pixels += u64::from(width) * u64::from(height);
                if pixels > limits.pixels {
                    return Err("the textures have too many pixels".into());
                }
                Some(image::decode(file)?)
            }
        };
        materials.push(Material {
            name: String::new(),
            color,
            texture,
        });
    }

    let part_count = reader.u32()? as usize;
    // A part is at least its 10 bytes of header.
    if part_count > MAX_PARTS || part_count * 10 > reader.left() {
        return Err(format!("{part_count} parts"));
    }
    let mut parts = Vec::with_capacity(part_count);
    let mut triangles = 0u64;
    for _ in 0..part_count {
        let material = match reader.u8()? {
            NO_MATERIAL => None,
            index if usize::from(index) < materials.len() => Some(usize::from(index)),
            _ => return Err("a part has an unknown material".into()),
        };
        let vertex_count = u64::from(reader.u32()?);
        let triangle_count = u64::from(reader.u32()?);
        let has_uvs = match reader.u8()? {
            0 => false,
            1 => true,
            _ => return Err("a part's UV flag is not right".into()),
        };
        triangles += triangle_count;
        if triangles > limits.triangles {
            return Err(format!("more than {} triangles", limits.triangles));
        }
        // What the part needs at least, before anything is allocated for it: 6 bytes a position,
        // 8 a UV pair, 3 a triangle (a byte a corner).
        let needed = vertex_count * if has_uvs { 14 } else { 6 } + triangle_count * 3;
        if needed > reader.left() as u64 {
            return Err("the model's data ends too soon".into());
        }
        let (vertex_count, triangle_count) = (vertex_count as usize, triangle_count as usize);

        let mut positions = vec![[0.0f32; 3]; vertex_count];
        for axis in 0..3 {
            let plane = reader.take(vertex_count * 2)?;
            let mut step = 0u16;
            for (position, delta) in positions.iter_mut().zip(plane.as_chunks::<2>().0) {
                step = step.wrapping_add(u16::from_le_bytes(*delta));
                position[axis] = dequantize(step, min[axis], max[axis]);
            }
        }
        let mut uvs = Vec::new();
        if has_uvs {
            uvs.reserve_exact(vertex_count);
            for _ in 0..vertex_count {
                uvs.push([reader.finite_f32()?, reader.finite_f32()?]);
            }
        }
        let mut corners = Vec::with_capacity(triangle_count);
        let mut previous = 0i64;
        for _ in 0..triangle_count {
            let mut triangle = [0u32; 3];
            for corner in &mut triangle {
                let index = previous + unzigzag(reader.varint()?);
                if index < 0 || index >= vertex_count as i64 {
                    return Err("a triangle points past its positions".into());
                }
                *corner = index as u32;
                previous = index;
            }
            corners.push(triangle);
        }
        parts.push(Part {
            name: String::new(),
            material,
            positions,
            uvs,
            corners,
        });
    }
    if reader.left() > 0 {
        return Err("the model's data has more after its end".into());
    }
    let model = Model { parts, materials };
    model.validate()?;
    Ok(model)
}

/// The bytes not read yet. Every read checks what is left: nothing here panics on any input.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn left(&self) -> usize {
        self.0.len()
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        if count > self.0.len() {
            return Err("the model's data ends too soon".into());
        }
        let (head, tail) = self.0.split_at(count);
        self.0 = tail;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, String> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn finite_f32(&mut self) -> Result<f32, String> {
        let bytes = self.take(4)?;
        let value = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        if !value.is_finite() {
            return Err("the model has a number that isn't one".into());
        }
        Ok(value)
    }

    /// A LEB128 number of at most 35 bits (a zigzag of a difference of two `u32`s needs 33).
    fn varint(&mut self) -> Result<u64, String> {
        let mut value = 0u64;
        for shift in (0..35).step_by(7) {
            let byte = self.u8()?;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err("a number in the model's data is too long".into())
    }
}
