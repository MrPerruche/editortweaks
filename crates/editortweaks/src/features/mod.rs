//! Every feature of the mod. To add one: create `features/<name>.rs` with a type implementing
//! [`Feature`], declare the module below and append it to [`ALL`].

use crate::framework::feature::Feature;

mod camera_speed;
mod hello;
mod move_slowdown;
mod precise_sliders;
mod property_limits;
mod slider_drag;
mod slider_formulas;

/// All features, in load order.
pub static ALL: &[&dyn Feature] = &[
    &hello::Hello,
    &property_limits::PropertyLimits,
    &precise_sliders::PreciseSliders,
    &slider_formulas::SliderFormulas,
    &slider_drag::SliderDrag,
    &camera_speed::CameraSpeed,
    &move_slowdown::MoveSlowdown,
];
