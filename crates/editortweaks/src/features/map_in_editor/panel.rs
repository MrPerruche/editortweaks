//! The map's controls in the editor's side panel. With nothing selected, the panel lists the
//! editor's settings (it asks the settings list for `Properties.Editor`, see
//! `docs/re/map-in-editor.md`), then a "Map" section of ours:
//!
//! - **Show map:** whether the map shows around the vehicle, like the map key. With no garage
//!   selected, showing it selects the one the editor was entered from, else the first. Listed only
//!   if the map can be shown at all.
//! - **Garage:** a dropdown of none, then the map's garages ("Garage 3 of 12"), the one the editor
//!   was entered from and those with a pinned spawn point of their own marked. The selected
//!   garage's spawn point is marked in the editor. Choosing none also hides the map.
//! - **Pinned spawn point:** the setting [`PINNED`], also in the options.
//!
//! What the rows show and change is [`Controls`], kept apart from the feature's state: a tick holds
//! that while it calls the game, and the panel reads the rows whenever the game lists them. The rows
//! only note what was asked. The next tick acts on it as on the map key, and has the panel list the
//! rows again ([`refresh`]) when it changed what they show: a garage selected for the map, or a map
//! that can't be shown.

use std::sync::{Mutex, MutexGuard, Once, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;

use super::placement;
use super::{GAME, Game, PINNED};
use crate::features::settings_list;
use crate::framework::settings::Setting;
use crate::game::garage::Garage;
use crate::game::kismet;
use crate::game::property_reflection::{Check, List, MAX_ITEMS, Value};
use crate::game::weak_object::WeakObject;
use crate::prelude::*;

/// What the editor's side panel asks the settings list for, with nothing selected.
const EDITOR_SETTINGS: &str = "Properties.Editor";

/// How many garages the list can hold: after "None", what's left of a list's items.
const MAX_GARAGES: usize = MAX_ITEMS - 1;

const NONE: &str = "None";

const GARAGE_DESCRIPTION: &str = "The garage whose spawn point is marked in the editor: a cross where the vehicle's box is centred once spawned there, the garage's spawn box and the vehicle's box. The map shows as at this garage. Choose None to hide them and the map.";

const SHOW_MAP_DESCRIPTION: &str = "Show the map around the vehicle, placed as it will be once spawned at the selected garage (if none is selected, the garage the editor was opened from, else the first). Also with the Show map key (Editor tweaks key bindings, numpad 0 by default).";

/// What the rows and the map key ask for, and what the rows show.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct Controls {
    /// The garage whose spawn point is marked.
    pub garage: Option<WeakObject>,
    /// Whether the map is asked for.
    pub show_map: bool,
    /// The editor they're for: the garage it was entered from is marked in the list.
    pub editor: Option<WeakObject>,
    /// Whether a row changed `garage` or `show_map` since the last tick.
    pub asked: bool,
}

impl Controls {
    const fn new() -> Self {
        Self {
            garage: None,
            show_map: false,
            editor: None,
            asked: false,
        }
    }
}

/// The editor's side panel, found for an editor.
#[derive(Clone, Copy)]
struct Found {
    /// The editor's address.
    editor: usize,
    panel: WeakObject,
}

static CONTROLS: Mutex<Controls> = Mutex::new(Controls::new());
static PANEL: Mutex<Option<Found>> = Mutex::new(None);

// SAFETY: weak object references, only used on the game thread (the mutexes just satisfy statics).
unsafe impl Send for Controls {}
unsafe impl Send for Found {}

/// The controls. Never hold them while calling the game: copy them out.
pub(super) fn controls() -> MutexGuard<'static, Controls> {
    CONTROLS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A new editor (or none): no garage selected, no map.
pub(super) fn reset(editor: Option<WeakObject>) {
    *controls() = Controls {
        editor,
        ..Controls::new()
    };
    *PANEL.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

/// Adds the Map section to the editor's settings. After engine init.
pub(super) fn add(game: &Game) -> Result<(), String> {
    let mut rows = Vec::new();
    if game.can_show && game.camera_agrees && game.isolation.is_some() {
        rows.push(settings_list::row(
            "MapShow",
            "Show map",
            SHOW_MAP_DESCRIPTION,
            Value::Check(&SHOW_MAP),
        )?);
    }
    rows.push(settings_list::row(
        "MapGarage",
        "Garage",
        GARAGE_DESCRIPTION,
        Value::List(&GARAGE_LIST),
    )?);
    rows.push(settings_list::setting_row(Setting::Toggle(&PINNED))?);
    settings_list::add(
        EDITOR_SETTINGS,
        vec![(kismet::string_to_text("Map")?, rows)],
    )
}

/// Has the editor's side panel list its rows again, ours with what they show now. Whether it was
/// found (and the game's code for it).
///
/// # Safety
/// `editor` must be the live editor. Game thread only, holding none of the feature's locks (the
/// rows read the controls while the panel lists them).
pub(super) unsafe fn refresh(game: &Game, editor: *mut UObjectBase) -> bool {
    let Some(panels) = &game.panel else {
        return false;
    };
    let known = *PANEL.lock().unwrap_or_else(PoisonError::into_inner);
    let mut panel = known
        .filter(|found| found.editor == editor as usize)
        .and_then(|found| found.panel.get());
    // SAFETY: per the contract; the panel found is live.
    unsafe {
        if panel.is_none() {
            panel = panels.of(editor);
            *PANEL.lock().unwrap_or_else(PoisonError::into_inner) = panel.map(|found| Found {
                editor: editor as usize,
                panel: WeakObject::new(found),
            });
        }
        if let Some(panel) = panel {
            panels.refresh(panel);
        }
    }
    panel.is_some()
}

/// The map's garages.
///
/// # Safety
/// Game thread only.
unsafe fn garages() -> Vec<Garage> {
    // SAFETY: per the contract.
    GAME.get()
        .map(|game| unsafe { game.garages.all() })
        .unwrap_or_default()
}

/// The garage list.
struct GarageList;

static GARAGE_LIST: GarageList = GarageList;

impl List for GarageList {
    fn items(&self) -> Vec<String> {
        static TOO_MANY: Once = Once::new();
        let editor = controls().editor.and_then(|editor| editor.get());
        // SAFETY: the panel lists rows on the game thread; the editor is live (just checked).
        let (garages, entry) = unsafe {
            (
                garages(),
                GAME.get()
                    .zip(editor)
                    .and_then(|(game, editor)| super::entry_garage(game, editor)),
            )
        };
        if garages.len() > MAX_GARAGES {
            TOO_MANY.call_once(|| {
                log!(
                    "the map has {} garages: the panel lists the first {MAX_GARAGES}",
                    garages.len()
                )
            });
        }
        let marks: Vec<Mark> = garages
            .iter()
            .map(|garage| Mark {
                entered: entry == Some(garage.actor),
                pinned: placement::has_pinned_point(garage),
            })
            .collect();
        labels(&marks)
    }

    fn chosen(&self) -> usize {
        let Some(garage) = controls().garage else {
            return 0;
        };
        // SAFETY: the panel lists rows on the game thread.
        let garages = unsafe { garages() };
        item_of(garages.iter().position(|known| garage.is(known.actor)))
    }

    fn choose(&self, index: usize) -> bool {
        let garage = match garage_of(index) {
            None => None,
            Some(position) => {
                // SAFETY: the panel changes rows on the game thread.
                let garages = unsafe { garages() };
                let Some(chosen) = garages.get(position) else {
                    return false;
                };
                // SAFETY: the map's garages are live objects.
                Some(unsafe { WeakObject::new(chosen.actor) })
            }
        };
        let mut controls = controls();
        controls.garage = garage;
        if garage.is_none() {
            controls.show_map = false;
        }
        controls.asked = true;
        true
    }

    /// A map has up to dozens of garages: side by side, they'd be slivers.
    fn dropdown(&self) -> bool {
        true
    }
}

/// The "Show map" check box.
struct ShowMap;

static SHOW_MAP: ShowMap = ShowMap;

impl Check for ShowMap {
    fn checked(&self) -> bool {
        controls().show_map
    }

    fn set_checked(&self, checked: bool) {
        let mut controls = controls();
        controls.show_map = checked;
        controls.asked = true;
    }
}

/// What the list notes about a garage.
#[derive(Clone, Copy, Default)]
struct Mark {
    /// The editor was entered from it.
    entered: bool,
    /// It has a pinned spawn point of its own.
    pinned: bool,
}

/// The garage list's items: none, then the garages (at most [`MAX_GARAGES`]), marked.
fn labels(marks: &[Mark]) -> Vec<String> {
    let count = marks.len();
    let garages = marks
        .iter()
        .take(MAX_GARAGES)
        .enumerate()
        .map(|(position, &mark)| label(position, count, mark));
    std::iter::once(NONE.to_owned()).chain(garages).collect()
}

/// The garage at `position` of `count`, e.g. "Garage 3 of 12 (entered here)".
fn label(position: usize, count: usize, mark: Mark) -> String {
    let notes: Vec<&str> = [
        (mark.entered, "entered here"),
        (mark.pinned, "has pinned spawn point"),
    ]
    .into_iter()
    .filter_map(|(noted, note)| noted.then_some(note))
    .collect();
    let label = format!("Garage {}", position + 1);
    if notes.is_empty() {
        label
    } else {
        format!("{label} ({})", notes.join(", "))
    }
}

/// The list's item for the garage at `position` among the map's (none: the first item). A garage
/// past what the list holds shows as none.
fn item_of(position: Option<usize>) -> usize {
    position
        .filter(|&position| position < MAX_GARAGES)
        .map_or(0, |position| position + 1)
}

/// The position among the map's garages of the list's item `index` (none for the first item).
fn garage_of(index: usize) -> Option<usize> {
    index.checked_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_starts_with_none_then_counts_the_garages() {
        let items = labels(&[Mark::default(); 3]);
        assert_eq!(
            items,
            ["None", "Garage 1 of 3", "Garage 2 of 3", "Garage 3 of 3"]
        );
    }

    #[test]
    fn the_entry_garage_and_pinned_points_are_marked() {
        let marks = [
            Mark {
                entered: true,
                pinned: false,
            },
            Mark {
                entered: false,
                pinned: true,
            },
            Mark {
                entered: true,
                pinned: true,
            },
        ];
        assert_eq!(
            labels(&marks)[1..],
            [
                "Garage 1 of 3 (entered here)",
                "Garage 2 of 3 (has pinned spawn point)",
                "Garage 3 of 3 (entered here, has pinned spawn point)",
            ]
        );
    }

    #[test]
    fn items_and_garages_map_both_ways() {
        assert_eq!(item_of(None), 0);
        assert_eq!(garage_of(0), None);
        for position in [0, 1, MAX_GARAGES - 1] {
            assert_eq!(garage_of(item_of(Some(position))), Some(position));
        }
    }

    #[test]
    fn the_list_never_holds_more_than_a_list_can() {
        let items = labels(&vec![Mark::default(); MAX_GARAGES + 10]);
        assert_eq!(items.len(), MAX_ITEMS);
        assert_eq!(item_of(Some(MAX_GARAGES)), 0);
    }
}
