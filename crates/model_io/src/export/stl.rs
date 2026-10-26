//! Writes an [`Export`] as a binary `.stl`: the triangles of every object, nothing else. STL
//! has no objects, colours or textures, so this is for slicers and CAD tools.
//!
//! Layout: an 80-byte header naming the export, a `u32` triangle count, then per triangle its
//! facet normal, its three vertices (`f32` each) and a `u16` attribute count of 0.
//!
//! Space: Z up and right-handed, `(X, -Y, Z) * scale` from the editor's (X, Y, Z) in
//! centimetres. The conversion is a reflection, so the corners keep their order and come out
//! counter-clockwise seen from outside, which STL's right-hand rule wants.

use super::Export;

/// `position` (the editor's space, cm) in STL's space, times `scale`.
fn to_stl(position: [f32; 3], scale: f32) -> [f32; 3] {
    [position[0], -position[1], position[2]].map(|axis| axis * scale)
}

/// The unit normal of the triangle `a`, `b`, `c` by the right-hand rule, or zeros if it has no
/// area (readers then work it out themselves).
fn facet_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let ab = [0, 1, 2].map(|axis| b[axis] - a[axis]);
    let ac = [0, 1, 2].map(|axis| c[axis] - a[axis]);
    let cross = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ];
    let length = (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt();
    if length > 0.0 && length.is_finite() {
        cross.map(|axis| axis / length)
    } else {
        [0.0; 3]
    }
}

/// `export` as the bytes of a binary `.stl`, `scale` times the editor's centimetres (0.01 for
/// metres, 10 for millimetres). Triangles pointing past their part's positions are left out.
pub fn write(export: &Export, scale: f32) -> Vec<u8> {
    let mut triangles: Vec<u8> = Vec::with_capacity(export.triangle_count() * 50);
    let mut count = 0u32;
    for part in export.objects.iter().flat_map(|object| &object.parts) {
        for corners in &part.corners {
            let [Some(&a), Some(&b), Some(&c)] =
                corners.map(|index| part.positions.get(index as usize))
            else {
                continue;
            };
            let [a, b, c] = [a, b, c].map(|position| to_stl(position, scale));
            for value in facet_normal(a, b, c).into_iter().chain(a).chain(b).chain(c) {
                triangles.extend_from_slice(&value.to_le_bytes());
            }
            triangles.extend_from_slice(&0u16.to_le_bytes());
            count += 1;
        }
    }

    let mut out = Vec::with_capacity(84 + triangles.len());
    // The header is free text, but must not start with "solid": readers would take the file for
    // the ASCII format.
    let name = export.name.replace(['\r', '\n'], " ");
    let mut header = format!("Brick Rigs editortweaks: {name}").into_bytes();
    header.resize(80, b' ');
    out.extend_from_slice(&header);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&triangles);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{Object, box_part};

    /// Each triangle's normal and three vertices.
    fn read_triangles(bytes: &[u8]) -> Vec<[[f32; 3]; 4]> {
        let count = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
        assert_eq!(bytes.len(), 84 + 50 * count);
        let float = |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        (0..count)
            .map(|n| {
                let start = 84 + 50 * n;
                core::array::from_fn(|k| {
                    core::array::from_fn(|axis| float(start + 12 * k + 4 * axis))
                })
            })
            .collect()
    }

    fn export_of(parts: Vec<crate::model::Part>) -> Export {
        Export {
            name: "Test".into(),
            objects: vec![Object {
                name: "Box".into(),
                parts,
            }],
            ..Export::default()
        }
    }

    #[test]
    fn a_box_faces_outward() {
        let export = export_of(vec![box_part([0.0; 3], [100.0, 200.0, 300.0])]);
        let bytes = write(&export, 0.01);
        assert!(bytes.starts_with(b"Brick Rigs"));
        let triangles = read_triangles(&bytes);
        assert_eq!(triangles.len(), 12);
        // Y is mirrored: the box spans y from -2 to 0 m, so its center is (0.5, -1, 1.5).
        let center = [0.5, -1.0, 1.5];
        for [normal, a, b, c] in triangles {
            let ab = [0, 1, 2].map(|i| b[i] - a[i]);
            let ac = [0, 1, 2].map(|i| c[i] - a[i]);
            let cross = [
                ab[1] * ac[2] - ab[2] * ac[1],
                ab[2] * ac[0] - ab[0] * ac[2],
                ab[0] * ac[1] - ab[1] * ac[0],
            ];
            // The stored normal is the winding's (right-hand rule) and points away from the center.
            let along: f32 = (0..3).map(|i| cross[i] * normal[i]).sum();
            assert!(along > 0.0);
            let outward: f32 = (0..3).map(|i| normal[i] * (a[i] - center[i])).sum();
            assert!(outward > 0.0, "{normal:?}");
            let length: f32 = normal.iter().map(|n| n * n).sum::<f32>().sqrt();
            assert!((length - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn empty_and_broken_exports_are_valid_files() {
        let bytes = write(&Export::default(), 1.0);
        assert_eq!(bytes.len(), 84);
        assert!(read_triangles(&bytes).is_empty());

        let mut part = box_part([0.0; 3], [1.0; 3]);
        part.corners.push([0, 1, 99]);
        let bytes = write(&export_of(vec![part]), 1.0);
        assert_eq!(read_triangles(&bytes).len(), 12);
    }
}
