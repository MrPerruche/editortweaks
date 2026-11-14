//! The navigation bar above the Local list: Back, Forward, Up (icons only), then the path ("Home", "Planes",
//! "Jets"), each part opening its level. A vehicle or folder dragged onto a path part (or Up) moves
//! there (`drag`). Made once per paged list (`game::nav_bar`) and remade as the list changes; hidden
//! where folders don't show (Subscribed, the "Off" view, a search).

use std::cell::RefCell;
use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;
use vehicle_folders::{Location, ViewMode, breadcrumbs};

use super::tiles::{self, Refresh};
use super::{BROWSERS, category_of, view_mode, with_state};
use crate::features::menu_buttons;
use crate::game::button_key::ButtonLabels;
use crate::game::kismet;
use crate::game::menu::ButtonPanels;
use crate::game::nav_bar::{Bar, NavBars};
use crate::game::reflection;
use crate::game::text::Text;
use crate::game::ui_style::{self, ColorStyleValues, SetColorStyle};
use crate::game::weak_object::WeakObject;
use crate::prelude::*;

/// What a bar button does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Nav {
    Back,
    Forward,
    Up,
    /// A part of the path: opens that level.
    Part(Location),
}

/// The icons' textures (the game's).
const ICONS: &str = "/Game/BrickRigs/UI/Textures/Icons/";

/// The bar's code and the texts and icons that don't change (after engine init).
struct Kit {
    bars: NavBars,
    panels: ButtonPanels,
    /// Pooled buttons keep their last style: each of ours gets the default (or the highlight while
    /// something is dragged over it).
    set_style: SetColorStyle,
    styles: ColorStyleValues,
    /// Back, Forward and Up show only their icons: no text, and their label collapsed (an empty
    /// one still widens the button on the right). `None` if not found (logged): labels stay.
    no_text: Text,
    labels: Option<ButtonLabels>,
    back: FName,
    forward: FName,
    up: FName,
    /// The path parts' icon: the first part's, then the others'.
    home_icon: FName,
    part_icon: FName,
}

// SAFETY: reflection records, names and texts kept for the whole game; only used on the game
// thread.
unsafe impl Send for Kit {}
unsafe impl Sync for Kit {}

static KIT: OnceLock<Kit> = OnceLock::new();

/// The bar shown, and its buttons.
struct Shown {
    bar: Bar,
    /// What it shows: the buttons' actions in order (remade only when they change).
    navs: Vec<Nav>,
    buttons: Vec<(WeakObject, Nav)>,
    visible: bool,
    /// Whether the bar's height was known when the entries were moved below it.
    measured: bool,
}

thread_local! {
    /// Game thread only.
    static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
}

/// Finds what the bar needs. After engine init.
pub fn start() -> Result<(), String> {
    let button =
        reflection::find_class("MenuButtonWidget").ok_or("class MenuButtonWidget not found")?;
    let icon = |name: &str| kismet::string_to_name(&format!("{ICONS}{name}"));
    let kit = Kit {
        bars: NavBars::find()?,
        panels: ButtonPanels::find()?,
        set_style: SetColorStyle::find(button)?,
        styles: ui_style::color_style_values()?,
        // Icons only: an empty text leaves the button's label blank (`UpdateDisplayText` only sets
        // its text block), as the property rows' small buttons are.
        no_text: kismet::string_to_text("")?,
        labels: ButtonLabels::find()
            .map_err(|reason| {
                log!("vehicle folders: the bar's arrows keep a blank label ({reason})")
            })
            .ok(),
        // The arrow keys' icons ("Left", "Right", "Up" in the input settings): bolder than
        // `T_Arrow_*`.
        back: icon("T_LeftKey.T_LeftKey")?,
        forward: icon("T_RightKey.T_RightKey")?,
        up: icon("T_UpKey.T_UpKey")?,
        home_icon: kismet::string_to_name(&format!("{ICONS}T_Garage.T_Garage"))?,
        part_icon: kismet::string_to_name(&format!("{ICONS}T_Folder.T_Folder"))?,
    };
    let _ = KIT.set(kit);
    // The arrows' collapsed labels go back before their buttons are pooled (`ButtonLabels`).
    menu_buttons::before_pooling(labels_before_pooling);
    menu_buttons::after_pooling(labels_after_pooling);
    menu_buttons::listen(clicked)
}

/// Any button panel is about to pool its unused buttons (see `ButtonLabels`).
fn labels_before_pooling(panel: *mut UObjectBase) {
    if let Some(kit) = KIT.get()
        && let Some(labels) = &kit.labels
    {
        // SAFETY: the game's live panel, on the game thread; its buttons read before it changes
        // them.
        labels.before_pooling(unsafe { kit.panels.buttons(panel) });
    }
}

/// The panel pooled them: the arrows' buttons among them show their labels again.
fn labels_after_pooling(panel: *mut UObjectBase) {
    if let Some(kit) = KIT.get()
        && let Some(labels) = &kit.labels
    {
        // SAFETY: as above, right after `labels_before_pooling` for the same panel (the hook runs
        // both around the game's pooling); the buttons it pooled are still live.
        unsafe { labels.after_pooling(kit.panels.buttons(panel)) };
    }
}

/// Shows the bar for the browser's page just loaded, or hides it where folders don't show.
///
/// # Safety
/// `browser` must be the live browser. Game thread only.
pub unsafe fn update(browser: *mut UObjectBase) {
    let (Some(kit), Some(browsers)) = (KIT.get(), BROWSERS.get()) else {
        return;
    };
    // SAFETY: per the contract.
    let (list, wanted) = unsafe {
        let wanted = category_of(browsers.query_type(browser)).is_some()
            && view_mode() != ViewMode::Off
            && !browsers.is_filtering(browser);
        (browsers.paged_list(browser), wanted)
    };
    let Some(list) = list else {
        return;
    };
    let known = SHOWN.with_borrow(|shown| {
        shown
            .as_ref()
            .filter(|shown| shown.bar.is_on(list))
            .map(|shown| {
                (
                    shown.bar,
                    shown.navs.clone(),
                    shown.visible && shown.measured,
                    shown.buttons.clone(),
                )
            })
    });
    if !wanted {
        if let Some((bar, _, true, _)) = known {
            // SAFETY: on the game thread.
            unsafe { kit.bars.hide(&bar) };
            SHOWN.with_borrow_mut(|shown| {
                if let Some(shown) = shown {
                    shown.visible = false;
                }
            });
        }
        return;
    }
    let navs = with_state(|state| {
        let mut navs = vec![Nav::Back, Nav::Forward, Nav::Up];
        navs.extend(
            breadcrumbs(state.here())
                .into_iter()
                .map(|(_, location)| Nav::Part(location)),
        );
        navs
    });
    let (bar, unchanged) = match known {
        Some((bar, shown_navs, visible, buttons)) => {
            // SAFETY: on the game thread.
            let intact = unsafe { still_shows(kit, &bar, &buttons) };
            if !intact {
                static TOLD: std::sync::Once = std::sync::Once::new();
                TOLD.call_once(|| {
                    log!("vehicle folders: the navigation bar was emptied by the game, remade")
                });
            }
            (bar, visible && intact && shown_navs == navs)
        }
        None => {
            // SAFETY: per the contract; the browser's own left panel is a button panel.
            let made = unsafe {
                browsers.button_panel(browser).and_then(|panel| {
                    let class = reflection::class_of(panel.cast()).cast();
                    kit.bars.make(browser, list, class)
                })
            };
            match made {
                Some(bar) => (bar, false),
                None => {
                    static TOLD: std::sync::Once = std::sync::Once::new();
                    TOLD.call_once(|| {
                        log!("vehicle folders: no navigation bar (the list isn't laid out as expected)")
                    });
                    return;
                }
            }
        }
    };
    if unchanged {
        return;
    }
    // SAFETY: on the game thread; the panel is our live bar, and each button it makes is live.
    let (buttons, measured) = unsafe { make_buttons(kit, &bar, &navs) };
    SHOWN.with_borrow_mut(|shown| {
        *shown = Some(Shown {
            bar,
            navs,
            buttons,
            visible: true,
            measured,
        })
    });
}

/// Whether `bar`'s panel still shows `buttons`, as they were made. The game empties the panel when
/// its Slate widget goes, e.g. as the browser closes (`UMenuButtonPanelWidget::NativeDestruct`,
/// BRMK 0x3D6160, zeroes the used count and pools every button), while the panel itself stays in
/// the list for the next opening: then the entries are still moved below a bar showing nothing.
///
/// # Safety
/// Game thread only.
unsafe fn still_shows(kit: &Kit, bar: &Bar, buttons: &[(WeakObject, Nav)]) -> bool {
    let Some(panel) = bar.panel.get_live() else {
        return false;
    };
    // SAFETY: a live button panel, not making buttons now; the slice is read before anything
    // changes them.
    let shown = unsafe { kit.panels.buttons(panel) };
    shown.len() == buttons.len()
        && shown
            .iter()
            .zip(buttons)
            .all(|(&shown, (made, _))| made.is(shown))
}

/// Remakes the bar's buttons for `navs`; and whether the bar's height was known (see
/// `NavBars::finish`).
///
/// # Safety
/// Game thread only.
unsafe fn make_buttons(kit: &Kit, bar: &Bar, navs: &[Nav]) -> (Vec<(WeakObject, Nav)>, bool) {
    // SAFETY: per the contract.
    let Some(panel) = (unsafe { kit.bars.begin(bar) }) else {
        return (Vec::new(), false);
    };
    let labels = with_state(|state| {
        breadcrumbs(state.here())
            .into_iter()
            .map(|(label, _)| label)
            .collect::<Vec<_>>()
    });
    let mut buttons = Vec::with_capacity(navs.len());
    let mut part = 0;
    for nav in navs {
        let made = match nav {
            Nav::Back | Nav::Forward | Nav::Up => {
                let icon = match nav {
                    Nav::Back => kit.back,
                    Nav::Forward => kit.forward,
                    _ => kit.up,
                };
                // SAFETY: our live bar, making its buttons.
                unsafe { kit.panels.add_button(panel, &kit.no_text, icon) }
            }
            Nav::Part(_) => {
                let icon = if part == 0 {
                    kit.home_icon
                } else {
                    kit.part_icon
                };
                let label = labels.get(part).map_or("", String::as_str);
                part += 1;
                let Ok(text) = kismet::string_to_text(label) else {
                    continue;
                };
                // SAFETY: as above; the button takes a reference of its own to the text, ours is
                // given back.
                unsafe {
                    let made = kit.panels.add_button(panel, &text, icon);
                    text.release();
                    made
                }
            }
        };
        if let Some(button) = made {
            // SAFETY: the button just made, live.
            unsafe {
                kit.set_style.call(button.cast(), kit.styles.default);
                if let Some(labels) = &kit.labels {
                    labels.show(button, matches!(nav, Nav::Part(_)));
                }
                buttons.push((WeakObject::new(button), nav.clone()));
            }
        }
    }
    // SAFETY: per the contract.
    let measured = unsafe { kit.bars.finish(bar) };
    (buttons, measured)
}

/// A menu button was clicked (`menu_buttons`): one of the bar's goes where it says.
fn clicked(button: *mut UObjectBase) {
    let Some(nav) = nav_of(button) else {
        return;
    };
    let moved = with_state(|state| {
        let history = state.history_mut();
        match nav {
            Nav::Back => history.back(),
            Nav::Forward => history.forward(),
            Nav::Up => history.up(),
            Nav::Part(location) => {
                let away = *history.current() != location;
                if away {
                    history.go(location);
                }
                away
            }
        }
    });
    if moved {
        tiles::unselect();
        tiles::show_next_frame(Refresh::FirstPage);
    }
}

/// What `button` does, if it's one of the bar's shown now.
fn nav_of(button: *mut UObjectBase) -> Option<Nav> {
    let kit = KIT.get()?;
    let (panel, nav) = SHOWN.with_borrow(|shown| {
        let shown = shown.as_ref().filter(|shown| shown.visible)?;
        let nav = shown
            .buttons
            .iter()
            .find(|(known, _)| known.is(button))
            .map(|(_, nav)| nav.clone())?;
        Some((shown.bar.panel, nav))
    })?;
    // SAFETY: on the game thread (clicks and timers run there); a live panel. The widget may have
    // been pooled and reused by another panel since it was ours.
    let still_ours = panel
        .get_live()
        .is_some_and(|panel| unsafe { kit.panels.shows(panel, button) });
    still_ours.then_some(nav)
}

/// The bar's buttons a drop can land on, and where each files things: Up and the path parts.
pub fn drop_targets() -> Vec<(*mut UObjectBase, Location)> {
    let here = with_state(|state| state.here().clone());
    let buttons = SHOWN.with_borrow(|shown| {
        shown
            .as_ref()
            .filter(|shown| shown.visible)
            .map(|shown| shown.buttons.clone())
            .unwrap_or_default()
    });
    buttons
        .into_iter()
        .filter_map(|(button, nav)| {
            let button = button.get_live()?;
            let target = match nav {
                Nav::Up => here.parent()?,
                Nav::Part(location) => location,
                Nav::Back | Nav::Forward => return None,
            };
            nav_of(button).is_some().then_some((button, target))
        })
        .collect()
}

/// Shows `button` (one of the bar's) as a drop target, or as usual again.
///
/// # Safety
/// `button` must be a live menu button. Game thread only.
pub unsafe fn show_target(button: *mut UObjectBase, target: bool) {
    let Some(kit) = KIT.get() else {
        return;
    };
    let style = if target {
        kit.styles.highlight
    } else {
        kit.styles.default
    };
    // SAFETY: per the contract.
    unsafe { kit.set_style.call(button.cast(), style) };
}

/// The area the entries show in, for hit tests: the list's scroll box.
///
/// # Safety
/// `list` must be a live paged list. Game thread only.
pub unsafe fn entries_area(list: *mut UObjectBase) -> Option<*mut UObjectBase> {
    // SAFETY: per the contract.
    unsafe { KIT.get()?.bars.scroll_box(list) }
}
