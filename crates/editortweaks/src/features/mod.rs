//! Every feature of the mod. To add one: create `features/<name>.rs` with a type implementing
//! [`Feature`], declare the module below and append it to [`ALL`]. Its settings, if any, go in
//! [`SETTINGS`].

use crate::framework::feature::Feature;
use crate::framework::settings::Section;

mod actuator_arrows;
mod brick_resize;
mod camera_speed;
mod editor_bounds;
mod hello;
/// The mod's rebindable keys, which other features read.
mod keybinds;
/// Shared hooks of the editor's mouse movement (not a feature).
mod mouse_moves;
mod move_slowdown;
mod precise_moves;
mod precise_sliders;
mod property_limits;
mod settings_menu;
mod slider_drag;
mod slider_formulas;
mod suspension_arrows;
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
    &editor_bounds::EditorBounds,
    &move_slowdown::MoveSlowdown,
    &precise_moves::PreciseMoves,
    &view_modes::ViewModes,
    &brick_resize::BrickResize,
    &actuator_arrows::ActuatorArrows,
    &suspension_arrows::SuspensionArrows,
    &settings_menu::SettingsMenu,
];

/// Every feature's settings, by section of the menu, in the menu's order (see `settings_menu`).
pub static SETTINGS: &[&[Section]] = &[&brick_resize::SETTINGS, &slider_drag::SETTINGS];
