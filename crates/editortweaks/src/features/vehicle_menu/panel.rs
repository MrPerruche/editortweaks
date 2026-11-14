//! The vehicle menu's settings in its own left panel, under the game's (category, sort, search):
//! while no vehicle is selected, the panel lists what the browser reflects
//! (`UUGCBrowserWidget::ReflectBrickProperties`, hooked), and our rows are added after the game's.
//! They show only while the browser lists local or subscribed vehicles, the lists they change.

use core::ffi::c_void;
use std::sync::OnceLock;

use vehicle_folders::History;

use super::details;
use super::tiles::{self, Refresh};
use super::{FOLDERS, shows_folders, with_state};
use crate::features::settings_list;
use crate::framework::timings;
use crate::game::property_reflection::{List, ReflectFn, Reflection, Row, Value};

pub static ORIGINAL_REFLECT: OnceLock<ReflectFn> = OnceLock::new();

/// Our rows, made at engine init and kept for the whole game (the game holds references to them).
static ROWS: OnceLock<Vec<Row>> = OnceLock::new();

/// The "Vehicle folders" setting as a dropdown: its four choices side by side don't fit the panel.
struct FolderView;

impl List for FolderView {
    fn items(&self) -> Vec<String> {
        FOLDERS.items()
    }

    fn chosen(&self) -> usize {
        FOLDERS.chosen()
    }

    fn choose(&self, index: usize) -> bool {
        FOLDERS.choose(index)
    }

    fn dropdown(&self) -> bool {
        true
    }
}

/// Makes our rows (after engine init).
pub fn start() -> Result<(), String> {
    let rows = settings_list::rows()?;
    let folders = rows
        .row(
            FOLDERS.key,
            FOLDERS.name,
            FOLDERS.description,
            Value::List(&FolderView),
            folders_changed,
        )?
        .when(shows_folders);
    let _ = ROWS.set(vec![folders]);
    Ok(())
}

/// The "Vehicle folders" choice changed in the panel: saved, and the list shown again from the top
/// (a folder shown may not be one in the new view, e.g. "Uncategorized").
fn folders_changed() {
    settings_list::save();
    tiles::unselect();
    with_state(|state| {
        state.local_history = History::new();
        state.subscribed_history = History::new();
    });
    tiles::show_next_frame(Refresh::FirstPage);
}

/// `UUGCBrowserWidget::ReflectBrickProperties`: the game's rows, then ours; a selected folder's
/// rows instead (`details`).
pub unsafe extern "C" fn reflect(browser: *const c_void, reflection: *mut Reflection) {
    let _span = timings::span("vehicle_menu::reflect");
    let original = *ORIGINAL_REFLECT.get().expect("hook recorded before use");
    // Not while the browser's settings are saved or loaded: only rows shown in its panel.
    // SAFETY: the game passes its live reflection; filling it to show rows, on the game thread.
    if !reflection.is_null()
        && unsafe { (*reflection).lists_rows() }
        && unsafe { details::add_rows(reflection) }
    {
        return;
    }
    // SAFETY: the original with its own arguments.
    unsafe {
        timings::game("UUGCBrowserWidget::ReflectBrickProperties", || {
            original(browser, reflection)
        })
    };
    // Not while the browser's settings are saved or loaded: only rows shown in its panel.
    // SAFETY: the game passes its live reflection.
    if reflection.is_null() || !unsafe { (*reflection).lists_rows() } {
        return;
    }
    let (Some(ours), Ok(rows)) = (ROWS.get(), settings_list::rows()) else {
        return;
    };
    for row in ours.iter().filter(|row| row.is_listed()) {
        // SAFETY: the reflection the game is filling, on the game thread; the row lives for the
        // whole game. Its rows end the game's last category (the browser's untitled one).
        unsafe { rows.add_row(reflection, row) };
    }
}
