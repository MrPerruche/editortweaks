//! The coop pages (`docs/coop-ui.md`): Editor tweaks, Coop... (or its key) opens the coop page in
//! the in-game menu (`settings_menu::Page`); Back there closes the menu. Its title says what coop
//! does. Its body lists what to act on, each row with its actions: requests to join (let in,
//! refuse) and the players with their colours (kick, ban) while hosting, the players as a member,
//! friends' invites (join) alone; then coop's options. Its buttons only lead to the other pages or
//! do what concerns the whole session: host, join a friend, invite friends, bans, stop hosting,
//! leave. Sub-pages list the friends hosting (join), the bans (lift) and the friends to invite.
//!
//! Questions ("Stop hosting?") and bans are the game's own popups (`dialogs`), over the page; if
//! the game's popups weren't found, a page of ours asks instead.
//!
//! The pages are made while the menu runs, never inside coop's tick: they only read the [`Status`]
//! coop leaves after each tick, and note what's clicked as [`Request`]s for the next one.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ::coop::PeerId;

use super::dialogs;
use crate::features::settings_menu::{self, Action, BodyRow, BodyValue, Button, Page, Shown, Then};

// The coop page lists coop's options after its rows.
pub static COOP: Page = Page {
    path: "Coop",
    category: "Properties.Settings.EditorTweaksCoop",
    sections: &super::SETTINGS,
    body: &["Join requests", "Players", "Invites"],
    make: || local_test_marked(coop_page()),
};

static JOIN: Page = Page {
    path: "Coop/CoopJoin",
    category: "Properties.Settings.EditorTweaksCoopJoin",
    sections: &[],
    body: &["Friends", "This match"],
    make: join_page,
};

static BANS: Page = Page {
    path: "Coop/CoopBans",
    category: "Properties.Settings.EditorTweaksCoopBans",
    sections: &[],
    body: &["Banned in this match"],
    make: bans_page,
};

static INVITE: Page = Page {
    path: "Coop/CoopInvite",
    category: "Properties.Settings.EditorTweaksCoopInvite",
    sections: &[],
    body: &["Friends playing Brick Rigs"],
    make: invite_page,
};

/// Asks before an action if the game's popups weren't found.
static CONFIRM: Page = Page {
    path: "Coop/CoopConfirm",
    category: "Properties.Settings.EditorTweaksCoopConfirm",
    sections: &[],
    body: &[],
    make: confirm_page,
};

const HOST_ICON: &str = "/Game/BrickRigs/UI/Textures/Icons/T_ServerHost.T_ServerHost";
/// The in-game menu's "Players" button's (`WBP_Menu`, `Name_Menu_Players`).
const PLAYER_ICON: &str = "/Game/BrickRigs/UI/Textures/Icons/T_Dummy.T_Dummy";
const EXIT_ICON: &str = "/Game/BrickRigs/UI/Textures/Icons/T_Exit.T_Exit";
const NO_ICON: &str = "/Game/BrickRigs/UI/Textures/Icons/T_Cancel.T_Cancel";
const RETRY_ICON: &str = "/Game/BrickRigs/UI/Textures/Icons/T_Retry.T_Retry";
/// The shield the game's kick and ban buttons show (`WBP_PlayersMenu` "Kick player",
/// `WBP_KickPlayerPopup` "Revoke kick").
const BAN_ICON: &str = "/Game/BrickRigs/UI/Textures/Icons/T_Admin.T_Admin";
const INVITE_ICON: &str = "/Game/BrickRigs/UI/Textures/Icons/T_Add.T_Add";

/// Lists the pages. Before engine init (in `on_load`).
pub fn add_pages() {
    for page in [&COOP, &JOIN, &BANS, &INVITE, &CONFIRM] {
        settings_menu::add_page(page);
    }
}

/// What coop does, as the pages show it.
#[derive(Clone, PartialEq, Default)]
pub struct Status {
    pub doing: Doing,
    /// Hosting: the editor holds another vehicle than the session's (the player came back to the
    /// editor with it), until they bring the session's back, share this one or stop hosting.
    pub elsewhere: bool,
    /// Hosting: a copy of the session's vehicle is kept (taken as the editor closed), to bring
    /// back.
    pub kept: bool,
    /// Whether coop links this game with another on this PC (the local test), not Steam friends.
    pub local: bool,
    /// Everyone in the session (us too), the subhost first.
    pub players: Vec<Player>,
    /// Hosting: who asks to join, oldest first.
    pub asking: Vec<(PeerId, String)>,
    /// Hosts to join: friends and players of our match found hosting lately, or the local test's
    /// other game.
    pub hosts: Vec<Host>,
    /// Looking for hosts: how many Steam friends play Brick Rigs, and how many others are in our
    /// match.
    pub friends_playing: usize,
    pub server_players: usize,
    /// Whether friends hosting are looked for now.
    pub looking: bool,
    /// Hosting: who our player banned.
    pub bans: Vec<Banned>,
    /// Hosting: Steam friends playing, to invite.
    pub friends: Vec<Friend>,
    /// Alone: who invited us lately (Steam id, name).
    pub invites: Vec<(PeerId, String)>,
}

#[derive(Clone, PartialEq, Default)]
pub enum Doing {
    #[default]
    Alone,
    Hosting,
    /// Asking this host to let us in.
    Asking(String),
    /// In this host's vehicle.
    Member(String),
}

/// Someone in the session.
#[derive(Clone, PartialEq)]
pub struct Player {
    pub peer: PeerId,
    pub name: String,
    /// The colour their selection and camera show in.
    pub color: &'static str,
    pub host: bool,
    pub you: bool,
}

/// A host to join.
#[derive(Clone, PartialEq)]
pub struct Host {
    /// Its Steam id (`None`: the local test's other game).
    pub peer: Option<PeerId>,
    pub name: String,
    /// A Steam friend (else a player of our match).
    pub friend: bool,
    /// Why it can't be joined (another version), if so.
    pub unfit: Option<String>,
}

/// Someone banned from our sessions.
#[derive(Clone, PartialEq)]
pub struct Banned {
    pub peer: PeerId,
    pub name: String,
    pub reason: String,
    /// Minutes left, rounded up (`None`: until the match ends).
    pub minutes: Option<u64>,
}

/// A Steam friend playing, to invite.
#[derive(Clone, PartialEq)]
pub struct Friend {
    pub peer: PeerId,
    pub name: String,
    /// Whether we invited them lately.
    pub invited: bool,
}

/// What the player clicked or confirmed, for coop's next tick.
#[derive(Clone)]
pub enum Request {
    Host,
    /// Join this host (see [`Host::peer`]), called this.
    Join(Option<PeerId>, String),
    /// Stop hosting, leave, or stop asking.
    Leave,
    /// Let this player in, or not.
    Answer(PeerId, bool),
    /// Look for friends hosting for a while.
    Look,
    /// Remove this member; they may ask again.
    Kick(PeerId),
    /// Open the ban dialog for this member, called this.
    OpenBan(PeerId, String),
    /// The ban dialog was confirmed: ban them for this long (0: until the match ends).
    Ban {
        peer: PeerId,
        name: String,
        reason: String,
        seconds: u64,
    },
    /// Lift this player's ban.
    Unban(PeerId),
    /// Invite this friend to our session.
    Invite(PeerId, String),
    /// Hosting, the editor holding another vehicle: it replaces the session's.
    Share,
    /// Hosting, the editor holding another vehicle: the session's comes back instead.
    Resume,
}

/// What the fallback confirmation page asks, and what its button then requests.
#[derive(Clone)]
struct Confirm {
    question: String,
    button: String,
    icon: &'static str,
    request: Request,
}

static STATUS: Mutex<Option<Status>> = Mutex::new(None);
static REQUESTS: Mutex<Vec<Request>> = Mutex::new(Vec::new());
static CONFIRMING: Mutex<Option<Confirm>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What was clicked or confirmed since the last call.
pub fn take_requests() -> Vec<Request> {
    std::mem::take(&mut *lock(&REQUESTS))
}

/// Notes `request` for coop's next tick.
pub fn request(request: Request) {
    lock(&REQUESTS).push(request);
}

/// Whether two statuses show the same on a page.
type Same = fn(&Status, &Status) -> bool;

/// Notes what coop does now; a page showing what changed is made anew. Game thread, outside coop's
/// own state (making a page reads only the status).
pub fn update(status: Status) {
    let was = {
        let mut shown = lock(&STATUS);
        let was = shown.clone().unwrap_or_default();
        *shown = Some(status.clone());
        was
    };
    let pages: [(&'static Page, Same); 4] = [
        (&COOP, |a, b| {
            a.doing == b.doing
                && a.elsewhere == b.elsewhere
                && a.kept == b.kept
                && a.asking == b.asking
                && a.players == b.players
                && a.bans.len() == b.bans.len()
                && a.invites == b.invites
        }),
        (&JOIN, |a, b| {
            a.doing == b.doing
                && a.hosts == b.hosts
                && a.looking == b.looking
                && a.friends_playing == b.friends_playing
                && a.server_players == b.server_players
        }),
        (&BANS, |a, b| a.doing == b.doing && a.bans == b.bans),
        (&INVITE, |a, b| {
            a.doing == b.doing && a.friends == b.friends && a.players == b.players
        }),
    ];
    for (page, same) in pages {
        if !same(&was, &status) {
            settings_menu::refresh(page);
        }
    }
}

fn status() -> Status {
    lock(&STATUS).clone().unwrap_or_default()
}

fn button(
    text: String,
    icon: &'static str,
    run: impl FnMut() + Send + 'static,
    then: Then,
) -> Button {
    Button {
        text,
        icon,
        run: Box::new(run),
        then,
    }
}

/// Asks `question` (the game's popup, over the page) before `request` goes; without the game's
/// popups, the confirmation page asks, its button called `text`.
fn ask(text: &str, icon: &'static str, question: String, request: Request) {
    if dialogs::available() {
        dialogs::ask(&question, request);
        return;
    }
    *lock(&CONFIRMING) = Some(Confirm {
        question,
        button: text.to_owned(),
        icon,
        request,
    });
    settings_menu::show(&CONFIRM);
}

const SHARE_QUESTION: &str = "This editor has another vehicle. Share it? It replaces the \
     session's vehicle for everyone, which is backed up first. Cancel: the others wait until you \
     share or stop hosting.";

const RESUME_QUESTION: &str = "This editor has another vehicle. Restore the session's vehicle, \
     with what the others built meanwhile? Yours is backed up first. Cancel: choose on the coop \
     page.";

/// Asks the subhost's player what to do with the editor's vehicle (it came back to the editor with
/// another than the session's): continue the session's if a copy was `kept`, else share this one.
/// Outside coop's tick.
pub fn ask_back(kept: bool) {
    if kept {
        ask(
            "Restore session's vehicle",
            HOST_ICON,
            RESUME_QUESTION.into(),
            Request::Resume,
        );
    } else {
        ask(
            "Share this vehicle",
            HOST_ICON,
            SHARE_QUESTION.into(),
            Request::Share,
        );
    }
}

/// A button asking `question` before `request` goes (see [`ask`]).
fn asking(text: &str, icon: &'static str, question: String, request: Request) -> Button {
    let text_owned = text.to_owned();
    button(
        text.to_owned(),
        icon,
        move || ask(&text_owned, icon, question.clone(), request.clone()),
        Then::Stay,
    )
}

/// A body row's action noting `request`.
fn act(name: &str, request: Request) -> (String, Action) {
    (
        name.to_owned(),
        Arc::new(move || self::request(request.clone())),
    )
}

/// A body row's action asking `question` before `request` goes (see [`ask`]).
fn act_asking(
    name: &str,
    icon: &'static str,
    question: String,
    request: Request,
) -> (String, Action) {
    let text = name.to_owned();
    (
        name.to_owned(),
        Arc::new(move || ask(&text, icon, question.clone(), request.clone())),
    )
}

/// A row of text only, across the row and centred (if its layout was found, else in the name's
/// 40% of it, where a longer text scrolls).
fn note(text: String) -> BodyRow {
    BodyRow {
        text,
        value: BodyValue::Note,
    }
}

fn actions(text: String, actions: Vec<(String, Action)>) -> BodyRow {
    BodyRow {
        text,
        value: BodyValue::Actions(actions),
    }
}

/// "Bob", "Bob and Carl", "Bob, Carl and Dave".
fn names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The others' names.
fn others(status: &Status) -> Vec<String> {
    status
        .players
        .iter()
        .filter(|player| !player.you)
        .map(|player| player.name.clone())
        .collect()
}

/// The players' rows: a host kicks or bans the others; everyone sees the colours.
fn player_rows(status: &Status) -> Vec<BodyRow> {
    let hosting = status.doing == Doing::Hosting;
    status
        .players
        .iter()
        .map(|player| {
            let tags: Vec<&str> = [(player.you, "you"), (player.host, "host")]
                .into_iter()
                .filter_map(|(is, tag)| is.then_some(tag))
                .chain([player.color])
                .collect();
            let text = format!("{} ({})", player.name, tags.join(", "));
            if !hosting || player.you {
                return note(text);
            }
            let (peer, name) = (player.peer, player.name.clone());
            actions(
                text,
                vec![
                    act_asking(
                        "Kick",
                        NO_ICON,
                        format!("Kick {name}? They can ask to join again."),
                        Request::Kick(peer),
                    ),
                    act("Ban...", Request::OpenBan(peer, name)),
                ],
            )
        })
        .collect()
}

/// A row joining `host` (see [`Host::peer`]), called `name`, once confirmed.
fn join_row(text: String, host: Option<PeerId>, name: &str) -> BodyRow {
    actions(
        text,
        vec![act_asking(
            "Join",
            PLAYER_ICON,
            format!("Join {name}? Your vehicle is backed up, then replaced by theirs."),
            Request::Join(host, name.to_owned()),
        )],
    )
}

/// The local test is easy to forget on (friends then find no host): pages say so in their title.
fn local_test_marked(mut shown: Shown) -> Shown {
    if status().local {
        shown.title.push_str(" (local test: this PC only)");
    }
    shown
}

fn coop_page() -> Shown {
    let status = status();
    // Joining lists them: they're found by then.
    request(Request::Look);
    match status.doing.clone() {
        Doing::Alone => Shown {
            title: "Coop: building alone".into(),
            buttons: vec![
                button(
                    "Host this vehicle".into(),
                    HOST_ICON,
                    || request(Request::Host),
                    Then::Close,
                ),
                button(
                    "Join someone".into(),
                    PLAYER_ICON,
                    || {},
                    Then::Open(JOIN.path),
                ),
            ],
            body: vec![
                Vec::new(),
                Vec::new(),
                status
                    .invites
                    .iter()
                    .map(|(peer, name)| join_row(format!("{name} invites you"), Some(*peer), name))
                    .collect(),
            ],
        },
        Doing::Hosting => {
            let requests = status
                .asking
                .iter()
                .map(|(peer, name)| {
                    actions(
                        name.clone(),
                        vec![
                            act("Let in", Request::Answer(*peer, true)),
                            act("Refuse", Request::Answer(*peer, false)),
                        ],
                    )
                })
                .collect();
            let others = others(&status);
            let question = if others.is_empty() {
                "Stop hosting?".into()
            } else {
                format!(
                    "Stop hosting? {} can't build in your vehicle any more.",
                    names(&others)
                )
            };
            let title = if status.elsewhere {
                "Coop: hosting, but not the session's vehicle".into()
            } else if others.is_empty() {
                "Coop: hosting, nobody joined yet".into()
            } else {
                format!("Coop: hosting, with {}", names(&others))
            };
            let resume = (status.elsewhere && status.kept).then(|| {
                button(
                    "Restore session's vehicle".into(),
                    HOST_ICON,
                    || request(Request::Resume),
                    Then::Close,
                )
            });
            let share = status.elsewhere.then(|| {
                asking(
                    "Share this vehicle",
                    HOST_ICON,
                    SHARE_QUESTION.into(),
                    Request::Share,
                )
            });
            Shown {
                title,
                buttons: resume
                    .into_iter()
                    .chain(share)
                    .chain([
                        button(
                            "Invite friends".into(),
                            INVITE_ICON,
                            || {},
                            Then::Open(INVITE.path),
                        ),
                        button(
                            format!("Bans ({})", status.bans.len()),
                            BAN_ICON,
                            || {},
                            Then::Open(BANS.path),
                        ),
                        asking("Stop hosting", EXIT_ICON, question, Request::Leave),
                    ])
                    .collect(),
                body: vec![requests, player_rows(&status)],
            }
        }
        Doing::Asking(host) => Shown {
            title: format!("Coop: asking {host} to let you in"),
            buttons: vec![button(
                "Cancel request".into(),
                NO_ICON,
                || request(Request::Leave),
                Then::Stay,
            )],
            body: Vec::new(),
        },
        Doing::Member(host) => {
            let with: Vec<String> = others(&status)
                .into_iter()
                .filter(|name| *name != host)
                .collect();
            let title = if with.is_empty() {
                format!("Coop: building in {host}'s vehicle")
            } else {
                format!("Coop: building in {host}'s vehicle, with {}", names(&with))
            };
            Shown {
                title,
                buttons: vec![asking(
                    "Leave session",
                    EXIT_ICON,
                    format!("Leave {host}'s vehicle? You keep it as it is now, to build alone."),
                    Request::Leave,
                )],
                body: vec![Vec::new(), player_rows(&status)],
            }
        }
    }
}

fn join_page() -> Shown {
    let status = status();
    request(Request::Look);
    if !matches!(status.doing, Doing::Alone) {
        return Shown {
            title: "Coop: leave first".into(),
            buttons: Vec::new(),
            body: Vec::new(),
        };
    }
    let title = if !status.hosts.is_empty() {
        "Join a vehicle"
    } else if status.looking {
        "Looking for hosts..."
    } else {
        "No hosts found"
    };
    let buttons = if status.looking {
        Vec::new()
    } else {
        // Looking again shows the page anew.
        vec![button(
            "Refresh".into(),
            RETRY_ICON,
            || request(Request::Look),
            Then::Open(JOIN.path),
        )]
    };
    let rows = |friends: bool| -> Vec<BodyRow> {
        status
            .hosts
            .iter()
            .filter(|host| host.friend == friends)
            .map(|host| match &host.unfit {
                Some(_) => note(format!("{}: different version", host.name)),
                None => join_row(host.name.clone(), host.peer, &host.name),
            })
            .collect()
    };
    // Nobody hosting: a short note saying who's there.
    let mut friends = rows(true);
    if friends.is_empty() {
        friends.push(note(match status.friends_playing {
            0 => "No friend playing".to_owned(),
            n => format!("No friend hosting ({n} playing)"),
        }));
    }
    let mut server = rows(false);
    if server.is_empty() {
        server.push(note(match status.server_players {
            0 => "No one else here".to_owned(),
            1 => "Nobody hosting (1 other player)".to_owned(),
            n => format!("Nobody hosting ({n} other players)"),
        }));
    }
    Shown {
        title: title.into(),
        buttons,
        body: vec![friends, server],
    }
}

fn bans_page() -> Shown {
    let status = status();
    let rows = status
        .bans
        .iter()
        .map(|ban| {
            let left = match ban.minutes {
                None => "until the match ends".to_owned(),
                Some(1) => "1 more minute".to_owned(),
                Some(minutes) => format!("{minutes} more minutes"),
            };
            let text = if ban.reason.is_empty() {
                format!("{}: {left}", ban.name)
            } else {
                format!("{}: {left} ({})", ban.name, ban.reason)
            };
            actions(
                text,
                vec![act_asking(
                    "Lift",
                    BAN_ICON,
                    format!("Lift {}'s ban? They can ask to join again.", ban.name),
                    Request::Unban(ban.peer),
                )],
            )
        })
        .collect();
    let title = if status.bans.is_empty() {
        "Coop: nobody is banned"
    } else {
        "Coop: banned players"
    };
    Shown {
        title: title.into(),
        buttons: Vec::new(),
        body: vec![rows],
    }
}

fn invite_page() -> Shown {
    let status = status();
    if status.doing != Doing::Hosting {
        return Shown {
            title: "Coop: host first".into(),
            buttons: Vec::new(),
            body: Vec::new(),
        };
    }
    let in_session: Vec<PeerId> = status.players.iter().map(|player| player.peer).collect();
    let rows: Vec<BodyRow> = status
        .friends
        .iter()
        .filter(|friend| !in_session.contains(&friend.peer))
        .map(|friend| {
            if friend.invited {
                note(format!("{} (invited)", friend.name))
            } else {
                actions(
                    friend.name.clone(),
                    vec![act(
                        "Invite",
                        Request::Invite(friend.peer, friend.name.clone()),
                    )],
                )
            }
        })
        .collect();
    let title = if rows.is_empty() {
        "Coop: no friends playing Brick Rigs"
    } else {
        "Coop: invite friends (they need Editor tweaks)"
    };
    Shown {
        title: title.into(),
        buttons: Vec::new(),
        body: vec![rows],
    }
}

fn confirm_page() -> Shown {
    let confirming = lock(&CONFIRMING).clone();
    let Some(confirm) = confirming else {
        return Shown {
            title: "Coop".into(),
            buttons: Vec::new(),
            body: Vec::new(),
        };
    };
    let then = confirm.request;
    Shown {
        title: confirm.question,
        buttons: vec![button(
            confirm.button,
            confirm.icon,
            move || request(then.clone()),
            Then::Close,
        )],
        body: Vec::new(),
    }
}
