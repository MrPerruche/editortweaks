//! STL: a bag of triangles, in a binary or a text flavour.
//!
//! Binary: an 80-byte header, a `u32` triangle count, then 50 bytes per triangle (a normal, three
//! vertices as three `f32` each, and a 2-byte attribute). Text: `solid name`, then
//! `facet normal .. / outer loop / vertex x y z (three times) / endloop / endfacet` per triangle.
//!
//! Gotchas: binary files may start with "solid" in their header too, so the size is checked first
//! (`84 + 50 * count` bytes means binary). There are no shared vertices and the stored normals
//! are often wrong, so every triangle gets its own three vertices (flat shading, which is how CAD
//! parts look anyway) and the normals are ignored.

use std::path::Path;

use crate::model::{MAX_TRIANGLES, Model, Part};

const HEADER_LEN: usize = 80;
/// Header plus the triangle count.
const BINARY_START: usize = HEADER_LEN + 4;
/// Normal (3 floats), three vertices (9 floats) and the attribute (2 bytes).
const TRIANGLE_LEN: usize = 12 * 4 + 2;
const NOT_STL: &str = "neither a binary nor a text STL (or a truncated binary one)";

/// Reads the STL file at `path`.
pub fn read(path: &Path) -> Result<Model, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    parse(&bytes)
}

/// Parses STL data, binary or text.
pub fn parse(bytes: &[u8]) -> Result<Model, String> {
    let (name, positions) = match binary_triangle_count(bytes) {
        Some(count) => ("stl".to_string(), parse_binary(bytes, count)?),
        None => parse_ascii(bytes)?,
    };
    if positions.is_empty() {
        return Err("no triangles".into());
    }
    let corners = (0..positions.len() as u32 / 3)
        .map(|t| [3 * t, 3 * t + 1, 3 * t + 2])
        .collect();
    let part = Part {
        name,
        positions,
        corners,
        ..Part::default()
    };
    Ok(Model {
        parts: vec![part],
        materials: vec![],
    })
}

/// The triangle count in the header if the file's size says it is a binary STL.
fn binary_triangle_count(bytes: &[u8]) -> Option<usize> {
    let count_bytes = bytes.get(HEADER_LEN..BINARY_START)?;
    let count = u32::from_le_bytes(count_bytes.try_into().ok()?) as usize;
    let expected = count.checked_mul(TRIANGLE_LEN)?.checked_add(BINARY_START)?;
    (expected == bytes.len()).then_some(count)
}

/// The vertices of a binary STL with `count` triangles (the size was checked already).
fn parse_binary(bytes: &[u8], count: usize) -> Result<Vec<[f32; 3]>, String> {
    check_triangle_count(count)?;
    let mut positions = Vec::with_capacity(count * 3);
    for triangle in bytes[BINARY_START..].as_chunks::<TRIANGLE_LEN>().0 {
        // Skip the normal (12 bytes); the attribute after the vertices is ignored too.
        for vertex in triangle[12..48].as_chunks::<12>().0 {
            positions.push(vertex_from_bytes(vertex)?);
        }
    }
    Ok(positions)
}

/// Three little-endian `f32`s.
fn vertex_from_bytes(bytes: &[u8; 12]) -> Result<[f32; 3], String> {
    let [x, y, z] = *bytes.as_chunks::<4>().0 else {
        unreachable!("12 bytes are three chunks of 4");
    };
    check_finite([x, y, z].map(f32::from_le_bytes))
}

/// The solid's name and the vertices (three per triangle) of a text STL.
///
/// Only `solid` (for the name) and `vertex` matter: the other words are structure that carries no
/// data we use, so they, the normals and anything unknown are skipped.
fn parse_ascii(bytes: &[u8]) -> Result<(String, Vec<[f32; 3]>), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| NOT_STL)?;
    let text = text.trim_start_matches('\u{feff}').trim_start();
    if !starts_with_ignore_case(text, "solid") {
        return Err(NOT_STL.into());
    }

    // The name is the rest of the first line: it may hold spaces.
    let (first_line, rest) = text.split_once('\n').unwrap_or((text, ""));
    let name = first_line["solid".len()..].trim();
    let name = if name.is_empty() { "stl" } else { name }.to_string();

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut words = rest.split_ascii_whitespace();
    while let Some(word) = words.next() {
        if !word.eq_ignore_ascii_case("vertex") {
            continue;
        }
        let mut vertex = [0.0; 3];
        for value in &mut vertex {
            let number = words.next().ok_or("a vertex is cut off")?;
            *value = number
                .parse()
                .map_err(|_| format!("{number:?} is not a number"))?;
        }
        positions.push(check_finite(vertex)?);
        check_triangle_count(positions.len().div_ceil(3))?;
    }
    if !positions.len().is_multiple_of(3) {
        return Err(format!(
            "{} vertices: not a whole number of triangles",
            positions.len()
        ));
    }
    Ok((name, positions))
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.as_bytes()
        .get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix.as_bytes()))
}

fn check_finite(vertex: [f32; 3]) -> Result<[f32; 3], String> {
    if vertex.iter().all(|value| value.is_finite()) {
        Ok(vertex)
    } else {
        Err("a vertex is not a finite number".into())
    }
}

/// Stops a file with too many triangles before it is copied into memory.
fn check_triangle_count(count: usize) -> Result<(), String> {
    if count > MAX_TRIANGLES {
        return Err(format!(
            "{count} triangles, more than the {MAX_TRIANGLES} a reference may have"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRIANGLES: [[[f32; 3]; 3]; 2] = [
        [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [0.5, 2.0, 1.5]],
    ];

    /// A binary STL of `TRIANGLES` with the given header text.
    fn binary(header: &str) -> Vec<u8> {
        let mut bytes = header.as_bytes().to_vec();
        bytes.resize(HEADER_LEN, 0);
        bytes.extend((TRIANGLES.len() as u32).to_le_bytes());
        for triangle in TRIANGLES {
            // A normal that must be ignored, the vertices, the attribute.
            for value in [9.0f32, 9.0, 9.0]
                .into_iter()
                .chain(triangle.into_iter().flatten())
            {
                bytes.extend(value.to_le_bytes());
            }
            bytes.extend([0, 0]);
        }
        bytes
    }

    fn check_two_triangles(model: &Model) {
        assert_eq!(model.parts.len(), 1);
        let part = &model.parts[0];
        assert_eq!(part.positions.len(), 6);
        assert_eq!(part.positions[0], [0.0, 0.0, 0.0]);
        assert_eq!(part.positions[5], [0.5, 2.0, 1.5]);
        assert_eq!(part.corners, vec![[0, 1, 2], [3, 4, 5]]);
        assert!(part.uvs.is_empty());
        assert_eq!(part.material, None);
        assert!(model.validate().is_ok());
    }

    #[test]
    fn binary_file() {
        let model = parse(&binary("made by a test")).unwrap();
        check_two_triangles(&model);
        assert_eq!(model.parts[0].name, "stl");
    }

    #[test]
    fn binary_file_whose_header_says_solid() {
        check_two_triangles(&parse(&binary("solid this is really binary")).unwrap());
    }

    #[test]
    fn ascii_file() {
        let text = "solid My Part\r\n\
            facet normal 0 0 1\r\n  outer loop\r\n    vertex 0 0 0\r\n    vertex 1.0 0 0\r\n    vertex 0 1e0 0\r\n  endloop\r\nendfacet\r\n\
            FACET NORMAL 0 0 1 OUTER LOOP VERTEX 0 0 1 VERTEX 1 0 1 VERTEX 0.5 2 1.5 ENDLOOP ENDFACET\r\n\
            endsolid My Part\r\n";
        let model = parse(text.as_bytes()).unwrap();
        check_two_triangles(&model);
        assert_eq!(model.parts[0].name, "My Part");
    }

    #[test]
    fn broken_files_are_errors() {
        let good = binary("");
        assert!(parse(&good[..good.len() - 10]).is_err());
        assert!(parse(&good[..40]).is_err());
        assert!(parse(b"").is_err());
        assert!(parse(b"solid empty\nendsolid empty\n").is_err());
        assert!(parse(b"solid a\nvertex 0 0 0\nvertex 1 0 0\nendsolid a\n").is_err());
        assert!(parse(b"solid a\nvertex 0 zero 0\n").is_err());
        assert!(parse(b"just some text").is_err());
    }

    #[test]
    fn too_many_triangles_are_refused_before_reading_them() {
        let mut bytes = binary("");
        bytes[HEADER_LEN..BINARY_START].copy_from_slice(&(MAX_TRIANGLES as u32 + 1).to_le_bytes());
        // The size no longer matches the count, so this is not even taken for binary.
        assert!(parse(&bytes).is_err());
        assert!(parse_binary(&bytes, MAX_TRIANGLES + 1).is_err());
    }
}
