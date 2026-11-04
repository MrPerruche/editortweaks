//! Coop building: several players editing one vehicle (`docs/re/coop/`). A beta.
//! What only testing needs (the local test, the probe) is in `coop-test` builds
//! only (`--coop-test`). The protocol and the session are the `coop` crate; this is the editor side.
//!
//! The coop page (`ui`; Editor tweaks, Coop... in the editor's top bar) says what the player wants,
//! kept across editors until it's done:
//! - Host: Steam friends playing Brick Rigs may ask to join the vehicle in the editor
//!   (`steam_link`); the player accepts or refuses each one on the page, told so by a HUD
//!   notification.
//! - Join a friend: the page lists the friends answering our hellos as hosting; the one picked is
//!   asked to let us in, and its vehicle then replaces ours (saved first, `backup`).
//! - With the local test setting ([`local_test`], test builds), both link two game instances on one PC over
//!   localhost instead (`local`).
//!
//! A session lasts until its player stops it or the match changes (another map or server, the
//! match restarting): leaving the editor (to test the vehicle, say) keeps it going, pumped by a
//! thread timer while no editor ticks ([`away_tick`]). What needs the editor waits; back in an
//! editor, the session's vehicle is found in it again ([`Here`], `apply::Target::refit`) and what
//! others did meanwhile is made. A member finding another vehicle there gets the session's anew; a
//! subhost is asked whether to share the one there instead (`Session::rehost`).
//!
//! Creating, deleting and moving bricks, changing their properties (those
//! `game::property_values` can sync), reordering them (seat order, camera order, ...) and grouping
//! them (editor and weld groups) then shows in every editor. Each role logs to its own file
//! (`log`). And, in test builds, outside of Steam's modes, a check at each start that Steam's
//! messages work (`probe`).
//!
//! Undo and redo undo the player's own edits only (`coop::undo`): each edit goes to the session
//! with what undoes it, worked out from what `changes` remembered just before (and deleted bricks
//! saved as they go). The game's own undo, which loads an older whole vehicle, doesn't run while
//! linked (`game::undo_redo`).
//!
//! Every few seconds the subhost describes its editor in short (`coop::audit`: the brick count, the
//! object order, digests of some bricks going round the vehicle, those edited lately first). A
//! member at the same commit with nothing of its own on the way compares its editor: bricks that
//! differ ask for a snapshot, an order that differs asks for the subhost's order.
//!
//! Each editor tick, while the editor is in its default mode (not moving or placing bricks):
//! finished local edits go to the session, then what arrived is applied, then snapshot (and
//! order) requests are answered, so a snapshot always holds exactly what was committed; then the
//! subhost audits. In other modes coop only sends: reading would mean editing bricks the player is
//! moving.

mod apply;
mod backup;
mod cameras;
mod changes;
mod checks;
mod dialogs;
mod local;
mod log;
mod notices;
mod presence;
#[cfg(feature = "coop-test")]
mod probe;
mod status_box;
mod steam_link;
mod ui;

pub use self::backup::AUTOSAVE_FOLDER;

use std::cell::{Cell, RefCell};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ::coop::audit::{self, Audit, Rounds};
use ::coop::ids::Slot;
use ::coop::tcp::TcpTransport;
use ::coop::transport::{Transport, pump};
use ::coop::vehicle::Vehicle;
use ::coop::{
    Change, CoopId, Difference, Digest, EndReason, Event, GroupKind, Groups, HostSettings, Member,
    Op, PeerId, PeerInfo, RefuseReason, Session, SessionError, SnapshotRequest, Transform, Value,
};
use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::tshared::FWeakObjectPtr;

use self::apply::Target;
use self::changes::{Buried, Object};
use self::local::Role;
use self::log::coop_log;
use self::steam_link::{Closed, SteamTransport};
use crate::features::editor_menu;
use crate::features::keybinds::{COOP_MENU, Keybind};
use crate::features::settings_menu;
use crate::framework::settings::{Count, Section, Setting, Toggle};
use crate::framework::timings;
use crate::game::brick_editor::{self, BrickEditor};
use crate::game::brick_groups::{self, BrickGroups};
use crate::game::chat::{self, Chat, Tone};
use crate::game::editor_mode::{self, EditorModes};
use crate::game::editor_object::SpawnTransforms;
use crate::game::editor_view::{self, Bricks};
use crate::game::input_sources::{self, InputSources};
use crate::game::object_order::{self, ObjectOrder};
use crate::game::property_values::{self, BrickValues};
use crate::game::ugc_autosave;
use crate::game::undo_redo;
use crate::game::vehicle_archive::{self, VehicleArchive};
use crate::game::vehicle_changes;
use crate::game::weak_object::{self, WeakObject};
use crate::game::zoom_notification::{self, ZoomNotification};
use crate::platform::keyboard;
use crate::platform::timer::ThreadTimer;
use crate::prelude::*;

pub struct Coop;

pub static LOCAL_TEST: Toggle = Toggle::new(
    "CoopLocalTest",
    "Coop local test",
    "Test builds only: links this game with another one on this PC instead of Steam friends.",
    false,
);

pub static REQUEST_OPENS_PAGE: Toggle = Toggle::new(
    "CoopRequestOpensPage",
    "Open page on join requests",
    "Someone asking to join opens the coop page. Off: only the message shows.",
    true,
);

/// Most players in a session we host, us included (`questions.md` Q8). Its index + 2.
pub static MAX_PLAYERS: Count = Count::new(
    "CoopMaxPlayers",
    "Most players",
    "How many can build in your vehicle, you included. Above 4 may be unstable.",
    2,
    32,
    4,
);

/// The feature's settings, in the options' Editor tweaks menu (the local test in test builds only).
pub static SETTINGS: [Section; 1] = [Section {
    title: "Coop building (beta)",
    settings: &[
        Setting::Count(&MAX_PLAYERS),
        Setting::Toggle(&REQUEST_OPENS_PAGE),
        Setting::Toggle(&cameras::THROUGH_BRICKS),
        #[cfg(feature = "coop-test")]
        Setting::Toggle(&LOCAL_TEST),
    ],
    unlisted: &[],
}];

/// Most players in a session we host, us included ([`MAX_PLAYERS`]).
fn max_players() -> usize {
    MAX_PLAYERS.get() as usize
}

/// Whether coop links two games on this PC instead of Steam friends: test builds only.
fn local_test() -> bool {
    cfg!(feature = "coop-test") && LOCAL_TEST.get()
}

/// Joining: how long friends hosting are looked for once the coop page showed, how often friends
/// playing are greeted again, how long a host stays listed after its last answer, and how long its
/// player has to answer.
const LOOK_FOR: Duration = Duration::from_secs(120);
/// How long an invite shows on the coop page, and a friend shows as invited.
const INVITE_FOR: Duration = Duration::from_secs(300);
const HELLO_EVERY: Duration = Duration::from_secs(15);
const FOUND_FOR: Duration = Duration::from_secs(40);
const ANSWER_WAIT: Duration = Duration::from_secs(180);
/// How often a join request's notification shows again while it waits (they fade by themselves).
const ASKING_EVERY: Duration = Duration::from_secs(4);
/// No editor ticked for this long (or it's gone): the player left the editor.
const AWAY_AFTER: Duration = Duration::from_secs(2);
/// Back in an editor, how long the session's vehicle is looked for in it (an editor may load it a
/// little after it opens) before it's taken for another.
const REFIT_WAIT: Duration = Duration::from_secs(3);
/// How often a subhost's editor holding another vehicle is looked at again for the session's.
const LOOK_AGAIN: Duration = Duration::from_secs(1);
/// How often the thread timer ticks the link while no editor does.
const AWAY_TICK_MS: u32 = 100;

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
    /// The HUD's notifications, to tell the player what coop does (or only the log, if not found).
    notes: Option<ZoomNotification>,
    /// The game's chat, to say who came or went (or why not: then the notifications say it).
    chat: Result<Chat, String>,
    /// What shows others' selections (or why not found: then they don't show).
    presence: Result<presence::Kit, String>,
    /// What shows others' cameras (or why not found: then they don't show, nor ours go out).
    cameras: Result<cameras::Kit, String>,
}

static GAME: OnceLock<Result<Game, String>> = OnceLock::new();

/// Whether coop took over the game's undo and redo (or why not).
static UNDO: OnceLock<Result<(), String>> = OnceLock::new();

fn game() -> Option<&'static Game> {
    let found = GAME.get_or_init(|| {
        let _span = timings::span("coop::game");
        let archive = vehicle_archive::find()?;
        coop_log!("vehicle archive: {}", archive.layout());
        if let Err(reason) = dialogs::start() {
            coop_log!("coop asks on its own pages, not in the game's popups: {reason}");
        }
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
            presence: presence::find().inspect_err(|reason| {
                coop_log!("others' selections won't show: {reason}");
            }),
            cameras: cameras::find().inspect_err(|reason| {
                coop_log!("others' cameras won't show: {reason}");
            }),
            chat: chat::find().inspect_err(|reason| {
                coop_log!("coop's news won't show in the chat: {reason}");
            }),
            notes: zoom_notification::code()
                .and_then(zoom_notification::find)
                .inspect_err(|reason| {
                    coop_log!("coop's notifications only go to the log: {reason}")
                })
                .ok(),
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
        log!("built in (protocol {}), beta", ::coop::message::PROTOCOL);
        vehicle_changes::on_destroyed(changes::destroyed)?;
        let undo = UNDO.get_or_init(|| undo_redo::take_over(changes::step_back_asked));
        if let Err(reason) = undo {
            log!("can't take over undo ({reason}): in coop it would load older vehicles");
        }
        ui::add_pages();
        editor_menu::on_click(&COOP_MENU, || PAGE_ASKED.store(true, Ordering::Relaxed));
        if let Err(reason) = brick_editor::before_end_play(editor_ending) {
            log!("the exiting coop autosave won't be written as the editor goes: {reason}");
        }
        brick_editor::after_tick(after_tick)
    }

    fn on_engine_init(&self) {
        if let Err(reason) = ugc_autosave::treat_as_autosave(backup::AUTOSAVE_FOLDER) {
            log!("the coop autosave won't be treated as an autosave: {reason}");
        }
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

/// How a link reaches its peers.
enum Wire {
    /// Another game on this PC (`local`).
    Local(TcpTransport),
    Steam(SteamTransport),
}

impl Wire {
    /// Peers whose connection closed since the last call.
    fn take_lost(&mut self) -> Vec<Closed> {
        match self {
            // A socket doesn't say why; a player leaving says goodbye first ([`network`]).
            Wire::Local(tcp) => tcp
                .take_lost()
                .into_iter()
                .map(|peer| Closed {
                    peer,
                    by_peer: false,
                })
                .collect(),
            Wire::Steam(steam) => steam.take_lost(),
        }
    }

    /// Logs how Steam reaches `peer` (relayed, through which relay), to see it in players' logs.
    fn log_connection(&self, peer: PeerId) {
        if let Wire::Steam(steam) = self {
            steam.log_connection(peer);
        }
    }

    /// What to call `peer`: Steam's name for a friend, else its name in our match, else what it
    /// called itself (else its SteamID).
    fn name_of(&mut self, peer: PeerId, called: &str) -> String {
        match self {
            Wire::Steam(steam) => steam.name_of(peer).unwrap_or_else(|| {
                if called.is_empty() {
                    peer.to_string()
                } else {
                    called.to_owned()
                }
            }),
            Wire::Local(_) => called.to_owned(),
        }
    }
}

impl Transport for Wire {
    fn send(&mut self, to: PeerId, bytes: &[u8], reliable: bool) {
        match self {
            Wire::Local(tcp) => tcp.send(to, bytes, reliable),
            Wire::Steam(steam) => steam.send(to, bytes, reliable),
        }
    }

    fn receive(&mut self) -> Vec<(PeerId, Vec<u8>)> {
        match self {
            Wire::Local(tcp) => tcp.receive(),
            Wire::Steam(steam) => steam.receive(),
        }
    }
}

/// A session with other games, in one editor.
struct Link {
    session: Session,
    transport: Wire,
    role: Role,
    /// Member: the subhost's name, and whether it let us in.
    host_name: String,
    joined: bool,
    /// Subhost: who asks to join (oldest first, answered in turn), and when their notification
    /// last showed.
    asking: Vec<(PeerId, String)>,
    asking_shown: Option<Instant>,
    /// Subhost: answers to them given on the coop page, for the next tick.
    answers: Vec<(PeerId, bool)>,
    /// Our slot once in, and every member (us too).
    own_slot: Option<Slot>,
    members: Vec<Member>,
    /// Subhost: members whose going the player was told of (a ban, a lost connection), not to be
    /// told again as they leave the list.
    told_gone: Vec<PeerId>,
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
    /// The session's vehicle in the game's vehicle list: as it started and as it's left.
    autosave: backup::AutoSave,
    /// Member: how much of the game's time goes to what the subhost sends (see [`overloaded`]).
    workload: ::coop::limits::Workload,
    /// Others' selections, and sending ours.
    presence: presence::Shown,
    /// Others' cameras, and sending ours.
    cameras: cameras::Cameras,
    /// Whether the editor holds the session's vehicle now.
    here: Here,
    /// While it doesn't: what arrived for the editor, in order, made once it does.
    missed: Vec<Event>,
    /// `editor`, to see it's gone (`None` if the game gave it no serial number).
    editor_pointer: Option<FWeakObjectPtr>,
    /// The match the session started in (its game state; `None`: unknown): it ends with it.
    in_match: Option<WeakObject>,
    /// Subhost: who was let in, let in again without asking (their game restarted, say), unless
    /// kicked or banned since.
    accepted: Vec<PeerId>,
    /// Member: the vehicle here isn't the session's, which was asked for: edits wait for it.
    resyncing: bool,
    /// Subhost: the session's vehicle as the editor last closed on it ([`editor_closing`]), to
    /// bring back if the player returns to another vehicle or none ([`resume`]).
    kept: Option<Kept>,
}

/// A copy of the session's vehicle: its bricks' coop ids, in object order, and the vehicle (a
/// [`Vehicle`], as a snapshot sends it).
struct Kept {
    ids: Vec<CoopId>,
    vehicle: Vec<u8>,
}

/// Whether the editor holds the session's vehicle.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Here {
    Editing,
    /// Out of the editor (testing the vehicle, say): the session goes on, ticked by the thread
    /// timer. Whether the editor held the session's vehicle when the player left it.
    Away(bool),
    /// In another editor since then: the session's vehicle is looked for in it ([`refit`]).
    Back(Instant),
    /// Subhost: in an editor holding another vehicle, until its player brings the session's back
    /// ([`resume`]), shares this one or stops hosting, or the session's turns up there (looked for
    /// again after the time held).
    Elsewhere(Instant),
}

/// What members asked the subhost for, answered once every event is applied.
#[derive(Default)]
struct Requests {
    snapshots: Vec<SnapshotRequest>,
    orders: Vec<PeerId>,
}

/// Looking for hosts: hellos to friends playing and the players of our match, noting those that
/// answer as hosting.
struct Seek {
    session: Session,
    transport: SteamTransport,
    editor: Object,
    hello_at: Instant,
    /// Players that answered as hosting, with when they last did.
    found: Vec<(ui::Host, Instant)>,
    /// How many friends play Brick Rigs, and how many others are in our match, as last greeted.
    friends_playing: usize,
    server_players: usize,
    /// Friends hosting that invited us, with when.
    invites: Vec<(PeerId, String, Instant)>,
}

enum State {
    Idle,
    Seeking(Box<Seek>),
    Linked(Box<Link>),
}

/// What the player asked for on the coop page, kept across editors until it's done or fails.
#[derive(Clone, PartialEq)]
enum Want {
    Nothing,
    Host,
    /// Join this host (`None`: the local test's other game), called this.
    Join(Option<PeerId>, String),
}

/// Coop on the game thread: what the player wants, and how far it got.
struct Driver {
    state: State,
    want: Want,
    /// Until when friends hosting are looked for (the coop page showed lately).
    look_until: Option<Instant>,
    /// Who our player banned: sessions we host later in the match refuse them too.
    bans: Vec<BanEntry>,
    /// Hosting over Steam: friends playing, and who we invited when.
    friends: Vec<(PeerId, String)>,
    invited: Vec<(PeerId, Instant)>,
    /// Why invites can't arrive (Steam unusable), once told.
    no_inbox: bool,
}

/// Someone our player banned from coop (the game's ban dialog), until the time they chose or the
/// match ends.
struct BanEntry {
    peer: PeerId,
    name: String,
    reason: String,
    /// `None`: until the match ends.
    until: Option<Instant>,
    /// The match it was made in (`None`: unknown, then it lasts until the game closes).
    in_match: Option<WeakObject>,
}

impl BanEntry {
    fn over(&self, now: Instant) -> bool {
        self.until.is_some_and(|until| until <= now) || self.in_match.is_some_and(match_over)
    }

    /// When it ends on `link`'s session clock.
    fn until(&self, link: &Link) -> u64 {
        self.until
            .map_or(u64::MAX, |until| session_time(link, until))
    }
}

/// Whether the match `ours` (its game state) is over: gone, or not the current one.
fn match_over(ours: WeakObject) -> bool {
    let current = dialogs::current_match().and_then(|current| current.get());
    ours.get().is_none() || ours.get() != current
}

/// How a link starts.
enum Opening {
    /// Over localhost (`local`): hosting, or joining the other game.
    LocalHost,
    LocalJoin,
    /// Hosting for friends.
    Host,
    /// Joining `host`, greeted by a [`Seek`].
    Join(Box<Seek>, PeerId),
}

/// The coop page was asked for from the editor's menu.
static PAGE_ASKED: AtomicBool = AtomicBool::new(false);
/// The subhost's player is to be asked what to do with the editor's vehicle, not the session's
/// (outside the driver).
static SHARE_ASKED: AtomicBool = AtomicBool::new(false);

thread_local! {
    // The game thread's: the editor ticks there.
    static DRIVER: RefCell<Driver> = const {
        RefCell::new(Driver {
            state: State::Idle,
            want: Want::Nothing,
            look_until: None,
            bans: Vec::new(),
            friends: Vec::new(),
            invited: Vec::new(),
            no_inbox: false,
        })
    };
    /// When an editor last ticked, to see the player left it.
    static EDITOR_TICKED: Cell<Option<Instant>> = const { Cell::new(None) };
    /// Ticks the link while no editor does ([`away_tick`]).
    static AWAY_TIMER: RefCell<Option<ThreadTimer>> = const { RefCell::new(None) };
}

fn after_tick(editor: *mut UObjectBase, _delta: f32) {
    static PAGE_KEY: AtomicBool = AtomicBool::new(false);
    let page_key = went_down(&COOP_MENU, &PAGE_KEY);
    let Some(game) = game() else { return };
    EDITOR_TICKED.set(Some(Instant::now()));
    AWAY_TIMER.with_borrow_mut(|timer| {
        if timer.is_none() {
            *timer = ThreadTimer::start(AWAY_TICK_MS, away_tick);
            if timer.is_none() {
                coop_log!("no thread timer: sessions pause while out of the editor");
            }
        }
    });
    dialogs::tick();
    let status = DRIVER.with_borrow_mut(|driver| {
        for request in ui::take_requests() {
            driver.request(game, editor, request);
        }
        driver.tick(game, editor);
        driver.status()
    });
    // While asking to join: a popup to stop waiting.
    let waiting = match &status.doing {
        ui::Doing::Asking(host) => Some(format!("Waiting for {host} to let you in...")),
        _ => None,
    };
    if dialogs::waiting(waiting.as_deref()) {
        ui::request(ui::Request::Leave);
    }
    if SHARE_ASKED.swap(false, Ordering::Relaxed) {
        ui::ask_back(status.kept);
    }
    notices::show(game);
    status_box::show(editor, &status);
    // Outside the driver: a page shown again reads the status only.
    ui::update(status);
    // A page showing is made anew as the status changed.
    if (PAGE_ASKED.swap(false, Ordering::Relaxed) || page_key) && !settings_menu::page_showing() {
        settings_menu::open_page(&ui::COOP);
    }
}

/// `editor` is about to go (the player left it, the map changed, the game is closing): the last
/// moment its vehicle can be read.
fn editor_ending(editor: *mut UObjectBase) {
    let Some(Ok(game)) = GAME.get() else { return };
    DRIVER.with(|driver| match driver.try_borrow_mut() {
        Ok(mut driver) => driver.editor_ending(game, editor),
        Err(_) => coop_log!("the editor went inside coop's own tick: no exiting coop autosave"),
    });
}

/// The thread timer's tick (on the game thread, about every frame): while no editor ticks, the
/// link goes on without one.
fn away_tick() {
    let Some(Ok(game)) = GAME.get() else { return };
    let editor_ticks = EDITOR_TICKED
        .get()
        .is_some_and(|at| at.elapsed() < AWAY_AFTER);
    let ticked = DRIVER.with(|driver| {
        // Never inside coop's own tick (should the game run timers while coop calls it).
        let Ok(mut driver) = driver.try_borrow_mut() else {
            return false;
        };
        driver.away_tick(game, editor_ticks)
    });
    if ticked {
        notices::show(game);
    }
}

impl Driver {
    /// See [`editor_ending`]. The session itself goes on, unless the game is closing: once out of
    /// the editor, this copy is the last there can be.
    fn editor_ending(&mut self, game: &Game, editor: Object) {
        if let State::Linked(link) = &mut self.state
            && link.editor == editor
            && link.here == Here::Editing
        {
            link.autosave.finish(game, link.editor, link.component);
        }
    }

    /// Does what the player clicked on the coop page.
    fn request(&mut self, game: &Game, editor: Object, request: ui::Request) {
        match request {
            ui::Request::Host if self.want == Want::Nothing => self.want = Want::Host,
            ui::Request::Join(peer, name) if self.want == Want::Nothing => {
                self.want = Want::Join(peer, name)
            }
            ui::Request::Leave => {
                if let State::Linked(link) = &mut self.state {
                    if link.editor == editor && link.here == Here::Editing {
                        link.autosave.finish(game, link.editor, link.component);
                    }
                    stop(link, "the player left");
                    self.state = State::Idle;
                }
                self.want = Want::Nothing;
            }
            ui::Request::Answer(peer, yes) => {
                if let State::Linked(link) = &mut self.state {
                    link.answers.push((peer, yes));
                }
            }
            ui::Request::Kick(peer) => {
                if let State::Linked(link) = &mut self.state
                    && link.role == Role::Host
                {
                    let _ = link.session.kick(peer);
                    link.accepted.retain(|p| *p != peer);
                }
            }
            ui::Request::OpenBan(peer, name) => {
                // SAFETY: the live editor ticking, on the game thread; its controller is the
                // local one.
                unsafe { dialogs::open_ban(game.editor.player_controller(editor), peer, &name) };
            }
            ui::Request::Ban {
                peer,
                name,
                reason,
                seconds,
            } => {
                let ban = BanEntry {
                    peer,
                    name: name.clone(),
                    reason,
                    until: (seconds > 0).then(|| Instant::now() + Duration::from_secs(seconds)),
                    in_match: dialogs::current_match(),
                };
                if let State::Linked(link) = &mut self.state
                    && link.role == Role::Host
                {
                    let _ = link.session.ban(peer, ban.until(link), &ban.reason);
                    link.accepted.retain(|p| *p != peer);
                    link.told_gone.push(peer);
                }
                let how_long = match seconds {
                    0 => "for this match".to_owned(),
                    1..120 => format!("for {seconds} seconds"),
                    _ => format!("for {} minutes", seconds.div_ceil(60)),
                };
                coop_log!("banned {name} ({peer}) {how_long}: \"{}\"", ban.reason);
                notices::say(Tone::Bad, &format!("Coop: {name} is banned {how_long}"));
                self.bans.retain(|ban| ban.peer != peer);
                self.bans.push(ban);
            }
            ui::Request::Unban(peer) => {
                if let Some(ban) = self.bans.iter().find(|ban| ban.peer == peer) {
                    notices::say(Tone::Good, &format!("Coop: {} is unbanned", ban.name));
                }
                self.bans.retain(|ban| ban.peer != peer);
                if let State::Linked(link) = &mut self.state
                    && link.role == Role::Host
                {
                    link.session.unban(peer);
                }
            }
            ui::Request::Invite(peer, name) => {
                let State::Linked(link) = &mut self.state else {
                    return;
                };
                if !matches!(link.transport, Wire::Steam(_)) {
                    coop_log!("invites need Steam: {name} not invited");
                    return;
                }
                let own = link
                    .members
                    .iter()
                    .find(|m| Some(m.slot) == link.own_slot)
                    .map(|m| m.name.clone())
                    .unwrap_or_default();
                if link.session.invite(peer, own).is_ok() {
                    coop_log!("invited {name} ({peer})");
                    self.invited.retain(|(p, _)| *p != peer);
                    self.invited.push((peer, Instant::now()));
                }
            }
            ui::Request::Look => self.look_until = Some(Instant::now() + LOOK_FOR),
            ui::Request::Share | ui::Request::Resume => {
                if let State::Linked(link) = &mut self.state
                    && link.role == Role::Host
                    && matches!(link.here, Here::Elsewhere(_))
                    && link.editor == editor
                {
                    match request {
                        ui::Request::Share => share(game, link),
                        _ => resume(game, link),
                    }
                }
            }
            ui::Request::Host | ui::Request::Join(..) => {
                coop_log!("the coop page asked for something else while busy: ignored")
            }
        }
    }

    /// Links, looks for hosts or steps the link, as the player wants.
    fn tick(&mut self, game: &Game, editor: Object) {
        let local = local_test();
        // Test builds, without Steam's use: a check that its messages work (the same channel).
        #[cfg(feature = "coop-test")]
        if local || (self.want == Want::Nothing && !matches!(self.state, State::Seeking(_))) {
            probe::tick(info);
        }
        self.end_with_the_match();
        match &mut self.state {
            // Back in the editor (or another one): the link goes on there.
            State::Linked(link) if link.editor != editor || matches!(link.here, Here::Away(_)) => {
                arrive(game, link, editor)
            }
            // Another editor: looking for hosts starts again there.
            State::Seeking(seek) if seek.editor != editor => {
                coop_log!("stopped looking for hosts: the editor changed");
                self.state = State::Idle;
            }
            _ => {}
        }
        match self.want.clone() {
            Want::Host if !matches!(self.state, State::Linked(_)) => {
                self.state = State::Idle;
                let opening = if local {
                    Opening::LocalHost
                } else {
                    Opening::Host
                };
                self.link(game, editor, opening);
            }
            Want::Join(peer, _) if !matches!(self.state, State::Linked(_)) => {
                let opening = match peer {
                    None => Opening::LocalJoin,
                    Some(host) => {
                        let seek = match std::mem::replace(&mut self.state, State::Idle) {
                            State::Seeking(seek) => Some(seek),
                            _ => look_for_hosts(editor),
                        };
                        match seek {
                            Some(seek) => Opening::Join(seek, host),
                            None => {
                                self.want = Want::Nothing;
                                return;
                            }
                        }
                    }
                };
                self.link(game, editor, opening);
            }
            Want::Nothing => {
                let look = !local && self.look_until.is_some_and(|until| Instant::now() < until);
                match &self.state {
                    // Alone in the editor, friends' invites may come: the link listens, only
                    // greeting friends while the player looks for hosts.
                    State::Idle if !local && (look || !self.no_inbox) => {
                        match look_for_hosts(editor) {
                            Some(seek) => self.state = State::Seeking(seek),
                            // Told why: not again until the page shows again.
                            None => {
                                self.look_until = None;
                                self.no_inbox = true;
                            }
                        }
                    }
                    State::Seeking(_) if local => self.state = State::Idle,
                    _ => {}
                }
            }
            _ => {}
        }
        match &mut self.state {
            State::Seeking(seek) => {
                let look = self.look_until.is_some_and(|until| Instant::now() < until);
                seek_tick(seek, look);
            }
            State::Linked(link) => {
                // Players of our match may ask to join, or answer.
                if let Wire::Steam(steam) = &mut link.transport {
                    steam.server(dialogs::match_players);
                }
                self.friends = match &mut link.transport {
                    Wire::Steam(steam) if link.role == Role::Host => steam
                        .friends()
                        .iter()
                        .map(|friend| (friend.id, friend.name.clone()))
                        .collect(),
                    _ => Vec::new(),
                };
                if !step(game, link) {
                    if link.editor == editor && link.here == Here::Editing {
                        link.autosave.finish(game, link.editor, link.component);
                    }
                    stop(link, "the session ended");
                    self.state = State::Idle;
                    self.want = Want::Nothing;
                }
            }
            State::Idle => {}
        }
    }

    /// While no editor ticks ([`away_tick`]; `editor_ticks`: one did lately): the link goes on
    /// without it. Whether it ticked.
    fn away_tick(&mut self, game: &Game, editor_ticks: bool) -> bool {
        let _span = timings::span("coop::away_tick");
        self.end_with_the_match();
        let State::Linked(link) = &mut self.state else {
            return false;
        };
        let gone = link
            .editor_pointer
            .is_some_and(|pointer| weak_object::resolve(pointer).is_none());
        if editor_ticks && !gone {
            return false;
        }
        if !matches!(link.here, Here::Away(_)) {
            go_away(game, link);
        }
        if !away_step(link) {
            stop(link, "the session ended");
            self.state = State::Idle;
            self.want = Want::Nothing;
        }
        true
    }

    /// Ends the link if the match it started in is over (another map or server, a restart).
    fn end_with_the_match(&mut self) {
        let State::Linked(link) = &mut self.state else {
            return;
        };
        if !link.in_match.is_some_and(match_over) {
            return;
        }
        let text = match link.role {
            Role::Host => "Coop: stopped hosting: the match changed".to_owned(),
            Role::Member => format!("Coop: left {}'s session: the match changed", link.host_name),
        };
        stop(link, "the match changed");
        notices::alert(&text);
        self.state = State::Idle;
        self.want = Want::Nothing;
    }

    /// Starts a link the player wants; if it can't start, they're told why and it's given up.
    fn link(&mut self, game: &Game, editor: Object, opening: Opening) {
        match start(game, editor, opening) {
            Some(mut link) => {
                // Bans still running hold in the new session too.
                let now = Instant::now();
                self.bans.retain(|ban| !ban.over(now));
                if link.role == Role::Host {
                    for ban in &self.bans {
                        let _ = link.session.ban(ban.peer, ban.until(&link), &ban.reason);
                    }
                }
                self.state = State::Linked(link)
            }
            None => self.want = Want::Nothing,
        }
    }

    /// What the coop page shows.
    fn status(&self) -> ui::Status {
        let (doing, asking) = match &self.state {
            State::Linked(link) => match link.role {
                Role::Host => (ui::Doing::Hosting, link.asking.clone()),
                Role::Member if link.joined => {
                    (ui::Doing::Member(link.host_name.clone()), Vec::new())
                }
                Role::Member => (ui::Doing::Asking(link.host_name.clone()), Vec::new()),
            },
            _ => match &self.want {
                Want::Join(_, name) => (ui::Doing::Asking(name.clone()), Vec::new()),
                _ => (ui::Doing::Alone, Vec::new()),
            },
        };
        let hosts = if local_test() {
            vec![ui::Host {
                peer: None,
                name: "the other game on this PC".into(),
                friend: true,
                unfit: None,
            }]
        } else if let State::Seeking(seek) = &self.state {
            seek.found.iter().map(|(host, _)| host.clone()).collect()
        } else {
            Vec::new()
        };
        let players = match &self.state {
            State::Linked(link) if link.role == Role::Host || link.joined => players(link),
            _ => Vec::new(),
        };
        let now = Instant::now();
        let bans = self
            .bans
            .iter()
            .filter(|ban| !ban.over(now))
            .map(|ban| ui::Banned {
                peer: ban.peer,
                name: ban.name.clone(),
                reason: ban.reason.clone(),
                minutes: ban.until.map(|until| {
                    until
                        .saturating_duration_since(now)
                        .as_secs()
                        .div_ceil(60)
                        .max(1)
                }),
            })
            .collect();
        let local = match &self.state {
            State::Linked(link) => matches!(link.transport, Wire::Local(_)),
            _ => local_test(),
        };
        ui::Status {
            doing,
            elsewhere: matches!(&self.state, State::Linked(link) if matches!(link.here, Here::Elsewhere(_))),
            kept: matches!(&self.state, State::Linked(link) if link.kept.is_some()),
            local,
            players,
            asking,
            hosts,
            friends_playing: match &self.state {
                State::Seeking(seek) => seek.friends_playing,
                _ => 0,
            },
            server_players: match &self.state {
                State::Seeking(seek) => seek.server_players,
                _ => 0,
            },
            looking: matches!(self.state, State::Seeking(_))
                && self.look_until.is_some_and(|until| Instant::now() < until),
            bans,
            friends: self
                .friends
                .iter()
                .map(|(peer, name)| ui::Friend {
                    peer: *peer,
                    name: name.clone(),
                    invited: self
                        .invited
                        .iter()
                        .any(|(p, at)| p == peer && at.elapsed() < INVITE_FOR),
                })
                .collect(),
            invites: match &self.state {
                State::Seeking(seek) => seek
                    .invites
                    .iter()
                    .filter(|(_, _, at)| at.elapsed() < INVITE_FOR)
                    .map(|(peer, name, _)| (*peer, name.clone()))
                    .collect(),
                _ => Vec::new(),
            },
        }
    }
}

/// Everyone in `link`'s session with their colours, the subhost first.
fn players(link: &Link) -> Vec<ui::Player> {
    let slots: Vec<Slot> = link.members.iter().map(|m| m.slot).collect();
    let mut players: Vec<ui::Player> = link
        .members
        .iter()
        .map(|m| ui::Player {
            peer: m.peer,
            name: m.name.clone(),
            color: ::coop::presence::color_of(&slots, m.slot).0,
            host: m.slot == ::coop::ids::SUBHOST_SLOT,
            you: Some(m.slot) == link.own_slot,
        })
        .collect();
    players.sort_by_key(|player| !player.host);
    players
}

/// `at` on the session's clock ([`Session::tick`]).
fn session_time(link: &Link, at: Instant) -> u64 {
    at.saturating_duration_since(link.started).as_millis() as u64
}

/// Whether `key` went down since the last call with the same `held`.
fn went_down(key: &Keybind, held: &AtomicBool) -> bool {
    let now = key.is_held();
    now && !held.swap(now, Ordering::Relaxed)
}

/// Starts looking for friends hosting (or tells the player why not).
fn look_for_hosts(editor: Object) -> Option<Box<Seek>> {
    let _span = timings::span("coop::look_for_hosts");
    log::start(Role::Member.name());
    let transport = match SteamTransport::new(true) {
        Ok(transport) => transport,
        Err(reason) => {
            coop_log!("can't use Steam: {reason}");
            notices::alert("Coop needs Steam to find friends' games.");
            return None;
        }
    };
    coop_log!("Steam: direct connections disabled, relay only; looking for hosts");
    Some(Box::new(Seek {
        session: Session::new(info()),
        transport,
        editor,
        hello_at: Instant::now(),
        found: Vec::new(),
        friends_playing: 0,
        server_players: 0,
        invites: Vec::new(),
    }))
}

/// Notes friends' invites; while `greet`, greets friends playing and the players of our match now
/// and then, and notes those answering as hosting.
fn seek_tick(seek: &mut Seek, greet: bool) {
    let server: Vec<PeerId> = seek
        .transport
        .server(dialogs::match_players)
        .iter()
        .map(|(id, _)| *id)
        .collect();
    let friends: Vec<PeerId> = seek.transport.friends().iter().map(|f| f.id).collect();
    seek.friends_playing = friends.len();
    seek.server_players = server.len();
    if greet && Instant::now() >= seek.hello_at {
        seek.hello_at = Instant::now() + HELLO_EVERY;
        let mut peers = friends;
        for id in server {
            if !peers.contains(&id) {
                peers.push(id);
            }
        }
        for peer in peers {
            seek.session.hello(peer);
        }
    }
    pump(&mut seek.session, &mut seek.transport);
    let ours = ::coop::message::PROTOCOL;
    let own_game = info().game_version;
    for event in seek.session.take_events() {
        match event {
            Event::Hello { peer, info } => {
                seek.found.retain(|(host, _)| host.peer != Some(peer));
                if !info.hosting {
                    continue;
                }
                let name = seek
                    .transport
                    .name_of(peer)
                    .unwrap_or_else(|| peer.to_string());
                let unfit = if !(info.protocol.0..=info.protocol.1).contains(&ours) {
                    Some(format!("Editor tweaks {}", info.mod_version))
                } else if info.game_version != own_game {
                    Some(format!("game {}", info.game_version))
                } else {
                    None
                };
                coop_log!(
                    "{name} hosts{}",
                    unfit.as_ref().map_or(String::new(), |why| format!(
                        ", with another version ({why})"
                    ))
                );
                let host = ui::Host {
                    peer: Some(peer),
                    name,
                    friend: seek.transport.is_friend(peer),
                    unfit,
                };
                seek.found.push((host, Instant::now()));
            }
            Event::Invited { peer, .. } => {
                let name = seek
                    .transport
                    .name_of(peer)
                    .unwrap_or_else(|| peer.to_string());
                coop_log!("{name} ({peer}) invites us");
                let new = !seek.invites.iter().any(|(p, _, _)| *p == peer);
                seek.invites.retain(|(p, _, _)| *p != peer);
                seek.invites.push((peer, name.clone(), Instant::now()));
                if new {
                    notices::say(
                        Tone::Good,
                        &format!("Coop: {name} invites you to build (see the coop page)"),
                    );
                }
            }
            Event::BadMessage { peer, error } => {
                coop_log!("unreadable message from {peer}: {error}")
            }
            _ => {}
        }
    }
    seek.found.retain(|(_, seen)| seen.elapsed() < FOUND_FOR);
    seek.invites.retain(|(_, _, at)| at.elapsed() < INVITE_FOR);
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

/// Links with other games, hosting or joining.
fn start(game: &Game, editor: Object, opening: Opening) -> Option<Box<Link>> {
    if let Err(reason) = hook(game, editor) {
        coop_log!("can't watch the editor: {reason}");
        return None;
    }
    // SAFETY: the editor is live (it's ticking).
    let component = unsafe { game.bricks.interface(editor) };
    if component.is_null() {
        return None;
    }
    let local = matches!(opening, Opening::LocalHost | Opening::LocalJoin);
    let (mut session, mut transport, role, joining) = match opening {
        Opening::LocalHost => match local::listen() {
            Ok(tcp) => (Session::new(info()), Wire::Local(tcp), Role::Host, None),
            Err(error) => {
                coop_log!("local test: can't listen: {error}");
                notices::alert("Coop: another game on this PC already hosts");
                return None;
            }
        },
        Opening::LocalJoin => match local::connect() {
            Ok(tcp) => (Session::new(info()), Wire::Local(tcp), Role::Member, None),
            Err(error) => {
                coop_log!("local test: can't connect: {error}");
                notices::alert("Coop: no other game on this PC hosts");
                return None;
            }
        },
        Opening::Host => match SteamTransport::new(true) {
            Ok(steam) => {
                coop_log!("Steam: direct connections disabled, relay only");
                (Session::new(info()), Wire::Steam(steam), Role::Host, None)
            }
            Err(reason) => {
                coop_log!("can't use Steam: {reason}");
                notices::alert("Coop needs Steam to play with friends.");
                return None;
            }
        },
        Opening::Join(seek, host) => {
            let Seek {
                session, transport, ..
            } = *seek;
            (session, Wire::Steam(transport), Role::Member, Some(host))
        }
    };
    // Join mode's log started when it began looking.
    if joining.is_none() {
        log::start(role.name());
    }
    log_inventory(game);
    let name = match &transport {
        Wire::Steam(steam) => steam.steam.own_name(),
        Wire::Local(_) => format!("{} {}", role.name(), std::process::id()),
    };
    let mut host_name = String::new();
    changes::watch(editor, component);
    match role {
        Role::Host => {
            // SAFETY: the editor is live.
            let objects = unsafe { objects(game, editor) };
            let settings = if local {
                // Approving too, so the join notification and its keys are tested on one PC.
                HostSettings {
                    code: local::CODE.into(),
                    max_members: 7,
                    approve: true,
                }
            } else {
                // Friends find the subhost themselves: no code, but its player says yes.
                HostSettings {
                    code: String::new(),
                    max_members: max_players() - 1,
                    approve: true,
                }
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
                "hosting this vehicle ({} objects), for {} players at most",
                objects.len(),
                if local { 8 } else { max_players() }
            );
        }
        Role::Member => match joining {
            Some(host) => {
                host_name = transport.name_of(host, "");
                session
                    .join(host, String::new(), name)
                    .expect("a seeking session is idle");
            }
            None => {
                host_name = "the other game".into();
                session
                    .join(local::HOST, local::CODE.into(), name)
                    .expect("a new session is idle");
                coop_log!("local test: joining the other instance");
            }
        },
    }
    Some(Box::new(Link {
        session,
        transport,
        role,
        host_name,
        joined: false,
        asking: Vec::new(),
        asking_shown: None,
        answers: Vec::new(),
        own_slot: (role == Role::Host).then_some(::coop::ids::SUBHOST_SLOT),
        members: Vec::new(),
        told_gone: Vec::new(),
        editor,
        component,
        loaded: false,
        bad_loads: 0,
        audit_at: Instant::now(),
        rounds: Rounds::default(),
        touched: Vec::new(),
        audits_failed: 0,
        started: Instant::now(),
        autosave: backup::AutoSave::new(),
        workload: ::coop::limits::Workload::default(),
        presence: presence::Shown::new((role == Role::Host).then_some(::coop::ids::SUBHOST_SLOT)),
        cameras: cameras::Cameras::new((role == Role::Host).then_some(::coop::ids::SUBHOST_SLOT)),
        here: Here::Editing,
        missed: Vec::new(),
        // SAFETY: the editor is live (it's ticking); game thread.
        editor_pointer: unsafe { weak_object::new_game_pointer(editor) }.ok(),
        in_match: dialogs::current_match(),
        accepted: Vec::new(),
        resyncing: false,
        kept: None,
    }))
}

/// Ends the link: tells the others, forgets the vehicle's ids.
fn stop(link: &mut Link, why: &str) {
    if let Some(Ok(kit)) = game().map(|game| &game.presence) {
        // SAFETY: on the game thread (the editor's tick).
        unsafe { link.presence.hide(kit) };
    }
    if let Some(Ok(kit)) = game().map(|game| &game.cameras) {
        // SAFETY: on the game thread (the editor's tick).
        unsafe { link.cameras.hide(kit) };
    }
    link.session.leave();
    send(link);
    changes::unwatch();
    coop_log!("coop stopped: {why}");
}

/// Sends what the session has queued, without reading.
fn send(link: &mut Link) {
    for out in link.session.take_outgoing() {
        link.transport.send(out.to, &out.bytes, out.reliable);
    }
}

/// One tick of a link; false once the session is over.
fn step(game: &Game, link: &mut Link) -> bool {
    match link.here {
        Here::Back(since) => refit(game, link, since),
        // The session's vehicle may still turn up: loaded late, or by the player.
        Here::Elsewhere(next) if Instant::now() >= next => {
            link.here = Here::Elsewhere(Instant::now() + LOOK_AGAIN);
            let target = Target {
                game,
                editor: link.editor,
                component: link.component,
            };
            if let Ok(reordered) = target.refit(&changes::remembered()) {
                coop_log!("found the session's vehicle in the editor after all");
                found_again(game, link, reordered);
            }
        }
        _ => {}
    }
    if link.here != Here::Editing {
        return away_step(link);
    }
    // SAFETY: the editor is live (it's ticking).
    let editing = unsafe { game.modes.is_editing(link.editor) };
    if let Ok(kit) = &game.presence
        && link.session.is_loaded()
    {
        let now = link.started.elapsed().as_millis() as u64;
        // SAFETY: the editor is live, its objects those `changes` knows (the vehicle is loaded).
        unsafe {
            link.presence
                .tick(kit, &mut link.session, link.editor, now, editing)
        };
    }
    if let Ok(kit) = &game.cameras
        && link.session.is_loaded()
    {
        let now = link.started.elapsed().as_millis() as u64;
        // SAFETY: the editor is live (it's ticking), on the game thread.
        unsafe { link.cameras.tick(kit, &mut link.session, link.editor, now) };
    }
    if !editing {
        send(link);
        return true;
    }
    if link.resyncing {
        // Edits of a vehicle the session's replaces.
        changes::take_edits();
        changes::forget_graves();
    } else {
        send_edits(game, link);
    }
    let target = Target {
        game,
        editor: link.editor,
        component: link.component,
    };
    for redo in changes::take_step_backs() {
        step_back(link, &target, redo);
    }
    let busy_since = Instant::now();
    let mut alive = network(link);
    let mut requests = Requests::default();
    let events: Vec<Event> = std::mem::take(&mut link.missed)
        .into_iter()
        .chain(link.session.take_events())
        .collect();
    for event in events {
        alive &= handle(link, Some(&target), event, &mut requests);
    }
    if alive && link.role == Role::Member && overloaded(link, busy_since) {
        return false;
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
        _ => {}
    }
    if link.session.is_loaded() {
        link.autosave.tick(game, link.editor, link.component);
    }
    send(link);
    alive
}

/// Member: counts the time this tick spent reading and applying what the subhost sent, and leaves
/// the session if the game spent most of a long while on it (`coop::limits::Workload`): a
/// subhost sending more than this game can apply, hostile or not, would otherwise keep it frozen.
/// Whether it left.
fn overloaded(link: &mut Link, since: Instant) -> bool {
    let busy = since.elapsed().as_millis() as u64;
    let now = link.started.elapsed().as_millis() as u64;
    if !link.workload.spent(now, busy) {
        return false;
    }
    coop_log!("left: applying what the subhost sends took most of the game's time for a while");
    notices::alert(&format!(
        "Coop: left {}'s session: your game can't keep up with it",
        link.host_name
    ));
    link.session.leave();
    send(link);
    true
}

/// One tick of a link whose editor doesn't hold the session's vehicle now ([`Here`]): the session
/// goes on, what needs the editor waits in `missed`. False once the session is over.
fn away_step(link: &mut Link) -> bool {
    let mut alive = network(link);
    for event in link.session.take_events() {
        alive &= handle(link, None, event, &mut Requests::default());
    }
    send(link);
    alive
}

/// What a link does whether its editor holds the vehicle or not: the session's clock, reading what
/// arrived, lost connections, an answer to our join request not coming, and the subhost's answers
/// to join requests. False if the session is over.
fn network(link: &mut Link) -> bool {
    link.session.tick(session_time(link, Instant::now()));
    if let Wire::Steam(steam) = &mut link.transport {
        steam.expect(link.session.expected_peers());
    }
    pump(&mut link.session, &mut link.transport);
    let mut alive = true;
    for Closed { peer, by_peer } in link.transport.take_lost() {
        match link.role {
            Role::Host => {
                // Only a member still in lost its connection: a kick ends one too, and one that
                // said goodbye (read just above, with its connection closing) or closed it on
                // purpose left, which the next members list tells.
                let still_in = link.session.expected_peers().contains(&peer);
                let name = link.members.iter().find(|m| m.peer == peer);
                match name.map(|m| m.name.clone()) {
                    Some(name) if still_in && !by_peer => {
                        notices::say(Tone::Plain, &format!("Coop: {name} lost connection"));
                        link.told_gone.push(peer);
                    }
                    _ => coop_log!("connection to {peer} ended"),
                }
                let _ = link.session.lost(peer);
                link.asking.retain(|(p, _)| *p != peer);
            }
            Role::Member => {
                let host = &link.host_name;
                notices::alert(&if by_peer {
                    format!("Coop: {host} stopped hosting")
                } else {
                    format!("Coop: lost connection to {host}")
                });
                alive = false;
            }
        }
    }
    if link.role == Role::Member && !link.joined && link.started.elapsed() > ANSWER_WAIT {
        notices::alert(&format!(
            "Coop: {} didn't answer your request",
            link.host_name
        ));
        alive = false;
    }
    if link.role == Role::Host {
        answer_joins(link);
    }
    alive
}

/// The player left the editor (or the timer noticed late): what showed of the others goes, they
/// stop seeing our selection and camera, and nothing names the editor's objects any more.
fn go_away(game: &Game, link: &mut Link) {
    let editing = link.here == Here::Editing;
    let alive = link.editor_pointer.and_then(weak_object::resolve).is_some();
    if editing && alive && link.role == Role::Host && link.kept.is_none() {
        keep(game, link);
    }
    link.here = Here::Away(editing);
    changes::detach();
    // SAFETY: game thread; what showed lives in the world, or went with the editor (only weak
    // pointers to it are followed).
    unsafe {
        if let Ok(kit) = &game.presence {
            link.presence.hide(kit);
        }
        if let Ok(kit) = &game.cameras {
            link.cameras.hide(kit);
        }
    }
    link.session.select(Vec::new());
    link.session.show_view(None);
    send(link);
    coop_log!("left the editor: the session goes on");
}

/// The editor ticking is `editor`. The link's own after a pause: as it was. Another (the player
/// left the link's and came back): the link moves to it and looks for the session's vehicle there
/// ([`Here::Back`]).
fn arrive(game: &Game, link: &mut Link, editor: Object) {
    let same = link.editor == editor
        && link
            .editor_pointer
            .and_then(weak_object::resolve)
            .is_some_and(|alive| alive == editor);
    if same {
        coop_log!("the editor ticks again");
        link.here = match link.here {
            Here::Away(true) => {
                changes::rewatch(link.editor, link.component);
                Here::Editing
            }
            // It never held the session's vehicle: look for it again.
            _ => Here::Back(Instant::now()),
        };
        return;
    }
    // SAFETY: the editor is live (it's ticking).
    let component = unsafe { game.bricks.interface(editor) };
    if component.is_null() {
        return; // Next tick.
    }
    if let Err(reason) = hook(game, editor) {
        coop_log!("can't watch this editor yet: {reason}");
        return;
    }
    if link.here == Here::Editing {
        go_away(game, link);
    }
    link.editor = editor;
    link.component = component;
    // SAFETY: as above; game thread.
    link.editor_pointer = unsafe { weak_object::new_game_pointer(editor) }.ok();
    link.presence.new_editor();
    link.cameras.new_editor();
    link.here = Here::Back(Instant::now());
    coop_log!("in an editor again: looking for the session's vehicle in it");
}

/// [`Here::Back`]: finds the session's vehicle in the editor (`Target::refit`) as soon as it's
/// there. A member not finding it gets the session's anew; a subhost asks its player whether to
/// share this one.
fn refit(game: &Game, link: &mut Link, since: Instant) {
    let target = Target {
        game,
        editor: link.editor,
        component: link.component,
    };
    let known = changes::remembered();
    let why = match target.refit(&known) {
        Ok(reordered) => {
            coop_log!(
                "found the session's vehicle in the editor ({} objects{}); {} things to catch up on",
                known.len(),
                if reordered { ", in another order" } else { "" },
                link.missed.len()
            );
            found_again(game, link, reordered);
            return;
        }
        Err(_) if since.elapsed() < REFIT_WAIT => return,
        Err(why) => why,
    };
    match link.role {
        Role::Member => {
            coop_log!("the editor holds another vehicle ({why}): asking for the session's");
            backup::now(
                game,
                link.editor,
                link.component,
                "vehicle before going back to the session",
            );
            changes::watch(link.editor, link.component);
            // A snapshot already here replaces the vehicle anyway; what came before it names
            // bricks this editor doesn't have.
            match link
                .missed
                .iter()
                .rposition(|event| matches!(event, Event::Load { .. }))
            {
                Some(at) => drop(link.missed.drain(..at)),
                None => {
                    link.missed.clear();
                    link.session.request_resync();
                }
            }
            link.resyncing = true;
            link.here = Here::Editing;
        }
        // SAFETY: the editor is live (it's ticking); game thread.
        Role::Host if link.kept.is_some() && unsafe { objects(game, link.editor) }.is_empty() => {
            coop_log!("the editor is empty: the session's vehicle comes back");
            resume(game, link);
        }
        Role::Host => {
            coop_log!("the editor holds another vehicle ({why}): asking what to do");
            link.here = Here::Elsewhere(Instant::now() + LOOK_AGAIN);
            SHARE_ASKED.store(true, Ordering::Relaxed);
        }
    }
}

/// The session's vehicle is in the editor again: building goes on. `reordered`: its objects came
/// in another order (the game's loader puts seats first): a subhost's members get this one, a
/// member asks for the subhost's.
fn found_again(game: &Game, link: &mut Link, reordered: bool) {
    link.here = Here::Editing;
    link.kept = None;
    if !reordered {
        return;
    }
    match link.role {
        Role::Host => {
            let members = link
                .members
                .iter()
                .filter(|member| Some(member.slot) != link.own_slot)
                .map(|member| member.peer)
                .collect();
            send_orders(game, link, members);
        }
        Role::Member => link.session.request_order(),
    }
}

/// The game closes the editor (to test the vehicle, say; `ExitEditor`, through `spawn_mode`'s
/// hook, while the editor is still there): a subhost sends its last edits and keeps a copy of the
/// session's vehicle, should its player come back to another vehicle or an empty editor.
pub fn editor_closing() {
    let Some(game) = game() else { return };
    DRIVER.with(|driver| {
        // Never inside coop's own tick.
        let Ok(mut driver) = driver.try_borrow_mut() else {
            return;
        };
        let State::Linked(link) = &mut driver.state else {
            return;
        };
        let alive = link.editor_pointer.and_then(weak_object::resolve).is_some();
        if link.role != Role::Host || link.here != Here::Editing || !alive {
            return;
        }
        send_edits(game, link);
        send(link);
        keep(game, link);
    });
}

/// Subhost, the editor holding the session's vehicle: keeps a copy of it ([`Link::kept`]).
fn keep(game: &Game, link: &mut Link) {
    match snapshot(game, link) {
        Ok(kept) => {
            coop_log!(
                "kept a copy of the session's vehicle: {} objects, {} KB",
                kept.ids.len(),
                kept.vehicle.len() / 1024
            );
            link.kept = Some(kept);
        }
        Err(reason) => coop_log!("couldn't keep a copy of the session's vehicle: {reason}"),
    }
}

/// Subhost, back with another vehicle or none: the copy kept as the editor closed comes back
/// (the editor's vehicle backed up first); what the others did since follows (`missed`).
fn resume(game: &Game, link: &mut Link) {
    let Some(kept) = link.kept.take() else {
        return;
    };
    backup::now(
        game,
        link.editor,
        link.component,
        "vehicle before going back to the session",
    );
    changes::watch(link.editor, link.component);
    let target = Target {
        game,
        editor: link.editor,
        component: link.component,
    };
    if target.load(&kept.ids, &kept.vehicle, false).drifted {
        coop_log!("  the session's vehicle came back differently: audits will show where");
    }
    link.here = Here::Editing;
    coop_log!(
        "the session's vehicle is back; {} things to catch up on",
        link.missed.len()
    );
}

/// Subhost, [`Here::Elsewhere`]: the editor's vehicle replaces the session's for everyone.
fn share(game: &Game, link: &mut Link) {
    // SAFETY: the editor is live (it's ticking); game thread.
    let objects = unsafe { objects(game, link.editor) };
    if objects.is_empty() {
        notices::alert("Coop: the editor is empty: nothing to share");
        return;
    }
    match link.session.rehost(objects.len()) {
        Ok(ids) => {
            changes::watch(link.editor, link.component);
            // SAFETY: as above.
            unsafe {
                adopt(game, &objects, &ids);
                changes::remember_order(&objects);
            }
            link.missed.clear();
            link.kept = None;
            link.touched.clear();
            link.rounds = Rounds::default();
            link.here = Here::Editing;
            coop_log!("shared the editor's vehicle ({} objects)", ids.len());
        }
        Err(error) => {
            coop_log!("can't share the editor's vehicle: {error:?}");
            notices::alert("Coop: this vehicle has too many bricks to share");
        }
    }
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

/// Does what the session asks, in `target` (`None`: the editor doesn't hold the session's vehicle
/// now, so what needs it waits in `missed`); false if it ended.
fn handle(link: &mut Link, target: Option<&Target>, event: Event, requests: &mut Requests) -> bool {
    let Some(target) = target else {
        match event {
            // The next one comes.
            Event::Audit(_) => {}
            // It replaces the vehicle: what came before it is in it.
            Event::Load { .. } => {
                link.missed
                    .retain(|e| !matches!(e, Event::Apply { .. } | Event::Reorder(_)));
                link.missed.push(event);
            }
            Event::Apply { .. }
            | Event::Reorder(_)
            | Event::SnapshotWanted(_)
            | Event::OrderWanted(_) => link.missed.push(event),
            other => return handle_anywhere(link, other),
        }
        return true;
    };
    match event {
        Event::Load { ids, vehicle } => {
            if !link.loaded {
                backup::now(
                    target.game,
                    link.editor,
                    link.component,
                    "vehicle before joining",
                );
            } else if ids.iter().all(|&id| changes::object(id).is_none())
                && !changes::known().is_empty()
            {
                // None of the bricks here: the subhost shared another vehicle.
                backup::now(
                    target.game,
                    link.editor,
                    link.component,
                    "session's vehicle before the host shared another",
                );
            }
            let outcome = target.load(&ids, &vehicle, !link.loaded);
            link.loaded = true;
            link.resyncing = false;
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
        other => return handle_anywhere(link, other),
    }
    true
}

/// Does what the session asks that doesn't need the editor; false if it ended.
fn handle_anywhere(link: &mut Link, event: Event) -> bool {
    match event {
        Event::Joined { slot } => {
            coop_log!("joined as slot {slot}");
            link.joined = true;
            link.own_slot = Some(slot);
            notices::say(
                Tone::Plain,
                &format!("Coop: you joined {}'s session", link.host_name),
            );
            link.presence.joined(slot);
            link.cameras.joined(slot);
            for peer in link.session.expected_peers() {
                link.transport.log_connection(peer);
            }
        }
        Event::Refused(reason) => {
            let tone = match reason {
                RefuseReason::Banned { .. } => Tone::Bad,
                _ => Tone::Plain,
            };
            notices::say(tone, &refusal_text(&link.host_name, &reason));
            return false;
        }
        Event::JoinAsked { peer, name } => {
            let name = link.transport.name_of(peer, &name);
            coop_log!("{name} ({peer}) asks to join");
            link.asking.retain(|(p, _)| *p != peer);
            link.asking.push((peer, name));
            link.asking_shown = None;
            if link.accepted.contains(&peer) {
                coop_log!("  let in earlier in this session: let in again");
                link.answers.push((peer, true));
            } else if REQUEST_OPENS_PAGE.get() {
                PAGE_ASKED.store(true, Ordering::Relaxed);
            }
        }
        Event::JoinWithdrawn { peer } => {
            if let Some(at) = link.asking.iter().position(|(p, _)| *p == peer) {
                link.asking.remove(at);
                link.asking_shown = None;
            }
        }
        Event::Members(members) => {
            let names: Vec<&str> = members.iter().map(|m| m.name.as_str()).collect();
            coop_log!("members: {}", names.join(", "));
            tell_who_changed(link, &members);
            link.members = members.clone();
            link.cameras.members(&members);
            link.presence.members(members);
        }
        Event::Drifted => coop_log!("drifted from the subhost: asked for a snapshot"),
        Event::UndoConflict => {
            coop_log!("an undo met someone else's edit of the same brick: asked for a snapshot")
        }
        Event::Ended(reason) => {
            coop_log!("session ended: {reason:?}");
            let host = &link.host_name;
            match reason {
                EndReason::Kicked => {
                    notices::alert(&format!("Coop: {host} removed you from the session"))
                }
                EndReason::Closed => notices::alert(&format!("Coop: {host} stopped hosting")),
                EndReason::HostFlooded(_) => notices::alert(&format!(
                    "Coop: left {host}'s session: it sent far too much data"
                )),
                EndReason::Left => {}
            }
            return false;
        }
        Event::BadMessage { peer, error } => coop_log!("unreadable message from {peer}: {error}"),
        Event::Flooded { peer, what } => {
            coop_log!("kicked {peer}: it sent far too much ({what:?}); it can't join again")
        }
        Event::Selection { slot, ids } => link.presence.selected(slot, ids),
        Event::Camera { slot, view } => link.cameras.seen(slot, view),
        // Already in a session: invites don't matter.
        Event::Hello { .. } | Event::Invited { .. } => {}
        // Made in the editor (`handle`).
        Event::Load { .. }
        | Event::Apply { .. }
        | Event::SnapshotWanted(_)
        | Event::OrderWanted(_)
        | Event::Audit(_)
        | Event::Reorder(_) => {}
    }
    true
}

/// What the player is told when `host` refuses their request to join.
fn refusal_text(host: &str, reason: &RefuseReason) -> String {
    match reason {
        RefuseReason::Refused => format!("Coop: {host} declined your request"),
        RefuseReason::Full => format!("Coop: {host}'s session is full"),
        RefuseReason::GameVersion => format!("Coop: {host} is on a different game version"),
        // Players never type a code: both sides of a Steam session use none, so this takes a
        // build that sends one.
        RefuseReason::WrongCode => format!("Coop: couldn't join {host}'s session"),
        RefuseReason::NotHosting => format!("Coop: {host} isn't hosting anymore"),
        RefuseReason::Banned { minutes, reason } => {
            let how_long = match *minutes {
                u16::MAX => String::new(),
                1 => " for another minute".to_owned(),
                minutes => format!(" for another {minutes} minutes"),
            };
            // Typed by the host's player: one line, whatever it holds.
            let reason = reason.replace(char::is_control, " ");
            let why = match reason.trim() {
                "" => String::new(),
                reason => format!(". Reason: {reason}"),
            };
            format!("Coop: you are banned from {host}'s session{how_long}{why}")
        }
    }
}

/// Says in the chat who came and who left, between the session's members so far and `members`. A
/// member arriving finds the others there: only those after it are news.
fn tell_who_changed(link: &mut Link, members: &[Member]) {
    let arriving = link.role == Role::Member && link.members.is_empty();
    if !arriving {
        for old in &link.members {
            if Some(old.slot) != link.own_slot
                && !members.iter().any(|m| m.slot == old.slot)
                && !link.told_gone.contains(&old.peer)
            {
                notices::say(Tone::Plain, &format!("Coop: {} left", old.name));
            }
        }
        for new in members {
            if Some(new.slot) != link.own_slot && !link.members.iter().any(|m| m.slot == new.slot) {
                notices::say(Tone::Plain, &format!("Coop: {} joined", new.name));
            }
        }
    }
    link.told_gone.clear();
}

/// Subhost: shows who asks to join, and answers them as the coop page says.
fn answer_joins(link: &mut Link) {
    for (peer, yes) in std::mem::take(&mut link.answers) {
        let Some(at) = link.asking.iter().position(|(p, _)| *p == peer) else {
            continue;
        };
        let (_, name) = link.asking.remove(at);
        link.asking_shown = None;
        let _ = link.session.answer_join(peer, yes);
        if yes {
            link.transport.log_connection(peer);
            if !link.accepted.contains(&peer) {
                link.accepted.push(peer);
            }
        } else {
            notices::say(Tone::Plain, &format!("Coop: you declined {name}'s request"));
        }
    }
    let Some((_, name)) = link.asking.first().cloned() else {
        return;
    };
    if link
        .asking_shown
        .is_none_or(|at| at.elapsed() >= ASKING_EVERY)
    {
        link.asking_shown = Some(Instant::now());
        let more = match link.asking.len() - 1 {
            0 => String::new(),
            n => format!(" ({n} more waiting)"),
        };
        notices::tell(&format!(
            "Coop: {name} asks to join: answer on the coop page{more}"
        ));
    }
}

/// Subhost: answers snapshot requests with the vehicle as it is now (every event applied, every
/// local edit sent).
fn answer(game: &Game, link: &mut Link, requests: Vec<SnapshotRequest>) {
    let Kept { ids, vehicle } = match snapshot(game, link) {
        Ok(snapshot) => snapshot,
        Err(reason) => {
            coop_log!("couldn't save the vehicle for a member: {reason}");
            return;
        }
    };
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

/// The editor's bricks that have coop ids, as a snapshot sends them. The editor must be live.
fn snapshot(game: &Game, link: &Link) -> Result<Kept, String> {
    // SAFETY: the editor is live (per the contract); game thread.
    let objects: Vec<Object> = unsafe { objects(game, link.editor) }
        .into_iter()
        .filter(|&object| changes::id_of(object).is_some())
        .collect();
    let ids = objects
        .iter()
        .filter_map(|&object| changes::id_of(object))
        .collect::<Vec<_>>();
    // SAFETY: the editor's live objects; game thread.
    let archive = unsafe { game.archive.save(link.component, &objects) }?;
    let groups = objects
        .iter()
        // SAFETY: as above.
        .map(|&object| unsafe { groups_of(game, object) })
        .collect::<Option<Vec<Groups>>>()
        .unwrap_or_default();
    let vehicle = Vehicle { archive, groups }.encode();
    Ok(Kept { ids, vehicle })
}
