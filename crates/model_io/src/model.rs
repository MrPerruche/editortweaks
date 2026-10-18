//! A model as plain data: parts of triangles, each with one material.

use crate::image::Image;

/// The most triangles a model may have. A reference is something to build next to, not a scan:
/// this keeps a wrong file from filling memory or stalling the mesh build.
pub const MAX_TRIANGLES: usize = 4_000_000;

/// The default angle in degrees above which the edges of a model without normals look sharp
/// (Blender's auto smooth default).
pub const SMOOTH_ANGLE: f32 = 30.0;

/// A model read from a file.
#[derive(Clone, Debug, Default)]
pub struct Model {
    pub parts: Vec<Part>,
    pub materials: Vec<Material>,
}

/// Triangles sharing one material.
///
/// Readers fill parts in the file's own space and winding (counter-clockwise seen from the front,
/// the convention of glTF, OBJ and STL); [`crate::space::to_unreal`] converts both, after which
/// [`unreal_facing`] tells which way each triangle faces.
#[derive(Clone, Debug, Default)]
pub struct Part {
    pub name: String,
    /// Index into [`Model::materials`], `None` for the default look.
    pub material: Option<usize>,
    pub positions: Vec<[f32; 3]>,
    /// One per position, or empty if the file has none.
    pub uvs: Vec<[f32; 2]>,
    /// Three indices into `positions` per triangle. The game shades triangles that share a vertex
    /// smoothly across it, so a hard edge needs its own vertices. Readers keep the file's vertex
    /// splits, or call [`Part::split_sharp_edges`] when the file has no normals.
    pub corners: Vec<[u32; 3]>,
}

/// How a part looks, as far as a reference needs.
#[derive(Clone, Debug)]
pub struct Material {
    pub name: String,
    /// Linear RGBA, multiplied with the texture if there is one.
    pub color: [f32; 4],
    pub texture: Option<Image>,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            name: String::new(),
            color: [1.0; 4],
            texture: None,
        }
    }
}

/// An axis-aligned box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Bounds {
    pub fn center(&self) -> [f32; 3] {
        [0, 1, 2].map(|axis| (self.min[axis] + self.max[axis]) * 0.5)
    }

    pub fn size(&self) -> [f32; 3] {
        [0, 1, 2].map(|axis| self.max[axis] - self.min[axis])
    }
}

/// The side Unreal draws a triangle on (not of unit length): the opposite of `(b - a) × (c - a)`
/// computed on its coordinates as they are. Put differently, Unreal's front faces are clockwise
/// when X, Y, Z are read as a right-handed space; the editor draws them left-handed (Y is the
/// vehicle's right), where they look counter-clockwise from the front. Checked in game: the
/// collision view's meshes and reference meshes both follow it.
pub fn unreal_facing(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let ab = [0, 1, 2].map(|axis| b[axis] - a[axis]);
    let ac = [0, 1, 2].map(|axis| c[axis] - a[axis]);
    [
        ac[1] * ab[2] - ac[2] * ab[1],
        ac[2] * ab[0] - ac[0] * ab[2],
        ac[0] * ab[1] - ac[1] * ab[0],
    ]
}

impl Part {
    /// This part cut into parts of at most `max_triangles` triangles each, in order, each with only
    /// the positions (and UVs) its triangles use. Large parts are built a piece at a time, so no
    /// single frame does all the work.
    pub fn split(&self, max_triangles: usize) -> Vec<Part> {
        let max_triangles = max_triangles.max(1);
        if self.corners.len() <= max_triangles {
            return vec![self.clone()];
        }
        let none = u32::MAX;
        // Where each of our positions went in the current piece.
        let mut moved = vec![none; self.positions.len()];
        self.corners
            .chunks(max_triangles)
            .map(|triangles| {
                let mut piece = Part {
                    name: self.name.clone(),
                    material: self.material,
                    ..Part::default()
                };
                let mut used = Vec::new();
                for triangle in triangles {
                    let corners = triangle.map(|index| {
                        let slot = &mut moved[index as usize];
                        if *slot == none {
                            *slot = piece.positions.len() as u32;
                            piece.positions.push(self.positions[index as usize]);
                            if !self.uvs.is_empty() {
                                piece.uvs.push(self.uvs[index as usize]);
                            }
                            used.push(index);
                        }
                        *slot
                    });
                    piece.corners.push(corners);
                }
                for index in used {
                    moved[index as usize] = none;
                }
                piece
            })
            .collect()
    }

    /// For a part read without normals: splits vertices shared by triangles that meet at more than
    /// `max_angle` degrees, so those edges look sharp (a box's) while gentler ones stay smooth (a
    /// curved surface's). Vertices no triangle uses are dropped. Every index in `corners` must be
    /// in range.
    pub fn split_sharp_edges(&mut self, max_angle: f32) {
        let min_dot = max_angle.to_radians().cos();
        let normals: Vec<[f32; 3]> = self.corners.iter().map(|t| self.face_normal(t)).collect();
        let smooth = |a: u32, b: u32| {
            let (a, b) = (normals[a as usize], normals[b as usize]);
            // A triangle without an area has no direction: it fits in with any neighbour.
            a == NO_NORMAL || b == NO_NORMAL || dot(a, b) >= min_dot
        };

        // Every edge of every triangle, sorted so the triangles sharing an edge are next to each
        // other. An edge is its two vertices, smaller index first.
        let mut edges: Vec<(u32, u32, u32)> = Vec::with_capacity(self.corners.len() * 3);
        for (triangle, corners) in self.corners.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (corners[k], corners[(k + 1) % 3]);
                if a != b {
                    edges.push((a.min(b), a.max(b), triangle as u32));
                }
            }
        }
        edges.sort_unstable();

        // Corner slots are `triangle * 3 + k`. Slots that end up in one set share a vertex.
        let mut sets = CornerSets::new(self.corners.len() * 3);
        let slot_of = |triangle: u32, vertex: u32| {
            let k = self.corners[triangle as usize]
                .iter()
                .position(|&corner| corner == vertex)
                .expect("an edge's vertex is one of its triangle's");
            triangle as usize * 3 + k
        };
        // More triangles on one edge than this is broken geometry: left sharp rather than compared
        // pair by pair.
        const MAX_ON_ONE_EDGE: usize = 16;
        for shared in edges
            .chunk_by(|a, b| (a.0, a.1) == (b.0, b.1))
            .filter(|shared| shared.len() <= MAX_ON_ONE_EDGE)
        {
            // Usually 2 triangles, so comparing all pairs is cheap.
            for (i, &(low, high, first)) in shared.iter().enumerate() {
                for &(_, _, second) in &shared[i + 1..] {
                    if first != second && smooth(first, second) {
                        sets.join(slot_of(first, low), slot_of(second, low));
                        sets.join(slot_of(first, high), slot_of(second, high));
                    }
                }
            }
        }

        // One vertex per set, numbered as they first appear.
        let mut vertex_of_set = vec![u32::MAX; self.corners.len() * 3];
        let mut positions = Vec::new();
        let mut uvs = Vec::new();
        let mut corners = Vec::with_capacity(self.corners.len());
        for (triangle, old) in self.corners.iter().enumerate() {
            corners.push([0, 1, 2].map(|k| {
                let set = sets.find(triangle * 3 + k);
                if vertex_of_set[set] == u32::MAX {
                    vertex_of_set[set] = positions.len() as u32;
                    positions.push(self.positions[old[k] as usize]);
                    if !self.uvs.is_empty() {
                        uvs.push(self.uvs[old[k] as usize]);
                    }
                }
                vertex_of_set[set]
            }));
        }
        self.positions = positions;
        self.uvs = uvs;
        self.corners = corners;
    }

    /// The direction a triangle faces with length 1, or [`NO_NORMAL`] if it has no area. Which of
    /// the two sides this is does not matter: triangles of a part all wind the same way.
    fn face_normal(&self, corners: &[u32; 3]) -> [f32; 3] {
        let [a, b, c] = corners.map(|index| self.positions[index as usize]);
        let normal = unreal_facing(a, b, c);
        let length = dot(normal, normal).sqrt();
        // A NaN (from a position that is not a number) fails both checks too.
        if length > 0.0 && length.is_finite() {
            normal.map(|axis| axis / length)
        } else {
            NO_NORMAL
        }
    }
}

/// Stands in for the normal of a triangle without an area.
const NO_NORMAL: [f32; 3] = [0.0; 3];

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Disjoint sets of corner slots (union-find): which corners end up sharing a vertex.
struct CornerSets {
    parent: Vec<u32>,
}

impl CornerSets {
    /// `count` sets of one slot each.
    fn new(count: usize) -> Self {
        Self {
            parent: (0..count as u32).collect(),
        }
    }

    /// The slot standing for `slot`'s set: the same for every slot of the set.
    fn find(&mut self, mut slot: usize) -> usize {
        while self.parent[slot] as usize != slot {
            // Skip a level on the way up so later searches are shorter.
            let grandparent = self.parent[self.parent[slot] as usize];
            self.parent[slot] = grandparent;
            slot = grandparent as usize;
        }
        slot
    }

    /// Puts the sets of `a` and `b` together.
    fn join(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        self.parent[a.max(b)] = a.min(b) as u32;
    }
}

impl Model {
    /// Triangles over all parts.
    pub fn triangle_count(&self) -> usize {
        self.parts.iter().map(|part| part.corners.len()).sum()
    }

    /// The box around every position, `None` for a model without any.
    pub fn bounds(&self) -> Option<Bounds> {
        let mut positions = self.parts.iter().flat_map(|part| part.positions.iter());
        let first = *positions.next()?;
        let mut bounds = Bounds {
            min: first,
            max: first,
        };
        for position in positions {
            bounds.min = [0, 1, 2].map(|axis| bounds.min[axis].min(position[axis]));
            bounds.max = [0, 1, 2].map(|axis| bounds.max[axis].max(position[axis]));
        }
        Some(bounds)
    }

    /// Checks what every reader must hand out: indices in range, UVs either absent or one per
    /// position, materials that exist, and no more than [`MAX_TRIANGLES`].
    pub fn validate(&self) -> Result<(), String> {
        if self.triangle_count() > MAX_TRIANGLES {
            return Err(format!(
                "{} triangles, more than the {MAX_TRIANGLES} a reference may have",
                self.triangle_count()
            ));
        }
        for part in &self.parts {
            let count = part.positions.len();
            if !part.uvs.is_empty() && part.uvs.len() != count {
                return Err(format!(
                    "part {:?}: {} UVs for {count} positions",
                    part.name,
                    part.uvs.len()
                ));
            }
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
            if part
                .material
                .is_some_and(|material| material >= self.materials.len())
            {
                return Err(format!("part {:?}: unknown material", part.name));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle() -> Part {
        Part {
            positions: vec![[0.0, 0.0, 0.0], [2.0, 0.0, -1.0], [0.0, 4.0, 3.0]],
            corners: vec![[0, 1, 2]],
            ..Part::default()
        }
    }

    #[test]
    fn bounds_cover_every_part() {
        let mut moved = triangle();
        moved.positions[0] = [-5.0, 0.0, 0.0];
        let model = Model {
            parts: vec![triangle(), moved],
            materials: vec![],
        };
        let bounds = model.bounds().unwrap();
        assert_eq!(bounds.min, [-5.0, 0.0, -1.0]);
        assert_eq!(bounds.max, [2.0, 4.0, 3.0]);
        assert_eq!(bounds.center(), [-1.5, 2.0, 1.0]);
        assert_eq!(Model::default().bounds(), None);
    }

    #[test]
    fn facing_follows_unreal() {
        // Corners going +Y then +Z from the origin: Unreal draws the side facing -X.
        let facing = unreal_facing([0.0; 3], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
        assert_eq!(facing, [-1.0, 0.0, 0.0]);
    }

    #[test]
    fn split_keeps_every_triangle() {
        // A strip of 5 triangles over 7 positions, with UVs.
        let part = Part {
            name: "strip".into(),
            material: Some(2),
            positions: (0..7).map(|i| [i as f32, (i % 2) as f32, 0.0]).collect(),
            uvs: (0..7).map(|i| [i as f32, 0.0]).collect(),
            corners: (0..5).map(|i| [i, i + 1, i + 2]).collect(),
        };
        assert_eq!(part.split(5).len(), 1);
        let pieces = part.split(2);
        assert_eq!(pieces.len(), 3);
        let mut triangles = Vec::new();
        for piece in &pieces {
            assert!(piece.corners.len() <= 2);
            assert_eq!(piece.material, Some(2));
            assert_eq!(piece.uvs.len(), piece.positions.len());
            let model = Model {
                parts: vec![piece.clone()],
                materials: vec![Material::default(); 3],
            };
            model.validate().unwrap();
            triangles.extend(
                piece
                    .corners
                    .iter()
                    .map(|t| t.map(|i| piece.positions[i as usize])),
            );
        }
        let original: Vec<_> = part
            .corners
            .iter()
            .map(|t| t.map(|i| part.positions[i as usize]))
            .collect();
        assert_eq!(triangles, original);
        // The first piece only holds the 4 positions its 2 triangles use.
        assert_eq!(pieces[0].positions.len(), 4);
    }

    /// The unit cube with 8 shared vertices (index = x + 2y + 4z) and 12 triangles, two per face,
    /// all wound the same way. The triangles of face `f` are `2f` and `2f + 1`.
    fn cube() -> Part {
        let quads = [
            [0, 2, 3, 1],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 4, 6, 2],
            [1, 3, 7, 5],
        ];
        let positions: Vec<[f32; 3]> = (0..8)
            .map(|i| [(i & 1) as f32, (i >> 1 & 1) as f32, (i >> 2 & 1) as f32])
            .collect();
        Part {
            uvs: positions
                .iter()
                .map(|p| [p[0] + 2.0 * p[1], p[2]])
                .collect(),
            positions,
            corners: quads
                .iter()
                .flat_map(|&[p, q, r, s]| [[p, q, r], [p, r, s]])
                .collect(),
            ..Part::default()
        }
    }

    /// The triangles of `part` as positions.
    fn triangles(part: &Part) -> Vec<[[f32; 3]; 3]> {
        let corners = part.corners.iter();
        corners
            .map(|t| t.map(|i| part.positions[i as usize]))
            .collect()
    }

    /// A strip bending `bend` degrees at each of its `segments` joints, one unit wide.
    fn curved_strip(segments: usize, bend: f32) -> Part {
        let mut part = Part::default();
        let mut at = [0.0f32, 0.0];
        for i in 0..=segments {
            part.positions.push([at[0], at[1], 0.0]);
            part.positions.push([at[0], at[1], 1.0]);
            let angle = (bend * i as f32).to_radians();
            at = [at[0] + angle.cos(), at[1] + angle.sin()];
        }
        for i in 0..segments as u32 {
            let (a, b, c, d) = (2 * i, 2 * i + 2, 2 * i + 3, 2 * i + 1);
            part.corners.extend([[a, b, c], [a, c, d]]);
        }
        part
    }

    #[test]
    fn cube_without_normals_gets_a_vertex_per_face_corner() {
        let mut part = cube();
        let before = triangles(&part);
        part.split_sharp_edges(SMOOTH_ANGLE);
        assert_eq!(part.positions.len(), 24);
        assert_eq!(part.corners.len(), 12);
        assert_eq!(triangles(&part), before);
        // The two triangles of a face still share their diagonal's two vertices.
        for face in part.corners.chunks(2) {
            let shared = face[0].iter().filter(|i| face[1].contains(i)).count();
            assert_eq!(shared, 2);
        }
        // And no vertex is shared between faces.
        for vertex in 0..24 {
            let faces: Vec<_> = (0..12)
                .filter(|&t| part.corners[t].contains(&vertex))
                .map(|t| t / 2)
                .collect();
            assert!(faces.windows(2).all(|pair| pair[0] == pair[1]));
        }
    }

    #[test]
    fn gentle_bends_stay_smooth() {
        let mut part = curved_strip(4, 10.0);
        let before = triangles(&part);
        assert_eq!(part.positions.len(), 10);
        part.split_sharp_edges(SMOOTH_ANGLE);
        assert_eq!(part.positions.len(), 10);
        assert_eq!(triangles(&part), before);

        // The same strip with a stricter limit is split at its three joints.
        let mut part = curved_strip(4, 10.0);
        part.split_sharp_edges(5.0);
        assert_eq!(part.positions.len(), 16);
        assert_eq!(triangles(&part), before);
    }

    #[test]
    fn splitting_keeps_uvs_with_their_vertices_and_drops_unused_ones() {
        let mut part = cube();
        // A vertex nobody uses, in front so the used ones keep their indices.
        part.positions.insert(0, [9.0, 9.0, 9.0]);
        part.uvs.insert(0, [9.0, 9.0]);
        for index in part.corners.iter_mut().flatten() {
            *index += 1;
        }
        part.split_sharp_edges(SMOOTH_ANGLE);
        assert_eq!(part.positions.len(), 24);
        assert_eq!(part.uvs.len(), 24);
        for (position, uv) in part.positions.iter().zip(&part.uvs) {
            assert_eq!(*uv, [position[0] + 2.0 * position[1], position[2]]);
        }
        let model = Model {
            parts: vec![part],
            materials: vec![],
        };
        assert!(model.validate().is_ok());
    }

    #[test]
    fn triangles_without_area_do_not_split_their_neighbours() {
        // A flat quad plus a sliver on its edge (all three corners on one line).
        let mut part = Part {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.5, 0.0, 0.0],
            ],
            corners: vec![[0, 1, 2], [0, 2, 3], [0, 4, 1]],
            ..Part::default()
        };
        part.split_sharp_edges(SMOOTH_ANGLE);
        assert_eq!(part.positions.len(), 5);
        assert_eq!(part.corners.len(), 3);
    }

    #[test]
    fn validate_catches_broken_parts() {
        let good = Model {
            parts: vec![triangle()],
            materials: vec![],
        };
        assert!(good.validate().is_ok());

        let mut bad_index = good.clone();
        bad_index.parts[0].corners[0][2] = 3;
        assert!(bad_index.validate().is_err());

        let mut bad_uvs = good.clone();
        bad_uvs.parts[0].uvs = vec![[0.0, 0.0]];
        assert!(bad_uvs.validate().is_err());

        let mut bad_material = good;
        bad_material.parts[0].material = Some(0);
        assert!(bad_material.validate().is_err());
    }
}
