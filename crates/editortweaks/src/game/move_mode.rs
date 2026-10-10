//! How the editor's move mode moves bricks, and what a move is made of, to compute moves again
//! (`features/precise_moves`). Found through BRMK 1.11.6, then in game 1.11.7 (build 25831956);
//! see `docs/re/editor-operations.md` ("How a move is computed").
//!
//! Every moved brick goes through `UBrickEditorMoveMode::SanitizeObjectTransform` (game RVA
//! 0xD019C0): from the transform the move gives it (`GetEffectiveObjectTransform`) to the spawn
//! location and rotation it keeps. `EndMode` writes the result into the brick (the commit),
//! `TickMode` shows it while moving.
//!
//! ```text
//! (FVector, FRotator) SanitizeObjectTransform(const UBrickEditorObject* Object,
//!                                             const FMoveTransform& Moved) const
//!     Location = Moved.Location, unless within 1e-4 per coordinate of EffectiveMoveLocation (if
//!         the flag 0x40 is set, as `BeginMode` does) or of the object's spawn location: then that
//!     Rotation = QuatToNiceRotator(Moved.Rotation), unless Moved.Rotation is within 1e-4 per
//!         component of EffectiveMoveRotation's (flag 0x80) or of the spawn rotation's: then that
//! ```
//!
//! `TickMode` sanitizes twice: each moved brick's transform, and the reference brick's move
//! without the brick snapping adjustment, which becomes `EffectiveMoveLocation`/`Rotation` (the
//! panel's numbers) while their flags are clear.
//!
//! When the gizmo's axis changes (e.g. releasing a gizmo handle), `ApplyAccumulatedInput` (game
//! 0xCCE610) folds the input so far into the move's own place and rotation, in `f32`.
//!
//! The move ([`Move`]) is the place the reference brick goes to (the one grabbed), computed in
//! `GetMoveTransformWithAccumulatedInput` (game 0xCE57E0) from the move's own place and rotation
//! plus the mouse's input so far (drag and turn, snapped to the grid and angle steps when snapping
//! is on), then adjusted by brick snapping (game 0xCCEB00). Every other brick keeps its place
//! relative to the reference: `Move * Reference⁻¹ * Brick`, the way `GetEffectiveObjectTransform`
//! (game 0xCE3140) does for bricks that aren't mirror mode counterparts.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::tshared::FWeakObjectPtr;

use crate::framework::sig::{FunctionSig, SigMatch};
use crate::game::editor_object::Vector;
use crate::game::reflection;
use crate::game::weak_object;

/// The start: the object's spawn location read (its offset at +5), the move mode's flags (offset
/// at +12) tested for an effective location (`test cl, 0x40`).
pub const SANITIZE: FunctionSig = FunctionSig {
    name: "UBrickEditorMoveMode::SanitizeObjectTransform",
    pattern: "f2 41 0f 10 40 ?? 48 8b f9 0f b6 49 ?? 49 8b d9 66 0f 6f 0d ?? ?? ?? ?? 48 8b f2 f3 0f 10 15 ?? ?? ?? ?? 0f 29 74 24 ?? 0f 29 7c 24 ?? 44 0f 29 44 24 ?? f2 0f 11 44 24 ?? f6 c1 40",
};

/// The rotation part: the object's spawn rotation read (its offset at +5), the flags tested for
/// an effective rotation (`test cl, cl; jns`), `EffectiveMoveRotation` turned into a quaternion
/// (its offset at +40), then compared with the moved rotation (its offset in `FMoveTransform` at
/// +52).
pub const SANITIZE_ROTATION: FunctionSig = FunctionSig {
    name: "UBrickEditorMoveMode::SanitizeObjectTransform rotation",
    pattern: "f2 41 0f 10 40 ?? 41 8b 40 ?? f2 0f 11 44 24 ?? 48 89 ac 24 ?? ?? 00 00 89 44 24 ?? 84 c9 79 ?? 48 8d 54 24 ?? 48 8d 8f ?? ?? 00 00 e8 ?? ?? ?? ?? 0f 10 63 ?? 48 83 c3",
};

/// `GetMoveTransformWithAccumulatedInput`'s start: the move's location (offset at +4) and rotation
/// (+27) into the result, `UBrickUserSettings::GetUserSettings()` (call at +43),
/// `GetGizmoTransformInternal(false)` (call at +62), the drag so far tested (offset at +79).
pub const MOVE_TRANSFORM: FunctionSig = FunctionSig {
    name: "UBrickEditorMoveMode::GetMoveTransformWithAccumulatedInput",
    pattern: "f2 0f 10 81 ?? ?? 00 00 48 8b fa f2 0f 11 02 48 8b d9 8b 81 ?? ?? 00 00 48 81 c1 ?? ?? 00 00 89 42 08 48 83 c2 10 e8 ?? ?? ?? ?? e8 ?? ?? ?? ?? 45 33 c0 48 8d 54 24 ?? 48 8b cb 48 8b f0 e8 ?? ?? ?? ?? f3 0f 10 54 24 ?? 0f 57 f6 0f 2e 73 ??",
};

/// Its drag snapping: snapping on (settings offset at +2), the gizmo's type compared with 1 (its
/// offset at +41: a free move snaps along the vehicle's axes, the others along the gizmo's), the
/// grid step (settings offset at +54).
pub const MOVE_GRID: FunctionSig = FunctionSig {
    name: "UBrickEditorMoveMode::GetMoveTransformWithAccumulatedInput grid",
    pattern: "80 be ?? ?? ?? ?? 00 0f 29 bc 24 ?? ?? ?? ?? 44 0f 29 84 24 ?? ?? ?? ?? 44 0f 29 9c 24 ?? ?? ?? ?? 0f 84 ?? ?? ?? ?? 80 7b ?? 01 48 8d 15 ?? ?? ?? ?? f3 0f 10 9e ?? ?? ?? ??",
};

/// Its turn: the turn so far tested (offset at +3), snapping on (settings offset at +23), the
/// angle step (settings offset at +34), handed to a lambda turning about each gizmo axis.
pub const MOVE_TURN: FunctionSig = FunctionSig {
    name: "UBrickEditorMoveMode::GetMoveTransformWithAccumulatedInput turn",
    pattern: "0f 2e 73 ?? 75 ?? 0f 2e 73 ?? 75 ?? 0f 2e b3 ?? ?? ?? ?? 74 ?? 80 be ?? ?? ?? ?? 00 74 ?? f3 0f 10 86 ?? ?? ?? ??",
};

/// `ApplyAccumulatedInput` (game 0xCCE610), run when the gizmo's axis changes (releasing a gizmo
/// handle, grabbing another, switching the gizmo between world and local): the move with its input
/// so far (call at +5) becomes the move's own place (location offset at +36) and rotation (through
/// `FQuat::Rotator`, in `f32`; offset at +75), the flags get [`FOLDED`] (offset at +84), and the
/// input is cleared.
pub const APPLY_INPUT: FunctionSig = FunctionSig {
    name: "UBrickEditorMoveMode::ApplyAccumulatedInput",
    pattern: "48 8d 54 24 ?? e8 ?? ?? ?? ?? f3 0f 10 44 24 ?? 48 8d 54 24 ?? f3 0f 10 4c 24 ?? 48 8d 4c 24 ?? f3 0f 11 83 ?? ?? ?? ?? f3 0f 10 44 24 ?? f3 0f 11 83 ?? ?? ?? ?? f3 0f 11 8b ?? ?? ?? ?? e8 ?? ?? ?? ?? f2 0f 10 00 f2 0f 11 83 ?? ?? ?? ?? 8b 40 08 80 4b ?? 08",
};

/// `GetGizmoTransformInternal(bool)` (game 0xCE46C0): with input, the move (call at +5) adjusted
/// by brick snapping (its offset at +13, the call at +27); without, the move's place and rotation;
/// then the rotation is none if the flags' bit 1 (offset at +104) says the gizmo is the world's.
pub const GIZMO_FRAME: FunctionSig = FunctionSig {
    name: "UBrickEditorMoveMode::GetGizmoTransformInternal",
    pattern: "48 8d 54 24 ?? e8 ?? ?? ?? ?? 48 8d 8f ?? ?? ?? ?? 4c 8d 44 24 ?? 48 8d 54 24 ?? e8 ?? ?? ?? ?? 48 8d 44 24 ?? eb ?? f2 0f 10 81 ?? ?? ?? ?? 48 8d 54 24 ?? 8b 81 ?? ?? ?? ?? 48 81 c1 ?? ?? ?? ?? f2 0f 11 44 24 ?? 89 44 24 ?? e8 ?? ?? ?? ?? 48 8d 44 24 ?? 0f 28 00 0f 28 48 10 48 8b c3 0f 29 03 0f 29 4b 10 f6 47 ?? 01",
};

/// `GetEffectiveObjectTransform`: past the mirrored reference, the object compared with the
/// reference brick (its weak pointer's offset at +3).
pub const REFERENCE: FunctionSig = FunctionSig {
    name: "UBrickEditorMoveMode::GetEffectiveObjectTransform reference",
    pattern: "49 8d 57 ?? 48 8b cf e8 ?? ?? ?? ?? 84 c0 0f 84 ?? ?? ?? ?? 48 8b 4f ?? 48 89 b4 24",
};

/// The start of the brick snapping adjustment (game 0xCCEB00): its rotation read (offset at +13).
const ADJUSTMENT_CODE: &str = "48 8b c4 48 81 ec ?? ?? ?? ?? 0f 10 51 20 41 0f 10 18";

/// `FMoveTransform`: a location (padded) and a rotation, aligned for SIMD.
#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct MoveTransform {
    pub location: [f32; 4],
    /// A quaternion (x, y, z, w).
    pub rotation: [f32; 4],
}

/// The returned `TTuple<FVector, FRotator>`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Sanitized {
    pub location: Vector,
    pub rotation: Vector,
}

/// MSVC returns the tuple through a hidden pointer after `this`, and returns that pointer.
pub type SanitizeFn = unsafe extern "C" fn(
    mode: *mut UObjectBase,
    out: *mut Sanitized,
    object: *const UObjectBase,
    moved: *const MoveTransform,
) -> *mut Sanitized;

/// `void UBrickEditorMoveMode::ApplyAccumulatedInput()`.
pub type ApplyInputFn = unsafe extern "C" fn(mode: *mut UObjectBase);

/// `static UBrickUserSettings* UBrickUserSettings::GetUserSettings()`.
type UserSettingsFn = unsafe extern "C" fn() -> *const u8;

/// The brick snapping adjustment: the move is moved by `offset`, then turned by `rotation` about
/// `pivot` (game 0xCCEB00: `Rotation = Q * Rotation; Location = Pivot + Q.Rotate(Location +
/// Offset - Pivot)`). None is a zero offset and no rotation.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Adjustment {
    pub offset: Vector,
    pub pivot: Vector,
    _padding: [f32; 2],
    /// A quaternion (x, y, z, w).
    pub rotation: [f32; 4],
}

/// What a move is made of, as the move mode keeps it.
#[derive(Debug)]
pub struct Move {
    /// Its own place and rotation (pitch, yaw, roll) before the input below.
    pub location: Vector,
    pub rotation: Vector,
    /// Input was folded into that place and rotation since the move began ([`APPLY_INPUT`]); else
    /// they're the reference brick's own.
    pub folded: bool,
    /// The gizmo is the world's (else it turns with the move).
    pub world_gizmo: bool,
    /// The drag snaps along the gizmo's axes (else the vehicle's: a free move).
    pub drag_along_gizmo: bool,
    /// The mouse's input so far: a drag (cm), and a turn about each gizmo axis (degrees).
    pub drag: Vector,
    pub turn: Vector,
    /// The grid and angle steps, 0 while snapping is off.
    pub grid: f32,
    pub angle: f32,
    pub adjustment: Adjustment,
    /// The brick the move is about.
    pub reference: *mut UObjectBase,
}

/// Where the move mode and the objects keep it all (offsets from the game's code).
pub struct MoveMode {
    /// SanitizeObjectTransform and ApplyAccumulatedInput, to hook.
    pub sanitize: *const u8,
    pub apply_input: *const u8,
    user_settings: UserSettingsFn,
    /// In editor objects: the spawn location and rotation.
    spawn_location: usize,
    spawn_rotation: usize,
    /// In the move mode.
    flags: usize,
    effective_rotation: usize,
    reference: usize,
    gizmo_type: usize,
    drag: usize,
    turn: usize,
    location: usize,
    rotation: usize,
    adjustment: usize,
    /// In the user settings.
    snapping: usize,
    grid: usize,
    angle: usize,
}

// SAFETY: game code and offsets living as long as the game; only used on the game thread.
unsafe impl Send for MoveMode {}
unsafe impl Sync for MoveMode {}

/// The move mode's flags: the gizmo is the world's; input folded into the move's place since it
/// began (set by `ApplyAccumulatedInput`, cleared by `BeginMode`); results close to
/// `EffectiveMoveLocation`, `Rotation` become them (both set by `BeginMode`, which also sets those
/// two to the reference brick's place; see the module's documentation).
const WORLD_GIZMO: u8 = 0x01;
const FOLDED: u8 = 0x08;
const EFFECTIVE_LOCATION: u8 = 0x40;
const EFFECTIVE_ROTATION: u8 = 0x80;

/// The gizmo type of a free move (see `game::editor_mode`).
const FREE_MOVE: u8 = 1;

/// `MoveTransform::rotation`'s offset, checked against the code.
const MOVED_ROTATION: u8 = 0x10;

/// Finds it all (before engine init).
pub fn find() -> Result<MoveMode, String> {
    let [
        start,
        rotation,
        transform,
        grid,
        turn,
        gizmo,
        reference,
        apply,
    ] = [
        &SANITIZE,
        &SANITIZE_ROTATION,
        &MOVE_TRANSFORM,
        &MOVE_GRID,
        &MOVE_TURN,
        &GIZMO_FRAME,
        &REFERENCE,
        &APPLY_INPUT,
    ]
    .map(FunctionSig::locate);
    let (start, rotation, transform, grid, turn, gizmo, reference, apply) = (
        start?, rotation?, transform?, grid?, turn?, gizmo?, reference?, apply?,
    );
    let same = |a: &SigMatch, b: &SigMatch, what: &str| {
        (a.function == b.function)
            .then_some(())
            .ok_or_else(|| format!("{what} isn't where expected"))
    };
    same(&start, &rotation, SANITIZE_ROTATION.name)?;
    same(&transform, &grid, MOVE_GRID.name)?;
    same(&transform, &turn, MOVE_TURN.name)?;
    // SAFETY: each offset lies inside its matched pattern (see the patterns' comments).
    unsafe {
        let byte = |at: &SigMatch, offset| usize::from(at.address.add(offset).read());
        let dword =
            |at: &SigMatch, offset| at.address.add(offset).cast::<u32>().read_unaligned() as usize;
        if byte(&rotation, 52) != usize::from(MOVED_ROTATION) {
            return Err(format!(
                "{}: unexpected FMoveTransform layout",
                SANITIZE_ROTATION.name
            ));
        }
        for (at, name) in [(&gizmo, GIZMO_FRAME.name), (&apply, APPLY_INPUT.name)] {
            if at.call_target(5) != Some(transform.function) {
                return Err(format!("{name}: doesn't call {}", MOVE_TRANSFORM.name));
            }
        }
        let adjust = gizmo
            .call_target(27)
            .ok_or_else(|| format!("{}: no adjustment call", GIZMO_FRAME.name))?;
        if !code_matches(adjust, ADJUSTMENT_CODE) {
            return Err(format!("{}: unexpected adjustment", GIZMO_FRAME.name));
        }
        let user_settings = transform
            .call_target(43)
            .ok_or_else(|| format!("{}: no GetUserSettings call", MOVE_TRANSFORM.name))?;
        let found = MoveMode {
            sanitize: start.function,
            apply_input: apply.function,
            user_settings: core::mem::transmute::<*const u8, UserSettingsFn>(user_settings),
            spawn_location: byte(&start, 5),
            spawn_rotation: byte(&rotation, 5),
            flags: byte(&start, 12),
            effective_rotation: dword(&rotation, 40),
            reference: byte(&reference, 3),
            gizmo_type: byte(&grid, 41),
            drag: byte(&transform, 79),
            turn: byte(&turn, 3),
            location: dword(&transform, 4),
            rotation: dword(&transform, 27),
            adjustment: dword(&gizmo, 13),
            snapping: dword(&grid, 2),
            grid: dword(&grid, 54),
            angle: dword(&turn, 34),
        };
        let consistent = found.flags == byte(&gizmo, 104)
            && found.flags == byte(&apply, 84)
            && found.location == dword(&gizmo, 43)
            && found.location == dword(&apply, 36)
            && found.rotation == dword(&gizmo, 61)
            && found.rotation == dword(&apply, 75)
            && found.snapping == dword(&turn, 23);
        if !consistent {
            return Err(format!("{}: offsets disagree", MOVE_TRANSFORM.name));
        }
        Ok(found)
    }
}

/// Whether the code at `address` matches `pattern` (`??` for any byte).
///
/// # Safety
/// `address` must point to at least as many readable bytes as the pattern has.
unsafe fn code_matches(address: *const u8, pattern: &str) -> bool {
    pattern
        .split_whitespace()
        .enumerate()
        .all(|(index, token)| {
            // SAFETY: per the contract.
            let byte = unsafe { address.add(index).read() };
            token == "??" || u8::from_str_radix(token, 16) == Ok(byte)
        })
}

impl MoveMode {
    /// Checks the offsets against reflection (after engine init): the code's
    /// `EffectiveMoveRotation` must be the reflected one, with `EffectiveMoveLocation` right
    /// before it.
    pub fn check(&self) -> Result<(), String> {
        let class = reflection::find_class("BrickEditorMoveMode")
            .ok_or("class BrickEditorMoveMode not found")?;
        let field = |name| reflection::field_offset(class, name, "StructProperty", 12);
        let rotation = field("EffectiveMoveRotation")?;
        let location = field("EffectiveMoveLocation")?;
        if rotation != self.effective_rotation || location + size_of::<Vector>() != rotation {
            return Err(format!(
                "{}: EffectiveMoveRotation at +0x{:X}, reflection says location +0x{location:X}, \
                 rotation +0x{rotation:X}",
                SANITIZE_ROTATION.name, self.effective_rotation
            ));
        }
        Ok(())
    }

    /// Where `object` is before the move: its spawn location and rotation.
    ///
    /// # Safety
    /// `object` must be a live editor object.
    pub unsafe fn spawn(&self, object: *const UObjectBase) -> (Vector, Vector) {
        // SAFETY: per the contract; both offsets come from the game's own code reading them.
        unsafe {
            (
                read(object, self.spawn_location),
                read(object, self.spawn_rotation),
            )
        }
    }

    /// `EffectiveMoveLocation` and `Rotation`, where the flags make results close to them become
    /// them (see the module's documentation).
    ///
    /// # Safety
    /// `mode` must be a live move mode, and [`Self::check`] must have passed.
    pub unsafe fn effective(&self, mode: *const UObjectBase) -> (Option<Vector>, Option<Vector>) {
        // SAFETY: per the contract; `EffectiveMoveLocation` comes right before the rotation
        // (checked against reflection), the flags' offset comes from the game's code.
        unsafe {
            let flags = read::<u8>(mode, self.flags);
            let location = self.effective_rotation - size_of::<Vector>();
            (
                (flags & EFFECTIVE_LOCATION != 0).then(|| read(mode, location)),
                (flags & EFFECTIVE_ROTATION != 0).then(|| read(mode, self.effective_rotation)),
            )
        }
    }

    /// The move `mode` is making, if it's about a brick.
    ///
    /// # Safety
    /// `mode` must be a live move mode. Game thread only.
    pub unsafe fn current(&self, mode: *const UObjectBase) -> Option<Move> {
        // SAFETY: per the contract; every offset comes from the game's own code reading it, and
        // the user settings are live while the game runs (the move mode reads them each frame).
        unsafe {
            let reference = weak_object::resolve(read::<FWeakObjectPtr>(mode, self.reference))?;
            let settings = (self.user_settings)();
            let snapping = !settings.is_null() && settings.add(self.snapping).read() != 0;
            let step = |offset| {
                if snapping {
                    settings.add(offset).cast::<f32>().read_unaligned()
                } else {
                    0.0
                }
            };
            let flags = read::<u8>(mode, self.flags);
            Some(Move {
                location: read(mode, self.location),
                rotation: read(mode, self.rotation),
                folded: flags & FOLDED != 0,
                world_gizmo: flags & WORLD_GIZMO != 0,
                drag_along_gizmo: read::<u8>(mode, self.gizmo_type) != FREE_MOVE,
                drag: read(mode, self.drag),
                turn: read(mode, self.turn),
                grid: step(self.grid),
                angle: step(self.angle),
                adjustment: read(mode, self.adjustment),
                reference,
            })
        }
    }

    /// Sets the move's own place and rotation (pitch, yaw, roll), the ones [`Move`] starts from.
    ///
    /// # Safety
    /// `mode` must be a live move mode. Game thread only.
    pub unsafe fn set_place(&self, mode: *mut UObjectBase, location: Vector, rotation: Vector) {
        // SAFETY: per the contract; both offsets come from the game's own code writing them.
        unsafe {
            write(mode, self.location, location);
            write(mode, self.rotation, rotation);
        }
    }
}

/// A `T` at `offset` in `object`.
///
/// # Safety
/// `object + offset` must hold a `T`.
unsafe fn read<T: Copy>(object: *const UObjectBase, offset: usize) -> T {
    // SAFETY: per the contract.
    unsafe { object.cast::<u8>().add(offset).cast::<T>().read_unaligned() }
}

/// Writes `value` at `offset` in `object`.
///
/// # Safety
/// `object + offset` must hold a `T`, which nothing else uses meanwhile.
unsafe fn write<T: Copy>(object: *mut UObjectBase, offset: usize, value: T) {
    // SAFETY: per the contract.
    unsafe {
        object
            .cast::<u8>()
            .add(offset)
            .cast::<T>()
            .write_unaligned(value)
    }
}
