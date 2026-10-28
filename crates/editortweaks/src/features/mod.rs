//! Every feature of the mod. To add one: create `features/<name>.rs` with a type implementing
//! [`Feature`], declare the module below and append it to [`ALL`]. Its settings, if any, go in
//! [`SETTINGS`].

use crate::framework::feature::Feature;
use crate::framework::settings::{Section, Setting};

mod actuator_arrows;
mod axis_permutations;
mod brick_names;
mod brick_resize;
mod camera_speed;
mod click_swap;
mod compact_panel;
mod connector_markers;
/// Coop building, only in builds with the `coop` cargo feature.
#[cfg(feature = "coop")]
mod coop;
mod copy_properties;
mod drag_select;
mod editor_bounds;
/// Shared hooks of the editor's mouse button actions (not a feature).
mod editor_buttons;
mod editor_floor;
mod editor_frame;
mod editor_lighting;
mod editor_menu;
mod free_pitch;
mod gun_preview;
mod hello;
/// The mod's rebindable keys, which other features read.
mod keybinds;
mod map_in_editor;
/// Shared hook of clicks on the game's menu buttons, where features add buttons (not a feature).
mod menu_buttons;
mod model_export;
/// Shared hooks of the editor's mouse movement (not a feature).
mod mouse_moves;
mod move_slowdown;
/// Shared hook of property panels' numeric rows updating (not a feature).
mod number_rows;
mod ortho_view;
mod precise_moves;
mod precise_sliders;
mod property_limits;
mod references;
/// Shared hook of the game's settings list, where features add rows (not a feature).
mod settings_list;
mod settings_menu;
mod slider_drag;
mod slider_formulas;
mod spawn_mode;
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
    &axis_permutations::AxisPermutations,
    &camera_speed::CameraSpeed,
    &free_pitch::FreePitch,
    &editor_frame::EditorFrame,
    &ortho_view::OrthoView,
    &editor_bounds::EditorBounds,
    &editor_floor::EditorFloor,
    &editor_lighting::EditorLighting,
    &move_slowdown::MoveSlowdown,
    &precise_moves::PreciseMoves,
    &view_modes::ViewModes,
    &brick_names::BrickNames,
    &brick_resize::BrickResize,
    &copy_properties::CopyProperties,
    &drag_select::DragSelect,
    &click_swap::ClickSwap,
    &actuator_arrows::ActuatorArrows,
    &suspension_arrows::SuspensionArrows,
    &connector_markers::ConnectorMarkers,
    &gun_preview::GunPreview,
    &map_in_editor::MapInEditor,
    &references::References,
    &compact_panel::CompactPanel,
    &spawn_mode::SpawnMode,
    &settings_menu::SettingsMenu,
    &model_export::ModelExport,
    &editor_menu::EditorMenu,
    #[cfg(feature = "coop")]
    &coop::Coop,
];

/// Every feature's settings, by section of the menu, in the menu's order (see `settings_menu`).
pub static SETTINGS: &[&[Section]] = &[
    &EDITOR,
    &brick_resize::SETTINGS,
    &references::SETTINGS,
    &model_export::SETTINGS,
    &PREVIEWS,
    #[cfg(feature = "coop")]
    &coop::SETTINGS,
];

/// The settings of the editor itself: which buttons select, the camera's pitch, what a box picks,
/// how selected bricks' connectors show, how far sliders drag, the value order button, how compact
/// the side panel is.
static EDITOR: [Section; 1] = [Section {
    title: "Editor",
    settings: &[
        Setting::Toggle(&click_swap::SWAP_CLICKS),
        Setting::Choice(&free_pitch::UPSIDE_DOWN),
        Setting::Choice(&drag_select::PICKS),
        Setting::Toggle(&connector_markers::AXIS_COLORS),
        Setting::Toggle(&slider_drag::LIFT_SLIDER_LIMITS),
        Setting::Toggle(&axis_permutations::ORDER_BUTTON),
        Setting::Choice(&compact_panel::COMPACT_PANEL),
    ],
    // The side panel's "Spawn mode" and "Disable lighting" rows set them.
    unlisted: &[
        Setting::Choice(&spawn_mode::SPAWN_MODE),
        Setting::Toggle(&editor_lighting::NO_LIGHTING),
    ],
}];

/// The settings of what the editor previews: where shots go, the tooltips of the shot preview and
/// the view modes, and the Physics view mode's centers and lift and drag.
static PREVIEWS: [Section; 1] = [Section {
    title: "Shot preview and view modes",
    settings: &[
        Setting::Choice(&gun_preview::SHOWS),
        Setting::Toggle(&gun_preview::HITS_VEHICLE),
        Setting::Choice(&view_modes::TOOLTIPS),
        Setting::Toggle(&view_modes::CENTERS),
        Setting::Choice(&view_modes::LIFT_DRAG),
    ],
    unlisted: &[],
}];
