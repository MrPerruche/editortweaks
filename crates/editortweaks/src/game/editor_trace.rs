//! The vehicle editor's own collision queries, and keeping them to the editor's own components.
//!
//! What the mouse points at (`ABrickEditor::GetObjectAtScreenPosition`, BRMK 0xB2240, game 1.11.7
//! 0xCE5E00) and where a placed brick goes (`GetPlaceObjectTransform`, BRMK 0xB33B0, game
//! 0xCE66D0) both take their channel from `ABrickEditor::GetEditorTraceParams(ECollisionChannel&,
//! FCollisionQueryParams&)` (BRMK 0xACDC0, game 0xCE2860: `ECC_Visibility`). Then they call
//! `UWorld::LineTraceSingleByChannel` (game 0x243F310) and `UWorld::SweepSingleByChannel` (game
//! 0x24481F0) with the engine's default response parameters (`FCollisionResponseParams::
//! DefaultResponseParam`, game 0x4B7A148: every object type blocks). See `docs/re/map-in-editor.md`.
//!
//! While [`isolate`] names a channel, the editor's queries use it instead, and hit only components
//! whose object type is that channel: the editor's own, which `features::map_in_editor` gives it.
//! No game system uses the channel, so the editor and the world don't see each other: the editor's
//! bricks can stand in the map without physics, characters or sensors noticing them, and clicks go
//! through the map to the bricks.

use core::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use brickrust::ue::coreuobject::UObjectBase;

use crate::framework::hook;
use crate::framework::sig::{self, FunctionSig, SigMatch};

/// `GetEditorTraceParams`, by its start: the channel set to 3, then the player controller's hidden
/// components added to the query's ignored ones (the controller's offset at +12, the list's at
/// +27).
pub const TRACE_PARAMS: FunctionSig = FunctionSig {
    name: "ABrickEditor::GetEditorTraceParams",
    pattern: "c7 02 03 00 00 00 4d 8b f0 48 8b 91 ?? ?? 00 00 48 8b d9 48 85 d2 74 ?? 48 81 c2 ?? ?? 00 00 49 8b c8",
};

/// The line trace in `GetObjectAtScreenPosition`: the default response parameters (their address
/// at +3), the query parameters and channel on the stack, then the call at [`PICK_CALL`].
pub const PICK_TRACE: FunctionSig = FunctionSig {
    name: "ABrickEditor::GetObjectAtScreenPosition trace",
    pattern: "48 8d 0d ?? ?? ?? ?? 48 89 4c 24 30 4c 8d 4c 24 ?? 48 8d 4d ?? 48 89 4c 24 28 4c 8d 44 24 ?? 8b 4c 24 ?? 48 8d 55 ?? 89 4c 24 20 48 8b c8 e8",
};
const PICK_CALL: usize = 46;

/// The sweep in `GetPlaceObjectTransform`, laid out the same way with a rotation (the identity
/// quaternion) and a shape; the call at [`PLACE_CALL`].
pub const PLACE_SWEEP: FunctionSig = FunctionSig {
    name: "ABrickEditor::GetPlaceObjectTransform sweep",
    pattern: "48 8d 0d ?? ?? ?? ?? 48 89 4c 24 40 4c 8d 4d ?? 48 8d 4d ?? 48 89 4c 24 38 4c 8d 45 ?? 48 8d 4d ?? 48 89 4c 24 30 48 8d 55 ?? 8b 4c 24 ?? 89 4c 24 28 48 8d 0d ?? ?? ?? ?? 48 89 4c 24 20 48 8b c8 e8",
};
const PLACE_CALL: usize = 65;

/// `ECC_Visibility`.
const VISIBILITY: u32 = 3;
/// `ECollisionResponse`.
const IGNORE: u8 = 0;
const BLOCK: u8 = 2;
/// No channel: the editor's queries are the game's.
const NONE: u32 = u32::MAX;

/// `FCollisionResponseParams`: a response (`ECollisionResponse`) to each of the 32 object types.
#[repr(C)]
struct ResponseParams([u8; 32]);

type TraceParamsFn =
    unsafe extern "C" fn(editor: *const UObjectBase, channel: *mut u32, params: *mut c_void);
type LineTraceFn = unsafe extern "C" fn(
    world: *const c_void,
    hit: *mut c_void,
    start: *const c_void,
    end: *const c_void,
    channel: u32,
    params: *const c_void,
    responses: *const ResponseParams,
) -> bool;
type SweepFn = unsafe extern "C" fn(
    world: *const c_void,
    hit: *mut c_void,
    start: *const c_void,
    end: *const c_void,
    rotation: *const c_void,
    channel: u32,
    shape: *const c_void,
    params: *const c_void,
    responses: *const ResponseParams,
) -> bool;

static ORIGINAL_TRACE_PARAMS: OnceLock<TraceParamsFn> = OnceLock::new();
static ORIGINAL_LINE_TRACE: OnceLock<LineTraceFn> = OnceLock::new();
static ORIGINAL_SWEEP: OnceLock<SweepFn> = OnceLock::new();

/// The channel of the isolated editor's queries, or [`NONE`].
static CHANNEL: AtomicU32 = AtomicU32::new(NONE);
/// The isolated editor (its address).
static EDITOR: AtomicUsize = AtomicUsize::new(0);
/// Set while [`as_editor`] runs a query of the editor's made elsewhere.
static BORROWED: AtomicBool = AtomicBool::new(false);

/// Finds the editor's queries, checks they are the editor's, and hooks them. Until [`isolate`] is
/// called, the hooks change nothing.
pub fn install() -> Result<(), String> {
    let params = TRACE_PARAMS.locate()?;
    let pick = PICK_TRACE.locate()?;
    let place = PLACE_SWEEP.locate()?;
    for (found, name) in [(&pick, PICK_TRACE.name), (&place, PLACE_SWEEP.name)] {
        // In all its parts: `GetPlaceObjectTransform` calls it from the second of its four.
        if !sig::direct_calls_in_all_parts(found.function).contains(&params.function) {
            return Err(format!(
                "{name}: its function doesn't call {}",
                TRACE_PARAMS.name
            ));
        }
    }
    // SAFETY: both displacements lie inside their matched patterns.
    let (pick_responses, place_responses) = unsafe { (lea_target(&pick), lea_target(&place)) };
    if pick_responses != place_responses {
        return Err(format!(
            "{} and {} pass different response parameters",
            PICK_TRACE.name, PLACE_SWEEP.name
        ));
    }
    // SAFETY: both calls lie inside their matched patterns.
    let (line_trace, sweep) =
        unsafe { (pick.call_target(PICK_CALL), place.call_target(PLACE_CALL)) };
    let line_trace = line_trace.ok_or(format!("{}: no call to a function", PICK_TRACE.name))?;
    let sweep = sweep.ok_or(format!("{}: no call to a function", PLACE_SWEEP.name))?;
    // SAFETY: each target is the start of the function its detour matches: the editor's own
    // function, and the two engine functions it calls, with the arguments laid out as above.
    unsafe {
        hook::install_typed(
            TRACE_PARAMS.name,
            params.function,
            trace_params_detour as TraceParamsFn,
            &ORIGINAL_TRACE_PARAMS,
        )?;
        hook::install_typed(
            "UWorld::LineTraceSingleByChannel",
            line_trace,
            line_trace_detour as LineTraceFn,
            &ORIGINAL_LINE_TRACE,
        )?;
        hook::install_typed(
            "UWorld::SweepSingleByChannel",
            sweep,
            sweep_detour as SweepFn,
            &ORIGINAL_SWEEP,
        )
    }
}

/// From now on, `editor`'s queries use `channel` and hit only components whose object type it is;
/// with `None`, they are the game's again.
pub fn isolate(isolated: Option<(*mut UObjectBase, u8)>) {
    match isolated {
        Some((editor, channel)) => {
            EDITOR.store(editor as usize, Ordering::Relaxed);
            CHANNEL.store(u32::from(channel), Ordering::Relaxed);
        }
        None => CHANNEL.store(NONE, Ordering::Relaxed),
    }
}

/// Runs `query`, a visibility query made like the editor's clicks (e.g. what's in the middle of the
/// view), as one of the isolated editor's own.
pub fn as_editor<T>(query: impl FnOnce() -> T) -> T {
    BORROWED.store(true, Ordering::Relaxed);
    let result = query();
    BORROWED.store(false, Ordering::Relaxed);
    result
}

/// The address the `lea rcx, [rip + displacement]` starting the match loads.
///
/// # Safety
/// The match must start with that instruction.
unsafe fn lea_target(found: &SigMatch) -> usize {
    // SAFETY: per the contract; the displacement is the instruction's last 4 bytes.
    let displacement = unsafe { found.address.add(3).cast::<i32>().read_unaligned() };
    (found.address as usize + 7).wrapping_add_signed(displacement as isize)
}

/// The isolated channel, if a query on `channel` is the isolated editor's.
fn editor_channel(channel: u32) -> Option<u32> {
    let isolated = CHANNEL.load(Ordering::Relaxed);
    let editors =
        channel == isolated || (channel == VISIBILITY && BORROWED.load(Ordering::Relaxed));
    (isolated != NONE && editors).then_some(isolated)
}

/// Responses that block only objects of type `channel`.
fn only(channel: u32) -> ResponseParams {
    let mut responses = [IGNORE; 32];
    responses[channel as usize] = BLOCK;
    ResponseParams(responses)
}

unsafe extern "C" fn trace_params_detour(
    editor: *const UObjectBase,
    channel: *mut u32,
    params: *mut c_void,
) {
    let original = *ORIGINAL_TRACE_PARAMS
        .get()
        .expect("hook recorded before use");
    // SAFETY: the original function with its own arguments; `channel` is the caller's
    // `ECollisionChannel&`, which the original just wrote.
    unsafe {
        original(editor, channel, params);
        let isolated = CHANNEL.load(Ordering::Relaxed);
        if isolated != NONE && EDITOR.load(Ordering::Relaxed) == editor as usize {
            channel.write(isolated);
        }
    }
}

unsafe extern "C" fn line_trace_detour(
    world: *const c_void,
    hit: *mut c_void,
    start: *const c_void,
    end: *const c_void,
    channel: u32,
    params: *const c_void,
    responses: *const ResponseParams,
) -> bool {
    let original = *ORIGINAL_LINE_TRACE.get().expect("hook recorded before use");
    // SAFETY: the original function with its own arguments, or with the editor's channel and
    // responses in place of the caller's (both live for the call).
    unsafe {
        match editor_channel(channel) {
            Some(isolated) => original(world, hit, start, end, isolated, params, &only(isolated)),
            None => original(world, hit, start, end, channel, params, responses),
        }
    }
}

unsafe extern "C" fn sweep_detour(
    world: *const c_void,
    hit: *mut c_void,
    start: *const c_void,
    end: *const c_void,
    rotation: *const c_void,
    channel: u32,
    shape: *const c_void,
    params: *const c_void,
    responses: *const ResponseParams,
) -> bool {
    let original = *ORIGINAL_SWEEP.get().expect("hook recorded before use");
    // SAFETY: as in `line_trace_detour`.
    unsafe {
        match editor_channel(channel) {
            Some(isolated) => original(
                world,
                hit,
                start,
                end,
                rotation,
                isolated,
                shape,
                params,
                &only(isolated),
            ),
            None => original(
                world, hit, start, end, rotation, channel, shape, params, responses,
            ),
        }
    }
}
