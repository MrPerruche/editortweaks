//! `UScalableBrick`: bricks with an editable size (the `BrickSize` vector property, in cm).
//!
//! Besides the property's own range, the brick constrains its size whenever `BrickSize` is modified
//! ([`POST_MODIFY`]; not on load, so vehicles keep
//! whatever size they were saved with) in `ConstraintBrickSize`, found through BRMK 1.11.6's export
//! `?ConstraintBrickSize@UScalableBrick@@MEAAXXZ` (no subclass overrides it). Game 1.11.7: RVA
//! 0xD2D190, `BrickSize` at +0x100 (BRMK: +0x108).
//!
//! ```text
//! void ConstraintBrickSize() {
//!     BrickSize = Max(BrickSize, GetMinBrickSize());   // virtual, per brick type
//!     BrickSize = Min(BrickSize, GetMaxBrickSize());   // virtual; UScalableBrick: 9000 per axis
//! }
//! ```

use core::ffi::c_void;

use brickrust::ue::fproperty::FProperty;

use crate::framework::sig::FunctionSig;
use crate::game::reflection;

pub const CONSTRAINT_BRICK_SIZE: FunctionSig = FunctionSig {
    name: "UScalableBrick::ConstraintBrickSize",
    // The three `maxss` against BrickSize's components (field offsets wildcarded).
    pattern: "f3 44 0f 5f 83 ?? ?? 00 00 f3 0f 5f b3 ?? ?? 00 00 f3 0f 5f bb ?? ?? 00 00",
};

pub type ConstraintBrickSizeFn = unsafe extern "C" fn(brick: *mut c_void);

/// `UScalableBrick::PostModifyBrickProperty` (game RVA 0xD4F560; BRMK export 0x1672A0), see
/// `game::brick_property`. Subclasses that override it call it first (e.g. `ULightBrick`'s, game
/// 0xD4F4E0, then updates its brightness), so it sees every scalable brick's changes:
///
/// ```text
/// SizeChanged = IsPropertyChanged(Event, "BrickSize");
/// SpacingChanged = IsPropertyChanged(Event, "ConnectorSpacing");
/// if (SizeChanged) ConstraintBrickSize();             // virtual +0x4F0
/// if (SizeChanged || SpacingChanged) { mirror them onto the mirrored brick; rebuild mesh, ... }
/// UBrick::PostModifyBrickProperty(Event);
/// ```
pub const POST_MODIFY: FunctionSig = FunctionSig {
    name: "UScalableBrick::PostModifyBrickProperty",
    // `sete` of the spacing check, then the size check calling ConstraintBrickSize.
    pattern: "41 0f 94 c7 49 8d 4d ?? 45 84 f6 74 ?? 49 8b 45 ?? ff 90 ?? ?? 00 00",
};

/// The `BrickSize` property of scalable bricks (an `FVector`: three floats).
pub fn brick_size_field() -> Result<*const FProperty, String> {
    let class = reflection::find_class("ScalableBrick").ok_or("class ScalableBrick not found")?;
    reflection::field(class, "BrickSize", "StructProperty", 12)
}

/// Every face connector spacing at its densest ([`connector_spacing`] gives 10 cm on all faces).
pub const DENSEST_CONNECTOR_SPACING: u16 = 0x0FFF;

/// The connector spacing of one face, in cm (`None` for a face without connectors).
///
/// `spacing` is the brick's `ConnectorSpacing` (`FScalableBrickConnectorSpacing`, 16 bits): two bits
/// per face at bit `4 * axis + 2 * positive` (`UBrickStatics::GetScalableBrickConnectorSpacingAxis`),
/// holding an `EConnectorSpacing`: 0 None, 1 Default (30 cm), 2 Half (15 cm), 3 Third (10 cm)
/// (distances from the game's `EConnectorSpacing` to distance switch).
pub fn connector_spacing(spacing: u16, axis: usize, positive: bool) -> Option<f32> {
    let shift = 4 * axis + if positive { 2 } else { 0 };
    match (spacing >> shift) & 3 {
        1 => Some(30.0),
        2 => Some(15.0),
        3 => Some(10.0),
        _ => None,
    }
}

/// How many connector points a box-shaped brick of `size` (cm) gets with `spacing` (see
/// [`connector_spacing`]); other shapes get fewer. Each face gets `max(1, round(side / spacing))`
/// points along each of its sides (`UScalableBrickBase::AddGeneratedConnector`).
pub fn connector_points(size: [f32; 3], spacing: u16) -> f64 {
    let along = |side: f32, spacing: f32| (f64::from(side) / f64::from(spacing)).round().max(1.0);
    let mut points = 0.0;
    for axis in 0..3 {
        // A face across `axis` spans the two other axes.
        let (a, b) = (size[(axis + 1) % 3], size[(axis + 2) % 3]);
        for positive in [false, true] {
            if let Some(spacing) = connector_spacing(spacing, axis, positive) {
                points += along(a, spacing) * along(b, spacing);
            }
        }
    }
    points
}
