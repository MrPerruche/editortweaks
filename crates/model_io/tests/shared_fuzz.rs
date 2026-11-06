//! Fuzzing the receiver's side of `model_io::shared`: whatever bytes arrive, `check` and
//! `read_model` answer and never panic (the game aborts on a panic).
//!
//! From valid cooked models and images, a seeded generator damages the bytes: cuts, flips,
//! inserted and removed bytes, boundary numbers where counts are. A model is damaged twice, as it
//! arrives and inside its zlib stream (unpacked, damaged, packed again, so that the damage gets
//! past zlib to the reader behind it). PNG and JPEG files are damaged and checked as images.
//!
//! `SHARED_FUZZ_SEEDS` says how many seeds (default: a few seconds). A panic is caught, its input
//! is kept in a file in the temporary folder, and the test fails at the end, naming them.

use std::panic::{AssertUnwindSafe, catch_unwind};

use miniz_oxide::deflate::compress_to_vec_zlib;
use miniz_oxide::inflate::decompress_to_vec_zlib;
use model_io::shared::{self, Aim, Checked, Limits};
use model_io::{Image, Material, Model, Part};

const DEFAULT_SEEDS: u64 = 2000;

/// A small xorshift generator: the same seed, the same inputs.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        for _ in 0..4 {
            rng.next();
        }
        rng
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A number below `limit` (0 for a limit of 0).
    fn below(&mut self, limit: usize) -> usize {
        if limit == 0 {
            return 0;
        }
        (self.next() % limit as u64) as usize
    }
}

/// Damages `bytes` in one random way.
fn damage(rng: &mut Rng, bytes: &mut Vec<u8>) {
    let at = rng.below(bytes.len());
    match rng.below(9) {
        0 => bytes.truncate(at),
        1 => {
            if let Some(byte) = bytes.get_mut(at) {
                *byte ^= 1 << rng.below(8);
            }
        }
        2 => {
            if let Some(byte) = bytes.get_mut(at) {
                *byte = rng.next() as u8;
            }
        }
        3 => {
            let count = 1 + rng.below(8);
            let new: Vec<u8> = (0..count).map(|_| rng.next() as u8).collect();
            bytes.splice(at..at, new);
        }
        4 => {
            let end = (at + 1 + rng.below(16)).min(bytes.len());
            bytes.drain(at..end);
        }
        5 => {
            let end = (at + 1 + rng.below(16)).min(bytes.len());
            let fill = [0x00, 0xff, 0x7f, 0x80][rng.below(4)];
            bytes[at..end].fill(fill);
        }
        6 => {
            let end = (at + 1 + rng.below(32)).min(bytes.len());
            let copy = bytes[at..end].to_vec();
            let to = rng.below(bytes.len());
            bytes.splice(to..to, copy);
        }
        7 => {
            // A number that makes a count or a size hard on a reader.
            let number = [
                0u32,
                1,
                2,
                0xff,
                0x100,
                0xffff,
                0x1_0000,
                0x7fff_ffff,
                0x8000_0000,
                u32::MAX,
            ][rng.below(10)];
            if at + 4 <= bytes.len() {
                bytes[at..at + 4].copy_from_slice(&number.to_le_bytes());
            }
        }
        _ => {
            // The same value as the one before: a stuck byte.
            if at > 0 {
                bytes[at] = bytes[at - 1];
            }
        }
    }
}

fn damaged_a_few_times(rng: &mut Rng, bytes: &[u8]) -> Vec<u8> {
    let mut bytes = bytes.to_vec();
    for _ in 0..1 + rng.below(3) {
        damage(rng, &mut bytes);
    }
    bytes
}

fn gradient(width: u32, height: u32, alpha: bool) -> Image {
    let mut rgba = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let a = if alpha { (x * 255 / width) as u8 } else { 255 };
            rgba.extend([
                (x * 255 / width) as u8,
                (y * 255 / height) as u8,
                (x ^ y) as u8,
                a,
            ]);
        }
    }
    Image {
        width,
        height,
        rgba,
    }
}

fn jpeg_of(image: &Image) -> Vec<u8> {
    let rgb: Vec<u8> = image
        .rgba
        .chunks(4)
        .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
        .collect();
    let mut bytes = Vec::new();
    jpeg_encoder::Encoder::new(&mut bytes, 80)
        .encode(
            &rgb,
            image.width as u16,
            image.height as u16,
            jpeg_encoder::ColorType::Rgb,
        )
        .unwrap();
    bytes
}

fn grid_part(n: usize, material: Option<usize>) -> Part {
    let mut part = Part {
        material,
        ..Part::default()
    };
    for y in 0..=n {
        for x in 0..=n {
            part.positions
                .push([x as f32, y as f32, ((x * y) % 5) as f32 * 0.25]);
            part.uvs.push([x as f32 / n as f32, y as f32 / n as f32]);
        }
    }
    let width = (n + 1) as u32;
    for y in 0..n as u32 {
        for x in 0..n as u32 {
            let a = y * width + x;
            part.corners
                .extend([[a, a + 1, a + width + 1], [a, a + width + 1, a + width]]);
        }
    }
    part
}

/// What the receiver allows in these tests: small, so that a lie costs little.
fn limits() -> Limits {
    Limits {
        triangles: 100_000,
        longest_side: 4096,
        pixels: 1 << 22,
    }
}

fn roomy() -> Aim {
    Aim {
        triangles: 1 << 30,
        longest_side: 4096,
        pixels: 1 << 30,
        bytes: 1 << 30,
    }
}

/// A cooked model with two parts, a JPEG and a PNG texture, and its declared counts.
fn model_sample() -> (Vec<u8>, u32, u32) {
    let material = |texture| Material {
        name: String::new(),
        color: [1.0, 0.5, 0.25, 1.0],
        texture,
    };
    let model = Model {
        parts: vec![
            grid_part(7, Some(0)),
            grid_part(4, Some(1)),
            grid_part(2, None),
        ],
        materials: vec![
            material(Some(gradient(24, 16, false))),
            material(Some(gradient(16, 16, true))),
        ],
    };
    let cooked = shared::cook_model(&model, &roomy()).unwrap();
    (cooked.bytes, cooked.triangles, cooked.pixels)
}

/// A cooked image of `file` (a PNG or JPEG), and its declared pixels.
fn image_sample(file: &[u8]) -> (Vec<u8>, u32) {
    let cooked = shared::cook_image(file, &roomy()).unwrap();
    (cooked.bytes, cooked.pixels)
}

/// What a check made of `bytes`: whether it passed. Retries an image with the pixels it really
/// has, so that damage past the size is found out too.
fn run(bytes: &[u8], triangles: u32, pixels: u32) -> bool {
    let read = shared::read_model(bytes).is_ok();
    match shared::check(bytes, &limits(), triangles, pixels) {
        Ok(Checked::Image { file, .. }) => {
            assert!(file.starts_with(&[0xFF, 0xD8]) || file.starts_with(&[0x89, b'P']));
            true
        }
        Ok(Checked::Model) => {
            assert!(read, "check passed a model that read_model refuses");
            true
        }
        Err(message) => {
            let real = message
                .split_once("pixels and has ")
                .and_then(|(_, rest)| rest.trim().parse::<u32>().ok());
            if let Some(real) = real {
                return shared::check(bytes, &limits(), triangles, real).is_ok();
            }
            false
        }
    }
}

fn keep_failure(seed: u64, bytes: &[u8]) -> String {
    let path = std::env::temp_dir().join(format!("shared_fuzz_panic_{seed}.bin"));
    let _ = std::fs::write(&path, bytes);
    format!("seed {seed}: {}", path.display())
}

#[test]
fn damaged_input_never_panics() {
    let seeds: u64 = std::env::var("SHARED_FUZZ_SEEDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_SEEDS);

    let (model, model_triangles, model_pixels) = model_sample();
    let model_body = decompress_to_vec_zlib(&model[10..]).unwrap();
    let png = gradient(20, 12, true).to_png().unwrap();
    let opaque_png = gradient(20, 12, false).to_png().unwrap();
    let jpeg = jpeg_of(&gradient(24, 16, false));
    let samples = [
        image_sample(&png),
        image_sample(&opaque_png),
        image_sample(&jpeg),
    ];
    // Every sample passes as it is.
    assert!(run(&model, model_triangles, model_pixels));
    for (bytes, pixels) in &samples {
        assert!(run(bytes, 0, *pixels));
    }

    let (mut passed, mut failures) = (0u64, Vec::new());
    for seed in 0..seeds {
        let mut rng = Rng::new(seed);
        let (bytes, triangles, pixels) = match rng.below(4) {
            0 => (
                damaged_a_few_times(&mut rng, &model),
                model_triangles,
                model_pixels,
            ),
            1 => {
                // Inside the zlib stream: damaged, packed again, with the length it really has or
                // one that is wrong.
                let body = damaged_a_few_times(&mut rng, &model_body);
                let length = if rng.below(8) == 0 {
                    rng.next() as u32
                } else {
                    body.len() as u32
                };
                let mut bytes = model[..6].to_vec();
                bytes.extend(length.to_le_bytes());
                bytes.extend(compress_to_vec_zlib(&body, 6));
                if rng.below(4) == 0 {
                    damage(&mut rng, &mut bytes);
                }
                (bytes, model_triangles, model_pixels)
            }
            2 => {
                // A PNG or JPEG file of its own, damaged, behind the header of an image.
                let (file, pixels) = match rng.below(3) {
                    0 => (&png, samples[0].1),
                    1 => (&opaque_png, samples[1].1),
                    _ => (&jpeg, samples[2].1),
                };
                let mut bytes = model[..4].to_vec();
                bytes.extend([1, 0]);
                bytes.extend(damaged_a_few_times(&mut rng, file));
                (bytes, 0, pixels)
            }
            _ => {
                let (bytes, pixels) = &samples[rng.below(3)];
                (damaged_a_few_times(&mut rng, bytes), 0, *pixels)
            }
        };
        match catch_unwind(AssertUnwindSafe(|| run(&bytes, triangles, pixels))) {
            Ok(true) => passed += 1,
            Ok(false) => {}
            Err(_) => failures.push(keep_failure(seed, &bytes)),
        }
    }
    println!("{seeds} damaged inputs, {passed} still passed the checks");
    assert!(failures.is_empty(), "panics:\n{}", failures.join("\n"));
}
