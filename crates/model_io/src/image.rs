//! Reference images and model textures, decoded to RGBA8.
//!
//! PNG (any colour type and bit depth) and JPEG (grayscale, YCbCr, CMYK) are read, told apart by
//! their first bytes, not by a file name: glTF and OBJ files may name anything. Anything bigger
//! than [`MAX_SIDE`] is scaled down, since a huge picture would only cost memory: the editor
//! shows it far smaller than that.
//!
//! Gotcha: files are untrusted and a few bytes can declare a gigantic picture, so the declared
//! size is checked against [`MAX_PIXELS`] before anything is allocated for it.

use std::io::Cursor;
use std::path::Path;

use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

/// The longest side an [`Image`] may have, in pixels.
pub const MAX_SIDE: u32 = 8192;

/// The most pixels a file may declare before it is decoded (16384 x 16384, a 1 GiB RGBA picture).
const MAX_PIXELS: u64 = 16384 * 16384;

/// The first bytes of every PNG file.
const PNG_MAGIC: [u8; 4] = [0x89, b'P', b'N', b'G'];

/// The first bytes of every JPEG file (a start-of-image marker, then another marker).
const JPEG_MAGIC: [u8; 3] = [0xFF, 0xD8, 0xFF];

/// An RGBA8 image.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// Width over height (1.0 for an empty image), so a plane can be given the picture's shape.
    pub fn aspect(&self) -> f32 {
        if self.width == 0 || self.height == 0 {
            return 1.0;
        }
        self.width as f32 / self.height as f32
    }

    /// The picture's average color, linear RGB (pixels weighted by their alpha) and the average
    /// alpha: for a look that can't show the picture itself. White if nothing is opaque. Big pictures
    /// are sampled, not read whole.
    pub fn average_color(&self) -> [f32; 4] {
        const MAX_SAMPLES: usize = 1 << 20;
        let to_linear: [f32; 256] =
            core::array::from_fn(|value| srgb_to_linear(value as f32 / 255.0));
        let pixels = self.rgba.as_chunks::<4>().0;
        let step = pixels.len().div_ceil(MAX_SAMPLES).max(1);
        let (mut sum, mut weight, mut samples) = ([0.0f64; 3], 0.0f64, 0usize);
        for &[r, g, b, a] in pixels.iter().step_by(step) {
            let alpha = f64::from(a) / 255.0;
            for (total, value) in sum.iter_mut().zip([r, g, b]) {
                *total += f64::from(to_linear[usize::from(value)]) * alpha;
            }
            weight += alpha;
            samples += 1;
        }
        if weight <= 0.0 {
            return [1.0, 1.0, 1.0, 0.0];
        }
        let [r, g, b] = sum.map(|total| (total / weight) as f32);
        [r, g, b, (weight / samples as f64) as f32]
    }

    /// The image with its colors multiplied by `tint` (linear RGB, as the engine's materials
    /// multiply them), alpha kept.
    pub fn tinted(&self, tint: [f32; 3]) -> Image {
        // The result for every value of a channel: one table per channel, then a lookup per pixel.
        let tables: [[u8; 256]; 3] = tint.map(|tint| {
            core::array::from_fn(|value| {
                let linear = srgb_to_linear(value as f32 / 255.0) * tint.max(0.0);
                (linear_to_srgb(linear.min(1.0)) * 255.0 + 0.5) as u8
            })
        });
        let mut rgba = self.rgba.clone();
        for pixel in rgba.as_chunks_mut::<4>().0 {
            for (value, table) in pixel.iter_mut().zip(&tables) {
                *value = table[usize::from(*value)];
            }
        }
        Image {
            rgba,
            ..self.clone()
        }
    }

    /// The image as a PNG file's contents: the engine's texture import (`ImportBufferAsTexture2D`)
    /// takes files, not pixels. Fast compression: it's decoded again right away.
    pub fn to_png(&self) -> Result<Vec<u8>, String> {
        self.encode_png(png::Compression::Fast, png::Filter::NoFilter)
    }

    /// The image as a PNG file that is meant to stay small (for sending), not quick to make.
    pub(crate) fn to_small_png(&self) -> Result<Vec<u8>, String> {
        self.encode_png(png::Compression::Balanced, png::Filter::Adaptive)
    }

    fn encode_png(
        &self,
        compression: png::Compression,
        filter: png::Filter,
    ) -> Result<Vec<u8>, String> {
        let not_written = |error: png::EncodingError| format!("PNG not written: {error}");
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(compression);
        encoder.set_filter(filter);
        let mut writer = encoder.write_header().map_err(not_written)?;
        writer.write_image_data(&self.rgba).map_err(not_written)?;
        writer.finish().map_err(not_written)?;
        Ok(bytes)
    }

    /// The image as a JPEG file (`quality` 1 to 100), alpha dropped.
    pub(crate) fn to_jpeg(&self, quality: u8) -> Result<Vec<u8>, String> {
        let not_written = |error: jpeg_encoder::EncodingError| format!("JPEG not written: {error}");
        let too_big = || {
            format!(
                "{} x {} pixels is too big for a JPEG",
                self.width, self.height
            )
        };
        let (width, height) = (
            u16::try_from(self.width).map_err(|_| too_big())?,
            u16::try_from(self.height).map_err(|_| too_big())?,
        );
        let rgb: Vec<u8> = self
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[r, g, b, _]| [r, g, b])
            .collect();
        let mut bytes = Vec::new();
        let mut encoder = jpeg_encoder::Encoder::new(&mut bytes, quality);
        encoder.set_optimized_huffman_tables(true);
        encoder
            .encode(&rgb, width, height, jpeg_encoder::ColorType::Rgb)
            .map_err(not_written)?;
        Ok(bytes)
    }

    /// Whether no pixel is see-through.
    pub(crate) fn is_opaque(&self) -> bool {
        self.rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| pixel[3] == 255)
    }

    /// The image scaled down to `width` x `height` (neither bigger than the image's own size) by
    /// averaging the pixels each new one covers.
    pub(crate) fn scaled_down(&self, width: u32, height: u32) -> Image {
        box_downscale(self, width, height)
    }
}

/// An sRGB-encoded value (0 to 1) as a linear one.
pub fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// A linear value (0 to 1) as an sRGB-encoded one.
pub fn linear_to_srgb(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

/// Undoes an unknown mapping of values to bytes, from samples of it: `outputs[i]` is the byte the
/// value `values[i]` came out as, the values increasing. For each byte, the value it stands for:
/// the mean of the samples that gave it, else interpolated between the nearest ones. `None` if the
/// bytes don't grow over at least half their range (no mapping to undo).
pub fn inverse_curve(values: &[f32; 256], outputs: &[u8; 256]) -> Option<[f32; 256]> {
    if outputs[255] < outputs[0].saturating_add(128) {
        return None;
    }
    let mut sums = [0.0f32; 256];
    let mut counts = [0u32; 256];
    for (&value, &output) in values.iter().zip(outputs) {
        sums[usize::from(output)] += value;
        counts[usize::from(output)] += 1;
    }
    let known: Vec<(usize, f32)> = (0..256)
        .filter(|&byte| counts[byte] > 0)
        .map(|byte| (byte, sums[byte] / counts[byte] as f32))
        .collect();
    let (&first, &last) = (known.first()?, known.last()?);
    Some(core::array::from_fn(|byte| {
        match known.iter().position(|&(known, _)| known >= byte) {
            None => last.1,
            Some(0) => first.1,
            Some(next) => {
                let ((low, below), (high, above)) = (known[next - 1], known[next]);
                let share = (byte - low) as f32 / (high - low) as f32;
                below + (above - below) * share
            }
        }
    }))
}

/// Reads and decodes the PNG or JPEG file at `path`.
pub fn read(path: &Path) -> Result<Image, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    decode(&bytes).map_err(|error| format!("{}: {error}", path.display()))
}

/// Decodes PNG or JPEG data, scaling it down to [`MAX_SIDE`] if it is bigger.
pub fn decode(bytes: &[u8]) -> Result<Image, String> {
    let image = if bytes.starts_with(&PNG_MAGIC) {
        decode_png(bytes)?
    } else if bytes.starts_with(&JPEG_MAGIC) {
        decode_jpeg(bytes)?
    } else {
        return Err("not a PNG or JPEG image".into());
    };

    if image.width.max(image.height) > MAX_SIDE {
        let (width, height) = fit_to_max_side(image.width, image.height);
        return Ok(box_downscale(&image, width, height));
    }
    Ok(image)
}

/// The size a PNG or JPEG file declares, as [`decode`] would give it (so scaled down to
/// [`MAX_SIDE`] if bigger), read from its header only: nothing is allocated for the pixels.
pub(crate) fn dimensions(bytes: &[u8]) -> Result<(u32, u32), String> {
    let (width, height) = if bytes.starts_with(&PNG_MAGIC) {
        let limits = png::Limits { bytes: usize::MAX };
        let decoder = png::Decoder::new_with_limits(Cursor::new(bytes), limits);
        let reader = decoder
            .read_info()
            .map_err(|error| format!("not a readable PNG: {error}"))?;
        reader.info().size()
    } else if bytes.starts_with(&JPEG_MAGIC) {
        let mut decoder = JpegDecoder::new(ZCursor::new(bytes));
        decoder
            .decode_headers()
            .map_err(|error| format!("not a readable JPEG: {error:?}"))?;
        let info = decoder.info().ok_or("not a readable JPEG: no header")?;
        (u32::from(info.width), u32::from(info.height))
    } else {
        return Err("not a PNG or JPEG image".into());
    };
    check_size(width, height)?;
    if width.max(height) > MAX_SIDE {
        return Ok(fit_to_max_side(width, height));
    }
    Ok((width, height))
}

/// Fails for a declared size nobody should be asked to decode.
fn check_size(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("the image is empty".into());
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(format!("{width} x {height} pixels is too big an image"));
    }
    Ok(())
}

/// What the samples of a decoded 8-bit pixel mean.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    Gray,
    GrayAlpha,
    Rgb,
    Rgba,
}

/// Decodes a PNG. The decoder brings palettes, low bit depths and 16-bit samples to 8-bit gray,
/// gray with alpha, RGB or RGBA; [`to_rgba`] does the rest.
fn decode_png(bytes: &[u8]) -> Result<Image, String> {
    let not_readable = |error: png::DecodingError| format!("not a readable PNG: {error}");
    // The decoder's default allocation limit (64 MiB) would refuse big but allowed pictures:
    // `check_size` is our limit.
    let limits = png::Limits { bytes: usize::MAX };
    let mut decoder = png::Decoder::new_with_limits(Cursor::new(bytes), limits);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(not_readable)?;

    let (width, height) = reader.info().size();
    check_size(width, height)?;
    let size = reader.output_buffer_size().ok_or("the image is too big")?;
    let mut pixels = vec![0; size];
    let frame = reader.next_frame(&mut pixels).map_err(not_readable)?;
    pixels.truncate(frame.buffer_size());

    let layout = match frame.color_type {
        png::ColorType::Grayscale => Layout::Gray,
        png::ColorType::GrayscaleAlpha => Layout::GrayAlpha,
        png::ColorType::Rgb => Layout::Rgb,
        png::ColorType::Rgba => Layout::Rgba,
        png::ColorType::Indexed => return Err("not a readable PNG: palette not expanded".into()),
    };
    Ok(Image {
        width,
        height,
        rgba: to_rgba(&pixels, layout),
    })
}

/// Decodes a JPEG to RGB (the decoder converts grayscale, YCbCr and CMYK for us), alpha 255.
fn decode_jpeg(bytes: &[u8]) -> Result<Image, String> {
    let not_readable =
        |error: zune_jpeg::errors::DecodeErrors| format!("not a readable JPEG: {error:?}");
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    decoder.decode_headers().map_err(not_readable)?;
    let info = decoder.info().ok_or("not a readable JPEG: no header")?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    check_size(width, height)?;

    let pixels = decoder.decode().map_err(not_readable)?;
    // The decoder may pick another output than asked for ("when it sees fit").
    let layout = match decoder.output_colorspace() {
        Some(ColorSpace::RGB) => Layout::Rgb,
        Some(ColorSpace::RGBA) => Layout::Rgba,
        Some(ColorSpace::Luma) => Layout::Gray,
        Some(ColorSpace::LumaA) => Layout::GrayAlpha,
        other => return Err(format!("JPEG colours not supported: {other:?}")),
    };
    Ok(Image {
        width,
        height,
        rgba: to_rgba(&pixels, layout),
    })
}

/// 8-bit samples in `layout` as RGBA8 (gray is repeated into R, G and B; missing alpha is 255).
fn to_rgba(pixels: &[u8], layout: Layout) -> Vec<u8> {
    match layout {
        Layout::Rgba => pixels.to_vec(),
        Layout::Rgb => pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[r, g, b]| [r, g, b, 255])
            .collect(),
        Layout::Gray => pixels.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        Layout::GrayAlpha => pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[v, a]| [v, v, v, a])
            .collect(),
    }
}

/// The size with the same aspect ratio whose longer side is [`MAX_SIDE`] (the shorter side is at
/// least one pixel).
fn fit_to_max_side(width: u32, height: u32) -> (u32, u32) {
    let longest = u64::from(width.max(height));
    let scale = |side: u32| ((u64::from(side) * u64::from(MAX_SIDE) / longest) as u32).max(1);
    (scale(width), scale(height))
}

/// `image` scaled down to `width` x `height` (neither bigger than the image's own size) by
/// averaging the block of source pixels each new pixel covers.
fn box_downscale(image: &Image, width: u32, height: u32) -> Image {
    let (src_width, src_height) = (image.width as usize, image.height as usize);
    let (width, height) = (width as usize, height as usize);
    // The source range along one axis that new pixel `index` covers: never empty.
    let span = |index: usize, new_len: usize, src_len: usize| {
        let start = index * src_len / new_len;
        let end = ((index + 1) * src_len / new_len).max(start + 1);
        start..end
    };

    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        let rows = span(y, height, src_height);
        for x in 0..width {
            let columns = span(x, width, src_width);
            let mut sums = [0u32; 4];
            for row in rows.clone() {
                for column in columns.clone() {
                    let at = (row * src_width + column) * 4;
                    for (sum, &value) in sums.iter_mut().zip(&image.rgba[at..at + 4]) {
                        *sum += u32::from(value);
                    }
                }
            }
            let count = (rows.len() * columns.len()) as u32;
            // Rounded to the nearest value.
            rgba.extend(sums.map(|sum| ((sum + count / 2) / count) as u8));
        }
    }
    Image {
        width: width as u32,
        height: height as u32,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_are_undone() {
        let values: [f32; 256] = core::array::from_fn(|i| srgb_to_linear(i as f32 / 255.0));
        // Stored as sRGB: each byte is its own value back.
        let encoded: [u8; 256] = core::array::from_fn(|i| i as u8);
        let curve = inverse_curve(&values, &encoded).unwrap();
        assert!((0..256).all(|byte| (curve[byte] - values[byte]).abs() < 1e-6));
        // Stored linearly: dark values share bytes, and the byte for half is about half.
        let linear: [u8; 256] = values.map(|value| (value * 255.0).round() as u8);
        let curve = inverse_curve(&values, &linear).unwrap();
        assert!((curve[128] - 128.0 / 255.0).abs() < 0.01, "{}", curve[128]);
        assert_eq!(curve[255], 1.0);
        assert!(curve[0] < 0.002);
        // Nothing came through.
        assert!(inverse_curve(&values, &[200; 256]).is_none());
    }

    #[test]
    fn average_color_is_linear_and_weighted_by_alpha() {
        // Black and sRGB 188 (about linear 0.5), opaque; a transparent red that doesn't count.
        let image = Image {
            width: 3,
            height: 1,
            rgba: vec![0, 0, 0, 255, 188, 188, 188, 255, 255, 0, 0, 0],
        };
        let [r, g, b, a] = image.average_color();
        assert!((r - 0.25).abs() < 0.01, "{r}");
        assert_eq!((r, r), (g, b));
        assert!((a - 2.0 / 3.0).abs() < 1e-6);
        assert_eq!(Image::default().average_color(), [1.0, 1.0, 1.0, 0.0]);
    }

    #[test]
    fn tinting_multiplies_in_linear_space() {
        let image = Image {
            width: 3,
            height: 1,
            rgba: vec![200, 100, 50, 77, 255, 255, 255, 255, 10, 20, 30, 0],
        };
        // White changes nothing.
        assert_eq!(image.tinted([1.0; 3]), image);
        // Black blanks the colors, not the alpha.
        let black = image.tinted([0.0; 3]);
        assert_eq!(black.rgba[0..4], [0, 0, 0, 77]);
        assert_eq!(black.rgba[8..12], [0, 0, 0, 0]);
        // Half of linear white is sRGB 188.
        let half = image.tinted([0.5; 3]);
        assert_eq!(half.rgba[4..8], [188, 188, 188, 255]);
        // A channel at a time; more than white doesn't wrap around.
        let red = image.tinted([1.0, 0.0, 2.0]);
        assert_eq!(red.rgba[4..8], [255, 0, 255, 255]);
    }

    /// A PNG of `size` pixels in the given colour type and depth, `data` being the packed samples.
    fn png_of(
        color: png::ColorType,
        depth: png::BitDepth,
        (width, height): (u32, u32),
        data: &[u8],
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(color);
        encoder.set_depth(depth);
        encoder
            .write_header()
            .expect("PNG header")
            .write_image_data(data)
            .expect("PNG data");
        bytes
    }

    /// A 3x2 PNG: the first pixel red, the last one half-transparent blue, the rest black.
    fn tiny_png() -> Vec<u8> {
        let mut pixels = [0, 0, 0, 255].repeat(6);
        pixels[0..4].copy_from_slice(&[255, 0, 0, 255]);
        pixels[20..24].copy_from_slice(&[0, 0, 255, 128]);
        png_of(png::ColorType::Rgba, png::BitDepth::Eight, (3, 2), &pixels)
    }

    /// Decodes a hex string (the JPEG fixtures below).
    fn from_hex(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).expect("hex digits"))
            .collect()
    }

    /// An 8x8 grayscale JPEG, all 128.
    fn gray_jpeg() -> Vec<u8> {
        from_hex(concat!(
            "ffd8ffe000104a46494600010200000100010000ffc0000b080008000801011100ffdb00430001010101010101010101",
            "010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101",
            "010101010101ffc4001f0000010501010101010100000000000000000102030405060708090a0bffc400b51000020103",
            "03020403050504040000017d01020300041105122131410613516107227114328191a1082342b1c11552d1f024336272",
            "82090a161718191a25262728292a3435363738393a434445464748494a535455565758595a636465666768696a737475",
            "767778797a838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9",
            "cad2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7e8e9eaf1f2f3f4f5f6f7f8f9faffda0008010100003f002bffd9",
        ))
    }

    /// An 8x8 JPEG (YCbCr), all red.
    fn red_jpeg() -> Vec<u8> {
        from_hex(concat!(
            "ffd8ffe000104a46494600010200000100010000ffc00011080008000803011100021101031101ffdb00430001010101",
            "010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101",
            "010101010101010101010101ffdb00430101010101010101010101010101010101010101010101010101010101010101",
            "010101010101010101010101010101010101010101010101010101010101010101ffc4001f0000010501010101010100",
            "000000000000000102030405060708090a0bffc400b5100002010303020403050504040000017d010203000411051221",
            "31410613516107227114328191a1082342b1c11552d1f02433627282090a161718191a25262728292a3435363738393a",
            "434445464748494a535455565758595a636465666768696a737475767778797a838485868788898a9293949596979899",
            "9aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4c5c6c7c8c9cad2d3d4d5d6d7d8d9dae1e2e3e4e5e6e7e8e9eaf1",
            "f2f3f4f5f6f7f8f9faffc4001f0100030101010101010101010000000000000102030405060708090a0bffc400b51100",
            "020102040403040705040400010277000102031104052131061241510761711322328108144291a1b1c109233352f015",
            "6272d10a162434e125f11718191a262728292a35363738393a434445464748494a535455565758595a63646566676869",
            "6a737475767778797a82838485868788898a92939495969798999aa2a3a4a5a6a7a8a9aab2b3b4b5b6b7b8b9bac2c3c4",
            "c5c6c7c8c9cad2d3d4d5d6d7d8d9dae2e3e4e5e6e7e8e9eaf2f3f4f5f6f7f8f9faffda000c03010002110311003f00fc",
            "5faff29cff00bf83ffd9",
        ))
    }

    #[test]
    fn decodes_a_png() {
        let image = decode(&tiny_png()).unwrap();
        assert_eq!((image.width, image.height), (3, 2));
        assert_eq!(image.rgba.len(), 3 * 2 * 4);
        assert_eq!(image.rgba[0..4], [255, 0, 0, 255]);
        // Row 1, column 2.
        assert_eq!(image.rgba[(3 + 2) * 4..(3 + 2) * 4 + 4], [0, 0, 255, 128]);
        assert_eq!(image.aspect(), 1.5);
    }

    #[test]
    fn png_colour_types_all_become_rgba() {
        use png::{BitDepth, ColorType};
        // Gray.
        let gray = png_of(ColorType::Grayscale, BitDepth::Eight, (2, 1), &[10, 200]);
        assert_eq!(
            decode(&gray).unwrap().rgba,
            [10, 10, 10, 255, 200, 200, 200, 255]
        );
        // Gray with alpha.
        let gray_alpha = png_of(ColorType::GrayscaleAlpha, BitDepth::Eight, (1, 1), &[9, 77]);
        assert_eq!(decode(&gray_alpha).unwrap().rgba, [9, 9, 9, 77]);
        // RGB.
        let rgb = png_of(ColorType::Rgb, BitDepth::Eight, (1, 1), &[1, 2, 3]);
        assert_eq!(decode(&rgb).unwrap().rgba, [1, 2, 3, 255]);
        // 16 bit RGB: the high bytes are kept.
        let deep = png_of(
            ColorType::Rgb,
            BitDepth::Sixteen,
            (1, 1),
            &[255, 1, 128, 2, 0, 3],
        );
        assert_eq!(decode(&deep).unwrap().rgba, [255, 128, 0, 255]);
        // 1 bit gray, two pixels in the top bits of one byte: a set bit is white.
        let one_bit = png_of(ColorType::Grayscale, BitDepth::One, (2, 1), &[0b1000_0000]);
        assert_eq!(
            decode(&one_bit).unwrap().rgba,
            [255, 255, 255, 255, 0, 0, 0, 255]
        );
    }

    #[test]
    fn png_palette_is_expanded() {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_palette(vec![255, 0, 0, 0, 255, 0]);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[1, 0])
            .unwrap();
        assert_eq!(
            decode(&bytes).unwrap().rgba,
            [0, 255, 0, 255, 255, 0, 0, 255]
        );
    }

    #[test]
    fn decodes_jpegs() {
        let gray = decode(&gray_jpeg()).unwrap();
        assert_eq!((gray.width, gray.height), (8, 8));
        for pixel in gray.rgba.as_chunks::<4>().0 {
            assert!(pixel[0].abs_diff(128) <= 2, "{pixel:?}");
            assert!(pixel[0] == pixel[1] && pixel[1] == pixel[2] && pixel[3] == 255);
        }

        let red = decode(&red_jpeg()).unwrap();
        assert_eq!((red.width, red.height), (8, 8));
        let pixel = &red.rgba[0..4];
        assert!(
            pixel[0] > 240 && pixel[1] < 15 && pixel[2] < 15,
            "{pixel:?}"
        );
        assert_eq!(pixel[3], 255);
    }

    #[test]
    fn png_round_trips_through_to_png() {
        let image = decode(&tiny_png()).unwrap();
        assert_eq!(decode(&image.to_png().unwrap()).unwrap(), image);
        // Pixels that don't fit the size are an error, not a panic.
        let broken = Image {
            width: 4,
            height: 4,
            rgba: vec![0; 3],
        };
        assert!(broken.to_png().is_err());
    }

    #[test]
    fn garbage_is_an_error() {
        assert_eq!(
            decode(b"definitely not an image"),
            Err("not a PNG or JPEG image".to_string())
        );
        assert!(decode(&[]).is_err());
        // Right magic, nothing behind it.
        assert!(decode(&PNG_MAGIC).is_err());
        assert!(decode(&JPEG_MAGIC).is_err());
        // Truncated real files.
        let png = tiny_png();
        assert!(decode(&png[..png.len() / 2]).is_err());
        let jpeg = red_jpeg();
        assert!(decode(&jpeg[..jpeg.len() / 2]).is_err());
    }

    #[test]
    fn empty_image_has_aspect_one() {
        assert_eq!(Image::default().aspect(), 1.0);
    }

    #[test]
    fn huge_declared_sizes_are_refused() {
        assert!(check_size(16384, 16384).is_ok());
        assert!(check_size(16385, 16384).is_err());
        assert!(check_size(u32::MAX, u32::MAX).is_err());
        assert!(check_size(0, 5).is_err());
    }

    #[test]
    fn big_sides_scale_down_keeping_the_aspect() {
        assert_eq!(fit_to_max_side(16384, 8192), (8192, 4096));
        assert_eq!(fit_to_max_side(9000, 9000), (8192, 8192));
        assert_eq!(fit_to_max_side(100_000, 1), (8192, 1));
    }

    #[test]
    fn box_downscale_averages_blocks() {
        // 4x2: the left half black, the right half white, but for one faint pixel.
        let black = [0, 0, 0, 255];
        let white = [255, 255, 255, 255];
        let faint = [255, 255, 255, 1];
        let image = Image {
            width: 4,
            height: 2,
            rgba: [black, black, faint, white, black, black, white, white].concat(),
        };

        let half = box_downscale(&image, 2, 1);
        assert_eq!((half.width, half.height), (2, 1));
        assert_eq!(half.rgba[0..4], [0, 0, 0, 255]);
        // The right block: colour 4 x 255 / 4, alpha (1 + 255 + 255 + 255) / 4 = 191.5.
        assert_eq!(half.rgba[4..8], [255, 255, 255, 192]);

        // The same size changes nothing.
        assert_eq!(box_downscale(&image, 4, 2), image);
        // A side can shrink to one pixel.
        assert_eq!(box_downscale(&image, 1, 1).rgba.len(), 4);
    }
}
