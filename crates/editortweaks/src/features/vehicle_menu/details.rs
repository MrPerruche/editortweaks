//! The selected folder in the vehicle menu's details (the right column), as a selected vehicle
//! shows its own: its name in the header in the accent color, its vehicles and subfolders in the
//! header's boxes (where a vehicle shows its size, weight, ...), then rows with the name on the
//! left and the value on the right: Rename (a text box, typed to rename it), when it was made, when
//! its newest vehicle was made and when one was last changed.
//!
//! The game fills the details from the browser while it has no row selected (`UPagedListWidget::
//! UpdateSelectedEntryDetails`: the header says "Nothing selected", then the panel lists the
//! browser's rows through the hooked `ReflectBrickProperties`: `panel`), which is the case with a
//! folder selected: the game never selects a tile. Our rows replace the browser's there, and the
//! header gets the folder (`game::entry_header`). The details are filled again when the selection
//! changes (`tiles::show`).
//!
//! The name box sends what's typed at every keystroke: the folder is renamed as soon as that's a
//! name it can take (not empty, not a sibling's, ...), its tile and the header following without
//! the page being built again (`tiles::renamed`), which would take the typing away. The box keeps
//! showing what was typed (a name not taken, spaces at the ends) until the folder is unselected.

use std::sync::OnceLock;

use vehicle_folders::{FolderPath, MAX_NAME_CHARS, Newest, ViewMode, newest_in};

use super::tiles::{self, Tile};
use super::{BROWSERS, State, save, shows_folders, view_mode, with_state};
use crate::features::settings_list;
use crate::framework::clock;
use crate::game::entry_header::EntryHeaders;
use crate::game::kismet::Dates;
use crate::game::property_reflection::{Editable, Reflection, Row, Value};
use crate::game::text::Text;
use crate::prelude::*;

/// The panel's rows for a folder, and their untitled section (kept for the whole game: the game
/// holds references to them).
struct Rows {
    section: Text,
    rows: Vec<Row>,
}

// SAFETY: game texts and rows, only used on the game thread.
unsafe impl Send for Rows {}
unsafe impl Sync for Rows {}

static ROWS: OnceLock<Rows> = OnceLock::new();

/// The header's parts, if found.
static HEADERS: OnceLock<EntryHeaders> = OnceLock::new();

/// Dates as the game shows a vehicle's, if found.
static DATES: OnceLock<Dates> = OnceLock::new();

/// What a time shows while it isn't known (a folder made before the mod noted times, a folder
/// without vehicles).
const UNKNOWN: &str = "-";

/// The name box.
struct FolderName;

impl Editable for FolderName {
    fn text(&self) -> String {
        with_state(|state| {
            let path = selected_folder(state)?;
            Some(match &state.name_draft {
                Some((drafted, typed)) if *drafted == path => typed.clone(),
                _ => path.name().to_owned(),
            })
        })
        .unwrap_or_default()
    }

    fn set_text(&self, text: &str) -> bool {
        rename(text);
        true
    }

    fn max_chars(&self) -> usize {
        MAX_NAME_CHARS
    }
}

/// A fact about the selected folder, shown read-only.
struct Fact(fn(&State, &FolderPath) -> String);

impl Editable for Fact {
    fn text(&self) -> String {
        with_state(|state| selected_folder(state).map(|path| (self.0)(state, &path)))
            .unwrap_or_default()
    }

    fn set_text(&self, _text: &str) -> bool {
        false
    }

    fn max_chars(&self) -> usize {
        // As long as it needs: never typed into.
        1024
    }

    fn read_only(&self) -> bool {
        true
    }
}

static NAME: FolderName = FolderName;

static CREATED: Fact = Fact(|state, path| date(state.tree().folder_created(path)));

static NEWEST_VEHICLE: Fact = Fact(|state, path| date(newest(state, path).created));

static LAST_UPDATE: Fact = Fact(|state, path| date(newest(state, path).updated));

/// The newest times among the folder's vehicles (its subfolders' too), as the last full listing
/// had them.
fn newest(state: &State, path: &FolderPath) -> Newest {
    let times = state
        .vehicle_times
        .iter()
        .map(|(id, created, updated)| (id.as_str(), *created, *updated));
    newest_in(state.tree(), path, times)
}

/// A time (unix seconds) as the details show a vehicle's: its date in the player's language (the
/// date and time in UTC if the game's way wasn't found).
fn date(time: Option<i64>) -> String {
    match (time, DATES.get()) {
        (Some(time), Some(dates)) => dates.date(time),
        (Some(time), None) => clock::utc_text(u64::try_from(time).unwrap_or(0)),
        (None, _) => UNKNOWN.to_owned(),
    }
}

/// Makes the rows and finds the header (after engine init).
pub fn start() -> Result<(), String> {
    let rows = settings_list::rows()?;
    let row = |key: &str, name: &str, description: &str, value: &'static dyn Editable| {
        rows.row(
            &format!("VehicleFolder{key}"),
            name,
            description,
            Value::Text(value),
            || {},
        )
        .map(Row::side_by_side)
    };
    let made = Rows {
        section: crate::game::kismet::string_to_text("")?,
        rows: vec![
            row(
                "Name",
                "Rename",
                "The folder's name: type a new one to rename it.",
                &NAME,
            )?,
            row(
                "Created",
                "Created folder",
                "When the folder was made (unknown for folders made before this was noted).",
                &CREATED,
            )?,
            row(
                "NewestVehicle",
                "Last created vehicle",
                "When the newest vehicle in it (in its subfolders too) was made.",
                &NEWEST_VEHICLE,
            )?,
            row(
                "LastUpdate",
                "Last updated vehicle",
                "When a vehicle in it (in its subfolders too) was last saved.",
                &LAST_UPDATE,
            )?,
        ],
    };
    let _ = ROWS.set(made);
    match Dates::find() {
        Ok(dates) => {
            let _ = DATES.set(dates);
        }
        Err(reason) => log!("vehicle folders: a folder's dates are in UTC ({reason})"),
    }
    match EntryHeaders::find() {
        Ok(headers) => {
            let _ = HEADERS.set(headers);
        }
        Err(reason) => {
            log!("vehicle folders: a selected folder isn't shown in the header ({reason})")
        }
    }
    Ok(())
}

/// The folder whose rows the panel shows, if a folder tile of the Local list is selected.
fn selected_folder(state: &State) -> Option<FolderPath> {
    match &state.selected.as_ref()?.tile {
        Tile::Folder(path) if state.ready => Some(path.clone()),
        _ => None,
    }
}

/// Fills `reflection` with the selected folder's rows instead of the browser's, and the header
/// with the folder, if a folder is selected; whether it did.
///
/// # Safety
/// `reflection` must be the live reflection the game is filling to show rows in the browser's
/// panel. Game thread only.
pub unsafe fn add_rows(reflection: *mut Reflection) -> bool {
    let shows = view_mode() != ViewMode::Off
        && shows_folders()
        && with_state(|state| selected_folder(state).is_some());
    let (true, Some(ours), Ok(rows)) = (shows, ROWS.get(), settings_list::rows()) else {
        return false;
    };
    // SAFETY: per the contract; the section's title and the rows live for the whole game.
    unsafe {
        rows.add_category(reflection, &ours.section);
        for row in &ours.rows {
            rows.add_row(reflection, row);
        }
    }
    // The game filled the header just before ("Nothing selected").
    show_header();
    true
}

/// Shows the selected folder in the header: its name, its vehicles and subfolders, the X enabled
/// to unselect it. With no folder selected (and, the caller's part, none of the game's), the X is
/// disabled again, as the game leaves it.
pub fn show_header() {
    let (folder, browser) = with_state(|state| {
        let folder = selected_folder(state).map(|path| {
            let subfolders = state.tree().subfolders(Some(&path)).len();
            (path, subfolders)
        });
        (folder, state.browser.and_then(|browser| browser.get_live()))
    });
    let (Some(browser), Some(headers), Some(browsers)) = (browser, HEADERS.get(), BROWSERS.get())
    else {
        return;
    };
    let Some((path, subfolders)) = folder else {
        // SAFETY: the live browser, on the game thread (the panel is filled there).
        let disabled = unsafe {
            match browsers.paged_list(browser) {
                Some(list) => headers.disable_clear(list),
                None => Ok(()),
            }
        };
        if let Err(reason) = disabled {
            log!("vehicle folders: the header's X isn't put back ({reason})");
        }
        return;
    };
    // As its tile counts them (the vehicles that exist): the page shown has its tile.
    let vehicles = tiles::vehicle_count(&Tile::Folder(path.clone())).unwrap_or(0);
    let stats = [count(vehicles, "vehicle"), count(subfolders, "subfolder")];
    // SAFETY: the live browser, on the game thread (the panel is filled there).
    let shown = unsafe {
        match browsers.paged_list(browser) {
            Some(list) => headers.show(list, path.name(), &stats),
            None => Ok(()),
        }
    };
    if let Err(reason) = shown {
        log!("vehicle folders: the header doesn't show the folder ({reason})");
    }
}

/// `n` things, as a header box shows them.
fn count(n: usize, thing: &str) -> String {
    let plural = if n == 1 { "" } else { "s" };
    format!("{n} {thing}{plural}")
}

/// `typed` was typed in the name box: the selected folder takes it if it can, and the box keeps
/// it either way.
fn rename(typed: &str) {
    let renamed = with_state(|state| {
        let path = selected_folder(state)?;
        let outcome = state.tree_mut().rename_folder(&path, typed);
        let new = outcome
            .as_ref()
            .ok()
            .cloned()
            .unwrap_or_else(|| path.clone());
        state.name_draft = Some((new.clone(), typed.to_owned()));
        // Paths compare ignoring case: a new casing is a rename too.
        let changed = outcome.is_ok() && new.to_string() != path.to_string();
        changed.then_some((path, new))
    });
    let Some((old, new)) = renamed else {
        return;
    };
    save();
    let count = tiles::vehicle_count(&Tile::Folder(old.clone())).unwrap_or(0);
    let label = tiles::label(&Tile::Folder(new.clone()), count);
    tiles::renamed(&old, &new, &label);
    show_header();
}
