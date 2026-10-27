//! How the vehicle spawns when the editor closes, a setting ([`SPAWN_MODE`], a row in the
//! editor's side panel only, first in its "Editor settings", not in the options menu): normal, god
//! or hardpin, the vehicle browser's spawn options (None, Invincible, Pinned). Softpin, which pins
//! only the vehicle's root brick (whatever is linked to the driver seat, or the first brick if there
//! is no seat) so that the rest of the vehicle hangs from it, is written but not offered for now
//! ([`SOFTPIN`]).
//!
//! The game builds the spawn request itself when the editor closes and never sets its spawn
//! option (`game::vehicle_spawn`). So both functions are hooked: `ExitEditor` marks the request it
//! is about to make, and `RestartAt` sets the option on that request (not on the browser's) before
//! the game reads it, as the browser would have. The game then checks it as it does the browser's
//! (e.g. a server that forbids invincibility refuses the request). Softpin has no option of its
//! own: the request asks for none, and once the vehicle is spawned the controller's own
//! `SetVehiclePinMode` pins its root brick, as the game's pin key does.

use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;
use core::ffi::c_void;

use super::settings_list::{self, Rework};
use crate::framework::hook;
use crate::framework::settings::{Alternative, Choice, Setting};
use crate::game::property_reflection::Row;
use crate::game::vehicle_spawn::{
    self, ExitEditorFn, PinVehicle, RestartAtFn, SpawnOption, SpawnRequest,
};
use crate::prelude::*;

/// How the vehicle spawns when the editor closes, a setting: the options of [`Mode`], in order.
pub static SPAWN_MODE: Choice = Choice::new(
    "SpawnMode",
    "Spawn mode",
    "How your vehicle spawns when you leave the vehicle editor. Normal, God and Hardpin are the \
     vehicle selector's spawn modes.",
    &[
        Alternative {
            key: "Normal",
            name: "Normal",
        },
        Alternative {
            key: "God",
            name: "God",
        },
        Alternative {
            key: "Hardpin",
            name: "Hardpin",
        },
    ],
    0,
);

/// Softpin's place in [`SPAWN_MODE`], not offered for now: to offer it, add this alternative
/// between God and Hardpin, and give `Mode::chosen` its index (2, Hardpin's becoming 3). Its
/// description: "Softpin pins only the vehicle's center (what is linked to the driver seat, or the
/// first brick if there is none)."
#[allow(dead_code)]
const SOFTPIN: Alternative = Alternative {
    key: "Softpin",
    name: "Softpin",
};

/// The options of [`SPAWN_MODE`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Normal,
    God,
    /// Not offered for now ([`SOFTPIN`]).
    #[allow(dead_code)]
    Softpin,
    Hardpin,
}

impl Mode {
    fn chosen() -> Self {
        match SPAWN_MODE.get() {
            1 => Self::God,
            2 => Self::Hardpin,
            _ => Self::Normal,
        }
    }

    /// The vehicle browser's option the spawn request carries for this mode: softpin pins after
    /// the spawn, so its request is a normal one.
    fn option(self) -> SpawnOption {
        match self {
            Self::Normal | Self::Softpin => SpawnOption::None,
            Self::God => SpawnOption::Invincible,
            Self::Hardpin => SpawnOption::Pinned,
        }
    }
}

/// What the side panel asks the settings list for, with nothing selected.
const EDITOR_SETTINGS: &str = "Properties.Editor";

/// The game's first row of the side panel's "Editor settings" (`EditorUIScale`), which our row goes
/// before.
const FIRST_ROW: &str = "EditorUIScale";

/// What the hooks need, found through reflection.
struct Spawning {
    request: SpawnRequest,
    pin: PinVehicle,
}

static ROW: OnceLock<Row> = OnceLock::new();
static SPAWNING: OnceLock<Spawning> = OnceLock::new();
static EXIT_ORIGINAL: OnceLock<ExitEditorFn> = OnceLock::new();
static RESTART_ORIGINAL: OnceLock<RestartAtFn> = OnceLock::new();
/// Whether the game is in `ExitEditor`, so that the next spawn request is the editor's.
static EXITING_EDITOR: AtomicBool = AtomicBool::new(false);

pub struct SpawnMode;

impl Feature for SpawnMode {
    fn id(&self) -> &'static str {
        "spawn_mode"
    }

    fn on_engine_init(&self) {
        match start() {
            Ok(()) => {
                log!("active: the vehicle spawns as the spawn mode says when the editor closes")
            }
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

fn start() -> Result<(), String> {
    let exit_editor = vehicle_spawn::EXIT_EDITOR.resolve()?;
    let restart_at = vehicle_spawn::RESTART_AT.resolve()?;
    let spawning = Spawning {
        request: SpawnRequest::find()?,
        pin: PinVehicle::find()?,
    };
    let row = settings_list::setting_row(Setting::Choice(&SPAWN_MODE))?;
    let _ = ROW.set(row);
    let _ = SPAWNING.set(spawning);
    // SAFETY: each target is the start of the function it was found for, and each detour has that
    // function's signature.
    unsafe {
        hook::install_typed(
            vehicle_spawn::EXIT_EDITOR.name,
            exit_editor,
            exit_editor_detour as ExitEditorFn,
            &EXIT_ORIGINAL,
        )?;
        hook::install_typed(
            vehicle_spawn::RESTART_AT.name,
            restart_at,
            restart_at_detour as RestartAtFn,
            &RESTART_ORIGINAL,
        )?;
    }
    settings_list::rework(EDITOR_SETTINGS, rework)
}

/// Our row goes before the game's first row of the editor settings.
fn rework(field: &str) -> Rework {
    match ROW.get() {
        Some(row) if field == FIRST_ROW => Rework::Before(row),
        _ => Rework::Keep,
    }
}

/// Marks the spawn request `ExitEditor` makes as the editor's.
unsafe extern "C" fn exit_editor_detour(controller: *mut UObjectBase, saved: *const c_void) {
    let original = *EXIT_ORIGINAL.get().expect("hook recorded before use");
    let was_exiting = EXITING_EDITOR.swap(true, Ordering::Relaxed);
    // SAFETY: the game's own arguments, passed on.
    unsafe { original(controller, saved) };
    EXITING_EDITOR.store(was_exiting, Ordering::Relaxed);
    // SAFETY: the controller is live, and this is the game thread.
    unsafe { super::editor_lighting::restore(controller) };
}

/// Gives the editor's spawn request the spawn mode's option, and pins the root brick afterwards
/// for softpin.
unsafe extern "C" fn restart_at_detour(controller: *mut UObjectBase, request: *mut u8) -> u8 {
    let original = *RESTART_ORIGINAL.get().expect("hook recorded before use");
    // Taken: only the first request after `ExitEditor` started is the editor's.
    let from_editor = EXITING_EDITOR.swap(false, Ordering::Relaxed);
    let spawning = SPAWNING.get().filter(|_| from_editor);
    let mode = Mode::chosen();
    let editor_spawn = spawning.filter(|spawning| {
        // SAFETY: the game calls this with a live request.
        unsafe { spawning.request.spawns_vehicle(request) }
    });
    if let Some(spawning) = editor_spawn {
        // SAFETY: as above; the request is the callee's own copy, which it may change.
        unsafe { spawning.request.set_option(request, mode.option()) };
    }
    // SAFETY: the game's own arguments, passed on.
    let result = unsafe { original(controller, request) };
    if let Some(spawning) = editor_spawn
        && mode == Mode::Softpin
        && spawning.request.succeeded(result)
    {
        // SAFETY: the controller is the live one that was called; this is the game thread.
        unsafe { spawning.pin.pin_root_brick(controller) };
    }
    result
}
