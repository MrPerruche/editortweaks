//! With a camera brick selected, a button in the editor's bottom bar ("Go to camera view") puts the
//! editor's camera where that camera brick sees from: its place and angle, as the game asks the brick
//! for them (`IViewTargetInterface::GetCameraViewPoint`, its lens included), and its field of view
//! through the editor's zoom. See `docs/re/editor-camera.md`, "A camera brick's view". The button is a
//! keybind ([`keybinds::CAMERA_VIEW`], no key by default) and an entry of the Editor tweaks menu, grey
//! without a camera brick selected (`editor_menu`).
//!
//! The editor's field of view is the player's times its zoom's factor, so one wider than the player's
//! unzoomed view can't be matched: the zoom then stays at none. The camera stays inside the editor's
//! bounds. Only this player's view changes: the vehicle and the other players are never touched.

use core::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Once, OnceLock};

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UClass;

use super::{editor_frame, editor_menu, keybinds, ortho_view};
use crate::game::actor::{self, Actors};
use crate::game::brick_editor::{self, BrickEditor};
use crate::game::editor_camera::{self, EditorCamera};
use crate::game::editor_object::{self, Selection};
use crate::game::editor_view::{self, Bricks};
use crate::game::reflection::{self, SingleCall};
use crate::game::view_target::{self, ZoomFields};
use crate::math;
use crate::prelude::*;

pub struct CameraView;

/// How many frames apart the selection is looked through: it goes through every brick, and the
/// button showing a moment late is fine. A click or the key looks at once.
const SCAN_EVERY: u32 = 6;

/// The game code this feature uses (after engine init).
struct Game {
    /// `CameraBrick`, and where its `IViewTargetInterface` is.
    camera: *const UClass,
    camera_interface: usize,
    bricks: Bricks,
    selection: Selection,
    actors: Actors,
    editor: BrickEditor,
    editor_camera: EditorCamera,
    zoom: ZoomFields,
    /// `AController::SetControlRotation(FRotator)`: the view's rotation, which the editor's camera
    /// follows.
    set_control_rotation: SingleCall,
    /// `APlayerController::PlayerCameraManager`, and `APlayerCameraManager::DefaultFOV` in it: the
    /// field of view `ABrickEditor::CalcCamera` multiplies by its zoom.
    camera_manager: usize,
    default_fov: usize,
}

// SAFETY: reflection records kept for the whole game; only used on the game thread.
unsafe impl Send for Game {}
unsafe impl Sync for Game {}

static GAME: OnceLock<Game> = OnceLock::new();

/// A camera brick was selected when last looked: the button shows.
static SELECTED: AtomicBool = AtomicBool::new(false);
/// The button was clicked: done at the editor's next tick.
static REQUESTED: AtomicBool = AtomicBool::new(false);
/// The key was held last frame.
static KEY_HELD: AtomicBool = AtomicBool::new(false);
/// Frames since the start, to look through the selection every [`SCAN_EVERY`].
static FRAMES: AtomicU32 = AtomicU32::new(0);

impl Feature for CameraView {
    fn id(&self) -> &'static str {
        "camera_view"
    }

    fn on_load(&self) -> LoadResult {
        editor_menu::on_click_when(
            &keybinds::CAMERA_VIEW,
            || REQUESTED.store(true, Ordering::Relaxed),
            || SELECTED.load(Ordering::Relaxed),
        );
        brick_editor::after_tick(after_tick)
    }

    fn on_engine_init(&self) {
        match start() {
            Ok(()) => log!("active: Go to camera view with a camera brick selected"),
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

fn start() -> Result<(), String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let camera = class("CameraBrick")?;
    // SAFETY: a live class.
    let camera_interface = unsafe {
        reflection::interface_offset(camera.cast(), "ViewTargetInterface")
            .ok_or("CameraBrick has no view target interface")?
    };
    let game = Game {
        camera,
        camera_interface,
        bricks: editor_view::bricks()?,
        selection: editor_object::selection()?,
        actors: actor::find()?,
        editor: brick_editor::find()?,
        editor_camera: editor_camera::code().and_then(editor_camera::find)?,
        zoom: view_target::zoom_fields()?,
        set_control_rotation: SingleCall::find(
            class("Controller")?,
            "SetControlRotation",
            "NewRotation",
            "StructProperty",
            12,
        )?,
        camera_manager: reflection::field_offset(
            class("PlayerController")?,
            "PlayerCameraManager",
            "ObjectProperty",
            8,
        )?,
        default_fov: reflection::field_offset(
            class("PlayerCameraManager")?,
            "DefaultFOV",
            "FloatProperty",
            4,
        )?,
    };
    let _ = GAME.set(game);
    Ok(())
}

/// Each frame: the key and a click go to the view, and the button follows the selection.
fn after_tick(editor: *mut UObjectBase, _delta: f32) {
    let Some(game) = GAME.get() else { return };
    let held = keybinds::CAMERA_VIEW.is_held();
    let was_held = KEY_HELD.swap(held, Ordering::Relaxed);
    let clicked = REQUESTED.swap(false, Ordering::Relaxed);
    let asked = clicked || (held && !was_held);
    let frame = FRAMES.fetch_add(1, Ordering::Relaxed);
    if !asked && frame % SCAN_EVERY != 0 {
        return;
    }
    // SAFETY: the live editor ticking, on the game thread.
    let camera = unsafe { game.selected_camera(editor) };
    let selected = camera.is_some();
    if SELECTED.swap(selected, Ordering::Relaxed) != selected {
        editor_menu::refresh_bottom_bar();
    }
    if asked && let Some(camera) = camera {
        // SAFETY: as above; the brick is one of the editor's, alive during its tick.
        unsafe { game.go_to(editor, camera) };
    }
}

impl Game {
    /// The first selected camera brick of `editor`.
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    unsafe fn selected_camera(&self, editor: *mut UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; the editor's objects are live or null. The selection comes
        // first: few objects are selected, and it's the cheaper to ask.
        unsafe {
            self.bricks
                .editor_objects(editor)
                .iter()
                .copied()
                .find(|&object| {
                    !object.is_null()
                        && self.selection.is_selected(object)
                        && reflection::with_parents(reflection::class_of(object.cast()))
                            .any(|class| class == self.camera.cast())
                })
        }
    }

    /// Puts `editor`'s camera where `camera` sees from.
    ///
    /// # Safety
    /// `editor` must be the live editor and `camera` a live camera brick. Game thread only.
    unsafe fn go_to(&self, editor: *mut UObjectBase, camera: *mut UObjectBase) {
        // SAFETY: per the contract; the interface is at its class's offset, the editor's found by
        // `editor_camera`, and the fields' offsets come from reflection.
        unsafe {
            let brick = camera
                .cast::<u8>()
                .add(self.camera_interface)
                .cast::<c_void>();
            let (location, rotation) = view_target::view_point(brick);
            let from_editor = math::sub(
                math::widen(location),
                math::widen(self.actors.location(editor)),
            );
            self.editor_camera.place_camera(
                editor,
                math::narrow(editor_frame::to_editor_space(from_editor)),
            );

            let controller = self.editor.player_controller(editor);
            if !controller.is_null() {
                self.set_control_rotation.call(controller, rotation);
            }
            let interface = self.editor_camera.view_target_interface(editor);
            if let Some(interface) = interface.filter(|_| editor_frame::is_ready()) {
                editor_frame::set_camera_rotation(interface, rotation);
            }
            // An orthographic view would take the new angle for a turn and swing the camera away.
            ortho_view::camera_placed();

            let fov = self
                .zoom
                .of(brick)
                .map(|zoom| zoom.value_at(view_target::zoom_ratio(brick).unwrap_or(0.0)));
            let base = self.default_fov(controller);
            let ours = interface.map(|interface| interface as *mut c_void);
            if let (Some(fov), Some(base), Some(ours)) = (fov, base, ours)
                && let Some(zoom) = self.zoom.of(ours)
            {
                let ratio = zoom.ratio_for(fov / base);
                view_target::glide_zoom_to(ours, ratio);
                static LOG_FIRST: Once = Once::new();
                LOG_FIRST.call_once(|| {
                    log!(
                        "a camera brick's view: {fov} degrees from {base}, the editor's zoom \
                         {zoom:?} at {ratio}"
                    )
                });
            }
        }
    }

    /// The field of view the editor's zoom multiplies, in degrees.
    ///
    /// # Safety
    /// `controller` must be null or a live player controller. Game thread only.
    unsafe fn default_fov(&self, controller: *mut UObjectBase) -> Option<f32> {
        if controller.is_null() {
            return None;
        }
        // SAFETY: per the contract; the offsets come from reflection, and the object property holds
        // a live camera manager or null.
        unsafe {
            let manager = controller
                .cast::<u8>()
                .add(self.camera_manager)
                .cast::<*const u8>()
                .read();
            if manager.is_null() {
                return None;
            }
            let fov = manager.add(self.default_fov).cast::<f32>().read();
            (fov > 0.0).then_some(fov)
        }
    }
}
