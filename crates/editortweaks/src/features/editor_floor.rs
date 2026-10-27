//! The editor's floor grid, sized to what the camera sees.
//!
//! The floor is a textured plane (`ABrickEditor::GridMeshComponent`, a 200 cm square mesh) that
//! `UpdateGridMesh` scales to `EditorBounds * 0.01` once, when it creates the component (BRMK
//! 0xF90E0), so it spans the box. Its material (`M_EditorGrid_Master`, unlit) has fixed sizes in
//! cm (30 cm cells, 900 cm large cells, the origin marker and the arrow towards it), so the plane
//! may be any size. With the bounds raised by `editor_bounds` it would span 200 km, and the grid
//! jitters as the camera moves (single precision). So after every editor tick its scale is the
//! smallest of [`STEP`] times the game's bounds (as in the game for a camera near the vehicle) that
//! reaches as far as the camera sees: its distance from the vehicle's origin plus [`REACH_PER_HEIGHT`]
//! times its height. Farther away you see further, but not tiny details; at huge sizes the grid
//! may jitter again (so does everything else there). The
//! plane stays centred on the origin, where the material draws its marker. Purely visual: the
//! click trace, the camera and the bricks use the raised bounds.

use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;

use crate::features::editor_bounds;
use crate::game::brick_editor::{self, BrickEditor};
use crate::game::editor_object::Vector;
use crate::game::reflection::{self, Getter};
use crate::game::scene_component::{self, SceneComponent};
use crate::prelude::*;

pub struct EditorFloor;

/// What the floor is sized with.
struct Code {
    scene: SceneComponent,
    editor: BrickEditor,
    /// Offset of `GridMeshComponent` in the editor.
    grid: usize,
    /// Offset of `APlayerController::PlayerCameraManager`.
    camera_manager: usize,
    /// `FVector APlayerCameraManager::GetCameraLocation() const`.
    camera_location: Getter<Vector>,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for Code {}
unsafe impl Sync for Code {}

static CODE: OnceLock<Code> = OnceLock::new();

/// `UpdateGridMesh` scales the grid by the bounds times this (the mesh is 200 cm wide).
const SCALE_PER_CM: f32 = 0.01;
/// The floor grows by this factor at a time, so it doesn't change every frame.
const STEP: f32 = 2.0;
/// How much farther than its height the camera sees the floor, as a multiple of the height.
const REACH_PER_HEIGHT: f32 = 30.0;
/// The floor shrinks once what it must reach is under this share of its size, so that a camera
/// at the edge of a step doesn't make it flicker between two.
const SHRINK_BELOW: f32 = 0.4;

impl Feature for EditorFloor {
    fn id(&self) -> &'static str {
        "editor_floor"
    }

    fn on_load(&self) -> LoadResult {
        brick_editor::after_tick(after_tick)
    }

    fn on_engine_init(&self) {
        let found = (|| {
            let editor =
                reflection::find_class("BrickEditor").ok_or("class BrickEditor not found")?;
            let controller = reflection::find_class("PlayerController")
                .ok_or("class PlayerController not found")?;
            let manager = reflection::find_class("PlayerCameraManager")
                .ok_or("class PlayerCameraManager not found")?;
            Ok::<_, String>(Code {
                scene: scene_component::find()?,
                editor: brick_editor::find()?,
                grid: reflection::field_offset(editor, "GridMeshComponent", "ObjectProperty", 8)?,
                camera_manager: reflection::field_offset(
                    controller,
                    "PlayerCameraManager",
                    "ObjectProperty",
                    8,
                )?,
                camera_location: Getter::find(manager, "GetCameraLocation", "StructProperty")?,
            })
        })();
        match found {
            Ok(code) => {
                let _ = CODE.set(code);
                log!("active");
            }
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

/// Resizes the floor to the camera's view when it differs.
fn after_tick(editor: *mut UObjectBase, _delta: f32) {
    let Some(code) = CODE.get() else { return };
    let Some(game) = editor_bounds::game_bounds() else {
        return;
    };
    // SAFETY: `editor` is the live editor ticking, on the game thread; the offset comes from
    // reflection and holds an object pointer.
    let grid = unsafe {
        editor
            .cast::<u8>()
            .add(code.grid)
            .cast::<*mut UObjectBase>()
            .read()
    };
    if grid.is_null() {
        return;
    }
    // SAFETY: `editor` is live (above), on the game thread.
    let controller = unsafe { code.editor.player_controller(editor) };
    if controller.is_null() {
        return;
    }
    // SAFETY: a live controller; the offset comes from reflection and holds an object pointer.
    let manager = unsafe {
        controller
            .cast::<u8>()
            .add(code.camera_manager)
            .cast::<*mut UObjectBase>()
            .read()
    };
    if manager.is_null() {
        return;
    }
    // SAFETY: `grid` and `manager` are live objects (just read), on the game thread.
    unsafe {
        let camera = code.camera_location.call(manager);
        let origin = code.scene.location(grid);
        let now = code.scene.scale(grid);
        let wanted = floor_scale(game, now[0] / SCALE_PER_CM, camera, origin);
        if now.iter().zip(wanted).any(|(a, b)| (a - b).abs() > 1e-4) {
            code.scene.set_scale(grid, wanted);
        }
    }
}

/// The floor's scale for a camera at `camera` over a vehicle origin at `origin`: the game's
/// bounds times the power of [`STEP`] that reaches what the camera sees, or the current one
/// (`half`, the floor's size now along the first axis) while that's still enough and not too much.
fn floor_scale(game: [f32; 3], half: f32, camera: Vector, origin: Vector) -> Vector {
    let [dx, dy, dz] = [0, 1, 2].map(|axis| camera[axis] - origin[axis]);
    let reach = dx.hypot(dy) + REACH_PER_HEIGHT * dz.abs();
    let least = (reach / game[0]).max(reach / game[1]).max(1.0);
    // The smallest step that reaches, or the current size while it's in the comfortable range.
    let current = half / game[0];
    let factor = if current >= least && least > current * SHRINK_BELOW {
        current
    } else {
        STEP.powf(least.log(STEP).ceil())
    };
    [
        game[0] * factor * SCALE_PER_CM,
        game[1] * factor * SCALE_PER_CM,
        game[2] * SCALE_PER_CM,
    ]
}
