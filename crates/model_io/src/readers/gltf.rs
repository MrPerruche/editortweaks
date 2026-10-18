//! glTF 2.0 (`.gltf` with its `.bin` and images, or `.glb`), read by hand on top of `serde_json`.
//!
//! The format in short: a JSON document lists `buffers` (raw bytes), `bufferViews` (slices of a
//! buffer), `accessors` (a view read as typed elements: floats, integers, 2 or 3 per element) and
//! the scene's `nodes`, which point at `meshes`. A `.glb` is the same JSON and the first buffer
//! in one file: a 12-byte header (`glTF`, version 2, total length) and then chunks of
//! `length, type, data`, each padded to 4 bytes. A buffer's bytes come from that GLB chunk, a
//! `data:` URI (base64) or a file next to the `.gltf`.
//!
//! Gotchas: a glTF is a tree of nodes, each with its own transform, and one mesh can be used by
//! several nodes. So the scene is walked and every mesh primitive is baked into a part in world
//! space. A node with a mirroring transform (a negative scale) turns triangles inside out, so
//! their corners are swapped to keep the fronts counter-clockwise. Material indices are glTF's
//! own, and UVs already have V pointing down, like the engine's. A primitive with a `NORMAL`
//! attribute is trusted to have its vertices split at hard edges; one without gets them split by
//! angle ([`Part::split_sharp_edges`]), since the game derives its shading from shared vertices.
//! (The spec says to shade such a primitive flat; the angle keeps curved surfaces smooth.) Files
//! are untrusted: every read is bounds-checked and file names may not leave the model's folder.
//!
//! Not read: sparse accessors, extensions (Draco, meshopt, KTX2/WebP textures), skins, animation.

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::image::{self, Image};
use crate::model::{MAX_TRIANGLES, Material, Model, Part, SMOOTH_ANGLE};

/// A node tree deeper than this is taken for a loop (the spec forbids them, files may not obey).
const MAX_DEPTH: usize = 64;

/// More nodes visited than this (a node can be the child of several others) is taken for a trap.
const MAX_VISITS: usize = 1_000_000;

/// A column-major 4x4 matrix, as glTF writes them: `m[column][row]`.
type Mat4 = [[f32; 4]; 4];

const IDENTITY: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// glTF's `componentType` numbers.
const BYTE: u32 = 5120;
const UNSIGNED_BYTE: u32 = 5121;
const SHORT: u32 = 5122;
const UNSIGNED_SHORT: u32 = 5123;
const UNSIGNED_INT: u32 = 5125;
const FLOAT: u32 = 5126;

/// The primitive `mode` of a plain triangle list (the default).
const TRIANGLES: u32 = 4;

/// GLB chunk types: "JSON" and "BIN\0" as little-endian numbers.
const JSON_CHUNK: u32 = 0x4E4F_534A;
const BIN_CHUNK: u32 = 0x004E_4942;

// --- The JSON document: only the fields we use, everything else is ignored. ---

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Document {
    #[serde(default)]
    scene: Option<usize>,
    #[serde(default)]
    scenes: Vec<Scene>,
    #[serde(default)]
    nodes: Vec<Node>,
    #[serde(default)]
    meshes: Vec<Mesh>,
    #[serde(default)]
    accessors: Vec<Accessor>,
    #[serde(default)]
    buffer_views: Vec<BufferView>,
    #[serde(default)]
    buffers: Vec<BufferInfo>,
    #[serde(default)]
    materials: Vec<MaterialInfo>,
    #[serde(default)]
    textures: Vec<TextureInfo>,
    #[serde(default)]
    images: Vec<ImageInfo>,
}

#[derive(Deserialize, Default)]
struct Scene {
    #[serde(default)]
    nodes: Vec<usize>,
}

#[derive(Deserialize)]
struct Node {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    children: Vec<usize>,
    #[serde(default)]
    mesh: Option<usize>,
    /// Column-major, instead of the three below.
    #[serde(default)]
    matrix: Option<[f32; 16]>,
    #[serde(default)]
    translation: Option<[f32; 3]>,
    /// A quaternion, `[x, y, z, w]`.
    #[serde(default)]
    rotation: Option<[f32; 4]>,
    #[serde(default)]
    scale: Option<[f32; 3]>,
}

#[derive(Deserialize)]
struct Mesh {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    primitives: Vec<Primitive>,
}

#[derive(Deserialize)]
struct Primitive {
    #[serde(default)]
    attributes: Attributes,
    #[serde(default)]
    indices: Option<usize>,
    #[serde(default)]
    material: Option<usize>,
    #[serde(default = "default_mode")]
    mode: u32,
}

fn default_mode() -> u32 {
    TRIANGLES
}

/// Accessor indices of the vertex attributes we read.
#[derive(Deserialize, Default)]
struct Attributes {
    #[serde(rename = "POSITION", default)]
    position: Option<usize>,
    #[serde(rename = "TEXCOORD_0", default)]
    texcoord_0: Option<usize>,
    /// Not read, only whether there is one (see [`Part::split_sharp_edges`]).
    #[serde(rename = "NORMAL", default)]
    normal: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Accessor {
    /// Without one the spec means all zeros; we don't read those.
    #[serde(default)]
    buffer_view: Option<usize>,
    #[serde(default)]
    byte_offset: usize,
    component_type: u32,
    #[serde(default)]
    normalized: bool,
    count: usize,
    /// `SCALAR`, `VEC2`, `VEC3` ...
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    sparse: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BufferView {
    buffer: usize,
    #[serde(default)]
    byte_offset: usize,
    byte_length: usize,
    /// Distance between elements; 0 or absent means tightly packed.
    #[serde(default)]
    byte_stride: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BufferInfo {
    #[serde(default)]
    uri: Option<String>,
    #[serde(default)]
    byte_length: Option<usize>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct MaterialInfo {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    pbr_metallic_roughness: PbrInfo,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PbrInfo {
    #[serde(default = "white")]
    base_color_factor: [f32; 4],
    #[serde(default)]
    base_color_texture: Option<TextureRef>,
}

impl Default for PbrInfo {
    fn default() -> Self {
        Self {
            base_color_factor: white(),
            base_color_texture: None,
        }
    }
}

fn white() -> [f32; 4] {
    [1.0; 4]
}

#[derive(Deserialize)]
struct TextureRef {
    index: usize,
}

#[derive(Deserialize)]
struct TextureInfo {
    #[serde(default)]
    source: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImageInfo {
    #[serde(default)]
    uri: Option<String>,
    #[serde(default)]
    buffer_view: Option<usize>,
}

// --- Reading ---

/// Reads the `.gltf` or `.glb` file at `path`, with the files it points to.
pub fn read(path: &Path) -> Result<Model, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let folder = path.parent().unwrap_or(Path::new(""));
    let gltf = Gltf::parse(&bytes, Some(folder))?;
    convert(&gltf)
}

/// Reads a self-contained glTF (a `.glb`, or JSON with embedded data) from memory.
pub fn from_slice(bytes: &[u8]) -> Result<Model, String> {
    let gltf = Gltf::parse(bytes, None)?;
    convert(&gltf)
}

/// A parsed glTF: the document and the bytes of all its buffers.
struct Gltf<'a> {
    document: Document,
    buffers: Vec<Vec<u8>>,
    /// Where relative file names are looked up; `None` when reading from memory.
    folder: Option<&'a Path>,
}

impl<'a> Gltf<'a> {
    /// Parses `.glb` or `.gltf` contents and loads the buffers.
    fn parse(bytes: &[u8], folder: Option<&'a Path>) -> Result<Self, String> {
        let (json, glb_buffer) = if bytes.starts_with(b"glTF") {
            let (json, buffer) = split_glb(bytes)?;
            (json, buffer)
        } else {
            let json = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
            (json, None)
        };
        let document: Document =
            serde_json::from_slice(json).map_err(|error| format!("not a glTF file: {error}"))?;
        let buffers = load_buffers(&document, glb_buffer, folder)?;
        Ok(Self {
            document,
            buffers,
            folder,
        })
    }

    /// The bytes of buffer view `index`.
    fn view_bytes(&self, index: usize) -> Result<&[u8], String> {
        let view = self
            .document
            .buffer_views
            .get(index)
            .ok_or_else(|| format!("buffer view {index} doesn't exist"))?;
        let buffer = self
            .buffers
            .get(view.buffer)
            .ok_or_else(|| format!("buffer {} doesn't exist", view.buffer))?;
        let end = view.byte_offset.checked_add(view.byte_length);
        end.and_then(|end| buffer.get(view.byte_offset..end))
            .ok_or_else(|| format!("buffer view {index} reaches past its buffer"))
    }

    /// The bytes of each element of accessor `index`, which must be of type `kind`. All bounds
    /// are checked here, so the caller can slice every element freely.
    fn elements(&self, index: usize, kind: &str) -> Result<impl Iterator<Item = &[u8]>, String> {
        let accessor = self
            .document
            .accessors
            .get(index)
            .ok_or_else(|| format!("accessor {index} doesn't exist"))?;
        if accessor.sparse.is_some() {
            return Err("sparse accessors aren't supported".into());
        }
        if accessor.kind != kind {
            return Err(format!(
                "accessor {index} holds {} where {kind} is needed",
                accessor.kind
            ));
        }
        let view = accessor
            .buffer_view
            .ok_or("accessors without a buffer view aren't supported")?;
        let components = match kind {
            "SCALAR" => 1,
            "VEC2" => 2,
            _ => 3,
        };
        let component_size = component_size(accessor.component_type)
            .ok_or_else(|| format!("accessor {index}: unsupported component type"))?;
        let size = components * component_size;

        let stride = match self.document.buffer_views.get(view) {
            Some(view) if view.byte_stride != 0 => view.byte_stride,
            _ => size,
        };
        let data = self
            .view_bytes(view)?
            .get(accessor.byte_offset..)
            .ok_or_else(|| format!("accessor {index} starts past its buffer view"))?;
        // The last element must fit; the count is checked against the data, not trusted.
        let needed = accessor
            .count
            .checked_sub(1)
            .map_or(Some(0), |last| last.checked_mul(stride)?.checked_add(size));
        if stride < size || needed.is_none_or(|needed| needed > data.len()) {
            return Err(format!("accessor {index} reaches past its buffer view"));
        }
        Ok((0..accessor.count).map(move |i| &data[i * stride..i * stride + size]))
    }

    /// Positions (accessor `index`, floats).
    fn read_positions(&self, index: usize) -> Result<Vec<[f32; 3]>, String> {
        if self.component_type(index)? != FLOAT {
            return Err("positions must be floats".into());
        }
        Ok(self.elements(index, "VEC3")?.map(floats).collect())
    }

    /// Texture coordinates (accessor `index`): floats, or normalized bytes or shorts.
    fn read_uvs(&self, index: usize) -> Result<Vec<[f32; 2]>, String> {
        let component_type = self.component_type(index)?;
        let normalized = self.document.accessors[index].normalized;
        let component: fn(&[u8], usize) -> f32 = match (component_type, normalized) {
            (FLOAT, _) => |element, at| float_at(element, at),
            (UNSIGNED_BYTE, true) => |element, at| f32::from(element[at]) / 255.0,
            (UNSIGNED_SHORT, true) => |element, at| f32::from(u16_at(element, at)) / 65535.0,
            _ => {
                return Err(
                    "texture coordinates must be floats or normalized bytes or shorts".into(),
                );
            }
        };
        Ok(self
            .elements(index, "VEC2")?
            .map(|element| [component(element, 0), component(element, 1)])
            .collect())
    }

    /// Triangle corner indices (accessor `index`, unsigned bytes, shorts or ints).
    fn read_indices(&self, index: usize) -> Result<Vec<u32>, String> {
        let read: fn(&[u8]) -> u32 = match self.component_type(index)? {
            UNSIGNED_BYTE => |element| u32::from(element[0]),
            UNSIGNED_SHORT => |element| u32::from(u16_at(element, 0)),
            UNSIGNED_INT => {
                |element| u32::from_le_bytes([element[0], element[1], element[2], element[3]])
            }
            _ => return Err("indices must be unsigned bytes, shorts or ints".into()),
        };
        Ok(self.elements(index, "SCALAR")?.map(read).collect())
    }

    /// The component type of accessor `index`.
    fn component_type(&self, index: usize) -> Result<u32, String> {
        self.document
            .accessors
            .get(index)
            .map(|accessor| accessor.component_type)
            .ok_or_else(|| format!("accessor {index} doesn't exist"))
    }
}

/// Bytes per component of a `componentType` we can read.
fn component_size(component_type: u32) -> Option<usize> {
    match component_type {
        BYTE | UNSIGNED_BYTE => Some(1),
        SHORT | UNSIGNED_SHORT => Some(2),
        UNSIGNED_INT | FLOAT => Some(4),
        _ => None,
    }
}

/// Component `at` of an element, as a little-endian float.
fn float_at(element: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(std::array::from_fn(|byte| element[at * 4 + byte]))
}

/// Component `at` of an element, as a little-endian unsigned short.
fn u16_at(element: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([element[at * 2], element[at * 2 + 1]])
}

/// All `N` components of an element, as floats.
fn floats<const N: usize>(element: &[u8]) -> [f32; N] {
    std::array::from_fn(|at| float_at(element, at))
}

/// The little-endian number at byte `at` of `bytes`, if it is all there.
fn number_at(bytes: &[u8], at: usize) -> Option<u32> {
    let word = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
}

/// The JSON and the optional binary chunk of a `.glb`.
fn split_glb(bytes: &[u8]) -> Result<(&[u8], Option<&[u8]>), String> {
    const TRUNCATED: &str = "the .glb is truncated";
    let version = number_at(bytes, 4).ok_or(TRUNCATED)?;
    if version != 2 {
        return Err(format!("glTF version {version} isn't supported"));
    }
    let total = number_at(bytes, 8).ok_or(TRUNCATED)? as usize;
    let mut rest = bytes.get(12..total).ok_or(TRUNCATED)?;

    let (mut json, mut binary) = (None, None);
    while !rest.is_empty() {
        let length = number_at(rest, 0).ok_or(TRUNCATED)? as usize;
        let kind = number_at(rest, 4).ok_or(TRUNCATED)?;
        let end = length.checked_add(8).ok_or(TRUNCATED)?;
        let data = rest.get(8..end).ok_or(TRUNCATED)?;
        match kind {
            JSON_CHUNK if json.is_none() => json = Some(data),
            BIN_CHUNK if binary.is_none() => binary = Some(data),
            _ => {}
        }
        // Chunks start on 4-byte boundaries.
        rest = rest.get(end.next_multiple_of(4)..).unwrap_or_default();
    }
    Ok((json.ok_or("the .glb has no JSON chunk")?, binary))
}

/// The bytes of every buffer: the GLB chunk for a first buffer without a URI, else the URI's data.
fn load_buffers(
    document: &Document,
    glb_buffer: Option<&[u8]>,
    folder: Option<&Path>,
) -> Result<Vec<Vec<u8>>, String> {
    let mut buffers = Vec::with_capacity(document.buffers.len());
    for (index, info) in document.buffers.iter().enumerate() {
        let mut bytes = match (&info.uri, glb_buffer) {
            (Some(uri), _) => load_uri(uri, folder)?,
            (None, Some(chunk)) if index == 0 => chunk.to_vec(),
            (None, _) => return Err(format!("buffer {index} has no data")),
        };
        if let Some(length) = info.byte_length {
            if bytes.len() < length {
                return Err(format!("buffer {index} is shorter than it says"));
            }
            // A GLB chunk is padded to 4 bytes.
            bytes.truncate(length);
        }
        buffers.push(bytes);
    }
    Ok(buffers)
}

/// The bytes a URI points to: a base64 `data:` URI, or a file in `folder`.
fn load_uri(uri: &str, folder: Option<&Path>) -> Result<Vec<u8>, String> {
    if let Some(data) = uri.strip_prefix("data:") {
        let (header, payload) = data.split_once(',').ok_or("a data URI has no data")?;
        if !header.ends_with(";base64") {
            return Err("only base64 data URIs are supported".into());
        }
        return decode_base64(payload);
    }
    let folder = folder.ok_or_else(|| format!("{uri}: files can't be read from memory"))?;
    let path = folder.join(relative_path(uri)?);
    std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))
}

/// The file path a relative URI names. URIs are percent-encoded (`%20` for a space); they may
/// not be absolute, have a scheme, or leave the folder with `..`, since files are untrusted.
fn relative_path(uri: &str) -> Result<PathBuf, String> {
    let name = percent_decode(uri)?;
    let path = PathBuf::from(&name);
    let stays_inside = path
        .components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
    if name.contains(':') || !stays_inside {
        return Err(format!("{uri}: only files next to the model are read"));
    }
    Ok(path)
}

/// `text` with every `%XX` replaced by the byte it names.
pub(crate) fn percent_decode(text: &str) -> Result<String, String> {
    let bad = || format!("{text}: bad percent-encoding");
    let mut out = Vec::with_capacity(text.len());
    let mut bytes = text.bytes();
    while let Some(byte) = bytes.next() {
        if byte != b'%' {
            out.push(byte);
            continue;
        }
        let digit = |byte: Option<u8>| char::from(byte?).to_digit(16);
        let high = digit(bytes.next()).ok_or_else(bad)?;
        let low = digit(bytes.next()).ok_or_else(bad)?;
        out.push((high * 16 + low) as u8);
    }
    String::from_utf8(out).map_err(|_| bad())
}

/// Standard base64 (`A-Z a-z 0-9 + /`), padding with `=` optional. Anything else, whitespace
/// included, is an error.
fn decode_base64(text: &str) -> Result<Vec<u8>, String> {
    let data = text.trim_end_matches('=');
    if text.len() - data.len() > 2 || data.len() % 4 == 1 {
        return Err("bad base64 data".into());
    }
    let mut out = Vec::with_capacity(data.len() * 3 / 4);
    // Each character gives 6 bits; `bits` holds the ones not yet written as whole bytes.
    let (mut bits, mut count) = (0u32, 0);
    for byte in data.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err("bad base64 data".into()),
        };
        bits = bits << 6 | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    Ok(out)
}

// --- From the document to a model ---

/// Builds the [`Model`]: the scene's meshes become parts.
fn convert(gltf: &Gltf) -> Result<Model, String> {
    let document = &gltf.document;
    let materials = document
        .materials
        .iter()
        .map(|material| convert_material(material, gltf))
        .collect();

    let mut walker = Walker {
        gltf,
        parts: Vec::new(),
        triangles: 0,
        visits: 0,
    };
    for node in root_nodes(document) {
        walker.visit(node, &IDENTITY, 0)?;
    }
    if walker.parts.is_empty() {
        return Err("no triangles".into());
    }
    Ok(Model {
        parts: walker.parts,
        materials,
    })
}

/// The nodes to start from: the default scene's, else the first scene's, else every node that no
/// other node has as a child.
fn root_nodes(document: &Document) -> Vec<usize> {
    let scene = document
        .scene
        .and_then(|index| document.scenes.get(index))
        .or_else(|| document.scenes.first());
    match scene {
        Some(scene) => scene.nodes.clone(),
        None => (0..document.nodes.len())
            .filter(|&index| {
                !document
                    .nodes
                    .iter()
                    .any(|node| node.children.contains(&index))
            })
            .collect(),
    }
}

/// The glTF material's base colour and base colour texture.
fn convert_material(material: &MaterialInfo, gltf: &Gltf) -> Material {
    let pbr = &material.pbr_metallic_roughness;
    // A texture that can't be read leaves the material plain: the model is still usable.
    let texture = pbr
        .base_color_texture
        .as_ref()
        .and_then(|reference| load_texture(reference.index, gltf).ok());
    Material {
        name: material.name.clone().unwrap_or_default(),
        color: pbr.base_color_factor,
        texture,
    }
}

/// Decodes texture `index`: its image, from a URI (data or file) or a buffer view.
fn load_texture(index: usize, gltf: &Gltf) -> Result<Image, String> {
    let document = &gltf.document;
    let source = document
        .textures
        .get(index)
        .and_then(|texture| texture.source)
        .ok_or("the texture has no image")?;
    let info = document
        .images
        .get(source)
        .ok_or("the image doesn't exist")?;
    match (&info.uri, info.buffer_view) {
        (Some(uri), _) => image::decode(&load_uri(uri, gltf.folder)?),
        (None, Some(view)) => image::decode(gltf.view_bytes(view)?),
        (None, None) => Err("the image has no data".into()),
    }
}

/// Walks the node tree, collecting one part per triangle primitive.
struct Walker<'a> {
    gltf: &'a Gltf<'a>,
    parts: Vec<Part>,
    triangles: usize,
    visits: usize,
}

impl Walker<'_> {
    /// Adds node `index`'s mesh and then its children, all placed by `parent` (the parent's world
    /// matrix) times the node's own.
    fn visit(&mut self, index: usize, parent: &Mat4, depth: usize) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("the nodes nest too deeply (a loop?)".into());
        }
        self.visits += 1;
        if self.visits > MAX_VISITS {
            return Err("the nodes are visited too often (a loop?)".into());
        }
        let document = &self.gltf.document;
        let node = document
            .nodes
            .get(index)
            .ok_or_else(|| format!("node {index} doesn't exist"))?;
        let world = multiply(parent, &node_matrix(node));

        if let Some(mesh) = node.mesh {
            let mesh = document
                .meshes
                .get(mesh)
                .ok_or_else(|| format!("mesh {mesh} doesn't exist"))?;
            let name = mesh
                .name
                .as_deref()
                .or(node.name.as_deref())
                .unwrap_or("mesh");
            for primitive in &mesh.primitives {
                self.add_primitive(primitive, name, &world)?;
            }
        }
        node.children
            .iter()
            .try_for_each(|&child| self.visit(child, &world, depth + 1))
    }

    /// Adds a part for `primitive`, placed by `world`. Anything but a plain triangle list
    /// (strips and fans are rare, points and lines are no surfaces) is skipped, and so is a
    /// primitive without positions.
    fn add_primitive(
        &mut self,
        primitive: &Primitive,
        name: &str,
        world: &Mat4,
    ) -> Result<(), String> {
        let gltf = self.gltf;
        let Some(position_accessor) = primitive.attributes.position else {
            return Ok(());
        };
        if primitive.mode != TRIANGLES {
            return Ok(());
        }
        let mut positions = gltf.read_positions(position_accessor)?;
        for position in &mut positions {
            *position = transform_point(world, *position);
        }
        let indices = match primitive.indices {
            Some(accessor) => gltf.read_indices(accessor)?,
            None => (0..positions.len() as u32).collect(),
        };
        let mut corners: Vec<[u32; 3]> = indices.as_chunks::<3>().0.to_vec();
        if corners
            .iter()
            .flatten()
            .any(|&i| i as usize >= positions.len())
        {
            return Err("a triangle points past its positions".into());
        }

        self.triangles += corners.len();
        if self.triangles > MAX_TRIANGLES {
            return Err(format!(
                "more than the {MAX_TRIANGLES} triangles a reference may have"
            ));
        }
        if is_mirroring(world) {
            for corner in &mut corners {
                corner.swap(1, 2);
            }
        }

        let uvs = match primitive.attributes.texcoord_0 {
            Some(accessor) => gltf.read_uvs(accessor)?,
            None => Vec::new(),
        };
        // UVs that don't match the positions are of no use.
        let uvs = if uvs.len() == positions.len() {
            uvs
        } else {
            Vec::new()
        };

        let mut part = Part {
            name: name.to_string(),
            material: primitive.material,
            positions,
            uvs,
            corners,
        };
        // Exporters split the vertices at the hard edges they author, but only when they write
        // normals. The spec says to shade a primitive without them flat; sharp edges by angle
        // keep its curved surfaces smooth.
        if primitive.attributes.normal.is_none() {
            part.split_sharp_edges(SMOOTH_ANGLE);
        }
        self.parts.push(part);
        Ok(())
    }
}

/// The node's local transform: its `matrix`, or translation * rotation * scale.
fn node_matrix(node: &Node) -> Mat4 {
    if let Some(matrix) = &node.matrix {
        let columns = matrix.as_chunks::<4>().0;
        return std::array::from_fn(|column| columns[column]);
    }
    let translation = node.translation.unwrap_or([0.0; 3]);
    let scale = node.scale.unwrap_or([1.0; 3]);
    let mut matrix = rotation_matrix(node.rotation.unwrap_or([0.0, 0.0, 0.0, 1.0]));
    // Scale applies first, so it scales the rotation's columns.
    for (column, factor) in matrix.iter_mut().zip(scale) {
        column.iter_mut().for_each(|value| *value *= factor);
    }
    matrix[3] = [translation[0], translation[1], translation[2], 1.0];
    matrix
}

/// The rotation matrix of the quaternion `[x, y, z, w]` (normalized first; a zero one is no
/// rotation).
fn rotation_matrix([x, y, z, w]: [f32; 4]) -> Mat4 {
    let length = (x * x + y * y + z * z + w * w).sqrt();
    if length == 0.0 || !length.is_finite() {
        return IDENTITY;
    }
    let [x, y, z, w] = [x, y, z, w].map(|value| value / length);
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + z * w),
            2.0 * (x * z - y * w),
            0.0,
        ],
        [
            2.0 * (x * y - z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z + x * w),
            0.0,
        ],
        [
            2.0 * (x * z + y * w),
            2.0 * (y * z - x * w),
            1.0 - 2.0 * (x * x + y * y),
            0.0,
        ],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// `a * b`: `b` applies first.
fn multiply(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut product = [[0.0; 4]; 4];
    for (column, out) in product.iter_mut().enumerate() {
        for (row, value) in out.iter_mut().enumerate() {
            *value = (0..4).map(|k| a[k][row] * b[column][k]).sum();
        }
    }
    product
}

/// `point` moved by `matrix` (a transform with no perspective part, as glTF nodes have).
fn transform_point(matrix: &Mat4, point: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|row| {
        matrix[0][row] * point[0]
            + matrix[1][row] * point[1]
            + matrix[2][row] * point[2]
            + matrix[3][row]
    })
}

/// Whether the matrix flips left and right (a negative determinant of its 3x3 part).
fn is_mirroring(matrix: &Mat4) -> bool {
    let [x, y, z] =
        [0, 1, 2].map(|column| [matrix[column][0], matrix[column][1], matrix[column][2]]);
    let cross = [
        y[1] * z[2] - y[2] * z[1],
        y[2] * z[0] - y[0] * z[2],
        y[0] * z[1] - y[1] * z[0],
    ];
    x[0] * cross[0] + x[1] * cross[1] + x[2] * cross[2] < 0.0
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    /// The triangle (0,0,0) (1,0,0) (0,1,0) as bytes, then its UVs.
    fn triangle_bin() -> Vec<u8> {
        let floats: [f32; 15] = [
            0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, /* UVs: */ 0.0, 0.0, 1.0, 0.25, 0.0,
            1.0,
        ];
        floats
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    /// A document with one triangle mesh over the buffer of [`triangle_bin`], placed by `nodes`
    /// (node 0 is the scene's root). Tests change parts of it.
    fn triangle_document(nodes: Value) -> Value {
        json!({
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": nodes,
            "meshes": [{"name": "tri", "primitives": [
                {"attributes": {"POSITION": 0, "TEXCOORD_0": 1}, "material": 0}]}],
            "materials": [{"name": "paint",
                "pbrMetallicRoughness": {"baseColorFactor": [1, 0.5, 0.25, 1]}}],
            "buffers": [{"byteLength": 60}],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": 36},
                {"buffer": 0, "byteOffset": 36, "byteLength": 24}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC2"}],
        })
    }

    /// `document` and `bin` as a `.glb`.
    fn glb_of(document: &Value, bin: &[u8]) -> Vec<u8> {
        let mut json = document.to_string().into_bytes();
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let mut bin = bin.to_vec();
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }

        let total = 12 + 8 + json.len() + 8 + bin.len();
        let mut out = b"glTF".to_vec();
        out.extend(2u32.to_le_bytes());
        out.extend((total as u32).to_le_bytes());
        out.extend((json.len() as u32).to_le_bytes());
        out.extend(b"JSON");
        out.extend(&json);
        out.extend((bin.len() as u32).to_le_bytes());
        out.extend(b"BIN\0");
        out.extend(&bin);
        out
    }

    /// A GLB with the triangle, and the given `nodes` JSON array.
    fn glb(nodes: &str) -> Vec<u8> {
        let nodes: Value = serde_json::from_str(nodes).unwrap();
        glb_of(&triangle_document(nodes), &triangle_bin())
    }

    /// Standard base64 with padding (the test side of [`decode_base64`]).
    fn encode_base64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let word = chunk.iter().enumerate().fold(0u32, |word, (at, &byte)| {
                word | u32::from(byte) << (16 - 8 * at)
            });
            for at in 0..4 {
                if at <= chunk.len() {
                    out.push(ALPHABET[(word >> (18 - 6 * at) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    /// A 1x1 red PNG.
    fn red_png() -> Vec<u8> {
        Image {
            width: 1,
            height: 1,
            rgba: vec![255, 0, 0, 255],
        }
        .to_png()
        .unwrap()
    }

    #[test]
    fn node_translation_moves_positions() {
        let model = from_slice(&glb(r#"[{"mesh":0,"translation":[10,20,30]}]"#)).unwrap();
        let part = &model.parts[0];
        assert_eq!(part.name, "tri");
        assert_eq!(
            part.positions,
            vec![[10.0, 20.0, 30.0], [11.0, 20.0, 30.0], [10.0, 21.0, 30.0]]
        );
        assert_eq!(part.corners, vec![[0, 1, 2]]);
        // UVs are kept as they are: glTF's V already points down.
        assert_eq!(part.uvs, vec![[0.0, 0.0], [1.0, 0.25], [0.0, 1.0]]);
        assert_eq!(part.material, Some(0));
        assert_eq!(model.materials[0].name, "paint");
        assert_eq!(model.materials[0].color, [1.0, 0.5, 0.25, 1.0]);
        assert!(model.validate().is_ok());
    }

    #[test]
    fn mirroring_node_swaps_corners() {
        let model = from_slice(&glb(r#"[{"mesh":0,"scale":[-1,1,1]}]"#)).unwrap();
        let part = &model.parts[0];
        // Without normals the vertices are renumbered by first use, so look at the triangle.
        let triangle = part.corners[0].map(|index| part.positions[index as usize]);
        assert_eq!(
            triangle,
            [[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]]
        );
    }

    #[test]
    fn child_nodes_inherit_their_parents_transform() {
        let nodes = r#"[{"children":[1],"scale":[2,2,2]},{"mesh":0,"translation":[1,0,0]}]"#;
        let model = from_slice(&glb(nodes)).unwrap();
        // Scaled by 2 after being moved by 1: (0,0,0) -> (2,0,0), (1,0,0) -> (4,0,0).
        assert_eq!(model.parts[0].positions[0], [2.0, 0.0, 0.0]);
        assert_eq!(model.parts[0].positions[1], [4.0, 0.0, 0.0]);
        assert_eq!(model.parts[0].corners, vec![[0, 1, 2]]);
    }

    #[test]
    fn rotation_and_matrix_place_nodes() {
        // A quarter turn about Z (w = z = sin 45 degrees) takes +X to +Y.
        let half = std::f32::consts::FRAC_1_SQRT_2;
        let nodes = json!([{"mesh": 0, "rotation": [0, 0, half, half]}]);
        let model = from_slice(&glb_of(&triangle_document(nodes), &triangle_bin())).unwrap();
        let moved = model.parts[0].positions[1];
        assert!(
            moved[0].abs() < 1e-6 && (moved[1] - 1.0).abs() < 1e-6,
            "{moved:?}"
        );

        // A column-major matrix: the translation is its last column.
        let nodes = json!([{"mesh": 0, "matrix": [1,0,0,0, 0,1,0,0, 0,0,1,0, 5,6,7,1]}]);
        let model = from_slice(&glb_of(&triangle_document(nodes), &triangle_bin())).unwrap();
        assert_eq!(model.parts[0].positions[0], [5.0, 6.0, 7.0]);
    }

    #[test]
    fn without_scenes_the_root_nodes_are_used() {
        let mut document = triangle_document(json!([{"children": [1]}, {"mesh": 0}]));
        document["scenes"] = json!([]);
        document.as_object_mut().unwrap().remove("scene");
        let model = from_slice(&glb_of(&document, &triangle_bin())).unwrap();
        assert_eq!(model.parts.len(), 1);
    }

    #[test]
    fn other_primitive_modes_are_skipped() {
        let mut document = triangle_document(json!([{"mesh": 0}]));
        document["meshes"][0]["primitives"][0]["mode"] = json!(1);
        let error = from_slice(&glb_of(&document, &triangle_bin())).unwrap_err();
        assert_eq!(error, "no triangles");
    }

    #[test]
    fn indices_of_each_size_are_read() {
        // A quad: 4 positions, two triangles by index.
        let positions: Vec<u8> = [
            0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0,
        ]
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
        for (component_type, indices) in [
            (5121, vec![0, 1, 2, 0, 2, 3]),
            (
                5123,
                [0u16, 1, 2, 0, 2, 3]
                    .iter()
                    .flat_map(|i| i.to_le_bytes())
                    .collect(),
            ),
            (
                5125,
                [0u32, 1, 2, 0, 2, 3]
                    .iter()
                    .flat_map(|i| i.to_le_bytes())
                    .collect(),
            ),
        ] {
            let mut bin = positions.clone();
            bin.extend(&indices);
            let mut document = triangle_document(json!([{"mesh": 0}]));
            document["buffers"] = json!([{"byteLength": bin.len()}]);
            document["bufferViews"] = json!([
                {"buffer": 0, "byteLength": 48},
                {"buffer": 0, "byteOffset": 48, "byteLength": indices.len()}]);
            document["accessors"] = json!([
                {"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3"},
                {"bufferView": 1, "componentType": component_type, "count": 6, "type": "SCALAR"}]);
            document["meshes"][0]["primitives"][0] =
                json!({"attributes": {"POSITION": 0}, "indices": 1});
            let model = from_slice(&glb_of(&document, &bin)).unwrap();
            assert_eq!(model.parts[0].corners, vec![[0, 1, 2], [0, 2, 3]]);
            assert!(model.parts[0].uvs.is_empty());
        }
    }

    #[test]
    fn sharp_edges_are_split_only_without_normals() {
        // Two triangles sharing the edge 0-1, folded 90 degrees; 4 positions, 16-bit indices and
        // (not read, but present) a normal per position.
        let floats: [f32; 24] = [
            0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, /* normals: */ 0.0,
            0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0,
        ];
        let mut bin: Vec<u8> = floats.iter().flat_map(|f| f.to_le_bytes()).collect();
        bin.extend([0u16, 1, 2, 0, 3, 1].iter().flat_map(|i| i.to_le_bytes()));
        let read = |attributes: Value| {
            let mut document = triangle_document(json!([{"mesh": 0}]));
            document["buffers"] = json!([{"byteLength": bin.len()}]);
            document["bufferViews"] = json!([
                {"buffer": 0, "byteLength": 48},
                {"buffer": 0, "byteOffset": 48, "byteLength": 48},
                {"buffer": 0, "byteOffset": 96, "byteLength": 12}]);
            document["accessors"] = json!([
                {"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3"},
                {"bufferView": 1, "componentType": 5126, "count": 4, "type": "VEC3"},
                {"bufferView": 2, "componentType": 5123, "count": 6, "type": "SCALAR"}]);
            document["meshes"][0]["primitives"][0] =
                json!({"attributes": attributes, "indices": 2});
            let model = from_slice(&glb_of(&document, &bin)).unwrap();
            assert!(model.validate().is_ok());
            model.parts[0].clone()
        };

        // Without normals the edge is sharp: both of its vertices are split.
        let part = read(json!({"POSITION": 0}));
        assert_eq!(part.positions.len(), 6);
        assert_eq!(part.corners.len(), 2);
        // With them the file's own vertices are kept.
        let part = read(json!({"POSITION": 0, "NORMAL": 1}));
        assert_eq!(part.positions.len(), 4);
        assert_eq!(part.corners, vec![[0, 1, 2], [0, 3, 1]]);
    }

    #[test]
    fn gltf_json_with_a_data_uri_buffer() {
        let mut document = triangle_document(json!([{"mesh": 0}]));
        let uri = format!(
            "data:application/octet-stream;base64,{}",
            encode_base64(&triangle_bin())
        );
        document["buffers"] = json!([{"byteLength": 60, "uri": uri}]);
        let model = from_slice(document.to_string().as_bytes()).unwrap();
        assert_eq!(model.parts[0].positions[1], [1.0, 0.0, 0.0]);
        assert_eq!(model.parts[0].uvs[1], [1.0, 0.25]);

        // A byte order mark in front is fine.
        let mut bytes = b"\xEF\xBB\xBF".to_vec();
        bytes.extend(document.to_string().as_bytes());
        assert!(from_slice(&bytes).is_ok());
    }

    #[test]
    fn interleaved_attributes_use_the_byte_stride() {
        // Per vertex: position (12 bytes) then UV (8 bytes) = 20 bytes.
        let vertices: [[f32; 5]; 3] = [
            [0.0, 0.0, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 1.0, 0.25],
            [0.0, 1.0, 0.0, 0.0, 1.0],
        ];
        let bin: Vec<u8> = vertices
            .iter()
            .flatten()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let mut document = triangle_document(json!([{"mesh": 0}]));
        document["buffers"] = json!([{"byteLength": 60}]);
        document["bufferViews"] = json!([{"buffer": 0, "byteLength": 60, "byteStride": 20}]);
        document["accessors"] = json!([
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
            {"bufferView": 0, "byteOffset": 12, "componentType": 5126, "count": 3, "type": "VEC2"}]);
        let model = from_slice(&glb_of(&document, &bin)).unwrap();
        let part = &model.parts[0];
        assert_eq!(part.positions[2], [0.0, 1.0, 0.0]);
        assert_eq!(part.uvs, vec![[0.0, 0.0], [1.0, 0.25], [0.0, 1.0]]);
    }

    #[test]
    fn normalized_byte_and_short_uvs_are_scaled() {
        let mut bin = triangle_bin()[..36].to_vec();
        bin.extend([0, 0, 255, 51, 0, 255]);
        let mut document = triangle_document(json!([{"mesh": 0}]));
        document["buffers"] = json!([{"byteLength": 42}]);
        document["bufferViews"][1] = json!({"buffer": 0, "byteOffset": 36, "byteLength": 6});
        document["accessors"][1] = json!({"bufferView": 1, "componentType": 5121, "normalized": true, "count": 3, "type": "VEC2"});
        let model = from_slice(&glb_of(&document, &bin)).unwrap();
        assert_eq!(model.parts[0].uvs, vec![[0.0, 0.0], [1.0, 0.2], [0.0, 1.0]]);

        // The same as shorts.
        let mut bin = triangle_bin()[..36].to_vec();
        bin.extend(
            [0u16, 0, 65535, 0, 0, 65535]
                .iter()
                .flat_map(|v| v.to_le_bytes()),
        );
        document["buffers"] = json!([{"byteLength": 48}]);
        document["bufferViews"][1] = json!({"buffer": 0, "byteOffset": 36, "byteLength": 12});
        document["accessors"][1]["componentType"] = json!(5123);
        let model = from_slice(&glb_of(&document, &bin)).unwrap();
        assert_eq!(model.parts[0].uvs, vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]);

        // Not normalized, the same bytes are no texture coordinates.
        document["accessors"][1]["normalized"] = json!(false);
        assert!(from_slice(&glb_of(&document, &bin)).is_err());
    }

    #[test]
    fn an_embedded_png_becomes_the_texture() {
        let png = red_png();
        let mut bin = triangle_bin();
        bin.extend(&png);
        let mut document = triangle_document(json!([{"mesh": 0}]));
        document["buffers"] = json!([{"byteLength": bin.len()}]);
        document["bufferViews"]
            .as_array_mut()
            .unwrap()
            .push(json!({"buffer": 0, "byteOffset": 60, "byteLength": png.len()}));
        document["images"] = json!([{"bufferView": 2, "mimeType": "image/png"}]);
        document["textures"] = json!([{"source": 0}]);
        document["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"] = json!({"index": 0});

        let model = from_slice(&glb_of(&document, &bin)).unwrap();
        let texture = model.materials[0].texture.as_ref().expect("a texture");
        assert_eq!((texture.width, texture.height), (1, 1));
        assert_eq!(texture.rgba, [255, 0, 0, 255]);

        // The same image as a data URI.
        document["images"] =
            json!([{"uri": format!("data:image/png;base64,{}", encode_base64(&png))}]);
        let model = from_slice(&glb_of(&document, &bin)).unwrap();
        assert!(model.materials[0].texture.is_some());

        // An image that can't be read leaves the material plain, not the model broken.
        document["images"] = json!([{"bufferView": 0}]);
        let model = from_slice(&glb_of(&document, &bin)).unwrap();
        assert!(model.materials[0].texture.is_none());
        assert_eq!(model.parts.len(), 1);
    }

    #[test]
    fn broken_accessors_are_errors() {
        let bad = |change: &dyn Fn(&mut Value)| {
            let mut document = triangle_document(json!([{"mesh": 0}]));
            change(&mut document);
            from_slice(&glb_of(&document, &triangle_bin()))
        };
        // More elements than the view holds.
        assert!(bad(&|d| d["accessors"][0]["count"] = json!(4)).is_err());
        // A huge count must not overflow or allocate.
        assert!(bad(&|d| d["accessors"][0]["count"] = json!(usize::MAX)).is_err());
        // Starting past the end of the view.
        assert!(bad(&|d| d["accessors"][0]["byteOffset"] = json!(100)).is_err());
        // A view past its buffer.
        assert!(bad(&|d| d["bufferViews"][0]["byteLength"] = json!(1000)).is_err());
        // Missing view, accessor or buffer.
        assert!(bad(&|d| d["accessors"][0]["bufferView"] = json!(9)).is_err());
        assert!(
            bad(&|d| d["meshes"][0]["primitives"][0]["attributes"]["POSITION"] = json!(9)).is_err()
        );
        assert!(bad(&|d| d["bufferViews"][0]["buffer"] = json!(3)).is_err());
        // A stride smaller than the element.
        assert!(bad(&|d| d["bufferViews"][0]["byteStride"] = json!(4)).is_err());
        // The wrong element type.
        assert!(bad(&|d| d["accessors"][0]["type"] = json!("VEC2")).is_err());
        // A buffer shorter than it says.
        assert!(bad(&|d| d["buffers"][0]["byteLength"] = json!(61)).is_err());
        // An index past the positions.
        assert!(bad(&|d| d["meshes"][0]["primitives"][0]["indices"] = json!(1)).is_err());
        // A node that doesn't exist.
        assert!(bad(&|d| d["scenes"][0]["nodes"] = json!([5])).is_err());

        let error = bad(&|d| d["accessors"][0]["sparse"] = json!({"count": 1})).unwrap_err();
        assert_eq!(error, "sparse accessors aren't supported");
    }

    #[test]
    fn node_loops_are_errors() {
        let document = triangle_document(json!([{"children": [0]}]));
        assert!(from_slice(&glb_of(&document, &triangle_bin())).is_err());
    }

    #[test]
    fn broken_glbs_are_errors() {
        let good = glb(r#"[{"mesh":0}]"#);
        assert!(from_slice(&good).is_ok());
        for length in [0, 4, 11, 12, 19, 25, good.len() - 1] {
            assert!(from_slice(&good[..length]).is_err(), "cut at {length}");
        }
        let mut version_one = good.clone();
        version_one[4] = 1;
        assert!(from_slice(&version_one).is_err());
        // A chunk that claims more than the file has.
        let mut long_chunk = good;
        long_chunk[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(from_slice(&long_chunk).is_err());
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(from_slice(b"not a glTF").is_err());
        assert!(from_slice(b"").is_err());
        assert!(from_slice(b"{}").is_err()); // no triangles
    }

    #[test]
    fn external_files_need_a_folder() {
        let mut document = triangle_document(json!([{"mesh": 0}]));
        document["buffers"] = json!([{"byteLength": 60, "uri": "tri.bin"}]);
        assert!(from_slice(document.to_string().as_bytes()).is_err());
    }

    #[test]
    fn reads_a_gltf_with_its_bin_file() {
        let folder = std::env::temp_dir().join(format!("model_io_gltf_{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("my tri.bin"), triangle_bin()).unwrap();
        let mut document = triangle_document(json!([{"mesh": 0}]));
        document["buffers"] = json!([{"byteLength": 60, "uri": "my%20tri.bin"}]);
        let path = folder.join("tri.gltf");
        std::fs::write(&path, document.to_string()).unwrap();

        let model = read(&path);
        std::fs::remove_dir_all(&folder).unwrap();
        assert_eq!(model.unwrap().parts[0].positions[1], [1.0, 0.0, 0.0]);
    }

    #[test]
    fn uris_may_not_leave_the_folder() {
        assert_eq!(relative_path("a.bin").unwrap(), PathBuf::from("a.bin"));
        assert_eq!(
            relative_path("sub/my%20file.bin").unwrap(),
            PathBuf::from("sub/my file.bin")
        );
        assert!(relative_path("./a.bin").is_ok());
        for bad in [
            "../a.bin",
            "sub/../../a.bin",
            "/etc/passwd",
            "C:/a.bin",
            "C:\\a.bin",
            "%2e%2e/a.bin",
            "https://example.com/a.bin",
            "a%zz.bin",
            "a%",
        ] {
            assert!(relative_path(bad).is_err(), "{bad}");
        }
        if cfg!(windows) {
            assert!(relative_path("..\\a.bin").is_err());
            assert!(relative_path("\\a.bin").is_err());
        }
    }

    #[test]
    fn base64_cases() {
        for (text, bytes) in [
            ("", &b""[..]),
            ("Zg==", b"f"),
            ("Zm8=", b"fo"),
            ("Zm9v", b"foo"),
            ("Zm9vYg==", b"foob"),
            ("Zm9vYmFy", b"foobar"),
            ("+/8=", &[0xFB, 0xFF]),
        ] {
            assert_eq!(decode_base64(text).unwrap(), bytes, "{text}");
            assert_eq!(encode_base64(bytes), text);
        }
        // Padding is optional.
        assert_eq!(decode_base64("Zg").unwrap(), b"f");
        assert_eq!(decode_base64("Zm8").unwrap(), b"fo");
        // Everything else is refused.
        for bad in [
            "Z", "Zm9v!", "Zm 9v", "Zm9v\n", "Z===", "Zg===", "=Zg=", "Zm9v-_", "Zm9vé",
        ] {
            assert!(decode_base64(bad).is_err(), "{bad:?}");
        }
        // All 256 byte values survive.
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(decode_base64(&encode_base64(&all)).unwrap(), all);
    }

    #[test]
    fn matrix_helpers() {
        let translate = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [5.0, 6.0, 7.0, 1.0],
        ];
        let scale = [
            [2.0, 0.0, 0.0, 0.0],
            [0.0, 2.0, 0.0, 0.0],
            [0.0, 0.0, 2.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        // Scale first, then translate.
        let both = multiply(&translate, &scale);
        assert_eq!(transform_point(&both, [1.0, 1.0, 1.0]), [7.0, 8.0, 9.0]);
        assert!(!is_mirroring(&both));
        let mirror = [
            [-1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        assert!(is_mirroring(&mirror));
        assert!(!is_mirroring(&multiply(&mirror, &mirror)));
        // A zero quaternion is no rotation.
        assert_eq!(rotation_matrix([0.0; 4]), IDENTITY);
    }
}
