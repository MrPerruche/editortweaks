//! The game's settings list, shared by the features that add rows of their own to it. Not a feature
//! itself: the loader hooks a function only once, so the features can't each hook it.
//!
//! `UBrickUserSettings::ReflectBrickProperties` lists the game's settings of the categories
//! (gameplay tags) a panel asks for: a settings page of the options asks for its own
//! (`Properties.Settings.Gameplay`, ...), the editor's side panel for `Properties.Editor` when
//! nothing is selected. Once the game listed its rows, each listing of ours ([`add`]) whose tag the
//! panel asked for adds its sections, in the order they were added. A listing added by [`replace`]
//! lists its sections instead of all that, while it says so. Rows are made with [`row`] and
//! [`setting_row`] (`game::property_reflection`). Each new set of tags the list is asked for is
//! logged once, to check the tags after a game update.
//!
//! Sections titled alike show as one, whichever listings add them: the game puts rows under an
//! earlier section only if its title is the very same text (`AddCategory` compares them with
//! `FText::IdenticalTo`, game 0x10A18B0: the same text data or display string, not equal strings),
//! so each title is made once ([`title`]).

use core::ffi::c_void;
use std::sync::{Mutex, Once, OnceLock, PoisonError};

use brickrust::ue::fname::FName;

use crate::features::SETTINGS;
use crate::framework::hook;
use crate::framework::settings::{self, Setting};
use crate::game::kismet;
use crate::game::property_reflection::{
    REFLECT_USER_SETTINGS, ReflectFn, Reflection, Row, Rows, Value,
};
use crate::game::reflection;
use crate::game::text::Text;
use crate::prelude::*;

/// A section of rows: its title, then its rows.
pub type Section = (&'static str, Vec<Row>);

/// Sections shown wherever a panel asks for `tag`.
struct Listing {
    tag: FName,
    /// The tag as given, for the log.
    tag_text: String,
    sections: Vec<(&'static Title, Vec<Row>)>,
    /// For a listing added by [`replace`]: whether it replaces the panel's rows now (else it isn't
    /// listed).
    replacing: Option<fn() -> bool>,
    /// Logs the first time it's shown.
    shown: Once,
}

/// A section title's text, made once per title.
struct Title {
    name: &'static str,
    text: Text,
}

// SAFETY: names, texts and rows kept for the whole game; only used on the game thread.
unsafe impl Send for Listing {}
unsafe impl Sync for Listing {}
unsafe impl Send for Title {}
unsafe impl Sync for Title {}

static ROWS: OnceLock<Result<Rows, String>> = OnceLock::new();
static ORIGINAL: OnceLock<ReflectFn> = OnceLock::new();
/// Whether the list is hooked, or why not.
static HOOKED: OnceLock<Result<(), String>> = OnceLock::new();
/// Kept for the whole game: the game holds references to the rows' properties.
static LISTINGS: Mutex<Vec<&'static Listing>> = Mutex::new(Vec::new());
/// Kept for the whole game, as the listings using them.
static TITLES: Mutex<Vec<&'static Title>> = Mutex::new(Vec::new());
/// The sets of tags the list was asked for (by their names' indices), each logged once.
static ASKED: Mutex<Vec<Vec<(u32, u32)>>> = Mutex::new(Vec::new());

/// Finds the game's code for rows and hooks the list, once (later calls give the first result).
/// Works before engine init.
pub fn install() -> Result<(), String> {
    HOOKED.get_or_init(hook_list).clone()
}

fn rows() -> Result<&'static Rows, String> {
    ROWS.get_or_init(Rows::find).as_ref().map_err(Clone::clone)
}

fn hook_list() -> Result<(), String> {
    rows()?;
    let target = REFLECT_USER_SETTINGS.resolve()?;
    // SAFETY: `target` is the start of ReflectBrickProperties, which `reflect` matches.
    unsafe {
        hook::install_typed(
            REFLECT_USER_SETTINGS.name,
            target,
            reflect as ReflectFn,
            &ORIGINAL,
        )
    }
}

/// Lists `sections` wherever a panel asks the settings list for `tag`, after the game's rows and the
/// sections added before. After engine init.
pub fn add(tag: &str, sections: Vec<Section>) -> Result<(), String> {
    add_listing(tag, sections, None)
}

/// Lists `sections` instead of everything else (the game's rows and the other listings') wherever
/// a panel asks the settings list for `tag` while `when` says so, e.g. the editor's side panel
/// showing something selected the way it shows a selected brick. After engine init.
pub fn replace(tag: &str, sections: Vec<Section>, when: fn() -> bool) -> Result<(), String> {
    add_listing(tag, sections, Some(when))
}

fn add_listing(
    tag: &str,
    sections: Vec<Section>,
    replacing: Option<fn() -> bool>,
) -> Result<(), String> {
    install()?;
    let sections = sections
        .into_iter()
        .map(|(name, rows)| Ok((title(name)?, rows)))
        .collect::<Result<_, String>>()?;
    let listing = Listing {
        tag: kismet::string_to_name(tag)?,
        tag_text: tag.to_owned(),
        sections,
        replacing,
        shown: Once::new(),
    };
    LISTINGS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(Box::leak(Box::new(listing)));
    Ok(())
}

/// The text of the section title `name`, the same for every listing.
fn title(name: &'static str) -> Result<&'static Title, String> {
    let mut titles = TITLES.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(&known) = titles.iter().find(|title| title.name == name) {
        return Ok(known);
    }
    let title: &'static Title = Box::leak(Box::new(Title {
        name,
        text: kismet::string_to_text(name)?,
    }));
    titles.push(title);
    Ok(title)
}

/// A row called `name` (`description` is its tooltip) showing a feature's own `value`; `key` tells
/// it from our other rows. After engine init.
pub fn row(key: &str, name: &str, description: &str, value: Value) -> Result<Row, String> {
    rows()?.row(key, name, description, value, || {})
}

/// A row for one of the mod's settings, saved to `EditorTweaks.ini` whenever a panel changes it.
/// After engine init.
pub fn setting_row(setting: Setting) -> Result<Row, String> {
    rows()?.setting_row(setting, save)
}

/// A setting changed in a panel: saves them all.
fn save() {
    if let Err(reason) = settings::save(SETTINGS) {
        log!("settings not saved: {reason}");
    }
}

/// The game's settings list: adds the sections of each listing whose tag the panel asks for, or
/// only those of a listing replacing them.
unsafe extern "C" fn reflect(user_settings: *const c_void, reflection: *mut Reflection) {
    let Some(original) = ORIGINAL.get() else {
        return;
    };
    let rows = match rows() {
        Ok(rows) => rows,
        Err(_) => {
            // SAFETY: forwards the caller's arguments.
            unsafe { original(user_settings, reflection) };
            return;
        }
    };
    // Copied out, so that a feature may add a listing meanwhile without a deadlock.
    let listings = LISTINGS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    // SAFETY: the reflection the game is filling, on the game thread. A reflection saving or
    // loading the settings shows no tag, so it's never replaced.
    unsafe {
        log_asked(&*reflection);
        let shown = |listing: &Listing| (*reflection).shows(&listing.tag);
        let replacing = listings
            .iter()
            .copied()
            .find(|&listing| shown(listing) && listing.replacing.is_some_and(|when| when()));
        if let Some(listing) = replacing {
            list(rows, reflection, listing);
            return;
        }
        original(user_settings, reflection);
        for listing in listings {
            if listing.replacing.is_none() && shown(listing) {
                list(rows, reflection, listing);
            }
        }
    }
}

/// Adds the sections of `listing` (their rows listed now) to `reflection`.
///
/// # Safety
/// `reflection` must be the reflection the game is filling. Game thread only.
unsafe fn list(rows: &Rows, reflection: *mut Reflection, listing: &Listing) {
    for (title, section) in &listing.sections {
        let listed: Vec<&Row> = section.iter().filter(|row| row.is_listed()).collect();
        if listed.is_empty() {
            continue;
        }
        // SAFETY: per the contract.
        unsafe {
            rows.add_category(reflection, &title.text);
            for row in listed {
                rows.add_row(reflection, row);
            }
        }
    }
    listing
        .shown
        .call_once(|| log!("rows listed for {}", listing.tag_text));
}

/// Logs the tags `reflection` asks for, the first time the list is asked for them.
///
/// # Safety
/// `reflection` must be the reflection the game is filling. Game thread only.
unsafe fn log_asked(reflection: &Reflection) {
    let tags = reflection.shown_categories();
    if tags.is_empty() {
        return;
    }
    let key: Vec<(u32, u32)> = tags
        .iter()
        .map(|tag| (tag.comparison_index, tag.number))
        .collect();
    let mut asked = ASKED.lock().unwrap_or_else(PoisonError::into_inner);
    if asked.contains(&key) {
        return;
    }
    asked.push(key);
    // SAFETY: the reflection's tags are valid names.
    let names: Vec<String> = tags
        .iter()
        .map(|tag| unsafe { reflection::name_text(tag) })
        .collect();
    log!("the settings list was asked for {names:?}");
}
