//! Every feature of the mod. To add one: create `features/<name>.rs` with a type implementing
//! [`Feature`], declare the module below and append it to [`ALL`].

use crate::framework::feature::Feature;

mod brick_resize;
mod camera_speed;
mod hello;
/// The mod's rebindable keys, which other features read.
mod keybinds;
/// Shared hooks of the editor's mouse movement (not a feature).
mod mouse_moves;
mod move_slowdown;
mod precise_moves;
mod precise_sliders;
mod property_limits;
mod slider_drag;
mod slider_formulas;
mod view_modes;

/// All features, in load order.
pub static ALL: &[&dyn Feature] = &[
    &hello::Hello,
    &keybinds::Keybinds,
    &property_limits::PropertyLimits,
    &precise_sliders::PreciseSliders,
    &slider_formulas::SliderFormulas,
    &slider_drag::SliderDrag,
    &camera_speed::CameraSpeed,
    &move_slowdown::MoveSlowdown,
    &precise_moves::PreciseMoves,
    &view_modes::ViewModes,
    &brick_resize::BrickResize,
];
