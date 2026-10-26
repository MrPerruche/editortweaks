//! A cooked static mesh's triangles: the render data (positions and indices) of one of its levels of
//! detail, the triangles the game draws (LOD 0 up close, the next ones further away). Layouts: UE 4.27 `UStaticMesh::Serialize`, `FStaticMeshRenderData::Serialize`,
//! `FStaticMeshLODResources::Serialize` and `SerializeBuffers`, and the vertex and index buffers'
//! `Serialize` (see `docs/re/cooked-content.md`).

use std::collections::HashMap;

use super::bytes::Bytes;
use super::package::Package;

/// The most vertices and indices read (brick meshes have thousands).
const MAX_VERTICES: u32 = 4_000_000;

/// A mesh's triangles in its own space (centimetres, Unreal's axes), corners at the same place
/// welded into one vertex.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Triangles {
    pub vertices: Vec<[f32; 3]>,
    /// Three vertex indices each.
    pub triangles: Vec<[u32; 3]>,
    /// Each triangle's material slot (its section's material index); empty where that isn't known
    /// (collision triangles).
    pub slots: Vec<u32>,
    /// Each triangle's corners' UVs (the mesh's first UV channel), empty where they aren't known.
    pub uvs: Vec<[[f32; 2]; 3]>,
}

/// The triangles of LOD 0 of the static mesh whose export data is `data`, in `package`.
pub fn triangles(package: &Package, data: &[u8]) -> Result<Triangles, String> {
    level(package, data, 0).map(|(_, triangles)| triangles)
}

/// The triangles of LOD `wanted` of the static mesh whose export data is `data`, in `package`, or of
/// its least detailed LOD if it has fewer (or the ones past it aren't in the package), and which LOD
/// that is.
pub fn level(package: &Package, data: &[u8], wanted: usize) -> Result<(usize, Triangles), String> {
    let mut bytes = Bytes::new(data);
    skip_properties(package, &mut bytes)?;
    // `UObject::Serialize`: the object's GUID, if it has one.
    if bytes.bool32()? {
        bytes.skip(16)?;
    }
    let strip = StripFlags::read(&mut bytes)?;
    if !strip.editor_data_stripped() {
        return Err("the mesh isn't cooked".into());
    }
    if !bytes.bool32()? {
        return Err("the mesh has no render data".into());
    }
    // Body setup and nav collision (package indices), lighting GUID.
    bytes.skip(4 + 4 + 16)?;
    let sockets = bytes.count(4)?;
    bytes.skip(sockets * 4)?;
    let count = bytes.count(1)?;
    if count == 0 {
        return Err("the mesh has no LODs".into());
    }
    let mut found = None;
    for index in 0..count.min(wanted.saturating_add(1)) {
        match lod(&mut bytes) {
            Ok(Some(triangles)) => found = Some((index, triangles)),
            Ok(None) => {}
            // The LODs before stand in: what follows can't be found.
            Err(reason) => match found {
                Some(_) => break,
                None => return Err(reason),
            },
        }
    }
    found.ok_or_else(|| "none of the mesh's LODs is in the package".into())
}

/// The next LOD (`FStaticMeshLODResources`), its triangles section by section (`None` if it was
/// cooked out: the platform doesn't draw it), leaving `bytes` at the one after it.
fn lod(bytes: &mut Bytes) -> Result<Option<Triangles>, String> {
    let strip = StripFlags::read(bytes)?;
    // `FStaticMeshSection`s: material index, first index, triangle count, smallest and largest
    // vertex index, then four flags (collision, shadow, force opaque, ray tracing).
    let section_count = bytes.count(36)?;
    let mut sections = Vec::with_capacity(section_count);
    for _ in 0..section_count {
        let material = bytes.u32()?;
        let first = bytes.u32()?;
        let count = bytes.u32()?;
        bytes.skip(8)?;
        for _ in 0..4 {
            bytes.bool32()?;
        }
        sections.push((material, first as usize, count as usize));
    }
    let _max_deviation = bytes.f32()?;
    let cooked_out = bytes.bool32()?;
    let inlined = bytes.bool32()?;
    if cooked_out {
        return Ok(None);
    }
    if strip.server_data_stripped() {
        return Err("the LOD isn't in the package".into());
    }
    if !inlined {
        return Err("the LOD is streamed (not read)".into());
    }
    let Buffers {
        positions,
        uvs,
        indices,
    } = buffers(bytes, strip.class, section_count)?;
    let mut triangles = Triangles::default();
    let mut welded: HashMap<[u32; 3], u32> = HashMap::new();
    let mut remap = Vec::with_capacity(positions.len());
    for position in &positions {
        // -0.0 and 0.0 are the same place.
        let key = position.map(|coordinate| (coordinate + 0.0).to_bits());
        let index = *welded.entry(key).or_insert_with(|| {
            triangles.vertices.push(*position);
            triangles.vertices.len() as u32 - 1
        });
        remap.push(index);
    }
    for (material, first, count) in sections {
        let corners = first
            .checked_add(count.checked_mul(3).ok_or("bad section")?)
            .and_then(|end| indices.get(first..end))
            .ok_or("a section is out of the index buffer")?;
        for corners in corners.as_chunks::<3>().0 {
            let mut triangle = [0; 3];
            let mut corner_uvs = [[0.0; 2]; 3];
            for ((welded, uv), &corner) in triangle.iter_mut().zip(&mut corner_uvs).zip(corners) {
                *welded = *remap
                    .get(corner as usize)
                    .ok_or("an index is out of the vertex buffer")?;
                *uv = uvs.get(corner as usize).copied().unwrap_or_default();
            }
            // Corners welded together leave no triangle.
            if triangle[0] != triangle[1]
                && triangle[1] != triangle[2]
                && triangle[0] != triangle[2]
            {
                triangles.triangles.push(triangle);
                triangles.slots.push(material);
                if !uvs.is_empty() {
                    triangles.uvs.push(corner_uvs);
                }
            }
        }
    }
    if triangles.triangles.is_empty() {
        return Err("the mesh has no triangles".into());
    }
    Ok(Some(triangles))
}

/// What's read of the buffers a LOD is drawn with.
struct Buffers {
    positions: Vec<[f32; 3]>,
    /// Each vertex's UV in the first channel (empty if the mesh has none).
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

/// The buffers a LOD is drawn with (`SerializeBuffers`): the positions, the first UV channel and the
/// indices; the rest is skipped. `class` is the LOD's class strip flags, `section_count` how many
/// sections it has.
fn buffers(bytes: &mut Bytes, class: u8, section_count: usize) -> Result<Buffers, String> {
    StripFlags::read(bytes)?;
    // `FPositionVertexBuffer`.
    let stride = bytes.u32()?;
    let vertex_count = bytes.u32()?;
    let (element, data) = bulk(bytes)?;
    if stride != 12 || element != 12 || data.len() != vertex_count as usize * 12 {
        return Err(format!(
            "unexpected position buffer ({vertex_count} vertices, stride {stride}, {} bytes)",
            data.len()
        ));
    }
    if vertex_count > MAX_VERTICES {
        return Err(format!("too many vertices ({vertex_count})"));
    }
    let positions = data
        .as_chunks::<12>()
        .0
        .iter()
        .map(|vertex| {
            core::array::from_fn(|axis| {
                let at = axis * 4;
                f32::from_le_bytes([vertex[at], vertex[at + 1], vertex[at + 2], vertex[at + 3]])
            })
        })
        .collect();
    // `FStaticMeshVertexBuffer`: UV count, vertex count, full-precision UVs, high-precision
    // tangents, then the tangents and the UVs (each vertex's channels one after the other).
    StripFlags::read(bytes)?;
    let channels = bytes.u32()? as usize;
    let uv_vertices = bytes.u32()?;
    let full_precision = bytes.bool32()?;
    bytes.bool32()?;
    bulk(bytes)?;
    let (element, data) = bulk(bytes)?;
    let uvs = if channels > 0 && uv_vertices == vertex_count {
        vertex_uvs(
            data,
            element,
            channels,
            full_precision,
            vertex_count as usize,
        )?
    } else {
        Vec::new()
    };
    // `FColorVertexBuffer`: stride, vertex count, the colours if there are any.
    StripFlags::read(bytes)?;
    let _stride = bytes.u32()?;
    if bytes.u32()? > 0 {
        bulk(bytes)?;
    }
    let (wide, data) = index_buffer(bytes)?;
    skip_after_indices(bytes, class, section_count)?;
    let indices = if wide {
        data.as_chunks::<4>()
            .0
            .iter()
            .map(|&index| u32::from_le_bytes(index))
            .collect()
    } else {
        data.as_chunks::<2>()
            .0
            .iter()
            .map(|&index| u32::from(u16::from_le_bytes(index)))
            .collect()
    };
    Ok(Buffers {
        positions,
        uvs,
        indices,
    })
}

/// The first UV channel of each of `count` vertices, from a vertex buffer's UVs (`data`, elements
/// of `element` bytes: one per channel per vertex, two `f32`s with `full_precision`, else two
/// halves).
fn vertex_uvs(
    data: &[u8],
    element: usize,
    channels: usize,
    full_precision: bool,
    count: usize,
) -> Result<Vec<[f32; 2]>, String> {
    let expected = if full_precision { 8 } else { 4 };
    if element != expected || data.len() != count * channels * element {
        return Err(format!(
            "unexpected UV buffer ({count} vertices, {channels} channels, elements of {element}              bytes, {} bytes)",
            data.len()
        ));
    }
    Ok(data
        .chunks_exact(channels * element)
        .map(|vertex| {
            core::array::from_fn(|axis| {
                if full_precision {
                    let at = axis * 4;
                    f32::from_le_bytes([vertex[at], vertex[at + 1], vertex[at + 2], vertex[at + 3]])
                } else {
                    let at = axis * 2;
                    half(u16::from_le_bytes([vertex[at], vertex[at + 1]]))
                }
            })
        })
        .collect())
}

/// An IEEE half-precision float (`FFloat16`) as an `f32`.
fn half(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1F);
    let fraction = f32::from(bits & 0x3FF);
    sign * match exponent {
        // Subnormal: no implicit leading 1.
        0 => fraction * 2f32.powi(-24),
        0x1F if fraction == 0.0 => f32::INFINITY,
        0x1F => f32::NAN,
        _ => (1.0 + fraction / 1024.0) * 2f32.powi(exponent - 15),
    }
}

/// An `FRawStaticIndexBuffer`: whether indices are 32-bit, then them as bytes.
fn index_buffer<'a>(bytes: &mut Bytes<'a>) -> Result<(bool, &'a [u8]), String> {
    let wide = bytes.bool32()?;
    let (element, data) = bulk(bytes)?;
    if element != 1 {
        return Err(format!(
            "unexpected index buffer (elements of {element} bytes)"
        ));
    }
    Ok((wide, data))
}

/// `FStripDataFlags`' class flags of a LOD (`EClassDataStripFlag`): what it was cooked without.
const ADJACENCY_STRIPPED: u8 = 1;
const REVERSED_INDICES_STRIPPED: u8 = 4;
const RAY_TRACING_STRIPPED: u8 = 8;

/// Skips what follows a LOD's index buffer (found in the installed meshes, see
/// `docs/re/cooked-content.md`): the other index buffers (reversed, depth only, reversed depth only,
/// adjacency; each after a `u32` flag), the ray tracing data (the same), the area-weighted samplers
/// (one per section and one for the whole LOD), and the buffers' sizes.
fn skip_after_indices(bytes: &mut Bytes, class: u8, section_count: usize) -> Result<(), String> {
    let reversed = class & REVERSED_INDICES_STRIPPED == 0;
    let mut extra_index_buffers = 1;
    if reversed {
        extra_index_buffers += 2;
    }
    if class & ADJACENCY_STRIPPED == 0 {
        extra_index_buffers += 1;
    }
    for _ in 0..extra_index_buffers {
        bytes.bool32()?;
        index_buffer(bytes)?;
    }
    if class & RAY_TRACING_STRIPPED == 0 {
        bytes.bool32()?;
        bulk(bytes)?;
    }
    // `FStaticMeshSectionAreaWeightedTriangleSampler`s and `FStaticMeshAreaWeightedSectionSampler`:
    // probabilities (f32s), aliases (i32s), the total weight.
    for _ in 0..=section_count {
        let probabilities = bytes.count(4)?;
        bytes.skip(probabilities * 4)?;
        let aliases = bytes.count(4)?;
        bytes.skip(aliases * 4)?;
        bytes.f32()?;
    }
    // `FStaticMeshBuffersSize`: three u32s.
    bytes.skip(12)
}

/// A `TResourceArray` serialized in bulk: the element size, then a `TArray` of them.
fn bulk<'a>(bytes: &mut Bytes<'a>) -> Result<(usize, &'a [u8]), String> {
    let element = usize::try_from(bytes.i32()?).map_err(|_| "bad element size")?;
    let count = bytes.count(element)?;
    Ok((element, bytes.take(count * element)?))
}

/// Skips the export's tagged properties (`FPropertyTag`s up to one named `None`).
fn skip_properties(package: &Package, bytes: &mut Bytes) -> Result<(), String> {
    loop {
        let name = package.name(bytes)?;
        if name == "None" {
            return Ok(());
        }
        let kind = package.name(bytes)?;
        let size = usize::try_from(bytes.i32()?).map_err(|_| format!("bad size of {name}"))?;
        let _array_index = bytes.i32()?;
        match kind.as_str() {
            "StructProperty" => {
                package.name(bytes)?;
                bytes.skip(16)?;
            }
            "BoolProperty" => bytes.skip(1)?,
            "ByteProperty" | "EnumProperty" | "ArrayProperty" | "SetProperty" => {
                package.name(bytes)?;
            }
            "MapProperty" => {
                package.name(bytes)?;
                package.name(bytes)?;
            }
            _ => {}
        }
        // The property's GUID, if it has one.
        if bytes.u8()? != 0 {
            bytes.skip(16)?;
        }
        bytes.skip(size)?;
    }
}

/// `FStripDataFlags`: what cooking left out.
struct StripFlags {
    global: u8,
    class: u8,
}

impl StripFlags {
    fn read(bytes: &mut Bytes) -> Result<Self, String> {
        let global = bytes.u8()?;
        let class = bytes.u8()?;
        Ok(Self { global, class })
    }

    fn editor_data_stripped(&self) -> bool {
        self.global & 1 != 0
    }

    fn server_data_stripped(&self) -> bool {
        self.global & 2 != 0
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::cooked::package::tests::header;

    /// The names [`export`]'s data uses.
    pub const NAMES: [&str; 6] = [
        "None",
        "Class",
        "StaticMesh",
        "Mesh",
        "IntProperty",
        "LightMapResolution",
    ];

    /// A cooked static mesh's export data: one tagged property, then one LOD with `positions` and,
    /// in one section, `indices` (32-bit if `wide`).
    pub fn export(positions: &[[f32; 3]], indices: &[u32], wide: bool) -> Vec<u8> {
        export_lods(&[(positions, indices)], wide)
    }

    /// Like [`export`], a LOD for each of `lods` (positions and indices).
    pub fn export_lods(lods: &[(&[[f32; 3]], &[u32])], wide: bool) -> Vec<u8> {
        let name = |out: &mut Vec<u8>, name: &str| {
            let index = NAMES.iter().position(|&known| known == name).unwrap() as i32;
            out.extend(index.to_le_bytes());
            out.extend(0i32.to_le_bytes());
        };
        let mut out = Vec::new();
        name(&mut out, "LightMapResolution");
        name(&mut out, "IntProperty");
        u32s(&mut out, &[4, 0]);
        out.push(0);
        u32s(&mut out, &[64]);
        name(&mut out, "None");
        // No GUID; strip flags; cooked; body setup, nav collision; lighting GUID; no sockets;
        // the LODs.
        u32s(&mut out, &[0]);
        out.extend([1, 0]);
        u32s(&mut out, &[1, 0, 0, 0, 0, 0, 0, 0, lods.len() as u32]);
        for (positions, indices) in lods {
            lod(&mut out, positions, indices, wide);
        }
        // What follows the LODs isn't read.
        out.extend([0xAB; 16]);
        out
    }

    fn u32s(out: &mut Vec<u8>, values: &[u32]) {
        for value in values {
            out.extend(value.to_le_bytes());
        }
    }

    /// A LOD: strip flags, one section, max deviation, not cooked out, inlined, its buffers.
    fn lod(out: &mut Vec<u8>, positions: &[[f32; 3]], indices: &[u32], wide: bool) {
        out.extend([1, 0]);
        let triangle_count = (indices.len() / 3) as u32;
        u32s(
            out,
            &[
                1,
                0,
                0,
                triangle_count,
                0,
                positions.len() as u32 - 1,
                1,
                1,
                0,
                1,
            ],
        );
        u32s(out, &[0, 0, 1]);
        // Positions.
        out.extend([1, 0]);
        u32s(
            out,
            &[12, positions.len() as u32, 12, positions.len() as u32],
        );
        for position in positions {
            for coordinate in position {
                out.extend(coordinate.to_le_bytes());
            }
        }
        // Tangents and UVs: one UV set, 8 and 4 bytes a vertex.
        out.extend([1, 0]);
        u32s(out, &[1, positions.len() as u32, 0, 0]);
        u32s(out, &[8, positions.len() as u32]);
        out.extend(vec![0; 8 * positions.len()]);
        u32s(out, &[4, positions.len() as u32]);
        for position in positions {
            for coordinate in &position[..2] {
                out.extend(half_bits(coordinate / 10.0).to_le_bytes());
            }
        }
        // No colours.
        out.extend([1, 0]);
        u32s(out, &[4, 0]);
        // Indices.
        let index_bytes = |out: &mut Vec<u8>| {
            u32s(out, &[u32::from(wide)]);
            let width = if wide { 4 } else { 2 };
            u32s(out, &[1, (indices.len() * width) as u32]);
            for &index in indices {
                if wide {
                    out.extend(index.to_le_bytes());
                } else {
                    out.extend((index as u16).to_le_bytes());
                }
            }
        };
        index_bytes(out);
        // Reversed (empty), depth only (the same indices), reversed depth only and adjacency
        // (empty), each after a flag; no ray tracing data.
        u32s(out, &[0, 0, 1, 0]);
        u32s(out, &[0]);
        index_bytes(out);
        u32s(out, &[0, 0, 1, 0, 0, 0, 1, 0]);
        u32s(out, &[0, 1, 0]);
        // Samplers for the section and the LOD: one triangle's, and none.
        u32s(out, &[1]);
        out.extend(1.0f32.to_le_bytes());
        u32s(out, &[1, 0]);
        out.extend(2.5f32.to_le_bytes());
        u32s(out, &[0, 0, 0]);
        // Buffer sizes.
        u32s(out, &[100, 6, 0]);
    }

    /// `value` (zero, or normal as a half) as a half, its fraction cut short.
    fn half_bits(value: f32) -> u16 {
        if value == 0.0 {
            return 0;
        }
        let bits = value.to_bits();
        let sign = ((bits >> 16) & 0x8000) as u16;
        let exponent = ((bits >> 23) & 0xFF) as i32 - 127 + 15;
        assert!((1..31).contains(&exponent), "no half for {value}");
        sign | ((exponent as u16) << 10) | ((bits >> 13) & 0x3FF) as u16
    }

    #[test]
    fn reads_halves() {
        assert_eq!(half(0x3C00), 1.0);
        assert_eq!(half(0xC000), -2.0);
        assert_eq!(half(0x3800), 0.5);
        assert_eq!(half(0x0001), 2f32.powi(-24));
        assert!((half(0x2E66) - 0.1).abs() < 1e-4);
    }

    fn package() -> Package {
        Package::parse(&header(
            &NAMES,
            &[("Class", 0, "StaticMesh")],
            &[(-1, "Mesh", 0)],
        ))
        .unwrap()
    }

    #[test]
    fn reads_lod_0() {
        let positions = [
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [0.0, 10.0, 0.0],
            [10.0, 10.0, 0.0],
        ];
        let indices = [0, 1, 2, 2, 1, 3];
        for wide in [false, true] {
            let read = triangles(&package(), &export(&positions, &indices, wide)).unwrap();
            assert_eq!(read.vertices, positions);
            assert_eq!(read.triangles, [[0, 1, 2], [2, 1, 3]]);
            // Each corner's UV: its position over 10.
            assert_eq!(read.uvs[1], [[0.0, 1.0], [1.0, 0.0], [1.0, 1.0]]);
        }
    }

    #[test]
    fn welds_corners() {
        // A seam: corners 1 and 3 are at the same place.
        let positions = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            [-0.0, 1.0, 0.0],
        ];
        let read = triangles(&package(), &export(&positions, &[0, 1, 2, 3, 4, 0], false)).unwrap();
        assert_eq!(read.vertices.len(), 3);
        assert_eq!(read.triangles, [[0, 1, 2], [1, 2, 0]]);
    }

    #[test]
    fn reads_the_lod_wanted_or_the_last() {
        let detailed = [
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [0.0, 10.0, 0.0],
            [10.0, 10.0, 0.0],
        ];
        let coarse = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]];
        let data = export_lods(
            &[(&detailed, &[0, 1, 2, 2, 1, 3]), (&coarse, &[0, 1, 2])],
            false,
        );
        let (index, lod_0) = level(&package(), &data, 0).unwrap();
        assert_eq!((index, lod_0.triangles.len()), (0, 2));
        let (index, lod_1) = level(&package(), &data, 1).unwrap();
        assert_eq!((index, lod_1.vertices), (1, coarse.to_vec()));
        assert_eq!(level(&package(), &data, 7).unwrap().0, 1);
        // A LOD that doesn't read leaves the one before.
        let mut cut = data.clone();
        cut.truncate(cut.len() - 60);
        assert_eq!(level(&package(), &cut, 1).unwrap().0, 0);
    }

    #[test]
    fn refuses_bad_indices() {
        let positions = [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        assert!(triangles(&package(), &export(&positions, &[0, 1, 7], false)).is_err());
        let mut data = export(&positions, &[0, 1, 2], false);
        data.truncate(data.len() - 30);
        assert!(triangles(&package(), &data).is_err());
    }
}
