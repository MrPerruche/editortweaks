//! Writes an [`Export`] as a `.glb`: glTF 2.0's binary container, the JSON document followed by
//! one buffer with every vertex and index.
//!
//! Layout: a root node named after the vehicle, one child node per object (no transform: the
//! positions are placed already), each with its own mesh of one primitive per part. Every
//! primitive has positions, normals ([`super::vertex_normals`]), UVs when the part has them, and
//! 32-bit indices. Looks become metallic-roughness materials, see-through ones blended (or cut
//! out at their alpha cutoff); their images are PNGs in the buffer, one texture each, repeating.
//!
//! Space: glTF is right-handed, +Y up, +Z forward, metres; from the editor's (X, Y, Z) that is
//! `(-Y, Z, X) / 100`, the reverse of [`crate::space::to_unreal`] with its default settings. The
//! conversion is a reflection, so the corners keep their order (see [`crate::space`]).

use std::path::Path;

use serde_json::{Value, json};

use super::{Export, Look, vertex_normals};
use crate::model::Part;

const FLOAT: u32 = 5126;
const UNSIGNED_INT: u32 = 5125;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
const LINEAR: u32 = 9729;
const LINEAR_MIPMAP_LINEAR: u32 = 9987;
const REPEAT: u32 = 10497;
const JSON_CHUNK: u32 = 0x4E4F_534A;
const BIN_CHUNK: u32 = 0x004E_4942;

/// `position` (the editor's space, cm) in glTF's space (m).
fn to_gltf(position: [f32; 3]) -> [f32; 3] {
    [-position[1], position[2], position[0]].map(|axis| axis / 100.0)
}

/// `normal` (the editor's space) in glTF's space.
fn normal_to_gltf(normal: [f32; 3]) -> [f32; 3] {
    [-normal[1], normal[2], normal[0]]
}

/// Writes `export` to `path` as a `.glb`.
pub fn save(export: &Export, path: &Path) -> Result<(), String> {
    let bytes = write(export)?;
    std::fs::write(path, bytes).map_err(|error| format!("{}: {error}", path.display()))
}

/// `export` as the bytes of a `.glb`.
pub fn write(export: &Export) -> Result<Vec<u8>, String> {
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
    let mut document = Document::default();
    let mut children = Vec::new();
    for object in &export.objects {
        let primitives: Vec<Value> = object
            .parts
            .iter()
            .filter(|part| !part.corners.is_empty())
            .map(|part| document.primitive(part, export.looks.len()))
            .collect::<Result<_, _>>()?;
        if primitives.is_empty() {
            continue;
        }
        document.meshes.push(json!({
            "name": object.name,
            "primitives": primitives,
        }));
        document.nodes.push(json!({
            "name": object.name,
            "mesh": document.meshes.len() - 1,
        }));
        children.push(document.nodes.len() - 1);
    }
    let root = document.nodes.len();
    let mut root_node = json!({ "name": export.name });
    if !children.is_empty() {
        root_node["children"] = json!(children);
    }
    document.nodes.push(root_node);

    let mut json = json!({
        "asset": { "version": "2.0", "generator": "Brick Rigs editortweaks" },
        "scene": 0,
        "scenes": [{ "name": export.name, "nodes": [root] }],
        "nodes": document.nodes,
        "materials": export.looks.iter().map(material).collect::<Vec<_>>(),
    });
    if !export.images.is_empty() {
        let mut images = Vec::with_capacity(export.images.len());
        for image in &export.images {
            let png = image.to_png()?;
            images
                .push(json!({ "bufferView": document.view(&png, None), "mimeType": "image/png" }));
        }
        json["images"] = json!(images);
        json["textures"] = json!(
            (0..export.images.len())
                .map(|image| json!({ "sampler": 0, "source": image }))
                .collect::<Vec<_>>()
        );
        json["samplers"] = json!([{
            "magFilter": LINEAR,
            "minFilter": LINEAR_MIPMAP_LINEAR,
            "wrapS": REPEAT,
            "wrapT": REPEAT,
        }]);
    }
    if !document.views.is_empty() {
        json["bufferViews"] = json!(document.views);
        json["buffers"] = json!([{ "byteLength": document.buffer.len() }]);
    }
    if !document.meshes.is_empty() {
        json["meshes"] = json!(document.meshes);
        json["accessors"] = json!(document.accessors);
    }
    let json = serde_json::to_vec(&json).map_err(|error| error.to_string())?;
    container(json, document.buffer)
}

/// The glTF material of `look`.
fn material(look: &Look) -> Value {
    let mut material = json!({
        "name": look.name,
        "pbrMetallicRoughness": {
            "baseColorFactor": look.color.map(|channel| channel.clamp(0.0, 1.0)),
            "metallicFactor": look.metallic.clamp(0.0, 1.0),
            "roughnessFactor": look.roughness.clamp(0.0, 1.0),
        },
    });
    if look.emissive.iter().any(|&channel| channel > 0.0) {
        material["emissiveFactor"] = json!(look.emissive.map(|channel| channel.clamp(0.0, 1.0)));
    }
    let textures = &look.textures;
    let pbr = &mut material["pbrMetallicRoughness"];
    if let Some(image) = textures.base_color {
        pbr["baseColorTexture"] = json!({ "index": image });
    }
    if let Some(image) = textures.metallic_roughness {
        pbr["metallicRoughnessTexture"] = json!({ "index": image });
    }
    for (slot, image) in [
        ("normalTexture", textures.normal),
        ("occlusionTexture", textures.occlusion),
        ("emissiveTexture", textures.emissive),
    ] {
        if let Some(image) = image {
            material[slot] = json!({ "index": image });
        }
    }
    if textures.emissive.is_some() && look.emissive.iter().all(|&channel| channel <= 0.0) {
        // The texture alone gives the light.
        material["emissiveFactor"] = json!([1.0, 1.0, 1.0]);
    }
    if look.double_sided {
        material["doubleSided"] = json!(true);
    }
    if let Some(cutoff) = look.alpha_cutoff {
        material["alphaMode"] = json!("MASK");
        material["alphaCutoff"] = json!(cutoff);
    } else if look.color[3] < 1.0 {
        material["alphaMode"] = json!("BLEND");
    }
    material
}

/// The parts of the document built so far, and the buffer they point into.
#[derive(Default)]
struct Document {
    nodes: Vec<Value>,
    meshes: Vec<Value>,
    accessors: Vec<Value>,
    views: Vec<Value>,
    buffer: Vec<u8>,
}

impl Document {
    /// The primitive drawing `part`, its data added to the buffer. `looks` is how many materials
    /// there are.
    fn primitive(&mut self, part: &Part, looks: usize) -> Result<Value, String> {
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
        let positions: Vec<[f32; 3]> = part.positions.iter().copied().map(to_gltf).collect();
        let normals: Vec<[f32; 3]> = vertex_normals(part)
            .into_iter()
            .map(normal_to_gltf)
            .collect();

        let (min, max) = bounds(&positions);
        let position = self.accessor(&floats(&positions), ARRAY_BUFFER, count, FLOAT, "VEC3");
        self.accessors[position]["min"] = json!(min);
        self.accessors[position]["max"] = json!(max);
        let normal = self.accessor(&floats(&normals), ARRAY_BUFFER, count, FLOAT, "VEC3");
        let mut attributes = json!({ "POSITION": position, "NORMAL": normal });
        if !part.uvs.is_empty() {
            attributes["TEXCOORD_0"] =
                json!(self.accessor(&floats(&part.uvs), ARRAY_BUFFER, count, FLOAT, "VEC2"));
        }
        let indices: Vec<u8> = part
            .corners
            .iter()
            .flatten()
            .flat_map(|index| index.to_le_bytes())
            .collect();
        let indices = self.accessor(
            &indices,
            ELEMENT_ARRAY_BUFFER,
            part.corners.len() * 3,
            UNSIGNED_INT,
            "SCALAR",
        );

        let mut primitive = json!({ "attributes": attributes, "indices": indices });
        if let Some(material) = part.material.filter(|&material| material < looks) {
            primitive["material"] = json!(material);
        }
        Ok(primitive)
    }

    /// Adds `data` to the buffer as a view of its own with one accessor over it, and returns the
    /// accessor's index.
    fn accessor(
        &mut self,
        data: &[u8],
        target: u32,
        count: usize,
        component_type: u32,
        kind: &str,
    ) -> usize {
        let view = self.view(data, Some(target));
        self.accessors.push(json!({
            "bufferView": view,
            "componentType": component_type,
            "count": count,
            "type": kind,
        }));
        self.accessors.len() - 1
    }

    /// Adds `data` to the buffer as a view of its own (for vertices or indices: `target`), and
    /// returns the view's index.
    fn view(&mut self, data: &[u8], target: Option<u32>) -> usize {
        // Every view starts on a 4-byte boundary, as floats and 32-bit indices need.
        let offset = self.buffer.len();
        self.buffer.extend_from_slice(data);
        self.buffer.resize(self.buffer.len().next_multiple_of(4), 0);
        let mut view = json!({
            "buffer": 0,
            "byteOffset": offset,
            "byteLength": data.len(),
        });
        if let Some(target) = target {
            view["target"] = json!(target);
        }
        self.views.push(view);
        self.views.len() - 1
    }
}

/// The little-endian bytes of `values`.
fn floats<const N: usize>(values: &[[f32; N]]) -> Vec<u8> {
    values
        .iter()
        .flatten()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// The smallest and largest of each coordinate (glTF requires them for positions).
fn bounds(positions: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for position in positions {
        for axis in 0..3 {
            min[axis] = min[axis].min(position[axis]);
            max[axis] = max[axis].max(position[axis]);
        }
    }
    (min, max)
}

/// The `.glb` file: a 12-byte header, the JSON chunk (padded with spaces) and the binary chunk
/// (padded with zeros).
fn container(mut json: Vec<u8>, mut binary: Vec<u8>) -> Result<Vec<u8>, String> {
    json.resize(json.len().next_multiple_of(4), b' ');
    binary.resize(binary.len().next_multiple_of(4), 0);
    let has_binary = !binary.is_empty();
    let total = 12 + 8 + json.len() + if has_binary { 8 + binary.len() } else { 0 };
    let too_big = || "the model is too big for a .glb (4 GB)".to_string();
    let total = u32::try_from(total).map_err(|_| too_big())?;

    let mut out = Vec::with_capacity(total as usize);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&total.to_le_bytes());
    for (kind, data) in [(JSON_CHUNK, &json), (BIN_CHUNK, &binary)] {
        if kind == BIN_CHUNK && !has_binary {
            continue;
        }
        let length = u32::try_from(data.len()).map_err(|_| too_big())?;
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(data);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::Object;
    use crate::readers::gltf;
    use crate::space::{ImportSettings, to_unreal};

    fn export() -> Export {
        let mut export = Export {
            name: "Test vehicle".into(),
            ..Export::default()
        };
        let red = export.look_index(Look {
            name: "red".into(),
            color: [1.0, 0.0, 0.0, 1.0],
            ..Look::default()
        });
        let glass = export.look_index(Look {
            name: "glass".into(),
            color: [0.8, 0.9, 1.0, 0.3],
            ..Look::default()
        });
        let part = |material, offset: f32| Part {
            name: String::new(),
            material: Some(material),
            positions: vec![
                [offset, 0.0, 0.0],
                [offset, 10.0, 0.0],
                [offset, 10.0, 20.0],
            ],
            uvs: vec![],
            corners: vec![[0, 1, 2]],
        };
        export.objects = vec![
            Object {
                name: "Brick 1".into(),
                parts: vec![part(red, 0.0), part(glass, 50.0)],
            },
            Object {
                name: "Empty".into(),
                parts: vec![],
            },
        ];
        export
    }

    #[test]
    fn reads_back_in_the_editors_space() {
        let export = export();
        let bytes = write(&export).unwrap();
        assert_eq!(&bytes[..4], b"glTF");
        assert_eq!(bytes.len() % 4, 0);
        let model = to_unreal(
            gltf::from_slice(&bytes).unwrap(),
            &ImportSettings::default(),
        )
        .unwrap();
        assert_eq!(model.parts.len(), 2);
        for (read, written) in model.parts.iter().zip(&export.objects[0].parts) {
            assert_eq!(read.corners, written.corners);
            for (a, b) in read.positions.iter().zip(&written.positions) {
                for axis in 0..3 {
                    assert!((a[axis] - b[axis]).abs() < 1e-4, "{a:?} vs {b:?}");
                }
            }
        }
        assert_eq!(model.materials.len(), 2);
        assert_eq!(model.materials[1].color, [0.8, 0.9, 1.0, 0.3]);
    }

    #[test]
    fn document_shape() {
        let bytes = write(&export()).unwrap();
        let length = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let document: Value = serde_json::from_slice(&bytes[20..20 + length]).unwrap();
        // The empty object gets no node: two nodes, the brick and the root holding it.
        assert_eq!(document["nodes"].as_array().unwrap().len(), 2);
        assert_eq!(document["nodes"][1]["children"], json!([0]));
        assert_eq!(document["scenes"][0]["nodes"], json!([1]));
        assert_eq!(document["materials"][1]["alphaMode"], json!("BLEND"));
        assert!(document["materials"][0].get("alphaMode").is_none());
        let primitive = &document["meshes"][0]["primitives"][0];
        assert!(primitive["attributes"].get("NORMAL").is_some());
        assert!(primitive["attributes"].get("TEXCOORD_0").is_none());
    }

    #[test]
    fn textures_are_embedded() {
        let mut export = export();
        let image = crate::image::Image {
            width: 1,
            height: 1,
            rgba: vec![255, 128, 0, 255],
        };
        let png = image.to_png().unwrap();
        export.images.push(image);
        export.looks[0].textures.base_color = Some(0);
        export.looks[1].alpha_cutoff = Some(0.5);
        let bytes = write(&export).unwrap();
        let length = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let document: Value = serde_json::from_slice(&bytes[20..20 + length]).unwrap();
        let material = &document["materials"][0]["pbrMetallicRoughness"];
        assert_eq!(material["baseColorTexture"]["index"], json!(0));
        assert_eq!(document["materials"][1]["alphaMode"], json!("MASK"));
        assert_eq!(document["textures"][0]["source"], json!(0));
        let view = &document["bufferViews"]
            [document["images"][0]["bufferView"].as_u64().unwrap() as usize];
        assert!(view.get("target").is_none());
        let start = 20 + length + 8 + view["byteOffset"].as_u64().unwrap() as usize;
        assert_eq!(&bytes[start..start + png.len()], &png[..]);

        export.looks[0].textures.normal = Some(3);
        assert!(write(&export).is_err());
    }

    #[test]
    fn nothing_to_write_is_still_a_file() {
        let bytes = write(&Export::default()).unwrap();
        assert_eq!(&bytes[..4], b"glTF");
        // A model without triangles reads as an error, not a crash.
        assert!(gltf::from_slice(&bytes).is_err());
    }

    #[test]
    fn broken_parts_are_refused() {
        let mut export = export();
        export.objects[0].parts[0].corners = vec![[0, 1, 7]];
        assert!(write(&export).is_err());
    }
}
