//! The game viewport's view mode (engine, UE 4.27 `EViewModeIndex`): lit, the game's, or unlit,
//! where each surface shows its plain colour, with no light, shadow or shading.
//!
//! The `viewmode` console command can't set it in the game: its handler
//! (`UGameViewportClient::HandleViewModeCommand`, game 0x25A1000) needs a cheat manager, and in
//! shipping builds always ends by putting lit back (see `docs/re/editor-lighting.md`). That ending
//! is what [`APPLY`] finds: it gives the viewport client's `ViewModeIndex` and `EngineShowFlags`
//! offsets (neither is reflected) and `ApplyViewMode`. [`ViewModes::set`] does the same with the
//! mode asked for; the viewport then draws in it each frame, until it is set again.
//!
//! The viewport client is reached by reflection: `UGameplayStatics::GetPlayerController(
//! WorldContextObject, PlayerIndex)`, its `Player` (a `ULocalPlayer`), and that one's
//! `ViewportClient`.

use core::ffi::c_void;

use brickrust::ue::coreuobject::UObjectBase;

use crate::framework::sig::{CodeShape, FunctionSig};
use crate::game::reflection::{self, Function, Parameters, put, take};

/// The end of `HandleViewModeCommand`: `ViewModeIndex = VMI_Lit` (its offset at +2), the
/// "Debug viewmodes not allowed in Test or Shipping builds." message logged, then
/// `ApplyViewMode(VMI_Lit, true, EngineShowFlags)` (the flags' offset at +28, the call at
/// [`APPLY_CALL`]).
pub const APPLY: FunctionSig = FunctionSig {
    name: "UGameViewportClient::HandleViewModeCommand shipping end",
    pattern: "c7 86 ?? ?? 00 00 03 00 00 00 48 8d 15 ?? ?? ?? ?? 49 8b ce e8 ?? ?? ?? ?? 4c 8d 86 ?? ?? 00 00 c7 86 ?? ?? 00 00 03 00 00 00 b2 01 b9 03 00 00 00 e8 ?? ?? ?? ??",
};
const APPLY_CALL: usize = 49;

/// `ApplyViewMode`, a leaf function (not in the unwind table, so checked by its whole code): a jump
/// table over the 29 view modes picking the post-processing flag, which goes in the flags' first
/// bit; one more flag only for mode 27.
const APPLY_VIEW_MODE: CodeShape = CodeShape {
    name: "ApplyViewMode",
    shape: "45 33 d2 4d 8b d8 83 f9 1c 77 1d 4c 8d 05 ?? ?? ?? ?? 48 63 c1 45 8b 8c 80 ?? ?? ?? ?? 4d 03 c8 41 ff e1 45 8b c2 eb 06 41 b8 01 00 00 00 41 83 23 fe 41 8b c2 84 d2 41 0f 45 c0 41 81 63 04 ff ff ff f7 41 09 03 b8 00 00 00 08 83 f9 1b 44 0f 44 d0 45 09 53 04 c3",
};

/// A view mode, as `EViewModeIndex` numbers it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(i32)]
pub enum ViewMode {
    Unlit = 2,
    Lit = 3,
}

/// `void ApplyViewMode(EViewModeIndex ViewModeIndex, bool bPerspective, FEngineShowFlags&
/// EngineShowFlags)`: the show flags that go with a view mode.
type ApplyFn = unsafe extern "C" fn(mode: i32, perspective: bool, flags: *mut c_void);

/// What sets view modes, after engine init.
pub struct ViewModes {
    /// `UGameplayStatics`' default object, which static functions are called on.
    statics: *mut UObjectBase,
    /// `UGameplayStatics::GetPlayerController`.
    get_controller: Function,
    /// Offsets of its parameters and return value.
    world: usize,
    index: usize,
    controller: usize,
    /// Offsets of `APlayerController::Player` and `ULocalPlayer::ViewportClient`.
    player: usize,
    viewport_client: usize,
    /// Offsets of `UGameViewportClient::ViewModeIndex` and `EngineShowFlags`.
    mode: usize,
    show_flags: usize,
    apply: ApplyFn,
}

// SAFETY: reflection records, game code and a default object living as long as the game; only
// used on the game thread.
unsafe impl Send for ViewModes {}
unsafe impl Sync for ViewModes {}

pub fn find() -> Result<ViewModes, String> {
    let found = APPLY.locate()?;
    // SAFETY: the offsets and the call (an E8, matched by the pattern) lie inside the match.
    let (mode, mode_again, show_flags, apply) = unsafe {
        let u32_at = |offset| found.address.add(offset).cast::<u32>().read_unaligned() as usize;
        let relative = u32_at(APPLY_CALL + 1) as i32 as isize;
        let apply = found.address.add(APPLY_CALL + 5).wrapping_offset(relative);
        (u32_at(2), u32_at(34), u32_at(28), apply)
    };
    if !APPLY_VIEW_MODE.matches(apply) {
        return Err("ApplyViewMode: the called code isn't the expected function".into());
    }
    // Both stores of `VMI_Lit` in the pattern are to `ViewModeIndex`.
    if mode != mode_again {
        return Err("HandleViewModeCommand: its two view mode stores differ".into());
    }

    let statics_class =
        reflection::find_class("GameplayStatics").ok_or("class GameplayStatics not found")?;
    let statics = reflection::find_object("Default__GameplayStatics", "GameplayStatics")
        .ok_or("GameplayStatics' default object not found")?;
    let controller_class =
        reflection::find_class("PlayerController").ok_or("class PlayerController not found")?;
    let local_player =
        reflection::find_class("LocalPlayer").ok_or("class LocalPlayer not found")?;
    let get_controller = Function::find(statics_class, "GetPlayerController")?;
    Ok(ViewModes {
        statics,
        world: get_controller.parameter("WorldContextObject", "ObjectProperty", 8)?,
        index: get_controller.parameter("PlayerIndex", "IntProperty", 4)?,
        controller: get_controller.parameter("ReturnValue", "ObjectProperty", 8)?,
        get_controller,
        player: reflection::field_offset(controller_class, "Player", "ObjectProperty", 8)?,
        viewport_client: reflection::field_offset(
            local_player,
            "ViewportClient",
            "ObjectProperty",
            8,
        )?,
        mode,
        show_flags,
        // SAFETY: the start of `ApplyViewMode`, which the game calls with these arguments.
        apply: unsafe { core::mem::transmute::<*const u8, ApplyFn>(apply) },
    })
}

impl ViewModes {
    /// Switches the first player's game viewport to `mode`, in the world of `context` (any live
    /// object of it).
    ///
    /// # Safety
    /// `context` must be a live object. Game thread only.
    pub unsafe fn set(&self, context: *mut UObjectBase, mode: ViewMode) -> Result<(), String> {
        // SAFETY: per the contract; the offsets come from reflection and the game's own code,
        // read on live objects of those classes.
        unsafe {
            let mut parameters = Parameters::new();
            put(&mut parameters, self.world, context);
            put(&mut parameters, self.index, 0i32);
            self.get_controller.call(self.statics, &mut parameters);
            let controller: *mut UObjectBase = take(&parameters, self.controller);
            if controller.is_null() {
                return Err("no player controller".into());
            }
            let field = |object: *mut UObjectBase, offset| {
                object
                    .cast::<u8>()
                    .add(offset)
                    .cast::<*mut UObjectBase>()
                    .read()
            };
            let player = field(controller, self.player);
            if player.is_null() {
                return Err("no local player".into());
            }
            let client = field(player, self.viewport_client).cast::<u8>();
            if client.is_null() {
                return Err("no game viewport".into());
            }
            client.add(self.mode).cast::<i32>().write(mode as i32);
            (self.apply)(mode as i32, true, client.add(self.show_flags).cast());
        }
        Ok(())
    }
}
