//! A turned editor's camera, made consistent. The game never turns the editor, so its camera code
//! only has to be right while the editor's rotation is the identity, and four of its functions
//! are only right then; `features/map_in_editor` turns the editor (a garage's yaw), so it needs
//! [`is_ready`]. The camera is a location relative to the editor, in the editor's space, and a
//! rotation in the world's (`C`; the editor's rotation is `R`). Every use of the two in the
//! editor's code was checked (found 2026-10-15, see `docs/re/map-in-editor.md`, "The turned
//! editor's camera"); the four that aren't right with a turn:
//!
//! - `GetCameraViewPoint` adds the relative location to the editor's place without turning it.
//!   Corrected: the view is at the editor's place plus `R` times the relative location.
//! - `UpdateViewRotation` turns the relative location around the selection (in the editor's
//!   space) by the rotation's change about the world's axes. Corrected: it runs with the rotation
//!   it keeps and the one it's given brought into the editor's space (`R⁻¹ · C`), then back.
//! - `AddCameraMovementInput` (the fly keys) and the move mode's `TickMode` (moving bricks with the
//!   mouse or the keys) take the camera's rotation into the editor's space as `C · R⁻¹`, where it is
//!   `R⁻¹ · C`: with a pitched camera their axes come out wrong (forward stays level, sideways
//!   climbs). Corrected: they run with the stored rotation replaced by `R⁻¹ · C · R`, whose
//!   `· R⁻¹` is the right rotation, then the stored one is put back.
//!
//! Nothing changes while no editor is turned ([`set`] says which one is): every hook then calls
//! straight through.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, Once, OnceLock, TryLockError};

use brickrust::ue::coreuobject::UObjectBase;

use crate::framework::hook;
use crate::framework::sig::SigMatch;
use crate::game::editor_camera::{
    ADD_CAMERA_MOVEMENT_INPUT, GET_CAMERA_VIEW_POINT, MOVE_MODE_CAMERA, UPDATE_VIEW_ROTATION,
};
use crate::math::{self, Quat, Vec3};
use crate::prelude::*;

pub struct EditorFrame;

/// Which editor is turned, where it is, and what its camera is called on.
#[derive(Clone, Copy)]
struct Frame {
    /// The editor's address, and that of its `IViewTargetInterface`.
    editor: usize,
    interface: usize,
    /// The editor's place in the world.
    location: Vec3,
    rotation: Quat,
}

static FRAME: Mutex<Option<Frame>> = Mutex::new(None);
/// Whether [`FRAME`] is set, to leave every call alone without taking its lock.
static TURNED: AtomicBool = AtomicBool::new(false);
static READY: AtomicBool = AtomicBool::new(false);
/// Offset in the interface of the relative location, read from the game's code.
static RELATIVE_LOCATION: OnceLock<usize> = OnceLock::new();
/// Offset in the editor of the camera's rotation, read from the game's code.
static CAMERA_ROTATION: OnceLock<usize> = OnceLock::new();

type ViewPointFn =
    unsafe extern "C" fn(interface: *mut u8, location: *mut [f32; 3], rotation: *mut [f32; 3]);
type UpdateRotationFn =
    unsafe extern "C" fn(interface: *mut u8, rotation: *mut [f32; 3], delta: f32);
type MovementInputFn = unsafe extern "C" fn(editor: *mut u8, axis: u32, value: f32);
type TickModeFn = unsafe extern "C" fn(mode: *mut UObjectBase, delta: f32);

static ORIGINAL_VIEW_POINT: OnceLock<ViewPointFn> = OnceLock::new();
static ORIGINAL_UPDATE_ROTATION: OnceLock<UpdateRotationFn> = OnceLock::new();
static ORIGINAL_MOVEMENT_INPUT: OnceLock<MovementInputFn> = OnceLock::new();
static ORIGINAL_TICK_MODE: OnceLock<TickModeFn> = OnceLock::new();

/// Offset in the interface of the view's rotation: both functions' patterns hold it.
const ROTATION: usize = 0x10;

/// How many outers up from a move mode its editor may be (it's made inside the editor).
const MAX_OUTERS: usize = 8;

impl Feature for EditorFrame {
    fn id(&self) -> &'static str {
        "editor_frame"
    }

    fn on_load(&self) -> LoadResult {
        let view_point = GET_CAMERA_VIEW_POINT.locate()?;
        // SAFETY: displacements inside the matched pattern (see its comment).
        let (x, y, z, rotation) = unsafe {
            (
                read_offset(&view_point, 69),
                read_offset(&view_point, 61),
                read_offset(&view_point, 49),
                view_point.address.add(91).read() as usize,
            )
        };
        if y != x + 4 || z != x + 8 || rotation != ROTATION {
            return Err(format!(
                "{}: unexpected offsets 0x{x:X}, 0x{y:X}, 0x{z:X}, 0x{rotation:X}",
                GET_CAMERA_VIEW_POINT.name
            ));
        }
        let camera_rotation = camera_rotation_in_editor()?;
        let movement = ADD_CAMERA_MOVEMENT_INPUT.locate()?;
        if movement.address != movement.function {
            return Err(format!(
                "{}: the pattern isn't at a function's start",
                ADD_CAMERA_MOVEMENT_INPUT.name
            ));
        }
        let tick_mode = MOVE_MODE_CAMERA.resolve()?;
        let update = UPDATE_VIEW_ROTATION.resolve()?;
        let _ = RELATIVE_LOCATION.set(x);
        let _ = CAMERA_ROTATION.set(camera_rotation);
        // SAFETY: each target is the start of the function its detour matches.
        unsafe {
            hook::install_typed(
                GET_CAMERA_VIEW_POINT.name,
                view_point.function,
                view_point_detour as ViewPointFn,
                &ORIGINAL_VIEW_POINT,
            )?;
            hook::install_typed(
                UPDATE_VIEW_ROTATION.name,
                update,
                update_rotation_detour as UpdateRotationFn,
                &ORIGINAL_UPDATE_ROTATION,
            )?;
            hook::install_typed(
                ADD_CAMERA_MOVEMENT_INPUT.name,
                movement.function,
                movement_input_detour as MovementInputFn,
                &ORIGINAL_MOVEMENT_INPUT,
            )?;
            hook::install_typed(
                "UBrickEditorMoveMode::TickMode",
                tick_mode,
                tick_mode_detour as TickModeFn,
                &ORIGINAL_TICK_MODE,
            )?;
        }
        READY.store(true, Ordering::Relaxed);
        Ok(())
    }
}

/// The camera rotation's offset in the editor, as both functions that take it into the editor's
/// space read it: they must agree, with the yaw and roll 8 bytes on.
fn camera_rotation_in_editor() -> Result<usize, String> {
    let movement = ADD_CAMERA_MOVEMENT_INPUT.locate()?;
    let tick_mode = MOVE_MODE_CAMERA.locate()?;
    // SAFETY: displacements inside the matched patterns (see their comments).
    let offsets = unsafe {
        [
            read_offset(&movement, 76),
            read_offset(&movement, 87),
            read_offset(&tick_mode, 4),
            read_offset(&tick_mode, 10),
        ]
    };
    let [rotation, rest, mode_rotation, mode_rest] = offsets;
    if mode_rotation != rotation || rest != rotation + 8 || mode_rest != rotation + 8 {
        return Err(format!(
            "the camera's rotation: unexpected offsets {offsets:X?} ({} and {})",
            ADD_CAMERA_MOVEMENT_INPUT.name, MOVE_MODE_CAMERA.name
        ));
    }
    Ok(rotation)
}

/// The 32-bit displacement at `at` in a match.
///
/// # Safety
/// `at` must be the place of a displacement inside `found`'s pattern.
unsafe fn read_offset(found: &SigMatch, at: usize) -> usize {
    // SAFETY: per the contract.
    unsafe { found.address.add(at).cast::<u32>().read_unaligned() as usize }
}

/// Where the relative location is in the editor's view target interface, as the game's code reads
/// it.
pub fn relative_location_offset() -> Option<usize> {
    RELATIVE_LOCATION.get().copied()
}

/// Where the camera's rotation is in the editor, as the game's code that moves the camera and
/// bricks reads it (the view target interface's offset plus [`view_rotation_offset`], if all is as
/// expected).
pub fn camera_rotation_offset() -> Option<usize> {
    CAMERA_ROTATION.get().copied()
}

/// Where the camera's rotation is in the editor's view target interface.
pub fn view_rotation_offset() -> usize {
    ROTATION
}

/// Whether the camera can be kept consistent for a turned editor (every function is hooked).
pub fn is_ready() -> bool {
    READY.load(Ordering::Relaxed)
}

/// Says that `editor` (its `IViewTargetInterface` at `interface`) is at `location`, turned by
/// `rotation`: its camera is then kept consistent.
pub fn set(editor: usize, interface: usize, location: Vec3, rotation: Quat) {
    *lock() = Some(Frame {
        editor,
        interface,
        location,
        rotation,
    });
    TURNED.store(true, Ordering::Relaxed);
}

/// Says that the editor isn't turned (any more).
pub fn clear() {
    TURNED.store(false, Ordering::Relaxed);
    *lock() = None;
}

/// Turns the camera's rotation (in the world) of the editor whose view target interface is at
/// `interface` by `turn`. When the editor turns, the player's view rotation turns with it so the
/// camera keeps showing the vehicle the same way; the game only copies the view rotation to the
/// camera's in `UpdateViewRotation`, the next frame, so this one would show the turned place with
/// the old rotation.
///
/// # Safety
/// `interface` must be a live editor's view target interface. Game thread only.
pub unsafe fn turn_camera(interface: usize, turn: Quat) {
    // SAFETY: per the contract; the rotation is at `ROTATION` in the interface (checked in
    // `on_load`).
    unsafe {
        let stored = (interface as *mut u8).add(ROTATION).cast::<[f32; 3]>();
        let camera = Quat::from_rotator(math::widen(stored.read_unaligned()));
        stored.write_unaligned(math::narrow((turn * camera).to_rotator()));
    }
}

/// A direction (or distance) in the world, in the editor's space: what moving the camera by it
/// means to the game, which keeps its location in the editor's space. The same while the editor
/// isn't turned.
pub fn to_editor_space(direction: Vec3) -> Vec3 {
    match current() {
        Some(frame) => frame.rotation.inverse().rotate(direction),
        None => direction,
    }
}

/// The reverse of [`to_editor_space`].
pub fn to_world_space(direction: Vec3) -> Vec3 {
    match current() {
        Some(frame) => frame.rotation.rotate(direction),
        None => direction,
    }
}

fn lock() -> MutexGuard<'static, Option<Frame>> {
    FRAME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn current() -> Option<Frame> {
    if !TURNED.load(Ordering::Relaxed) {
        return None;
    }
    match FRAME.try_lock() {
        Ok(frame) => *frame,
        Err(TryLockError::Poisoned(poisoned)) => *poisoned.into_inner(),
        Err(TryLockError::WouldBlock) => None,
    }
}

/// The turned editor's frame if `interface` is its camera's.
fn frame_of(interface: *mut u8) -> Option<Frame> {
    current().filter(|frame| frame.interface == interface as usize)
}

/// The rotation to store so that the game's `C · R⁻¹` comes out as the camera's rotation in the
/// editor's space, `R⁻¹ · C` (`C` the camera's rotation `world`, `R` the editor's `turn`):
/// `R⁻¹ · C · R`. As a rotator, it may have a roll; the game makes a quaternion of it either way.
fn presented_rotation(world: [f32; 3], turn: Quat) -> [f32; 3] {
    let camera = Quat::from_rotator(math::widen(world));
    math::narrow((turn.inverse() * camera * turn).to_rotator())
}

/// Runs `call`, a game function that takes the camera's rotation into the editor's space as
/// `C · R⁻¹`, with the stored rotation of `editor` (the turned one) replaced by
/// [`presented_rotation`], then puts back the world's exactly. Neither such function writes the
/// rotation; if one did, the write is logged once and dropped.
///
/// # Safety
/// `editor` must be the live editor `frame` is about. Game thread only.
unsafe fn with_presented_rotation(editor: *mut u8, frame: &Frame, call: impl FnOnce()) {
    static LOG_WRITTEN: Once = Once::new();
    let Some(&offset) = CAMERA_ROTATION.get() else {
        return call();
    };
    // SAFETY: per the contract; the rotation is at `offset` in the editor (read from these
    // functions' own code in `on_load`).
    unsafe {
        let stored = editor.add(offset).cast::<[f32; 3]>();
        let world = stored.read_unaligned();
        let presented = presented_rotation(world, frame.rotation);
        stored.write_unaligned(presented);
        call();
        if stored.read_unaligned() != presented {
            LOG_WRITTEN.call_once(|| {
                log!("the game changed the camera's rotation while it was presented; put back")
            });
        }
        stored.write_unaligned(world);
    }
}

/// Whether `editor` is `object` or one of its outers.
///
/// # Safety
/// `object` must be a live object. Game thread only.
unsafe fn is_inside(object: *const UObjectBase, editor: usize) -> bool {
    let mut current = object;
    for _ in 0..MAX_OUTERS {
        if current.is_null() {
            return false;
        }
        if current as usize == editor {
            return true;
        }
        // SAFETY: per the contract; an object's outers are live objects (or null).
        current = unsafe { (*current).outer_private.cast() };
    }
    false
}

unsafe extern "C" fn view_point_detour(
    interface: *mut u8,
    location: *mut [f32; 3],
    rotation: *mut [f32; 3],
) {
    let original = *ORIGINAL_VIEW_POINT.get().expect("hook recorded before use");
    // SAFETY: the original function with its own arguments.
    unsafe { original(interface, location, rotation) };
    let (Some(frame), Some(&offset)) = (frame_of(interface), RELATIVE_LOCATION.get()) else {
        return;
    };
    // SAFETY: the game calls this with its interface and two vectors; the relative location is at
    // `offset` in the interface (checked in `on_load`).
    unsafe {
        let relative = interface.add(offset).cast::<[f32; 3]>().read_unaligned();
        let place = math::add(frame.location, frame.rotation.rotate(math::widen(relative)));
        location.write(math::narrow(place));
    }
}

unsafe extern "C" fn update_rotation_detour(
    interface: *mut u8,
    rotation: *mut [f32; 3],
    delta: f32,
) {
    let original = *ORIGINAL_UPDATE_ROTATION
        .get()
        .expect("hook recorded before use");
    let Some(frame) = frame_of(interface) else {
        // SAFETY: the original function with its own arguments.
        return unsafe { original(interface, rotation, delta) };
    };
    // The result is the rotator of the turned rotation nearest the one given, so a camera that
    // goes over the top (see `features/free_pitch`: pitch beyond ±90°, no roll) keeps its form,
    // which the game's limits would otherwise mangle: its roll is held at zero.
    let convert = |by: Quat, rotator: [f32; 3]| {
        let turned = by * Quat::from_rotator(math::widen(rotator));
        math::narrow(turned.to_rotator_near(math::widen(rotator)))
    };
    let into = frame.rotation.inverse();
    // SAFETY: the game calls this with its interface (whose rotation is at `ROTATION`) and a
    // rotator. Both rotations go into the editor's space for the call and back after, so the game
    // finds them there like the relative location and the bricks. What the game leaves as it
    // was given is put back exactly as it was (no rounding piles up in a view that doesn't
    // move), and a rotator that is the stored one itself is converted once.
    unsafe {
        let stored = interface.add(ROTATION).cast::<[f32; 3]>();
        let same = core::ptr::eq(stored, rotation);
        let (world_stored, world_given) = (stored.read_unaligned(), rotation.read_unaligned());
        let (local_stored, local_given) = (convert(into, world_stored), convert(into, world_given));
        stored.write_unaligned(local_stored);
        if !same {
            rotation.write_unaligned(local_given);
        }
        original(interface, rotation, delta);
        // A changed rotator comes back as the one nearest the world's it replaces.
        let back = |now: [f32; 3], local: [f32; 3], world: [f32; 3]| {
            if now == local {
                world
            } else {
                let turned = frame.rotation * Quat::from_rotator(math::widen(now));
                math::narrow(turned.to_rotator_near(math::widen(world)))
            }
        };
        let (now_stored, now_given) = (stored.read_unaligned(), rotation.read_unaligned());
        stored.write_unaligned(back(now_stored, local_stored, world_stored));
        if !same {
            rotation.write_unaligned(back(now_given, local_given, world_given));
        }
    }
}

unsafe extern "C" fn movement_input_detour(editor: *mut u8, axis: u32, value: f32) {
    let original = *ORIGINAL_MOVEMENT_INPUT
        .get()
        .expect("hook recorded before use");
    match current().filter(|frame| frame.editor == editor as usize) {
        // SAFETY: the game calls this on its live editor, the turned one; the original function
        // with its own arguments.
        Some(frame) => unsafe {
            with_presented_rotation(editor, &frame, || original(editor, axis, value))
        },
        // SAFETY: the original function with its own arguments.
        None => unsafe { original(editor, axis, value) },
    }
}

unsafe extern "C" fn tick_mode_detour(mode: *mut UObjectBase, delta: f32) {
    let original = *ORIGINAL_TICK_MODE.get().expect("hook recorded before use");
    // SAFETY: the game calls this on a live move mode, on the game thread.
    let frame = current().filter(|frame| unsafe { is_inside(mode, frame.editor) });
    match frame {
        // SAFETY: the mode is inside the turned editor, which is live while its mode ticks; the
        // original function with its own arguments.
        Some(frame) => unsafe {
            with_presented_rotation(frame.editor as *mut u8, &frame, || original(mode, delta))
        },
        // SAFETY: the original function with its own arguments.
        None => unsafe { original(mode, delta) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the game does with the stored rotation in the two functions: its quaternion times the
    /// editor's inverse, in that order.
    fn game_relative(stored: [f32; 3], turn: Quat) -> Quat {
        Quat::from_rotator(math::widen(stored)) * turn.inverse()
    }

    #[test]
    fn presented_rotation_gives_the_camera_in_the_editors_space() {
        for yaw in [0.0, 90.0, -90.0, 97.09, 134.5, 180.0] {
            let turn = Quat::from_rotator([0.0, yaw, 0.0]);
            for pitch in [-89.5, -60.0, -35.0, 0.0, 30.0, 89.5] {
                for camera_yaw in [-150.0, 0.0, 20.0, 179.0] {
                    let world = [pitch as f32, camera_yaw as f32, 0.0];
                    let camera = Quat::from_rotator(math::widen(world));
                    let wanted = turn.inverse() * camera;
                    let got = game_relative(presented_rotation(world, turn), turn);
                    for axis in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
                        let (want, have) = (wanted.rotate(axis), got.rotate(axis));
                        let gap = math::length(math::sub(want, have));
                        assert!(
                            gap < 1e-5,
                            "yaw {yaw}, camera {world:?}, axis {axis:?}: {want:?} vs {have:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn an_upside_down_camera_stays_upside_down() {
        // Pitch 100° is the same rotation as pitch 80°, yaw and roll half a turn on.
        let camera = [100.0, 30.0, 0.0];
        let rotation = Quat::from_rotator(camera);
        let near = rotation.to_rotator_near(camera);
        for (got, want) in near.into_iter().zip(camera) {
            assert!((got - want).abs() < 1e-6, "{near:?} vs {camera:?}");
        }
        // With no reference near it, the pitch within ±90° is the nearest.
        let level = rotation.to_rotator_near([80.0, -150.0, 180.0]);
        assert!(level[0].abs() <= 90.0, "{level:?}");
    }

    #[test]
    fn the_games_order_is_wrong_with_a_pitched_camera() {
        let turn = Quat::from_rotator([0.0, 90.0, 0.0]);
        let world = [-35.0, 20.0, 0.0];
        let camera = Quat::from_rotator(math::widen(world));
        let forward = game_relative(world, turn).rotate([1.0, 0.0, 0.0]);
        let wanted = (turn.inverse() * camera).rotate([1.0, 0.0, 0.0]);
        // The game's forward stays level; the right one goes down with the camera.
        assert!(forward[2].abs() < 1e-9, "{forward:?}");
        assert!(wanted[2] < -0.5, "{wanted:?}");
    }
}
