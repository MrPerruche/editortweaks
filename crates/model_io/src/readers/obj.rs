//! Wavefront OBJ, read with the `tobj` crate, plus the `.mtl` file next to it if there is one.
//!
//! Gotchas: OBJ's texture V points up while the engine's points down, so V is flipped here. Faces
//! may have any number of corners (tobj triangulates them), and tobj's `single_index` mode gives
//! every position/UV/normal combination its own vertex, so the file's hard edges stay split. A
//! file without normals has no such splits, so its sharp edges are split by angle instead
//! ([`Part::split_sharp_edges`]). A broken or missing `.mtl` or texture only costs the look, never
//! the model. tobj rejects a whole file over a `usemtl` line naming no material, which Blender 5
//! writes for objects without one: those lines are read as naming no known material
//! ([`name_unnamed_materials`]).

use std::borrow::Cow;
use std::path::Path;

use crate::image;
use crate::model::{Material, Model, Part, SMOOTH_ANGLE};

const OPTIONS: tobj::LoadOptions = tobj::LoadOptions {
    single_index: true,
    triangulate: true,
    ignore_points: true,
    ignore_lines: true,
};

/// What an unnamed `usemtl` is read as: a material no `.mtl` file defines, so none.
const NO_MATERIAL: &str = "editortweaks-no-material";

/// Reads the OBJ file at `path`.
pub fn read(path: &Path) -> Result<Model, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let folder = path.parent().unwrap_or(Path::new(""));
    convert_text(&String::from_utf8_lossy(&bytes), folder, |name| {
        tobj::load_mtl(folder.join(name))
    })
}

/// Reads an OBJ file's `text`; `load_mtl` reads the material libraries it names, its textures
/// are relative to `folder`.
fn convert_text(
    text: &str,
    folder: &Path,
    load_mtl: impl Fn(&Path) -> tobj::MTLLoadResult,
) -> Result<Model, String> {
    let text = name_unnamed_materials(text);
    let (models, materials) = tobj::load_obj_buf(&mut text.as_bytes(), &OPTIONS, load_mtl)
        .map_err(|error| error.to_string())?;
    // No .mtl, or one that doesn't parse: carry on with the default look.
    convert(models, materials.unwrap_or_default(), folder)
}

/// `text` with its `usemtl` lines that name no material naming [`NO_MATERIAL`] instead.
fn name_unnamed_materials(text: &str) -> Cow<'_, str> {
    let unnamed = |line: &str| line.trim() == "usemtl";
    if !text.lines().any(unnamed) {
        return Cow::Borrowed(text);
    }
    let mut named = String::with_capacity(text.len());
    for line in text.lines() {
        if unnamed(line) {
            named.push_str("usemtl ");
            named.push_str(NO_MATERIAL);
        } else {
            named.push_str(line);
        }
        named.push('\n');
    }
    Cow::Owned(named)
}

/// Turns tobj's result into a [`Model`]. Texture paths are relative to `folder`.
fn convert(
    models: Vec<tobj::Model>,
    materials: Vec<tobj::Material>,
    folder: &Path,
) -> Result<Model, String> {
    let materials: Vec<Material> = materials
        .iter()
        .map(|material| convert_material(material, folder))
        .collect();
    let parts: Vec<Part> = models
        .iter()
        .filter(|model| !model.mesh.indices.is_empty())
        .map(|model| convert_part(model, materials.len()))
        .collect();
    if parts.is_empty() {
        return Err("no triangles".into());
    }
    Ok(Model { parts, materials })
}

fn convert_part(model: &tobj::Model, material_count: usize) -> Part {
    let mesh = &model.mesh;
    let positions: Vec<[f32; 3]> = mesh.positions.as_chunks::<3>().0.to_vec();
    // One UV per position or none at all; V is flipped (see the module docs).
    let uvs = if mesh.texcoords.len() == positions.len() * 2 {
        mesh.texcoords
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&[u, v]| [u, 1.0 - v])
            .collect()
    } else {
        Vec::new()
    };
    let mut part = Part {
        name: model.name.clone(),
        material: mesh.material_id.filter(|&id| id < material_count),
        positions,
        uvs,
        corners: mesh.indices.as_chunks::<3>().0.to_vec(),
    };
    if mesh.normals.is_empty() {
        part.split_sharp_edges(SMOOTH_ANGLE);
    }
    part
}

/// The diffuse colour (with `d` as alpha) and diffuse texture of an MTL material.
fn convert_material(material: &tobj::Material, folder: &Path) -> Material {
    let [r, g, b] = material.diffuse.unwrap_or([1.0; 3]);
    let texture = material
        .diffuse_texture
        .as_deref()
        .filter(|name| !name.is_empty())
        .and_then(|name| image::read(&folder.join(name)).ok());
    Material {
        name: material.name.clone(),
        color: [r, g, b, material.dissolve.unwrap_or(1.0)],
        texture,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    /// Loads `obj` the way [`read`] does, with `mtl` as the contents of any material library.
    fn load(obj: &str, mtl: &str) -> Result<Model, String> {
        convert_text(obj, Path::new(""), |_| {
            tobj::load_mtl_buf(&mut Cursor::new(mtl))
        })
    }

    #[test]
    fn unnamed_material_is_no_material() {
        // As Blender 5 writes an object without a material, after one with a material.
        let obj = "mtllib car.mtl
v 0 0 0
v 1 0 0
v 0 1 0
usemtl red
f 1 2 3
o bare
                   usemtl 
f 1 3 2
usemtl
f 2 3 1
";
        let model = load(
            obj,
            "newmtl red
Kd 1 0 0
",
        )
        .unwrap();
        let materials: Vec<Option<usize>> = model.parts.iter().map(|part| part.material).collect();
        assert_eq!(materials, [Some(0), None]);
        assert_eq!(model.parts[1].corners.len(), 2);
        // An MTL file with nothing but comments.
        assert!(
            load(
                obj,
                "# Blender 5.0.1 MTL File
"
            )
            .is_ok()
        );
    }

    #[test]
    fn quad_becomes_two_triangles() {
        let model = load("v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3 4\n", "").unwrap();
        let part = &model.parts[0];
        assert_eq!(part.positions.len(), 4);
        assert_eq!(part.corners.len(), 2);
        assert!(part.uvs.is_empty());
        assert_eq!(part.material, None);
        assert!(model.validate().is_ok());
    }

    /// A unit cube as an OBJ: 8 positions and 6 quads, with one normal per face (`f 1//1 ...`)
    /// if `with_normals`, without any otherwise.
    fn cube_obj(with_normals: bool) -> String {
        let mut obj = String::new();
        for i in 0..8 {
            obj += &format!("v {} {} {}\n", i & 1, i >> 1 & 1, i >> 2 & 1);
        }
        if with_normals {
            for normal in ["0 0 -1", "0 0 1", "0 -1 0", "0 1 0", "-1 0 0", "1 0 0"] {
                obj += &format!("vn {normal}\n");
            }
        }
        let faces = [
            [1, 3, 4, 2],
            [5, 6, 8, 7],
            [1, 2, 6, 5],
            [3, 7, 8, 4],
            [1, 5, 7, 3],
            [2, 4, 8, 6],
        ];
        for (face, quad) in faces.iter().enumerate() {
            let corners: Vec<String> = quad
                .iter()
                .map(|v| match with_normals {
                    true => format!("{v}//{}", face + 1),
                    false => v.to_string(),
                })
                .collect();
            obj += &format!("f {}\n", corners.join(" "));
        }
        obj
    }

    #[test]
    fn sharp_edges_are_split_only_without_normals() {
        // No `vn` in the faces: the 8 shared vertices become 24.
        let model = load(&cube_obj(false), "").unwrap();
        assert_eq!(model.parts[0].positions.len(), 24);
        assert_eq!(model.parts[0].corners.len(), 12);
        assert!(model.validate().is_ok());

        // A normal per face: tobj already made 24 vertices, left as they are.
        let model = load(&cube_obj(true), "").unwrap();
        assert_eq!(model.parts[0].positions.len(), 24);
        assert!(model.validate().is_ok());
    }

    #[test]
    fn uvs_are_flipped_and_hard_edges_stay_split() {
        // With a normal, so the angle-based splitting of files without normals stays out of it.
        let obj = "v 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0 0\nvt 1 0.25\nvt 0 1\nvt 0.5 0.5\nvn 0 0 1\n\
                   f 1/1/1 2/2/1 3/3/1\nf 1/4/1 3/3/1 2/2/1\n";
        let model = load(obj, "").unwrap();
        let part = &model.parts[0];
        // Position 1 is used with two different UVs: two vertices.
        assert_eq!(part.positions.len(), 4);
        assert_eq!(part.uvs.len(), 4);
        assert_eq!(part.uvs[0], [0.0, 1.0]);
        assert_eq!(part.uvs[1], [1.0, 0.75]);
        assert!(model.validate().is_ok());
    }

    #[test]
    fn material_colour_and_part_assignment() {
        let obj = "mtllib car.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl red\nf 1 2 3\n";
        let mtl = "newmtl blue\nKd 0 0 1\nnewmtl red\nKd 1 0.5 0\nd 0.25\n";
        let model = load(obj, mtl).unwrap();
        assert_eq!(model.materials.len(), 2);
        let red = model.parts[0].material.expect("the part uses a material");
        assert_eq!(model.materials[red].name, "red");
        assert_eq!(model.materials[red].color, [1.0, 0.5, 0.0, 0.25]);
        assert!(model.materials[red].texture.is_none());
        assert!(model.validate().is_ok());
    }

    #[test]
    fn broken_material_library_is_not_an_error() {
        let obj = "mtllib missing.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl red\nf 1 2 3\n";
        let (models, materials) = tobj::load_obj_buf(&mut Cursor::new(obj), &OPTIONS, |_| {
            Err(tobj::LoadError::OpenFileFailed)
        })
        .unwrap();
        let model = convert(models, materials.unwrap_or_default(), Path::new("")).unwrap();
        assert!(model.materials.is_empty());
        assert_eq!(model.parts[0].material, None);
        assert!(model.validate().is_ok());
    }

    #[test]
    fn missing_texture_is_not_an_error() {
        let obj = "mtllib car.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl tex\nf 1 2 3\n";
        let model = load(obj, "newmtl tex\nKd 1 1 1\nmap_Kd does_not_exist.png\n").unwrap();
        assert_eq!(model.materials.len(), 1);
        assert!(model.materials[0].texture.is_none());
    }

    #[test]
    fn no_faces_is_an_error() {
        assert!(load("v 0 0 0\nv 1 0 0\nl 1 2\n", "").is_err());
    }
}
