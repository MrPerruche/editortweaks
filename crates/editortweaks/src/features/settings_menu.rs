//! The mod's settings in the game's options, as a sub-menu like the game's own (Gameplay, User
//! interface, ...): an "Editor tweaks" button among the options' buttons opens the context
//! `<root>/Options/EditorTweaks`, whose page lists every setting of [`crate::features::SETTINGS`]
//! by section, saved to `EditorTweaks.ini` as they change (see `framework::settings`). Back returns
//! to the options.
//!
//! The game does the work: its menu opens the context (title, Back button, transition) and shows
//! the page, a settings page that lists the game's settings of its categories. Ours is the gameplay
//! settings page (`WBP_GameplaySettings`, nothing but a settings page for the gameplay category),
//! listing our category instead while in our context, and the game's settings list
//! (`UBrickUserSettings::ReflectBrickProperties`, shared through `features::settings_list`) gets our
//! rows when a page asks for our category. We only add our button and show our page where the
//! menu's Blueprint does for its own contexts: once it made the new context's buttons, before the
//! panel pools the unused ones. See `game::menu` and `game::property_reflection`.

use std::sync::{Mutex, OnceLock};

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;

use super::settings_list;
use crate::features::SETTINGS;
use crate::framework::hook;
use crate::game::kismet;
use crate::game::menu::{
    self, AddPropertiesFn, ClickedFn, Menus, OpenMenuFn, RemoveUnusedButtonsFn,
};
use crate::game::text::Text;
use crate::game::weak_object::WeakObject;
use crate::prelude::*;

pub struct SettingsMenu;

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

static MENUS: OnceLock<Menus> = OnceLock::new();
static OURS: OnceLock<Ours> = OnceLock::new();

static ORIGINAL_OPEN_MENU: OnceLock<OpenMenuFn> = OnceLock::new();
static ORIGINAL_REMOVE_UNUSED_BUTTONS: OnceLock<RemoveUnusedButtonsFn> = OnceLock::new();
static ORIGINAL_CLICKED: OnceLock<ClickedFn> = OnceLock::new();
static ORIGINAL_ADD_PROPERTIES: OnceLock<AddPropertiesFn> = OnceLock::new();

/// Names and texts, made once after engine init.
struct Ours {
    category: FName,
    title: Text,
    icon: FName,
    page: FName,
    context: FName,
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
    /// The menu in our context, until it opens another.
    in_ours: Option<WeakObject>,
}

// SAFETY: the menus `OpenMenu` runs for (alive while it runs) and weak object references, only
// used on the game thread.
unsafe impl Send for State {}

static STATE: Mutex<State> = Mutex::new(State {
    opening: Vec::new(),
    button: None,
    in_ours: None,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
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

/// Makes our names, texts and rows, finds the menus and hooks them.
fn start() -> Result<(), String> {
    let mut sections = Vec::new();
    for section in SETTINGS.iter().copied().flatten() {
        let rows = section
            .settings
            .iter()
            .map(|&setting| settings_list::setting_row(setting))
            .collect::<Result<Vec<_>, _>>()?;
        sections.push((section.title, rows));
    }
    settings_list::add(CATEGORY, sections)?;
    let _ = OURS.set(Ours {
        category: kismet::string_to_name(CATEGORY)?,
        title: kismet::string_to_text(TITLE)?,
        icon: kismet::string_to_name(ICON)?,
        page: kismet::string_to_name(PAGE)?,
        context: kismet::string_to_name(CONTEXT)?,
    });
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
            "UMenuButtonWidget::OnClicked",
            menus.on_clicked,
            clicked as ClickedFn,
            &ORIGINAL_CLICKED,
        )?;
        hook::install_typed(
            "UMenuSettingsPageWidget::AddProperties_Implementation",
            menus.add_properties,
            add_properties as AddPropertiesFn,
            &ORIGINAL_ADD_PROPERTIES,
        )?;
    }
    Ok(())
}

/// A menu opens a context: notes which while it does (see `remove_unused_buttons`).
unsafe extern "C" fn open_menu(menu: *mut UObjectBase, context: FName) {
    let Some(original) = ORIGINAL_OPEN_MENU.get() else {
        return;
    };
    // SAFETY: the name the game passes.
    let leaf = unsafe { menu::context_leaf(&context) };
    state().opening.push((menu, leaf));
    // SAFETY: forwards the caller's arguments.
    unsafe { original(menu, context) };
    state().opening.pop();
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
/// get our button, our context our page.
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
            .in_ours
            .as_ref()
            .is_some_and(|in_ours| in_ours.is(menu))
        {
            state.in_ours = None;
        }
    }
    // SAFETY: per the contract.
    unsafe {
        match leaf {
            OPTIONS => match menus.add_button(panel, &ours.title, ours.icon) {
                Some(button) => {
                    state().button = Some((WeakObject::new(button), WeakObject::new(menu)));
                }
                None => log!("no button added to the options"),
            },
            CONTEXT => {
                state().in_ours = Some(WeakObject::new(menu));
                menus.show_page(menu, ours.page, &ours.title);
            }
            _ => {}
        }
    }
}

/// A menu button was clicked: ours opens our context, as the options' buttons open theirs.
unsafe extern "C" fn clicked(button: *mut UObjectBase) {
    let Some(original) = ORIGINAL_CLICKED.get() else {
        return;
    };
    // SAFETY: on the game thread, before the click changes anything.
    let menu = unsafe { menu_of_ours(button) };
    // SAFETY: forwards the caller's argument; ours has no delegate, so this does nothing for it.
    unsafe { original(button) };
    if let (Some(menu), Some(menus), Some(ours)) = (menu, MENUS.get(), OURS.get()) {
        // SAFETY: a live menu, on the game thread.
        unsafe { menus.open(menu, ours.context) };
    }
}

/// The menu showing `button`, if it's our button.
///
/// # Safety
/// Game thread only.
unsafe fn menu_of_ours(button: *mut UObjectBase) -> Option<*mut UObjectBase> {
    let menus = MENUS.get()?;
    let menu = state()
        .button
        .as_ref()
        .filter(|(ours, _)| ours.is(button))
        .and_then(|(_, shown_in)| shown_in.get())?;
    // SAFETY: a live menu; the button must still be among its panel's (not pooled and reused by
    // another panel since).
    unsafe {
        let panel = menus.button_panel(menu)?;
        menus.shows(panel, button).then_some(menu)
    }
}

/// A settings page lists its settings: the page shown in our context lists our category, for the
/// time of it (pages are pooled and reused, so it keeps its own).
unsafe extern "C" fn add_properties(page: *mut UObjectBase) {
    let Some(original) = ORIGINAL_ADD_PROPERTIES.get() else {
        return;
    };
    let (Some(menus), Some(ours)) = (MENUS.get(), OURS.get()) else {
        // SAFETY: forwards the caller's argument.
        unsafe { original(page) };
        return;
    };
    let in_ours = state().in_ours.as_ref().and_then(WeakObject::get);
    // SAFETY: the live page being constructed and a live menu, on the game thread.
    unsafe {
        if !in_ours.is_some_and(|menu| menus.current_page(menu) == page) {
            original(page);
            return;
        }
        let categories = menus.categories(page);
        let own = categories.to_vec();
        categories.fill(ours.category);
        original(page);
        for (category, own) in menus.categories(page).iter_mut().zip(own) {
            *category = own;
        }
    }
}
