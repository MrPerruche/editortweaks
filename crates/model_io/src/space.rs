//! From a file's space to the editor's.
//!
//! Model files are right-handed with counter-clockwise front faces. glTF fixes its axes (+Y up,
//! +Z forward, metres); OBJ and STL don't say, so the user picks them per reference.
//!
//! Unreal is left-handed: X forward, Y right, Z up, in centimetres. Going from one to the other is a
//! reflection of the coordinates (which keeps the shape: only the axes' handedness differs). It also
//! turns each triangle's `(b - a) × (c - a)` around, which is just what Unreal's winding wants (see
//! [`crate::model::unreal_facing`]): the corners stay in their order.

use serde::{Deserialize, Serialize};

use crate::model::{Bounds, Model};

/// An axis and its direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    #[serde(rename = "+X")]
    PosX,
    #[serde(rename = "-X")]
    NegX,
    #[serde(rename = "+Y")]
    PosY,
    #[serde(rename = "-Y")]
    NegY,
    #[serde(rename = "+Z")]
    PosZ,
    #[serde(rename = "-Z")]
    NegZ,
}

impl Axis {
    pub const ALL: [Axis; 6] = [
        Axis::PosX,
        Axis::NegX,
        Axis::PosY,
        Axis::NegY,
        Axis::PosZ,
        Axis::NegZ,
    ];

    /// The unit vector along this axis.
    pub fn vector(self) -> [f32; 3] {
        match self {
            Axis::PosX => [1.0, 0.0, 0.0],
            Axis::NegX => [-1.0, 0.0, 0.0],
            Axis::PosY => [0.0, 1.0, 0.0],
            Axis::NegY => [0.0, -1.0, 0.0],
            Axis::PosZ => [0.0, 0.0, 1.0],
            Axis::NegZ => [0.0, 0.0, -1.0],
        }
    }

    /// Which of X, Y, Z (0, 1, 2) this is, ignoring the direction.
    pub fn index(self) -> usize {
        match self {
            Axis::PosX | Axis::NegX => 0,
            Axis::PosY | Axis::NegY => 1,
            Axis::PosZ | Axis::NegZ => 2,
        }
    }

    /// As written in the panel and the file: "+X", "-Z", ...
    pub fn label(self) -> &'static str {
        match self {
            Axis::PosX => "+X",
            Axis::NegX => "-X",
            Axis::PosY => "+Y",
            Axis::NegY => "-Y",
            Axis::PosZ => "+Z",
            Axis::NegZ => "-Z",
        }
    }
}

/// The length one unit of the file stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Units {
    #[serde(rename = "m")]
    Metres,
    #[serde(rename = "cm")]
    Centimetres,
    #[serde(rename = "mm")]
    Millimetres,
    #[serde(rename = "in")]
    Inches,
    #[serde(rename = "ft")]
    Feet,
}

impl Units {
    pub const ALL: [Units; 5] = [
        Units::Metres,
        Units::Centimetres,
        Units::Millimetres,
        Units::Inches,
        Units::Feet,
    ];

    /// Centimetres (Unreal units) per unit of the file.
    pub fn centimetres(self) -> f32 {
        match self {
            Units::Metres => 100.0,
            Units::Centimetres => 1.0,
            Units::Millimetres => 0.1,
            Units::Inches => 2.54,
            Units::Feet => 30.48,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Units::Metres => "m",
            Units::Centimetres => "cm",
            Units::Millimetres => "mm",
            Units::Inches => "in",
            Units::Feet => "ft",
        }
    }
}

/// Which point of the model ends up at the reference's position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pivot {
    /// The file's own origin.
    Origin,
    /// The middle of the model's box.
    Center,
    /// The middle of the box's bottom face (the model stands on the position).
    Bottom,
}

/// How to read a model's coordinates: per reference, saved in the sidecar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportSettings {
    /// The file's up axis.
    pub up: Axis,
    /// The axis the model's front faces in the file: it becomes the editor's forward (+X).
    pub forward: Axis,
    pub units: Units,
    pub pivot: Pivot,
}

impl Default for ImportSettings {
    /// glTF's fixed space, which is also what Blender's OBJ export writes by default.
    fn default() -> Self {
        Self {
            up: Axis::PosY,
            forward: Axis::PosZ,
            units: Units::Metres,
            pivot: Pivot::Origin,
        }
    }
}

impl ImportSettings {
    /// STL files mostly come from CAD and 3D printing: Z up, millimetres, front towards -Y
    /// (Blender's and most CAD tools' front view).
    pub fn stl() -> Self {
        Self {
            up: Axis::PosZ,
            forward: Axis::NegY,
            units: Units::Millimetres,
            pivot: Pivot::Origin,
        }
    }

    /// These settings with `up` as the up axis; forward moves to another axis if it was on that one.
    pub fn with_up(self, up: Axis) -> Self {
        let forward = Self::other_than(self.forward, up);
        Self {
            up,
            forward,
            ..self
        }
    }

    /// These settings with `forward` as the forward axis; up moves to another axis if it was on that
    /// one.
    pub fn with_forward(self, forward: Axis) -> Self {
        let up = Self::other_than(self.up, forward);
        Self {
            up,
            forward,
            ..self
        }
    }

    /// `axis`, or if it's on the same axis as `taken` the first axis that isn't.
    fn other_than(axis: Axis, taken: Axis) -> Axis {
        if axis.index() != taken.index() {
            return axis;
        }
        Axis::ALL
            .into_iter()
            .find(|other| other.index() != taken.index())
            .unwrap_or(axis)
    }

    /// Up and forward must be different axes, or there's no way to tell left from right.
    pub fn check(&self) -> Result<(), String> {
        if self.up.index() == self.forward.index() {
            return Err(format!(
                "up ({}) and forward ({}) are on the same axis",
                self.up.label(),
                self.forward.label()
            ));
        }
        Ok(())
    }
}

/// Moves `model` from its file's space into the editor's (see the module docs), then puts its
/// pivot at the origin.
pub fn to_unreal(mut model: Model, settings: &ImportSettings) -> Result<Model, String> {
    settings.check()?;
    let forward = settings.forward.vector();
    let up = settings.up.vector();
    // Right-handed: forward x up points right.
    let right = cross(forward, up);
    let scale = settings.units.centimetres();

    for part in &mut model.parts {
        for position in &mut part.positions {
            let p = *position;
            *position = [
                dot(p, forward) * scale,
                dot(p, right) * scale,
                dot(p, up) * scale,
            ];
        }
    }

    if let Some(bounds) = model.bounds() {
        let offset = pivot_point(&bounds, settings.pivot);
        for position in model
            .parts
            .iter_mut()
            .flat_map(|part| part.positions.iter_mut())
        {
            for axis in 0..3 {
                position[axis] -= offset[axis];
            }
        }
    }
    Ok(model)
}

/// Where `pivot` is on a model with `bounds` (in Unreal space: Z up).
fn pivot_point(bounds: &Bounds, pivot: Pivot) -> [f32; 3] {
    match pivot {
        Pivot::Origin => [0.0; 3],
        Pivot::Center => bounds.center(),
        Pivot::Bottom => {
            let [x, y, _] = bounds.center();
            [x, y, bounds.min[2]]
        }
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Part;

    #[test]
    fn changing_one_axis_keeps_the_other_apart() {
        let settings = ImportSettings::default(); // +Y up, +Z forward
        let up_z = settings.with_up(Axis::NegZ);
        assert_eq!((up_z.up, up_z.forward), (Axis::NegZ, Axis::PosX));
        assert!(up_z.check().is_ok());
        let kept = settings.with_up(Axis::PosX);
        assert_eq!((kept.up, kept.forward), (Axis::PosX, Axis::PosZ));
        let forward_y = settings.with_forward(Axis::NegY);
        assert_eq!((forward_y.up, forward_y.forward), (Axis::PosX, Axis::NegY));
        assert_eq!(forward_y.units, settings.units);
    }

    fn model(positions: Vec<[f32; 3]>) -> Model {
        let corners = (0..positions.len() as u32 / 3)
            .map(|t| [t * 3, t * 3 + 1, t * 3 + 2])
            .collect();
        Model {
            parts: vec![Part {
                positions,
                corners,
                ..Part::default()
            }],
            materials: vec![],
        }
    }

    fn converted(positions: Vec<[f32; 3]>, settings: ImportSettings) -> Model {
        to_unreal(model(positions), &settings).unwrap()
    }

    #[test]
    fn gltf_axes_and_metres() {
        // glTF: +Z forward, +Y up, -X right (its spec), metres.
        let out = converted(
            vec![[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]],
            ImportSettings::default(),
        );
        assert_eq!(
            out.parts[0].positions,
            vec![[100.0, 0.0, 0.0], [0.0, 0.0, 100.0], [0.0, 100.0, 0.0]]
        );
    }

    #[test]
    fn stl_axes_and_millimetres() {
        // Z up, front towards -Y: a point 10 mm in front and 5 mm up.
        let out = converted(
            vec![[0.0, -10.0, 5.0], [10.0, 0.0, 0.0], [0.0, 0.0, 0.0]],
            ImportSettings::stl(),
        );
        assert_eq!(out.parts[0].positions[0], [1.0, 0.0, 0.5]);
        // Looking at the model's front (from -Y) with Z up, +X is on the viewer's right: the
        // model's left, so the editor's -Y.
        assert_eq!(out.parts[0].positions[1], [0.0, -1.0, 0.0]);
    }

    /// A triangle facing the file's front (counter-clockwise from there, right-handed) must face
    /// the editor's front (+X) as Unreal draws it.
    #[test]
    fn front_faces_stay_in_front() {
        for settings in [ImportSettings::default(), ImportSettings::stl()] {
            let f = settings.forward.vector();
            let u = settings.up.vector();
            let r = cross(f, u);
            // Right-handed: from u to r is counter-clockwise seen from f.
            let file = vec![[0.0; 3], u, r];
            assert_eq!(cross(u, r), f);
            let out = converted(file, settings);

            let part = &out.parts[0];
            let [a, b, c] = part.corners[0].map(|i| part.positions[i as usize]);
            let facing = crate::model::unreal_facing(a, b, c);
            assert!(
                facing[0] > 0.0 && facing[1] == 0.0 && facing[2] == 0.0,
                "{settings:?}: faces {facing:?}"
            );
        }
    }

    #[test]
    fn pivots() {
        let positions = vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 4.0, 6.0]];
        let center = ImportSettings {
            pivot: Pivot::Center,
            ..ImportSettings::default()
        };
        let bounds = converted(positions.clone(), center).bounds().unwrap();
        assert_eq!(bounds.center(), [0.0, 0.0, 0.0]);

        let bottom = ImportSettings {
            pivot: Pivot::Bottom,
            ..ImportSettings::default()
        };
        let bounds = converted(positions, bottom).bounds().unwrap();
        assert_eq!(bounds.min[2], 0.0);
        assert_eq!([bounds.center()[0], bounds.center()[1]], [0.0, 0.0]);
    }

    #[test]
    fn same_axis_is_an_error() {
        let settings = ImportSettings {
            up: Axis::PosZ,
            forward: Axis::NegZ,
            ..ImportSettings::default()
        };
        assert!(to_unreal(Model::default(), &settings).is_err());
    }

    #[test]
    fn settings_in_toml() {
        let text = toml::to_string(&ImportSettings::stl()).unwrap();
        assert!(text.contains("up = \"+Z\""), "{text}");
        assert!(text.contains("units = \"mm\""), "{text}");
        // Missing keys fall back to the defaults.
        let read: ImportSettings = toml::from_str("units = \"in\"").unwrap();
        assert_eq!(
            read,
            ImportSettings {
                units: Units::Inches,
                ..ImportSettings::default()
            }
        );
    }
}
