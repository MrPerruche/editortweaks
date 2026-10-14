//! Bindings to Brick Rigs and Unreal Engine that brickrust doesn't provide.
//!
//! Each module documents what it binds and how it was found (BRMK export name, pattern, ...),
//! so it can be found again after a game update. Prefer reflection names over hardcoded offsets.

pub mod actuator;
pub mod axle;
pub mod brick_editor;
pub mod brick_materials;
pub mod brick_model;
pub mod brick_property;
pub mod collision;
pub mod editor_input;
pub mod editor_mode;
pub mod editor_object;
pub mod editor_static_info;
pub mod editor_view;
pub mod input_table;
pub mod kismet;
pub mod line_batcher;
pub mod menu;
pub mod mesh_builder;
pub mod mesh_component;
pub mod mirror_mode;
pub mod move_mode;
pub mod numeric_property;
pub mod player_input;
pub mod property_reflection;
pub mod property_widget;
pub mod reflection;
pub mod scalable_brick;
pub mod slate;
pub mod slider;
pub mod slider_drag;
pub mod slider_text;
pub mod text;
pub mod ui_library;
pub mod ui_style;
pub mod view_target;
pub mod viewport;
pub mod weak_object;
pub mod world;
