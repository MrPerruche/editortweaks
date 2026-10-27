//! A vehicle as a model to write to a file: one object per brick, each made of parts that share a
//! look, all in the editor's space (Unreal's axes and centimetres, see [`crate::space`]). The mod
//! fills it from the editor's bricks; [`glb`], [`obj`] and [`stl`] write it ([`Export::join_by_look`]
//! first, for formats where a few meshes are handier than one per brick).

pub mod glb;
pub mod obj;
pub mod stl;
pub mod text;

use crate::image::Image;
use crate::model::{Part, unreal_facing};

/// Everything to write.
#[derive(Clone, Debug, Default)]
pub struct Export {
    /// The vehicle's: the name of the node holding the bricks.
    pub name: String,
    pub objects: Vec<Object>,
    pub looks: Vec<Look>,
    /// The images the looks' textures ([`LookTextures`]) point into.
    pub images: Vec<Image>,
}

/// One brick: its parts are placed already (positions in the editor's space, not the brick's).
/// [`Part::material`] is an index into [`Export::looks`].
#[derive(Clone, Debug, Default)]
pub struct Object {
    pub name: String,
    pub parts: Vec<Part>,
}

/// How a part looks, as glTF's metallic-roughness materials describe it.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub name: String,
    /// Linear RGBA; an alpha below 1 makes the part see-through.
    pub color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    /// Linear RGB light the part gives off, 0 to 1.
    pub emissive: [f32; 3],
    pub textures: LookTextures,
    /// Where the alpha is below this, the part is cut out (a glyph's outline); `None` blends
    /// see-through looks instead.
    pub alpha_cutoff: Option<f32>,
    /// Drawn from behind too (text).
    pub double_sided: bool,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            name: String::new(),
            color: [1.0; 4],
            metallic: 0.0,
            roughness: 0.5,
            emissive: [0.0; 3],
            textures: LookTextures::default(),
            alpha_cutoff: None,
            double_sided: false,
        }
    }
}

/// The textures a look shows, as glTF's metallic-roughness materials use them: indices into
/// [`Export::images`], drawn with the parts' UVs ([`Part::uvs`]), repeating.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LookTextures {
    /// sRGB colour and alpha, multiplied with [`Look::color`].
    pub base_color: Option<usize>,
    /// Linear: roughness in green and metallic in blue, multiplied with [`Look::roughness`] and
    /// [`Look::metallic`].
    pub metallic_roughness: Option<usize>,
    /// Linear tangent-space normals.
    pub normal: Option<usize>,
    /// Linear: ambient occlusion in red.
    pub occlusion: Option<usize>,
    /// sRGB, multiplied with [`Look::emissive`].
    pub emissive: Option<usize>,
}

impl LookTextures {
    /// The images used, with their glTF material slot.
    pub fn used(&self) -> impl Iterator<Item = usize> + '_ {
        [
            self.base_color,
            self.metallic_roughness,
            self.normal,
            self.occlusion,
            self.emissive,
        ]
        .into_iter()
        .flatten()
    }
}

impl Export {
    /// The index of a look equal to `look` in [`Export::looks`], added if there's none yet: bricks
    /// of one colour and material share theirs.
    pub fn look_index(&mut self, look: Look) -> usize {
        match self.looks.iter().position(|known| *known == look) {
            Some(index) => index,
            None => {
                self.looks.push(look);
                self.looks.len() - 1
            }
        }
    }

    /// Replaces the objects with one per look used, named after it (parts without a look go in
    /// "Default"), each made of one part per look: the parts' triangles joined, in the order they
    /// first appear. Parts with UVs and parts without can't share arrays (UVs aren't made up), so
    /// they stay two parts. For a vehicle of thousands of bricks that a program should see as a
    /// few meshes.
    pub fn join_by_look(&mut self) {
        let mut objects: Vec<(Option<usize>, Object)> = Vec::new();
        for part in std::mem::take(&mut self.objects)
            .into_iter()
            .flat_map(|object| object.parts)
        {
            let material = part
                .material
                .filter(|&material| material < self.looks.len());
            let index = match objects.iter().position(|(look, _)| *look == material) {
                Some(index) => index,
                None => {
                    let name = match material {
                        Some(material) => self.looks[material].name.clone(),
                        None => "Default".to_string(),
                    };
                    let object = Object {
                        name,
                        parts: vec![],
                    };
                    objects.push((material, object));
                    objects.len() - 1
                }
            };
            let parts = &mut objects[index].1.parts;
            let textured = !part.uvs.is_empty();
            match parts
                .iter_mut()
                .find(|joined| joined.uvs.is_empty() != textured)
            {
                Some(joined) => append(joined, &part),
                None => parts.push(Part { material, ..part }),
            }
        }
        self.objects = objects.into_iter().map(|(_, object)| object).collect();
    }

    pub fn triangle_count(&self) -> usize {
        self.objects
            .iter()
            .flat_map(|object| &object.parts)
            .map(|part| part.corners.len())
            .sum()
    }
}

/// Adds `part`'s triangles to `target` (both with UVs or both without), its indices moved past
/// the positions already there.
fn append(target: &mut Part, part: &Part) {
    let offset = target.positions.len() as u32;
    target.positions.extend_from_slice(&part.positions);
    target.uvs.extend_from_slice(&part.uvs);
    target.corners.extend(
        part.corners
            .iter()
            .map(|corners| corners.map(|index| index + offset)),
    );
}

/// One normal per position of `part`, pointing to the side Unreal draws (see [`unreal_facing`]):
/// the average of the triangles around it, larger ones counting more. A position no triangle with
/// an area uses gets straight up. Every index in `corners` must be in range.
pub fn vertex_normals(part: &Part) -> Vec<[f32; 3]> {
    let mut sums = vec![[0.0f32; 3]; part.positions.len()];
    for corners in &part.corners {
        let [a, b, c] = corners.map(|index| part.positions[index as usize]);
        // Not normalized: its length is twice the triangle's area.
        let facing = unreal_facing(a, b, c);
        for index in corners {
            let sum = &mut sums[*index as usize];
            for axis in 0..3 {
                sum[axis] += facing[axis];
            }
        }
    }
    sums.into_iter()
        .map(|sum| {
            let length = (sum[0] * sum[0] + sum[1] * sum[1] + sum[2] * sum[2]).sqrt();
            if length > 0.0 && length.is_finite() {
                sum.map(|axis| axis / length)
            } else {
                [0.0, 0.0, 1.0]
            }
        })
        .collect()
}

/// Gives `part` the UVs the game's brick materials draw their textures with: a box projection of
/// its positions (cm, the editor's axes), each vertex along the axis its normal is closest to
/// (`MF_CheapTriplanarProjection`: X wins ties, then Y), times `tiling` (textures per cm). The game
/// picks the axis per pixel: a triangle whose corners pick different ones is smeared here.
pub fn box_uvs(part: &mut Part, tiling: f32) {
    projected_uvs(part, |uv| uv.map(|value| value * tiling));
}

/// Gives `part` the UVs the game's brick materials draw a brick's pattern with: the projection
/// [`box_uvs`] makes, times `scale` (patterns per cm, per axis), moved by `offset`, then turned
/// counter-clockwise by `rotation` radians around the UVs' origin (`MF_RotateVector2D`).
pub fn pattern_uvs(part: &mut Part, scale: [f32; 2], offset: [f32; 2], rotation: f32) {
    let (sin, cos) = rotation.sin_cos();
    projected_uvs(part, |[u, v]| {
        let [x, y] = [u * scale[0] + offset[0], v * scale[1] + offset[1]];
        [x * cos - y * sin, x * sin + y * cos]
    });
}

/// Gives `part` the box projection [`box_uvs`] describes, in cm, made UVs by `place`.
fn projected_uvs(part: &mut Part, place: impl Fn([f32; 2]) -> [f32; 2]) {
    const TOLERANCE: f32 = 0.01;
    let normals = vertex_normals(part);
    part.uvs = part
        .positions
        .iter()
        .zip(&normals)
        .map(|(&[x, y, z], normal)| {
            let [nx, ny, nz] = normal.map(f32::abs);
            let uv = if nx >= ny - TOLERANCE && nx >= nz - TOLERANCE {
                [y, z]
            } else if ny >= nz - TOLERANCE {
                [x, z]
            } else {
                [x, y]
            };
            place(uv)
        })
        .collect();
}

/// The box from `min` to `max` as 12 triangles facing out (Unreal's winding, see
/// [`unreal_facing`]), each side with its own 4 corners so its edges stay sharp.
pub fn box_part(min: [f32; 3], max: [f32; 3]) -> Part {
    let corner = |x: bool, y: bool, z: bool| {
        [
            if x { max[0] } else { min[0] },
            if y { max[1] } else { min[1] },
            if z { max[2] } else { min[2] },
        ]
    };
    let mut part = Part::default();
    // Each side: its axis and which end, then its corners around it.
    for axis in 0..3 {
        for high in [false, true] {
            let corners: [[f32; 3]; 4] = core::array::from_fn(|k| {
                // The two other axes go around the side: (0,0), (1,0), (1,1), (0,1).
                let (u, v) = [(false, false), (true, false), (true, true), (false, true)][k];
                let mut bits = [false; 3];
                bits[axis] = high;
                bits[(axis + 1) % 3] = u;
                bits[(axis + 2) % 3] = v;
                corner(bits[0], bits[1], bits[2])
            });
            let first = part.positions.len() as u32;
            part.positions.extend(corners);
            // Opposite sides go around the other way, to face opposite ways.
            let [b, c, d] = if high { [1, 2, 3] } else { [3, 2, 1] };
            part.corners.push([first, first + b, first + c]);
            part.corners.push([first, first + c, first + d]);
        }
    }
    face_outward(&mut part);
    part
}

/// Turns `part`'s triangles around if they face in: if, all together, they face towards the
/// inside of the volume they enclose (seen as Unreal draws them, see [`unreal_facing`]). For closed
/// shapes, whatever winding their source had (PhysX's, a convex hull's, a mirrored brick's); a
/// shape without a volume (flat) stays as it is.
pub fn face_outward(part: &mut Part) {
    let count = part.positions.len().max(1) as f64;
    let center: [f64; 3] = core::array::from_fn(|axis| {
        part.positions
            .iter()
            .map(|p| f64::from(p[axis]))
            .sum::<f64>()
            / count
    });
    // Each triangle's facing times its distance from the center along it: their sum is the volume
    // (times 2) when they face out, minus the volume when they face in.
    let mut volume = 0.0;
    for corners in &part.corners {
        let [a, b, c] = corners.map(|index| {
            let p = part.positions[index as usize];
            core::array::from_fn::<f64, 3, _>(|axis| f64::from(p[axis]) - center[axis])
        });
        let facing = unreal_facing(
            a.map(|value| value as f32),
            b.map(|value| value as f32),
            c.map(|value| value as f32),
        );
        volume +=
            f64::from(facing[0]) * a[0] + f64::from(facing[1]) * a[1] + f64::from(facing[2]) * a[2];
    }
    if volume < 0.0 {
        for corners in &mut part.corners {
            corners.swap(1, 2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether every triangle of `part` faces away from `center`.
    fn faces_out(part: &Part, center: [f32; 3]) -> bool {
        part.corners.iter().all(|corners| {
            let [a, b, c] = corners.map(|index| part.positions[index as usize]);
            let facing = unreal_facing(a, b, c);
            let middle: [f32; 3] = core::array::from_fn(|axis| (a[axis] + b[axis] + c[axis]) / 3.0);
            (0..3)
                .map(|axis| facing[axis] * (middle[axis] - center[axis]))
                .sum::<f32>()
                > 0.0
        })
    }

    #[test]
    fn boxes_face_out() {
        let part = box_part([0.0, -1.0, 2.0], [4.0, 1.0, 3.0]);
        assert_eq!(part.positions.len(), 24);
        assert_eq!(part.corners.len(), 12);
        assert!(faces_out(&part, [2.0, 0.0, 2.5]));
    }

    #[test]
    fn inward_shapes_are_turned_around() {
        let mut part = box_part([0.0; 3], [1.0; 3]);
        for corners in &mut part.corners {
            corners.swap(0, 1);
        }
        assert!(!faces_out(&part, [0.5; 3]));
        face_outward(&mut part);
        assert!(faces_out(&part, [0.5; 3]));
        // Already out: left as it is.
        let before = part.corners.clone();
        face_outward(&mut part);
        assert_eq!(part.corners, before);
    }

    #[test]
    fn pattern_uvs_scale_move_then_turn() {
        let mut part = box_part([0.0; 3], [100.0, 200.0, 300.0]);
        let mut boxed = part.clone();
        box_uvs(&mut boxed, 1.0);
        let quarter = core::f32::consts::FRAC_PI_2;
        pattern_uvs(&mut part, [0.5, 0.25], [1.0, 2.0], quarter);
        for (uv, [u, v]) in part.uvs.iter().zip(&boxed.uvs) {
            // Scaled and moved: (u / 2 + 1, v / 4 + 2); a quarter turn takes (x, y) to (-y, x).
            let expected = [-(v * 0.25 + 2.0), u * 0.5 + 1.0];
            assert!(
                (uv[0] - expected[0]).abs() < 1e-3 && (uv[1] - expected[1]).abs() < 1e-3,
                "{uv:?} {expected:?}"
            );
        }
    }

    #[test]
    fn box_uvs_follow_the_facing_axis() {
        let mut part = box_part([0.0; 3], [100.0, 200.0, 300.0]);
        box_uvs(&mut part, 0.01);
        assert_eq!(part.uvs.len(), part.positions.len());
        for (position, uv) in part.positions.iter().zip(&part.uvs) {
            let candidates = [
                [position[1], position[2]],
                [position[0], position[2]],
                [position[0], position[1]],
            ]
            .map(|pair| pair.map(|value| value * 0.01));
            assert!(candidates.contains(uv), "{position:?} {uv:?}");
        }
        // The top face (+Z) projects along Z: its UVs are X and Y.
        let tops: Vec<_> = part
            .positions
            .iter()
            .zip(&part.uvs)
            .filter(|(p, _)| p[2] == 300.0)
            .map(|(_, uv)| *uv)
            .collect();
        assert!(tops.contains(&[1.0, 2.0]));
    }

    #[test]
    fn equal_looks_are_shared() {
        let mut export = Export::default();
        let red = Look {
            color: [1.0, 0.0, 0.0, 1.0],
            ..Look::default()
        };
        assert_eq!(export.look_index(red.clone()), 0);
        assert_eq!(export.look_index(Look::default()), 1);
        assert_eq!(export.look_index(red), 0);
        assert_eq!(export.looks.len(), 2);
    }

    #[test]
    fn joining_merges_bricks_of_one_look() {
        let mut export = Export::default();
        let red = export.look_index(Look {
            name: "red".into(),
            ..Look::default()
        });
        let blue = export.look_index(Look {
            name: "blue".into(),
            ..Look::default()
        });
        let brick = |material, x: f32, uvs: bool| {
            let mut part = box_part([x, 0.0, 0.0], [x + 1.0, 1.0, 1.0]);
            part.material = material;
            if uvs {
                box_uvs(&mut part, 1.0);
            }
            part
        };
        let object = |parts| Object {
            name: "brick".into(),
            parts,
        };
        export.objects = vec![
            object(vec![brick(Some(red), 0.0, false), brick(None, 5.0, false)]),
            object(vec![
                brick(Some(red), 2.0, false),
                brick(Some(blue), 3.0, false),
            ]),
            object(vec![brick(Some(red), 4.0, true)]),
        ];
        export.join_by_look();
        let names: Vec<&str> = export.objects.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["red", "Default", "blue"]);
        // Two red bricks joined: 48 positions, 24 triangles, the second's indices moved by 24.
        let red_parts = &export.objects[0].parts;
        assert_eq!(red_parts.len(), 2);
        assert_eq!(red_parts[0].material, Some(red));
        assert_eq!(red_parts[0].positions.len(), 48);
        assert_eq!(red_parts[0].corners.len(), 24);
        assert!(red_parts[0].uvs.is_empty());
        assert!(
            red_parts[0].corners[12..]
                .iter()
                .flatten()
                .all(|&i| i >= 24)
        );
        assert_eq!(red_parts[0].positions[24][0], 2.0);
        // The one with UVs stays apart.
        assert_eq!(red_parts[1].uvs.len(), 24);
        assert_eq!(export.objects[1].parts[0].material, None);
        assert_eq!(export.triangle_count(), 60);
    }

    #[test]
    fn normals_point_to_the_drawn_side() {
        // Two triangles of a flat square at z = 0, wound as Unreal draws them from above.
        let part = Part {
            positions: vec![
                [0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                [5.0, 5.0, 5.0],
            ],
            corners: vec![[0, 1, 2], [0, 2, 3]],
            ..Part::default()
        };
        let facing = unreal_facing(part.positions[0], part.positions[1], part.positions[2]);
        let up = if facing[2] > 0.0 { 1.0 } else { -1.0 };
        let normals = vertex_normals(&part);
        for normal in &normals[..4] {
            assert_eq!(*normal, [0.0, 0.0, up]);
        }
        // Unused.
        assert_eq!(normals[4], [0.0, 0.0, 1.0]);
    }
}
