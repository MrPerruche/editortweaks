//! Every feature of the mod. To add one: create `features/<name>.rs` with a type implementing
//! [`Feature`], declare the module below and append it to [`ALL`]. Its settings, if any, go in
//! [`SETTINGS`].

use crate::framework::feature::Feature;
use crate::framework::settings::{Section, Setting};

mod actuator_arrows;
mod brick_resize;
mod camera_speed;
mod click_swap;
mod drag_select;
mod editor_bounds;
/// Shared hooks of the editor's mouse button actions (not a feature).
mod editor_buttons;
mod editor_frame;
mod hello;
/// The mod's rebindable keys, which other features read.
mod keybinds;
mod map_in_editor;
/// Shared hooks of the editor's mouse movement (not a feature).
mod mouse_moves;
mod move_slowdown;
mod ortho_view;
mod precise_moves;
mod precise_sliders;
mod property_limits;
/// Shared hook of the game's settings list, where features add rows (not a feature).
mod settings_list;
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
    &editor_frame::EditorFrame,
    &ortho_view::OrthoView,
    &editor_bounds::EditorBounds,
    &move_slowdown::MoveSlowdown,
    &precise_moves::PreciseMoves,
    &view_modes::ViewModes,
    &brick_resize::BrickResize,
    &drag_select::DragSelect,
    &click_swap::ClickSwap,
    &actuator_arrows::ActuatorArrows,
    &suspension_arrows::SuspensionArrows,
    &map_in_editor::MapInEditor,
    &settings_menu::SettingsMenu,
];

/// Every feature's settings, by section of the menu, in the menu's order (see `settings_menu`).
pub static SETTINGS: &[&[Section]] = &[
    &SELECTION,
    &brick_resize::SETTINGS,
    &map_in_editor::SETTINGS,
    &slider_drag::SETTINGS,
];

/// The settings of selecting bricks: which buttons, and what a box picks.
static SELECTION: [Section; 1] = [Section {
    title: "Selection",
    settings: &[
        Setting::Toggle(&click_swap::SWAP_CLICKS),
        Setting::Choice(&drag_select::PICKS),
    ],
}];
