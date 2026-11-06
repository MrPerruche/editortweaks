use std::collections::BTreeSet;

use miniz_oxide::deflate::compress_to_vec_zlib;
use miniz_oxide::inflate::decompress_to_vec_zlib;

use super::format::{self, Body, EncodedMaterial};
use super::*;
use crate::image::Image;
use crate::model::{Material, Part};
use crate::readers::{self, Format};

/// Numbers that look random, from a seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// A grid of `n` x `n` quads (2 n^2 triangles) with a bumpy surface and UVs. Connected, or with
/// every quad's four vertices its own (`faceted`).
fn grid_part(n: usize, faceted: bool, material: Option<usize>) -> Part {
    let height = |x: usize, y: usize| ((x as f32 * 0.3).sin() + (y as f32 * 0.2).cos()) * 0.5;
    let mut part = Part {
        material,
        ..Part::default()
    };
    let vertex = |part: &mut Part, x: usize, y: usize| {
        part.positions.push([x as f32, y as f32, height(x, y)]);
        part.uvs.push([x as f32 / n as f32, y as f32 / n as f32]);
    };
    if faceted {
        for y in 0..n {
            for x in 0..n {
                let base = part.positions.len() as u32;
                for (dx, dy) in [(0, 0), (1, 0), (1, 1), (0, 1)] {
                    vertex(&mut part, x + dx, y + dy);
                }
                part.corners
                    .extend([[base, base + 1, base + 2], [base, base + 2, base + 3]]);
            }
        }
    } else {
        for y in 0..=n {
            for x in 0..=n {
                vertex(&mut part, x, y);
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
    }
    part
}

/// A picture: smooth if it can be compressed well, else noise. Transparent for `alpha`.
fn picture(width: u32, height: u32, alpha: bool, noisy: bool) -> Image {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut rgba = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let [r, g, b] = if noisy {
                let random = rng.next();
                [random as u8, (random >> 8) as u8, (random >> 16) as u8]
            } else {
                [
                    (x * 255 / width) as u8,
                    (y * 255 / height) as u8,
                    ((x + y) % 256) as u8,
                ]
            };
            let a = if alpha { (x * 255 / width) as u8 } else { 255 };
            rgba.extend([r, g, b, a]);
        }
    }
    Image {
        width,
        height,
        rgba,
    }
}

fn material(texture: Option<Image>) -> Material {
    Material {
        name: String::new(),
        color: [0.5, 0.25, 1.0, 1.0],
        texture,
    }
}

fn model_of(parts: Vec<Part>, materials: Vec<Material>) -> Model {
    Model { parts, materials }
}

/// An aim nothing exceeds.
fn roomy() -> Aim {
    Aim {
        triangles: 1 << 40,
        longest_side: 8192,
        pixels: 1 << 40,
        bytes: 1 << 40,
    }
}

fn limits_of(aim: &Aim) -> Limits {
    Limits {
        triangles: aim.triangles.min(MAX_TRIANGLES as u64),
        longest_side: aim.longest_side,
        pixels: aim.pixels.min(1 << 32),
    }
}

fn roomy_limits() -> Limits {
    limits_of(&roomy())
}

/// What `check` is given for a cooked file.
fn checked(cooked: &Cooked, limits: &Limits) -> Result<Checked, String> {
    check(&cooked.bytes, limits, cooked.triangles, cooked.pixels)
}

fn bounds_of(model: &Model) -> ([f32; 3], [f32; 3]) {
    let bounds = model.bounds().unwrap();
    (bounds.min, bounds.max)
}

/// The triangles of a model by the grid squares of their corners (the x and y of grid positions
/// are whole numbers), whatever the order.
fn grid_triangles(model: &Model) -> BTreeSet<[(i32, i32); 3]> {
    model
        .parts
        .iter()
        .flat_map(|part| {
            part.corners.iter().map(|triangle| {
                let mut key = triangle.map(|index| {
                    let [x, y, _] = part.positions[index as usize];
                    (x.round() as i32, y.round() as i32)
                });
                key.sort();
                key
            })
        })
        .collect()
}

fn png_of(image: &Image) -> Vec<u8> {
    image.to_png().unwrap()
}

// ---- The format ----

#[test]
fn a_model_survives_writing_and_reading() {
    let texture = png_of(&picture(8, 4, true, false));
    let first = Part {
        material: Some(1),
        positions: vec![
            [-3.0, 10.0, 0.5],
            [4.0, 20.0, 1.5],
            [0.0, 15.0, -2.0],
            [1.0, 12.0, 7.0],
        ],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [-0.5, 2.5], [0.25, 0.75]],
        corners: vec![[0, 1, 2], [3, 2, 1], [3, 0, 1]],
        ..Part::default()
    };
    let second = Part {
        material: None,
        positions: vec![[2.0, 11.0, 3.0], [3.0, 19.0, 0.0], [-1.0, 13.0, 2.0]],
        corners: vec![[2, 1, 0]],
        ..Part::default()
    };
    let body = Body {
        materials: vec![
            EncodedMaterial {
                color: [1.0, 0.5, 0.25, 0.75],
                texture: None,
            },
            EncodedMaterial {
                color: [0.0, 1.0, 2.0, 1.0],
                texture: Some(texture.clone()),
            },
        ],
        parts: vec![first.clone(), second.clone()],
    };
    let file = format::write(&body).unwrap();
    let model = read_model(&file).unwrap();

    assert_eq!(model.materials.len(), 2);
    assert_eq!(model.materials[0].color, [1.0, 0.5, 0.25, 0.75]);
    assert!(model.materials[0].texture.is_none());
    assert_eq!(model.materials[1].color, [0.0, 1.0, 2.0, 1.0]);
    let decoded = model.materials[1].texture.as_ref().unwrap();
    assert_eq!(*decoded, image::decode(&texture).unwrap());

    // Bounds over both parts: x -3 to 4, y 10 to 20, z -2 to 7.
    let steps = [7.0 / 65535.0, 10.0 / 65535.0, 9.0 / 65535.0];
    assert_eq!(model.parts.len(), 2);
    for (read, wrote) in model.parts.iter().zip([&first, &second]) {
        assert_eq!(read.material, wrote.material);
        assert_eq!(read.corners, wrote.corners);
        assert_eq!(read.uvs, wrote.uvs);
        for (a, b) in read.positions.iter().zip(&wrote.positions) {
            for axis in 0..3 {
                assert!((a[axis] - b[axis]).abs() <= steps[axis], "{a:?} {b:?}");
            }
        }
    }
    // The most extreme positions come back exactly.
    assert_eq!(model.parts[0].positions[1][0], 4.0);
    assert_eq!(model.parts[0].positions[0][0], -3.0);
}

#[test]
fn an_empty_model_is_a_model() {
    let cooked = cook_model(&Model::default(), &roomy()).unwrap();
    assert_eq!(
        (cooked.triangles, cooked.pixels, cooked.simplified),
        (0, 0, false)
    );
    assert!(read_model(&cooked.bytes).unwrap().parts.is_empty());
    assert!(matches!(
        checked(&cooked, &roomy_limits()),
        Ok(Checked::Model)
    ));
}

#[test]
fn content_is_the_hash_of_the_bytes() {
    let cooked = cook_model(&Model::default(), &roomy()).unwrap();
    assert_eq!(cooked.content(), content_hash(&cooked.bytes));
    assert_eq!(content_hash(b"a"), 0xaf63_dc4c_8601_ec8c);
}

// ---- Cooking models ----

#[test]
fn a_model_within_the_aim_is_not_simplified() {
    let model = model_of(vec![grid_part(20, false, None)], vec![]);
    let cooked = cook_model(&model, &roomy()).unwrap();
    assert!(!cooked.simplified);
    assert_eq!(cooked.triangles, 800);
    let read = read_model(&cooked.bytes).unwrap();
    // The same triangles, over the same vertices, only their order is another.
    assert_eq!(
        read.parts[0].positions.len(),
        model.parts[0].positions.len()
    );
    assert_eq!(grid_triangles(&read), grid_triangles(&model));
    // The UVs are dropped for want of a picture to show.
    assert!(read.parts[0].uvs.is_empty());
}

#[test]
fn a_big_connected_mesh_is_simplified_to_the_aim() {
    let model = model_of(vec![grid_part(150, false, None)], vec![]);
    assert_eq!(model.triangle_count(), 45_000);
    let aim = Aim {
        triangles: 5_000,
        ..roomy()
    };
    let cooked = cook_model(&model, &aim).unwrap();
    assert!(cooked.simplified);
    assert!(cooked.triangles <= 5_000, "{}", cooked.triangles);
    assert!(cooked.triangles >= 4_000, "{}", cooked.triangles);
    let read = read_model(&cooked.bytes).unwrap();
    assert_eq!(read.triangle_count(), cooked.triangles as usize);
    assert!(matches!(
        checked(&cooked, &limits_of(&aim)),
        Ok(Checked::Model)
    ));
    // The shape is the same: the box about as big.
    let (min, max) = bounds_of(&read);
    for axis in 0..2 {
        assert!(min[axis] <= 2.0 && max[axis] >= 148.0, "{min:?} {max:?}");
    }
}

#[test]
fn a_big_faceted_mesh_is_simplified_too() {
    // Every triangle has vertices of its own: it has to be welded first to shrink.
    let model = model_of(vec![grid_part(150, true, None)], vec![]);
    let aim = Aim {
        triangles: 5_000,
        ..roomy()
    };
    let cooked = cook_model(&model, &aim).unwrap();
    assert!(cooked.simplified);
    assert!(cooked.triangles <= 5_000, "{}", cooked.triangles);
    assert!(cooked.triangles >= 2_500, "{}", cooked.triangles);
    let read = read_model(&cooked.bytes).unwrap();
    let (min, max) = bounds_of(&read);
    for axis in 0..2 {
        assert!(min[axis] <= 3.0 && max[axis] >= 147.0, "{min:?} {max:?}");
    }
}

#[test]
fn a_cube_with_vertices_of_its_own_keeps_hard_edges() {
    // 12 triangles over 24 vertices: welded for simplifying, its edges split again afterwards.
    let corners = [[0, 1, 2], [0, 2, 3]];
    let mut part = Part::default();
    for (axis, sign) in [(0, 0.0), (0, 1.0), (1, 0.0), (1, 1.0), (2, 0.0), (2, 1.0)] {
        let base = part.positions.len() as u32;
        for (u, v) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            let mut position = [0.0; 3];
            position[axis] = sign;
            position[(axis + 1) % 3] = u;
            position[(axis + 2) % 3] = v;
            part.positions.push(position);
        }
        part.corners.extend(corners.map(|t| t.map(|i| i + base)));
    }
    let big = model_of(vec![part], vec![]);
    let aim = Aim {
        triangles: 8,
        ..roomy()
    };
    let cooked = cook_model(&big, &aim).unwrap();
    assert!(cooked.triangles <= 8);
    let read = read_model(&cooked.bytes).unwrap();
    // Shared vertices only between triangles that meet gently: a corner of a cube isn't one.
    let part = &read.parts[0];
    assert!(
        part.positions.len() > 8,
        "{} positions",
        part.positions.len()
    );
}

#[test]
fn textured_parts_of_several_materials_fit_together() {
    let model = model_of(
        vec![
            grid_part(60, false, Some(0)),
            grid_part(30, true, Some(1)),
            grid_part(10, false, Some(2)),
            grid_part(5, false, None),
        ],
        vec![
            material(Some(picture(512, 256, false, false))),
            material(Some(picture(256, 256, true, false))),
            material(None),
        ],
    );
    let total = model.triangle_count();
    let aim = Aim {
        triangles: total as u64 / 3,
        longest_side: 128,
        pixels: 128 * 128,
        bytes: 1 << 30,
    };
    let cooked = cook_model(&model, &aim).unwrap();
    assert!(cooked.simplified);
    assert!(cooked.triangles as u64 <= aim.triangles);
    assert!(cooked.triangles as u64 > aim.triangles * 2 / 3);
    assert!(u64::from(cooked.pixels) <= aim.pixels);

    let read = read_model(&cooked.bytes).unwrap();
    assert_eq!(read.parts.len(), 4);
    assert_eq!(model_pixels(&read), u64::from(cooked.pixels));
    for texture in read.materials.iter().filter_map(|m| m.texture.as_ref()) {
        assert!(texture.width.max(texture.height) <= 128);
    }
    // The pictures kept their shape (2:1 and 1:1), the materials their colour and order.
    let first = read.materials[0].texture.as_ref().unwrap();
    assert_eq!(first.width, first.height * 2);
    let second = read.materials[1].texture.as_ref().unwrap();
    assert_eq!(second.width, second.height);
    assert!(read.materials[2].texture.is_none());
    assert_eq!(read.materials[0].color, [0.5, 0.25, 1.0, 1.0]);
    assert_eq!(
        read.parts.iter().map(|p| p.material).collect::<Vec<_>>(),
        [Some(0), Some(1), Some(2), None]
    );
    // UVs stay where a picture shows.
    assert_eq!(read.parts[0].uvs.len(), read.parts[0].positions.len());
    assert!(read.parts[2].uvs.is_empty());
    assert!(matches!(
        checked(&cooked, &limits_of(&aim)),
        Ok(Checked::Model)
    ));
}

#[test]
fn pixels_are_dropped_when_there_is_no_room_for_one_each() {
    let model = model_of(
        vec![grid_part(4, false, Some(0))],
        vec![material(Some(picture(16, 16, false, false)))],
    );
    let aim = Aim {
        pixels: 0,
        ..roomy()
    };
    let cooked = cook_model(&model, &aim).unwrap();
    assert!(cooked.simplified);
    assert_eq!(cooked.pixels, 0);
    let read = read_model(&cooked.bytes).unwrap();
    assert!(read.materials[0].texture.is_none());
    assert!(read.parts[0].uvs.is_empty());
}

#[test]
fn the_bytes_of_a_model_are_met() {
    let model = model_of(
        vec![grid_part(100, false, Some(0))],
        vec![material(Some(picture(400, 400, false, true)))],
    );
    let free = cook_model(&model, &roomy()).unwrap();
    assert!(!free.simplified || free.triangles < 20_000);
    let aim = Aim {
        bytes: free.bytes.len() as u64 / 4,
        ..roomy()
    };
    let cooked = cook_model(&model, &aim).unwrap();
    assert!(
        cooked.bytes.len() as u64 <= aim.bytes,
        "{} > {}",
        cooked.bytes.len(),
        aim.bytes
    );
    assert!(cooked.simplified);
    assert!(read_model(&cooked.bytes).is_ok());
    // Nothing is small enough for a few bytes.
    let tiny = Aim {
        bytes: 40,
        ..roomy()
    };
    assert!(cook_model(&model, &tiny).is_err());
}

#[test]
fn a_model_without_room_for_triangles_is_refused() {
    let model = model_of(vec![grid_part(4, false, None)], vec![]);
    let aim = Aim {
        triangles: 0,
        ..roomy()
    };
    assert!(cook_model(&model, &aim).is_err());
}

#[test]
fn a_broken_model_is_refused() {
    let mut model = model_of(vec![grid_part(4, false, None)], vec![]);
    model.parts[0].positions[3][1] = f32::NAN;
    assert!(cook_model(&model, &roomy()).is_err());
    let mut model = model_of(vec![grid_part(4, false, None)], vec![]);
    model.parts[0].corners[0][0] = 9999;
    assert!(cook_model(&model, &roomy()).is_err());
    let mut model = model_of(vec![grid_part(4, false, None)], vec![]);
    model.parts[0].positions[0][0] = 1e12;
    assert!(cook_model(&model, &roomy()).is_err());
}

#[test]
fn only_used_materials_are_kept_and_too_many_parts_are_merged() {
    let triangle = |material: usize, x: f32| Part {
        material: Some(material),
        positions: vec![[x, 0.0, 0.0], [x + 1.0, 0.0, 0.0], [x, 1.0, 0.0]],
        corners: vec![[0, 1, 2]],
        ..Part::default()
    };
    let parts: Vec<Part> = (0..70_000).map(|i| triangle(i % 2 * 2, i as f32)).collect();
    // Materials 0 and 2 are used, 1 isn't.
    let model = model_of(parts, vec![material(None), material(None), material(None)]);
    let cooked = cook_model(&model, &roomy()).unwrap();
    assert_eq!(cooked.triangles, 70_000);
    assert!(!cooked.simplified);
    let read = read_model(&cooked.bytes).unwrap();
    assert_eq!(read.materials.len(), 2);
    assert_eq!(read.parts.len(), 2);
    assert_eq!(
        read.parts.iter().map(|p| p.material).collect::<Vec<_>>(),
        [Some(0), Some(1)]
    );
}

/// A triangle of its own for each of `materials`.
fn one_triangle_each(materials: Vec<Material>) -> Model {
    let parts = (0..materials.len())
        .map(|i| Part {
            material: Some(i),
            positions: vec![
                [i as f32, 0.0, 0.0],
                [i as f32 + 1.0, 0.0, 0.0],
                [i as f32, 1.0, 0.0],
            ],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            corners: vec![[0, 1, 2]],
            ..Part::default()
        })
        .collect();
    model_of(parts, materials)
}

fn colored(color: [f32; 4]) -> Material {
    Material {
        color,
        ..Material::default()
    }
}

/// `count` colours, far apart from one another.
fn distinct_colors(count: usize) -> Vec<[f32; 4]> {
    (0..count)
        .map(|i| {
            let step = |of: usize| (of % 5) as f32 / 4.0;
            [step(i), step(i / 5), step(i / 25), 1.0]
        })
        .collect()
}

#[test]
fn at_most_64_materials_stay_as_they_are() {
    let colors = distinct_colors(64);
    let model = one_triangle_each(colors.iter().map(|&color| colored(color)).collect());
    let cooked = cook_model(&model, &roomy()).unwrap();
    assert!(!cooked.simplified);
    let read = read_model(&cooked.bytes).unwrap();
    let kept: Vec<[f32; 4]> = read.materials.iter().map(|m| m.color).collect();
    assert_eq!(kept, colors);
}

#[test]
fn past_64_materials_the_closest_colours_are_combined() {
    // The file that showed this had 69. Here 5 of them nearly repeat others.
    let mut colors = distinct_colors(64);
    for i in 0..5 {
        let near = colors[i * 10];
        colors.push([near[0] + 0.001, near[1], near[2], near[3]]);
    }
    let model = one_triangle_each(colors.iter().map(|&color| colored(color)).collect());
    let cooked = cook_model(&model, &roomy()).unwrap();
    assert!(cooked.simplified);
    assert_eq!(cooked.triangles, 69);
    assert!(checked(&cooked, &roomy_limits()).is_ok());
    let read = read_model(&cooked.bytes).unwrap();
    assert_eq!(read.materials.len(), 64);
    assert_eq!(read.parts.len(), 69);
    // Every triangle looks about as it did.
    for part in &read.parts {
        // Positions come back quantized: the part's leftmost x is its index, nearly.
        let leftmost = part.positions.iter().map(|p| p[0]).fold(f32::MAX, f32::min);
        let original = colors[leftmost.round() as usize];
        let shown = read.materials[part.material.unwrap()].color;
        for (a, b) in original.iter().zip(&shown) {
            assert!((a - b).abs() < 0.01, "{original:?} shown as {shown:?}");
        }
    }
}

#[test]
fn any_number_of_plain_materials_is_cooked() {
    let colors: Vec<[f32; 4]> = (0..1000)
        .map(|i| [i as f32 / 1000.0, 0.5, 1.0 - i as f32 / 1000.0, 1.0])
        .collect();
    let model = one_triangle_each(colors.iter().map(|&color| colored(color)).collect());
    let cooked = cook_model(&model, &roomy()).unwrap();
    assert!(checked(&cooked, &roomy_limits()).is_ok());
    assert_eq!(read_model(&cooked.bytes).unwrap().materials.len(), 64);
}

#[test]
fn materials_of_one_colour_become_one() {
    // Nothing tells them apart, so one material is all they need.
    let model = one_triangle_each(vec![colored([0.5, 0.5, 0.5, 1.0]); 70]);
    let cooked = cook_model(&model, &roomy()).unwrap();
    assert!(!cooked.simplified);
    let read = read_model(&cooked.bytes).unwrap();
    assert_eq!(read.materials.len(), 1);
    assert_eq!(read.materials[0].color, [0.5, 0.5, 0.5, 1.0]);
    assert_eq!(read.parts.len(), 70);
}

#[test]
fn past_64_materials_some_pictures_become_their_average() {
    // The first ones are all red pictures, and cover more than the rest; then plain blues.
    let red = Image {
        width: 2,
        height: 2,
        rgba: [255, 0, 0, 255].repeat(4),
    };
    let mut materials: Vec<Material> = (0..60)
        .map(|_| Material {
            texture: Some(red.clone()),
            ..Material::default()
        })
        .collect();
    materials.extend((0..10).map(|_| colored([0.0, 0.0, 1.0, 1.0])));
    let mut model = one_triangle_each(materials);
    for part in &mut model.parts[..48] {
        part.positions.extend([[0.0, 0.0, 1.0]]);
        part.uvs.push([1.0, 1.0]);
        part.corners.push([0, 1, 3]);
    }
    let cooked = cook_model(&model, &roomy()).unwrap();
    assert!(cooked.simplified);
    assert!(checked(&cooked, &roomy_limits()).is_ok());
    let read = read_model(&cooked.bytes).unwrap();
    assert_eq!(read.materials.len(), 50);
    let pictured = read.materials.iter().filter(|m| m.texture.is_some());
    assert_eq!(pictured.count(), 48);
    for part in &read.parts {
        let material = &read.materials[part.material.unwrap()];
        // A part without a picture has no use for UVs.
        assert_eq!(part.uvs.is_empty(), material.texture.is_none());
        if material.texture.is_none() {
            let color = material.color;
            let red = color[0] > 0.99 && color[2] < 0.01;
            let blue = color[2] > 0.99 && color[0] < 0.01;
            assert!(red || blue, "{color:?}");
        }
    }
}

// ---- Cooking images ----

fn jpeg_of(image: &Image, quality: u8) -> Vec<u8> {
    image.to_jpeg(quality).unwrap()
}

fn unwrapped(cooked: &Cooked) -> &[u8] {
    &cooked.bytes[6..]
}

#[test]
fn a_jpeg_within_the_aim_is_kept_as_it_is() {
    let file = jpeg_of(&picture(64, 48, false, false), 85);
    let aim = Aim {
        bytes: file.len() as u64 + 6,
        ..roomy()
    };
    let cooked = cook_image(&file, &aim).unwrap();
    assert!(!cooked.simplified);
    assert_eq!((cooked.triangles, cooked.pixels), (0, 64 * 48));
    assert_eq!(unwrapped(&cooked), file);
    assert_eq!(&cooked.bytes[..6], b"ETRF\x01\x00");
}

#[test]
fn a_png_with_alpha_stays_a_png() {
    let file = png_of(&picture(64, 48, true, false));
    let cooked = cook_image(&file, &roomy()).unwrap();
    assert!(!cooked.simplified);
    assert_eq!(unwrapped(&cooked), file);

    // Too big: scaled, and still a PNG with its alpha.
    let aim = Aim {
        longest_side: 32,
        ..roomy()
    };
    let cooked = cook_image(&file, &aim).unwrap();
    assert!(cooked.simplified);
    assert_eq!(cooked.pixels, 32 * 24);
    let small = image::decode(unwrapped(&cooked)).unwrap();
    assert_eq!((small.width, small.height), (32, 24));
    assert!(unwrapped(&cooked).starts_with(&[0x89, b'P', b'N', b'G']));
    assert!(!small.is_opaque());
}

#[test]
fn a_picture_with_too_many_pixels_is_scaled() {
    let file = jpeg_of(&picture(200, 100, false, false), 90);
    let aim = Aim {
        pixels: 5_000,
        ..roomy()
    };
    let cooked = cook_image(&file, &aim).unwrap();
    assert!(cooked.simplified);
    assert!(
        u64::from(cooked.pixels) <= 5_000 && cooked.pixels > 4_000,
        "{}",
        cooked.pixels
    );
    let small = image::decode(unwrapped(&cooked)).unwrap();
    assert_eq!(small.width * small.height, cooked.pixels);
    assert_eq!(small.width, small.height * 2);
}

#[test]
fn the_bytes_of_an_image_are_met() {
    // Noise doesn't compress: it takes lower qualities, then smaller sizes.
    let noisy = picture(256, 256, false, true);
    let png = png_of(&noisy);
    assert!(png.len() > 150_000);
    let as_jpeg = cook_image(
        &png,
        &Aim {
            bytes: 120_000,
            ..roomy()
        },
    )
    .unwrap();
    // An opaque PNG that doesn't fit becomes a JPEG.
    assert!(unwrapped(&as_jpeg).starts_with(&[0xFF, 0xD8]));
    assert!(as_jpeg.bytes.len() <= 120_000);

    let aim = Aim {
        bytes: 8_000,
        ..roomy()
    };
    let cooked = cook_image(&png, &aim).unwrap();
    assert!(
        cooked.bytes.len() as u64 <= aim.bytes,
        "{}",
        cooked.bytes.len()
    );
    assert!(cooked.simplified);
    assert!(cooked.pixels < 256 * 256);

    // With alpha it stays a PNG and shrinks until it fits.
    let transparent = png_of(&picture(256, 256, true, true));
    let aim = Aim {
        bytes: 20_000,
        ..roomy()
    };
    let cooked = cook_image(&transparent, &aim).unwrap();
    assert!(cooked.bytes.len() as u64 <= aim.bytes);
    assert!(unwrapped(&cooked).starts_with(&[0x89, b'P']));

    // Nothing is that small.
    assert!(
        cook_image(
            &png,
            &Aim {
                bytes: 30,
                ..roomy()
            }
        )
        .is_err()
    );
    assert!(
        cook_image(
            &png,
            &Aim {
                pixels: 0,
                ..roomy()
            }
        )
        .is_err()
    );
    assert!(cook_image(b"not an image", &roomy()).is_err());
}

// ---- Checking what arrives ----

fn image_check(file: &[u8], limits: &Limits) -> Result<Checked, String> {
    let cooked = cook_image(file, &roomy()).unwrap();
    checked(&cooked, limits)
}

#[test]
fn a_received_image_is_made_again() {
    let opaque = jpeg_of(&picture(40, 30, false, false), 50);
    let Ok(Checked::Image { file, extension }) = image_check(&opaque, &roomy_limits()) else {
        panic!("not an image");
    };
    assert_eq!(extension, "jpg");
    assert_ne!(file, opaque);
    let again = image::decode(&file).unwrap();
    assert_eq!((again.width, again.height), (40, 30));

    let transparent = png_of(&picture(40, 30, true, false));
    let Ok(Checked::Image { file, extension }) = image_check(&transparent, &roomy_limits()) else {
        panic!("not an image");
    };
    assert_eq!(extension, "png");
    assert_eq!(
        image::decode(&file).unwrap(),
        image::decode(&transparent).unwrap()
    );
}

#[test]
fn the_declared_counts_must_be_true() {
    let image = cook_image(&png_of(&picture(40, 30, true, false)), &roomy()).unwrap();
    let limits = roomy_limits();
    assert!(check(&image.bytes, &limits, 0, 1200).is_ok());
    assert!(check(&image.bytes, &limits, 0, 1199).is_err());
    assert!(check(&image.bytes, &limits, 1, 1200).is_err());

    let model = sample_model();
    assert!(check(&model.bytes, &limits, model.triangles, model.pixels).is_ok());
    assert!(check(&model.bytes, &limits, model.triangles + 1, model.pixels).is_err());
    assert!(check(&model.bytes, &limits, model.triangles - 1, model.pixels).is_err());
    assert!(check(&model.bytes, &limits, model.triangles, model.pixels + 1).is_err());
    assert!(check(&model.bytes, &limits, model.triangles, 0).is_err());
}

#[test]
fn the_limits_are_kept() {
    let model = sample_model();
    let mut limits = roomy_limits();
    assert!(checked(&model, &limits).is_ok());
    limits.triangles = u64::from(model.triangles) - 1;
    assert!(checked(&model, &limits).is_err());
    limits = roomy_limits();
    limits.pixels = u64::from(model.pixels) - 1;
    assert!(checked(&model, &limits).is_err());
    limits = roomy_limits();
    limits.longest_side = 63;
    assert!(checked(&model, &limits).is_err());

    let image = cook_image(&png_of(&picture(100, 40, false, false)), &roomy()).unwrap();
    assert!(
        checked(
            &image,
            &Limits {
                longest_side: 100,
                ..roomy_limits()
            }
        )
        .is_ok()
    );
    assert!(
        checked(
            &image,
            &Limits {
                longest_side: 99,
                ..roomy_limits()
            }
        )
        .is_err()
    );
    assert!(
        checked(
            &image,
            &Limits {
                pixels: 3999,
                ..roomy_limits()
            }
        )
        .is_err()
    );
}

/// A model with a picture: 2 parts and 2 materials.
fn sample_model() -> Cooked {
    let model = model_of(
        vec![grid_part(6, false, Some(0)), grid_part(3, true, Some(1))],
        vec![
            material(Some(picture(64, 40, false, false))),
            material(None),
        ],
    );
    cook_model(&model, &roomy()).unwrap()
}

/// The body of a cooked model file.
fn body_of(file: &[u8]) -> Vec<u8> {
    decompress_to_vec_zlib(&file[10..]).unwrap()
}

/// A model file of `body`, with the header and length of a good one.
fn file_of(body: &[u8]) -> Vec<u8> {
    format::pack(body).unwrap()
}

fn refused(bytes: &[u8], limits: &Limits, triangles: u32, pixels: u32) -> bool {
    check(bytes, limits, triangles, pixels).is_err()
}

#[test]
fn a_wrong_header_is_refused() {
    let model = sample_model();
    let limits = roomy_limits();
    let (t, p) = (model.triangles, model.pixels);
    assert!(!refused(&model.bytes, &limits, t, p));
    let with = |at: usize, value: u8| {
        let mut bytes = model.bytes.clone();
        bytes[at] = value;
        bytes
    };
    assert!(refused(&with(0, b'X'), &limits, t, p));
    assert!(refused(&with(3, b'G'), &limits, t, p));
    assert!(refused(&with(4, 2), &limits, t, p));
    assert!(refused(&with(4, 0), &limits, t, p));
    assert!(refused(&with(5, 2), &limits, t, p));
    assert!(refused(&with(5, 255), &limits, t, p));
    // A model said to be an image and the other way around.
    assert!(refused(&with(5, 0), &limits, t, p));
    let image = cook_image(&png_of(&picture(8, 8, false, false)), &roomy()).unwrap();
    let mut as_model = image.bytes.clone();
    as_model[5] = 1;
    assert!(refused(&as_model, &limits, 0, 64));
    for short in 0..6 {
        assert!(refused(&model.bytes[..short], &limits, t, p));
    }
    assert!(refused(b"", &limits, 0, 0));
}

#[test]
fn a_cut_model_is_refused_at_every_length() {
    let model = sample_model();
    let limits = roomy_limits();
    for length in 0..model.bytes.len() {
        assert!(
            refused(
                &model.bytes[..length],
                &limits,
                model.triangles,
                model.pixels
            ),
            "length {length}"
        );
    }
}

#[test]
fn more_after_the_end_is_refused() {
    let model = sample_model();
    let limits = roomy_limits();
    let mut body = body_of(&model.bytes);
    body.push(0);
    assert!(refused(
        &file_of(&body),
        &limits,
        model.triangles,
        model.pixels
    ));
    // A body missing its last byte, even if it says so.
    let mut body = body_of(&model.bytes);
    body.pop();
    assert!(refused(
        &file_of(&body),
        &limits,
        model.triangles,
        model.pixels
    ));
    // Bytes after the zlib stream.
    let mut longer = model.bytes.clone();
    longer.extend_from_slice(&[0, 0, 0, 0]);
    assert!(refused(&longer, &limits, model.triangles, model.pixels));
}

/// A part of one triangle for a hand-made body: 3 vertices, then its planes and corners.
fn raw_part(material: u8, corners: [u8; 3]) -> Vec<u8> {
    let mut part = vec![material];
    part.extend(3u32.to_le_bytes());
    part.extend(1u32.to_le_bytes());
    part.push(0);
    part.extend([0u8; 18]);
    part.extend(corners);
    part
}

/// A hand-made body: bounds 0 to 1, no materials, then `parts`.
fn raw_body(part_count: u32, parts: &[Vec<u8>]) -> Vec<u8> {
    let mut body = Vec::new();
    for value in [0.0f32, 0.0, 0.0, 1.0, 1.0, 1.0] {
        body.extend(value.to_le_bytes());
    }
    body.push(0);
    body.extend(part_count.to_le_bytes());
    for part in parts {
        body.extend(part);
    }
    body
}

#[test]
fn a_hand_made_body_is_read_and_its_lies_are_refused() {
    let limits = roomy_limits();
    // Corners 0, +1, +1 (zigzag 0, 2, 2): a triangle over the three vertices.
    let good = raw_body(1, &[raw_part(255, [0, 2, 2])]);
    assert!(check(&file_of(&good), &limits, 1, 0).is_ok());
    // An index past the vertices, a negative one, a material that isn't there.
    let past = raw_body(1, &[raw_part(255, [0, 2, 4])]);
    assert!(refused(&file_of(&past), &limits, 1, 0));
    let negative = raw_body(1, &[raw_part(255, [1, 0, 0])]);
    assert!(refused(&file_of(&negative), &limits, 1, 0));
    let unknown = raw_body(1, &[raw_part(0, [0, 2, 2])]);
    assert!(refused(&file_of(&unknown), &limits, 1, 0));
    // A varint that never ends.
    let endless = raw_body(1, &[raw_part(255, [0, 0xff, 0xff])]);
    assert!(refused(&file_of(&endless), &limits, 1, 0));
    let mut long = raw_part(255, [0, 2, 2]);
    long.truncate(long.len() - 1);
    long.extend([0xff; 8]);
    assert!(refused(&file_of(&raw_body(1, &[long])), &limits, 1, 0));
    // Part counts that are not the parts there.
    assert!(refused(
        &file_of(&raw_body(2, &[raw_part(255, [0, 2, 2])])),
        &limits,
        1,
        0
    ));
    assert!(refused(
        &file_of(&raw_body(0, &[raw_part(255, [0, 2, 2])])),
        &limits,
        1,
        0
    ));
    assert!(refused(&file_of(&raw_body(70_000, &[])), &limits, 1, 0));
    assert!(refused(&file_of(&raw_body(u32::MAX, &[])), &limits, 1, 0));
}

#[test]
fn numbers_that_are_not_numbers_are_refused() {
    let limits = roomy_limits();
    let good = raw_body(1, &[raw_part(255, [0, 2, 2])]);
    for bits in [
        f32::NAN.to_bits(),
        f32::INFINITY.to_bits(),
        2e9f32.to_bits(),
    ] {
        for at in [0, 4, 12, 20] {
            let mut body = good.clone();
            body[at..at + 4].copy_from_slice(&bits.to_le_bytes());
            assert!(refused(&file_of(&body), &limits, 1, 0), "{bits:x} at {at}");
        }
    }
    // Bounds the wrong way round.
    let mut body = good.clone();
    body[12..16].copy_from_slice(&(-1.0f32).to_le_bytes());
    assert!(refused(&file_of(&body), &limits, 1, 0));

    // A colour, and a UV.
    let mut with_material = Vec::new();
    with_material.extend_from_slice(&good[..24]);
    with_material.push(1);
    for value in [1.0f32, f32::NAN, 1.0, 1.0] {
        with_material.extend(value.to_le_bytes());
    }
    with_material.extend(0u32.to_le_bytes());
    with_material.extend(0u32.to_le_bytes());
    assert!(refused(&file_of(&with_material), &limits, 0, 0));

    let mut part = vec![255];
    part.extend(3u32.to_le_bytes());
    part.extend(1u32.to_le_bytes());
    part.push(1);
    part.extend([0u8; 18]);
    for _ in 0..3 {
        part.extend(0.5f32.to_le_bytes());
        part.extend(f32::NAN.to_le_bytes());
    }
    part.extend([0, 2, 2]);
    assert!(refused(&file_of(&raw_body(1, &[part])), &limits, 1, 0));
}

#[test]
fn huge_counts_with_few_bytes_are_refused_before_anything_is_allocated() {
    let limits = roomy_limits();
    for (vertices, triangles) in [
        (u32::MAX, 0u32),
        (0, u32::MAX),
        (u32::MAX, u32::MAX),
        (1 << 28, 1 << 20),
    ] {
        let mut part = vec![255];
        part.extend(vertices.to_le_bytes());
        part.extend(triangles.to_le_bytes());
        part.push(1);
        part.extend([0u8; 40]);
        let file = file_of(&raw_body(1, &[part]));
        assert!(refused(&file, &limits, 0, 0), "{vertices} {triangles}");
    }
    // A texture longer than the body.
    let mut body = raw_body(0, &[])[..24].to_vec();
    body.push(1);
    for value in [1.0f32; 4] {
        body.extend(value.to_le_bytes());
    }
    body.extend(u32::MAX.to_le_bytes());
    body.extend([0u8; 16]);
    assert!(refused(&file_of(&body), &limits, 0, 0));
    // A texture that says it is huge.
    let mut png = png_of(&picture(4, 4, false, false));
    let at = png.windows(4).position(|w| w == b"IHDR").unwrap() + 4;
    png[at..at + 8].copy_from_slice(&[0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff]);
    let mut body = raw_body(0, &[])[..24].to_vec();
    body.push(1);
    for value in [1.0f32; 4] {
        body.extend(value.to_le_bytes());
    }
    body.extend((png.len() as u32).to_le_bytes());
    body.extend(&png);
    body.extend(0u32.to_le_bytes());
    assert!(refused(&file_of(&body), &limits, 0, 0));
}

#[test]
fn a_zlib_bomb_is_refused() {
    let limits = Limits {
        triangles: 1000,
        longest_side: 64,
        pixels: 4096,
    };
    let bomb = compress_to_vec_zlib(&vec![0u8; 100 << 20], 9);
    assert!(bomb.len() < 200_000);
    // Says it is small, unpacks to a lot.
    let mut file = b"ETRF\x01\x01".to_vec();
    file.extend(100u32.to_le_bytes());
    file.extend(&bomb);
    assert!(refused(&file, &limits, 0, 0));
    // Says it is as big as it is: past what the limits could ever need.
    let mut file = b"ETRF\x01\x01".to_vec();
    file.extend((100u32 << 20).to_le_bytes());
    file.extend(&bomb);
    assert!(refused(&file, &limits, 0, 0));
    // Says nothing sensible.
    let mut file = b"ETRF\x01\x01".to_vec();
    file.extend(u32::MAX.to_le_bytes());
    file.extend(&bomb);
    assert!(refused(&file, &limits, 0, 0));
    // Not zlib at all.
    let mut file = b"ETRF\x01\x01".to_vec();
    file.extend(100u32.to_le_bytes());
    file.extend([0x13; 50]);
    assert!(refused(&file, &limits, 0, 0));
}

#[test]
fn bad_images_are_refused() {
    let limits = roomy_limits();
    let image = |bytes: &[u8]| {
        let mut file = b"ETRF\x01\x00".to_vec();
        file.extend(bytes);
        file
    };
    assert!(refused(&image(b""), &limits, 0, 0));
    assert!(refused(&image(b"GIF89a"), &limits, 0, 0));
    assert!(refused(
        &image(&[0x89, b'P', b'N', b'G', 1, 2, 3]),
        &limits,
        0,
        0
    ));
    assert!(refused(
        &image(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]),
        &limits,
        0,
        0
    ));
    // A PNG that says it is far bigger than the limits.
    let mut png = png_of(&picture(4, 4, false, false));
    let at = png.windows(4).position(|w| w == b"IHDR").unwrap() + 4;
    png[at..at + 8].copy_from_slice(&[0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff]);
    assert!(refused(&image(&png), &limits, 0, 0));
}

// ---- Reading files ----

#[test]
fn a_cooked_model_is_read_as_a_file() {
    let model = model_of(
        vec![grid_part(10, false, Some(0))],
        vec![material(Some(picture(16, 16, false, false)))],
    );
    let cooked = cook_model(&model, &roomy()).unwrap();
    let folder = std::env::temp_dir().join(format!("model_io_shared_{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join(format!("{:016x}.{EXTENSION}", cooked.content()));
    std::fs::write(&path, &cooked.bytes).unwrap();
    assert_eq!(Format::of(&path), Some(Format::Shared));
    let read = readers::read(&path).unwrap();
    assert_eq!(read.triangle_count(), 200);
    assert!(read.materials[0].texture.is_some());
    assert!(Format::Shared.has_fixed_space());
    assert!(crate::companions::of(&path).found.is_empty());

    // Not a cooked model.
    std::fs::write(&path, b"ETRF").unwrap();
    assert!(readers::read(&path).is_err());
    std::fs::remove_dir_all(&folder).unwrap();
}
