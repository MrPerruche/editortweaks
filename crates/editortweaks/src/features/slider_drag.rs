//! Dragging sliders further.
//!
//! - Alt-drag: while Alt ([`keybinds::KEEP_DRAGGING_SLIDERS`], rebindable) is held during a drag,
//!   the value keeps changing as long as the mouse is away from where Alt was pressed: faster the
//!   farther it is (four times as fast at twice the distance) and the larger the value is (10 to 100
//!   takes as long as 100 to 1000), see [`ALT_DRAG_SPEED`]. Shift and Control slow it down, as they
//!   do normal drags (the game's own keys for that, which stay). Releasing Alt carries on as a
//!   normal drag from there.
//! - With [`LIFT_SLIDER_LIMITS`] (a setting), no drag of a slider of the vehicle editor's values
//!   stops at the slider's ends (plain, Shift, Ctrl or Alt): values only stop at the property's own
//!   limits (`property_limits` and its rules, some of which stop drags before typed values:
//!   `drag_min`). In absolute mode (the value under the cursor), dragging past an end carries on
//!   proportionally. Arrow keys still stop at the ends. All other sliders stop there too
//!   (`property_limits::slider_limit`). So an absolute drag doesn't stop where the (hidden) cursor
//!   meets the screen's edge, the cursor is moved back from the edges once the drag is past the
//!   slider's end, and the drag sees it where it would be without that ([`CursorShift`]). Never in
//!   a relative drag: the game keeps that cursor over the slider itself, and moving it away made
//!   the value jump (the brick size jumping to several metres, reported after the first release).
//! - The cursor of a drag stays in the game's window ([`keep_cursor_in_window`]), as a single
//!   monitor's edges would keep it: with more monitors it would wander off to them, and show there
//!   when the drag ends.
//!
//! How: the slider's mouse move handler (`game::slider_drag`) is hooked. Its clamp to the slider's
//! range is removed from its code or put back, as the setting and the dragged slider say when a
//! drag moves; `property_limits` then keeps the out-of-range value when it's set. During an Alt-drag the game sees a Shift-drag that didn't move, which keeps the value,
//! while a timer (`platform::timer`, about once per frame) moves it. When a property stops a
//! dragged value at its limit, the drag carries on from there ([`stored`]): moving back takes
//! effect right away instead of first undoing the movement past the limit. The row's other sliders
//! show the values kept too (with the axes locked, the game gave them the dragged value).

use core::ffi::c_void;
use std::cell::{Cell, RefCell};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use super::keybinds;
use super::property_limits;
use crate::framework::hook;
use crate::framework::settings::Toggle;
use crate::framework::timings;
use crate::game::numeric_property::NumericValue;
use crate::game::property_widget::{self, PropertyWidgets};
use crate::game::slate::{self, Geometry, PointerEvent};
use crate::game::slider::{self, RangeFields, SetValueInternalFn, ValueFields};
use crate::game::slider_drag::{self, DragModes, MouseDrag, MouseMoveFn};
use crate::game::weak_object::WeakObject;
use crate::platform::cursor;
use crate::platform::keyboard;
use crate::platform::timer::ThreadTimer;
use crate::prelude::*;

/// Lift the slider's own limits from every kind of drag in the vehicle editor, so only the
/// property's limits apply.
pub static LIFT_SLIDER_LIMITS: Toggle = Toggle::new(
    "LiftSliderLimits",
    "Drag past slider ends",
    "In the vehicle editor, dragging a brick's slider carries on past its ends, up to the \
     property's own limits. Arrow keys still stop at the ends.",
    true,
);

/// Alt-drag speed with the mouse one slider width away from where Alt was pressed (four times that
/// at two widths): the value changes by this much of its own size per second, e.g. 1.0 moves 100
/// by 100 per second. So it grows exponentially, ×2.7 per second at 1.0.
const ALT_DRAG_SPEED: f64 = 1.0;
/// Near zero, where that gets slow, the value still changes by at least this much of the slider's
/// range per second (with the mouse one slider width away). Above zero.
const ALT_DRAG_MIN_SPEED: f64 = 0.01;
/// Alt-drag speed factor while Shift is held too: as much slower as Shift makes normal drags
/// (0.002 instead of 0.05 per mouse count).
const ALT_DRAG_SHIFT_FACTOR: f32 = 1.0 / 25.0;
/// Alt-drag speed factor while Control is held too (with Shift too, both apply).
const ALT_DRAG_CONTROL_FACTOR: f32 = 1.0 / 4.0;
/// The Alt-drag timer's interval, in milliseconds. Windows runs it at most once per pass of the
/// game's message loop, i.e. once per frame.
const ALT_DRAG_INTERVAL_MS: u32 = 10;
/// The longest time an Alt-drag step covers, in seconds, so a hitch doesn't make the value jump.
const MAX_STEP_SECONDS: f32 = 0.1;
/// During a drag past a slider's ends, the cursor is moved back once it's this close to the left or
/// right edge of where it can go, as a share of that width ...
const EDGE_SHARE: f32 = 1.0 / 8.0;
/// ... to this far from that edge: still on the same side of the screen (the panel's), where it
/// shows again after the drag.
const EDGE_RETURN_SHARE: f32 = 1.0 / 4.0;

pub struct SliderDrag;

static MOUSE_DRAG: OnceLock<MouseDrag> = OnceLock::new();
static MOUSE_MOVE: OnceLock<MouseMoveFn> = OnceLock::new();
/// Which drags are absolute; unknown, the cursor is never moved back from the screen's edges.
static DRAG_MODES: OnceLock<DragModes> = OnceLock::new();
/// `SetValueInternal` as the game calls it, i.e. through `property_limits`' hook.
static SET_VALUE_INTERNAL: OnceLock<SetValueInternalFn> = OnceLock::new();
/// Set once reflection is available; until then (or if it fails) drags stay as they are.
static SLIDERS: OnceLock<Sliders> = OnceLock::new();

struct Sliders {
    drag: &'static MouseDrag,
    set_value_internal: SetValueInternalFn,
    fields: ValueFields,
    range: RangeFields,
    /// Finds the property row of a dragged slider, whose other sliders may need updating.
    rows: Option<PropertyWidgets>,
    /// Whether drags may carry on past the slider's ends: sliders keep values outside of their
    /// range (`property_limits`). Otherwise `SetValueInternal` would clamp them anyway, and drags
    /// would carry on past the end unseen.
    can_lift: bool,
    /// Whether they do now: the clamp is lifted from the game's code (see [`sync_limits`]).
    lifted: AtomicBool,
    /// Whether lifting or restoring the clamp failed: it's left as it is.
    stuck: AtomicBool,
}

thread_local! {
    /// The Alt-drag in progress, if any.
    static ALT_DRAG: RefCell<Option<AltDrag>> = const { RefCell::new(None) };
    /// The property row of the slider that last reached a limit, found once per slider.
    static ROW: RefCell<Option<Row>> = const { RefCell::new(None) };
    /// Whether a drag step runs, whose stores [`stored`] records.
    static IN_DRAG_STEP: Cell<bool> = const { Cell::new(false) };
    /// The last value a property stored during the drag step that runs.
    static LAST_STORE: Cell<Option<Store>> = const { Cell::new(None) };
    /// How far the drag's cursor was moved back from the screen's edges, if it was.
    static CURSOR_SHIFT: Cell<Option<CursorShift>> = const { Cell::new(None) };
}

/// How far the cursor of a drag was moved back from the screen's edges ([`keep_cursor_in_window`]):
/// the drag sees it that much further ([`with_shifted_cursor`]), where it would be without that.
#[derive(Clone, Copy)]
struct CursorShift {
    /// In pixels along X: where the drag sees the cursor minus where it is.
    offset: f32,
    /// The modifier keys held then. Shift and Control choose between the absolute and the relative
    /// drag, so with others, the drag may have changed modes and the cursor's place starts anew.
    modifier_keys: u16,
}

struct AltDrag {
    slider: WeakObject,
    /// The mouse movement along the slider since Alt was pressed, in pixels (raw mouse counts).
    offset: f32,
    /// The slider's width on screen, in pixels.
    width: f32,
    last_step: Instant,
    /// Runs [`alt_drag_tick`]; stops when the Alt-drag is dropped.
    _timer: ThreadTimer,
}

struct Row {
    slider: WeakObject,
    /// Its `UNumericPropertyWidget`; none outside of property panels (e.g. settings).
    widget: Option<WeakObject>,
}

/// A value a property stored: the one asked for, and the one it kept.
#[derive(Clone, Copy)]
struct Store {
    requested: NumericValue,
    stored: NumericValue,
}

impl Feature for SliderDrag {
    fn id(&self) -> &'static str {
        "slider_drag"
    }

    fn on_load(&self) -> LoadResult {
        let drag = slider_drag::mouse_drag()?;
        let set_value_internal = slider::SET_VALUE_INTERNAL.resolve()?;
        let target = drag.mouse_move;
        let _ = MOUSE_DRAG.set(drag);
        // SAFETY: the start of SetValueInternal, whose signature `SetValueInternalFn` is.
        let set_value_internal =
            unsafe { core::mem::transmute::<*const u8, SetValueInternalFn>(set_value_internal) };
        let _ = SET_VALUE_INTERNAL.set(set_value_internal);
        match slider_drag::drag_modes() {
            Ok(modes) => {
                let _ = DRAG_MODES.set(modes);
            }
            Err(reason) => log!("absolute drags stop at the screen's edges: {reason}"),
        }
        // SAFETY: `target` is the start of NativeOnMouseMove, and `on_mouse_move` has its
        // signature.
        unsafe {
            hook::install_typed(
                slider_drag::DRAGGING_CHECK.name,
                target,
                on_mouse_move as MouseMoveFn,
                &MOUSE_MOVE,
            )
        }
    }

    fn on_engine_init(&self) {
        match engine_init() {
            Ok(true) => log!("active (drags may go past the slider's ends: a setting)"),
            Ok(false) => log!("active (drags stop at the slider's ends)"),
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

fn engine_init() -> Result<bool, String> {
    let (Some(drag), Some(&set_value_internal)) = (MOUSE_DRAG.get(), SET_VALUE_INTERNAL.get())
    else {
        return Err("not loaded".into());
    };
    let sliders = Sliders {
        drag,
        set_value_internal,
        fields: slider::value_fields()?,
        range: slider::range_fields()?,
        rows: property_widget::property_widgets()
            .inspect_err(|reason| {
                log!("locked axes show values past a limit until released: {reason}")
            })
            .ok(),
        can_lift: can_lift(),
        lifted: AtomicBool::new(false),
        stuck: AtomicBool::new(false),
    };
    let can_lift = sliders.can_lift;
    let _ = SLIDERS.set(sliders);
    Ok(can_lift)
}

/// Whether the drag's clamp to the slider's range may be lifted: sliders keep values outside of
/// their range (`property_limits`).
fn can_lift() -> bool {
    let keeps = property_limits::keeps_slider_values();
    if !keeps {
        log!("drags stop at the slider's ends: sliders clamp their values");
    }
    keeps
}

/// Lifts the drag's clamp to the slider's range, or puts it back, as [`LIFT_SLIDER_LIMITS`] and
/// the dragged `slider` say now. Whether drags carry on past the slider's ends.
///
/// # Safety
/// Game thread only, outside of `NativeOnMouseMove` (whose code holds the clamp).
unsafe fn sync_limits(sliders: &Sliders, slider: *const c_void) -> bool {
    let lifted = sliders.lifted.load(Ordering::Relaxed);
    let wanted = sliders.can_lift
        && LIFT_SLIDER_LIMITS.get()
        && property_limits::slider_limit(slider).is_some();
    if wanted == lifted || sliders.stuck.load(Ordering::Relaxed) {
        return lifted;
    }
    // SAFETY: per the contract, nothing runs the clamp meanwhile.
    let done = unsafe {
        if wanted {
            sliders.drag.lift_clamp()
        } else {
            sliders.drag.restore_clamp()
        }
    };
    match done {
        Ok(()) => {
            sliders.lifted.store(wanted, Ordering::Relaxed);
            wanted
        }
        Err(reason) => {
            log!("drag limits left as they are: {reason}");
            sliders.stuck.store(true, Ordering::Relaxed);
            lifted
        }
    }
}

unsafe extern "C" fn on_mouse_move(
    slider: *mut c_void,
    reply: *mut c_void,
    geometry: *const Geometry,
    event: *const PointerEvent,
) -> *mut c_void {
    let _span = timings::span("slider_drag::on_mouse_move");
    let original = *MOUSE_MOVE.get().expect("hook recorded before use");
    // SAFETY: forwards the caller's arguments (the event possibly changed, see below).
    let call = || unsafe {
        timings::game(slider_drag::DRAGGING_CHECK.name, || {
            original(slider, reply, geometry, event)
        })
    };

    let Some(sliders) = SLIDERS.get() else {
        return call();
    };
    // SAFETY: NativeOnMouseMove is a UBrickSliderWidget member, so `slider` is one.
    if !unsafe { sliders.drag.is_dragging(slider) } {
        // A drag starts with moves that aren't dragging yet: its cursor wasn't moved back.
        CURSOR_SHIFT.set(None);
        return call();
    }
    // SAFETY: on the game thread, before NativeOnMouseMove runs.
    let lifted = unsafe { sync_limits(sliders, slider) };
    // SAFETY: the caller's event and geometry, valid during the call.
    let (moved, left, width, modifier_keys) = unsafe {
        (
            (*event).cursor_delta[0],
            (*geometry).absolute_left(),
            (*geometry).absolute_width(),
            (*event).modifier_keys,
        )
    };
    if !keybinds::KEEP_DRAGGING_SLIDERS.is_held() {
        ALT_DRAG.take();
        // SAFETY: the caller's event: only this thread uses it during the call, and it's restored
        // before returning.
        let reply =
            unsafe { with_shifted_cursor(event.cast_mut(), || drag_step(sliders, slider, call)) };
        // SAFETY: a live slider being dragged.
        let value_type = unsafe { sliders.fields.value_type(slider) };
        let absolute = DRAG_MODES
            .get()
            .is_some_and(|modes| modes.is_absolute(value_type, modifier_keys));
        let past_ends = (lifted && absolute).then_some((left, left + width));
        keep_cursor_in_window(lifted, past_ends, modifier_keys);
        // Without lifted limits, an absolute drag stops at the slider's end, on screen.
        if lifted {
            // SAFETY: a live slider being dragged, on the game thread.
            unsafe { keep_drag_within_limit(sliders, slider) };
        }
        return reply;
    }

    // An Alt-drag is relative: the game keeps the cursor over the slider.
    CURSOR_SHIFT.set(None);
    alt_drag_moved(slider, moved, width);
    // SAFETY: the caller's event: only this thread uses it during the call, and it's restored
    // before returning.
    unsafe { as_still_shift_drag(event.cast_mut(), call) }
}

/// How far a drag of `slider` may go now.
///
/// # Safety
/// `slider` must be a live slider. Game thread only.
unsafe fn drag_bounds(sliders: &Sliders, slider: *const c_void) -> (f32, f32) {
    match property_limits::slider_limit(slider) {
        Some(limit) if sliders.lifted.load(Ordering::Relaxed) => limit,
        // SAFETY: per the contract.
        _ => unsafe { sliders.range.get(slider) },
    }
}

/// Keeps a drag within how far its slider's values go, with the clamp lifted from the game's code:
/// without this the drag would run past the end unseen and moving back would take effect late.
///
/// # Safety
/// `slider` must be a live slider. Game thread only.
unsafe fn keep_drag_within_limit(sliders: &Sliders, slider: *mut c_void) {
    // SAFETY: per the contract.
    unsafe {
        let (min, max) = drag_bounds(sliders, slider);
        let dragged = sliders.drag.drag_value(slider);
        if dragged < min || dragged > max {
            sliders.drag.set_drag_value(slider, dragged.clamp(min, max));
        }
    }
}

/// Runs `call` with `event` turned into a Shift-drag that didn't move: the game then keeps the value
/// (the Alt-drag timer changes it) and keeps the hidden cursor over the slider.
///
/// # Safety
/// `event` must be a valid event that nothing else uses during `call`, other than through it.
unsafe fn as_still_shift_drag<R>(event: *mut PointerEvent, call: impl FnOnce() -> R) -> R {
    // SAFETY: per the contract.
    unsafe {
        let saved = ((*event).modifier_keys, (*event).cursor_delta);
        (*event).modifier_keys = slate::SHIFT;
        (*event).cursor_delta = [0.0; 2];
        let result = call();
        ((*event).modifier_keys, (*event).cursor_delta) = saved;
        result
    }
}

/// Runs `call` with `event`'s cursor where it would be if [`keep_cursor_in_window`] hadn't moved it:
/// an absolute drag carries on from there. A relative drag doesn't read it.
///
/// # Safety
/// `event` must be a valid event that nothing else uses during `call`, other than through it.
unsafe fn with_shifted_cursor<R>(event: *mut PointerEvent, call: impl FnOnce() -> R) -> R {
    // SAFETY: per the contract.
    unsafe {
        let shift = CURSOR_SHIFT
            .get()
            .filter(|shift| shift.modifier_keys == (*event).modifier_keys);
        CURSOR_SHIFT.set(shift);
        let Some(shift) = shift else {
            return call();
        };
        let saved = (*event).screen_space_position;
        (*event).screen_space_position[0] += shift.offset;
        let result = call();
        (*event).screen_space_position = saved;
        result
    }
}

/// Keeps the (hidden) cursor of a drag in the game's window, or wherever it's confined to. In an
/// absolute drag that may go past the slider's ends (`slider`: its left and right edge on screen),
/// it's also moved back once the drag is past them and the cursor nears the left or right edge,
/// where it would stop: the drag then goes on as far as the mouse does, and how far the cursor was
/// moved is remembered for [`with_shifted_cursor`]. Otherwise it just stops at the edges, as it
/// does on a single monitor (past the slider's end anyway), or with the slider's limits `lifted`
/// isn't moved at all: a relative drag keeps it over the slider itself.
fn keep_cursor_in_window(lifted: bool, slider: Option<(f32, f32)>, modifier_keys: u16) {
    let (Some((x, y)), Some(area)) = (cursor::position(), cursor::window_area()) else {
        return;
    };
    let target_y = y.clamp(area.top, area.bottom - 1);
    let shift = CURSOR_SHIFT
        .get()
        .filter(|shift| shift.modifier_keys == modifier_keys);
    // Where the drag sees the cursor: over the slider, it's far enough from the edges.
    let seen = x as f32 + shift.map_or(0.0, |shift| shift.offset);
    let moved_back = slider
        .filter(|&(left, right)| seen < left || seen >= right)
        .and_then(|_| off_edge(x, area.left, area.right));
    let target_x = moved_back.unwrap_or_else(|| {
        if lifted {
            x
        } else {
            x.clamp(area.left, area.right - 1)
        }
    });
    if (target_x, target_y) == (x, y) || !cursor::set_position(target_x, target_y) {
        return;
    }
    if moved_back.is_some() {
        CURSOR_SHIFT.set(Some(CursorShift {
            offset: shift.map_or(0.0, |shift| shift.offset) + (x - target_x) as f32,
            modifier_keys,
        }));
    }
}

/// Where to move a cursor at `x` that can go from `left` to `right` (exclusive), if it's near
/// either edge (see [`EDGE_SHARE`]).
fn off_edge(x: i32, left: i32, right: i32) -> Option<i32> {
    let width = (right - left) as f32;
    let (near, back) = (
        (width * EDGE_SHARE) as i32,
        (width * EDGE_RETURN_SHARE) as i32,
    );
    if near < 1 || back <= near {
        return None;
    }
    if x < left + near {
        Some(left + back)
    } else if x >= right - near {
        Some(right - back)
    } else {
        None
    }
}

/// Runs `step`, a drag step of `slider` that may store its value into properties. If a property
/// stopped that value at its limit, the drag carries on from the value kept.
fn drag_step<R>(sliders: &Sliders, slider: *mut c_void, step: impl FnOnce() -> R) -> R {
    LAST_STORE.set(None);
    IN_DRAG_STEP.set(true);
    let result = step();
    IN_DRAG_STEP.set(false);
    if let Some(store) = LAST_STORE.take() {
        // SAFETY: the slider this step dragged, alive during the step.
        unsafe { follow_limit(sliders, slider, store) };
    }
    result
}

/// # Safety
/// `slider` must be a live slider being dragged. Game thread only.
unsafe fn follow_limit(sliders: &Sliders, slider: *mut c_void, store: Store) {
    let used = usize::from(store.requested.num_used).min(3);
    let (requested, stored) = (&store.requested.data[..used], &store.stored.data[..used]);
    if requested == stored {
        return;
    }
    // The value the slider sent: `OnSliderValueChanged` put it into its component of the
    // property's value, and with the axes locked into every component and the row's other sliders.
    // SAFETY: per the contract.
    let (sent, row) = unsafe { (sliders.fields.value(slider), row_sliders(sliders, slider)) };
    // That component: known from the row, else the first one that holds the value sent.
    let component = match row {
        Some(row) => row.iter().position(|&other| other == slider.cast_const()),
        None => (0..used).find(|&i| requested[i] == sent && stored[i] != sent),
    };
    // Not this slider's value (another property stored during the step).
    let Some(component) = component.filter(|&i| requested.get(i) == Some(&sent)) else {
        return;
    };

    let kept = stored[component];
    if kept != sent {
        // SAFETY: per the contract.
        unsafe { sliders.drag.set_drag_value(slider, kept) };
    }
    // Show the values kept.
    // SAFETY: the dragged slider and its row's sliders are live.
    unsafe {
        match row {
            Some(row) => {
                for (&other, &value) in row.iter().zip(stored) {
                    show(sliders, other.cast_mut(), value);
                }
            }
            None => show(sliders, slider, kept),
        }
    }
}

/// Shows `value` in `slider` without sending it on (it's stored already).
///
/// # Safety
/// `slider` must be a live slider. Game thread only.
unsafe fn show(sliders: &Sliders, slider: *mut c_void, value: f32) {
    // SAFETY: per the contract; without clamping, `property_limits` leaves the value alone.
    unsafe {
        if sliders.fields.value(slider) != value {
            (sliders.set_value_internal)(slider, value, 0, 0, usize::from(slider::INTERACTIVE));
        }
    }
}

/// The sliders of `slider`'s property row, by component, if it's in one.
///
/// # Safety
/// `slider` must be a live slider. Game thread only. The slice is only valid during this frame.
unsafe fn row_sliders<'a>(sliders: &Sliders, slider: *mut c_void) -> Option<&'a [*const c_void]> {
    let rows = sliders.rows.as_ref()?;
    let widget = ROW.with_borrow_mut(|row| {
        let known = row
            .as_ref()
            .is_some_and(|row| row.slider.is(slider.cast()) && row.slider.get().is_some());
        if !known {
            // SAFETY: `slider` per the contract, and `row_of` only returns live objects.
            *row = Some(unsafe {
                Row {
                    slider: WeakObject::new(slider.cast()),
                    widget: rows.row_of(slider).map(|widget| WeakObject::new(widget)),
                }
            });
        }
        row.as_ref()?.widget.as_ref()?.get()
    })?;
    // SAFETY: a live NumericPropertyWidget, its sliders read now.
    let row = unsafe { rows.sliders(widget) };
    row.contains(&slider.cast_const()).then_some(row)
}

/// Adds the mouse movement of an Alt-drag of `slider`, starting the Alt-drag if needed.
fn alt_drag_moved(slider: *mut c_void, moved: f32, width: f32) {
    ALT_DRAG.with_borrow_mut(|alt_drag| {
        if alt_drag
            .as_ref()
            .is_none_or(|drag| !drag.slider.is(slider.cast()))
        {
            *alt_drag = start_alt_drag(slider);
        }
        if let Some(drag) = alt_drag {
            drag.offset += moved;
            drag.width = width;
        }
    });
}

fn start_alt_drag(slider: *mut c_void) -> Option<AltDrag> {
    let Some(timer) = ThreadTimer::start(ALT_DRAG_INTERVAL_MS, alt_drag_tick) else {
        log!("can't start a timer for Alt-dragging");
        return None;
    };
    Some(AltDrag {
        // SAFETY: the slider whose mouse move is being handled, so alive.
        slider: unsafe { WeakObject::new(slider.cast()) },
        offset: 0.0,
        width: 0.0,
        last_step: Instant::now(),
        _timer: timer,
    })
}

/// The Alt-drag timer: moves the value by the time since the last step.
fn alt_drag_tick() {
    let step = ALT_DRAG.with_borrow_mut(|alt_drag| {
        let drag = alt_drag.as_mut()?;
        let slider = drag.slider.get()?;
        let now = Instant::now();
        let seconds = (now - drag.last_step).as_secs_f32().min(MAX_STEP_SECONDS);
        drag.last_step = now;
        Some((slider.cast::<c_void>(), drag.offset / drag.width, seconds))
    });
    let (Some(sliders), Some((slider, distance, seconds))) = (SLIDERS.get(), step) else {
        ALT_DRAG.take();
        return;
    };
    // SAFETY: still in the object array, so alive, and a slider (it was being dragged).
    let dragging = unsafe { sliders.drag.is_dragging(slider) };
    if !dragging || !keybinds::KEEP_DRAGGING_SLIDERS.is_held() {
        // Released: a further mouse move drags normally from here.
        ALT_DRAG.take();
        return;
    }
    // Shift and Control slow it down: as if less time had passed.
    let seconds = seconds * modifier_factor();
    // SAFETY: as above; timers run on the thread that started them, the game thread.
    unsafe { alt_drag_step(sliders, slider, distance, seconds) };
}

/// The Alt-drag speed factor of the modifier keys held right now (Alt aside).
fn modifier_factor() -> f32 {
    let mut factor = 1.0;
    if keyboard::is_shift_down() {
        factor *= ALT_DRAG_SHIFT_FACTOR;
    }
    if keyboard::is_control_down() {
        factor *= ALT_DRAG_CONTROL_FACTOR;
    }
    factor
}

/// Moves `slider`'s value as an Alt-drag held `distance` slider widths away does in `seconds`.
///
/// # Safety
/// `slider` must be a live slider being dragged. Game thread only.
unsafe fn alt_drag_step(sliders: &Sliders, slider: *mut c_void, distance: f32, seconds: f32) {
    // SAFETY: per the contract.
    let ((min, max), current) =
        unsafe { (sliders.range.get(slider), sliders.drag.drag_value(slider)) };
    let span = if max > min && (max - min).is_finite() {
        max - min
    } else {
        1.0
    };
    // SAFETY: per the contract.
    let (low, high) = unsafe { drag_bounds(sliders, slider) };
    let target = alt_drag_target(current, span, distance * distance.abs() * seconds)
        .max(low)
        .min(high);
    if !target.is_finite() || target == current {
        return;
    }
    // Like the game's own drag step (`game::slider_drag`), except for the once-per-frame check.
    // SAFETY: per the contract.
    unsafe { sliders.drag.set_drag_value(slider, target) };
    drag_step(sliders, slider, || {
        // SAFETY: per the contract.
        unsafe {
            (sliders.set_value_internal)(slider, target, 1, 1, usize::from(slider::INTERACTIVE))
        }
    });
}

/// Where an Alt-drag takes `current` on a slider spanning `span`: `amount` is the time in seconds
/// times the mouse's distance in slider widths, squared (negative to the left).
fn alt_drag_target(current: f32, span: f32, amount: f32) -> f32 {
    if amount == 0.0 {
        return current;
    }
    // The speed is `ALT_DRAG_SPEED * hypot(value, scale)`: proportional to the value far from zero,
    // `span * ALT_DRAG_MIN_SPEED` at zero. With `value = scale * sinh(u)`, that's `u` changing at
    // a constant rate, so a step of any length lands exactly where it should (no overshooting
    // past zero when frames are slow).
    let scale = f64::from(span) * ALT_DRAG_MIN_SPEED / ALT_DRAG_SPEED;
    let u = (f64::from(current) / scale).asinh() + ALT_DRAG_SPEED * f64::from(amount);
    (scale * u.sinh()) as f32
}

/// Called by `property_limits` for every numeric value a property stores: `requested` is the value
/// asked for, `stored` the one kept.
pub(crate) fn stored(requested: &NumericValue, stored: &NumericValue) {
    if IN_DRAG_STEP.get() {
        LAST_STORE.set(Some(Store {
            requested: *requested,
            stored: *stored,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_near_an_edge_goes_back_on_that_side() {
        // A 1920 pixel wide screen: within 240 of an edge, back to 480 from it.
        assert_eq!(off_edge(960, 0, 1920), None);
        assert_eq!(off_edge(240, 0, 1920), None);
        assert_eq!(off_edge(239, 0, 1920), Some(480));
        assert_eq!(off_edge(1919, 0, 1920), Some(1440));
        assert_eq!(off_edge(1680, 0, 1920), Some(1440));
        // A second monitor left of the main one.
        assert_eq!(off_edge(-1920, -1920, 1920), Some(-1440));
        // No room to move.
        assert_eq!(off_edge(0, 0, 4), None);
    }
}
