//! The map in the vehicle editor, to build "city covers" (vehicles that cover the map and are
//! spawned at one garage, so they line up with it). Everything is off until asked for, in the
//! editor's side panel with nothing selected ("Map", see [`panel`]):
//!
//! - A garage chosen in the list has its spawn point marked in the editor by a cross (forward red,
//!   right green, up blue) in the vehicle's space, with the garage's spawn box and the vehicle's
//!   bounding box: the game puts the bounding box's centre on the spawn point (see [`placement`]).
//! - "Show map" (or [`keybinds::SHOW_MAP`]) shows the map around the vehicle, as it will be once
//!   the vehicle is spawned at the selected garage (if none is selected, the one the editor was
//!   opened from, else the first). The editor actor is teleported to where the vehicle will land,
//!   and the game's hiding of the map is skipped (`game::hidden_components`); characters and
//!   vehicles stay hidden from this player's view only (`game::hidden_actors`). The editor goes
//!   back where it was when the map is hidden, and when the editor closes.
//!
//! The vehicle is never touched: only the local editor actor moves, so a vanilla player loading the
//! same vehicle gets the same result. While the map shows, the editor and the world don't see each
//! other ([`isolation`]): the editor's bricks push nothing and block nothing, and nothing (physics,
//! characters, sensors) finds them, so the map can show alone, hosting or joined. The editor's
//! clicks go through the map to its bricks. While the mouse is held the editor doesn't move; it
//! follows the vehicle's bounding box a second later (the box changes as bricks are added or
//! moved), which costs a reflected call per brick.
//!
//! Where it is documented: `docs/re/map-in-editor.md` (findings), `docs/city-cover-helper.md`
//! (design and the risks to check in game).

mod isolation;
mod panel;
mod placement;

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use brickrust::ue::coreuobject::{EObjectFlags, GObjects, UObjectBase};
use brickrust::ue::uclass::UStruct;

use self::isolation::{Isolated, Isolation};
use self::placement::{GaragePlace, Landing, Pose, VehicleBox};
use super::editor_frame;
use super::keybinds;
use crate::framework::hook;
use crate::framework::settings::{Section, Setting, Toggle};
use crate::game::actor::{self, Actors};
use crate::game::brick_editor::{self, BrickEditor};
use crate::game::editor_camera::{self, EditorCamera};
use crate::game::editor_input;
use crate::game::editor_mode::{self, EditorModes};
use crate::game::editor_object::{LocalBounds, Places, Vector};
use crate::game::editor_panel::{self, EditorPanel};
use crate::game::editor_view::{self, Bricks};
use crate::game::garage::{self, Garage, Garages};
use crate::game::hidden_actors::{self, HiddenActors};
use crate::game::hidden_components;
use crate::game::line_batcher::{self, Line, LineBatcher};
use crate::game::net_mode::{self, NetMode, Role};
use crate::game::reflection::{self, BoolField, Getter, NativeThunk, SingleCall};
use crate::game::vehicle_box;
use crate::game::weak_object::WeakObject;
use crate::game::world;
use crate::math::{self, Quat, Vec3};
use crate::prelude::*;

/// This feature's lines, among others' (see `LineBatcher::replace`).
const LINES: &str = "map_in_editor";

/// How often new characters and vehicles are looked for to hide while the map shows.
const HIDE_REFRESH: Duration = Duration::from_millis(2000);

/// How far the cross's axes reach, in cm.
const CROSS: f64 = 200.0;

/// The settings of this feature.
pub static SETTINGS: [Section; 1] = [Section {
    title: "Map",
    settings: &[Setting::Toggle(&PINNED)],
}];

/// Whether the garage's pinned spawn point is the one to use: the game uses it when the vehicle
/// browser's spawn option is Pinned, which city covers need (unpinned, they fall). Garages without
/// one use the normal one either way. On by default, as `docs/city-cover-helper.md` planned.
static PINNED: Toggle = Toggle::new(
    "MapPinnedSpawn",
    "Pinned spawn point",
    "Place the vehicle where it lands when spawned with the vehicle browser's spawn option set to Pinned, as city covers are. Only garages with a pinned spawn point of their own differ. Turn off for vehicles you spawn normally.",
    true,
);

pub struct MapInEditor;

/// Set once everything is found; until then (or if that fails) nothing happens.
static GAME: OnceLock<Game> = OnceLock::new();
static STATE: Mutex<State> = Mutex::new(State::new());

static LOG_STARTED: std::sync::Once = std::sync::Once::new();

/// Whether the vehicle's bounding box may have changed since it was worked out: set when an editor
/// operation ends (`PostModify`) and by undo and redo. It's then worked out again when the
/// editor isn't in a move mode and a garage is selected.
static BOX_DIRTY: AtomicBool = AtomicBool::new(true);
static ORIGINAL_UNDO: OnceLock<NativeThunk> = OnceLock::new();
static ORIGINAL_REDO: OnceLock<NativeThunk> = OnceLock::new();
/// Set once the editor's `PostModify` hook was tried (it's only tried once).
static POST_MODIFY_TRIED: AtomicBool = AtomicBool::new(false);

/// The game code and reflection this feature uses.
struct Game {
    editor: BrickEditor,
    bricks: Bricks,
    places: Places,
    bounds: LocalBounds,
    actors: Actors,
    garages: Garages,
    lines: LineBatcher,
    net: NetMode,
    /// `AActor::bReplicates`: a replicated editor would move for other players too.
    replicates: BoolField,
    /// The classes hidden while the map shows: characters (pawns) and vehicles.
    hidden_classes: [*const UStruct; 2],
    hidden_actors: HiddenActors,
    /// What keeps the editor and the world apart while the map shows; without it, no map.
    isolation: Option<Isolation>,
    /// `UPrimitiveComponent* UBrickEditorObject::GetRootComponent()`: each brick's, to check they
    /// were all kept apart.
    root: Getter<*mut UObjectBase>,
    /// The editor's side panel, to list the Map rows again when the map key or a tick changed them.
    panel: Option<EditorPanel>,
    modes: EditorModes,
    /// The editor's camera, for the address of its view target interface (`editor_frame`).
    camera: EditorCamera,
    /// `FRotator AController::GetControlRotation()` and `SetControlRotation(FRotator)`: the view's
    /// rotation in the world, which the editor's camera follows.
    control_rotation: Getter<Vector>,
    set_control_rotation: SingleCall,
    /// Whether the game's hiding can be skipped.
    can_show: bool,
    /// Whether the camera's offsets from the game's code and reflection agree (see
    /// `engine_init`).
    camera_agrees: bool,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for Game {}
unsafe impl Sync for Game {}

impl Feature for MapInEditor {
    fn id(&self) -> &'static str {
        "map_in_editor"
    }

    fn on_load(&self) -> LoadResult {
        brick_editor::after_tick(after_tick)
    }

    // A new map: the lines may be stale if the editor closed without a view to notice.
    fn on_map_load(&self) {
        let Some(game) = GAME.get() else { return };
        let mut state = state();
        // SAFETY: on the game thread, after a map load.
        unsafe { leave(game, &mut state) };
    }

    fn on_engine_init(&self) {
        match engine_init() {
            Ok(()) => LOG_STARTED.call_once(|| {
                log!(
                    "active: the editor's side panel (Map, with nothing selected) marks a garage's \
                     spawn point and shows the map, as does the map key (numpad zero){}",
                    match GAME.get() {
                        Some(game) if !game.can_show =>
                            " (map hiding not found: the map can't be shown)".to_owned(),
                        Some(Game {
                            isolation: Some(isolation),
                            ..
                        }) => format!(
                            " (the editor's clicks on collision channel {} while it shows)",
                            isolation.channel()
                        ),
                        _ => String::new(),
                    }
                )
            }),
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

fn engine_init() -> Result<(), String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let object = class("BrickEditorObject")?;
    let controller = class("Controller")?;
    let game = Game {
        editor: brick_editor::find()?,
        bricks: editor_view::bricks()?,
        places: Places::find()?,
        bounds: LocalBounds::find()?,
        actors: actor::find()?,
        garages: garage::find()?,
        lines: line_batcher::find()?,
        net: net_mode::find()?,
        replicates: reflection::bool_field(class("Actor")?.cast(), "bReplicates")?,
        hidden_classes: [class("Pawn")?.cast(), class("BrickVehicle")?.cast()],
        hidden_actors: hidden_actors::find()?,
        isolation: optional(isolation::find(), "the map can't be shown"),
        root: Getter::find(object, "GetRootComponent", "ObjectProperty")?,
        panel: optional(
            editor_panel::find(),
            "the Map rows show what the map key changed only once the panel lists them again",
        ),
        modes: editor_mode::find()?,
        camera: editor_camera::code().and_then(editor_camera::find)?,
        control_rotation: Getter::find(controller, "GetControlRotation", "StructProperty")?,
        set_control_rotation: SingleCall::find(
            controller,
            "SetControlRotation",
            "NewRotation",
            "StructProperty",
            12,
        )?,
        can_show: optional(hidden_components::install(), "the map can't be shown").is_some(),
        camera_agrees: false,
    };
    optional(hook_undo_redo(), "undoing doesn't update the box");
    // The camera's offsets in the editor, from the game's code and reflection: the relative
    // location where `GetCameraViewPoint` reads it (in the view target interface) and where
    // `SetCameraRelativeLocation` writes it; the rotation where the view target interface keeps
    // it and where the code that moves the camera and bricks reads it.
    let interface = game.camera.view_target_offset();
    let location = editor_frame::relative_location_offset();
    let rotation = editor_frame::camera_rotation_offset();
    let agrees = match (location, rotation, interface) {
        (Some(location), Some(rotation), Some(interface)) => {
            location + interface == game.camera.camera_location()
                && rotation == interface + editor_frame::view_rotation_offset()
        }
        _ => false,
    };
    if !agrees {
        log!(
            "the map can't be shown: the camera isn't where the game's code says (relative location \
             read at +{location:X?} in the interface at +{interface:X?}, written at +{:X}; \
             rotation at +{rotation:X?})",
            game.camera.camera_location()
        );
    }
    let game = Game {
        camera_agrees: agrees,
        ..game
    };
    if game.can_show {
        hidden_components::on_view(on_view);
    }
    let game = GAME.get_or_init(|| game);
    optional(
        panel::add(game),
        "no Map rows in the editor's panel (the map key still shows the map)",
    );
    Ok(())
}

/// Marks the box as possibly changed when the game undoes or redoes: those restore a snapshot and
/// may not end an operation.
fn hook_undo_redo() -> Result<(), String> {
    let undo = editor_input::button_action("Action_Undo")?;
    let redo = editor_input::button_action("Action_Redo")?;
    // SAFETY: both targets are exec thunks, and the detours are NativeThunks.
    unsafe {
        hook::install_typed(
            "UBaseEditorInputComponent::execAction_Undo",
            undo.thunk,
            undo_detour as NativeThunk,
            &ORIGINAL_UNDO,
        )?;
        hook::install_typed(
            "UBaseEditorInputComponent::execAction_Redo",
            redo.thunk,
            redo_detour as NativeThunk,
            &ORIGINAL_REDO,
        )
    }
}

unsafe extern "C" fn undo_detour(context: *mut c_void, stack: *mut c_void, result: *mut c_void) {
    BOX_DIRTY.store(true, Ordering::Relaxed);
    let original = *ORIGINAL_UNDO.get().expect("hook recorded before use");
    // SAFETY: the thunk's own arguments, passed on.
    unsafe { original(context, stack, result) }
}

unsafe extern "C" fn redo_detour(context: *mut c_void, stack: *mut c_void, result: *mut c_void) {
    BOX_DIRTY.store(true, Ordering::Relaxed);
    let original = *ORIGINAL_REDO.get().expect("hook recorded before use");
    // SAFETY: the thunk's own arguments, passed on.
    unsafe { original(context, stack, result) }
}

fn mark_box_dirty() {
    BOX_DIRTY.store(true, Ordering::Relaxed);
}

/// An editor operation ended: the box may have changed. Selecting is an operation too, so the
/// flags of each kind seen are logged once, to tell later which ones can't change the box.
fn operation_ended(flags: u32) {
    static SEEN: Mutex<Vec<u32>> = Mutex::new(Vec::new());
    mark_box_dirty();
    let mut seen = SEEN.lock().unwrap_or_else(PoisonError::into_inner);
    if !seen.contains(&flags) {
        seen.push(flags);
        log!("an editor operation ended with the flags 0x{flags:X}");
    }
}

/// `found`, or `None` after logging what goes `without` it.
fn optional<T>(found: Result<T, String>, without: &str) -> Option<T> {
    match found {
        Ok(found) => Some(found),
        Err(reason) => {
            log!("{without}: {reason}");
            None
        }
    }
}

/// What the feature is doing, for the editor it's about. What's asked for (the garage, the map)
/// is in [`panel::Controls`].
struct State {
    /// The editor everything below is about.
    editor: Option<WeakObject>,
    /// Whether the map is on (editor moved, hiding skipped).
    shown: bool,
    /// Where the editor was before it was moved (location, rotation), and where it was put.
    original: Option<[Vector; 2]>,
    placed: Option<[Vector; 2]>,
    /// The vehicle's box, and where it lands at the selected garage, as last worked out.
    vehicle_box: Option<VehicleBox>,
    landing: Option<(VehicleBox, Landing)>,
    /// Whether each class (by address) is one to hide while the map shows, so each object's
    /// parents aren't walked every time.
    class_hidden: BTreeMap<usize, bool>,
    /// Characters and vehicles hidden while the map shows, the player controller whose view hides
    /// them, and when they were looked for.
    hidden: Vec<WeakObject>,
    hidden_from: Option<WeakObject>,
    hidden_looked: Option<Instant>,
    /// The editor's components kept apart from the world while the map shows.
    isolated: Isolated,
    /// This machine's part in the game, when the map was last allowed to show.
    role: Option<Role>,
    /// Whether the map key was held last tick, to tell presses.
    held: bool,
    /// Whether any line of ours is drawn, to clear them once.
    drawn: bool,
    /// Whether the pinned setting was on when the landing was worked out.
    pinned: bool,
}

// SAFETY: weak object references, only used on the game thread (the mutex just satisfies
// statics).
unsafe impl Send for State {}

impl State {
    const fn new() -> Self {
        Self {
            editor: None,
            shown: false,
            original: None,
            placed: None,
            vehicle_box: None,
            landing: None,
            class_hidden: BTreeMap::new(),
            hidden: Vec::new(),
            hidden_from: None,
            hidden_looked: None,
            isolated: Isolated::new(),
            role: None,
            held: false,
            drawn: false,
            pinned: false,
        }
    }
}

/// Whether the map shows around the editor (false while that can't be told). For features that
/// look at the world around the vehicle, e.g. where its shots land.
pub fn shows_map() -> bool {
    STATE.try_lock().is_ok_and(|state| state.shown)
}

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// At each view of the player: when the editor isn't open, everything we changed goes back.
fn on_view(editor_open: bool) {
    if editor_open {
        return;
    }
    // Not while the editor's tick holds the state (it can't be this thread, but be safe).
    let Ok(mut state) = STATE.try_lock() else {
        return;
    };
    if state.editor.is_none()
        && !state.drawn
        && state.hidden.is_empty()
        && state.isolated.is_empty()
    {
        return;
    }
    let Some(game) = GAME.get() else { return };
    // SAFETY: on the game thread, in a view; what's restored is checked alive.
    unsafe { leave(game, &mut state) };
}

/// The editor closed (or another one opened): puts back what was hidden and the collision of the
/// editor's components still around, takes the lines away and forgets the editor and what was asked
/// for. The editor itself is gone or new, so it isn't moved back.
///
/// # Safety
/// Game thread only.
unsafe fn leave(game: &Game, state: &mut State) {
    hidden_components::set_show_map(false);
    editor_frame::clear();
    mark_box_dirty();
    panel::reset(None);
    // SAFETY: per the contract.
    unsafe {
        if let Some(isolation) = &game.isolation {
            isolation.restore(&mut state.isolated);
        }
        show_hidden(game, state);
        clear_lines(game, state);
    }
    *state = State::new();
}

/// Hooks the editor's `PostModify` (the first time): every operation ends there, which may change
/// the vehicle's box.
fn hook_post_modify(game: &Game, editor: *mut UObjectBase) {
    if POST_MODIFY_TRIED.swap(true, Ordering::Relaxed) {
        return;
    }
    // SAFETY: the editor is live while it ticks; game thread.
    let hooked = unsafe { game.editor.after_modify(editor, operation_ended) };
    optional(hooked, "editing doesn't update the box");
}

fn after_tick(editor: *mut UObjectBase, _delta: f32) {
    static PANEL_MISSING: std::sync::Once = std::sync::Once::new();
    let Some(game) = GAME.get() else { return };
    let (new_editor, changed) = {
        let mut state = state();
        // SAFETY: the editor is live while it ticks; game thread.
        unsafe {
            let new_editor = !state.editor.as_ref().is_some_and(|known| known.is(editor));
            if new_editor {
                leave(game, &mut state);
                state.editor = Some(WeakObject::new(editor));
                panel::reset(state.editor);
            }
            hook_post_modify(game, editor);
            (new_editor, tick(game, &mut state, editor))
        }
    };
    // A new editor's panel may have listed the rows before the editor ticked, so without the
    // garage it was entered from; its widget may not exist yet either, which is no failure.
    if new_editor || changed {
        // SAFETY: as above; the state is let go, which the rows don't read anyway.
        let found = unsafe { panel::refresh(game, editor) };
        if !found && changed {
            PANEL_MISSING.call_once(|| {
                log!("the editor's side panel wasn't found: its Map rows update when it lists them")
            });
        }
    }
}

/// One editor tick: acts on what the map key and the panel's rows asked for. Whether it changed
/// what the rows show.
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
unsafe fn tick(game: &Game, state: &mut State, editor: *mut UObjectBase) -> bool {
    let held = keybinds::SHOW_MAP.is_held();
    let pressed = held && !state.held;
    state.held = held;

    // Copied out: the panel reads them while it lists the rows, which may happen while this calls
    // the game.
    let asked = *panel::controls();
    let mut controls = panel::Controls {
        asked: false,
        ..asked
    };
    let mut force = asked.asked;
    if pressed {
        controls.show_map = !controls.show_map;
        force = true;
    }
    if PINNED.get() != state.pinned {
        state.pinned = PINNED.get();
        force = true;
    }

    // A garage that's gone (or no garage) isn't selected any more.
    let mut garage = controls
        .garage
        .and_then(|garage| garage.get())
        // SAFETY: a live object.
        .and_then(|actor| unsafe { game.garages.read(actor) });
    if garage.is_none() && controls.garage.is_some() {
        controls.garage = None;
        force = true;
    }
    if controls.show_map && garage.is_none() {
        // SAFETY: per the contract.
        garage = unsafe { select_default_garage(game, editor, &mut controls) };
        force = true;
    }

    // Does the map show? Only with a garage, and what keeps the editor and the world apart.
    let mut want_map = controls.show_map && garage.is_some();
    if want_map && (force || !state.shown) {
        // SAFETY: per the contract.
        match unsafe { may_show(game, editor) } {
            Ok(role) => state.role = Some(role),
            Err(reason) => {
                log!("the map isn't shown: {reason}");
                controls.show_map = false;
                want_map = false;
            }
        }
    }

    // The box is worked out again when it may have changed, but not while bricks are being moved
    // (the move only counts once it ends) and only if a garage shows where it lands.
    // SAFETY: per the contract.
    let moving = unsafe { game.modes.moving(editor) }.is_some();
    let mut new_box = false;
    if garage.is_some()
        && (state.vehicle_box.is_none() || (!moving && BOX_DIRTY.load(Ordering::Relaxed)))
    {
        BOX_DIRTY.store(false, Ordering::Relaxed);
        // SAFETY: per the contract.
        state.vehicle_box = unsafe { refresh_box(game, editor) };
        new_box = true;
    }
    if let Some(garage) = garage
        && (force || new_box)
    {
        // SAFETY: per the contract.
        unsafe { work_out_landing(game, state, &garage, force) };
    }

    let mut show = want_map && state.landing.is_some();
    // Before the editor first moves into the map, and when its bricks may have changed.
    if show && (!state.shown || new_box) {
        // SAFETY: per the contract.
        if let Err(reason) = unsafe { check_isolation(game, state, editor) } {
            log!("the map isn't shown: {reason}");
            controls.show_map = false;
            show = false;
        }
    }
    // SAFETY: per the contract.
    unsafe {
        let controller = game.editor.player_controller(editor);
        place_editor(game, state, editor, controller, show);
        let hide = state.shown;
        hide_others(game, state, editor, controller, hide);
        draw(game, state, editor, garage.as_ref());
    }

    *panel::controls() = controls;
    (controls.garage, controls.show_map) != (asked.garage, asked.show_map)
}

/// The garage `editor` was opened from (its actor), if it was one.
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
unsafe fn entry_garage(game: &Game, editor: *mut UObjectBase) -> Option<*mut UObjectBase> {
    // SAFETY: per the contract; the controller is null or live, and so is its entry point.
    unsafe {
        let controller = game.editor.player_controller(editor);
        if controller.is_null() {
            return None;
        }
        let entry = game.garages.entry_point(controller);
        game.garages.read(entry).map(|garage| garage.actor)
    }
}

/// Selects the garage the map shows at when none is: the one `editor` was opened from, else the
/// first. If the map has none, the map isn't shown.
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
unsafe fn select_default_garage(
    game: &Game,
    editor: *mut UObjectBase,
    controls: &mut panel::Controls,
) -> Option<Garage> {
    // SAFETY: per the contract.
    let (garages, entry) = unsafe { (game.garages.all(), entry_garage(game, editor)) };
    let entry = entry.and_then(|entry| garages.iter().position(|garage| garage.actor == entry));
    let Some(index) = entry.or((!garages.is_empty()).then_some(0)) else {
        log!("the map isn't shown: the map has no garage");
        controls.show_map = false;
        return None;
    };
    log!("garage {} of {} selected", index + 1, garages.len());
    let garage = garages[index];
    // SAFETY: the map's garages are live objects.
    controls.garage = Some(unsafe { WeakObject::new(garage.actor) });
    Some(garage)
}

/// Whether the map may be shown now, and this machine's part in the game: the editor and the world
/// can be kept apart ([`isolation`]), and in a networked game the editor actor is this machine's
/// alone (not replicated).
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
unsafe fn may_show(game: &Game, editor: *mut UObjectBase) -> Result<Role, String> {
    if !game.can_show {
        return Err("the game's hiding of the map wasn't found".into());
    }
    if !game.camera_agrees {
        return Err("the camera isn't where the game's code says (see the log's start)".into());
    }
    if !editor_frame::is_ready() {
        return Err("the turned editor's camera can't be kept right (editor_frame is off)".into());
    }
    // SAFETY: per the contract.
    if unsafe { game.camera.view_target_interface(editor) }.is_none() {
        return Err("the editor's view target wasn't found".into());
    }
    if game.isolation.is_none() {
        return Err(
            "the editor and the world can't be kept apart (see the log's start: \"the map can't be shown\")"
                .into(),
        );
    }
    // SAFETY: per the contract.
    unsafe {
        let world = world::current();
        if world.is_null() {
            return Err("there's no world".into());
        }
        let role = game.net.role(world);
        if role != Role::Standalone && game.replicates.get(editor.cast_const().cast()) {
            return Err(format!(
                "the editor actor is replicated ({role:?}): other players would see it move"
            ));
        }
        Ok(role)
    }
}

/// Keeps the editor apart from the world ([`isolation`]) and checks that every brick was: their
/// root components must all be, or the map would show among bricks that still collide with it (the
/// isolation works from the editor's own list of its components). If one wasn't, the map isn't
/// shown, and [`place_editor`] puts everything back.
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
unsafe fn check_isolation(
    game: &Game,
    state: &mut State,
    editor: *mut UObjectBase,
) -> Result<(), String> {
    let isolation = game
        .isolation
        .as_ref()
        .ok_or("the editor and the world can't be kept apart")?;
    // SAFETY: per the contract; the editor's objects are live or null, so are their roots.
    unsafe {
        isolation.apply(&mut state.isolated, editor);
        let roots: Vec<_> = game
            .bricks
            .editor_objects(editor)
            .iter()
            .copied()
            .filter(|object| !object.is_null())
            .map(|object| game.root.call(object))
            .filter(|root| !root.is_null())
            .collect();
        let missed = isolation.missed(roots.iter().copied());
        if !state.shown || missed > 0 {
            log!(
                "the editor's {} bricks: {missed} still collide with the world ({} components kept apart in all)",
                roots.len(),
                state.isolated.len()
            );
        }
        if missed > 0 {
            return Err(format!(
                "{missed} of the editor's {} bricks couldn't be kept apart from the world",
                roots.len()
            ));
        }
    }
    Ok(())
}

/// Works out the vehicle's box again, logging how long it took (when it was slow, or the first
/// time).
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
unsafe fn refresh_box(game: &Game, editor: *mut UObjectBase) -> Option<VehicleBox> {
    static LOG_FIRST: std::sync::Once = std::sync::Once::new();
    let started = Instant::now();
    // SAFETY: per the contract.
    let found = unsafe { vehicle_box(game, editor) };
    let took = started.elapsed();
    LOG_FIRST.call_once(|| log!("the vehicle's box took {took:.1?}"));
    if took > Duration::from_millis(20) {
        log!("the vehicle's box took {took:.1?} (slow)");
    }
    found
}

/// Where the vehicle lands at `garage` from the box worked out last; logs it when `log_it`.
///
/// # Safety
/// Game thread only.
unsafe fn work_out_landing(game: &Game, state: &mut State, garage: &Garage, log_it: bool) {
    let Some(vehicle) = state.vehicle_box else {
        state.landing = None;
        return;
    };
    // SAFETY: the garage is live (just read).
    let place = unsafe {
        GaragePlace {
            pose: Pose {
                location: math::widen(game.actors.location(garage.actor)),
                rotation: Quat::from_rotator(math::widen(game.actors.rotation(garage.actor))),
            },
            scale: math::widen(game.actors.scale(garage.actor)),
        }
    };
    let landing = placement::land(garage, &place, PINNED.get(), &vehicle);
    if log_it {
        // Where the box's middle lands, to compare with a spawned vehicle's place.
        let middle = math::scale(math::add(vehicle.min, vehicle.max), 0.5);
        let world = |point: Vec3| {
            math::add(
                landing.vehicle.location,
                landing.vehicle.rotation.rotate(point),
            )
        };
        log!(
            "vehicle box {:.1?} to {:.1?}; spawn point in the vehicle {:.1?}, in the world {:.1?}; the vehicle lands at {:.1?}, rotation {:.2?}, its box's middle at {:.1?} (garage at {:.1?}, rotation {:.2?})",
            vehicle.min,
            vehicle.max,
            landing.spawn_point,
            world(landing.spawn_point),
            landing.vehicle.location,
            landing.vehicle.rotation.to_rotator(),
            world(middle),
            place.pose.location,
            place.pose.rotation.to_rotator(),
        );
    }
    state.landing = Some((vehicle, landing));
}

/// The smallest box around every editor object's bounds, in the vehicle's space, as the game's
/// `CalcBrickEditorObjectsBounds` works it out.
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
unsafe fn vehicle_box(game: &Game, editor: *mut UObjectBase) -> Option<VehicleBox> {
    // SAFETY: per the contract.
    let found =
        unsafe { vehicle_box::vehicle_box(&game.bricks, &game.places, &game.bounds, editor) };
    found.map(|[min, max]| VehicleBox { min, max })
}

/// Moves the editor to where the vehicle lands (when `show`), or back where it was. The camera
/// keeps looking at the vehicle from where it was (the view turns with the editor, see
/// [`turn_view`]) and `editor_frame` keeps its location right. The editor is kept apart from the
/// world ([`isolation`]) before it moves into the map, every tick it's there, and until it's out.
///
/// # Safety
/// `editor` must be the live editor, `controller` its player controller or null. Game thread only.
unsafe fn place_editor(
    game: &Game,
    state: &mut State,
    editor: *mut UObjectBase,
    controller: *mut UObjectBase,
    show: bool,
) {
    // SAFETY: per the contract.
    unsafe {
        if show
            && let Some(isolation) = &game.isolation
            && let Some((_, landing)) = &state.landing
            && let Some(interface) = game.camera.view_target_interface(editor)
        {
            isolation.apply(&mut state.isolated, editor);
            let target = [
                math::narrow(landing.vehicle.location),
                math::narrow(landing.vehicle.rotation.to_rotator()),
            ];
            if state.original.is_none() {
                state.original = Some([game.actors.location(editor), game.actors.rotation(editor)]);
            }
            let same = state.placed.is_some_and(|placed| {
                (0..3).all(|axis| {
                    (placed[0][axis] - target[0][axis]).abs() < 0.01
                        && angle_gap(placed[1][axis], target[1][axis]) < 0.01
                })
            });
            if !same {
                let before = game.actors.rotation(editor);
                game.actors.teleport(editor, target[0], target[1]);
                editor_frame::set(
                    editor as usize,
                    interface,
                    math::widen(target[0]),
                    Quat::from_rotator(math::widen(target[1])),
                );
                turn_view(game, controller, interface, before, target[1]);
                state.placed = Some(target);
            }
            if !state.shown {
                state.shown = true;
                log!(
                    "map shown ({}): the editor moved from {:.1?} to {:.1?}",
                    role_text(state.role),
                    state.original,
                    target
                );
            }
            hidden_components::set_show_map(true);
        } else {
            hidden_components::set_show_map(false);
            if let Some([location, rotation]) = state.original.take() {
                let before = game.actors.rotation(editor);
                game.actors.teleport(editor, location, rotation);
                editor_frame::clear();
                if let Some(interface) = game.camera.view_target_interface(editor) {
                    turn_view(game, controller, interface, before, rotation);
                }
                log!("map hidden: the editor is back at {location:.1?}");
            }
            if let Some(isolation) = &game.isolation {
                isolation.restore(&mut state.isolated);
            }
            state.placed = None;
            state.shown = false;
        }
    }
}

/// How the log names this machine's part in the game.
fn role_text(role: Option<Role>) -> &'static str {
    match role {
        Some(Role::Standalone) => "single-player",
        Some(Role::Host) => "hosting",
        Some(Role::Client) => "joined",
        None => "unknown game",
    }
}

/// Turns the player's view by what the editor turned from `before` to `after` (pitch, yaw, roll),
/// so that the vehicle looks the same from the camera: the control rotation (in the world), which
/// the camera's follows from the next frame, and the camera's own (its view target `interface`)
/// for this one.
///
/// # Safety
/// `controller` must be null or the live controller, `interface` the live editor's view target
/// interface. Game thread only.
unsafe fn turn_view(
    game: &Game,
    controller: *mut UObjectBase,
    interface: usize,
    before: Vector,
    after: Vector,
) {
    let turn =
        Quat::from_rotator(math::widen(after)) * Quat::from_rotator(math::widen(before)).inverse();
    // SAFETY: per the contract; the controller has both functions.
    unsafe {
        editor_frame::turn_camera(interface, turn);
        if controller.is_null() {
            return;
        }
        let view = Quat::from_rotator(math::widen(game.control_rotation.call(controller)));
        let turned = math::narrow((turn * view).to_rotator());
        game.set_control_rotation.call(controller, turned);
    }
}

/// The smallest turn between two angles in degrees.
fn angle_gap(a: f32, b: f32) -> f32 {
    ((a - b + 180.0).rem_euclid(360.0) - 180.0).abs()
}

/// While the map shows, hides the characters and vehicles from `controller`'s view (the game hid
/// them with the rest of the map), looking for new ones now and then; when it doesn't, shows the
/// ones it hid. Only this player's view changes (`game::hidden_actors`), even when hosting.
///
/// # Safety
/// `editor` must be the live editor, `controller` its player controller or null. Game thread only.
unsafe fn hide_others(
    game: &Game,
    state: &mut State,
    editor: *mut UObjectBase,
    controller: *mut UObjectBase,
    hide: bool,
) {
    static EMPTIED: std::sync::Once = std::sync::Once::new();
    // SAFETY: per the contract.
    unsafe {
        let known = state.hidden_from.as_ref().and_then(WeakObject::get);
        if !hide || controller.is_null() || known.is_some_and(|known| known != controller) {
            show_hidden(game, state);
        }
        if !hide || controller.is_null() {
            return;
        }
        if state
            .hidden_looked
            .is_some_and(|at| at.elapsed() < HIDE_REFRESH)
        {
            return;
        }
        let started = Instant::now();
        state.hidden_looked = Some(started);
        state.hidden_from = Some(WeakObject::new(controller));
        // The list must keep what was added: if the game empties it, nothing stays hidden.
        if state
            .hidden
            .iter()
            .filter_map(WeakObject::get)
            .any(|actor| !game.hidden_actors.contains(controller, actor))
        {
            EMPTIED.call_once(|| {
                log!("the game empties the player's hidden actors: characters and vehicles show")
            });
        }
        let editor_class = reflection::class_of(editor.cast()) as usize;
        // On the game thread, GObjects slots hold live objects or null, and every object has a
        // valid class.
        for object in GObjects().iter() {
            if object.is_null()
                || (*object)
                    .object_flags
                    .contains(EObjectFlags::RF_ClassDefaultObject)
            {
                continue;
            }
            let class = reflection::class_of(object.cast());
            let hide = *state.class_hidden.entry(class as usize).or_insert_with(|| {
                let parents: Vec<_> = reflection::with_parents(class).collect();
                parents
                    .iter()
                    .any(|class| game.hidden_classes.contains(class))
                    && !parents.iter().any(|&class| class as usize == editor_class)
            });
            if !hide
                || game.actors.is_hidden(object)
                || state.hidden.iter().any(|known| known.is(object))
            {
                continue;
            }
            game.hidden_actors.add(controller, object);
            state.hidden.push(WeakObject::new(object));
        }
        let took = started.elapsed();
        if took > Duration::from_millis(20) {
            log!("looking for characters and vehicles to hide took {took:.1?}");
        }
    }
}

/// Shows again the characters and vehicles [`hide_others`] hid, taking them out of the player's
/// hidden actors (destroyed ones are gone from it already).
///
/// # Safety
/// Game thread only.
unsafe fn show_hidden(game: &Game, state: &mut State) {
    state.hidden_looked = None;
    let controller = state.hidden_from.take().and_then(|known| known.get());
    for hidden in state.hidden.drain(..) {
        if let (Some(controller), Some(actor)) = (controller, hidden.get()) {
            // SAFETY: the controller is the live player controller (just checked alive).
            unsafe { game.hidden_actors.remove(controller, actor) };
        }
    }
}

/// Draws the selected `garage`'s spawn point (a cross), its spawn box and the vehicle's box, in the
/// vehicle's space where the editor is now; clears them if none is selected.
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
unsafe fn draw(game: &Game, state: &mut State, editor: *mut UObjectBase, garage: Option<&Garage>) {
    // SAFETY: per the contract.
    unsafe {
        let batcher = game.lines.batcher();
        let landing = state.landing.filter(|_| garage.is_some());
        let (Some((vehicle, landing)), false) = (landing, batcher.is_null()) else {
            clear_lines(game, state);
            return;
        };
        let origin = math::widen(game.actors.location(editor));
        let turn = Quat::from_rotator(math::widen(game.actors.rotation(editor)));
        let world = |point: Vec3| math::add(origin, turn.rotate(point));
        let mut lines = Vec::new();
        let spawn = landing.spawn_point;
        for (axis, color) in [
            ([1.0, 0.0, 0.0], [1.0, 0.1, 0.1]),
            ([0.0, 1.0, 0.0], [0.1, 1.0, 0.1]),
            ([0.0, 0.0, 1.0], [0.2, 0.4, 1.0]),
        ] {
            lines.push(line(
                world(spawn),
                world(math::add(spawn, math::scale(axis, CROSS))),
                color,
                3.0,
                true,
            ));
        }
        if let Some(garage) = garage {
            let half = math::widen(garage.extent);
            boxed(
                &mut lines,
                &world,
                math::sub(spawn, half),
                math::add(spawn, half),
                [1.0, 0.8, 0.1],
            );
        }
        boxed(
            &mut lines,
            &world,
            vehicle.min,
            vehicle.max,
            [0.5, 0.5, 0.5],
        );
        game.lines.replace(batcher, LINES, &lines);
        state.drawn = true;
    }
}

/// Adds the 12 edges of the box `min` to `max` (in the vehicle's space), through `world`.
fn boxed(
    lines: &mut Vec<Line>,
    world: &impl Fn(Vec3) -> Vec3,
    min: Vec3,
    max: Vec3,
    color: [f32; 3],
) {
    let corner = |index: usize| {
        world([
            if index & 1 == 0 { min[0] } else { max[0] },
            if index & 2 == 0 { min[1] } else { max[1] },
            if index & 4 == 0 { min[2] } else { max[2] },
        ])
    };
    for start in 0..8usize {
        for bit in [1, 2, 4] {
            if start & bit == 0 {
                lines.push(line(corner(start), corner(start | bit), color, 0.0, false));
            }
        }
    }
}

fn line(start: Vec3, end: Vec3, color: [f32; 3], thickness: f32, on_top: bool) -> Line {
    Line {
        start: math::narrow(start),
        end: math::narrow(end),
        color,
        thickness,
        on_top,
    }
}

/// Takes our lines away, if any are drawn.
///
/// # Safety
/// Game thread only.
unsafe fn clear_lines(game: &Game, state: &mut State) {
    if !state.drawn {
        return;
    }
    state.drawn = false;
    // SAFETY: per the contract; the batcher is null or live.
    unsafe {
        let batcher = game.lines.batcher();
        if !batcher.is_null() {
            game.lines.replace(batcher, LINES, &[]);
        }
    }
}
