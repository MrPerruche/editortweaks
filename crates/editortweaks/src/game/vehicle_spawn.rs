//! How the game spawns the player's vehicle when the editor closes, and the spawn options of the
//! vehicle browser (None, Invincible, Pinned) that the spawn request carries. Found through BRMK
//! 1.11.6's exports, then in game 1.11.7 (build 25831956); see `docs/re/spawn-mode.md`.
//!
//! `ABrickPlayerController::ExitEditor(const FUGCFileInfo&)` (BRMK 0x2A7E70, game RVA 0xE1E680)
//! closes the editor and, for a vehicle, builds an `FPlayerSpawnRequest` and hands it to
//! `ABrickPlayerController::RestartAt(FPlayerSpawnRequest)` (BRMK 0x2D9B90, game 0xE418A0). The
//! request's `VehicleSpawnType` (+0x110) is `SpawnNew` or `ReplaceCurrent` and its
//! `VehicleInvincibility` (+0x111) the browser's option; `ExitEditor` never sets it (the
//! byte is whatever was on its stack). `ABrickGameMode::RestartPlayerAt` reads it when it spawns
//! the vehicle: `Invincible` makes it undamageable, `Pinned` sets the pin mode `AllBricks` before
//! the vehicle is built.

use core::ffi::c_void;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UStruct;

use crate::framework::sig::FunctionSig;
use crate::game::reflection::{self, SingleCall};

/// `void ABrickPlayerController::ExitEditor(const FUGCFileInfo& Saved)`: found by the editor
/// being closed (`SetBrickEditor(nullptr)`, first thing after the editor's field was tested) and
/// the file info's validity byte tested right after.
pub const EXIT_EDITOR: FunctionSig = FunctionSig {
    name: "ABrickPlayerController::ExitEditor",
    pattern: "48 83 b9 ?? ?? 00 00 00 48 8b ea 48 8b d9 0f 84 ?? ?? ?? ?? 48 89 70 08 33 d2 48 8b b1 ?? ?? 00 00 48 89 78 10 33 ff 48 89 b9 ?? ?? 00 00 e8 ?? ?? ?? ?? 40 38 7d 08",
};

/// [`EXIT_EDITOR`]'s signature.
pub type ExitEditorFn = unsafe extern "C" fn(controller: *mut UObjectBase, saved: *const c_void);

/// `EPlayerSpawnResult ABrickPlayerController::RestartAt(FPlayerSpawnRequest Request)`: the
/// request is passed by reference (it is bigger than a register). Found by its start: it checks
/// the request (`CanRestartAt`), tells the player if it failed, then tests whether the editor is
/// open.
pub const RESTART_AT: FunctionSig = FunctionSig {
    name: "ABrickPlayerController::RestartAt",
    pattern: "45 33 c0 48 8b f2 48 8b f9 e8 ?? ?? ?? ?? 44 0f b6 f0 84 c0 74 ?? 48 8d 54 24 ?? 48 8d 4e 28 e8 ?? ?? ?? ?? 4c 8b c0 41 0f b6 d6 48 8b cf e8 ?? ?? ?? ?? 48 8d 4e 28 e8 ?? ?? ?? ?? 41 0f b6 c6 e9 ?? ?? ?? ?? 45 33 f6 f6 87 ?? ?? 00 00 02",
};

/// [`RESTART_AT`]'s signature; the result is an `EPlayerSpawnResult`.
pub type RestartAtFn = unsafe extern "C" fn(controller: *mut UObjectBase, request: *mut u8) -> u8;

/// The spawn options of the vehicle browser (`EPlayerVehicleSpawnInvincibility`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpawnOption {
    None,
    Invincible,
    Pinned,
}

/// The fields of an `FPlayerSpawnRequest` this mod reads and writes, and the enum values they hold.
pub struct SpawnRequest {
    spawn_type: usize,
    invincibility: usize,
    spawn_new: u8,
    replace_current: u8,
    none: u8,
    invincible: u8,
    pinned: u8,
    /// `EPlayerSpawnResult::Success`.
    success: u8,
}

impl SpawnRequest {
    /// Looks the request's fields and the enums up through reflection. After engine init.
    pub fn find() -> Result<Self, String> {
        let request: *const UStruct = reflection::find_object("PlayerSpawnRequest", "ScriptStruct")
            .ok_or("struct PlayerSpawnRequest not found")?
            .cast_const()
            .cast();
        let value = |name: &'static str, entry: &str| {
            let full = format!("{name}::{entry}");
            reflection::enum_value(name, &full)
                .and_then(|value| u8::try_from(value).map_err(|_| format!("{full} isn't a byte")))
        };
        let spawn_type = enum_field(request, "VehicleSpawnType")?;
        let invincibility = enum_field(request, "VehicleInvincibility")?;
        Ok(Self {
            spawn_type,
            invincibility,
            spawn_new: value("EPlayerVehicleSpawnType", "SpawnNew")?,
            replace_current: value("EPlayerVehicleSpawnType", "ReplaceCurrent")?,
            none: value("EPlayerVehicleSpawnInvincibility", "None")?,
            invincible: value("EPlayerVehicleSpawnInvincibility", "Invincible")?,
            pinned: value("EPlayerVehicleSpawnInvincibility", "Pinned")?,
            success: value("EPlayerSpawnResult", "Success")?,
        })
    }

    /// Whether `request` asks for a vehicle: a new one, or one replacing the player's current one
    /// (the other types teleport, unflip, scrap or exit).
    ///
    /// # Safety
    /// `request` must point to a live `FPlayerSpawnRequest`.
    pub unsafe fn spawns_vehicle(&self, request: *const u8) -> bool {
        // SAFETY: per the contract; the field lies within the struct (found through reflection).
        let spawn_type = unsafe { request.add(self.spawn_type).read() };
        spawn_type == self.spawn_new || spawn_type == self.replace_current
    }

    /// Sets the browser's option the request carries.
    ///
    /// # Safety
    /// `request` must point to a live, writable `FPlayerSpawnRequest`.
    pub unsafe fn set_option(&self, request: *mut u8, option: SpawnOption) {
        let value = match option {
            SpawnOption::None => self.none,
            SpawnOption::Invincible => self.invincible,
            SpawnOption::Pinned => self.pinned,
        };
        // SAFETY: per the contract; the field lies within the struct.
        unsafe { request.add(self.invincibility).write(value) };
    }

    /// Whether `RestartAt`'s result says the vehicle was spawned.
    pub fn succeeded(&self, result: u8) -> bool {
        result == self.success
    }
}

/// The offset of an enum field (a byte, or an enum property of a byte) of `ustruct`.
fn enum_field(ustruct: *const UStruct, name: &'static str) -> Result<usize, String> {
    reflection::struct_field_offset(ustruct, name, "ByteProperty", 1)
        .or_else(|_| reflection::struct_field_offset(ustruct, name, "EnumProperty", 1))
        .map_err(|reason| format!("{reason} (it has {})", field_names(ustruct)))
}

/// The names, offsets and sizes of `ustruct`'s fields, for an error message.
fn field_names(ustruct: *const UStruct) -> String {
    // SAFETY: `ustruct` comes from reflection; property records live as long as their struct.
    unsafe {
        reflection::properties(ustruct)
            .map(|property| {
                let name = reflection::name_text(&(*property).field.name_private);
                format!(
                    "{name}@{:#x}x{}",
                    (*property).offset_internal,
                    (*property).element_size
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Pins the player's vehicle the way the game's pin keys do: `ABrickPlayerController::
/// SetVehiclePinMode(EVehiclePinMode)` (BRMK 0x2DE8F0), which sets the pin mode of the vehicle the
/// controller drives (through the server in a multiplayer game) if the player may use cheats.
pub struct PinVehicle {
    set_pin_mode: SingleCall,
    /// `EVehiclePinMode::RootBrick`: only the vehicle's root brick is pinned.
    root_brick: u8,
}

// SAFETY: reflection records living as long as the game; only called on the game thread.
unsafe impl Send for PinVehicle {}
unsafe impl Sync for PinVehicle {}

impl PinVehicle {
    /// After engine init.
    pub fn find() -> Result<Self, String> {
        let controller = reflection::find_class("BrickPlayerController")
            .ok_or("class BrickPlayerController not found")?;
        let set_pin_mode = SingleCall::find_only(
            controller,
            "SetVehiclePinMode",
            &["ByteProperty", "EnumProperty"],
            1,
        )?;
        let root_brick = reflection::enum_value("EVehiclePinMode", "EVehiclePinMode::RootBrick")?;
        Ok(Self {
            set_pin_mode,
            root_brick: u8::try_from(root_brick).map_err(|_| "RootBrick isn't a byte")?,
        })
    }

    /// Pins only the root brick of the vehicle `controller` drives.
    ///
    /// # Safety
    /// `controller` must be a live `ABrickPlayerController`. Game thread only.
    pub unsafe fn pin_root_brick(&self, controller: *mut UObjectBase) {
        // SAFETY: per the contract; the parameter is an `EVehiclePinMode` byte (checked in `find`).
        unsafe { self.set_pin_mode.call(controller, self.root_brick) }
    }
}
