//! The flat rectangle a reference image is shown on, in Unreal space (X forward, Y right, Z up,
//! centimetres, Unreal's winding: [`crate::model::unreal_facing`]).
//!
//! Unrotated, the picture stands upright facing forward (+X): someone in front of the vehicle,
//! looking back at it, sees it the right way round. The reference's rotation turns it from there
//! (a side view is a quarter turn of yaw, a top view a quarter turn of pitch). Two-sided, the back
//! shows the picture mirrored, as if seen through the paper.

use serde::{Deserialize, Serialize};

use crate::model::Part;

/// Which point of the picture is at the reference's position.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImagePivot {
    #[default]
    Center,
    /// The bottom left corner, as seen from the front.
    Corner,
}

/// How the picture is laid out on its rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaneLayout {
    /// Width in centimetres; the height follows from `aspect`.
    pub width: f32,
    /// The picture's width over its height.
    pub aspect: f32,
    /// Mirror the picture left-right, or upside down.
    pub flip_x: bool,
    pub flip_y: bool,
    pub pivot: ImagePivot,
    /// Also triangles facing back (for materials drawing one side only).
    pub double_sided: bool,
}

/// Two triangles showing the whole picture, as described in the module docs.
pub fn image_plane(layout: &PlaneLayout) -> Part {
    let width = layout.width;
    let height = if layout.aspect > 0.0 {
        width / layout.aspect
    } else {
        width
    };
    // From the front, the viewer's right is the vehicle's left (-Y).
    let (left, right) = match layout.pivot {
        ImagePivot::Center => (width / 2.0, -width / 2.0),
        ImagePivot::Corner => (0.0, -width),
    };
    let (bottom, top) = match layout.pivot {
        ImagePivot::Center => (-height / 2.0, height / 2.0),
        ImagePivot::Corner => (0.0, height),
    };
    let u = |u: f32| if layout.flip_x { 1.0 - u } else { u };
    // Texture rows go down.
    let v = |v: f32| if layout.flip_y { 1.0 - v } else { v };
    Part {
        name: "image".into(),
        material: None,
        // Top left, top right, bottom right, bottom left, as seen from the front.
        positions: vec![
            [0.0, left, top],
            [0.0, right, top],
            [0.0, right, bottom],
            [0.0, left, bottom],
        ],
        uvs: vec![
            [u(0.0), v(0.0)],
            [u(1.0), v(0.0)],
            [u(1.0), v(1.0)],
            [u(0.0), v(1.0)],
        ],
        corners: if layout.double_sided {
            [FRONT, BACK].concat()
        } else {
            FRONT.to_vec()
        },
    }
}

/// The rectangle's triangles facing forward, and the same ones facing back.
const FRONT: [[u32; 3]; 2] = [[0, 2, 1], [0, 3, 2]];
const BACK: [[u32; 3]; 2] = [[0, 1, 2], [0, 2, 3]];

/// What a picture shows the vehicle from: a blueprint's views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
}

impl View {
    pub const ALL: [View; 6] = [
        View::Front,
        View::Back,
        View::Left,
        View::Right,
        View::Top,
        View::Bottom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            View::Front => "Front",
            View::Back => "Back",
            View::Left => "Left",
            View::Right => "Right",
            View::Top => "Top",
            View::Bottom => "Bottom",
        }
    }

    /// The rotation (pitch, yaw, roll in degrees) turning the picture to face someone looking at
    /// the vehicle from this side, the right way round (top views with the vehicle's front up).
    pub fn rotation(self) -> [f64; 3] {
        match self {
            View::Front => [0.0, 0.0, 0.0],
            View::Back => [0.0, 180.0, 0.0],
            View::Left => [0.0, -90.0, 0.0],
            View::Right => [0.0, 90.0, 0.0],
            View::Top => [90.0, 0.0, 180.0],
            View::Bottom => [-90.0, 0.0, 0.0],
        }
    }

    /// The axis the viewer looks along (0, 1, 2 for X, Y, Z) and the side they stand on: +1 on the
    /// positive side (the front is +X, the right +Y, the top +Z).
    fn viewer(self) -> (usize, f64) {
        match self {
            View::Front => (0, 1.0),
            View::Back => (0, -1.0),
            View::Right => (1, 1.0),
            View::Left => (1, -1.0),
            View::Top => (2, 1.0),
            View::Bottom => (2, -1.0),
        }
    }

    /// The axis across the picture, left to right.
    fn across(self) -> usize {
        match self {
            View::Front | View::Back | View::Top | View::Bottom => 1,
            View::Left | View::Right => 0,
        }
    }

    /// Where a picture of this view goes for a vehicle in the box `[min, max]` (none: an empty
    /// vehicle at the origin): behind the vehicle as the viewer sees it (so it shows through
    /// it in an orthographic view), `gap` cm from the box, centred on it, with the rotation of
    /// [`View::rotation`] and a width spanning the box across the picture (if it has one).
    pub fn placement(self, bounds: Option<[[f64; 3]; 2]>, gap: f64) -> Placed {
        let [min, max] = bounds.unwrap_or([[0.0; 3]; 2]);
        let mut position: [f64; 3] = core::array::from_fn(|axis| (min[axis] + max[axis]) / 2.0);
        let (axis, side) = self.viewer();
        position[axis] = if side > 0.0 {
            min[axis] - gap
        } else {
            max[axis] + gap
        };
        let across = self.across();
        let width = max[across] - min[across];
        Placed {
            position,
            rotation: self.rotation(),
            width: (bounds.is_some() && width > 0.0).then_some(width),
        }
    }
}

/// A picture's place from [`View::placement`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub position: [f64; 3],
    pub rotation: [f64; 3],
    /// The width to give the picture, if the vehicle has a size to go by.
    pub width: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> PlaneLayout {
        PlaneLayout {
            width: 200.0,
            aspect: 2.0,
            flip_x: false,
            flip_y: false,
            pivot: ImagePivot::Center,
            double_sided: false,
        }
    }

    #[test]
    fn size_follows_the_aspect() {
        let part = image_plane(&layout());
        assert_eq!(part.positions[0], [0.0, 100.0, 50.0]);
        assert_eq!(part.positions[2], [0.0, -100.0, -50.0]);
    }

    #[test]
    fn corner_pivot_starts_at_the_origin() {
        let part = image_plane(&PlaneLayout {
            pivot: ImagePivot::Corner,
            ..layout()
        });
        assert_eq!(part.positions[3], [0.0, 0.0, 0.0]);
        assert_eq!(part.positions[1], [0.0, -200.0, 100.0]);
    }

    fn facings(part: &Part) -> Vec<f32> {
        part.corners
            .iter()
            .map(|corners| {
                let [a, b, c] = corners.map(|i| part.positions[i as usize]);
                let facing = crate::model::unreal_facing(a, b, c);
                assert_eq!([facing[1], facing[2]], [0.0, 0.0]);
                facing[0].signum()
            })
            .collect()
    }

    #[test]
    fn faces_forward_and_upright() {
        let part = image_plane(&layout());
        assert_eq!(facings(&part), [1.0, 1.0]);
        // The picture's top left pixel is at the top left of someone in front, looking back at
        // the vehicle: the vehicle's right (+Y) is on their left.
        assert_eq!(part.uvs[0], [0.0, 0.0]);
        assert!(part.positions[0][1] > 0.0 && part.positions[0][2] > 0.0);
    }

    #[test]
    fn double_sided_adds_the_back() {
        let part = image_plane(&PlaneLayout {
            double_sided: true,
            ..layout()
        });
        assert_eq!(facings(&part), [1.0, 1.0, -1.0, -1.0]);
    }

    /// Unreal's `FRotationMatrix` axes for a rotation (pitch, yaw, roll in degrees): where the
    /// picture's facing (+X), left edge (+Y) and top (+Z) end up.
    fn turned(rotation: [f64; 3]) -> [[f64; 3]; 3] {
        let round = |v: [f64; 3]| v.map(|x| (x * 1e9).round() / 1e9 + 0.0);
        crate::pick::rotation_axes(rotation).map(round)
    }

    /// For someone looking from a side (`toward` them, with `up` up), the picture must face them,
    /// stand upright, and have its left edge on their left: up × facing (Unreal's right is
    /// `up × forward` and they look along `-toward`).
    #[test]
    fn views_face_the_viewer_the_right_way_round() {
        let cases = [
            (View::Front, [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            (View::Back, [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            (View::Right, [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            (View::Left, [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]),
            (View::Top, [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
            (View::Bottom, [0.0, 0.0, -1.0], [1.0, 0.0, 0.0]),
        ];
        for (view, toward, up) in cases {
            let [facing, left, top] = turned(view.rotation());
            // The viewer's left is -(up × forward) = up × toward.
            let viewer_left = [
                up[1] * toward[2] - up[2] * toward[1],
                up[2] * toward[0] - up[0] * toward[2],
                up[0] * toward[1] - up[1] * toward[0],
            ];
            assert_eq!(facing, toward, "{view:?} facing");
            assert_eq!(top, up, "{view:?} top");
            assert_eq!(left, viewer_left, "{view:?} left");
        }
    }

    #[test]
    fn views_go_behind_the_vehicle() {
        let bounds = Some([[-100.0, -50.0, 0.0], [300.0, 50.0, 120.0]]);
        let front = View::Front.placement(bounds, 10.0);
        assert_eq!(front.position, [-110.0, 0.0, 60.0]);
        assert_eq!(front.width, Some(100.0));
        let right = View::Right.placement(bounds, 10.0);
        assert_eq!(right.position, [100.0, -60.0, 60.0]);
        assert_eq!(right.width, Some(400.0));
        let top = View::Top.placement(bounds, 10.0);
        assert_eq!(top.position, [100.0, 0.0, -10.0]);
        let empty = View::Back.placement(None, 10.0);
        assert_eq!(empty.position, [10.0, 0.0, 0.0]);
        assert_eq!(empty.width, None);
    }

    #[test]
    fn flips() {
        let part = image_plane(&PlaneLayout {
            flip_x: true,
            flip_y: true,
            ..layout()
        });
        assert_eq!(part.uvs[0], [1.0, 1.0]);
    }
}
