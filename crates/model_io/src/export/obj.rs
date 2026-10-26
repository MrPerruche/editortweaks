//! Writes an [`Export`] as a Wavefront `.obj` with its `.mtl` material library and the textures
//! the materials use, for programs (Blender, slicers, most 3D tools) that read OBJ but not glTF.
//!
//! Layout: one `o` per object, and inside it a `usemtl` and the faces per part. Every part has
//! positions (`v`), normals ([`super::vertex_normals`], `vn`) and UVs when it has them (`vt`);
//! indices are 1-based and count on across the whole file.
//!
//! Space: like [`super::glb`]'s, Y up and right-handed, `(-Y, Z, X) * scale` from the editor's
//! (X, Y, Z) in centimetres, which is what importers expect from OBJ by default. The conversion
//! is a reflection, so the corners keep their order and come out counter-clockwise, the front
//! face in OBJ.
//!
//! Materials: the base `Kd`, `d`, `Ke`, and the PBR extension (`Pr`, `Pm`) that Blender reads;
//! textures as `map_Kd` (base colour), `norm` (normals) and `map_Ke` (emissive). A look's
//! metallic-roughness texture packs both in one image (green and blue), which OBJ's separate
//! `map_Pr`/`map_Pm` can't express, so it is left out, and so is occlusion.

use std::fmt::Write as _;
use std::path::Path;

use super::{Export, Look, vertex_normals};
use crate::image::{Image, linear_to_srgb};
use crate::model::Part;

/// What the material of parts that have none is called, unless a look has taken the name.
const DEFAULT_MATERIAL: &str = "Default";

/// The files of an OBJ export.
#[derive(Clone, Debug, Default)]
pub struct Obj {
    /// The `.obj`'s text.
    pub obj: String,
    /// The `.mtl`'s text, what the `.obj`'s `mtllib` line names.
    pub mtl: String,
    /// The textures the `.mtl` points to: file name and PNG bytes, each file once. They go next to
    /// the `.mtl`.
    pub images: Vec<(String, Vec<u8>)>,
}

/// `position` (the editor's space, cm) in OBJ's space, times `scale`.
fn to_obj(position: [f32; 3], scale: f32) -> [f32; 3] {
    [-position[1], position[2], position[0]].map(|axis| axis * scale)
}

/// `normal` (the editor's space) in OBJ's space.
fn normal_to_obj(normal: [f32; 3]) -> [f32; 3] {
    [-normal[1], normal[2], normal[0]]
}

/// `export` as an `.obj` and its `.mtl`, `scale` times the editor's centimetres (0.01 for metres,
/// 10 for millimetres). `material_library` is the `.mtl`'s file name: the `.obj` names it, and the
/// texture files are named after it (`<name without extension>_<n>.png`).
pub fn write(export: &Export, scale: f32, material_library: &str) -> Result<Obj, String> {
    if let Some(look) = export.looks.iter().find(|look| {
        look.textures
            .used()
            .any(|image| image >= export.images.len())
    }) {
        return Err(format!(
            "look {:?}: a texture points past the images",
            look.name
        ));
    }
    let base = Path::new(material_library).file_stem().map_or_else(
        || "textures".to_string(),
        |stem| stem.to_string_lossy().into(),
    );

    let (names, default_name) = material_names(&export.looks);
    let mut obj = String::new();
    let _ = writeln!(
        obj,
        "# {} (Brick Rigs editortweaks)",
        one_line(&export.name)
    );
    let _ = writeln!(obj, "mtllib {material_library}");
    // How many positions and UVs came before, as OBJ counts them across the whole file.
    let (mut vertices, mut uvs) = (0usize, 0usize);
    let mut default_used = false;
    for object in &export.objects {
        let parts: Vec<&Part> = object
            .parts
            .iter()
            .filter(|part| !part.corners.is_empty())
            .collect();
        if parts.is_empty() {
            continue;
        }
        let _ = writeln!(obj, "o {}", safe_name(&object.name, "object"));
        for part in parts {
            check(part)?;
            let material = part.material.filter(|&material| material < names.len());
            default_used |= material.is_none();
            let name = material.map_or(&default_name, |material| &names[material]);
            let _ = writeln!(obj, "usemtl {name}");
            for position in &part.positions {
                let [x, y, z] = to_obj(*position, scale);
                let _ = writeln!(obj, "v {x} {y} {z}");
            }
            for [u, v] in &part.uvs {
                let _ = writeln!(obj, "vt {u} {v}");
            }
            for normal in vertex_normals(part) {
                let [x, y, z] = normal_to_obj(normal);
                let _ = writeln!(obj, "vn {x} {y} {z}");
            }
            let textured = !part.uvs.is_empty();
            for corners in &part.corners {
                obj.push('f');
                for &index in corners {
                    let vertex = vertices + index as usize + 1;
                    if textured {
                        let uv = uvs + index as usize + 1;
                        let _ = write!(obj, " {vertex}/{uv}/{vertex}");
                    } else {
                        let _ = write!(obj, " {vertex}//{vertex}");
                    }
                }
                obj.push('\n');
            }
            vertices += part.positions.len();
            uvs += part.uvs.len();
        }
    }

    let mut files = ImageFiles::new(&base, export.images.len());
    let mut mtl = String::new();
    let _ = writeln!(
        mtl,
        "# {} (Brick Rigs editortweaks)",
        one_line(&export.name)
    );
    for (look, name) in export.looks.iter().zip(&names) {
        material(&mut mtl, name, look, &mut files);
    }
    if default_used {
        material(&mut mtl, &default_name, &Look::default(), &mut files);
    }
    Ok(Obj {
        obj,
        mtl,
        images: files.write(&export.images)?,
    })
}

/// A name for each look, and the one for parts without a look: safe in an `.mtl` (no spaces) and
/// all different.
fn material_names(looks: &[Look]) -> (Vec<String>, String) {
    let mut names: Vec<String> = Vec::with_capacity(looks.len() + 1);
    let wanted = looks
        .iter()
        .map(|look| safe_name(&look.name, "look"))
        .chain([DEFAULT_MATERIAL.to_string()]);
    for name in wanted {
        let mut candidate = name.clone();
        let mut number = 2;
        while names.contains(&candidate) {
            candidate = format!("{name}_{number}");
            number += 1;
        }
        names.push(candidate);
    }
    let default_name = names.pop().unwrap_or_default();
    (names, default_name)
}

/// `name` with anything but letters, digits, `-`, `_` and `.` replaced by `_`, or `fallback` if
/// that leaves nothing.
fn safe_name(name: &str, fallback: &str) -> String {
    let name: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.is_empty() {
        fallback.into()
    } else {
        name
    }
}

/// `text` on one line, for a comment.
fn one_line(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
}

/// Refuses parts whose triangles or UVs don't fit their positions.
fn check(part: &Part) -> Result<(), String> {
    let count = part.positions.len();
    if part
        .corners
        .iter()
        .flatten()
        .any(|&index| index as usize >= count)
    {
        return Err(format!(
            "part {:?}: a triangle points past its positions",
            part.name
        ));
    }
    if !part.uvs.is_empty() && part.uvs.len() != count {
        return Err(format!("part {:?}: not one UV per position", part.name));
    }
    Ok(())
}

/// The texture files the materials refer to, named as they are first asked for.
struct ImageFiles {
    base: String,
    /// The file name of each image, once something refers to it.
    names: Vec<Option<String>>,
}

impl ImageFiles {
    fn new(base: &str, images: usize) -> Self {
        Self {
            base: base.to_string(),
            names: vec![None; images],
        }
    }

    /// The file name of `image` (an index into [`Export::images`]), which will be written.
    fn name(&mut self, image: usize) -> &str {
        let base = &self.base;
        self.names[image].get_or_insert_with(|| format!("{base}_{image}.png"))
    }

    /// The files of the images something refers to, in the images' order.
    fn write(self, images: &[Image]) -> Result<Vec<(String, Vec<u8>)>, String> {
        self.names
            .into_iter()
            .zip(images)
            .filter_map(|(name, image)| name.map(|name| (name, image)))
            .map(|(name, image)| Ok((name, image.to_png()?)))
            .collect()
    }
}

/// Appends the `.mtl` entry of `look` called `name`.
fn material(mtl: &mut String, name: &str, look: &Look, files: &mut ImageFiles) {
    let textures = &look.textures;
    let [r, g, b, alpha] = look.color.map(|channel| channel.clamp(0.0, 1.0));
    let [r, g, b] = [r, g, b].map(linear_to_srgb);
    let mut emissive = look
        .emissive
        .map(|channel| linear_to_srgb(channel.clamp(0.0, 1.0)));
    if textures.emissive.is_some() && look.emissive.iter().all(|&channel| channel <= 0.0) {
        // The texture alone gives the light.
        emissive = [1.0; 3];
    }
    let _ = writeln!(mtl, "\nnewmtl {name}");
    let _ = writeln!(mtl, "Kd {r} {g} {b}");
    let _ = writeln!(mtl, "Ke {} {} {}", emissive[0], emissive[1], emissive[2]);
    let _ = writeln!(mtl, "d {alpha}");
    let _ = writeln!(mtl, "Pr {}", look.roughness.clamp(0.0, 1.0));
    let _ = writeln!(mtl, "Pm {}", look.metallic.clamp(0.0, 1.0));
    let _ = writeln!(mtl, "illum 2");
    for (keyword, image) in [
        ("map_Kd", textures.base_color),
        ("map_Ke", textures.emissive),
        ("norm", textures.normal),
    ] {
        if let Some(image) = image {
            let _ = writeln!(mtl, "{keyword} {}", files.name(image));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{Object, box_part, box_uvs};

    fn export() -> Export {
        let mut export = Export {
            name: "Test vehicle".into(),
            ..Export::default()
        };
        let red = export.look_index(Look {
            name: "red paint".into(),
            color: [1.0, 0.0, 0.0, 0.5],
            ..Look::default()
        });
        let mut first = box_part([0.0; 3], [100.0, 200.0, 300.0]);
        first.material = Some(red);
        let second = box_part([0.0; 3], [10.0; 3]);
        export.objects = vec![
            Object {
                name: "Brick 1".into(),
                parts: vec![first],
            },
            Object {
                name: "Brick 2".into(),
                parts: vec![second],
            },
        ];
        export
    }

    fn lines<'a>(text: &'a str, prefix: &'a str) -> impl Iterator<Item = &'a str> {
        text.lines().filter(move |line| line.starts_with(prefix))
    }

    /// The vertex index (1-based) of each corner of a face line.
    fn face_indices(line: &str) -> Vec<usize> {
        line.split_whitespace()
            .skip(1)
            .map(|corner| corner.split('/').next().unwrap().parse().unwrap())
            .collect()
    }

    #[test]
    fn a_box_is_written_in_obj_space() {
        let obj = write(&export(), 0.01, "car.mtl").unwrap();
        assert!(obj.obj.contains("mtllib car.mtl"));
        assert_eq!(lines(&obj.obj, "o ").count(), 2);
        assert_eq!(lines(&obj.obj, "v ").count(), 48);
        assert_eq!(lines(&obj.obj, "vn ").count(), 48);
        assert_eq!(lines(&obj.obj, "vt ").count(), 0);
        assert_eq!(lines(&obj.obj, "f ").count(), 24);
        // The box's far corner (300 up, 200 right, 100 forward) in metres: (-2, 3, 1).
        assert!(lines(&obj.obj, "v ").any(|line| line == "v -2 3 1"));
        // Indices are 1-based, and the second box continues after the first one's 24 positions.
        let faces: Vec<Vec<usize>> = lines(&obj.obj, "f ").map(face_indices).collect();
        assert!(lines(&obj.obj, "f ").next().unwrap().starts_with("f 1//1 "));
        assert!(
            faces[..12]
                .iter()
                .flatten()
                .all(|index| (1..=24).contains(index))
        );
        assert!(
            faces[12..]
                .iter()
                .flatten()
                .all(|index| (25..=48).contains(index))
        );
        // Parts without a look use the default material.
        assert!(obj.obj.contains("usemtl red_paint"));
        assert!(obj.obj.contains("usemtl Default"));
        assert!(obj.mtl.contains("newmtl red_paint"));
        assert!(obj.mtl.contains("newmtl Default"));
        assert!(obj.mtl.contains("d 0.5"));
        assert!(obj.images.is_empty());
    }

    #[test]
    fn faces_stay_counter_clockwise() {
        let obj = write(&export(), 1.0, "car.mtl").unwrap();
        let positions: Vec<Vec<f32>> = lines(&obj.obj, "v ")
            .map(|line| line[2..].split(' ').map(|v| v.parse().unwrap()).collect())
            .collect();
        // Every face of the first box points away from its center, (50, 100, 150) cm in the
        // editor's space and so (-100, 150, 50) in OBJ's.
        let center = [-100.0, 150.0, 50.0];
        for line in lines(&obj.obj, "f ").take(12) {
            let corners: Vec<&Vec<f32>> = face_indices(line)
                .into_iter()
                .map(|index| &positions[index - 1])
                .collect();
            let ab: Vec<f32> = (0..3).map(|i| corners[1][i] - corners[0][i]).collect();
            let ac: Vec<f32> = (0..3).map(|i| corners[2][i] - corners[0][i]).collect();
            let normal = [
                ab[1] * ac[2] - ab[2] * ac[1],
                ab[2] * ac[0] - ab[0] * ac[2],
                ab[0] * ac[1] - ab[1] * ac[0],
            ];
            let outward: f32 = (0..3)
                .map(|i| normal[i] * (corners[0][i] - center[i]))
                .sum();
            assert!(outward > 0.0, "{line}");
        }
    }

    #[test]
    fn textures_are_written_and_referenced() {
        let mut export = export();
        export.images.push(Image {
            width: 1,
            height: 1,
            rgba: vec![255, 128, 0, 255],
        });
        // Never referenced: not written.
        export.images.push(Image {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 255],
        });
        export.looks[0].textures.base_color = Some(0);
        export.looks[0].textures.normal = Some(0);
        box_uvs(&mut export.objects[0].parts[0], 0.01);
        let obj = write(&export, 1.0, "my car.mtl").unwrap();
        assert_eq!(obj.images.len(), 1);
        assert_eq!(obj.images[0].0, "my car_0.png");
        assert_eq!(obj.images[0].1, export.images[0].to_png().unwrap());
        assert!(obj.mtl.contains("map_Kd my car_0.png"));
        assert!(obj.mtl.contains("norm my car_0.png"));
        assert_eq!(lines(&obj.obj, "vt ").count(), 24);
        assert!(
            lines(&obj.obj, "f ")
                .next()
                .unwrap()
                .starts_with("f 1/1/1 ")
        );
        // The part without UVs has none in its faces.
        assert!(lines(&obj.obj, "f ").last().unwrap().contains("//"));

        export.looks[0].textures.normal = Some(5);
        assert!(write(&export, 1.0, "car.mtl").is_err());
    }

    #[test]
    fn material_names_are_safe_and_unique() {
        let looks: Vec<Look> = ["a b", "a_b", "", "Default"]
            .into_iter()
            .map(|name| Look {
                name: name.into(),
                ..Look::default()
            })
            .collect();
        let (names, default_name) = material_names(&looks);
        assert_eq!(names, ["a_b", "a_b_2", "look", "Default"]);
        assert_eq!(default_name, "Default_2");
    }

    #[test]
    fn broken_parts_are_refused() {
        let mut export = export();
        export.objects[0].parts[0].corners = vec![[0, 1, 99]];
        assert!(write(&export, 1.0, "car.mtl").is_err());
    }
}
