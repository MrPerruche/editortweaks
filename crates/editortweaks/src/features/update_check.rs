//! Update reminder: at start, asks GitHub which Editor Tweaks release is the latest and, if it's
//! newer than this one, says so in one of the game's popups (`game::popup`) once the main menu
//! shows (a popup opened sooner is closed by the game as its screen changes). Confirm opens the releases page in the browser, Cancel reminds again
//! next start, and our "Don't remind me until next update" button remembers that release in
//! `EditorTweaks.ini`, so only a newer one reminds again.
//!
//! What goes out: one HTTPS GET to GitHub's API (`releases/latest` of the mod's repository,
//! `platform::https`, Windows' own TLS) naming us `editortweaks/<version>`; nothing else. Only the
//! answer's `tag_name` is read, and only as three numbers (`release_check`): no text of it is
//! shown or run, and the page opened is ours, never one from the answer. It runs on a thread of
//! its own, so a slow or missing connection never holds the game. Off with "Check for updates".

use std::cell::RefCell;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UClass;
use release_check::Version;

use super::{menu_buttons, popup_answers, settings_list, settings_menu};
use crate::framework::settings::{Note, Section, Setting, Toggle};
use crate::framework::timings;
use crate::game::kismet;
use crate::game::menu::WindowManager;
use crate::game::popup::Popups;
use crate::game::reflection;
use crate::game::weak_object::WeakObject;
use crate::platform::https;
use crate::platform::timer::ThreadTimer;
use crate::prelude::*;

pub static CHECK: Toggle = Toggle::new(
    "CheckForUpdates",
    "Check for updates",
    "At start, asks GitHub which Editor Tweaks release is the latest (only that is asked, nothing \
     about you is sent) and, if it's newer than yours, offers its download page. From the next start.",
    true,
);

/// The release the player asked not to be reminded of again (`vX.Y.Z`); empty for none.
pub static SKIPPED: Note = Note::new(
    "SkippedUpdate",
    "The release \"Don't remind me until next update\" was chosen for (vX.Y.Z), empty for none.",
);

pub static SETTINGS: [Section; 1] = [Section {
    title: "Updates",
    settings: &[Setting::Toggle(&CHECK)],
    unlisted: &[Setting::Note(&SKIPPED)],
}];

/// GitHub's API: the repository's latest release (not drafts or pre-releases).
const API_HOST: &str = "api.github.com";
const API_PATH: &str = "/repos/MrPerruche/editortweaks/releases/latest";
const API_HEADERS: &str =
    "Accept: application/vnd.github+json\r\nX-GitHub-Api-Version: 2022-11-28\r\n";
/// Who asks (GitHub's API wants a `User-Agent`).
const AGENT: &str = concat!("editortweaks/", env!("CARGO_PKG_VERSION"));
/// The answer is a few kilobytes (the release's notes and files); anything much larger isn't it.
const MAX_ANSWER: usize = 512 * 1024;
/// The page Confirm opens: GitHub sends it to the latest release's.
const RELEASES_PAGE: &str = "https://github.com/MrPerruche/editortweaks/releases/latest";

const SKIP_TEXT: &str = "Don't remind me until next update";
const SKIP_ICON: &str = "/Game/BrickRigs/UI/Textures/Icons/T_Cancel.T_Cancel";

/// How often the game thread looks for the answer and the main menu, and how long it waits for
/// the answer (the request itself times out sooner, `platform::https`). The main menu it waits for
/// all session: a player going straight into a map is reminded once back.
const TICK_MS: u32 = 250;
const GIVE_UP_AFTER: Duration = Duration::from_secs(60);
/// The main menu's root context (`settings_menu::menu_root`).
const MAIN_MENU: &str = "MainMenu";

pub struct UpdateCheck;

impl Feature for UpdateCheck {
    fn id(&self) -> &'static str {
        "update_check"
    }

    fn on_engine_init(&self) {
        if !CHECK.get() {
            log!("not checking for updates (CheckForUpdates is off)");
            return;
        }
        let Some(current) = Version::parse(env!("CARGO_PKG_VERSION")) else {
            log!("not checking for updates: this build's version isn't X.Y.Z");
            return;
        };
        let started = std::thread::Builder::new()
            .name("editortweaks update check".into())
            .spawn(move || {
                let answer = latest().map(|latest| (current, latest));
                *ANSWER.lock().unwrap_or_else(PoisonError::into_inner) = Some(answer);
            });
        if let Err(error) = started {
            log!("not checking for updates: no thread ({error})");
            return;
        }
        // On the game thread, as `ThreadTimer` needs.
        match ThreadTimer::start(TICK_MS, tick) {
            Some(timer) => TICKING.with_borrow_mut(|ticking| {
                *ticking = Some(Ticking {
                    _timer: timer,
                    since: Instant::now(),
                    waiting: None,
                });
            }),
            None => log!("not checking for updates: no timer"),
        }
    }
}

/// The latest release's version, asked from GitHub. Off the game thread: it waits for the answer.
fn latest() -> Result<Version, String> {
    let answer = https::get(AGENT, API_HOST, API_PATH, API_HEADERS, MAX_ANSWER)?;
    let answer = String::from_utf8_lossy(&answer);
    let tag = release_check::tag_name(&answer).ok_or("the answer has no tag_name")?;
    Version::parse(tag).ok_or_else(|| format!("the latest release's tag {tag:?} isn't vX.Y.Z"))
}

/// The check thread's answer (this version and the latest), once it has one.
static ANSWER: Mutex<Option<Result<(Version, Version), String>>> = Mutex::new(None);

/// While waiting for the answer and for the popup to open.
struct Ticking {
    _timer: ThreadTimer,
    since: Instant,
    /// Since when a newer release waits for the main menu, and whether that was told twice
    /// (when it began, and what showed instead after a while).
    waiting: Option<(Instant, bool)>,
}

/// After how long waiting for the main menu tells what shows instead (for when it never does).
const TELL_WAITING_AFTER: Duration = Duration::from_secs(15);

/// The reminder shown: its params, our button (if it was made), and the release it tells of.
struct Shown {
    params: WeakObject,
    skip: Option<WeakObject>,
    latest: Version,
}

/// The popups, and what tells the main menu shows; found when first needed.
struct Kit {
    popups: Popups,
    windows: WindowManager,
    /// `UMenuWidget`: the main menu's widget (and the in-game menu's).
    menu_class: *const UClass,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for Kit {}
unsafe impl Sync for Kit {}

static KIT: OnceLock<Result<Kit, String>> = OnceLock::new();

// Game thread only: the timer, the button clicks and the popups' confirming all run there.
thread_local! {
    static TICKING: RefCell<Option<Ticking>> = const { RefCell::new(None) };
    static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
}

fn kit() -> Result<&'static Kit, String> {
    KIT.get_or_init(|| {
        let popups = Popups::find()?;
        let windows = WindowManager::find()?;
        let menu_class =
            reflection::find_class("MenuWidget").ok_or("class MenuWidget not found")?;
        popup_answers::intercept(confirmed)?;
        menu_buttons::listen(clicked)?;
        Ok(Kit {
            popups,
            windows,
            menu_class,
        })
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// The thread timer's tick, on the game thread: once the answer is in, tells it; if there's a newer
/// release, waits for the main menu to show the reminder; then stops.
fn tick() {
    let _span = timings::span("update_check::tick");
    let answer = ANSWER
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let since = TICKING.with_borrow(|ticking| ticking.as_ref().map(|ticking| ticking.since));
    let Some(since) = since else { return };
    let Some(answer) = answer else {
        if since.elapsed() > GIVE_UP_AFTER {
            log!("no answer about updates in {} s", GIVE_UP_AFTER.as_secs());
            stop();
        }
        return;
    };
    let (current, latest) = match answer {
        Ok(versions) => versions,
        Err(reason) => {
            log!("couldn't check for updates: {reason}");
            stop();
            return;
        }
    };
    let skipped = Version::parse(&SKIPPED.get());
    let Some(newer) = release_check::to_remind(current, latest, skipped) else {
        match skipped {
            Some(skipped) if latest > current => {
                log!("{latest} is out, not reminded (skipped up to {skipped})")
            }
            _ => log!("up to date ({current}; latest release {latest})"),
        }
        stop();
        return;
    };
    let kit = match kit() {
        Ok(kit) => kit,
        Err(reason) => {
            log!("{newer} is out (this is {current}), but no reminder can show: {reason}");
            stop();
            return;
        }
    };
    // SAFETY: on the game thread (the timer runs there).
    if !unsafe { main_menu_shows(kit) } {
        tell_waiting(kit, current, newer);
        return;
    }
    match show(kit, current, newer) {
        Ok(()) => log!("{newer} is out (this is {current}): reminder shown"),
        Err(reason) => {
            log!("{newer} is out (this is {current}), but the reminder didn't open: {reason}")
        }
    }
    stop();
}

/// Tells that `newer` waits for the main menu (once), and after a while what shows instead.
fn tell_waiting(kit: &Kit, current: Version, newer: Version) {
    // Whether waiting just began, and whether it's time to tell what shows.
    let (began, late) = TICKING.with_borrow_mut(|ticking| {
        let Some(ticking) = ticking.as_mut() else {
            return (false, false);
        };
        let began = ticking.waiting.is_none();
        let (since, told) = ticking.waiting.get_or_insert((Instant::now(), false));
        let late = !*told && since.elapsed() > TELL_WAITING_AFTER;
        *told |= late;
        (began, late)
    });
    if began {
        log!("{newer} is out (this is {current}): the reminder waits for the main menu");
    }
    if late {
        // SAFETY: on the game thread; a live widget, its class's name.
        let shows = unsafe {
            kit.windows
                .active_widget()
                .map_or("nothing".to_owned(), |widget| {
                    reflection::name_text(reflection::struct_name(reflection::class_of(
                        widget.cast(),
                    )))
                })
        };
        log!(
            "still waiting for the main menu: the screen shows {shows}, the last menu's root is \"{}\"",
            settings_menu::menu_root()
        );
    }
}

/// Whether the main menu shows as the main screen, set up: the window manager's active widget is a
/// menu, the last one opened at its root `MainMenu` (in a world, the menu is `InGameMenu`'s, and
/// it closes; until the screen is set, the active widget is none or the intro's).
///
/// # Safety
/// Game thread only.
unsafe fn main_menu_shows(kit: &Kit) -> bool {
    // SAFETY: on the game thread; a live widget.
    let menu = unsafe {
        kit.windows
            .active_widget()
            .is_some_and(|widget| reflection::is_a(widget, kit.menu_class.cast()))
    };
    menu && settings_menu::menu_root() == MAIN_MENU
}

/// Stops the timer (from inside its own callback, which `ThreadTimer` allows).
fn stop() {
    TICKING.with_borrow_mut(Option::take);
}

/// Opens the reminder of `latest`: a question with Confirm and Cancel, and our button. An error
/// means no popup opened.
fn show(kit: &Kit, current: Version, latest: Version) -> Result<(), String> {
    // Everything made first: once the popup is open, nothing may fail.
    let icon = kismet::string_to_name(SKIP_ICON)?;
    let label = kismet::string_to_text(SKIP_TEXT)?;
    let question = match kismet::string_to_text(&format!(
        "Editor Tweaks {latest} is out (you have {current}). Confirm opens its download page."
    )) {
        Ok(question) => question,
        Err(reason) => {
            // SAFETY: our own text, not used after.
            unsafe { label.release() };
            return Err(reason);
        }
    };
    // SAFETY: on the game thread (the timer runs there); our texts are released once the popup
    // and its button hold their own copies.
    let (opened, added) = unsafe {
        let opened = kit.popups.ask(&question);
        question.release();
        let added = match &opened {
            Ok(params) => kit.popups.add_button(*params, &label, icon),
            Err(_) => Err(String::new()),
        };
        label.release();
        (opened, added)
    };
    let params = opened?;
    let skip = match added {
        // SAFETY: the live button just made.
        Ok(button) => Some(unsafe { WeakObject::new(button) }),
        Err(reason) => {
            // Still a reminder, only without its way to skip this release.
            log!("the reminder has no \"{SKIP_TEXT}\" button: {reason}");
            None
        }
    };
    SHOWN.with_borrow_mut(|shown| {
        *shown = Some(Shown {
            // SAFETY: the live params just made.
            params: unsafe { WeakObject::new(params) },
            skip,
            latest,
        });
    });
    Ok(())
}

/// Whether `params` are the reminder's (still shown).
fn is_shown(params: *mut UObjectBase) -> bool {
    SHOWN.with_borrow(|shown| {
        shown
            .as_ref()
            .is_some_and(|shown| !params.is_null() && shown.params.is(params))
    })
}

/// A popup is confirmed (`popup_answers`): ours opens the releases page, and the game closes it.
fn confirmed(container: *mut UObjectBase) -> bool {
    let Ok(kit) = kit() else { return false };
    // SAFETY: the live container being confirmed, on the game thread.
    let params = unsafe { kit.popups.params_of(container) };
    if !is_shown(params) {
        return false;
    }
    SHOWN.with_borrow_mut(Option::take);
    match kismet::launch_url(RELEASES_PAGE) {
        Ok(()) => log!("reminder confirmed: releases page opened"),
        Err(reason) => log!("reminder confirmed, but the releases page didn't open: {reason}"),
    }
    false
}

/// A menu button is clicked (`menu_buttons`): ours remembers the release not to remind about and
/// closes the reminder.
fn clicked(button: *mut UObjectBase) {
    let Ok(kit) = kit() else { return };
    let ours = SHOWN.with_borrow(|shown| {
        let shown = shown.as_ref()?;
        let skip = shown.skip.as_ref()?;
        skip.is(button)
            .then(|| Some((shown.params.get()?, shown.latest)))
            .flatten()
    });
    let Some((params, latest)) = ours else { return };
    // Buttons are pooled: only ours while our popup shows it.
    // SAFETY: on the game thread (menu clicks run there).
    let Some(container) = (unsafe { kit.popups.shows_button(params, button) }) else {
        return;
    };
    SHOWN.with_borrow_mut(Option::take);
    SKIPPED.set(&latest.to_string());
    settings_list::save();
    log!("not reminding about {latest} again");
    // SAFETY: on the game thread; the container shows our popup.
    unsafe { kit.popups.cancel(container) };
}
