//! Bindings to Brick Rigs and Unreal Engine that brickrust doesn't provide.
//!
//! Each module documents what it binds and how it was found (BRMK export name, pattern, ...),
//! so it can be found again after a game update. Prefer reflection names over hardcoded offsets.

pub mod brick_property;
pub mod editor_input;
pub mod numeric_property;
pub mod property_widget;
pub mod reflection;
pub mod scalable_brick;
pub mod slate;
pub mod slider;
pub mod slider_drag;
pub mod slider_text;
pub mod text;
pub mod ui_library;
pub mod view_target;
pub mod weak_object;
