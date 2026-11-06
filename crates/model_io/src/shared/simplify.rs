//! Fewer triangles for a part, through meshoptimizer (`meshopt`).
//!
//! The simplifier only collapses edges between vertices that are joined through the indices, so a
//! mesh whose every face has vertices of its own (an STL's) can't shrink: it is welded first, by
//! position and UV, then by position alone. Failing that the sloppy simplifier, which always
//! reaches the target, and last of all, dropping triangles evenly.

use meshopt::{SimplifyOptions, VertexDataAdapter};

use crate::model::{Part, SMOOTH_ANGLE};

/// A simplified result more than this percent of the target is "well over": welding is tried.
const WELL_OVER_PERCENT: usize = 125;

/// The error simplification may reach: more than the mesh is big, so the target count decides.
const TARGET_ERROR: f32 = 1.0;

/// How many of `target` triangles each part gets, in proportion to its own `counts` and none more
/// than its own: what is left of the rounding down goes to the parts that lost the most.
pub(super) fn allocate(counts: &[usize], target: usize) -> Vec<usize> {
    let total: u128 = counts.iter().map(|&count| count as u128).sum();
    if total == 0 {
        return vec![0; counts.len()];
    }
    let target = (target as u128).min(total);
    let mut shares: Vec<usize> = counts
        .iter()
        .map(|&count| (count as u128 * target / total) as usize)
        .collect();
    let mut left = target as usize - shares.iter().sum::<usize>();
    // The biggest fractions first: count * target mod total, compared without dividing.
    let mut order: Vec<usize> = (0..counts.len()).collect();
    order.sort_by_key(|&part| std::cmp::Reverse(counts[part] as u128 * target % total));
    for part in order {
        if left == 0 {
            break;
        }
        if shares[part] < counts[part] {
            shares[part] += 1;
            left -= 1;
        }
    }
    shares
}

/// `part` with the triangles in a better order for the vertex cache (which compresses better too),
/// nothing else changed.
pub(super) fn tidied(part: &Part) -> Part {
    let indices = flat(&part.corners);
    let ordered = meshopt::optimize_vertex_cache(&indices, part.positions.len());
    Part {
        corners: triangles(&ordered),
        ..part.clone()
    }
}

/// `part` with at most `target` triangles, welded as above if it comes to that. A part without
/// UVs gets its hard edges back after a weld by position alone, since the game shades by shared
/// vertices. Every index of the part must be in range.
pub(super) fn reduced(part: &Part, target: usize) -> Part {
    if part.corners.len() <= target {
        return tidied(part);
    }
    let textured = !part.uvs.is_empty();
    let mut mesh = Mesh {
        positions: part.positions.clone(),
        uvs: part.uvs.clone(),
        indices: flat(&part.corners),
    };
    let mut indices = mesh.simplify(target);
    let mut by_position = false;
    // Welding by position and UV changes nothing that shows. By position alone it costs a
    // textured part its seams, so that is only for a result well over.
    for by_uv in [true, false] {
        if by_uv && !textured {
            // Without UVs that is the weld by position alone, which is the next step.
            continue;
        }
        let triangles = indices.len() / 3;
        let over = if by_uv || !textured {
            triangles > target
        } else {
            well_over(triangles, target)
        };
        if !over {
            continue;
        }
        mesh = mesh.welded(by_uv);
        by_position = !by_uv;
        indices = mesh.simplify(target);
    }
    if indices.len() / 3 > target {
        // Sloppy simplification lands anywhere below the target, even at nothing: only a result
        // of at least half of it is better than dropping triangles.
        let sloppy = mesh.simplify_sloppy(&indices, target);
        if sloppy.len() / 3 >= target / 2 {
            indices = sloppy;
        }
    }
    if indices.len() / 3 > target {
        indices = dropped_evenly(&indices, target);
    }

    let ordered = meshopt::optimize_vertex_cache(&indices, mesh.positions.len());
    let mut result = mesh.part_of(&ordered, part);
    if by_position && !textured {
        result.split_sharp_edges(SMOOTH_ANGLE);
    }
    result
}

fn well_over(triangles: usize, target: usize) -> bool {
    triangles > target * WELL_OVER_PERCENT / 100 + 2
}

/// Every `n`th triangle, so that `target` are left.
fn dropped_evenly(indices: &[u32], target: usize) -> Vec<u32> {
    let count = indices.len() / 3;
    (0..target)
        .flat_map(|kept| {
            let triangle = (kept as u128 * count as u128 / target as u128) as usize;
            indices[triangle * 3..triangle * 3 + 3].iter().copied()
        })
        .collect()
}

fn flat(corners: &[[u32; 3]]) -> Vec<u32> {
    corners.iter().flatten().copied().collect()
}

fn triangles(indices: &[u32]) -> Vec<[u32; 3]> {
    indices.as_chunks::<3>().0.to_vec()
}

/// A part's vertices and its triangles as a flat index list, the shape the simplifier takes.
struct Mesh {
    positions: Vec<[f32; 3]>,
    /// One per position, or empty.
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl Mesh {
    /// The simplifier's result for `target` triangles: indices into this mesh's vertices.
    fn simplify(&self, target: usize) -> Vec<u32> {
        let bytes = position_bytes(&self.positions);
        let Ok(vertices) = VertexDataAdapter::new(&bytes, 12, 0) else {
            return self.indices.clone();
        };
        meshopt::simplify(
            &self.indices,
            &vertices,
            target * 3,
            TARGET_ERROR,
            SimplifyOptions::None,
            None,
        )
    }

    /// The sloppy simplifier's result for `target` triangles, from the triangles `indices`.
    fn simplify_sloppy(&self, indices: &[u32], target: usize) -> Vec<u32> {
        let bytes = position_bytes(&self.positions);
        let Ok(vertices) = VertexDataAdapter::new(&bytes, 12, 0) else {
            return indices.to_vec();
        };
        meshopt::simplify_sloppy(indices, &vertices, target * 3, TARGET_ERROR, None)
    }

    /// The same mesh with vertices at one position (and with one UV, if `by_uv`) made one. The
    /// first of them keeps its UV.
    fn welded(&self, by_uv: bool) -> Mesh {
        // The bit patterns of the values, with -0.0 as 0.0: the vertices that look the same.
        let bits = |value: f32| (value + 0.0).to_bits();
        let (count, remap) = if by_uv && !self.uvs.is_empty() {
            let keys: Vec<[u32; 5]> = self
                .positions
                .iter()
                .zip(&self.uvs)
                .map(|(p, uv)| [bits(p[0]), bits(p[1]), bits(p[2]), bits(uv[0]), bits(uv[1])])
                .collect();
            meshopt::generate_vertex_remap(&keys, Some(&self.indices))
        } else {
            let keys: Vec<[u32; 3]> = self.positions.iter().map(|p| p.map(bits)).collect();
            meshopt::generate_vertex_remap(&keys, Some(&self.indices))
        };
        Mesh {
            positions: meshopt::remap_vertex_buffer(&self.positions, count, &remap),
            uvs: if self.uvs.is_empty() {
                Vec::new()
            } else {
                meshopt::remap_vertex_buffer(&self.uvs, count, &remap)
            },
            indices: meshopt::remap_index_buffer(Some(&self.indices), self.positions.len(), &remap),
        }
    }

    /// A part with `like`'s material of the triangles `indices`, and the vertices they use in the
    /// order they first appear.
    fn part_of(&self, indices: &[u32], like: &Part) -> Part {
        let none = u32::MAX;
        let mut moved = vec![none; self.positions.len()];
        let mut part = Part {
            name: like.name.clone(),
            material: like.material,
            ..Part::default()
        };
        for &index in indices {
            let slot = &mut moved[index as usize];
            if *slot == none {
                *slot = part.positions.len() as u32;
                part.positions.push(self.positions[index as usize]);
                if !self.uvs.is_empty() {
                    part.uvs.push(self.uvs[index as usize]);
                }
            }
        }
        part.corners = indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|triangle| triangle.map(|index| moved[index as usize]))
            .collect();
        part
    }
}

/// The positions as the little-endian floats the simplifier reads.
fn position_bytes(positions: &[[f32; 3]]) -> Vec<u8> {
    positions
        .iter()
        .flatten()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_is_proportional_and_never_over() {
        assert_eq!(allocate(&[100, 300], 100), [25, 75]);
        // Rounding down leaves 1: it goes to the part that lost the most.
        assert_eq!(allocate(&[10, 10, 10], 10), [4, 3, 3]);
        assert_eq!(allocate(&[2, 1000], 5), [0, 5]);
        // A target over the total gives every part all of its triangles.
        assert_eq!(allocate(&[3, 4], 100), [3, 4]);
        assert_eq!(allocate(&[], 5), Vec::<usize>::new());
        assert_eq!(allocate(&[0, 0], 5), [0, 0]);
        for (counts, target) in [(vec![7, 13, 1, 99], 37), (vec![1; 50], 20)] {
            assert_eq!(allocate(&counts, target).iter().sum::<usize>(), target);
        }
    }

    #[test]
    fn dropping_evenly_keeps_the_target() {
        let indices: Vec<u32> = (0..30).collect();
        let kept = dropped_evenly(&indices, 4);
        assert_eq!(kept.len(), 12);
        assert_eq!(kept[0..3], [0, 1, 2]);
        assert!(kept.as_chunks::<3>().0.iter().all(|t| t[1] == t[0] + 1));
    }
}
