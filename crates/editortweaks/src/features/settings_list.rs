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
//!
//! The game's own rows can be reworked too ([`rework`]): while the game lists its rows for a tag
//! asked for, each row it adds (`AddProperty`, hooked then) can stay, show its list as a dropdown,
//! give its place to a row of ours ([`row_named`], under the game's name), or go, its value shown by
//! a row of ours placed before. A row going hands the game the edit info of the row showing its
//! value: callers only write the row's description there (or nothing), which is put back after.
//!
//! Our number rows can show each value in its own unit (`Number::component_unit`): after such a
//! row updated its sliders (`number_rows`), the sliders that differ get their own value type.

use core::cell::RefCell;
use core::ffi::c_void;
use std::sync::{Mutex, Once, OnceLock, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;

use crate::features::{SETTINGS, number_rows};
use crate::framework::hook;
use crate::framework::settings::{self, Setting};
use crate::game::kismet;
use crate::game::property_reflection::{
    self, ADD_PROPERTY, AddPropertyFn, REFLECT_USER_SETTINGS, ReflectFn, Reflection, Row, Rows,
    Value,
};
use crate::game::property_widget::{self, PropertyWidgets};
use crate::game::reflection;
use crate::game::slider::{self, SetValueType};
use crate::game::slider_text::FString;
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

/// What becomes of one of the game's rows as the game lists it ([`rework`]).
pub enum Rework {
    /// The game's row, as it is.
    Keep,
    /// The game's row, its list shown as a dropdown.
    Dropdown,
    /// A row of ours in its place.
    Replace(&'static Row),
    /// No row: the row of ours given, placed before (by [`Rework::Replace`]), shows its value. If
    /// that row isn't listed, the game's row stays.
    MergeInto(&'static Row),
}

/// Tells what becomes of the game's row named `name` (the field it edits, e.g.
/// `"bEditorMirrorAxisEnabled"`; a struct's fields by their own name, e.g. `"SunAngle"`).
pub type ReworkFn = fn(name: &str) -> Rework;

/// The game's rows reworked wherever a panel asks for `tag`.
struct Reworker {
    tag: FName,
    rework: ReworkFn,
}

static REWORKERS: Mutex<Vec<Reworker>> = Mutex::new(Vec::new());
static ADD_ORIGINAL: OnceLock<AddPropertyFn> = OnceLock::new();
/// Whether `AddProperty` is hooked, or why not.
static ADD_HOOKED: OnceLock<Result<(), String>> = OnceLock::new();

/// The game listing its rows with some reworked, on this thread.
struct Reworking {
    reworks: Vec<ReworkFn>,
    /// Our rows placed instead of the game's, with their edit infos.
    placed: Vec<(&'static Row, *mut u8)>,
    /// Edit infos of the game's lists to show as dropdowns, once the game set them.
    dropdowns: Vec<*mut u8>,
}

thread_local! {
    static REWORKING: RefCell<Option<Reworking>> = const { RefCell::new(None) };
}

/// Our number rows' sliders' own units: found on first use (after engine init), `None` if missing.
struct Units {
    widgets: PropertyWidgets,
    set_type: SetValueType,
}

/// Finds the game's code for rows and hooks the list, once (later calls give the first result).
/// Works before engine init.
pub fn install() -> Result<(), String> {
    HOOKED.get_or_init(hook_list).clone()
}

/// Reworks the game's rows wherever a panel asks the settings list for `tag`, as `rework` tells.
/// After engine init.
pub fn rework(tag: &str, rework: ReworkFn) -> Result<(), String> {
    install()?;
    ADD_HOOKED.get_or_init(hook_add_property).clone()?;
    REWORKERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(Reworker {
            tag: kismet::string_to_name(tag)?,
            rework,
        });
    Ok(())
}

fn hook_add_property() -> Result<(), String> {
    let target = ADD_PROPERTY.resolve()?;
    // SAFETY: `target` is the start of AddProperty, which `add_property` matches.
    unsafe {
        hook::install_typed(
            ADD_PROPERTY.name,
            target,
            add_property as AddPropertyFn,
            &ADD_ORIGINAL,
        )
    }
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
    }?;
    // Without it, a number row of ours shows every value in its first one's unit.
    if let Err(reason) = number_rows::listen(own_units) {
        log!("number rows show their values in one unit: {reason}");
    }
    Ok(())
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

/// A row standing in for one of the game's ([`Rework::Replace`]): its property is called `property`,
/// the name of the game's, so that the game reacts to it changing as to its own row; it saves and
/// tells what reacts to the change itself. The row's functions write the game's values. After
/// engine init.
pub fn row_named(
    property: &str,
    name: &str,
    description: &str,
    value: Value,
) -> Result<Row, String> {
    rows()?.row_named(property, name, description, value, || {})
}

/// A row for one of the mod's settings, saved to `EditorTweaks.ini` whenever a panel changes it.
/// After engine init.
pub fn setting_row(setting: Setting) -> Result<Row, String> {
    rows()?.setting_row(setting, save)
}

/// A setting changed in a panel: saves them all. Also for rows of our own that change a setting.
pub fn save() {
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
        list_reworked(reflection, || original(user_settings, reflection));
        for listing in listings {
            if listing.replacing.is_none() && shown(listing) {
                list(rows, reflection, listing);
            }
        }
    }
}

/// Runs `list`, the game listing its rows into `reflection`, with the reworks of the tags it shows
/// applied to them.
///
/// # Safety
/// `reflection` must be the reflection the game is filling. Game thread only.
unsafe fn list_reworked(reflection: *const Reflection, list: impl FnOnce()) {
    let reworks: Vec<ReworkFn> = REWORKERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        // SAFETY: per the contract; read before the game goes on filling it.
        .filter(|reworker| unsafe { (*reflection).shows(&reworker.tag) })
        .map(|reworker| reworker.rework)
        .collect();
    if reworks.is_empty() || ADD_ORIGINAL.get().is_none() {
        list();
        return;
    }
    let previous = REWORKING.replace(Some(Reworking {
        reworks,
        placed: Vec::new(),
        dropdowns: Vec::new(),
    }));
    list();
    let done = REWORKING.replace(previous);
    let Some(done) = done else {
        return;
    };
    // SAFETY: the edit infos the game just made for this reflection, which holds them; the game's
    // own code wrote over ours, so ours go back.
    unsafe {
        for (row, info) in done.placed {
            property_reflection::describe(info, row);
        }
        for info in done.dropdowns {
            property_reflection::show_as_dropdown(info);
        }
    }
}

/// `AddProperty`: while the game lists rows to rework ([`list_reworked`]), each of its rows is
/// reworked as the first rework with something to say tells.
unsafe extern "C" fn add_property(
    reflection: *mut Reflection,
    property: *const c_void,
    name: *const FString,
    display_name: *const Text,
) -> *mut u8 {
    let original = *ADD_ORIGINAL.get().expect("hook recorded before use");
    // SAFETY: forwards the caller's arguments unchanged.
    let add = || unsafe { original(reflection, property, name, display_name) };
    let reworks = REWORKING.with_borrow(|state| state.as_ref().map(|state| state.reworks.clone()));
    let Some(reworks) = reworks else {
        return add();
    };
    // SAFETY: the game passes the field's name as a live string.
    let field = unsafe { (*name).to_string() };
    let rework = reworks
        .iter()
        .map(|rework| rework(&field))
        .find(|rework| !matches!(rework, Rework::Keep))
        .unwrap_or(Rework::Keep);
    match rework {
        Rework::Keep => add(),
        Rework::Dropdown => {
            let info = add();
            if !info.is_null() {
                with_reworking(|state| state.dropdowns.push(info));
            }
            info
        }
        Rework::Replace(row) => {
            let Ok(rows) = rows() else {
                return add();
            };
            // Our row goes through this hook too, under the game's name: not reworked again.
            let state = REWORKING.take();
            // SAFETY: the reflection the game is filling, in its current category.
            let info = unsafe { rows.add_row(reflection, row) };
            REWORKING.set(state);
            if info.is_null() {
                return add();
            }
            with_reworking(|state| state.placed.push((row, info)));
            info
        }
        Rework::MergeInto(row) => {
            let placed = REWORKING.with_borrow(|state| {
                state.as_ref().and_then(|state| {
                    state
                        .placed
                        .iter()
                        .find(|(placed, _)| core::ptr::eq(*placed, row))
                        .map(|&(_, info)| info)
                })
            });
            placed.unwrap_or_else(add)
        }
    }
}

fn with_reworking(change: impl FnOnce(&mut Reworking)) {
    REWORKING.with_borrow_mut(|state| {
        if let Some(state) = state {
            change(state);
        }
    });
}

/// A numeric row `row` updates (`number_rows`): if it's one of ours whose values differ in unit,
/// its sliders get their own units once filled.
unsafe fn own_units(row: *mut c_void, update: &mut dyn FnMut()) {
    static UNITS: OnceLock<Option<Units>> = OnceLock::new();
    let Some(number) = property_reflection::noting_number_shown(update) else {
        return;
    };
    let units = UNITS.get_or_init(|| {
        let found = || -> Result<Units, String> {
            Ok(Units {
                widgets: property_widget::property_widgets()?,
                set_type: slider::set_value_type()?,
            })
        };
        found()
            .map_err(|reason| log!("number rows show their values in one unit: {reason}"))
            .ok()
    });
    let Some(units) = units else {
        return;
    };
    // SAFETY: a live row on the game thread (per `number_rows`); its sliders are live
    // `UBrickSliderWidget`s, copied out since setting a type runs game code.
    unsafe {
        let sliders = units.widgets.sliders(row.cast::<UObjectBase>()).to_vec();
        for (index, slider) in sliders.into_iter().enumerate() {
            let unit = number.component_unit(index);
            if unit != number.unit() {
                units.set_type.call(slider.cast_mut().cast(), unit as u8);
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
