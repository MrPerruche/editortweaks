//! The editor's moves and turns computed in double precision, so bricks don't drift.
//!
//! The editor computes each moved brick's new place in `f32`: the move's rotation from its angles
//! (sines and cosines from a polynomial), quaternion products, the brick's place relative to the
//! one grabbed, and back to angles. Every step rounds, so turning bricks by 90° gives rotations
//! like 89.99999° and places a hair off, and repeated edits pile these up.
//!
//! This computes the same move again in `f64`, from the same inputs: the bricks' stored places and
//! rotations, and the move as the move mode keeps it (its place and rotation, the mouse's drag and
//! turn snapped like the game snaps them, the brick snapping adjustment; see `game::move_mode`).
//! Only the final result is rounded, to the `f32` the game stores: the one nearest the exact
//! result, so a turn that should give 90° gives 90°. Nothing is rounded to steps; only the `f64`
//! noise of turning near zero (below 1e-9) is stored as zero, which `f32` would otherwise keep
//! (`math::narrow_settled`).
//!
//! The move mode also rounds midway: when the gizmo's axis changes (releasing a gizmo handle,
//! grabbing another), it folds the input so far into the move's own place and rotation, in `f32`.
//! That fold is computed again too, and kept in `f64` for the rest of the move; the game gets the
//! nearest `f32` values.
//!
//! The results replace the game's only where they agree with them to within the game's own
//! rounding ([`ROTATION_MATCH`], [`LOCATION_MATCH`]): where the move isn't the plain one modeled
//! here (mirror mode counterparts, moves the editor's bounds clamp), the game's result stays. Like
//! the game, rotations and places a move leaves as they were stay exactly as they were. Only the
//! editor changes: not how vehicles drive, and a vehicle saved this way loads the same without the
//! mod.
//!
//! How: `UBrickEditorMoveMode::SanitizeObjectTransform`, the last step of every brick's move, and
//! `ApplyAccumulatedInput`, the fold, are hooked; their results are replaced after they run.
//!
//! Other features moving bricks through the move mode use the same math ([`effective`],
//! [`carried`]) and give the move its place through [`place_move`], which keeps it exact the same
//! way.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, Once, OnceLock, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;

use crate::framework::hook;
use crate::game::editor_object::Vector;
use crate::game::move_mode::{
    self, Adjustment, ApplyInputFn, Move, MoveMode, MoveTransform, SanitizeFn, Sanitized,
};
use crate::math::{self, Quat, Vec3};
use crate::prelude::*;

/// How far the game's rotation for a brick may be from ours for ours to replace it (quaternion
/// components; about 0.001°). The game's own rounding stays far below (about 0.00002°); a move
/// other than the one modeled here differs by far more.
const ROTATION_MATCH: f64 = 1e-5;
/// The same for places (cm): this much, plus this share of the distance from the vehicle's center
/// (the game's rounding grows with it).
const LOCATION_MATCH: f64 = 1e-3;
const LOCATION_MATCH_SHARE: f64 = 1e-6;

pub struct PreciseMoves;

static ORIGINAL_SANITIZE: OnceLock<SanitizeFn> = OnceLock::new();
static ORIGINAL_APPLY_INPUT: OnceLock<ApplyInputFn> = OnceLock::new();
/// Set once the offsets are checked against reflection; until then the hooks change nothing.
static READY: AtomicBool = AtomicBool::new(false);

/// The last fold (or place given by [`place_move`]), in `f64`.
static FOLD: Mutex<Option<Fold>> = Mutex::new(None);

/// A fold of a move's input into its own place and rotation.
#[derive(Clone, Copy)]
struct Fold {
    /// The move mode's address.
    mode: usize,
    /// The place and rotation (pitch, yaw, roll) the game was given: the nearest to `exact`.
    location: Vector,
    rotation: Vector,
    exact: Place,
}

/// Logs the first brick computed again and the first fold, to tell in the log what happens, and
/// the first few results left as the game computed them, with what they were computed from.
static LOG_EXACT: Once = Once::new();
static LOG_FOLD: Once = Once::new();
static MISMATCHES: AtomicU32 = AtomicU32::new(0);
const LOGGED_MISMATCHES: u32 = 5;

impl Feature for PreciseMoves {
    fn id(&self) -> &'static str {
        "precise_moves"
    }

    fn on_load(&self) -> LoadResult {
        let found = move_mode::shared()?;
        let (sanitize, apply_input) = (found.sanitize, found.apply_input);
        // SAFETY: each target is the start of the function its signature names, and each detour
        // has that function's signature.
        unsafe {
            hook::install_typed(
                move_mode::SANITIZE.name,
                sanitize,
                sanitize_detour as SanitizeFn,
                &ORIGINAL_SANITIZE,
            )?;
            hook::install_typed(
                move_mode::APPLY_INPUT.name,
                apply_input,
                apply_input_detour as ApplyInputFn,
                &ORIGINAL_APPLY_INPUT,
            )
        }
    }

    fn on_engine_init(&self) {
        match move_mode::shared().map(MoveMode::check) {
            Ok(Ok(())) => {
                READY.store(true, Ordering::Relaxed);
                log!("active");
            }
            Ok(Err(reason)) => log!("inactive: {reason}"),
            Err(_) => {}
        }
    }
}

/// The offsets, once checked.
fn found() -> Option<&'static MoveMode> {
    move_mode::shared()
        .ok()
        .filter(|_| READY.load(Ordering::Relaxed))
}

unsafe extern "C" fn sanitize_detour(
    mode: *mut UObjectBase,
    out: *mut Sanitized,
    object: *const UObjectBase,
    moved: *const MoveTransform,
) -> *mut Sanitized {
    let original = *ORIGINAL_SANITIZE.get().expect("hook recorded before use");
    // SAFETY: the original function with its own arguments.
    let result = unsafe { original(mode, out, object, moved) };
    if let Some(found) = found() {
        // SAFETY: the game's own arguments: a live move mode and editor object, the transform the
        // move gives the object, and the result the original just wrote.
        unsafe { replace(found, mode, &mut *out, object, &*moved) };
    }
    result
}

unsafe extern "C" fn apply_input_detour(mode: *mut UObjectBase) {
    let original = *ORIGINAL_APPLY_INPUT
        .get()
        .expect("hook recorded before use");
    // SAFETY: the game's own argument, a live move mode.
    let before = found().and_then(|found| unsafe { found.current(mode) });
    // SAFETY: the original function with its own argument.
    unsafe { original(mode) };
    if let (Some(found), Some(before)) = (found(), before) {
        // SAFETY: as above.
        unsafe { replace_fold(found, mode, &before) };
    }
}

/// A place and rotation in `f64`.
#[derive(Clone, Copy, Debug)]
pub struct Place {
    pub location: Vec3,
    pub rotation: Quat,
}

impl Place {
    /// A stored location and rotator.
    pub fn stored(location: Vector, rotation: Vector) -> Self {
        Self {
            location: math::widen(location),
            rotation: Quat::from_rotator(math::widen(rotation)),
        }
    }

    /// How far `self` is from `game`, as a share of how far it may be to replace it: at most 1 to
    /// agree (see [`ROTATION_MATCH`], [`LOCATION_MATCH`]).
    fn apart(&self, game: &Place) -> f64 {
        let rotation = self.rotation.distance(game.rotation);
        let location = (0..3)
            .map(|axis| (self.location[axis] - game.location[axis]).abs())
            .fold(0.0, f64::max);
        let magnitude = self
            .location
            .into_iter()
            .fold(0.0, |largest, value| f64::max(largest, value.abs()));
        f64::max(
            rotation / ROTATION_MATCH,
            location / (LOCATION_MATCH + LOCATION_MATCH_SHARE * magnitude),
        )
    }
}

/// Replaces the game's result `out` for `object` by the move computed in `f64`, where they agree.
///
/// # Safety
/// `mode` must be a live move mode, `object` a live editor object. Game thread only.
unsafe fn replace(
    found: &MoveMode,
    mode: *const UObjectBase,
    out: &mut Sanitized,
    object: *const UObjectBase,
    moved: &MoveTransform,
) {
    // SAFETY: per the contract; the reference is a live editor object (the move mode's).
    let (Some(current), (location, rotation), (effective_location, effective_rotation)) = (unsafe {
        (
            found.current(mode),
            found.spawn(object),
            found.effective(mode),
        )
    }) else {
        return;
    };
    // SAFETY: as above.
    let (reference_location, reference_rotation) = unsafe { found.spawn(current.reference) };
    let reference = Place::stored(reference_location, reference_rotation);
    let brick = Place::stored(location, rotation);

    // The game's result, to agree with.
    let [x, y, z, _] = moved.location;
    let game = Place {
        location: math::widen([x, y, z]),
        rotation: Quat::widen(moved.rotation),
    };
    let input = with_input(&current, base(mode, &current));
    let mut place = carried(&adjusted(&input, &current.adjustment), &reference, &brick);
    // The panel's numbers are the reference brick's move without the brick snapping adjustment.
    if object == current.reference.cast_const() {
        let unadjusted = carried(&input, &reference, &brick);
        if unadjusted.apart(&game) < place.apart(&game) {
            place = unadjusted;
        }
    }
    if place.apart(&game) > 1.0 {
        log_mismatch(
            "a move",
            &place,
            &game,
            &current,
            Some((&brick, &reference)),
        );
        return;
    }

    let before = *out;
    // Like the game, leave what it kept as it was (or as the move began).
    if out.rotation != rotation && Some(out.rotation) != effective_rotation {
        out.rotation = math::narrow_settled(place.rotation.to_rotator());
    }
    if out.location != location && Some(out.location) != effective_location {
        out.location = math::narrow_settled(place.location);
    }
    if out.rotation != before.rotation || out.location != before.location {
        LOG_EXACT.call_once(|| {
            log!(
                "first move computed in double precision: rotation {:?} instead of the game's \
                 {:?}, location {:?} instead of {:?}",
                out.rotation,
                before.rotation,
                out.location,
                before.location
            )
        });
    }
}

/// After the game folded the input of the move `before` into the move's own place and rotation:
/// replaces them by the nearest to the fold computed in `f64`, which is kept for what follows,
/// where they agree.
///
/// # Safety
/// `mode` must be a live move mode. Game thread only.
unsafe fn replace_fold(found: &MoveMode, mode: *mut UObjectBase, before: &Move) {
    // Without input the game folds nothing.
    if before.drag == [0.0; 3] && before.turn == [0.0; 3] {
        return;
    }
    let exact = with_input(before, base(mode, before));
    // SAFETY: per the contract.
    let Some(after) = (unsafe { found.current(mode) }) else {
        return;
    };
    let game = Place::stored(after.location, after.rotation);
    let mut fold = FOLD.lock().unwrap_or_else(PoisonError::into_inner);
    if exact.apart(&game) > 1.0 {
        *fold = None;
        drop(fold);
        log_mismatch("a fold", &exact, &game, before, None);
        return;
    }
    let location = math::narrow_settled(exact.location);
    let rotation = math::narrow_settled(exact.rotation.to_rotator());
    // SAFETY: per the contract; the game is done with them until the next input.
    unsafe { found.set_place(mode, location, rotation) };
    *fold = Some(Fold {
        mode: mode as usize,
        location,
        rotation,
        exact,
    });
    LOG_FOLD.call_once(|| {
        log!(
            "first fold computed in double precision: rotation {rotation:?} instead of the \
             game's {:?}, location {location:?} instead of {:?}",
            after.rotation,
            after.location
        )
    });
}

/// Where `current` starts from: its own place and rotation, as last folded (or given by
/// [`place_move`]) in `f64` while they're still the ones the game was given then; else as the game
/// keeps them (when the move began, the reference brick's stored place and rotation). A move
/// beginning where the last one left its reference brick starts from the same exact place, which
/// rounds to the same.
fn base(mode: *const UObjectBase, current: &Move) -> Place {
    let fold = *FOLD.lock().unwrap_or_else(PoisonError::into_inner);
    match fold {
        Some(fold)
            if fold.mode == mode as usize
                && fold.location == current.location
                && fold.rotation == current.rotation =>
        {
            fold.exact
        }
        _ => Place::stored(current.location, current.rotation),
    }
}

/// Where the move `current` of `mode` puts its reference brick now, input and brick snapping
/// included, in `f64`.
pub fn effective(mode: *const UObjectBase, current: &Move) -> Place {
    adjusted(
        &with_input(current, base(mode, current)),
        &current.adjustment,
    )
}

/// Puts the move of `mode` at `place` (where it puts its reference brick), with no input or brick
/// snapping on top (see `MoveMode::replace`): the game gets the nearest `f32` values, `place` is
/// kept for the bricks' places computed from them.
///
/// # Safety
/// `mode` must be a live move mode. Game thread only.
pub unsafe fn place_move(found: &MoveMode, mode: *mut UObjectBase, place: Place) {
    let location = math::narrow_settled(place.location);
    let rotation = math::narrow_settled(place.rotation.to_rotator());
    // SAFETY: per the contract.
    unsafe { found.replace(mode, location, rotation) };
    *FOLD.lock().unwrap_or_else(PoisonError::into_inner) = Some(Fold {
        mode: mode as usize,
        location,
        rotation,
        exact: place,
    });
}

/// Where the move puts the reference brick before brick snapping, from `base`:
/// `GetMoveTransformWithAccumulatedInput` in `f64` (see `game::move_mode`).
fn with_input(current: &Move, base: Place) -> Place {
    let gizmo = if current.world_gizmo {
        Quat::IDENTITY
    } else {
        base.rotation
    };
    let mut location = base.location;
    let drag = math::widen(current.drag);
    let grid = f64::from(current.grid);
    if grid > 0.0 {
        // Snapped to the grid along the gizmo's axes (the vehicle's for a free move): whole steps,
        // the nearest (from half a step below).
        let frame = if current.drag_along_gizmo {
            gizmo
        } else {
            Quat::IDENTITY
        };
        let snapped = frame
            .inverse()
            .rotate(drag)
            .map(|offset| ((offset + grid / 2.0) / grid).floor() * grid);
        location = math::add(location, frame.rotate(snapped));
    } else {
        location = math::add(location, drag);
    }

    let mut rotation = base.rotation;
    let angle = f64::from(current.angle);
    for axis in 0..3 {
        let mut turn = f64::from(current.turn[axis]);
        if angle > 0.0 {
            turn = ((turn + angle / 2.0) / angle).floor() * angle;
        }
        if turn != 0.0 {
            let mut unit = [0.0; 3];
            unit[axis] = 1.0;
            rotation = Quat::from_axis_angle(gizmo.rotate(unit), turn) * rotation;
        }
    }
    Place { location, rotation }
}

/// `moved` with the brick snapping adjustment (game 0xCCEB00) in `f64`.
fn adjusted(moved: &Place, adjustment: &Adjustment) -> Place {
    let turned = Quat::widen(adjustment.rotation);
    let pivot = math::widen(adjustment.pivot);
    let shifted = math::add(moved.location, math::widen(adjustment.offset));
    Place {
        location: math::add(pivot, turned.rotate(math::sub(shifted, pivot))),
        rotation: turned * moved.rotation,
    }
}

/// Where a brick at `brick` goes when the reference brick goes from `reference` to `moved`: it
/// keeps its place relative to it (`GetEffectiveObjectTransform`, for bricks that aren't mirror
/// mode counterparts).
pub fn carried(moved: &Place, reference: &Place, brick: &Place) -> Place {
    let back = reference.rotation.inverse();
    let relative = back.rotate(math::sub(brick.location, reference.location));
    Place {
        location: math::add(moved.location, moved.rotation.rotate(relative)),
        rotation: moved.rotation * (back * brick.rotation),
    }
}

/// Logs the first few results left as the game computed them: ours, the game's, and what they come
/// from (`places`: the brick's and the reference brick's).
fn log_mismatch(
    what: &str,
    ours: &Place,
    game: &Place,
    current: &Move,
    places: Option<(&Place, &Place)>,
) {
    if MISMATCHES.fetch_add(1, Ordering::Relaxed) >= LOGGED_MISMATCHES {
        return;
    }
    let shown =
        |place: &Place| format!("{:?} at {:?}", place.rotation.to_rotator(), place.location);
    let places = places.map_or(String::new(), |(brick, reference)| {
        format!(", brick {}, reference {}", shown(brick), shown(reference))
    });
    log!(
        "{what} left as the game computed it ({:.1} times the match limits apart): ours {}, the \
         game's {}; from {current:?}{places}",
        ours.apart(game),
        shown(ours),
        shown(game)
    );
}
