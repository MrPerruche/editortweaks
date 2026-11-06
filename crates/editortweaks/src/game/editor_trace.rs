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

use core::cell::Cell;
use core::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Once, OnceLock};

use brickrust::ue::coreuobject::UObjectBase;

use crate::framework::hook;
use crate::framework::sig::{self, FunctionSig, SigMatch};
use crate::framework::timings;
use crate::prelude::log;

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

/// What makes a sweep through the engine's Blueprint library (`UKismetSystemLibrary`) the query the
/// game's own C++ makes: the library can't ask for these.
#[derive(Clone, Copy, Debug)]
pub struct GameQuery {
    /// The channel to trace, if not the one the library was asked for.
    pub channel: Option<u32>,
    /// Object types that block, on top of those the library's responses block.
    pub blocks: &'static [u8],
    /// `FCollisionQueryParams::bFindInitialOverlaps`: false makes the sweep ignore what it starts
    /// inside of.
    pub find_initial_overlaps: bool,
    /// `FCollisionQueryParams::bIgnoreTouches`.
    pub ignore_touches: bool,
}

thread_local! {
    static GAME_QUERY: Cell<Option<GameQuery>> = const { Cell::new(None) };
    static GAME_QUERY_RAN: Cell<bool> = const { Cell::new(false) };
}

/// Runs `sweep`, a sweep made through the engine's Blueprint library, as `query` says, and whether
/// the hook on the engine's sweep saw it (else it ran as the library made it).
pub fn as_game_query<T>(query: GameQuery, sweep: impl FnOnce() -> T) -> (T, bool) {
    GAME_QUERY.set(Some(query));
    GAME_QUERY_RAN.set(false);
    let result = sweep();
    GAME_QUERY.set(None);
    (result, GAME_QUERY_RAN.get())
}

/// Offsets of the flags of `FCollisionQueryParams` in the shipping game, after two 8-byte names (in
/// BRMK, with 12-byte names, they're 8 bytes later: `ABrickVehicle::GetVehicleSpawnOffset` and
/// `ABrickPlayerController::SweepPawnOrMarker` write the first two there).
const QUERY_COMPLEX: usize = 0x10;
const QUERY_FIND_INITIAL_OVERLAPS: usize = 0x11;
const QUERY_RETURN_PHYSICAL_MATERIAL: usize = 0x13;
const QUERY_IGNORE_BLOCKS: usize = 0x14;
const QUERY_IGNORE_TOUCHES: usize = 0x15;

/// Sets the flags of `params`, the library's `FCollisionQueryParams` on its stack, if they are where
/// they should be: the library asks for a simple trace returning physical materials, the
/// constructor finds initial overlaps, and nothing is ignored. Whether it could.
///
/// # Safety
/// `params` must be the live, writable `FCollisionQueryParams` of a library trace asking for simple
/// collision.
unsafe fn set_query_flags(params: *const c_void, query: GameQuery) -> bool {
    let params = params.cast_mut().cast::<u8>();
    // SAFETY: per the contract; every offset lies within the struct.
    unsafe {
        let is = |offset: usize, value: u8| params.add(offset).read() == value;
        let found = is(QUERY_COMPLEX, 0)
            && is(QUERY_FIND_INITIAL_OVERLAPS, 1)
            && is(QUERY_RETURN_PHYSICAL_MATERIAL, 1)
            && is(QUERY_IGNORE_BLOCKS, 0)
            && is(QUERY_IGNORE_TOUCHES, 0);
        if found {
            params
                .add(QUERY_FIND_INITIAL_OVERLAPS)
                .write(u8::from(query.find_initial_overlaps));
            params
                .add(QUERY_IGNORE_TOUCHES)
                .write(u8::from(query.ignore_touches));
        }
        found
    }
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
    let _span = timings::span("game::editor_trace::trace_params_detour");
    let original = *ORIGINAL_TRACE_PARAMS
        .get()
        .expect("hook recorded before use");
    // SAFETY: the original function with its own arguments; `channel` is the caller's
    // `ECollisionChannel&`, which the original just wrote.
    unsafe {
        timings::game(TRACE_PARAMS.name, || original(editor, channel, params));
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
    let _span = timings::span("game::editor_trace::line_trace_detour");
    let original = *ORIGINAL_LINE_TRACE.get().expect("hook recorded before use");
    // SAFETY: the original function with its own arguments, or with the editor's channel and
    // responses in place of the caller's (both live for the call).
    unsafe {
        match editor_channel(channel) {
            Some(isolated) => timings::game("UWorld::LineTraceSingleByChannel", || {
                original(world, hit, start, end, isolated, params, &only(isolated))
            }),
            None => timings::game("UWorld::LineTraceSingleByChannel", || {
                original(world, hit, start, end, channel, params, responses)
            }),
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
    let _span = timings::span("game::editor_trace::sweep_detour");
    let original = *ORIGINAL_SWEEP.get().expect("hook recorded before use");
    if let Some(query) = GAME_QUERY.get() {
        static FLAGS_MOVED: Once = Once::new();
        GAME_QUERY_RAN.set(true);
        // SAFETY: `as_game_query` runs library traces of simple collision only, whose parameters
        // are on the library's stack for the call; `responses` is valid for the call.
        unsafe {
            if !set_query_flags(params, query) {
                FLAGS_MOVED.call_once(|| {
                    log!(
                        "a collision query's flags aren't where expected: where a vehicle spawns \
                         may be off in tight places"
                    )
                });
            }
            let mut blocking = (*responses).0;
            for &object_type in query.blocks {
                blocking[usize::from(object_type)] = BLOCK;
            }
            return timings::game("UWorld::SweepSingleByChannel", || {
                original(
                    world,
                    hit,
                    start,
                    end,
                    rotation,
                    query.channel.unwrap_or(channel),
                    shape,
                    params,
                    &ResponseParams(blocking),
                )
            });
        }
    }
    // SAFETY: as in `line_trace_detour`.
    unsafe {
        match editor_channel(channel) {
            Some(isolated) => timings::game("UWorld::SweepSingleByChannel", || {
                original(
                    world,
                    hit,
                    start,
                    end,
                    rotation,
                    isolated,
                    shape,
                    params,
                    &only(isolated),
                )
            }),
            None => timings::game("UWorld::SweepSingleByChannel", || {
                original(
                    world, hit, start, end, rotation, channel, shape, params, responses,
                )
            }),
        }
    }
}
