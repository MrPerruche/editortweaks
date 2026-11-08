//! The coop page's dialogs, the game's own popups (`game::popup`): questions with Confirm and
//! Cancel ("Stop hosting?"), and the game's ban dialog for a member (with its Player ID if they're
//! in our match). Confirming a question requests what it asked about; confirming a ban bans them
//! from our coop sessions only: the dialog is closed as cancelled, so the game never kicks or bans
//! anyone from the match.

use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use ::coop::PeerId;
use brickrust::ue::coreuobject::UObjectBase;

use super::log::coop_log;
use super::ui::{self, Request};
use crate::features::popup_answers;
use crate::framework::timings;
use crate::game::kismet;
use crate::game::match_players::MatchPlayers;
use crate::game::popup::{HiddenButton, Popups};
use crate::game::weak_object::WeakObject;

struct Kit {
    popups: Popups,
    /// The match's players, for their ids (or the ban dialog shows none).
    players: Option<MatchPlayers>,
}

static KIT: OnceLock<Result<Kit, String>> = OnceLock::new();

/// The dialogs open. Game thread only; never locked while calling the game.
struct State {
    /// Questions shown: their params, and what confirming requests.
    asked: Vec<(WeakObject, Request)>,
    banning: Option<Banning>,
    /// The waiting popup shown ([`waiting`]), its hidden Confirm (shown again once it closes),
    /// and whether one showed since the wait began.
    waiting: Option<WeakObject>,
    hidden: Option<HiddenButton>,
    waited: bool,
}

/// The ban dialog shown.
struct Banning {
    container: WeakObject,
    params: WeakObject,
    peer: PeerId,
    /// Whether we enable its Confirm (the game only does for a player of the match).
    enable: bool,
}

// SAFETY: weak object references, only used on the game thread.
unsafe impl Send for State {}

static STATE: Mutex<State> = Mutex::new(State {
    asked: Vec::new(),
    banning: None,
    waiting: None,
    hidden: None,
    waited: false,
});

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Finds the popups and listens to their confirming, once. After engine init.
pub fn start() -> Result<(), String> {
    let kit = KIT.get_or_init(|| {
        let popups = Popups::find()?;
        popup_answers::intercept(confirmed)?;
        let players = MatchPlayers::find()
            .inspect_err(|reason| coop_log!("the ban dialog shows no player ids: {reason}"))
            .ok();
        Ok(Kit { popups, players })
    });
    kit.as_ref().map(|_| ()).map_err(Clone::clone)
}

fn kit() -> Option<&'static Kit> {
    KIT.get().and_then(|kit| kit.as_ref().ok())
}

/// Whether questions show in the game's popups (else the coop page asks them itself).
pub fn available() -> bool {
    kit().is_some()
}

/// The match now (its game state), if known: bans last until it ends.
pub fn current_match() -> Option<WeakObject> {
    let players = kit()?.players.as_ref()?;
    // SAFETY: on the game thread (coop's tick and the menus run there); a live game state.
    unsafe { players.game_state().map(|state| WeakObject::new(state)) }
}

/// The match's players (SteamID, name), us too; none without a match, or if they can't be read.
pub fn match_players() -> Vec<(PeerId, String)> {
    let Some(players) = kit().and_then(|kit| kit.players.as_ref()) else {
        return Vec::new();
    };
    // SAFETY: on the game thread (coop's tick runs there).
    unsafe { players.players() }
}

/// Asks `question` in a popup over whatever shows; confirming requests `request`. Game thread only.
pub fn ask(question: &str, request: Request) {
    let Some(kit) = kit() else {
        return;
    };
    let shown = kismet::string_to_text(question).and_then(|text| {
        // SAFETY: on the game thread; our text is released once the popup holds its own copy.
        unsafe {
            let shown = kit.popups.ask(&text);
            text.release();
            shown
        }
    });
    match shown {
        Ok(params) => {
            coop_log!("asked: \"{question}\"");
            let mut state = state();
            state.asked.retain(|(params, _)| params.get().is_some());
            // SAFETY: the live params just made.
            state
                .asked
                .push((unsafe { WeakObject::new(params) }, request));
        }
        Err(reason) => coop_log!("\"{question}\" not asked: {reason}"),
    }
}

/// Tells `text` in a popup with only Confirm, over whatever shows. Game thread only.
pub fn inform(text: &str) -> Result<(), String> {
    let kit = kit().ok_or("no popups")?;
    kismet::string_to_text(text).and_then(|text| {
        // SAFETY: on the game thread; our text is released once the popup holds its own copy.
        unsafe {
            let shown = kit.popups.inform(&text);
            text.release();
            shown.map(|_| ())
        }
    })
}

/// Every tick: while `text` is given, shows it in a popup with only Cancel (once per wait); once
/// it isn't, closes the popup. Whether the player closed it (to stop waiting). Game thread only.
pub fn waiting(text: Option<&str>) -> bool {
    let Some(kit) = kit() else {
        return false;
    };
    let shown = state().waiting.as_ref().and_then(WeakObject::get);
    // SAFETY: on the game thread; live params.
    let container = shown.and_then(|params| unsafe { kit.popups.container_of(params) });
    let Some(text) = text else {
        let mut state = state();
        state.waited = false;
        state.waiting = None;
        let hidden = state.hidden.take();
        drop(state);
        // SAFETY: a live container showing our popup; then its button, on the game thread.
        unsafe {
            if let Some(container) = container {
                kit.popups.cancel(container);
            }
            if let Some(hidden) = hidden {
                kit.popups.unhide(hidden);
            }
        }
        return false;
    };
    if state().waiting.is_some() {
        if container.is_some() {
            return false;
        }
        let hidden = {
            let mut state = state();
            state.waiting = None;
            state.hidden.take()
        };
        if let Some(hidden) = hidden {
            // SAFETY: on the game thread.
            unsafe { kit.popups.unhide(hidden) };
        }
        return true;
    }
    if state().waited {
        return false;
    }
    state().waited = true;
    let shown = kismet::string_to_text(text).and_then(|text| {
        // SAFETY: on the game thread; our text is released once the popup holds its own copy.
        unsafe {
            let shown = kit.popups.wait(&text);
            text.release();
            shown
        }
    });
    match shown {
        Ok((params, hidden)) => {
            let mut state = state();
            // SAFETY: the live params just made.
            state.waiting = Some(unsafe { WeakObject::new(params) });
            state.hidden = hidden;
        }
        Err(reason) => coop_log!("\"{text}\" not shown: {reason}"),
    }
    false
}

/// Opens the game's ban dialog for the member `peer`, called `name`.
///
/// # Safety
/// `controller` must be the local `ABasePlayerController`. Game thread only.
pub unsafe fn open_ban(controller: *mut UObjectBase, peer: PeerId, name: &str) {
    let Some(kit) = kit() else {
        return;
    };
    // SAFETY: per the contract; the id is read during the call only, while its player lives.
    let opened = unsafe {
        let id = kit
            .players
            .as_ref()
            .and_then(|players| players.net_id(peer));
        kit.popups
            .open_ban(controller, id, name)
            .map(|shown| (shown, id.is_some()))
    };
    match opened {
        Ok(((container, params), in_match)) => {
            coop_log!(
                "ban dialog for {name}{}",
                if in_match { " (in the match)" } else { "" }
            );
            // SAFETY: the live container and params just found.
            state().banning = Some(unsafe {
                Banning {
                    container: WeakObject::new(container),
                    params: WeakObject::new(params),
                    peer,
                    enable: !in_match,
                }
            });
        }
        Err(reason) => coop_log!("no ban dialog for {name}: {reason}"),
    }
}

/// Every tick: keeps the ban dialog's Confirm enabled for a member without a player id.
pub fn tick() {
    let Some(kit) = kit() else {
        return;
    };
    let banning = state().banning.as_ref().map(|banning| {
        (
            banning.container.get(),
            banning.params.get(),
            banning.enable,
        )
    });
    let Some((container, params, enable)) = banning else {
        return;
    };
    // SAFETY: on the game thread; live objects.
    let open = match (container, params) {
        (Some(container), Some(params)) => unsafe {
            let open = kit.popups.shows(container, params);
            if open && enable {
                kit.popups.enable_ban_confirm(container);
            }
            open
        },
        _ => false,
    };
    if !open {
        state().banning = None;
    }
}

/// A popup is confirmed (`popup_answers`): one of our questions requests what it asked about (the
/// game then closes it); our ban dialog bans from coop and closes as cancelled (handled: `true`);
/// any other popup goes on as the game wants.
fn confirmed(container: *mut UObjectBase) -> bool {
    let _span = timings::span("coop::confirmed");
    let Some(kit) = kit() else {
        return false;
    };
    // SAFETY: the live container being confirmed, on the game thread.
    let params = unsafe { kit.popups.params_of(container) };
    let question = {
        let mut state = state();
        state
            .asked
            .iter()
            .position(|(ours, _)| !params.is_null() && ours.is(params))
            .map(|at| state.asked.swap_remove(at).1)
    };
    if let Some(request) = question {
        coop_log!("a question was confirmed");
        ui::request(request);
        return false;
    }
    let banning = {
        let mut state = state();
        let ours = state
            .banning
            .as_ref()
            .is_some_and(|banning| !params.is_null() && banning.params.is(params));
        if ours { state.banning.take() } else { None }
    };
    let Some(banning) = banning else {
        return false;
    };
    // SAFETY: our live ban params; the values are read before the dialog closes.
    unsafe {
        if let Some(ban) = kit.popups.ban(params) {
            ui::request(Request::Ban {
                peer: banning.peer,
                name: ban.name,
                reason: ban.reason,
                seconds: ban.seconds,
            });
        }
        kit.popups.cancel(container);
    }
    true
}
