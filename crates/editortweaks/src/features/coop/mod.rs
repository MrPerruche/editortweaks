//! Coop building: several players editing one vehicle (`docs/re/coop/`). Work in progress, and
//! only in builds with the `coop` cargo feature (`et.py build --coop`); release builds don't
//! contain any of it. The protocol and the session are the `coop` crate; this is the editor side.
//!
//! For now (milestone 3, `protocol.md`): the local test. With its setting on, two game instances on
//! one PC link over localhost (`local`): the first editor opened hosts its vehicle, the other
//! joins and gets it. Creating, deleting and moving bricks, changing their properties (those
//! `game::property_values` can sync), reordering them (seat order, camera order, ...) and grouping
//! them (editor and weld groups) then shows in both. Each instance logs to its own file
//! (`log`). And at each start, a check that Steam's messages work (`probe`).
//!
//! Undo and redo undo the player's own edits only (`coop::undo`): each edit goes to the session
//! with what undoes it, worked out from what `changes` remembered just before (and deleted bricks
//! saved as they go). The game's own undo, which loads an older whole vehicle, doesn't run while
//! linked (`game::undo_redo`).
//!
//! Every few seconds the subhost describes its editor in short (`coop::audit`: the brick count, the
//! object order, digests of some bricks going round the vehicle, those edited lately first). A
//! member at the same commit with nothing of its own on the way compares its editor: bricks that
//! differ ask for a snapshot, an order that differs asks for the subhost's order. The drift key
//! (`drift`) makes a member differ on purpose, to see that work.
//!
//! Each editor tick, while the editor is in its default mode (not moving or placing bricks):
//! finished local edits go to the session, then what arrived is applied, then snapshot (and
//! order) requests are answered, so a snapshot always holds exactly what was committed; then the
//! subhost audits. In other modes coop only sends: reading would mean editing bricks the player is
//! moving.

mod apply;
mod backup;
mod changes;
mod checks;
mod drift;
mod local;
mod log;
mod probe;

use std::cell::RefCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ::coop::audit::{self, Audit, Rounds};
use ::coop::tcp::TcpTransport;
use ::coop::transport::{Transport, pump};
use ::coop::vehicle::Vehicle;
use ::coop::{
    Change, CoopId, Difference, Digest, Event, GroupKind, Groups, HostSettings, Op, PeerId,
    PeerInfo, Session, SessionError, SnapshotRequest, Transform, Value,
};
use brickrust::ue::coreuobject::UObjectBase;

use self::apply::Target;
use self::changes::{Buried, Object};
use self::local::Role;
use self::log::coop_log;
use crate::framework::settings::{Section, Setting, Toggle};
use crate::game::brick_editor::{self, BrickEditor};
use crate::game::brick_groups::{self, BrickGroups};
use crate::game::editor_mode::{self, EditorModes};
use crate::game::editor_object::SpawnTransforms;
use crate::game::editor_view::{self, Bricks};
use crate::game::input_sources::{self, InputSources};
use crate::game::object_order::{self, ObjectOrder};
use crate::game::property_values::{self, BrickValues};
use crate::game::undo_redo;
use crate::game::vehicle_archive::{self, VehicleArchive};
use crate::game::vehicle_changes;
use crate::platform::keyboard;
use crate::prelude::*;

pub struct Coop;

pub static LOCAL_TEST: Toggle = Toggle::new(
    "CoopLocalTest",
    "Local coop test",
    "Links this game with a second one running on this PC (coop builds only): the first editor \
     opened hosts its vehicle, the other joins it. Only creating, deleting, moving, reordering \
     and grouping bricks and changing their properties are shared so far. Undo and redo undo \
     your own edits only. The second game checks every few seconds that its vehicle is still the \
     first's (Ctrl+F8 there makes it differ on purpose, to test that).",
    false,
);

/// The feature's settings, in the options' Editor tweaks menu.
pub static SETTINGS: [Section; 1] = [Section {
    title: "Coop building (in development)",
    settings: &[Setting::Toggle(&LOCAL_TEST)],
    unlisted: &[],
}];

/// How long to wait before linking again after a failure or the end of a session.
const RETRY: Duration = Duration::from_secs(2);

/// The game code coop uses, found on the first editor tick (after engine init).
pub struct Game {
    editor: BrickEditor,
    bricks: Bricks,
    modes: EditorModes,
    archive: VehicleArchive,
    spawn: SpawnTransforms,
    values: BrickValues,
    /// The game's reorder, to apply someone else's (or why not: then only that is refused).
    order: Result<ObjectOrder, String>,
    /// Where objects keep their groups (or why not known: then only groups don't sync).
    groups: Result<BrickGroups, String>,
    /// What the panel allows that peers' edits are held to (or why not known: then what it
    /// checks is refused).
    checks: Result<checks::Checks, String>,
    /// The game's check of an input channel's sources (or why not found: then sources are
    /// refused).
    sources: Result<InputSources, String>,
}

static GAME: OnceLock<Result<Game, String>> = OnceLock::new();

/// Whether coop took over the game's undo and redo (or why not).
static UNDO: OnceLock<Result<(), String>> = OnceLock::new();

fn game() -> Option<&'static Game> {
    let found = GAME.get_or_init(|| {
        let archive = vehicle_archive::find()?;
        coop_log!("vehicle archive: {}", archive.layout());
        Ok(Game {
            editor: brick_editor::find()?,
            bricks: editor_view::bricks()?,
            modes: editor_mode::find()?,
            archive,
            spawn: SpawnTransforms::find()?,
            values: property_values::find()?,
            order: object_order::find(),
            groups: brick_groups::find(),
            checks: checks::Checks::find(),
            sources: input_sources::find(),
        })
    });
    match found {
        Ok(game) => Some(game),
        Err(reason) => {
            static LOGGED: AtomicBool = AtomicBool::new(false);
            if !LOGGED.swap(true, Ordering::Relaxed) {
                coop_log!("coop unavailable: {reason}");
            }
            None
        }
    }
}

impl Feature for Coop {
    fn id(&self) -> &'static str {
        "coop"
    }

    fn on_load(&self) -> LoadResult {
        log!(
            "built in (protocol {}), work in progress",
            ::coop::message::PROTOCOL
        );
        vehicle_changes::on_destroyed(changes::destroyed)?;
        let undo = UNDO.get_or_init(|| undo_redo::take_over(changes::step_back_asked));
        if let Err(reason) = undo {
            log!("can't take over undo ({reason}): in coop it would load older vehicles");
        }
        brick_editor::after_tick(after_tick)
    }
}

/// What we tell peers about ourselves.
fn info() -> PeerInfo {
    PeerInfo {
        protocol: (::coop::message::PROTOCOL, ::coop::message::PROTOCOL),
        mod_version: env!("CARGO_PKG_VERSION").into(),
        game_version: crate::TARGET_GAME_VERSION.to_string_lossy().into_owned(),
        hosting: false,
    }
}

/// A session with another instance, in one editor.
struct Link {
    session: Session,
    transport: TcpTransport,
    role: Role,
    /// A live `AVehicleEditor` (the one ticking), and its interface component.
    editor: Object,
    component: Object,
    /// Whether a vehicle was loaded (a later one is a resync).
    loaded: bool,
    /// Loads that didn't match their ids in a row: after a few, asking again won't help.
    bad_loads: u32,
    /// Subhost: when to audit next, the bricks to audit next, and those edits touched since the
    /// last audit (audited first).
    audit_at: Instant,
    rounds: Rounds,
    touched: Vec<CoopId>,
    /// Member: audits in a row that found bricks differing (see [`audited`]).
    audits_failed: u32,
    /// The session's clock starts here (its rate limits follow it).
    started: Instant,
    backups: backup::Backups,
}

/// What members asked the subhost for, answered once every event is applied.
#[derive(Default)]
struct Requests {
    snapshots: Vec<SnapshotRequest>,
    orders: Vec<PeerId>,
}

enum State {
    Idle { retry_at: Instant },
    Linked(Box<Link>),
}

thread_local! {
    // The game thread's: the editor ticks there.
    static STATE: RefCell<State> = RefCell::new(State::Idle { retry_at: Instant::now() });
}

fn after_tick(editor: *mut UObjectBase, _delta: f32) {
    probe::tick(info);
    STATE.with_borrow_mut(|state| {
        if let State::Linked(link) = state
            && (!LOCAL_TEST.get() || link.editor != editor)
        {
            let why = if LOCAL_TEST.get() {
                "the editor changed"
            } else {
                "local test turned off"
            };
            stop(link, why);
            *state = State::Idle {
                retry_at: Instant::now(),
            };
        }
        if !LOCAL_TEST.get() {
            return;
        }
        let Some(game) = game() else { return };
        if let State::Idle { retry_at } = state {
            if Instant::now() < *retry_at {
                return;
            }
            match start(game, editor) {
                Some(link) => *state = State::Linked(link),
                None => {
                    *retry_at = Instant::now() + RETRY;
                    return;
                }
            }
        }
        let State::Linked(link) = state else { return };
        if !step(game, link) {
            stop(link, "the session ended");
            *state = State::Idle {
                retry_at: Instant::now() + RETRY,
            };
        }
    });
}

/// The editor's objects, without empty entries.
///
/// # Safety
/// `editor` must be a live `ABrickEditor`. Game thread only.
unsafe fn objects(game: &Game, editor: Object) -> Vec<Object> {
    // SAFETY: per the contract; copied at once.
    unsafe { game.bricks.editor_objects(editor) }
        .iter()
        .copied()
        .filter(|object| !object.is_null())
        .collect()
}

/// Gives `objects` their coop `ids`, in order, and remembers where they are, their property
/// values and their groups (to see them change).
///
/// # Safety
/// `objects` must be live editor objects. Game thread only.
unsafe fn adopt(game: &Game, objects: &[Object], ids: &[CoopId]) {
    changes::assign(objects, ids);
    // SAFETY: per the contract.
    unsafe { game.spawn.learn(objects.iter().copied()) };
    for (&object, &id) in objects.iter().zip(ids) {
        // SAFETY: per the contract.
        if let Some(transform) = unsafe { place_of(game, object) } {
            changes::place(id, transform);
        }
        // SAFETY: per the contract.
        if let Some(values) = unsafe { values_of(game, object) } {
            changes::remember_values(id, values);
        }
        // SAFETY: per the contract.
        if let Some(groups) = unsafe { groups_of(game, object) } {
            changes::remember_groups(id, groups);
        }
    }
}

/// `object`'s groups; `None` if where objects keep them isn't known.
///
/// # Safety
/// `object` must be a live editor object. Game thread only.
unsafe fn groups_of(game: &Game, object: Object) -> Option<Groups> {
    let groups = game.groups.as_ref().ok()?;
    // SAFETY: per the contract.
    Some(unsafe { groups.read(object) })
}

/// Remembers the groups of every brick with a coop id as they are now (after coop set some).
fn remember_all_groups(game: &Game) {
    for (object, id) in changes::known() {
        // SAFETY: objects with coop ids are live (deleted ones are forgotten as they go).
        if let Some(groups) = unsafe { groups_of(game, object) } {
            changes::remember_groups(id, groups);
        }
    }
}

/// The `Group` ops putting the brick `id` from groups `was` into groups `now`.
fn group_ops(id: CoopId, was: Groups, now: Groups) -> impl Iterator<Item = Op> {
    GroupKind::ALL
        .into_iter()
        .filter(move |&kind| was[kind as usize] != now[kind as usize])
        .map(move |kind| Op::Group {
            id,
            kind,
            group: now[kind as usize],
        })
}

/// The group changes of every brick since its groups were last remembered (remembering the new
/// ones), each undone by the group it was in.
fn regroups(game: &Game) -> Vec<Change> {
    let mut changes = Vec::new();
    for (object, id) in changes::known() {
        // SAFETY: objects with coop ids are live (deleted ones are forgotten as they go).
        let Some(now) = (unsafe { groups_of(game, object) }) else {
            continue;
        };
        if let Some(was) = changes::groups_changed(id, now) {
            changes.extend(group_ops(id, was, now).zip(group_ops(id, now, was)).map(
                |(done, undo)| Change {
                    done: vec![done],
                    undo: vec![undo],
                },
            ));
        }
    }
    changes
}

/// Remembers the property values of every brick with a coop id as they are now (after coop set
/// some: setting one may change others, which mustn't go back as edits of ours).
fn remember_all_values(game: &Game) {
    for (object, id) in changes::known() {
        // SAFETY: objects with coop ids are live (deleted ones are forgotten as they go).
        if let Some(values) = unsafe { values_of(game, object) } {
            changes::remember_values(id, values);
        }
    }
}

/// `object`'s syncable property values, as kept to compare (see `property_values`); `None` if it
/// isn't a brick.
///
/// # Safety
/// `object` must be a live editor object. Game thread only.
unsafe fn values_of(game: &Game, object: Object) -> Option<Vec<u8>> {
    // SAFETY: per the contract.
    let class = unsafe { game.values.of(object) }?;
    // SAFETY: per the contract.
    Some(unsafe { class.read(object) })
}

/// Logs which brick properties coop syncs and which it can't (after a game update, the place to
/// look first).
fn log_inventory(game: &Game) {
    // SAFETY: game thread (the editor's tick).
    let inventory = unsafe { game.values.inventory() };
    coop_log!(
        "brick properties: {} synced, over {} brick classes",
        inventory.synced.len(),
        inventory.classes
    );
    for problem in game.values.problems() {
        coop_log!("  can't sync some: {problem}");
    }
    if inventory.skipped.is_empty() {
        coop_log!("  every brick property is synced");
    } else {
        let skipped: Vec<&str> = inventory.skipped.iter().map(String::as_str).collect();
        coop_log!("  not synced: {}", skipped.join(", "));
    }
    if let Err(reason) = &game.order {
        coop_log!("  can't apply reorders: {reason}");
    }
    if let Err(reason) = &game.groups {
        coop_log!("  can't sync groups: {reason}");
    }
    if let Err(reason) = &game.checks {
        coop_log!("  peers' reorders and cameras' seats refused: {reason}");
    }
    if let Err(reason) = &game.sources {
        coop_log!("  peers' input sources refused: {reason}");
    }
    if let Some(Err(reason)) = UNDO.get() {
        coop_log!("  can't take over undo: {reason}");
    }
}

/// `value` as it travels: bricks by their coop ids (bricks a local edit just deleted too, for
/// undoing it); with how many of its bricks have none (never shared), left out.
fn travelling(value: property_values::Value) -> (Value, usize) {
    match value {
        property_values::Value::Data(bytes) => (Value::Raw(bytes), 0),
        property_values::Value::Text(text) => (Value::Text(text), 0),
        property_values::Value::Bricks(bricks) => {
            let ids: Vec<CoopId> = bricks
                .iter()
                .filter_map(|&brick| changes::id_of(brick).or_else(|| changes::id_of_buried(brick)))
                .collect();
            let left_out = bricks.len() - ids.len();
            (Value::Bricks(ids), left_out)
        }
    }
}

/// The `Set` op giving the brick `id` the value `value` of `property` (see [`travelling`]; bricks
/// left out are logged).
fn set_op(id: CoopId, property: &str, value: property_values::Value) -> Op {
    let (value, left_out) = travelling(value);
    if left_out > 0 {
        coop_log!("{property}: {left_out} of its bricks aren't shared, left out");
    }
    Op::Set {
        id,
        property: property.into(),
        value,
    }
}

/// `object`'s digest (`coop::audit`): its place, its synced property values as they travel and its
/// groups. `None` if it isn't a brick coop reads.
///
/// # Safety
/// `object` must be a live editor object. Game thread only.
unsafe fn digest_of(game: &Game, object: Object) -> Option<Digest> {
    // SAFETY: per the contract; the values are taken from what was just read.
    unsafe {
        let place = place_of(game, object)?;
        let class = game.values.of(object)?;
        let now = class.read(object);
        let values: Vec<(String, Value)> = class
            .differences(&game.values, &[], &now)
            .into_iter()
            .map(|(property, value, _)| (property.name.clone(), travelling(value).0))
            .collect();
        let groups = groups_of(game, object).unwrap_or_default();
        Some(Digest::of(
            &place,
            values.iter().map(|(name, value)| (name.as_str(), value)),
            groups,
        ))
    }
}

/// The property changes of every brick since its values were last remembered (remembering the new
/// ones), each undone by the value it had (if that's still possible: an asset may be gone).
fn property_sets(game: &Game) -> Vec<Change> {
    let mut sets = Vec::new();
    for (object, id) in changes::known() {
        // SAFETY: objects with coop ids are live (deleted ones are forgotten as they go); the
        // values are read and compared at once.
        unsafe {
            let Some(class) = game.values.of(object) else {
                continue;
            };
            let now = class.read(object);
            let Some(was) = changes::values_changed(id, now.clone()) else {
                continue;
            };
            for (property, value, earlier) in class.differences(&game.values, &was, &now) {
                sets.push(Change {
                    done: vec![set_op(id, &property.name, value)],
                    undo: earlier
                        .map(|earlier| set_op(id, &property.name, earlier))
                        .into_iter()
                        .collect(),
                });
            }
        }
    }
    sets
}

/// What follows the `Create` of `objects` (new, with coop `ids`), which their archive can't carry:
/// their brick references (an archive of only them may leave out references to other bricks) as
/// sets, and their groups (the loader gives loaded bricks new group numbers).
///
/// # Safety
/// `objects` must be live editor objects. Game thread only.
unsafe fn creation_extras(game: &Game, objects: &[Object], ids: &[CoopId]) -> Vec<Op> {
    let mut ops = Vec::new();
    for (&object, &id) in objects.iter().zip(ids) {
        // SAFETY: per the contract.
        unsafe {
            if let Some(class) = game.values.of(object) {
                for (property, value) in class.references(&game.values, object) {
                    ops.push(set_op(id, &property.name, value));
                }
            }
            if let Some(groups) = groups_of(game, object) {
                ops.extend(group_ops(id, [0; 2], groups));
            }
        }
    }
    ops
}

/// Where `object` is, as the editor keeps it.
///
/// # Safety
/// `object` must be a live editor object. Game thread only.
unsafe fn place_of(game: &Game, object: Object) -> Option<Transform> {
    // SAFETY: per the contract.
    let (location, rotation) = unsafe { game.spawn.read(object) }?;
    Some(Transform { location, rotation })
}

/// The bricks that moved since their places were last remembered (remembering the new ones), each
/// undone by the place it had.
fn moves(game: &Game) -> Vec<Change> {
    changes::known()
        .into_iter()
        .filter_map(|(object, id)| {
            // SAFETY: objects with coop ids are live (deleted ones are forgotten as they go).
            let transform = unsafe { place_of(game, object) }?;
            let was = changes::moved(id, transform)?;
            Some(Change {
                done: vec![Op::Move { id, transform }],
                undo: was
                    .map(|transform| Op::Move { id, transform })
                    .into_iter()
                    .collect(),
            })
        })
        .collect()
}

/// What bringing back `bricks` (the editor's objects with their coop ids, about to be deleted by a
/// local edit, still whole) takes: the editor's archive of them, then what it can't carry, as
/// for creating them ([`creation_extras`]). Nothing if they can't be saved (logged).
fn bury(component: Object, bricks: &[(Object, CoopId)]) -> Buried {
    let ids: Vec<CoopId> = bricks.iter().map(|&(_, id)| id).collect();
    let objects: Vec<Object> = bricks.iter().map(|&(object, _)| object).collect();
    let Some(game) = game() else {
        return Buried {
            ids,
            undo: Vec::new(),
        };
    };
    // SAFETY: the objects are live, their component's (they're about to be destroyed); game
    // thread (the game's own delete).
    let undo = match unsafe { game.archive.save(component, &objects) } {
        Ok(archive) => {
            // SAFETY: as above.
            let extras = unsafe { creation_extras(game, &objects, &ids) };
            std::iter::once(Op::Create {
                ids: ids.clone(),
                archive,
            })
            .chain(extras)
            .collect()
        }
        Err(reason) => {
            coop_log!("deleting {} objects can't be undone: {reason}", ids.len());
            Vec::new()
        }
    };
    Buried { ids, undo }
}

/// Hooks what coop needs from `editor` (the first time).
fn hook(game: &Game, editor: Object) -> Result<(), String> {
    static AFTER_MODIFY: AtomicBool = AtomicBool::new(false);
    // SAFETY: the editor is live (it's ticking); game thread.
    unsafe {
        vehicle_changes::on_created(editor, changes::created)?;
        if !AFTER_MODIFY.load(Ordering::Relaxed) {
            game.editor.after_modify(editor, changes::modified)?;
            AFTER_MODIFY.store(true, Ordering::Relaxed);
        }
    }
    Ok(())
}

/// Links with the other instance, hosting or joining.
fn start(game: &Game, editor: Object) -> Option<Box<Link>> {
    if let Err(reason) = hook(game, editor) {
        coop_log!("can't watch the editor: {reason}");
        return None;
    }
    // SAFETY: the editor is live (it's ticking).
    let component = unsafe { game.bricks.interface(editor) };
    if component.is_null() {
        return None;
    }
    let (transport, role) = match local::link() {
        Ok(linked) => linked,
        Err(error) => {
            coop_log!("local test: can't link: {error}");
            return None;
        }
    };
    log::start(role.name());
    log_inventory(game);
    let name = format!("{} {}", role.name(), std::process::id());
    let mut session = Session::new(info());
    changes::watch(editor, component);
    match role {
        Role::Host => {
            // SAFETY: the editor is live.
            let objects = unsafe { objects(game, editor) };
            let settings = HostSettings {
                code: local::CODE.into(),
                max_members: 7,
            };
            // Random enough that a member's frames from an older session drop.
            let seed = std::process::id()
                ^ std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .subsec_nanos();
            match session.host(seed, name, settings, objects.len()) {
                // SAFETY: the editor's live objects.
                Ok(ids) => unsafe { adopt(game, &objects, &ids) },
                Err(error) => {
                    coop_log!("can't host: {error:?}");
                    changes::unwatch();
                    return None;
                }
            }
            coop_log!(
                "local test: hosting this vehicle ({} objects)",
                objects.len()
            );
        }
        Role::Member => {
            session
                .join(local::HOST, local::CODE.into(), name)
                .expect("a new session is idle");
            coop_log!("local test: joining the other instance");
        }
    }
    Some(Box::new(Link {
        session,
        transport,
        role,
        editor,
        component,
        loaded: false,
        bad_loads: 0,
        audit_at: Instant::now(),
        rounds: Rounds::default(),
        touched: Vec::new(),
        audits_failed: 0,
        started: Instant::now(),
        backups: backup::Backups::new(),
    }))
}

/// Ends the link: tells the others, forgets the vehicle's ids.
fn stop(link: &mut Link, why: &str) {
    link.session.leave();
    send(link);
    changes::unwatch();
    coop_log!("local test stopped: {why}");
}

/// Sends what the session has queued, without reading.
fn send(link: &mut Link) {
    for out in link.session.take_outgoing() {
        link.transport.send(out.to, &out.bytes, out.reliable);
    }
}

/// One tick of a link; false once the session is over.
fn step(game: &Game, link: &mut Link) -> bool {
    // SAFETY: the editor is live (it's ticking).
    if !unsafe { game.modes.is_editing(link.editor) } {
        send(link);
        return true;
    }
    send_edits(game, link);
    let target = Target {
        game,
        editor: link.editor,
        component: link.component,
    };
    for redo in changes::take_step_backs() {
        step_back(link, &target, redo);
    }
    link.session.tick(link.started.elapsed().as_millis() as u64);
    pump(&mut link.session, &mut link.transport);
    let mut alive = true;
    for peer in link.transport.take_lost() {
        match link.role {
            Role::Host => {
                coop_log!("lost member {peer}");
                let _ = link.session.lost(peer);
            }
            Role::Member => {
                coop_log!("lost the host");
                alive = false;
            }
        }
    }
    let mut requests = Requests::default();
    for event in link.session.take_events() {
        alive &= handle(link, &target, event, &mut requests);
    }
    if !requests.snapshots.is_empty() {
        answer(game, link, requests.snapshots);
    }
    if !requests.orders.is_empty() {
        send_orders(game, link, requests.orders);
    }
    match link.role {
        Role::Host if Instant::now() >= link.audit_at => {
            link.audit_at = Instant::now() + Duration::from_secs(audit::pace::SECONDS);
            audit(game, link);
        }
        Role::Member if drift::pressed() => drift::drift(&target),
        _ => {}
    }
    if link.session.is_loaded() {
        link.backups.tick(game, link.editor, link.component);
    }
    send(link);
    alive
}

/// The bricks `ops` change, to audit first (not a reorder's, which names many; not deleted ones).
fn bricks_of(ops: &[Op]) -> Vec<CoopId> {
    ops.iter()
        .flat_map(|op| match op {
            Op::Create { ids, .. } => ids.clone(),
            Op::Move { id, .. } | Op::Set { id, .. } | Op::Group { id, .. } => vec![*id],
            Op::Delete { .. } | Op::Order { .. } => Vec::new(),
        })
        .collect()
}

/// The coop ids of the editor's bricks, in object order.
fn ids_in_order(game: &Game, editor: Object) -> Vec<CoopId> {
    // SAFETY: the editor is live (it's ticking); game thread.
    unsafe { objects(game, editor) }
        .into_iter()
        .filter_map(changes::id_of)
        .collect()
}

/// Subhost: describes the editor to the members (`coop::audit`): the bricks edits touched lately
/// first, then the next ones going round the vehicle.
fn audit(game: &Game, link: &mut Link) {
    let touched = std::mem::take(&mut link.touched);
    if link.session.members().len() < 2 {
        return;
    }
    let ids = ids_in_order(game, link.editor);
    let bricks = link
        .rounds
        .pick(&ids, &touched, audit::pace::BRICKS)
        .into_iter()
        .filter_map(|id| {
            let object = changes::object(id)?;
            // SAFETY: objects with coop ids are live (deleted ones are forgotten as they go);
            // game thread.
            Some((id, unsafe { digest_of(game, object) }?))
        })
        .collect();
    let _ = link
        .session
        .audit(ids.len() as u32, audit::order_hash(ids), bricks);
}

/// Member: compares the editor with the subhost's `audit`. Bricks that differ ask for a snapshot,
/// an order that differs for the subhost's order. After two snapshots that didn't mend it, only
/// logs (once): asking again won't help.
fn audited(link: &mut Link, target: &Target, audit: &Audit) {
    // A dragged slider (or color wheel) changes bricks before its edit is finished and sent: left
    // to the next audit.
    if keyboard::is_mouse_button_down() {
        return;
    }
    let game = target.game;
    // SAFETY: the editor is live (it's ticking); game thread.
    let objects = unsafe { objects(game, link.editor) };
    let ids: Vec<CoopId> = objects.iter().filter_map(|&o| changes::id_of(o)).collect();
    let differences = audit.compare(objects.len() as u32, audit::order_hash(ids), |id| {
        let object = changes::object(id)?;
        // SAFETY: objects with coop ids are live; game thread.
        unsafe { digest_of(game, object) }
    });
    if differences.is_empty() {
        if link.audits_failed > 0 {
            coop_log!("audit: the vehicle matches the subhost's again");
            link.audits_failed = 0;
        }
        return;
    }
    if !differences.iter().any(|d| d.needs_snapshot()) {
        coop_log!(
            "audit: the bricks are in another object order than the subhost's: asked for its order"
        );
        link.session.request_order();
        return;
    }
    link.audits_failed += 1;
    match link.audits_failed {
        1 | 2 => {
            coop_log!(
                "audit: the vehicle differs from the subhost's: {}",
                describe(&differences)
            );
            link.session.request_resync();
        }
        3 => coop_log!(
            "audit: the vehicle still differs after 2 snapshots ({}): not asking again",
            describe(&differences)
        ),
        _ => {}
    }
}

/// `differences`, for the log (the first few).
fn describe(differences: &[Difference]) -> String {
    let mut text: Vec<String> = differences
        .iter()
        .take(5)
        .map(|difference| match difference {
            Difference::Count { subhost, here } => {
                format!("{here} objects here, {subhost} at the subhost")
            }
            Difference::Missing(id) => format!("{id:?} missing"),
            Difference::Place(id) => format!("{id:?} elsewhere"),
            Difference::Values(id) => format!("{id:?} with other property values"),
            Difference::Groups(id) => format!("{id:?} in other groups"),
            Difference::Order => "another object order".into(),
        })
        .collect();
    if differences.len() > text.len() {
        text.push(format!("{} more", differences.len() - text.len()));
    }
    text.join(", ")
}

/// Subhost: sends the members of `peers` the editor's object order.
fn send_orders(game: &Game, link: &mut Link, peers: Vec<PeerId>) {
    let ids = ids_in_order(game, link.editor);
    for peer in peers {
        coop_log!("sending the object order to {peer}");
        let _ = link.session.send_order(peer, ids.clone());
    }
}

/// Passes the finished local edits to the session, each change with what undoes it.
fn send_edits(game: &Game, link: &mut Link) {
    for edit in changes::take_edits() {
        let mut changes: Vec<Change> = Vec::new();
        // Deleted bricks come back as they were, with their ids, and where they were in the
        // object order.
        let order = changes::order();
        for buried in edit.buried {
            changes.push(Change {
                done: buried.ids.iter().map(|&id| Op::Delete { id }).collect(),
                undo: buried.undo,
            });
        }
        if let Some(ids) = ::coop::undo::restore_order(&order, &edit.deleted) {
            changes.push(Change {
                done: Vec::new(),
                undo: vec![Op::Order { ids }],
            });
        }
        if !edit.created.is_empty() {
            let created = match link.session.new_ids(edit.created.len()) {
                Ok(ids) => {
                    // SAFETY: the objects are the live editor's (destroyed ones were taken out of
                    // the edit); game thread.
                    match unsafe { game.archive.save(link.component, &edit.created) } {
                        Ok(archive) => {
                            // SAFETY: as above.
                            let extras = unsafe {
                                adopt(game, &edit.created, &ids);
                                creation_extras(game, &edit.created, &ids)
                            };
                            let undo = ids.iter().map(|&id| Op::Delete { id }).collect();
                            Some((Op::Create { ids, archive }, extras, undo))
                        }
                        Err(reason) => {
                            coop_log!("couldn't save {} new objects: {reason}", edit.created.len());
                            None
                        }
                    }
                }
                Err(error) => {
                    coop_log!("no coop ids for new objects: {error:?}");
                    None
                }
            };
            if let Some((create, extras, undo)) = created {
                changes.push(Change {
                    done: std::iter::once(create).chain(extras).collect(),
                    undo,
                });
            }
        }
        // Places are read now, not as of this edit: a later edit's moves go out early, which is
        // harmless (each move is the brick's whole place), and theirs then find nothing new.
        let moved = if edit.moved { moves(game) } else { Vec::new() };
        // The same for property values; a delete drops references to the deleted bricks.
        let sets = if edit.properties || !edit.deleted.is_empty() {
            property_sets(game)
        } else {
            Vec::new()
        };
        // And the object order.
        let order = if edit.ordered {
            // SAFETY: the editor is live (it's ticking); game thread.
            changes::reordered(&unsafe { objects(game, link.editor) })
        } else {
            None
        };
        // And groups.
        let regrouped = if edit.grouped {
            regroups(game)
        } else {
            Vec::new()
        };
        let summary = format!(
            "{} created, {} deleted, {} moved, {} properties set, {} regrouped{}",
            edit.created.len(),
            edit.deleted.len(),
            moved.len(),
            sets.len(),
            regrouped.len(),
            match &order {
                Some((ids, _)) => format!(", {} reordered", ids.len()),
                None => String::new(),
            }
        );
        changes.extend(moved);
        changes.extend(sets);
        changes.extend(order.map(|(ids, back)| Change {
            done: vec![Op::Order { ids }],
            undo: vec![Op::Order { ids: back }],
        }));
        changes.extend(regrouped);
        if changes.iter().all(|change| change.done.is_empty()) {
            continue;
        }
        if link.role == Role::Host {
            for change in &changes {
                link.touched.extend(bricks_of(&change.done));
            }
        }
        match link.session.edit_undoable(changes) {
            Ok(()) => coop_log!("sent an edit: {summary}"),
            Err(SessionError::NotLoaded) => {
                coop_log!("dropped an edit before the vehicle arrived: {summary}")
            }
            Err(error) => coop_log!("dropped an edit ({error:?}): {summary}"),
        }
    }
    changes::forget_graves();
}

/// Undoes (or redoes) the player's newest step, here and in the session.
fn step_back(link: &mut Link, target: &Target, redo: bool) {
    let what = if redo { "redo" } else { "undo" };
    let back = if redo {
        link.session.redo()
    } else {
        link.session.undo()
    };
    let back = match back {
        Ok(Some(back)) => back,
        Ok(None) => return coop_log!("nothing to {what}"),
        Err(error) => return coop_log!("can't {what} yet ({error:?})"),
    };
    let outcome = target.apply(&back.ops, false);
    if link.role == Role::Host {
        link.touched.extend(bricks_of(&back.ops));
    }
    coop_log!(
        "{what}: {} ops, {} changes skipped (someone else changed them since)",
        back.ops.len(),
        back.skipped
    );
    if outcome.refused > 0 {
        coop_log!("  {} ops refused as impossible here", outcome.refused);
    }
    if outcome.drifted {
        match link.role {
            Role::Member => link.session.request_resync(),
            Role::Host => coop_log!("  the subhost's editor drifted from the session"),
        }
    }
}

/// Does what the session asks; false if it ended.
fn handle(link: &mut Link, target: &Target, event: Event, requests: &mut Requests) -> bool {
    match event {
        Event::Joined { slot } => coop_log!("joined as slot {slot}"),
        Event::Refused(reason) => {
            coop_log!("refused: {reason:?}");
            return false;
        }
        Event::Load { ids, vehicle } => {
            if !link.loaded {
                backup::now(
                    target.game,
                    link.editor,
                    link.component,
                    "vehicle before joining",
                );
            }
            let outcome = target.load(&ids, &vehicle, !link.loaded);
            link.loaded = true;
            if outcome.drifted {
                link.bad_loads += 1;
                if link.bad_loads < 3 {
                    link.session.request_resync();
                } else {
                    coop_log!("the vehicle loads differently here every time: not asking again");
                }
            } else {
                link.bad_loads = 0;
            }
        }
        Event::Apply { author, ops } => {
            if link.role == Role::Host {
                link.touched.extend(bricks_of(&ops));
            }
            let outcome = target.apply(&ops, true);
            coop_log!("applied an edit of slot {author}: {} ops", ops.len());
            if outcome.refused > 0 {
                coop_log!("  {} ops refused as impossible here", outcome.refused);
            }
            if outcome.drifted {
                match link.role {
                    Role::Member => link.session.request_resync(),
                    Role::Host => coop_log!("  the subhost's editor drifted from the session"),
                }
            }
        }
        Event::SnapshotWanted(request) => requests.snapshots.push(request),
        Event::OrderWanted(peer) => requests.orders.push(peer),
        Event::Audit(audit) => audited(link, target, &audit),
        Event::Reorder(ids) => {
            let outcome = target.apply(&[Op::Order { ids }], false);
            coop_log!(
                "put the bricks in the subhost's object order{}",
                if outcome.drifted || outcome.refused > 0 {
                    ", but it didn't come out so"
                } else {
                    ""
                }
            );
        }
        Event::Members(members) => {
            let names: Vec<&str> = members.iter().map(|m| m.name.as_str()).collect();
            coop_log!("members: {}", names.join(", "));
        }
        Event::Drifted => coop_log!("drifted from the subhost: asked for a snapshot"),
        Event::UndoConflict => {
            coop_log!("an undo met someone else's edit of the same brick: asked for a snapshot")
        }
        Event::Ended(reason) => {
            coop_log!("session ended: {reason:?}");
            return false;
        }
        Event::BadMessage { peer, error } => coop_log!("unreadable message from {peer}: {error}"),
        Event::Flooded { peer, what } => {
            coop_log!("kicked {peer}: it sent far too much ({what:?}); it can't join again")
        }
        Event::Hello { .. } | Event::Selection { .. } => {}
    }
    true
}

/// Subhost: answers snapshot requests with the vehicle as it is now (every event applied, every
/// local edit sent).
fn answer(game: &Game, link: &mut Link, requests: Vec<SnapshotRequest>) {
    // SAFETY: the editor is live.
    let objects: Vec<Object> = unsafe { objects(game, link.editor) }
        .into_iter()
        .filter(|&object| changes::id_of(object).is_some())
        .collect();
    let ids = objects
        .iter()
        .filter_map(|&object| changes::id_of(object))
        .collect::<Vec<_>>();
    // SAFETY: the editor's live objects; game thread.
    let archive = match unsafe { game.archive.save(link.component, &objects) } {
        Ok(archive) => archive,
        Err(reason) => {
            coop_log!("couldn't save the vehicle for a member: {reason}");
            return;
        }
    };
    let groups = objects
        .iter()
        // SAFETY: as above.
        .map(|&object| unsafe { groups_of(game, object) })
        .collect::<Option<Vec<Groups>>>()
        .unwrap_or_default();
    let vehicle = Vehicle { archive, groups }.encode();
    for request in requests {
        coop_log!(
            "sending the vehicle to {}: {} objects, {} KB",
            request.peer,
            ids.len(),
            vehicle.len() / 1024
        );
        let _ = link
            .session
            .send_snapshot(request, ids.clone(), vehicle.clone());
    }
}
