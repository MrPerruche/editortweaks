//! The mod's menu in the vehicle editor's top bar, "Editor tweaks", next to the game's File,
//! Modify, Selection and Groups, before the exit buttons. Its entries are keybinds made for it ([`keybinds::menu_keybinds`]),
//! each run by the function a feature gives [`on_click`]; they show like the game's actions, with
//! their key.
//!
//! How (`game::input_help`, `docs/re/editor-menu.md`): after the top bar's Blueprint adds the game's
//! categories (`PopulateDashboard`, hooked), the menu's category ([`keybinds::menu_category`]) is
//! added the same way. Opened, the input help lists the editor's bound actions of the category,
//! which ours has none of: after it updates (`UpdateActions`, hooked) the menu's actions are shown
//! instead, run through the editor's input component (remembered from the editor's tick). That
//! component doesn't know them, so `ExecuteAction` (hooked) answers for them: enabled, and on a
//! click the feature's function runs.
//!
//! An entry given [`on_click_when`] is grey while it isn't available, and while it is it also shows
//! as a row of the editor's bottom bar, added after the bar's own widgets the same way;
//! [`refresh_bottom_bar`] has the bar list its widgets again when that changes.

use std::sync::{Mutex, Once, OnceLock, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;

use super::keybinds::{self, Keybind};
use crate::framework::{hook, timings};
use crate::game::brick_editor;
use crate::game::input_help::{self, ExecuteActionFn, InputHelp, PopulateFn, UpdateActionsFn};
use crate::game::weak_object::WeakObject;
use crate::prelude::*;

/// What a click on a menu entry runs. During the click (the input help's event): keep it short,
/// e.g. note a request for the editor's next tick.
pub type Handler = fn();

/// Runs `handler` when `keybind`'s menu entry is clicked. Entries without one aren't shown. Before
/// engine init (in `on_load`).
pub fn on_click(keybind: &'static Keybind, handler: Handler) {
    add_entry(Entry {
        keybind,
        handler,
        available: None,
    });
}

/// [`on_click`] for an entry that only applies at times, as `available` tells: otherwise it's grey,
/// and when it is it also shows in the editor's bottom bar. `available` runs whenever the game asks
/// about the entry, and while the bottom bar lists its widgets: keep it short. Call
/// [`refresh_bottom_bar`] when its answer changes. Before engine init (in `on_load`).
pub fn on_click_when(keybind: &'static Keybind, handler: Handler, available: fn() -> bool) {
    add_entry(Entry {
        keybind,
        handler,
        available: Some(available),
    });
}

fn add_entry(entry: Entry) {
    ENTRIES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(entry);
}

/// Has the editor's bottom bar list its widgets again, ours among them as they're available now.
/// Game thread only, not while it's listing them.
pub fn refresh_bottom_bar() {
    let bar = BOTTOM_BAR
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|bar| bar.0.get_live());
    if let (Some(game), Some(bar)) = (GAME.get(), bar) {
        // SAFETY: a live dashboard, on the game thread, per the contract not while it populates.
        unsafe { game.update_dashboard(bar) };
    }
}

pub struct EditorMenu;

/// A menu entry that something runs.
#[derive(Clone, Copy)]
struct Entry {
    keybind: &'static Keybind,
    handler: Handler,
    /// `None`: always.
    available: Option<fn() -> bool>,
}

impl Entry {
    fn is_available(&self) -> bool {
        self.available.is_none_or(|available| available())
    }
}

static ENTRIES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
static GAME: OnceLock<InputHelp> = OnceLock::new();
/// The editor's input component, which the menu's actions are shown run through (any input
/// component would do: [`execute_action`] answers for them first).
static EDITOR_INPUT: Mutex<Option<Kept>> = Mutex::new(None);
/// The editor's bottom bar, last seen populating.
static BOTTOM_BAR: Mutex<Option<Kept>> = Mutex::new(None);

/// A weak pointer to an object kept between calls.
struct Kept(WeakObject);

// SAFETY: a weak pointer, only used on the game thread.
unsafe impl Send for Kept {}

static ORIGINAL_POPULATE: OnceLock<PopulateFn> = OnceLock::new();
static ORIGINAL_UPDATE_ACTIONS: OnceLock<UpdateActionsFn> = OnceLock::new();
static ORIGINAL_EXECUTE: OnceLock<ExecuteActionFn> = OnceLock::new();

static LOG_ADDED: Once = Once::new();
static LOG_NOT_MOVED: Once = Once::new();
static LOG_SHOWN: Once = Once::new();
static LOG_NO_CATEGORY: Once = Once::new();
static LOG_NO_ACTIONS: Once = Once::new();
static LOG_BAR: Once = Once::new();

impl Feature for EditorMenu {
    fn id(&self) -> &'static str {
        "editor_menu"
    }

    fn on_load(&self) -> LoadResult {
        brick_editor::after_tick(remember_input)
    }

    fn on_engine_init(&self) {
        match engine_init() {
            Ok(()) => log!("active: Editor tweaks menu in the editor's top bar"),
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

fn engine_init() -> Result<(), String> {
    let execute = input_help::EXECUTE_ACTION.resolve()?;
    let found = InputHelp::find()?;
    let game = GAME.get_or_init(|| found);
    // SAFETY: each target is the start of the function its detour matches (see
    // `game::input_help`), only called on the game thread.
    unsafe {
        hook::install_typed(
            "UDashboardWidget::PopulateDashboard",
            game.populate,
            populate as PopulateFn,
            &ORIGINAL_POPULATE,
        )?;
        hook::install_typed(
            "UInputHelpWidget::UpdateActions",
            game.update_actions,
            update_actions as UpdateActionsFn,
            &ORIGINAL_UPDATE_ACTIONS,
        )?;
        hook::install_typed(
            input_help::EXECUTE_ACTION.name,
            execute,
            execute_action as ExecuteActionFn,
            &ORIGINAL_EXECUTE,
        )
    }
}

/// The menu's actions that something runs, in the menu's order.
fn menu_actions() -> Vec<FName> {
    let entries = ENTRIES.lock().unwrap_or_else(PoisonError::into_inner);
    keybinds::menu_keybinds()
        .filter(|keybind| {
            entries
                .iter()
                .any(|entry| core::ptr::eq(entry.keybind, *keybind))
        })
        .filter_map(Keybind::action)
        .collect()
}

/// `name`'s menu entry, if it's one.
fn entry_of(name: &FName) -> Option<Entry> {
    let entries = ENTRIES.lock().unwrap_or_else(PoisonError::into_inner);
    entries
        .iter()
        .find(|entry| {
            entry
                .keybind
                .action()
                .is_some_and(|action| same(&action, name))
        })
        .copied()
}

/// The actions of the entries shown in the bottom bar now.
fn bottom_bar_actions() -> Vec<FName> {
    let entries: Vec<Entry> = ENTRIES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|entry| entry.available.is_some())
        .copied()
        .collect();
    entries
        .iter()
        .filter(|entry| entry.is_available())
        .filter_map(|entry| entry.keybind.action())
        .collect()
}

/// The bottom bar populated: remembers it, and adds the entries available now.
///
/// # Safety
/// `bar` must be the live bottom bar, populating. Game thread only.
unsafe fn populate_bottom_bar(game: &InputHelp, bar: *mut UObjectBase) {
    // SAFETY: per the contract.
    *BOTTOM_BAR.lock().unwrap_or_else(PoisonError::into_inner) =
        Some(Kept(unsafe { WeakObject::new(bar) }));
    let actions = bottom_bar_actions();
    if actions.is_empty() {
        return;
    }
    let input = EDITOR_INPUT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|input| input.0.get_live());
    let Some(input) = input else {
        return;
    };
    for action in actions {
        // SAFETY: per the contract; the component is alive, the name is a table's row's.
        match unsafe { game.add_bar_action(bar, input, action) } {
            Ok(()) => LOG_BAR.call_once(|| log!("an entry added to the editor's bottom bar")),
            Err(reason) => LOG_BAR.call_once(|| log!("bottom bar entry not added: {reason}")),
        }
    }
}

fn same(a: &FName, b: &FName) -> bool {
    a.comparison_index == b.comparison_index && a.number == b.number
}

/// A dashboard adds its widgets: after the top bar's, the menu.
unsafe extern "C" fn populate(dashboard: *mut UObjectBase) {
    let _span = timings::span("editor_menu::populate");
    let Some(original) = ORIGINAL_POPULATE.get() else {
        return;
    };
    // SAFETY: forwards the caller's argument.
    unsafe {
        timings::game("UDashboardWidget::PopulateDashboard", || {
            original(dashboard)
        })
    };
    let Some(game) = GAME.get() else {
        return;
    };
    // SAFETY: the live dashboard populating, on the game thread.
    if unsafe { game.is_bottom_bar(dashboard) } {
        // SAFETY: as above.
        unsafe { populate_bottom_bar(game, dashboard) };
        return;
    }
    let Some(menu) = keybinds::menu_category() else {
        LOG_NO_CATEGORY.call_once(|| log!("menu not added: its category isn't made yet"));
        return;
    };
    if menu_actions().is_empty() {
        LOG_NO_ACTIONS.call_once(|| log!("menu not added: no entry has a handler"));
        return;
    }
    // SAFETY: the live dashboard populating, on the game thread; the category lives for the whole
    // game.
    unsafe {
        if !game.is_top_bar(dashboard) {
            return;
        }
        if let Err(reason) = game.add_category(dashboard, menu) {
            LOG_ADDED.call_once(|| log!("menu not added: {reason}"));
            return;
        }
        LOG_ADDED.call_once(|| log!("menu added to the editor's top bar"));
        if let Err(reason) = game.move_before_actions(dashboard) {
            LOG_NOT_MOVED.call_once(|| log!("menu left after the exit buttons: {reason}"));
        }
    }
}

/// The editor ticked: remembers its input component, if it's a new one.
fn remember_input(editor: *mut UObjectBase, _delta: f32) {
    let Some(game) = GAME.get() else {
        return;
    };
    let mut remembered = EDITOR_INPUT.lock().unwrap_or_else(PoisonError::into_inner);
    // SAFETY: the ticking editor, on the game thread.
    let Some(input) = (unsafe { game.editor_input(editor) }) else {
        return;
    };
    if remembered
        .as_ref()
        .is_some_and(|known| known.0.is(input) && known.0.get_live().is_some())
    {
        return;
    }
    // SAFETY: the editor's live component.
    *remembered = Some(Kept(unsafe { WeakObject::new(input) }));
}

/// The input help lists its actions (each frame, from its tick; the game's code only lists them
/// again when the input stack changed): for the menu, ours, whenever the game's listed its own
/// (none) in their place.
unsafe extern "C" fn update_actions(help: *mut UObjectBase, force: bool) {
    let _span = timings::span("editor_menu::update_actions");
    let Some(original) = ORIGINAL_UPDATE_ACTIONS.get() else {
        return;
    };
    // SAFETY: forwards the caller's arguments.
    unsafe { timings::game("UInputHelpWidget::UpdateActions", || original(help, force)) };
    let (Some(game), Some(menu)) = (GAME.get(), keybinds::menu_category()) else {
        return;
    };
    // SAFETY: the live input help, on the game thread.
    if unsafe { game.shown_category(help) } != Some(menu) {
        return;
    }
    let actions = menu_actions();
    // SAFETY: as above.
    let shown = unsafe { game.shown_action_count(help) };
    if shown == Some(actions.len() as i32) {
        // Ours still.
        return;
    }
    let input = EDITOR_INPUT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|input| input.0.get_live());
    let Some(input) = input else {
        LOG_SHOWN.call_once(|| log!("menu opened without the editor's input component"));
        return;
    };
    // SAFETY: as above; the component is alive, the names are the tables' rows'.
    match unsafe { game.show_actions(help, input, &actions) } {
        Ok(()) => LOG_SHOWN.call_once(|| log!("menu opened: {} entries", actions.len())),
        Err(reason) => LOG_SHOWN.call_once(|| log!("menu entries not shown: {reason}")),
    }
}

/// An action widget runs or asks about its action: the menu's are enabled while available, and run
/// their handler.
unsafe extern "C" fn execute_action(
    component: *mut UObjectBase,
    result: *mut u8,
    name: *const FName,
    params: *const u8,
) -> *mut u8 {
    let _span = timings::span("editor_menu::execute_action");
    let Some(original) = ORIGINAL_EXECUTE.get() else {
        return result;
    };
    // SAFETY: forwards the caller's arguments: it writes the result (`State` 2 for ours, which no
    // component binds).
    let returned = unsafe {
        timings::game(input_help::EXECUTE_ACTION.name, || {
            original(component, result, name, params)
        })
    };
    let Some(game) = GAME.get() else {
        return returned;
    };
    // SAFETY: the caller's name and parameters, valid for the call.
    let (entry, run) = unsafe { (entry_of(&*name), game.params(params)) };
    if let Some(entry) = entry.filter(Entry::is_available) {
        // SAFETY: the result the original just made.
        unsafe { game.enable(returned) };
        // A click: not a held widget's run each frame.
        if run.real && !run.secondary && run.axis == 0.0 {
            // Each time: clicks are rare, and a click must run its entry once.
            log!("menu entry clicked");
            (entry.handler)();
        }
    }
    returned
}
