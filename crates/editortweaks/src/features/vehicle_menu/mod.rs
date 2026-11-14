//! The vehicle menu's folders and pins (work in progress, only in `--vehicle-menu` builds). See
//! `docs/vehicle-menu.md`.
//!
//! - [`listing`]: the Local and Subscribed lists show one folder level, pins first, a page at a time
//!   (ours).
//! - [`tiles`]: folder tiles in the grid, their clicks, navigation.
//! - [`folders_file`]: `EditorTweaks-Folders.toml`, beside the vehicles.
//! - [`guard`]: the game deletes only vehicles' own folders.
//! - [`panel`]: the menu's settings in its own left panel.
//! - [`details`]: the selected folder in the details column (the header, Rename and its times).
//! - [`bar`]: the navigation bar above the list (Back, Forward, Up, the path).
//! - [`drag`]: dragging vehicles and folders onto folders.
//! - [`subscribed`]: the Subscribed list as one Steam query for every subscribed item, so it gets
//!   folders like the Local list.

mod bar;
mod buttons;
mod details;
mod drag;
mod folders_file;
mod guard;
mod listing;
mod panel;
mod subscribed;
mod tiles;

use std::cell::RefCell;
use std::sync::OnceLock;

use vehicle_folders::{Category, FolderPath, History, Location, Store, Tree, ViewMode};

use crate::framework::hook;
use crate::framework::settings::{Alternative, Choice, Section, Setting};
use crate::game::kismet::{self, Texts};
use crate::game::ugc_browser::{self, Browsers, query_type};
use crate::game::ugc_decor::Decorations;
use crate::game::ugc_item::{ItemWidgets, WidgetFn};
use crate::game::weak_object::WeakObject;
use crate::prelude::*;

pub static FOLDERS: Choice = Choice::new(
    "VehicleFolders",
    "Vehicle folders",
    "How the vehicle menu lists your local and subscribed vehicles. Off: the game's list (pinned vehicles still \
     first). Mixed: folders sorted in among the vehicles. Folders first: the folders, then the \
     vehicles in none. Uncategorized folder: only folders at the top, the vehicles in none in \
     \"Uncategorized\".",
    &[
        Alternative {
            key: "Off",
            name: "Off",
        },
        Alternative {
            key: "Mixed",
            name: "Mixed",
        },
        Alternative {
            key: "FoldersFirst",
            name: "Folders first",
        },
        Alternative {
            key: "UncategorizedFolder",
            name: "Uncategorized folder",
        },
    ],
    2,
);

/// Saved with the others, but changed in the vehicle menu's own panel (`panel`), not the options.
pub static SETTINGS: [Section; 1] = [Section {
    title: "Vehicle menu",
    settings: &[],
    unlisted: &[Setting::Choice(&FOLDERS)],
}];

/// The "Vehicle folders" setting as the listing's mode.
fn view_mode() -> ViewMode {
    match FOLDERS.get() {
        0 => ViewMode::Off,
        1 => ViewMode::Mixed,
        3 => ViewMode::UncategorizedFolder,
        _ => ViewMode::FoldersFirst,
    }
}

/// The browser's reflected parts, the row widgets' and the text conversions (after engine init).
static BROWSERS: OnceLock<Browsers> = OnceLock::new();
static ITEMS: OnceLock<ItemWidgets> = OnceLock::new();
static TEXTS: OnceLock<Texts> = OnceLock::new();
/// What we add to rows (corner icons, folder previews), if the row widgets are laid out as
/// expected.
static DECOR: OnceLock<Decorations> = OnceLock::new();

/// The category with folders a browser category (`EUGCQueryType`) is, if it has them: Local, and
/// Subscribed once its listing works (`subscribed`).
fn category_of(query_type: u8) -> Option<Category> {
    match query_type {
        query_type::LOCAL => Some(Category::Local),
        query_type::SUBSCRIBED if subscribed::available() => Some(Category::Subscribed),
        _ => None,
    }
}

/// The category with folders the browser last seen loading a page shows, if it shows one (Local
/// before any was seen).
fn shown_category() -> Option<Category> {
    let browser = with_state(|state| state.browser.and_then(|browser| browser.get_live()));
    match (browser, BROWSERS.get()) {
        // SAFETY: the live browser, on the game thread (every hook and timer of the menu runs
        // there).
        (Some(browser), Some(browsers)) => category_of(unsafe { browsers.query_type(browser) }),
        _ => Some(Category::Local),
    }
}

/// Whether the browser shows a list folders and pins change (Local, Subscribed).
fn shows_folders() -> bool {
    shown_category().is_some()
}

/// The id `row` is filed under in the category shown: a folder tile's own (`tiles`), a local
/// vehicle's folder name, a subscribed one's Workshop id; empty if it has none.
///
/// # Safety
/// `row` must be a live row (`FUGCQueryResult`). Game thread only.
unsafe fn row_key(row: *const u8) -> String {
    let Some(items) = ITEMS.get() else {
        return String::new();
    };
    // SAFETY: per the contract.
    let local = unsafe { items.rows.local_id(row) };
    let subscribed = with_state(|state| state.shown == Category::Subscribed);
    if !subscribed || tiles::Tile::of_id(&local).is_some() {
        return local;
    }
    // SAFETY: per the contract.
    unsafe { subscribed::workshop_id(row) }
        .map(|id| id.to_string())
        .unwrap_or_default()
}

/// What the vehicle menu keeps between its pages.
struct State {
    /// Whether the folders file was read (from engine init).
    ready: bool,
    store: Store,
    /// Whether changes may be written to the file (see `folders_file::Loaded`).
    writable: bool,
    /// The category with folders shown last (`load_page` follows the browser): what the tree,
    /// the history and the selection below are about.
    shown: Category,
    /// Where each category's list is, and where it was.
    local_history: History,
    subscribed_history: History,
    /// The browser last seen loading a page.
    browser: Option<WeakObject>,
    /// The folder tile shown selected.
    selected: Option<tiles::Selected>,
    /// What's typed in the selected folder's Name box while it isn't a name the folder can take
    /// (empty, taken, ...), and the folder (`details`).
    name_draft: Option<(FolderPath, String)>,
    /// Every vehicle's (id, created, updated) as the last full listing had them (unix seconds),
    /// for a selected folder's details.
    vehicle_times: Vec<(String, i64, i64)>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            ready: false,
            store: Store::default(),
            writable: false,
            shown: Category::Local,
            local_history: History::new(),
            subscribed_history: History::new(),
            browser: None,
            selected: None,
            name_draft: None,
            vehicle_times: Vec::new(),
        }
    }
}

impl State {
    /// The shown category's folders.
    fn tree(&self) -> &Tree {
        self.store.tree(self.shown)
    }

    /// The shown category's folders, to change.
    fn tree_mut(&mut self) -> &mut Tree {
        self.store.tree_mut(self.shown)
    }

    /// Where the shown category's list is.
    fn here(&self) -> &Location {
        match self.shown {
            Category::Local => self.local_history.current(),
            Category::Subscribed => self.subscribed_history.current(),
        }
    }

    /// The shown category's history, to move.
    fn history_mut(&mut self) -> &mut History {
        match self.shown {
            Category::Local => &mut self.local_history,
            Category::Subscribed => &mut self.subscribed_history,
        }
    }

    /// Leaves a folder of the shown category deleted or renamed since for one that exists.
    fn fix_up_history(&mut self) {
        match self.shown {
            Category::Local => self.local_history.fix_up(&self.store.local),
            Category::Subscribed => self.subscribed_history.fix_up(&self.store.subscribed),
        }
    }

    /// Whether `category`'s list differs from the game's: folders shown, or vehicles pinned.
    fn changes_list(&self, category: Category, mode: ViewMode) -> bool {
        mode != ViewMode::Off
            || self
                .store
                .tree(category)
                .vehicles()
                .any(|(_, entry)| entry.pinned)
    }

    /// The browser shows `category` now: the selection (of the other one) goes.
    fn show(&mut self, category: Category) {
        if self.shown != category {
            self.shown = category;
            self.selected = None;
            self.name_draft = None;
        }
    }
}

// Game thread only: every hook and timer of the menu runs there.
thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Runs `f` with the menu's state. Never call the game from `f`: its hooks may need the state too.
fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with_borrow_mut(f)
}

pub struct VehicleMenu;

impl Feature for VehicleMenu {
    fn id(&self) -> &'static str {
        "vehicle_menu"
    }

    fn on_load(&self) -> LoadResult {
        ugc_browser::hook_load_page(listing::load_page, &listing::ORIGINAL_LOAD_PAGE)?;
        ugc_browser::hook_query(listing::query, &listing::ORIGINAL_QUERY)?;
        ugc_browser::hook_query_complete(
            listing::query_complete,
            &listing::ORIGINAL_QUERY_COMPLETE,
        )?;
        ugc_browser::hook_entry_clicked(tiles::entry_clicked, &tiles::ORIGINAL_CLICKED)?;
        ugc_browser::hook_delete_item(guard::delete_item, &guard::ORIGINAL_DELETE)?;
        // Without it the view can't be chosen, but the default one works.
        if let Err(reason) =
            ugc_browser::hook_reflect_panel(panel::reflect, &panel::ORIGINAL_REFLECT)
        {
            log!("vehicle folders: no settings in the menu's panel ({reason})");
        }
        log!("vehicle menu: work-in-progress build (folders and pins)");
        Ok(())
    }

    fn on_engine_init(&self) {
        if let Err(reason) = start() {
            log!("vehicle folders off: {reason}");
        }
    }
}

/// Finds the browser's parts, hooks the row widgets and reads the folders file.
fn start() -> Result<(), String> {
    let browsers = Browsers::find()?;
    let items = ItemWidgets::find()?;
    let texts = kismet::texts()?;
    // Our pages copy rows by the reflected size: it must be the game's own step.
    let stride = ugc_browser::result_stride()?;
    if stride != items.rows.size {
        return Err(format!(
            "rows are {:#x} bytes, the game's results step by {stride:#x}",
            items.rows.size
        ));
    }
    // SAFETY: `initialize_entry` is the implementation of the parameterless `InitializeEntry`
    // (found through its exec thunk), which takes only the widget.
    unsafe {
        hook::install_typed(
            "UUGCItemWidget::InitializeEntry_Implementation",
            items.initialize_entry,
            tiles::initialize_entry as WidgetFn,
            &tiles::ORIGINAL_INITIALIZE,
        )?;
    }
    if let Some(reason) = items.rows.no_times() {
        log!("vehicle folders: a folder's vehicles' times aren't shown ({reason})");
    }
    let _ = BROWSERS.set(browsers);
    let _ = ITEMS.set(items);
    let _ = TEXTS.set(texts);
    let loaded = folders_file::load();
    let (folders, vehicles) = (
        loaded.store.local.folders().count(),
        loaded.store.local.vehicles().count(),
    );
    with_state(|state| {
        state.store = loaded.store;
        state.writable = loaded.writable;
        state.ready = true;
    });
    log!("vehicle folders: {folders} local folders, {vehicles} vehicles filed");
    if let Err(reason) = subscribed::start() {
        log!("vehicle folders: no folders in Subscriptions ({reason})");
    }
    if let Err(reason) = buttons::start() {
        log!("vehicle folders: no buttons ({reason})");
    }
    match Decorations::find() {
        Ok(decor) => {
            if let Some(reason) = decor.no_thumbnails() {
                log!("vehicle folders: no thumbnails on folder tiles ({reason})");
            }
            if let Some(reason) = decor.no_accent() {
                log!("vehicle folders: folder tiles not in the accent color ({reason})");
            }
            let _ = DECOR.set(decor);
        }
        Err(reason) => log!("vehicle folders: no pins or thumbnails on the rows ({reason})"),
    }
    if let Err(reason) = panel::start() {
        log!("vehicle folders: no settings in the menu's panel ({reason})");
    }
    if let Err(reason) =
        ugc_browser::hook_clear_selection(tiles::clear_selection, &tiles::ORIGINAL_CLEAR)
    {
        log!("vehicle folders: the details' X doesn't unselect a folder ({reason})");
    }
    if let Err(reason) = details::start() {
        log!("vehicle folders: a selected folder isn't shown in the menu's panel ({reason})");
    }
    if let Err(reason) = bar::start() {
        log!("vehicle folders: no navigation bar ({reason})");
    }
    if let Err(reason) = drag::start() {
        log!("vehicle folders: no drag and drop ({reason})");
    }
    Ok(())
}

/// Writes the folders file after a change (logged if it fails).
fn save() {
    let result = with_state(|state| state.writable.then(|| folders_file::save(&state.store)));
    match result {
        Some(Err(reason)) => log!("vehicle folders: {reason}"),
        None => log!("vehicle folders: the file wasn't readable, changes aren't saved"),
        Some(Ok(())) => {}
    }
}
