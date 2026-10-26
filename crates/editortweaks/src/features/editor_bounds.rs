//! Editor distance limit: bricks and the editor's camera may go [`BOUNDS`] from the vehicle's
//! origin along each axis, instead of the game's limit (`EditorBounds`, logged at start-up).
//!
//! The editor clamps moved bricks and its camera into the box `±EditorBounds`, a field of its
//! static settings' default object, which it reads on every use (see `docs/re/editor-bounds.md`).
//! That box also sizes the click trace (its diagonal) and the translucent wall shown near its
//! edge, so they follow. Only the editor reads it: vehicles load and spawn as without the mod.
//!
//! The box also sizes the floor's snapping grid (15 cm cells), which bricks snap onto while moved
//! with snapping on: past about ±3.5 km its cell count overflows the game's 32-bit area
//! comparison, and snapping a brick to the floor walks trillions of cells, freezing the game. So
//! while the game computes brick snapping (`GetMoveSnappingOffset`, hooked through
//! `game::move_mode`), the box is the game's own again (at most [`SNAPPING_BOUNDS`]): bricks snap
//! to the floor as without the mod near the origin, and not farther out. If that hook can't be
//! set up, the bounds go up to [`SNAPPING_BOUNDS`] only.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;

use crate::game::editor_static_info::{self, BoundsField};
use crate::game::move_mode;
use crate::game::weak_object::WeakObject;
use crate::prelude::*;

/// How far from the vehicle's origin bricks and the camera may go along each axis, in cm
/// (10 000 000 cm = 100 km). Larger limits of the game are kept.
const BOUNDS: f32 = 10_000_000.0;

/// The largest bounds the game's brick snapping may see, in cm (3 km): below about 347 000 cm the
/// floor's snapping grid has at most 46 340 cells per side, whose area still fits the game's
/// signed 32-bit comparison. Also the limit when snapping can't be given the game's bounds.
const SNAPPING_BOUNDS: f32 = 300_000.0;

pub struct EditorBounds;

static FIELD: OnceLock<BoundsField> = OnceLock::new();
/// Set once brick snapping runs with the game's bounds ([`snap_with_game_bounds`]).
static SNAPPING_SAFE: AtomicBool = AtomicBool::new(false);
/// The static settings objects raised so far, with their bounds before.
static RAISED: Mutex<Vec<Raised>> = Mutex::new(Vec::new());

struct Raised {
    info: WeakObject,
    game: [f32; 3],
}

// SAFETY: the object pointer is only used on the game thread (the mutex just satisfies statics).
unsafe impl Send for Raised {}

impl Feature for EditorBounds {
    fn id(&self) -> &'static str {
        "editor_bounds"
    }

    fn on_engine_init(&self) {
        match editor_static_info::find() {
            Ok(field) => {
                let _ = FIELD.set(field);
                guard_snapping();
                raise();
            }
            Err(reason) => log!("inactive: {reason}"),
        }
    }

    // The editor's static settings are a Blueprint class, loaded with the editor's map at the
    // latest.
    fn on_map_load(&self) {
        if FIELD.get().is_some() {
            guard_snapping();
            raise();
        }
    }
}

/// How far from the vehicle's origin the editor lets things go along each axis right now, in cm
/// (half the box): what bricks' position sliders span. `None` if the bounds weren't found.
/// Game thread only.
pub fn bounds() -> Option<[f32; 3]> {
    let field = FIELD.get()?;
    let raised = RAISED.lock().unwrap_or_else(PoisonError::into_inner);
    raised.iter().find_map(|record| {
        let info = record.info.get()?;
        // SAFETY: a live static settings object (just checked), on the game thread.
        Some(unsafe { field.get(info) })
    })
}

/// Has the game's brick snapping run with the game's bounds, unless it already does.
fn guard_snapping() {
    if SNAPPING_SAFE.load(Ordering::Relaxed) {
        return;
    }
    match move_mode::shared().and_then(|moves| moves.snap_around(snap_with_game_bounds)) {
        Ok(()) => {
            SNAPPING_SAFE.store(true, Ordering::Relaxed);
            log!("brick snapping keeps the game's bounds (at most ±{SNAPPING_BOUNDS} cm)");
        }
        Err(reason) => log!("bounds kept to ±{SNAPPING_BOUNDS} cm: {reason}"),
    }
}

/// Raises the bounds of every loaded static settings object to at least [`BOUNDS`] (or
/// [`SNAPPING_BOUNDS`] while snapping doesn't get the game's bounds), recording the game's.
fn raise() {
    let Some(field) = FIELD.get() else { return };
    let limit = if SNAPPING_SAFE.load(Ordering::Relaxed) {
        BOUNDS
    } else {
        SNAPPING_BOUNDS
    };
    let mut raised = RAISED.lock().unwrap_or_else(PoisonError::into_inner);
    raised.retain(|record| record.info.get().is_some());
    for info in field.defaults() {
        // SAFETY: `defaults` only returns live static settings objects; on the game thread.
        unsafe {
            let now = field.get(info);
            let game = match raised.iter().find(|record| record.info.is(info)) {
                Some(record) => record.game,
                None => {
                    raised.push(Raised {
                        info: WeakObject::new(info),
                        game: now,
                    });
                    now
                }
            };
            let wanted = game.map(|axis| axis.max(limit));
            if wanted != now {
                field.set(info, wanted);
                log!("editor bounds: ±{game:?} cm -> ±{wanted:?} cm");
            }
        }
    }
}

/// Runs the game's brick snapping computation `snap` with every static settings object back at
/// the game's bounds (at most [`SNAPPING_BOUNDS`]), then puts the raised ones back.
fn snap_with_game_bounds(snap: &mut dyn FnMut()) {
    let Some(field) = FIELD.get() else {
        return snap();
    };
    // Copied out: the lock isn't held while game code runs.
    let infos: Vec<(*mut UObjectBase, [f32; 3])> = RAISED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter_map(|record| Some((record.info.get()?, record.game)))
        .collect();
    // SAFETY: each object was just checked live, and the game thread (where the game computes
    // snapping) is the only one using them.
    unsafe {
        let before: Vec<[f32; 3]> = infos.iter().map(|&(info, _)| field.get(info)).collect();
        for &(info, game) in &infos {
            field.set(info, game.map(|axis| axis.min(SNAPPING_BOUNDS)));
        }
        snap();
        for (&(info, _), bounds) in infos.iter().zip(before) {
            field.set(info, bounds);
        }
    }
}
