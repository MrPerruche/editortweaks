//! The names bricks get in the editor. Edit this file to rename bricks or add entries.
//!
//! Each entry is `("class", "name")`. The class is the brick's Blueprint, named as in the game's
//! `Bricks` folder (`BP_ScalableWedge`). The name is what the editor shows everywhere: the brick
//! list, the selection, tooltips. Bricks that show their size ("Brick 2x2x1") still show it after
//! the name.
//!
//! To find a brick's class, run `et.py log` after the editor opened: it lists the bricks that
//! still share a name, with their classes. Entries without a brick (a typo, or a brick the game
//! hasn't loaded yet) are listed there too.
//!
//! This only changes what the editor displays: vehicles, saves and what other players see are
//! untouched.

/// All renamed bricks. `BP_ScalableBrick` and `BP_LightBrick` keep the game's names ("Scalable
/// brick", "Light brick"), which the others of their kind shared.
pub const NAMES: &[(&str, &str)] = &[
    // Scalable bricks.
    ("BP_ScalableButton", "Scalable button"),
    ("BP_ScalableBrick", "Scalable cube"),
    ("BP_ScalableCone", "Scalable cone"),
    ("BP_ScalableConeFlat", "Scalable cone (flat)"),
    ("BP_ScalableConeRounded", "Scalable cone (rounded)"),
    ("BP_ScalableCorner", "Scalable corner"),
    ("BP_ScalableCornerN", "Scalable corner cut"),
    ("BP_ScalableCornerRounded", "Scalable corner (rounded)"),
    (
        "BP_ScalableCornerRoundedN",
        "Scalable corner (inner rounded)",
    ),
    ("BP_ScalableZylinder", "Scalable cylinder"),
    ("BP_ScalableCylinder90R0", "Scalable cylinder 90° (small)"),
    ("BP_ScalableCylinder90R1", "Scalable cylinder 90° (large)"),
    ("BP_ScalableHalfCone", "Scalable half cone"),
    ("BP_ScalableHalfCylinder", "Scalable half cylinder"),
    ("BP_ScalableHemisphere", "Scalable hemisphere"),
    ("BP_ScalablePyramid", "Scalable pyramid"),
    ("BP_ScalablePyramidCorner", "Scalable pyramid corner"),
    (
        "BP_ScalablePyramidCornerRounded",
        "Scalable pyramid corner (rounded)",
    ),
    ("BP_ScalableQuarterCone", "Scalable quarter cone"),
    ("BP_ScalableQuarterSphere", "Scalable quarter sphere"),
    ("BP_ScalableRamp", "Scalable ramp"),
    ("BP_ScalableRampRounded", "Scalable ramp (rounded)"),
    ("BP_ScalableRampRoundedN", "Scalable ramp (inner rounded)"),
    ("BP_ScalableSquareToCircle", "Scalable square to cylinder"),
    (
        "BP_ScalableSquareToQuarterCircle",
        "Scalable square to quarter cylinder",
    ),
    ("BP_ScalableWedge", "Scalable wedge"),
    ("BP_ScalableWedgeCorner", "Scalable wedge corner"),
    // Light bricks.
    ("BP_LightCone", "Light cone"),
    ("BP_LightConeFlat", "Light cone (flat)"),
    ("BP_LightCylinder", "Light cylinder"),
    ("BP_LightHemisphere", "Light hemisphere"),
    ("BP_LightRamp", "Light ramp"),
    ("BP_LightRampRounded", "Light ramp (rounded)"),
    ("BP_LightRampRoundedN", "Light ramp (inner rounded)"),
];

/// The name for the brick Blueprint `class`, if it has an entry.
pub fn of(class: &str) -> Option<&'static str> {
    NAMES
        .iter()
        .find_map(|&(entry, name)| (entry == class).then_some(name))
}
