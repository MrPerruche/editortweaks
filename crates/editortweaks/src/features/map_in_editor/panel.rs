//! The map's controls in the editor's side panel. With nothing selected, the panel lists the
//! editor's settings (it asks the settings list for `Properties.Editor`, see
//! `docs/re/map-in-editor.md`), then a "References" section of ours, starting with the map's rows
//! (the reference images and models' rows follow, from `features/references`):
//!
//! - **Show map:** a dropdown of none, then the map's garages ("Garage 3"), the one the editor was
//!   entered from marked. A garage with a pinned spawn point of its own is listed twice: first at
//!   that point ("Garage 3 (pinned spawn)", where the game spawns the vehicle when the vehicle
//!   browser's spawn option is Pinned, as city covers need: unpinned, they fall), then at its
//!   normal one. The selected garage's spawn point is marked in the editor, and the map shows
//!   around the vehicle as at that garage (if it can be shown). The map key goes to none and back
//!   to the last garage (if none, the default one at its pinned point).
//!
//! What the row shows and changes is [`Controls`], kept apart from the feature's state: a tick holds
//! that while it calls the game, and the panel reads the row whenever the game lists it. The row
//! only notes what was asked. The next tick acts on it as on the map key, and has the panel list the
//! rows again ([`refresh`]) when it changed what the row shows: the map key, or a garage gone.

use std::sync::{Mutex, MutexGuard, Once, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;

use super::placement;
use super::{GAME, Game};
use crate::features::settings_list;
use crate::game::garage::Garage;
use crate::game::property_reflection::{List, MAX_ITEMS, Value};
use crate::game::weak_object::WeakObject;
use crate::prelude::*;

/// What the editor's side panel asks the settings list for, with nothing selected.
pub const EDITOR_SETTINGS: &str = "Properties.Editor";

/// The section's title. The map is a reference too: the `references` feature lists its rows in the
/// same section, after these (sections titled alike show as one, see `features/settings_list`).
pub const SECTION: &str = "References";

/// How many garage items the list can hold: after "None", what's left of a list's items.
const MAX_GARAGES: usize = MAX_ITEMS - 1;

const NONE: &str = "Hidden";

const GARAGE_DESCRIPTION: &str = "Show the map, positionned to match where your vehicle will be placed if spawned at a garage. Some garages may spawn you differently if you spawn your vehicle pinned.";

/// What the row and the map key ask for, and what the row shows.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct Controls {
    /// The garage whose spawn point is marked.
    pub garage: Option<WeakObject>,
    /// Whether its pinned spawn point is the one to use (garages without one use the normal one
    /// either way). On until a garage's normal spawn point is chosen.
    pub pinned: bool,
    /// Whether the map is to show: a garage was chosen, and the map wasn't refused since.
    pub show_map: bool,
    /// The garage (and `pinned`) last chosen before none, for the map key to go back to.
    pub last: Option<(WeakObject, bool)>,
    /// The editor they're for: the garage it was entered from is marked in the list.
    pub editor: Option<WeakObject>,
    /// Whether the row changed `garage` since the last tick.
    pub asked: bool,
}

impl Controls {
    const fn new() -> Self {
        Self {
            garage: None,
            pinned: true,
            show_map: false,
            last: None,
            editor: None,
            asked: false,
        }
    }

    /// No garage, no map; the garage is kept as the last one.
    pub fn hide(&mut self) {
        if let Some(garage) = self.garage.take() {
            self.last = Some((garage, self.pinned));
        }
        self.show_map = false;
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
    // Where the map can't be shown, the garages' spawn points can still be marked.
    let name = if game.can_show && game.camera_agrees && game.isolation.is_some() {
        "Show map"
    } else {
        "Garage"
    };
    let row = settings_list::row(
        "MapGarage",
        name,
        GARAGE_DESCRIPTION,
        Value::List(&GARAGE_LIST),
    )?;
    settings_list::add(EDITOR_SETTINGS, vec![(SECTION, vec![row])])
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
        let marks = marks(&garages, entry);
        let needed: usize = marks
            .iter()
            .map(|mark| 1 + usize::from(mark.pinned_point))
            .sum();
        if needed > MAX_GARAGES {
            TOO_MANY.call_once(|| {
                log!(
                    "the map's {} garages need {needed} items: the panel lists the first {MAX_GARAGES}",
                    garages.len()
                )
            });
        }
        labels(&marks)
    }

    fn chosen(&self) -> usize {
        let controls = *controls();
        let Some(garage) = controls.garage else {
            return 0;
        };
        // SAFETY: the panel lists rows on the game thread.
        let garages = unsafe { garages() };
        let Some(position) = garages.iter().position(|known| garage.is(known.actor)) else {
            return 0;
        };
        let wanted = Item {
            position,
            pinned: placement::has_pinned_point(&garages[position]).then_some(controls.pinned),
        };
        item_of(&items(&marks(&garages, None)), wanted)
    }

    fn choose(&self, index: usize) -> bool {
        let chosen = match index.checked_sub(1) {
            None => None,
            Some(at) => {
                // SAFETY: the panel changes rows on the game thread.
                let garages = unsafe { garages() };
                let Some(item) = items(&marks(&garages, None)).get(at).copied() else {
                    return false;
                };
                let Some(garage) = garages.get(item.position) else {
                    return false;
                };
                // SAFETY: the map's garages are live objects.
                let garage = unsafe { WeakObject::new(garage.actor) };
                Some((garage, item.pinned.unwrap_or(true)))
            }
        };
        let mut controls = controls();
        match chosen {
            Some((garage, pinned)) => {
                controls.garage = Some(garage);
                controls.pinned = pinned;
                controls.show_map = true;
            }
            None => controls.hide(),
        }
        controls.asked = true;
        true
    }

    /// A map has up to dozens of garages: side by side, they'd be slivers.
    fn dropdown(&self) -> bool {
        true
    }
}

/// What the list notes about a garage.
#[derive(Clone, Copy, Default)]
struct Mark {
    /// The editor was entered from it.
    entered: bool,
    /// It has a pinned spawn point of its own.
    pinned_point: bool,
}

/// What the list notes about each of `garages`, `entry` being the one the editor was entered from.
fn marks(garages: &[Garage], entry: Option<*mut UObjectBase>) -> Vec<Mark> {
    garages
        .iter()
        .map(|garage| Mark {
            entered: entry == Some(garage.actor),
            pinned_point: placement::has_pinned_point(garage),
        })
        .collect()
}

/// One of the list's garage items: the garage at `position` among the map's, at its pinned spawn
/// point or its normal one (`None`: it has no pinned spawn point).
#[derive(Clone, Copy, PartialEq, Debug)]
struct Item {
    position: usize,
    pinned: Option<bool>,
}

/// The list's garage items (at most [`MAX_GARAGES`]): each garage, twice if it has a pinned spawn
/// point (pinned first, so it's the one picked by default).
fn items(marks: &[Mark]) -> Vec<Item> {
    marks
        .iter()
        .enumerate()
        .flat_map(|(position, mark)| {
            let spawns: &[Option<bool>] = if mark.pinned_point {
                &[Some(true), Some(false)]
            } else {
                &[None]
            };
            spawns.iter().map(move |&pinned| Item { position, pinned })
        })
        .take(MAX_GARAGES)
        .collect()
}

/// The garage list's labels: none, then each item, e.g. "Garage 3 (entered here, pinned spawn)".
fn labels(marks: &[Mark]) -> Vec<String> {
    let garages = items(marks)
        .into_iter()
        .map(|item| label(item, marks[item.position]));
    std::iter::once(NONE.to_owned()).chain(garages).collect()
}

fn label(item: Item, mark: Mark) -> String {
    let notes: Vec<&str> = [
        (mark.entered, "closest"),
        (item.pinned == Some(true), "pinned"),
    ]
    .into_iter()
    .filter_map(|(noted, note)| noted.then_some(note))
    .collect();
    let label = format!("Garage {}", item.position + 1);
    if notes.is_empty() {
        label
    } else {
        format!("{label} ({})", notes.join(", "))
    }
}

/// The list's item showing `wanted` among `items` (none, the first item, if it isn't listed).
fn item_of(items: &[Item], wanted: Item) -> usize {
    items
        .iter()
        .position(|&item| item == wanted)
        .map_or(0, |at| at + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_starts_with_none_then_counts_the_garages() {
        let items = labels(&[Mark::default(); 3]);
        assert_eq!(items, ["None", "Garage 1", "Garage 2", "Garage 3"]);
    }

    #[test]
    fn garages_with_a_pinned_point_are_listed_twice_pinned_first() {
        let marks = [
            Mark {
                entered: true,
                pinned_point: false,
            },
            Mark {
                entered: false,
                pinned_point: true,
            },
            Mark {
                entered: true,
                pinned_point: true,
            },
        ];
        assert_eq!(
            labels(&marks)[1..],
            [
                "Garage 1 (entered here)",
                "Garage 2 (pinned spawn)",
                "Garage 2",
                "Garage 3 (entered here, pinned spawn)",
                "Garage 3 (entered here)",
            ]
        );
    }

    #[test]
    fn items_map_both_ways() {
        let marks = [
            Mark::default(),
            Mark {
                entered: false,
                pinned_point: true,
            },
        ];
        let items = items(&marks);
        for (at, &item) in items.iter().enumerate() {
            assert_eq!(item_of(&items, item), at + 1);
        }
        let missing = Item {
            position: 5,
            pinned: None,
        };
        assert_eq!(item_of(&items, missing), 0);
    }

    #[test]
    fn the_list_never_holds_more_than_a_list_can() {
        let marks = vec![
            Mark {
                entered: false,
                pinned_point: true,
            };
            MAX_GARAGES
        ];
        assert_eq!(labels(&marks).len(), MAX_ITEMS);
    }
}
