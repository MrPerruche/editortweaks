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
//! The game does the work: its menu opens the context (title, Back button, transition) and shows
//! the page, a settings page that lists the game's settings of its categories. Ours is the gameplay
//! settings page (`WBP_GameplaySettings`, nothing but a settings page for the gameplay category),
//! listing our category instead while in our context, and the game's settings list
//! (`UBrickUserSettings::ReflectBrickProperties`, shared through `features::settings_list`) gets our
//! rows when a page asks for our category. We only add our buttons and show our page where the
//! menu's Blueprint does for its own contexts: once it made the new context's buttons, before the
//! panel pools the unused ones. See `game::menu` and `game::property_reflection`.

use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;

use super::{menu_buttons, settings_list};
use crate::features::SETTINGS;
use crate::framework::hook;
use crate::framework::settings::Section;
use crate::game::kismet;
use crate::game::menu::{
    self, AddPropertiesFn, Menus, OpenMenuFn, RemoveUnusedButtonsFn, WindowManager,
};
use crate::game::text::Text;
use crate::game::weak_object::WeakObject;
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
    state().pending = Some((dialog, Instant::now()));
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
    /// The dialog [`open`] asked for, until the menu shows its root.
    pending: Option<(&'static Dialog, Instant)>,
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
    let _ = OURS.set(Ours {
        category: kismet::string_to_name(CATEGORY)?,
        title: kismet::string_to_text(TITLE)?,
        icon: kismet::string_to_name(ICON)?,
        page: kismet::string_to_name(PAGE)?,
        context: kismet::string_to_name(CONTEXT)?,
        dialogs: dialog_names,
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
    menu_buttons::listen(clicked)
}

/// A menu opens a context: notes which while it does (see `remove_unused_buttons`). A menu showing
/// its root while a dialog is being opened goes on to the dialog.
unsafe extern "C" fn open_menu(menu: *mut UObjectBase, context: FName) {
    let Some(original) = ORIGINAL_OPEN_MENU.get() else {
        return;
    };
    // SAFETY: the name the game passes.
    let leaf = unsafe { menu::context_leaf(&context) };
    state().opening.push((menu, leaf.clone()));
    // SAFETY: forwards the caller's arguments.
    unsafe { original(menu, context) };
    state().opening.pop();
    if !ROOTS.contains(&leaf.as_str()) {
        return;
    }
    let pending = state().pending.take();
    let Some((dialog, asked)) = pending.filter(|(_, asked)| asked.elapsed() < OPENING) else {
        return;
    };
    let (Some(menus), Some(ours)) = (MENUS.get(), OURS.get()) else {
        return;
    };
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

/// A button panel pools the buttons it doesn't use any more. A menu's panel does so once the menu
/// made its new context's buttons: ours go in first.
unsafe extern "C" fn remove_unused_buttons(panel: *mut UObjectBase) {
    let Some(original) = ORIGINAL_REMOVE_UNUSED_BUTTONS.get() else {
        return;
    };
    let opening = state().opening.last().cloned();
    if let (Some((menu, leaf)), Some(menus), Some(ours)) = (opening, MENUS.get(), OURS.get()) {
        // SAFETY: the menu `OpenMenu` runs for, on the game thread.
        unsafe {
            if menus.button_panel(menu) == Some(panel) {
                context_made(menus, ours, menu, panel, &leaf);
            }
        }
    }
    // SAFETY: forwards the caller's argument.
    unsafe { original(panel) };
}

/// `menu` made the buttons of its new context, whose last part is `leaf`, on `panel`: the options
/// get our button, our context our page, a dialog's context its page and button.
///
/// # Safety
/// `menu` must be a live menu and `panel` its button panel. Game thread only.
unsafe fn context_made(
    menus: &Menus,
    ours: &Ours,
    menu: *mut UObjectBase,
    panel: *mut UObjectBase,
    leaf: &str,
) {
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
                let Some(names) = ours
                    .dialogs
                    .iter()
                    .find(|names| names.dialog.context == leaf)
                else {
                    return;
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
}

/// A menu button was clicked (`menu_buttons`): ours opens our context, as the options' buttons open
/// theirs; a dialog's closes the menu and runs the dialog's action.
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
        }
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
    let Some(original) = ORIGINAL_ADD_PROPERTIES.get() else {
        return;
    };
    let Some(menus) = MENUS.get() else {
        // SAFETY: forwards the caller's argument.
        unsafe { original(page) };
        return;
    };
    let in_ours = state()
        .in_ours
        .as_ref()
        .and_then(|(menu, category)| Some((menu.get()?, *category)));
    // SAFETY: the live page being constructed and a live menu, on the game thread.
    unsafe {
        let Some((_, category)) = in_ours.filter(|&(menu, _)| menus.current_page(menu) == page)
        else {
            original(page);
            return;
        };
        let categories = menus.categories(page);
        let own = categories.to_vec();
        categories.fill(category);
        original(page);
        for (category, own) in menus.categories(page).iter_mut().zip(own) {
            *category = own;
        }
    }
}
