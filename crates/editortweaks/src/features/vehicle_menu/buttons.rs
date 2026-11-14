//! Our buttons in the vehicle browser's left panel, after the game's (Local and Subscribed lists): New folder
//! while nothing is selected (the folder made is selected, to name it in the side panel:
//! `details`); for the selected vehicle or folder tile Pin / Unpin, Delete folder for a folder.
//! Moving is dragging (`drag`). The panel makes its buttons in `UpdateButtons`, then pools the
//! rest: ours go in just before (`menu_buttons::before_pooling`), at the end; once pooled, Pin /
//! Unpin moves right before the game's Duplicate (after Spawn and its spawn mode), found by its
//! text's string table key (`Action_Duplicate`, `game::button_key`) in any language. A click
//! changes the folders file and shows the list again on the next frame.

use std::cell::{Cell, RefCell};
use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;
use vehicle_folders::{FolderError, FolderPath, Location, NEW_FOLDER_NAME, Tree, ViewMode};

use super::tiles::{self, Refresh, Tile};
use super::{BROWSERS, ITEMS, category_of, save, view_mode, with_state};
use crate::features::menu_buttons;
use crate::framework::clock;
use crate::game::button_key::ButtonKeys;
use crate::game::kismet;
use crate::game::menu::{ButtonGrid, ButtonPanels};
use crate::game::reflection;
use crate::game::text::Text;
use crate::game::ui_style::{self, ColorStyleValues, SetColorStyle};
use crate::game::weak_object::WeakObject;
use crate::prelude::*;

/// What a button of ours does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    NewFolder,
    Pin,
    Unpin,
    DeleteFolder,
}

impl Action {
    const ALL: [Action; 4] = [
        Action::NewFolder,
        Action::Pin,
        Action::Unpin,
        Action::DeleteFolder,
    ];

    /// The button's text and icon (a texture of the game's).
    fn look(self) -> (&'static str, &'static str) {
        const ICONS: &str = "/Game/BrickRigs/UI/Textures/Icons/";
        let (text, icon) = match self {
            Action::NewFolder => ("New folder", "T_Add.T_Add"),
            Action::Pin => ("Pin", "T_PinRoot.T_PinRoot"),
            Action::Unpin => ("Unpin", "T_PinRoot.T_PinRoot"),
            Action::DeleteFolder => ("Delete folder", "T_DeleteFile.T_DeleteFile"),
        };
        // Leaked once per action at start: the icon paths live as long as the game.
        (text, Box::leak(format!("{ICONS}{icon}").into_boxed_str()))
    }

    /// The button's color style, like the game's (its Delete is red): New folder green, Delete
    /// folder red, the rest the default.
    fn style(self, styles: &ColorStyleValues) -> u8 {
        match self {
            Action::NewFolder => styles.positive,
            Action::DeleteFolder => styles.negative,
            _ => styles.default,
        }
    }
}

/// What a button acts on: the selected vehicle (the game's selection) or folder tile (ours).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Vehicle(String),
    Folder(FolderPath),
}

/// The buttons' texts and icons, and the panels' code (after engine init).
struct Kit {
    panels: ButtonPanels,
    /// A pooled button may keep another's style (Delete's red): each of ours gets its own.
    set_style: SetColorStyle,
    styles: ColorStyleValues,
    looks: Vec<(Action, Text, FName)>,
    /// To put Pin / Unpin among the game's buttons; `None` if not found (they stay last, logged).
    order: Option<(ButtonGrid, ButtonKeys)>,
}

// SAFETY: reflection records, names and texts kept for the whole game; only used on the game
// thread.
unsafe impl Send for Kit {}
unsafe impl Sync for Kit {}

static KIT: OnceLock<Kit> = OnceLock::new();

thread_local! {
    /// Our buttons in the browser's panel now, and what each does. Game thread only.
    static SHOWN: RefCell<Vec<(WeakObject, Action)>> = const { RefCell::new(Vec::new()) };
    /// The category the browser showed when its buttons were last made.
    static MADE_FOR: Cell<Option<u8>> = const { Cell::new(None) };
}

/// Finds what the buttons need and starts making them. After engine init.
pub fn start() -> Result<(), String> {
    let button =
        reflection::find_class("MenuButtonWidget").ok_or("class MenuButtonWidget not found")?;
    let looks = Action::ALL
        .iter()
        .map(|&action| {
            let (text, icon) = action.look();
            Ok((
                action,
                kismet::string_to_text(text)?,
                kismet::string_to_name(icon)?,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let kit = Kit {
        panels: ButtonPanels::find()?,
        set_style: SetColorStyle::find(button)?,
        styles: ui_style::color_style_values()?,
        looks,
        order: ButtonGrid::find()
            .and_then(|grid| Ok((grid, ButtonKeys::find()?)))
            .map_err(|reason| {
                log!("vehicle folders: Pin stays after the game's buttons ({reason})")
            })
            .ok(),
    };
    let _ = KIT.set(kit);
    menu_buttons::listen(clicked)?;
    menu_buttons::before_pooling(make);
    menu_buttons::after_pooling(arrange);
    Ok(())
}

/// The live browser, once the folders file is read.
fn browser() -> Option<*mut UObjectBase> {
    with_state(|state| {
        state
            .ready
            .then_some(state.browser)
            .flatten()
            .and_then(|browser| browser.get_live())
    })
}

/// What's selected in `browser`: our folder tile, else the game's vehicle.
///
/// # Safety
/// `browser` must be the live browser. Game thread only.
unsafe fn target(browser: *mut UObjectBase) -> Option<Target> {
    let tile = with_state(|state| {
        state
            .selected
            .as_ref()
            .map(|selected| selected.tile.clone())
    });
    if let Some(Tile::Folder(path)) = tile {
        return Some(Target::Folder(path));
    }
    let (browsers, items) = (BROWSERS.get()?, ITEMS.get()?);
    // SAFETY: per the contract; the entry is the game's live row widget, its row its own.
    unsafe {
        let entry = browsers.selected_entry(browser)?;
        let row = items.row_of(entry)?;
        let id = super::row_key(row);
        (Tile::of_id(&id).is_none() && !id.is_empty()).then_some(Target::Vehicle(id))
    }
}

/// `panel` made its buttons: if it's the browser's showing a list with folders, ours go after
/// them.
fn make(panel: *mut UObjectBase) {
    let (Some(kit), Some(browsers), Some(browser)) = (KIT.get(), BROWSERS.get(), browser()) else {
        return;
    };
    // SAFETY: the live browser, on the game thread (panels make their buttons there).
    let (browser_panel, local) = unsafe {
        (
            browsers.button_panel(browser) == Some(panel),
            category_of(browsers.query_type(browser)).is_some(),
        )
    };
    if !browser_panel {
        return;
    }
    // SAFETY: as above.
    MADE_FOR.set(Some(unsafe { browsers.query_type(browser) }));
    // Its buttons are made anew: ours from before may be pooled, or reused as the game's.
    SHOWN.with_borrow_mut(Vec::clear);
    if !local {
        return;
    }
    // SAFETY: as above.
    let target = unsafe { target(browser) };
    let folders = view_mode() != ViewMode::Off;
    let actions = with_state(|state| {
        let tree = state.tree();
        let mut actions = Vec::new();
        if folders && target.is_none() {
            actions.push(Action::NewFolder);
        }
        match &target {
            Some(Target::Vehicle(id)) => {
                actions.push(if tree.is_pinned(id) {
                    Action::Unpin
                } else {
                    Action::Pin
                });
            }
            Some(Target::Folder(path)) => {
                actions.push(if tree.is_folder_pinned(path) {
                    Action::Unpin
                } else {
                    Action::Pin
                });
                actions.push(Action::DeleteFolder);
            }
            None => {}
        }
        actions
    });
    let mut shown = Vec::with_capacity(actions.len());
    for action in actions {
        let Some((_, text, icon)) = kit.looks.iter().find(|(known, _, _)| *known == action) else {
            continue;
        };
        // SAFETY: the browser's live panel, making its buttons; the button it makes is live.
        unsafe {
            if let Some(button) = kit.panels.add_button(panel, text, *icon) {
                kit.set_style.call(button.cast(), action.style(&kit.styles));
                shown.push((WeakObject::new(button), action));
            }
        }
    }
    SHOWN.with_borrow_mut(|known| *known = shown);
}

/// `browser` loaded a page: if it shows another category than its buttons were made for, they're
/// made again. The game doesn't: switching category only loads the first page
/// (`UUGCBrowserWidget::PostModifyBrickProperty`, BRMK 0x3E1F80, skips its `UpdateButtons` call for
/// `UGCQueryType`), so New folder stayed hidden in Local (or shown in Online) until the
/// selection changed.
///
/// # Safety
/// `browser` must be the live browser. Game thread only.
pub unsafe fn follow_category(browser: *mut UObjectBase) {
    let Some(browsers) = BROWSERS.get() else {
        return;
    };
    // SAFETY: per the contract.
    let now = unsafe { browsers.query_type(browser) };
    if MADE_FOR.get().is_some_and(|made_for| made_for != now) {
        tiles::show_next_frame(Refresh::ButtonsOnly);
    }
}

/// The string table key of the game's Duplicate button (`WBP_UGCBrowser`'s `CreateDuplicateButton`).
const DUPLICATE_KEY: &str = "Action_Duplicate";

/// `panel` pooled its unused buttons: if it shows our Pin or Unpin, that goes right before the
/// game's Duplicate (where there is one: a vehicle selected).
fn arrange(panel: *mut UObjectBase) {
    let Some(Some((grid, keys))) = KIT.get().map(|kit| kit.order.as_ref()) else {
        return;
    };
    let pin = SHOWN.with_borrow(|shown| {
        shown
            .iter()
            .find(|(_, action)| matches!(action, Action::Pin | Action::Unpin))
            .and_then(|(button, _)| button.get_live())
    });
    let Some(pin) = pin else {
        return;
    };
    let kit = KIT.get().expect("checked above");
    // SAFETY: the panel just pooled its buttons, on the game thread; the ones it shows are live
    // menu buttons.
    unsafe {
        let buttons = kit.panels.buttons(panel);
        if !buttons.contains(&pin) {
            return;
        }
        let Some(duplicate) = buttons
            .iter()
            .copied()
            .find(|&button| keys.key(button).as_deref() == Some(DUPLICATE_KEY))
        else {
            return;
        };
        let wrong = grid.put_before(&kit.panels, panel, &[pin], duplicate);
        if !wrong.is_empty() {
            log!("vehicle folders: Pin not moved: {}", wrong.join("; "));
        }
    }
}

/// A menu button was clicked (`menu_buttons`): ours acts.
fn clicked(button: *mut UObjectBase) {
    let action = SHOWN.with_borrow(|shown| {
        shown
            .iter()
            .find(|(known, _)| known.is(button))
            .map(|&(_, action)| action)
    });
    let (Some(action), Some(kit), Some(browsers), Some(browser)) =
        (action, KIT.get(), BROWSERS.get(), browser())
    else {
        return;
    };
    // SAFETY: the live browser, on the game thread (clicks are UI code). The widget may have been
    // reused by another panel since it was ours.
    let target = unsafe {
        let still_ours = browsers
            .button_panel(browser)
            .is_some_and(|panel| kit.panels.shows(panel, button));
        if !still_ours {
            return;
        }
        target(browser)
    };
    act(action, target);
}

/// Does `action` on `target`, saves, and shows the list again.
fn act(action: Action, target: Option<Target>) {
    let outcome = with_state(|state| {
        let here = state.here().clone();
        let tree = state.tree_mut();
        match (action, target) {
            (Action::NewFolder, _) => {
                let parent = folder_of(&here);
                tree.free_folder_name(parent.as_ref(), NEW_FOLDER_NAME)
                    .map_err(FolderError::from)
                    .and_then(|name| tree.create_folder(parent.as_ref(), &name))
                    .inspect(|folder| stamp_created(tree, folder))
                    .map(Outcome::Made)
                    .map_err(|error| error.to_string())
            }
            (Action::Pin | Action::Unpin, Some(target)) => {
                let pinned = action == Action::Pin;
                match target {
                    Target::Vehicle(id) => {
                        tree.set_vehicle_pinned(&id, pinned);
                        Ok(Outcome::Changed(Refresh::SamePage))
                    }
                    Target::Folder(path) => tree
                        .set_folder_pinned(&path, pinned)
                        .map(|()| Outcome::Changed(Refresh::SamePage))
                        .map_err(|error| error.to_string()),
                }
            }
            (Action::DeleteFolder, Some(Target::Folder(path))) => tree
                .delete_folder(&path)
                .map(|()| Outcome::Changed(Refresh::SamePage))
                .map_err(|error| error.to_string()),
            _ => Ok(Outcome::ButtonsOnly),
        }
    });
    match outcome {
        Ok(Outcome::Changed(refresh)) => {
            tiles::unselect();
            save();
            tiles::show_next_frame(refresh);
        }
        Ok(Outcome::Made(folder)) => {
            tiles::select_later(folder);
            save();
            tiles::show_next_frame(Refresh::SamePage);
        }
        Ok(Outcome::ButtonsOnly) => tiles::show_next_frame(Refresh::ButtonsOnly),
        Err(reason) => log!("vehicle folders: {action:?} not done: {reason}"),
    }
}

enum Outcome {
    /// The folders changed: saved, and the list shown again.
    Changed(Refresh),
    /// A folder was made: saved, selected, and the list shown again.
    Made(FolderPath),
    /// Nothing changes but the buttons.
    ButtonsOnly,
}

/// Records that `folder` was made now (shown in its details).
pub fn stamp_created(tree: &mut Tree, folder: &FolderPath) {
    let now = i64::try_from(clock::now()).unwrap_or(i64::MAX);
    // Can't fail: the folder was just made.
    let _ = tree.set_folder_created(folder, now);
}

/// The folder new things go into at `here` (`None`: the top level).
fn folder_of(here: &Location) -> Option<FolderPath> {
    match here {
        Location::Folder(path) => Some(path.clone()),
        Location::Root | Location::Uncategorized => None,
    }
}
