//! The mod's settings in the game's options, as a sub-menu like the game's own (Gameplay, User
//! interface, ...): an "Editor tweaks" button among the options' buttons opens the context
//! `<root>/Options/EditorTweaks`, whose page lists every setting of [`crate::features::SETTINGS`]
//! by section, saved to `EditorTweaks.ini` as they change (see `framework::settings`). Back returns
//! to the options.
//!
//! Features' dialogs ([`Dialog`]) are made the same way: [`open`] opens the in-game menu (as Escape
//! does) and, once it opened its root context, the dialog's context `<root>/<context>`, whose page
//! lists the dialog's settings, with a button of its own that closes the menu and runs the
//! dialog's action. Back returns to the in-game menu.
//!
//! Features' pages ([`Page`]) too, opened with [`open_page`], but their title and buttons are made
//! each time they show ([`Page::make`]), and a button may open another page instead of closing the
//! menu: menus of their own, whose buttons change with what the feature does. A page's body may
//! list rows of its own above its settings ([`BodyRow`]: a text, with actions side by side or
//! alone), made each time it shows too: lists of things to act on, while its buttons only lead
//! elsewhere or do what concerns the whole page. Back on a page at
//! the top (opened only by [`open_page`]) closes the menu, back to where it was opened from.
//!
//! The game does the work: its menu opens the context (title, Back button, transition) and shows
//! the page, a settings page that lists the game's settings of its categories. Ours is the gameplay
//! settings page (`WBP_GameplaySettings`, nothing but a settings page for the gameplay category),
//! listing our category instead while in our context, and the game's settings list
//! (`UBrickUserSettings::ReflectBrickProperties`, shared through `features::settings_list`) gets our
//! rows when a page asks for our category. We only add our buttons and show our page where the
//! menu's Blueprint does for its own contexts: once it made the new context's buttons, before the
//! panel pools the unused ones. See `game::menu` and `game::property_reflection`.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;

use super::{menu_buttons, settings_list};
use crate::features::SETTINGS;
use crate::framework::hook;
use crate::framework::settings::Section;
use crate::framework::timings;
use crate::game::kismet;
use crate::game::menu::{
    self, AddPropertiesFn, ButtonGrid, Menus, OpenMenuFn, RemoveUnusedButtonsFn, WindowManager,
};
use crate::game::property_reflection::{Label, List, Value};
use crate::game::row_layout::{self, RowLayout, Widened};
use crate::game::text::Text;
use crate::game::weak_object::WeakObject;
use crate::platform::timer::ThreadTimer;
use crate::prelude::*;

pub struct SettingsMenu;

/// A feature's dialog: a page of the in-game menu listing settings, with a button running
/// [`Dialog::action`]. Added with [`add_dialog`], opened with [`open`].
pub struct Dialog {
    /// The last part of its context, a sub-context of the menu's root (as `Options` is).
    pub context: &'static str,
    pub title: &'static str,
    /// Its settings' category: a gameplay tag the game doesn't know, which only its page asks for.
    pub category: &'static str,
    pub sections: &'static [Section],
    /// The button's text, and its icon (a texture's path).
    pub button: &'static str,
    pub icon: &'static str,
    /// Runs when the button is clicked, once the menu is closed. During the click: keep it short,
    /// e.g. note a request for the editor's next tick.
    pub action: fn(),
}

/// A feature's page of the in-game menu, made anew each time it shows: a title, buttons after Back,
/// and settings listed beside them. Added with [`add_page`], opened with [`open_page`] or by
/// another page's button.
pub struct Page {
    /// Its context below the menu's root, parts separated by `/` (e.g. `Coop/CoopJoin`); the last
    /// part is unique among pages and dialogs. Back opens the context before.
    pub path: &'static str,
    /// Its settings' category (see [`Dialog::category`]), and the settings (may be none).
    pub category: &'static str,
    pub sections: &'static [Section],
    /// The titles of its body's sections, listed before its settings ([`Shown::body`]).
    pub body: &'static [&'static str],
    /// Its title, buttons and body now. Runs while the menu makes the page: only read what the
    /// feature noted, don't call into the feature's running code.
    pub make: fn() -> Shown,
}

/// What a [`Page`] shows.
pub struct Shown {
    pub title: String,
    pub buttons: Vec<Button>,
    /// The rows of each section of [`Page::body`], in order (missing ones: none). A section shows
    /// at most [`BODY_ROWS`] rows, and none without rows.
    pub body: Vec<Vec<BodyRow>>,
}

/// A row of a page's body: its text, and beside it its actions, or nothing.
pub struct BodyRow {
    pub text: String,
    pub value: BodyValue,
}

pub enum BodyValue {
    /// Actions side by side, as a list's items: clicking one runs it (keep it short, as a
    /// button's [`Button::run`]).
    Actions(Vec<(String, Action)>),
    /// Nothing: the row is only its text, across the whole row and centred (a list without items,
    /// which shows nothing beside its name, the name given nearly all the width:
    /// `game::row_layout`).
    Note,
}

pub type Action = Arc<dyn Fn() + Send + Sync>;

/// Rows a body section shows at most: they are made once, each showing the row in its place.
pub const BODY_ROWS: usize = 16;

/// The body rows each page showed last (by path), which its rows read as the menu lists them.
static BODIES: Mutex<Vec<(&'static str, Body)>> = Mutex::new(Vec::new());

/// A page's body: each section's rows.
type Body = Vec<Vec<BodyRow>>;

/// Row `row` of section `section` of the body of the page at `page`: whatever it showed last.
struct BodySlot {
    page: &'static str,
    section: usize,
    row: usize,
}

impl BodySlot {
    fn read<T>(&self, read: impl FnOnce(&BodyRow) -> T) -> Option<T> {
        let bodies = BODIES.lock().unwrap_or_else(PoisonError::into_inner);
        let (_, body) = bodies.iter().find(|(page, _)| *page == self.page)?;
        body.get(self.section)?.get(self.row).map(read)
    }
}

impl Label for BodySlot {
    fn label(&self) -> Option<String> {
        self.read(|row| row.text.clone())
    }
}

impl List for BodySlot {
    fn items(&self) -> Vec<String> {
        self.read(|row| match &row.value {
            BodyValue::Actions(actions) => actions.iter().map(|(name, _)| name.clone()).collect(),
            BodyValue::Note => Vec::new(),
        })
        .unwrap_or_default()
    }

    /// None (past the items: none shows chosen).
    fn chosen(&self) -> usize {
        self.read(|row| match &row.value {
            BodyValue::Actions(actions) => actions.len(),
            BodyValue::Note => 0,
        })
        .unwrap_or(0)
    }

    fn choose(&self, index: usize) -> bool {
        let action = self
            .read(|row| match &row.value {
                BodyValue::Actions(actions) => actions.get(index).map(|(_, run)| run.clone()),
                BodyValue::Note => None,
            })
            .flatten();
        // Run without the lock: the action may note a request, or make the page anew.
        if let Some(action) = action {
            action();
        }
        true
    }
}

/// The rows of `page`'s body, made once: [`BODY_ROWS`] per section, each showing what's in its place.
fn body_rows(page: &'static Page) -> Result<Vec<settings_list::Section>, String> {
    let leaf = page.path.rsplit('/').next().unwrap_or(page.path);
    let mut sections = Vec::new();
    for (section, &title) in page.body.iter().enumerate() {
        let mut rows = Vec::new();
        for row in 0..BODY_ROWS {
            let slot: &'static BodySlot = Box::leak(Box::new(BodySlot {
                page: page.path,
                section,
                row,
            }));
            rows.push(
                settings_list::row(
                    &format!("{leaf}_{section}_{row}"),
                    "",
                    "",
                    Value::List(slot),
                )?
                .labelled(slot),
            );
        }
        sections.push((title, rows));
    }
    Ok(sections)
}

/// A page's button.
pub struct Button {
    pub text: String,
    /// A texture's path.
    pub icon: &'static str,
    /// Runs each time it's clicked, before [`Button::then`]. Keep it short, e.g. note a request
    /// for the editor's next tick.
    pub run: Box<dyn FnMut() + Send>,
    pub then: Then,
}

/// What the menu does after a page's button ran.
pub enum Then {
    Close,
    /// Leaves the menu as it is (e.g. the button opened a popup over it).
    Stay,
    /// Opens the page at this path (see [`Page::path`]).
    Open(&'static str),
}

/// Lists `page`, to be opened with [`open_page`]. Before engine init (in `on_load`).
pub fn add_page(page: &'static Page) {
    PAGES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(page);
}

/// Opens the in-game menu on `page` (added with [`add_page`]). Game thread only.
pub fn open_page(page: &'static Page) {
    let Some(window) = WINDOW.get() else {
        log!("{} not opened: the menu wasn't found", page.path);
        return;
    };
    state().pending = Some((Pending::Page(page), Instant::now()));
    // SAFETY: on the game thread, per the contract.
    if !unsafe { window.open_menu() } {
        state().pending = None;
        log!("{} not opened: no window manager", page.path);
    }
}

/// Shows `page` in the menu showing one of the pages now, if one does. Game thread only.
pub fn show(page: &'static Page) {
    let Some(menus) = MENUS.get() else {
        return;
    };
    let menu = state().page_shown.as_ref().and_then(|(menu, _)| menu.get());
    if let Some(menu) = menu {
        // SAFETY: a live menu, on the game thread.
        unsafe { open_path(menus, menu, page.path) };
    }
}

/// Whether a menu shows one of the pages now. Game thread only.
pub fn page_showing() -> bool {
    let menu = state().page_shown.as_ref().and_then(|(menu, _)| menu.get());
    // SAFETY: on the game thread, per the contract.
    menu.is_some_and(|menu| {
        WINDOW
            .get()
            .is_some_and(|window| unsafe { window.is_active(menu) })
    })
}

/// Makes `page` anew where a menu shows it (as what it shows changed). Game thread only, and not
/// from the page's own [`Page::make`].
pub fn refresh(page: &'static Page) {
    let Some(menus) = MENUS.get() else {
        return;
    };
    let shown = state()
        .page_shown
        .as_ref()
        .filter(|(_, shown)| core::ptr::eq(*shown, page))
        .and_then(|(menu, _)| menu.get());
    let Some(menu) = shown else {
        return;
    };
    // Closed since (Escape): opening contexts would change the input mode of a menu not shown.
    // SAFETY: on the game thread, per the contract.
    if !WINDOW
        .get()
        .is_some_and(|window| unsafe { window.is_active(menu) })
    {
        state().page_shown = None;
        log!("{} not made again: its menu closed", page.path);
        return;
    }
    let before = page.path.rsplit_once('/').map_or("", |(before, _)| before);
    // SAFETY: a live menu, on the game thread. `OpenMenu` does nothing for the context shown: the
    // one before first.
    unsafe {
        open_path(menus, menu, before);
        open_path(menus, menu, page.path);
    }
}

/// Opens the context `path` below `menu`'s root (the root itself if empty).
///
/// # Safety
/// `menu` must be a live menu. Game thread only.
unsafe fn open_path(menus: &Menus, menu: *mut UObjectBase, path: &str) {
    let root = state().root.clone();
    let context = if path.is_empty() {
        root
    } else {
        format!("{root}/{path}")
    };
    match kismet::string_to_name(&context) {
        // SAFETY: per the contract.
        Ok(name) => unsafe { menus.open(menu, name) },
        Err(reason) => log!("{context} not opened: {reason}"),
    }
}

/// Lists `dialog`, to be opened with [`open`]. Before engine init (in `on_load`).
pub fn add_dialog(dialog: &'static Dialog) {
    DIALOGS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(dialog);
}

/// Opens the in-game menu on `dialog` (added with [`add_dialog`]). Game thread only.
pub fn open(dialog: &'static Dialog) {
    let Some(window) = WINDOW.get() else {
        log!("{} not opened: the menu wasn't found", dialog.title);
        return;
    };
    state().pending = Some((Pending::Dialog(dialog), Instant::now()));
    // SAFETY: on the game thread, per the contract.
    if !unsafe { window.open_menu() } {
        state().pending = None;
        log!("{} not opened: no window manager", dialog.title);
    }
}

/// The button's text and the page's title.
const TITLE: &str = "Editor tweaks";

/// Our category: a gameplay tag the game doesn't know, which only our page asks for.
const CATEGORY: &str = "Properties.Settings.EditorTweaks";

/// The button's icon.
const ICON: &str = "/Game/BrickRigs/UI/Textures/Icons/T_VehicleEditor.T_VehicleEditor";

/// Our page: the game's gameplay settings page, listing our category instead.
const PAGE: &str = "/Game/BrickRigs/UI/Menu/Settings/WBP_GameplaySettings.WBP_GameplaySettings_C";

/// The last part of the options' context (`MainMenu/Options`, `InGameMenu/Options`): it gets our
/// button.
const OPTIONS: &str = "Options";

/// The last part of our context, a sub-context of the options (as `Gameplay` is).
const CONTEXT: &str = "EditorTweaks";

/// The menu's root contexts (`UMenuWidget::NativeConstruct` opens one), where a dialog being
/// opened takes over.
const ROOTS: [&str; 2] = ["InGameMenu", "MainMenu"];

/// How long after [`open`] the menu may take to show its root (it loads its widget first).
const OPENING: Duration = Duration::from_secs(5);

static DIALOGS: Mutex<Vec<&'static Dialog>> = Mutex::new(Vec::new());
static PAGES: Mutex<Vec<&'static Page>> = Mutex::new(Vec::new());
static MENUS: OnceLock<Menus> = OnceLock::new();
static WINDOW: OnceLock<WindowManager> = OnceLock::new();
static OURS: OnceLock<Ours> = OnceLock::new();

static ORIGINAL_OPEN_MENU: OnceLock<OpenMenuFn> = OnceLock::new();
static ORIGINAL_REMOVE_UNUSED_BUTTONS: OnceLock<RemoveUnusedButtonsFn> = OnceLock::new();
static ORIGINAL_ADD_PROPERTIES: OnceLock<AddPropertiesFn> = OnceLock::new();

/// Names and texts, made once after engine init.
struct Ours {
    category: FName,
    title: Text,
    icon: FName,
    page: FName,
    context: FName,
    dialogs: Vec<DialogNames>,
    pages: Vec<PageNames>,
}

/// A page's names.
struct PageNames {
    page: &'static Page,
    /// The last part of its path.
    leaf: &'static str,
    category: FName,
}

/// A dialog's names and texts.
struct DialogNames {
    dialog: &'static Dialog,
    context: FName,
    title: Text,
    category: FName,
    button: Text,
    icon: FName,
}

// SAFETY: names and texts kept for the whole game; only used on the game thread.
unsafe impl Send for Ours {}
unsafe impl Sync for Ours {}

/// What the menus do. Game thread only; never locked while calling the game.
struct State {
    /// The menus opening a context (`OpenMenu` running), innermost last, with the context's last
    /// part.
    opening: Vec<(*mut UObjectBase, String)>,
    /// Our button and the menu showing it, until that menu opens another context.
    button: Option<(WeakObject, WeakObject)>,
    /// A dialog's button, the menu showing it and the dialog, until that menu opens another
    /// context.
    dialog_button: Option<(WeakObject, WeakObject, &'static Dialog)>,
    /// The menu in our context or a dialog's, and the category its page lists, until it opens
    /// another.
    in_ours: Option<(WeakObject, FName)>,
    /// The dialog or page asked for, until the menu shows its root.
    pending: Option<(Pending, Instant)>,
    /// The page a menu shows, and its buttons with what they do, until that menu opens another
    /// context.
    page_shown: Option<(WeakObject, &'static Page)>,
    page_buttons: Vec<PageButton>,
    /// How many times a page was made: a click whose button's page was made again meanwhile
    /// doesn't put the button back.
    pages_made: u64,
    /// The root of the menu last opened (`InGameMenu` in a world, `MainMenu` in the main menu).
    root: String,
    /// The rows widened for a page's notes, to put back before a page lists rows again.
    widened: Vec<Widened>,
    /// The page of ours whose notes wait for their rows ([`notes_tick`]), its path and since when.
    notes_waiting: Option<(WeakObject, &'static str, Instant)>,
}

/// A page's button shown, with what it runs and does next.
type PageButton = (WeakObject, Box<dyn FnMut() + Send>, Then);

/// What [`open`] or [`open_page`] asked for.
#[derive(Clone, Copy)]
enum Pending {
    Dialog(&'static Dialog),
    Page(&'static Page),
}

// SAFETY: the menus `OpenMenu` runs for (alive while it runs) and weak object references, only
// used on the game thread.
unsafe impl Send for State {}

static STATE: Mutex<State> = Mutex::new(State {
    opening: Vec::new(),
    button: None,
    dialog_button: None,
    in_ours: None,
    pending: None,
    page_shown: None,
    page_buttons: Vec::new(),
    pages_made: 0,
    root: String::new(),
    widened: Vec::new(),
    notes_waiting: None,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Feature for SettingsMenu {
    fn id(&self) -> &'static str {
        "settings_menu"
    }

    fn on_load(&self) -> LoadResult {
        settings_list::install()
    }

    fn on_engine_init(&self) {
        match start() {
            Ok(()) => log!("active: \"{TITLE}\" in the options"),
            Err(reason) => log!("not in the options: {reason}"),
        }
    }
}

/// The rows of `sections`, by section (sections without settings to show are left out).
fn rows(
    sections: impl IntoIterator<Item = &'static Section>,
) -> Result<Vec<settings_list::Section>, String> {
    let mut rows = Vec::new();
    for section in sections {
        if section.settings.is_empty() {
            continue;
        }
        let section_rows = section
            .settings
            .iter()
            .map(|&setting| settings_list::setting_row(setting))
            .collect::<Result<Vec<_>, _>>()?;
        rows.push((section.title, section_rows));
    }
    Ok(rows)
}

/// Makes our names, texts and rows, finds the menus and hooks them.
fn start() -> Result<(), String> {
    settings_list::add(CATEGORY, rows(SETTINGS.iter().copied().flatten())?)?;
    let dialogs = DIALOGS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let mut dialog_names = Vec::new();
    for dialog in dialogs {
        settings_list::add(dialog.category, rows(dialog.sections)?)?;
        dialog_names.push(DialogNames {
            dialog,
            context: kismet::string_to_name(dialog.context)?,
            title: kismet::string_to_text(dialog.title)?,
            category: kismet::string_to_name(dialog.category)?,
            button: kismet::string_to_text(dialog.button)?,
            icon: kismet::string_to_name(dialog.icon)?,
        });
    }
    let pages = PAGES.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let mut page_names = Vec::new();
    for page in pages {
        if !page.body.is_empty() {
            settings_list::add(page.category, body_rows(page)?)?;
        }
        settings_list::add(page.category, rows(page.sections)?)?;
        page_names.push(PageNames {
            page,
            leaf: page.path.rsplit('/').next().unwrap_or(page.path),
            category: kismet::string_to_name(page.category)?,
        });
    }
    let _ = OURS.set(Ours {
        category: kismet::string_to_name(CATEGORY)?,
        title: kismet::string_to_text(TITLE)?,
        icon: kismet::string_to_name(ICON)?,
        page: kismet::string_to_name(PAGE)?,
        context: kismet::string_to_name(CONTEXT)?,
        dialogs: dialog_names,
        pages: page_names,
    });
    match WindowManager::find() {
        Ok(window) => {
            let _ = WINDOW.set(window);
        }
        Err(reason) => log!("dialogs don't open: {reason}"),
    }
    let found = Menus::find()?;
    let menus = MENUS.get_or_init(|| found);
    // SAFETY: each target is the start of the function its detour matches (see `game::menu`); the
    // menus are only used on the game thread, which is running this.
    unsafe {
        hook::install_typed(
            "UMenuWidget::OpenMenu",
            menus.open_menu,
            open_menu as OpenMenuFn,
            &ORIGINAL_OPEN_MENU,
        )?;
        hook::install_typed(
            "UMenuButtonPanelWidget::RemoveUnusedButtons",
            menus.remove_unused_buttons,
            remove_unused_buttons as RemoveUnusedButtonsFn,
            &ORIGINAL_REMOVE_UNUSED_BUTTONS,
        )?;
        hook::install_typed(
            "UMenuSettingsPageWidget::AddProperties_Implementation",
            menus.add_properties,
            add_properties as AddPropertiesFn,
            &ORIGINAL_ADD_PROPERTIES,
        )?;
    }
    menu_buttons::listen(clicked)?;
    menu_buttons::intercept(before_click)
}

/// A menu opens a context: notes which while it does (see `remove_unused_buttons`). A menu showing
/// its root while a dialog is being opened goes on to the dialog.
unsafe extern "C" fn open_menu(menu: *mut UObjectBase, context: FName) {
    let _span = timings::span("settings_menu::open_menu");
    let Some(original) = ORIGINAL_OPEN_MENU.get() else {
        return;
    };
    // SAFETY: the name the game passes.
    let leaf = unsafe { menu::context_leaf(&context) };
    state().opening.push((menu, leaf.clone()));
    // SAFETY: forwards the caller's arguments.
    unsafe { timings::game("UMenuWidget::OpenMenu", || original(menu, context)) };
    state().opening.pop();
    if !ROOTS.contains(&leaf.as_str()) {
        return;
    }
    let pending = {
        let mut state = state();
        state.root = leaf;
        state.pending.take()
    };
    let Some((pending, asked)) = pending.filter(|(_, asked)| asked.elapsed() < OPENING) else {
        return;
    };
    let (Some(menus), Some(ours)) = (MENUS.get(), OURS.get()) else {
        return;
    };
    match pending {
        Pending::Dialog(dialog) => {
            if let Some(names) = ours
                .dialogs
                .iter()
                .find(|names| core::ptr::eq(names.dialog, dialog))
            {
                log!(
                    "{} opened after {} ms",
                    dialog.title,
                    asked.elapsed().as_millis()
                );
                // SAFETY: the live menu `OpenMenu` ran for, on the game thread.
                unsafe { menus.open(menu, names.context) };
            }
        }
        Pending::Page(page) => {
            log!(
                "{} opened after {} ms",
                page.path,
                asked.elapsed().as_millis()
            );
            // SAFETY: as above.
            unsafe { open_path(menus, menu, page.path) };
        }
    }
}

/// A button panel pools the buttons it doesn't use any more. A menu's panel does so once the menu
/// made its new context's buttons: ours go in first.
unsafe extern "C" fn remove_unused_buttons(panel: *mut UObjectBase) {
    let _span = timings::span("settings_menu::remove_unused_buttons");
    let Some(original) = ORIGINAL_REMOVE_UNUSED_BUTTONS.get() else {
        return;
    };
    let opening = state().opening.last().cloned();
    let mut made_page = false;
    if let (Some((menu, leaf)), Some(menus), Some(ours)) = (opening, MENUS.get(), OURS.get()) {
        // SAFETY: the menu `OpenMenu` runs for, on the game thread.
        unsafe {
            if menus.button_panel(menu) == Some(panel) {
                made_page = context_made(menus, ours, menu, panel, &leaf);
            }
        }
    }
    // SAFETY: forwards the caller's argument.
    unsafe {
        timings::game("UMenuButtonPanelWidget::RemoveUnusedButtons", || {
            original(panel)
        })
    };
    if let (true, Some(menus)) = (made_page, MENUS.get()) {
        // SAFETY: the menu's live panel, done making buttons, on the game thread.
        unsafe { check_order(menus, panel) };
    }
}

/// A page's buttons were made, the rest pooled: Back (the game's) first, then the page's, each in
/// its cell and shown. The game makes them so, but the coop page once showed one of its buttons
/// above Back (2026-11-01, cause not found): what was wrong is put back and logged.
///
/// # Safety
/// `panel` must be a live menu's button panel, done making buttons. Game thread only.
unsafe fn check_order(menus: &Menus, panel: *mut UObjectBase) {
    static GRID: OnceLock<Option<ButtonGrid>> = OnceLock::new();
    let grid = GRID.get_or_init(|| match ButtonGrid::find() {
        Ok(grid) => Some(grid),
        Err(reason) => {
            log!("pages' buttons not checked: {reason}");
            None
        }
    });
    let Some(grid) = grid else {
        return;
    };
    let (path, ours) = {
        let state = state();
        let path = state.page_shown.as_ref().map_or("", |(_, page)| page.path);
        let ours: Vec<_> = state
            .page_buttons
            .iter()
            .filter_map(|(button, _, _)| button.get())
            .collect();
        (path, ours)
    };
    // SAFETY: per the contract.
    let wrong = unsafe { grid.put_in_order(&menus.panels, panel, &ours) };
    if !wrong.is_empty() {
        log!("{path}: buttons put back in order: {}", wrong.join("; "));
    }
}

/// `menu` made the buttons of its new context, whose last part is `leaf`, on `panel`: the options
/// get our button, our context our page, a dialog's context its page and button. Whether it was a
/// feature's page ([`Page`]).
///
/// # Safety
/// `menu` must be a live menu and `panel` its button panel. Game thread only.
unsafe fn context_made(
    menus: &Menus,
    ours: &Ours,
    menu: *mut UObjectBase,
    panel: *mut UObjectBase,
    leaf: &str,
) -> bool {
    {
        // The panel reuses its buttons for the new context, and the menu left ours.
        let mut state = state();
        if state
            .button
            .as_ref()
            .is_some_and(|(_, shown_in)| shown_in.is(menu))
        {
            state.button = None;
        }
        if state
            .dialog_button
            .as_ref()
            .is_some_and(|(_, shown_in, _)| shown_in.is(menu))
        {
            state.dialog_button = None;
        }
        if state
            .in_ours
            .as_ref()
            .is_some_and(|(in_ours, _)| in_ours.is(menu))
        {
            state.in_ours = None;
        }
        if state
            .page_shown
            .as_ref()
            .is_some_and(|(shown_in, _)| shown_in.is(menu))
        {
            state.page_shown = None;
            state.page_buttons.clear();
        }
    }
    // SAFETY: per the contract.
    unsafe {
        match leaf {
            OPTIONS => match menus.panels.add_button(panel, &ours.title, ours.icon) {
                Some(button) => {
                    state().button = Some((WeakObject::new(button), WeakObject::new(menu)));
                }
                None => log!("no button added to the options"),
            },
            CONTEXT => {
                state().in_ours = Some((WeakObject::new(menu), ours.category));
                menus.show_page(menu, ours.page, &ours.title);
            }
            _ => {
                if let Some(names) = ours.pages.iter().find(|names| names.leaf == leaf) {
                    show_page(menus, ours, menu, panel, names);
                    return true;
                }
                let Some(names) = ours
                    .dialogs
                    .iter()
                    .find(|names| names.dialog.context == leaf)
                else {
                    return false;
                };
                match menus.panels.add_button(panel, &names.button, names.icon) {
                    Some(button) => {
                        state().dialog_button =
                            Some((WeakObject::new(button), WeakObject::new(menu), names.dialog));
                    }
                    None => log!("no button added to {}", names.dialog.title),
                }
                state().in_ours = Some((WeakObject::new(menu), names.category));
                menus.show_page(menu, ours.page, &names.title);
            }
        }
    }
    false
}

/// Shows a page in `menu` (whose context is the page's), made now: its buttons on `panel`, its
/// title and settings.
///
/// # Safety
/// As for [`context_made`].
unsafe fn show_page(
    menus: &Menus,
    ours: &Ours,
    menu: *mut UObjectBase,
    panel: *mut UObjectBase,
    names: &PageNames,
) {
    let shown = (names.page.make)();
    {
        let mut bodies = BODIES.lock().unwrap_or_else(PoisonError::into_inner);
        bodies.retain(|(page, _)| *page != names.page.path);
        bodies.push((names.page.path, shown.body));
    }
    let mut buttons = Vec::new();
    for button in shown.buttons {
        let made = kismet::string_to_text(&button.text)
            .and_then(|text| Ok((text, kismet::string_to_name(button.icon)?)))
            // SAFETY: per the contract.
            .map(|(text, icon)| unsafe { menus.panels.add_button(panel, &text, icon) });
        match made {
            // SAFETY: a live button just made.
            Ok(Some(made)) => {
                buttons.push((unsafe { WeakObject::new(made) }, button.run, button.then))
            }
            Ok(None) => log!("{}: button \"{}\" not added", names.page.path, button.text),
            Err(reason) => log!(
                "{}: button \"{}\" not added: {reason}",
                names.page.path,
                button.text
            ),
        }
    }
    {
        let mut state = state();
        // SAFETY: a live menu, per the contract.
        let weak_menu = unsafe { WeakObject::new(menu) };
        state.page_shown = Some((weak_menu, names.page));
        state.page_buttons = buttons;
        state.pages_made += 1;
        state.in_ours = Some((weak_menu, names.category));
    }
    match kismet::string_to_text(&shown.title) {
        // SAFETY: per the contract.
        Ok(title) => unsafe { menus.show_page(menu, ours.page, &title) },
        Err(reason) => log!("{}: no page: {reason}", names.page.path),
    }
}

/// A menu button was clicked (`menu_buttons`): ours opens our context, as the options' buttons open
/// theirs; a dialog's closes the menu and runs the dialog's action; a page's runs and does what it
/// says next.
fn clicked(button: *mut UObjectBase) {
    let (Some(menus), Some(ours)) = (MENUS.get(), OURS.get()) else {
        return;
    };
    // SAFETY: on the game thread (button clicks are UI code); ours have no delegate, so the click
    // changed nothing yet.
    unsafe {
        let options = shown_in(menus, button, |state| {
            state
                .button
                .as_ref()
                .map(|(ours, shown_in)| (*ours, *shown_in))
        });
        if let Some(menu) = options {
            menus.open(menu, ours.context);
            return;
        }
        let dialog = state()
            .dialog_button
            .as_ref()
            .filter(|(ours, _, _)| ours.is(button))
            .map(|(_, _, dialog)| *dialog);
        let shown = shown_in(menus, button, |state| {
            state
                .dialog_button
                .as_ref()
                .map(|(ours, shown_in, _)| (*ours, *shown_in))
        });
        if let (Some(menu), Some(dialog)) = (shown, dialog) {
            menus.close(menu);
            (dialog.action)();
            return;
        }
        page_clicked(menus, button);
    }
}

/// `button` was clicked: if it's a page's, runs it and does what it says next.
///
/// # Safety
/// Game thread only.
unsafe fn page_clicked(menus: &Menus, button: *mut UObjectBase) {
    let picked = {
        let mut state = state();
        let menu = state.page_shown.as_ref().and_then(|(menu, _)| menu.get());
        let at = state
            .page_buttons
            .iter()
            .position(|(ours, _, _)| ours.is(button));
        match (menu, at) {
            (Some(menu), Some(at)) => {
                Some((menu, at, state.pages_made, state.page_buttons.remove(at)))
            }
            _ => None,
        }
    };
    let Some((menu, at, made, (ours, mut run, then))) = picked else {
        return;
    };
    // SAFETY: a live menu; the button must still be among its panel's (not pooled and reused by
    // another panel since).
    let shown = unsafe {
        menus
            .button_panel(menu)
            .is_some_and(|panel| menus.panels.shows(panel, button))
    };
    if !shown {
        return;
    }
    run();
    match then {
        Then::Close => {
            state().page_shown = None;
            // SAFETY: as above.
            unsafe { menus.close(menu) };
        }
        Then::Stay => {
            // The page stays: its button works again (a question cancelled is asked again), and
            // isn't taken for Back. Unless `run` made a page meanwhile, its buttons now.
            let mut state = state();
            if state.pages_made == made {
                let at = at.min(state.page_buttons.len());
                state.page_buttons.insert(at, (ours, run, Then::Stay));
            }
        }
        // SAFETY: as above.
        Then::Open(path) => unsafe { open_path(menus, menu, path) },
    }
}

/// Before the game sees a click: Back on a page at the top closes the menu instead of showing its
/// root. The page's buttons are Back and ours, so a button of its panel that isn't ours is Back.
fn before_click(button: *mut UObjectBase) -> bool {
    let Some(menus) = MENUS.get() else {
        return false;
    };
    let shown = {
        let state = state();
        let top = state
            .page_shown
            .as_ref()
            .filter(|(_, page)| !page.path.contains('/'))
            .and_then(|(menu, _)| menu.get());
        top.filter(|_| {
            !state
                .page_buttons
                .iter()
                .any(|(ours, _, _)| ours.is(button))
        })
    };
    let Some(menu) = shown else {
        return false;
    };
    // SAFETY: a live menu, on the game thread (a click); the button must be among its panel's.
    unsafe {
        let back = menus
            .button_panel(menu)
            .is_some_and(|panel| menus.panels.shows(panel, button));
        if back {
            state().page_shown = None;
            menus.close(menu);
        }
        back
    }
}

/// The menu showing `button`, if it's the button `ours` picks from the state (with the menu it was
/// shown in).
///
/// # Safety
/// Game thread only.
unsafe fn shown_in(
    menus: &Menus,
    button: *mut UObjectBase,
    ours: impl FnOnce(&State) -> Option<(WeakObject, WeakObject)>,
) -> Option<*mut UObjectBase> {
    let menu = ours(&state())
        .filter(|(ours, _)| ours.is(button))
        .and_then(|(_, shown_in)| shown_in.get())?;
    // SAFETY: a live menu; the button must still be among its panel's (not pooled and reused by
    // another panel since).
    unsafe {
        let panel = menus.button_panel(menu)?;
        menus.panels.shows(panel, button).then_some(menu)
    }
}

/// A settings page lists its settings: the page shown in our context (or a dialog's) lists our
/// category (or the dialog's), for the time of it (pages are pooled and reused, so it keeps its
/// own).
unsafe extern "C" fn add_properties(page: *mut UObjectBase) {
    let _span = timings::span("settings_menu::add_properties");
    let Some(original) = ORIGINAL_ADD_PROPERTIES.get() else {
        return;
    };
    let Some(menus) = MENUS.get() else {
        // SAFETY: forwards the caller's argument.
        unsafe {
            timings::game(
                "UMenuSettingsPageWidget::AddProperties_Implementation",
                || original(page),
            )
        };
        return;
    };
    // Rows are reused: what notes changed goes back first, and notes still waiting are another
    // listing's.
    let widened = {
        let mut state = state();
        state.notes_waiting = None;
        std::mem::take(&mut state.widened)
    };
    if let Some(layout) = layout() {
        for row in &widened {
            // SAFETY: on the game thread (a page lists its rows).
            unsafe { layout.restore(row) };
        }
    }
    let in_ours = state()
        .in_ours
        .as_ref()
        .and_then(|(menu, category)| Some((menu.get()?, *category)));
    // SAFETY: the live page being constructed and a live menu, on the game thread.
    unsafe {
        let Some((menu, category)) = in_ours.filter(|&(menu, _)| menus.current_page(menu) == page)
        else {
            timings::game(
                "UMenuSettingsPageWidget::AddProperties_Implementation",
                || original(page),
            );
            return;
        };
        let categories = menus.categories(page);
        let own = categories.to_vec();
        categories.fill(category);
        timings::game(
            "UMenuSettingsPageWidget::AddProperties_Implementation",
            || original(page),
        );
        for (category, own) in menus.categories(page).iter_mut().zip(own) {
            *category = own;
        }
        lay_out_notes_soon(menu, page);
    }
}

/// The rows' layout, found when a page first has notes (`None` if it couldn't be: notes keep the
/// row's layout, their text in the name's part).
fn layout() -> Option<&'static RowLayout> {
    static LAYOUT: OnceLock<Option<RowLayout>> = OnceLock::new();
    LAYOUT
        .get_or_init(|| {
            row_layout::find()
                .inspect_err(|reason| log!("notes keep the rows' layout: {reason}"))
                .ok()
        })
        .as_ref()
}

/// How often [`notes_tick`] looks for a page's rows: about every frame.
const NOTES_TICK_MS: u32 = 10;

/// How long notes wait for their rows before they're left as they are.
const NOTES_WAIT: Duration = Duration::from_secs(2);

thread_local! {
    /// Runs [`notes_tick`] while a page's notes wait for their rows.
    static NOTES_TIMER: RefCell<Option<ThreadTimer>> = const { RefCell::new(None) };
}

/// `page` lists its rows in `menu`: if it's a page of ours showing notes, their rows get the whole
/// width once they're there (`BodyValue::Note`). The panel lists them after this, and a row gets
/// its value widget a frame later, so [`notes_tick`] widens them from the next frame on.
///
/// # Safety
/// `menu` must be a live menu and `page` its live settings page. Game thread only.
unsafe fn lay_out_notes_soon(menu: *mut UObjectBase, page: *mut UObjectBase) {
    let shown = state()
        .page_shown
        .as_ref()
        .filter(|(shown_in, _)| shown_in.is(menu))
        .map(|(_, page)| page.path);
    let Some(path) = shown else {
        return;
    };
    if notes_of(path).is_empty() || layout().is_none() {
        return;
    }
    // SAFETY: per the contract, a live page.
    let page = unsafe { WeakObject::new(page) };
    state().notes_waiting = Some((page, path, Instant::now()));
    NOTES_TIMER.with_borrow_mut(|timer| {
        if timer.is_none() {
            *timer = ThreadTimer::start(NOTES_TICK_MS, notes_tick);
            if timer.is_none() {
                log!("notes keep the rows' layout: no thread timer");
            }
        }
    });
}

/// The texts of the notes the page at `path` showed last.
fn notes_of(path: &str) -> Vec<String> {
    let bodies = BODIES.lock().unwrap_or_else(PoisonError::into_inner);
    bodies
        .iter()
        .filter(|(shown, _)| *shown == path)
        .flat_map(|(_, body)| body.iter().flatten())
        .filter(|row| matches!(row.value, BodyValue::Note))
        .map(|row| row.text.clone())
        .collect()
}

/// Widens the rows of the waiting page's notes, until all are or [`NOTES_WAIT`] passed. On the
/// game thread (a thread timer of it).
fn notes_tick() {
    let _span = timings::span("settings_menu::notes_tick");
    let waiting = state()
        .notes_waiting
        .as_ref()
        .map(|(page, path, since)| (page.get(), *path, *since));
    let (Some((Some(page), path, since)), Some(layout)) = (waiting, layout()) else {
        stop_notes();
        return;
    };
    let notes = notes_of(path);
    // What each row showed and whether it's a note across its row, for the log.
    let mut seen = Vec::new();
    let mut widened = Vec::new();
    // SAFETY: a live page (its weak reference), on the game thread; its rows are live widgets.
    unsafe {
        for row in layout.rows(page) {
            let name = layout.name(row);
            let note = name.as_ref().is_some_and(|name| notes.contains(name));
            let mut wide = note && layout.is_wide(row);
            if note
                && !wide
                && let Some(done) = layout.widen(row)
            {
                widened.push(done);
                wide = true;
            }
            seen.push((name, note, wide));
        }
    }
    state().widened.extend(widened);
    let wide = seen.iter().filter(|&&(_, _, wide)| wide).count();
    if wide < notes.len() && since.elapsed() < NOTES_WAIT {
        return;
    }
    stop_notes();
    // Told when it changes, not at every refresh.
    static LAST: Mutex<(usize, usize)> = Mutex::new((0, 0));
    let counts = (wide, notes.len());
    if std::mem::replace(
        &mut *LAST.lock().unwrap_or_else(PoisonError::into_inner),
        counts,
    ) == counts
    {
        return;
    }
    log!(
        "{path}: {wide} of {} notes across their rows after {} ms",
        notes.len(),
        since.elapsed().as_millis()
    );
    if wide < notes.len() {
        let rows: Vec<String> = seen
            .iter()
            .map(|(name, note, wide)| match (name, *note && !wide) {
                (None, _) => "(no name)".to_owned(),
                (Some(name), true) => format!("\"{name}\" (not widened)"),
                (Some(name), false) => format!("\"{name}\""),
            })
            .collect();
        let rows = if rows.is_empty() {
            "none listed".to_owned()
        } else {
            rows.join(", ")
        };
        log!("{path}: rows {rows}");
    }
}

/// Stops [`notes_tick`] (also from inside it).
fn stop_notes() {
    state().notes_waiting = None;
    NOTES_TIMER.with_borrow_mut(|timer| *timer = None);
}
