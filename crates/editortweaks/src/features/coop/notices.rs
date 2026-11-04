//! What coop tells the player: who came, went or was refused in the game's chat ([`say`]), what
//! waits for an answer in a notice on the HUD ([`tell`]), and what ends their session in a popup
//! ([`alert`]), which shows over menus too. The HUD shows no notification while a menu is open (or
//! the player hid it): notices wait for it, for [`WAIT_FOR`], and show together then. A single-player
//! game has no chat: what's said there is a notice instead. All are shown by [`show`], once a tick,
//! outside the driver.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::Game;
use super::dialogs;
use super::log::coop_log;
use crate::game::chat::Tone;

/// How long a notice waits for the HUD before it's dropped (it's in the coop log anyway).
const WAIT_FOR: Duration = Duration::from_secs(60);
/// Waiting notices shown together at most: the latest.
const AT_ONCE: usize = 3;

struct Waiting {
    said: Vec<(Tone, String)>,
    notices: Vec<(String, Instant)>,
    alerts: Vec<String>,
    /// Why the chat didn't show what was said, logged once until it does.
    no_chat: Option<String>,
    /// Why the HUD didn't show the waiting notices, logged once until it does.
    blocked: Option<String>,
}

static WAITING: Mutex<Waiting> = Mutex::new(Waiting {
    said: Vec::new(),
    notices: Vec::new(),
    alerts: Vec::new(),
    no_chat: None,
    blocked: None,
});

fn waiting() -> MutexGuard<'static, Waiting> {
    WAITING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Shows `text` in the game's chat, to this player only (and logs it); see the module's doc.
pub fn say(tone: Tone, text: &str) {
    coop_log!("said in the chat: {text}");
    waiting().said.push((tone, text.to_owned()));
}

/// Shows `text` on the HUD, as soon as it can (and logs it).
pub fn tell(text: &str) {
    coop_log!("told the player: {text}");
    let mut waiting = waiting();
    waiting.notices.retain(|(waits, _)| waits != text);
    waiting.notices.push((text.to_owned(), Instant::now()));
}

/// Shows `text` in a popup the player confirms (on the HUD if popups aren't found), for what they
/// must not miss: their session ended or couldn't start.
pub fn alert(text: &str) {
    coop_log!("alerted the player: {text}");
    waiting().alerts.push(text.to_owned());
}

/// What was said goes to the chat, or to the notices if there's no chat to show it.
fn show_said(game: &Game) {
    let said = std::mem::take(&mut waiting().said);
    for (tone, text) in said {
        let shown = match &game.chat {
            // SAFETY: game thread ([`show`]'s contract).
            Ok(chat) => unsafe { chat.say(tone, &text) },
            Err(reason) => Err(reason.clone()),
        };
        let mut waiting = waiting();
        match shown {
            Ok(()) => waiting.no_chat = None,
            Err(reason) => {
                if waiting.no_chat.as_ref() != Some(&reason) {
                    coop_log!("  on the HUD instead of the chat: {reason}");
                    waiting.no_chat = Some(reason);
                }
                waiting.notices.retain(|(waits, _)| *waits != text);
                waiting.notices.push((text, Instant::now()));
            }
        }
    }
}

/// Every tick, on the game thread: says what's to be said in the chat, opens the alerts, and shows
/// the waiting notices if the HUD can.
pub fn show(game: &Game) {
    show_said(game);
    let alerts = std::mem::take(&mut waiting().alerts);
    for alert in alerts {
        if let Err(reason) = dialogs::inform(&alert) {
            coop_log!("  no popup ({reason}): on the HUD instead");
            let mut waiting = waiting();
            waiting.notices.retain(|(waits, _)| *waits != alert);
            waiting.notices.push((alert, Instant::now()));
        }
    }
    let text = {
        let mut waiting = waiting();
        let before = waiting.notices.len();
        waiting
            .notices
            .retain(|(_, since)| since.elapsed() < WAIT_FOR);
        if waiting.notices.len() < before {
            coop_log!("  {} notice(s) never shown", before - waiting.notices.len());
        }
        if waiting.notices.is_empty() {
            return;
        }
        let texts: Vec<&str> = waiting
            .notices
            .iter()
            .map(|(text, _)| text.as_str())
            .collect();
        together(&texts)
    };
    let Some(notes) = &game.notes else {
        waiting().notices.clear();
        return;
    };
    // SAFETY: game thread (the editor ticks there); the overlay is live while the HUD shows.
    let shown = unsafe {
        match notes.overlay() {
            Some(overlay) => notes.show_text(overlay, "editortweaks_coop", &text),
            None => Err("the HUD doesn't show (a menu does?)".into()),
        }
    };
    let mut waiting = waiting();
    match shown {
        Ok(()) => {
            waiting.notices.clear();
            waiting.blocked = None;
        }
        Err(reason) => {
            if waiting.blocked.as_ref() != Some(&reason) {
                coop_log!("  notices wait: {reason}");
                waiting.blocked = Some(reason);
            }
        }
    }
}

/// Notices shown as one: the latest [`AT_ONCE`], with "Coop:" once.
fn together(texts: &[&str]) -> String {
    if let [text] = texts {
        return (*text).to_owned();
    }
    let latest = &texts[texts.len().saturating_sub(AT_ONCE)..];
    let parts: Vec<&str> = latest
        .iter()
        .map(|text| text.strip_prefix("Coop: ").unwrap_or(text))
        .collect();
    let more = match texts.len() - latest.len() {
        0 => String::new(),
        n => format!(" (and {n} earlier)"),
    };
    format!("Coop: {}{more}", parts.join("; "))
}
