//! Folder tiles: rows of ours in the vehicle grid, shown with the folder icon. A click selects one
//! (our highlight, the game's selection cleared, the side panel showing the folder: `details`), a
//! click on it again unselects it, a double click opens it; the "up" tile opens the parent at once.
//! The game never selects a tile, so none of its buttons (Spawn, Delete, ...) can act on one. See
//! `docs/vehicle-menu.md`, "Folder tiles".

use core::ffi::c_void;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;
use vehicle_folders::{FolderPath, Location, UNCATEGORIZED_LABEL};

use super::{BROWSERS, DECOR, ITEMS, TEXTS, details, row_key, shows_folders, with_state};
use crate::framework::timings;
use crate::game::reflection::NativeThunk;
use crate::game::ugc_browser::EntryClickedFn;
use crate::game::ugc_decor::{Badge, Look};
use crate::game::ugc_item::{TileIcon, WidgetFn};
use crate::game::ugc_thumbnail::Thumbnail;
use crate::game::weak_object::WeakObject;
use crate::platform::timer::ThreadTimer;
use crate::prelude::*;

/// Starts every tile's `LocalItemId`: a control character, which no vehicle folder's name can hold
/// (Windows forbids them in file names), so no vehicle is ever taken for a tile.
const MARK: &str = "\u{1}ET:";

/// What a tile opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tile {
    Folder(FolderPath),
    Uncategorized,
    /// The parent of the folder shown.
    Up,
}

impl Tile {
    /// The tile's `LocalItemId`.
    fn id(&self) -> String {
        match self {
            Tile::Folder(path) => format!("{MARK}F/{path}"),
            Tile::Uncategorized => format!("{MARK}U"),
            Tile::Up => format!("{MARK}P"),
        }
    }

    /// The tile a `LocalItemId` names, if it's one of ours.
    pub fn of_id(id: &str) -> Option<Tile> {
        let rest = id.strip_prefix(MARK)?;
        match rest {
            "U" => Some(Tile::Uncategorized),
            "P" => Some(Tile::Up),
            _ => FolderPath::new(rest.strip_prefix("F/")?)
                .ok()
                .map(Tile::Folder),
        }
    }

    /// Where opening it goes, from `here`.
    fn destination(&self, here: &Location) -> Location {
        match self {
            Tile::Folder(path) => Location::Folder(path.clone()),
            Tile::Uncategorized => Location::Uncategorized,
            Tile::Up => here.parent().unwrap_or_default(),
        }
    }
}

/// The name a folder tile shows.
pub fn label(tile: &Tile, vehicle_count: usize) -> String {
    match tile {
        Tile::Folder(path) => format!("{} ({vehicle_count})", path.name()),
        Tile::Uncategorized => format!("{UNCATEGORIZED_LABEL} ({vehicle_count})"),
        Tile::Up => "..".into(),
    }
}

/// A row of ours, kept for the session and reused (the game only copies the rows it's given).
#[derive(Clone, Copy)]
struct OurRow(*mut u8);

thread_local! {
    /// Our rows by tile id. Game thread only, like everything here.
    static ROWS: RefCell<HashMap<String, OurRow>> = RefCell::new(HashMap::new());
    /// The deferred navigation's timer, while it waits for the next frame.
    static NAVIGATING: RefCell<Option<ThreadTimer>> = const { RefCell::new(None) };
    /// The thumbnails each tile of the page shown previews, by tile id.
    static PREVIEWS: RefCell<HashMap<String, Vec<Option<Thumbnail>>>> =
        RefCell::new(HashMap::new());
    /// The vehicles in each tile of the page shown (its subfolders' too), by tile id.
    static COUNTS: RefCell<HashMap<String, usize>> = RefCell::new(HashMap::new());
}

/// The thumbnails the tiles of a new page preview (read while the game's rows were alive), for
/// their widgets to show.
pub fn set_previews(previews: Vec<(Tile, Vec<Option<Thumbnail>>)>) {
    let previews = previews
        .into_iter()
        .map(|(tile, thumbnails)| (tile.id(), thumbnails))
        .collect();
    PREVIEWS.set(previews);
}

/// The vehicles in each tile of a new page (with their subfolders'), as its label counts them.
pub fn set_counts(counts: Vec<(Tile, usize)>) {
    let counts = counts
        .into_iter()
        .map(|(tile, count)| (tile.id(), count))
        .collect();
    COUNTS.set(counts);
}

/// The vehicles in `tile` (with its subfolders'), if it's on the page shown.
pub fn vehicle_count(tile: &Tile) -> Option<usize> {
    COUNTS.with_borrow(|counts| counts.get(&tile.id()).copied())
}

/// The row for `tile`, made the first time, showing `label`.
///
/// # Safety
/// Game thread only, and not while the game is copying an earlier row of ours.
pub unsafe fn row(tile: &Tile, label: &str) -> Option<*const u8> {
    let rows = &ITEMS.get()?.rows;
    let id = tile.id();
    let existing = ROWS.with_borrow(|known| known.get(&id).copied());
    let row = match existing {
        Some(row) => row,
        None => {
            // SAFETY: per the contract.
            let made = match unsafe { rows.new_row() } {
                Ok(made) => made,
                Err(reason) => {
                    log!("vehicle folders: no folder tile ({reason})");
                    return None;
                }
            };
            // SAFETY: just made, not used by the game.
            unsafe { rows.set_local_id(made, &id) };
            ROWS.with_borrow_mut(|known| known.insert(id, OurRow(made)));
            OurRow(made)
        }
    };
    // SAFETY: ours; the game copied what it was given before (the caller's contract).
    unsafe { rows.set_title(row.0, label) };
    Some(row.0.cast_const())
}

/// The tile a row widget shows, if it shows one.
///
/// # Safety
/// `widget` must be a live object or null. Game thread only.
unsafe fn tile_of_widget(widget: *const UObjectBase) -> Option<Tile> {
    let items = ITEMS.get()?;
    // SAFETY: per the contract; the row is the widget's own.
    unsafe {
        let row = items.row_of(widget)?;
        Tile::of_id(&items.rows.local_id(row))
    }
}

pub static ORIGINAL_INITIALIZE: OnceLock<WidgetFn> = OnceLock::new();
pub static ORIGINAL_CLICKED: OnceLock<EntryClickedFn> = OnceLock::new();

/// `UUGCItemWidget::InitializeEntry_Implementation`: a tile is shown as a folder, the rest as the
/// game shows them.
pub unsafe extern "C" fn initialize_entry(widget: *mut UObjectBase) {
    let _span = timings::span("vehicle_menu::initialize_entry");
    let original = *ORIGINAL_INITIALIZE.get().expect("hook recorded before use");
    // SAFETY: the game passes a live row widget.
    let tile = unsafe { tile_of_widget(widget) };
    let (Some(tile), Some(items), Some(texts)) = (tile, ITEMS.get(), TEXTS.get()) else {
        // A vehicle without a thumbnail shows the image's brush: not a tile's icon left on this
        // pooled widget.
        if let Some(items) = ITEMS.get() {
            // SAFETY: as above.
            unsafe { items.show_own_image(widget) };
        }
        // SAFETY: the original with its own argument.
        unsafe { timings::game("UUGCItemWidget::InitializeEntry", || original(widget)) };
        // SAFETY: as above.
        unsafe { show_decor(widget, None) };
        return;
    };
    // SAFETY: as above; the row is the widget's own.
    let title = unsafe {
        items
            .rows
            .title(items.row_of(widget).expect("a tile's row"))
    };
    let name = texts.text(&title);
    // SAFETY: a live row widget, on the game thread; our text is released once shown.
    unsafe {
        let icon = if tile == Tile::Up {
            TileIcon::Up
        } else {
            TileIcon::Folder
        };
        items.show_folder(widget, &name, icon);
        name.release();
    }
    let selected = with_state(|state| {
        state
            .selected
            .as_ref()
            .is_some_and(|selected| selected.tile == tile)
    });
    // SAFETY: as above.
    unsafe { show_decor(widget, Some(&tile)) };
    if selected {
        // A tile's widget is reused when the page is rebuilt: our highlight goes with the tile.
        select(widget, tile);
    }
}

/// Shows ours on a row widget showing a vehicle (`tile` `None`) or a tile: on a folder the accent
/// area, its vehicles' thumbnails in a 2x2 grid (blank cells when fewer) and the folder icon in
/// the corner, on a pinned vehicle or folder of the Local list the pin there instead. The widget
/// may have shown another row before.
///
/// # Safety
/// `widget` must be a live row widget. Game thread only.
unsafe fn show_decor(widget: *mut UObjectBase, tile: Option<&Tile>) {
    let (Some(decor), Some(items)) = (DECOR.get(), ITEMS.get()) else {
        return;
    };
    let pinned = shows_folders()
        && match tile {
            Some(Tile::Folder(path)) => {
                with_state(|state| state.ready && state.tree().is_folder_pinned(path))
            }
            Some(_) => false,
            None => {
                // SAFETY: per the contract; the row is the widget's own.
                let id = unsafe { items.row_of(widget).map(|row| row_key(row)) };
                id.is_some_and(|id| with_state(|state| state.ready && state.tree().is_pinned(&id)))
            }
        };
    let grid = match tile {
        Some(tile @ (Tile::Folder(_) | Tile::Uncategorized)) => Some(
            PREVIEWS
                .with_borrow(|previews| previews.get(&tile.id()).cloned())
                .unwrap_or_default(),
        ),
        _ => None,
    };
    let accent = grid.is_some();
    let badge = if pinned {
        Some(Badge::Pin)
    } else {
        grid.as_ref().map(|_| Badge::Folder)
    };
    // SAFETY: per the contract.
    unsafe {
        decor.show(
            widget,
            Look {
                badge,
                grid,
                accent,
            },
        )
    };
}

/// `UPagedListWidget::OnEntryClicked`: a click on a tile is ours, the rest the game's.
pub unsafe extern "C" fn entry_clicked(
    list: *mut UObjectBase,
    entry: *mut UObjectBase,
    double_click: u8,
) {
    let _span = timings::span("vehicle_menu::entry_clicked");
    let original = *ORIGINAL_CLICKED.get().expect("hook recorded before use");
    // The release of a drag on the pressed row, which may show another item by now.
    if super::drag::swallows_click() {
        return;
    }
    // SAFETY: the game passes a live row widget (or null).
    let Some(tile) = (unsafe { tile_of_widget(entry) }) else {
        unselect();
        // SAFETY: the original with its own arguments.
        unsafe {
            timings::game("UPagedListWidget::OnEntryClicked", || {
                original(list, entry, double_click)
            })
        };
        return;
    };
    let opens = tile == Tile::Up || double_click != 0;
    let selected_already = with_state(|state| {
        state
            .selected
            .as_ref()
            .is_some_and(|known| known.tile == tile)
    });
    if opens {
        // Not selected while it's left.
    } else if selected_already {
        // Clicked again: unselected, as the game does with a vehicle.
        unselect();
    } else {
        // Ours first, so that the side panel the game fills below shows the folder.
        select(entry, tile.clone());
    }
    // The game's selection goes, as for a click on an empty spot: its buttons and side panel
    // update (only if it had one; `show` does it otherwise).
    // SAFETY: the original with its own list and no entry, which it handles.
    unsafe {
        timings::game("UPagedListWidget::OnEntryClicked", || {
            original(list, core::ptr::null_mut(), 0)
        })
    };
    if opens {
        open(&tile);
    } else {
        show_next_frame(Refresh::ButtonsOnly);
    }
}

pub static ORIGINAL_CLEAR: OnceLock<NativeThunk> = OnceLock::new();

/// `UPagedListWidget::ClearSelectedEntryWidget`'s exec thunk: the details header's X (enabled for a
/// selected folder, `details`) unselects our folder too, then the game clears its own selection.
pub unsafe extern "C" fn clear_selection(
    context: *mut c_void,
    stack: *mut c_void,
    result: *mut c_void,
) {
    let _span = timings::span("vehicle_menu::clear_selection");
    let original = *ORIGINAL_CLEAR.get().expect("hook recorded before use");
    let browser = with_state(|state| {
        state
            .selected
            .is_some()
            .then_some(state.browser)
            .flatten()
            .and_then(|browser| browser.get_live())
    });
    // SAFETY: the live browser, on the game thread (the thunk runs there).
    let ours = browser
        .zip(BROWSERS.get())
        .is_some_and(|(browser, browsers)| unsafe {
            browsers.paged_list(browser) == Some(context.cast())
        });
    if ours {
        unselect();
        // The details and the buttons follow (the game's own selection is cleared below).
        show_next_frame(Refresh::ButtonsOnly);
    }
    // SAFETY: the thunk's own arguments, passed on.
    unsafe {
        timings::game("UPagedListWidget::ClearSelectedEntryWidget", || {
            original(context, stack, result)
        })
    };
}

/// A tile highlighted as selected.
pub struct Selected {
    /// The widget showing it; `None` until a page shows it (a folder just made is selected before
    /// the page showing it is built).
    widget: Option<WeakObject>,
    pub tile: Tile,
}

impl Selected {
    /// The widget, if it still shows this tile (widgets are reused for other rows).
    fn showing(&self) -> Option<*mut UObjectBase> {
        let widget = self.widget?.get_live()?;
        // SAFETY: a live object, on the game thread (every caller is).
        (unsafe { tile_of_widget(widget) }.as_ref() == Some(&self.tile)).then_some(widget)
    }
}

/// Highlights `widget`'s tile as the selection, the previous one unhighlighted.
fn select(widget: *mut UObjectBase, tile: Tile) {
    let Some(items) = ITEMS.get() else {
        return;
    };
    let previous = with_state(|state| state.selected.take());
    if let Some(previous) = previous.as_ref()
        && previous.tile != tile
    {
        with_state(|state| state.name_draft = None);
    }
    if let Some(previous) = previous.and_then(|previous| previous.showing())
        && previous != widget
    {
        // SAFETY: a live row widget, on the game thread.
        unsafe { items.show_selected(previous, false) };
    }
    // SAFETY: the game's live row widget, on the game thread.
    let widget = unsafe {
        items.show_selected(widget, true);
        WeakObject::new(widget)
    };
    with_state(|state| {
        state.selected = Some(Selected {
            widget: Some(widget),
            tile,
        })
    });
}

/// Selects `folder` before a page shows it (one just made): highlighted as its widget shows it,
/// the side panel showing it once the list is shown again. The game's selection goes now.
pub fn select_later(folder: FolderPath) {
    unselect();
    clear_game_selection();
    with_state(|state| {
        state.selected = Some(Selected {
            widget: None,
            tile: Tile::Folder(folder),
        })
    });
}

/// Unselects the game's row (a vehicle), if any: our selection takes over.
fn clear_game_selection() {
    let (Some(browsers), Some(original)) = (BROWSERS.get(), ORIGINAL_CLICKED.get()) else {
        return;
    };
    let browser = with_state(|state| state.browser.and_then(|browser| browser.get_live()));
    // SAFETY: the live browser, on the game thread (our buttons and timers run there). No entry
    // is a click on an empty spot, which unselects (its buttons and side panel update).
    unsafe {
        let Some(browser) = browser else {
            return;
        };
        if browsers.selected_entry(browser).is_some()
            && let Some(list) = browsers.paged_list(browser)
        {
            original(list, core::ptr::null_mut(), 0);
        }
    }
}

/// Whether `widget` shows the selected tile.
pub fn is_selected(widget: *mut UObjectBase) -> bool {
    let showing = with_state(|state| {
        state
            .selected
            .as_ref()
            .and_then(|selected| Some((selected.widget?, selected.tile.clone())))
    });
    showing.is_some_and(|(selected, tile)| {
        selected.is(widget)
            // SAFETY: the selected widget, live (it's `widget`), on the game thread.
            && unsafe { tile_of_widget(widget) }.as_ref() == Some(&tile)
    })
}

/// Unhighlights the selected tile, if any (a name typed for it that wasn't taken is forgotten).
pub fn unselect() {
    let Some(previous) = with_state(|state| {
        state.name_draft = None;
        state.selected.take()
    }) else {
        return;
    };
    if let (Some(items), Some(widget)) = (ITEMS.get(), previous.showing()) {
        // SAFETY: a live row widget, on the game thread.
        unsafe { items.show_selected(widget, false) };
    }
}

/// The selected folder went from `old` to `new` (renamed while its name is typed) without the page
/// being built again, which would take the typing away: its tile follows (our row, the widget's
/// copy of it, its label `label`, what's kept by tile), so clicks, drops and the next page still
/// find it.
pub fn renamed(old: &FolderPath, new: &FolderPath, label: &str) {
    let Some(items) = ITEMS.get() else {
        return;
    };
    let (old_tile, new_tile) = (Tile::Folder(old.clone()), Tile::Folder(new.clone()));
    let (old_id, new_id) = (old_tile.id(), new_tile.id());
    let row = ROWS.with_borrow_mut(|rows| rows.remove(&old_id));
    if let Some(row) = row {
        // SAFETY: our row, on the game thread, not being copied by the game (it copies rows only
        // while building a page, not during typing).
        unsafe {
            items.rows.set_local_id(row.0, &new_id);
            items.rows.set_title(row.0, label);
        }
        ROWS.with_borrow_mut(|rows| rows.insert(new_id.clone(), row));
    }
    PREVIEWS.with_borrow_mut(|previews| {
        if let Some(preview) = previews.remove(&old_id) {
            previews.insert(new_id.clone(), preview);
        }
    });
    COUNTS.with_borrow_mut(|counts| {
        if let Some(count) = counts.remove(&old_id) {
            counts.insert(new_id.clone(), count);
        }
    });
    let widget = with_state(|state| {
        let selected = state
            .selected
            .as_mut()
            .filter(|known| known.tile == old_tile)?;
        let widget = selected.showing();
        selected.tile = new_tile.clone();
        widget
    });
    let (Some(widget), Some(texts)) = (widget, TEXTS.get()) else {
        return;
    };
    // SAFETY: the live row widget showing the folder, on the game thread; its row is its own copy
    // (strings of the game's allocator). Our text is released once shown.
    unsafe {
        if let Some(row) = items.row_of(widget) {
            items.rows.set_local_id(row.cast_mut(), &new_id);
            items.rows.set_title(row.cast_mut(), label);
        }
        let name = texts.text(label);
        items.show_folder(widget, &name, TileIcon::Folder);
        name.release();
        show_decor(widget, Some(&new_tile));
        // Showing it again passed the game's (unselected) state.
        items.show_selected(widget, true);
    }
}

/// Opens what `tile` leads to.
fn open(tile: &Tile) {
    unselect();
    with_state(|state| {
        let destination = tile.destination(state.here());
        state.history_mut().go(destination);
    });
    show_next_frame(Refresh::FirstPage);
}

/// What to show again on the next frame (the strongest asked for wins).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Refresh {
    /// The buttons, for a new selection.
    ButtonsOnly,
    /// The page shown, after a change to what it lists.
    SamePage,
    /// The first page, for a new location.
    FirstPage,
}

thread_local! {
    static WANTED: std::cell::Cell<Option<Refresh>> = const { std::cell::Cell::new(None) };
}

/// Shows the list (and the buttons) again on the next frame: not inside the click being handled,
/// whose widgets the new page reuses.
pub fn show_next_frame(refresh: Refresh) {
    let before = WANTED.get();
    WANTED.set(Some(before.map_or(refresh, |wanted| wanted.max(refresh))));
    if NAVIGATING.with_borrow(Option::is_some) {
        return;
    }
    match ThreadTimer::start(10, show) {
        Some(timer) => {
            NAVIGATING.with_borrow_mut(|waiting| *waiting = Some(timer));
        }
        None => log!("vehicle folders: no timer, the change shows on the next page change"),
    }
}

/// The timer's tick: shows what was asked for, then stops.
fn show() {
    let _span = timings::span("vehicle_menu::show");
    NAVIGATING.with_borrow_mut(Option::take);
    let Some(refresh) = WANTED.take() else {
        return;
    };
    let browser = with_state(|state| state.browser.and_then(|b| b.get_live()));
    let (Some(browser), Some(browsers)) = (browser, BROWSERS.get()) else {
        return;
    };
    // SAFETY: the live browser, on the game thread (the timer runs there).
    unsafe {
        match refresh {
            Refresh::FirstPage => browsers.show_first_page(browser),
            Refresh::SamePage => browsers.refresh(browser),
            Refresh::ButtonsOnly => {}
        }
        browsers.update_buttons(browser);
        // The side panel for our selection (the folder's rows), or for none: the game's own
        // selection fills it itself.
        if browsers.selected_entry(browser).is_none() {
            browsers.refresh_panel(browser);
            details::show_header();
        }
    }
}
