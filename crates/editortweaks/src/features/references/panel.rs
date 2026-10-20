//! The references' rows in the editor's side panel.
//!
//! With nothing selected, in the References section after the map's rows
//! (`features/map_in_editor/panel.rs`):
//!
//! - **Add reference**: Image... or Model..., opening the Windows file dialog; the new reference
//!   links to its file where it is, or copies it next to the vehicle, as the setting [`ADD_AS`]
//!   says (the File row changes it afterwards).
//! - **Select references** (the setting [`select::SELECTABLE`]), **Show references** (also a key,
//!   `keybinds::SHOW_REFERENCES`) to hide them all for now, and **Hidden references** to show again
//!   those made invisible (opacity 0, or hidden in the sidecar), which can't be clicked.
//!
//! With references selected (clicked, see [`select`]), the panel shows them instead of the
//! editor's settings, as it shows selected bricks (`settings_list::replace`): their opacity, link
//! or copy, position, rotation, width (images) or scale (models); for models their pivot, and
//! for OBJ and STL files (glTF fixes them) the file's up and front axes and units. The first
//! selected reference's values show; a change goes to every selected reference the row fits:
//! positions move them together, the other values are set on each.
//!
//! Rows run on the game thread while the panel lists or changes them (never inside a tick of the
//! feature), so they use the feature's state directly; each change shows at the next tick and is
//! written to the sidecar a moment later ([`State::save`]). Rows that come and go with the
//! selection have the panel list its rows again at the next tick.

use std::path::Path;

use model_io::plane::View;
use model_io::readers::Format;
use model_io::sidecar::{Kind, Reference, SourceMode};
use model_io::space::{Axis, Pivot, Units};

use super::{CODE, State, edit, files, select, state};
use crate::features::settings_list;
use crate::framework::settings::{Alternative, Choice, Setting};
use crate::game::property_reflection::{Check, List, Number, Row, Unit, Value};
use crate::prelude::*;

/// What the editor's side panel asks the settings list for, with no brick selected, and the
/// section's title: the map's section (`map_in_editor::panel`), which these rows join.
const EDITOR_SETTINGS: &str = "Properties.Editor";
const SECTION: &str = "References";

/// How far from the vehicle's box a new picture goes (placed behind it as seen from the front), in
/// cm.
const VIEW_GAP: f64 = 20.0;

/// The ends of the position sliders (typed values go further, up to the limit), in cm.
const POSITION_RANGE: (f32, f32) = (-10_000.0, 10_000.0);
/// The ends of an image's width, in cm.
const WIDTH_RANGE: (f32, f32) = (1.0, 100_000.0);
/// The ends of a model's scale (1 is its size in the file).
const SCALE_RANGE: (f32, f32) = (0.001, 100.0);

/// A row: its key, name, tooltip, value, and when it's listed.
type RowSpec = (
    &'static str,
    &'static str,
    &'static str,
    Value,
    fn() -> bool,
);

/// Whether new references link to their file or copy it, a setting.
pub static ADD_AS: Choice = Choice::new(
    "ReferencesAddAs",
    "New references",
    "Whether a reference you add links to its file where it is, or copies it into the vehicle's folder, so moving or deleting the original doesn't lose it (on this PC only: copies aren't uploaded with the vehicle). The File row of a selected reference changes it afterwards.",
    &[
        Alternative {
            key: "Link",
            name: "Link to the file",
        },
        Alternative {
            key: "Copy",
            name: "Copy the file",
        },
    ],
    0,
);

/// The rows with nothing selected, after the map's.
const GENERAL: [RowSpec; 3] = [
    (
        "ReferencesAdd",
        "Add reference",
        ADD_DESCRIPTION,
        Value::List(&ADD),
        always,
    ),
    (
        "ReferencesShown",
        "Show references",
        SHOWN_DESCRIPTION,
        Value::Check(&SHOWN),
        has_references,
    ),
    (
        "ReferencesHidden",
        "Hidden references",
        HIDDEN_DESCRIPTION,
        Value::List(&SHOW_HIDDEN),
        has_hidden,
    ),
];

/// Where the setting [`select::SELECTABLE`] goes among [`GENERAL`]: after the Add row.
const SELECTABLE_AT: usize = 1;

/// The sections with references selected: their titles and rows.
const SELECTED: [(&str, &[RowSpec]); 2] = [
    (
        "Reference",
        &[
            (
                "ReferenceOpacity",
                "Opacity",
                OPACITY_DESCRIPTION,
                Value::Number(&OPACITY),
                always,
            ),
            (
                "ReferenceFile",
                "File",
                FILE_DESCRIPTION,
                Value::List(&FILE),
                always,
            ),
            (
                "ReferencePosition",
                "Position",
                POSITION_DESCRIPTION,
                Value::Number(&POSITION),
                always,
            ),
            (
                "ReferenceRotation",
                "Rotation",
                ROTATION_DESCRIPTION,
                Value::Number(&ROTATION),
                always,
            ),
            (
                "ReferenceWidth",
                "Width",
                WIDTH_DESCRIPTION,
                Value::Number(&WIDTH),
                has_image,
            ),
            (
                "ReferenceScale",
                "Scale",
                SCALE_DESCRIPTION,
                Value::Number(&SCALE),
                has_model,
            ),
        ],
    ),
    (
        "Model file",
        &[
            (
                "ReferencePivot",
                "Pivot",
                PIVOT_DESCRIPTION,
                Value::List(&PIVOT),
                has_model,
            ),
            (
                "ReferenceUp",
                "Up axis",
                UP_DESCRIPTION,
                Value::List(&UP),
                has_axes,
            ),
            (
                "ReferenceForward",
                "Front axis",
                FORWARD_DESCRIPTION,
                Value::List(&FORWARD),
                has_axes,
            ),
            (
                "ReferenceUnits",
                "Units",
                UNITS_DESCRIPTION,
                Value::List(&UNITS),
                has_axes,
            ),
        ],
    ),
];

/// Adds the rows to the editor's side panel. After engine init. A row that can't be made (e.g. no
/// number rows in this game version) is left out, the others stay.
pub(super) fn add() -> Result<(), String> {
    let mut general = Vec::new();
    for (index, spec) in GENERAL.iter().enumerate() {
        if index == SELECTABLE_AT {
            match settings_list::setting_row(Setting::Toggle(&select::SELECTABLE)) {
                Ok(row) => general.push(row.when(has_references)),
                Err(reason) => log!("no row for Select references: {reason}"),
            }
        }
        push_row(&mut general, spec);
    }
    settings_list::add(EDITOR_SETTINGS, vec![(SECTION, general)])?;

    let sections = SELECTED
        .iter()
        .map(|&(title, specs)| {
            let mut rows = Vec::new();
            for spec in specs {
                push_row(&mut rows, spec);
            }
            (title, rows)
        })
        .collect();
    settings_list::replace(EDITOR_SETTINGS, sections, is_selected)
}

/// Makes the row `spec` and adds it to `rows`, or logs why it can't be made.
fn push_row(rows: &mut Vec<Row>, &(key, name, description, value, when): &RowSpec) {
    match settings_list::row(key, name, description, value) {
        Ok(row) => rows.push(row.when(when)),
        Err(reason) => log!("no row {name}: {reason}"),
    }
}

fn always() -> bool {
    true
}

const ADD_DESCRIPTION: &str = "Add a picture (PNG or JPEG, such as a blueprint, placed behind the vehicle as seen from the front) or a 3D model (glTF, GLB, OBJ or STL) to build after. Whether it links to the file or copies it is the New references setting (Editor tweaks options). Only you see it; the vehicle doesn't change.";
const HIDDEN_DESCRIPTION: &str =
    "Some references are invisible (opacity 0), so they can't be clicked: show them again.";
const SHOWN_DESCRIPTION: &str = "Uncheck to make every reference invisible for now (also the Show references key, 2 by default); they come back as they were. Hide (H) hides the selected ones only, like bricks, until Unhide all (Alt + H).";
const OPACITY_DESCRIPTION: &str = "How opaque the reference is: see-through ones let the vehicle show through them (models dithered or smooth: a setting, Editor tweaks options). At 0 it's invisible and can't be clicked. With several references selected, the first one's values show here, and a change goes to all of them.";
const POSITION_DESCRIPTION: &str =
    "Where the reference is, from the vehicle's origin. Several selected move together.";
const ROTATION_DESCRIPTION: &str =
    "How the reference is turned: about the vehicle's front axis, side axis and vertical axis.";
const WIDTH_DESCRIPTION: &str = "How wide the picture is (its height follows).";
const SCALE_DESCRIPTION: &str = "The model's size: 100% is its size in its file.";
const FILE_DESCRIPTION: &str = "Link: the reference shows the file where it is. Copy: the file is copied into the vehicle's folder, so moving or deleting the original doesn't lose it (on this PC only: copies aren't uploaded with the vehicle).";
const PIVOT_DESCRIPTION: &str = "Which point of the model is at the reference's position: the file's origin, the middle of the model, or the middle of its bottom.";
const UP_DESCRIPTION: &str =
    "The axis pointing up in the model's file (Blender writes +Y in OBJ files, Z-up tools +Z).";
const FORWARD_DESCRIPTION: &str =
    "The axis the model's front faces in its file: it faces the vehicle's front.";
const UNITS_DESCRIPTION: &str = "What one unit of the model's file is.";

/// An edit's context besides the reference.
pub(super) struct Context<'a> {
    folder: Option<&'a Path>,
}

/// `read` of the first selected reference it gives something for.
fn first_selected<T>(read: impl FnMut(&Reference) -> Option<T>) -> Option<T> {
    let state = state()?;
    state
        .selected
        .iter()
        .filter_map(|id| {
            state
                .sidecar
                .references
                .iter()
                .find(|reference| reference.id == *id)
        })
        .find_map(read)
}

/// Whether some selected reference passes `test`.
fn any_selected(test: fn(&Reference) -> bool) -> bool {
    first_selected(|reference| test(reference).then_some(())).is_some()
}

/// Changes each selected reference with `change`, then saves. Whether it could change them all.
fn edit_selected(mut change: impl FnMut(&mut Reference, &Context) -> Result<(), String>) -> bool {
    let Some(mut state) = state() else {
        return false;
    };
    let state = &mut *state;
    let folder = state.folder.clone();
    let context = Context {
        folder: folder.as_deref(),
    };
    let mut all = true;
    let mut changed_any = false;
    for reference in state
        .sidecar
        .references
        .iter_mut()
        .filter(|reference| state.selected.contains(&reference.id))
    {
        let mut changed = reference.clone();
        if let Err(reason) = change(&mut changed, &context) {
            log!("{}: {reason}", changed.name);
            all = false;
            continue;
        }
        if changed != *reference {
            *reference = changed;
            changed_any = true;
        }
    }
    if changed_any {
        state.save();
    }
    all
}

/// Adds a reference of `kind` for the file at `path`, linked or copied as `mode` says, and selects
/// it: an image placed behind the vehicle as seen from the front, as wide as the vehicle.
pub(super) fn add_file(state: &mut State, kind: Kind, path: &Path, mode: SourceMode) {
    let folder = state.folder.clone();
    let id = match edit::add(&mut state.sidecar, folder.as_deref(), kind, path, mode) {
        Ok(id) => id,
        Err(reason) => {
            log!("not added: {reason}");
            return;
        }
    };
    let placed = (kind == Kind::Image).then(|| View::Front.placement(vehicle_box(state), VIEW_GAP));
    if let Some(reference) = state.sidecar.references.last_mut() {
        if let Some(placed) = placed {
            reference.position = placed.position;
            reference.rotation = placed.rotation;
            if let (Some(width), Some(image)) = (placed.width, reference.image.as_mut()) {
                image.width = width;
            }
        }
        log!(
            "added {} ({}, {:?})",
            reference.name,
            path.display(),
            reference.source.mode
        );
    }
    state.select(vec![id]);
    state.save();
}

/// The box around the vehicle's bricks, if there are any.
fn vehicle_box(state: &State) -> Option<[[f64; 3]; 2]> {
    let code = CODE.get()?;
    let editor = state.editor.as_ref()?.get()?;
    // SAFETY: rows run on the game thread; the editor is live (just checked).
    unsafe { code.boxes.of(editor) }
}

fn has_references() -> bool {
    state().is_some_and(|state| !state.sidecar.references.is_empty())
}

/// Whether a reference is hidden: invisible, so it can't be clicked.
fn is_hidden(reference: &Reference) -> bool {
    !reference.visible || reference.opacity <= 0.0
}

fn has_hidden() -> bool {
    state().is_some_and(|state| state.sidecar.references.iter().any(is_hidden))
}

fn is_selected() -> bool {
    any_selected(|_| true)
}

fn is_image(reference: &Reference) -> bool {
    reference.kind == Kind::Image
}

fn is_model(reference: &Reference) -> bool {
    reference.kind == Kind::Model
}

/// A model whose format leaves its axes and units to the user.
fn leaves_axes(reference: &Reference) -> bool {
    is_model(reference)
        && Format::of(Path::new(&reference.source.path))
            .is_some_and(|format| !format.has_fixed_space())
}

fn has_image() -> bool {
    any_selected(is_image)
}

fn has_model() -> bool {
    any_selected(is_model)
}

fn has_axes() -> bool {
    any_selected(leaves_axes)
}

fn labels(labels: &[&str]) -> Vec<String> {
    labels.iter().map(|&label| label.to_owned()).collect()
}

/// A row of buttons that act when clicked: none of them stays chosen (the list's chosen index is
/// past its items).
struct Buttons {
    labels: &'static [&'static str],
    click: fn(usize),
}

impl List for Buttons {
    fn items(&self) -> Vec<String> {
        labels(self.labels)
    }

    fn chosen(&self) -> usize {
        self.labels.len()
    }

    fn choose(&self, index: usize) -> bool {
        (self.click)(index);
        true
    }
}

/// The kinds the Add row offers, in order.
const ADD_KINDS: [Kind; 2] = [Kind::Image, Kind::Model];

static ADD: Buttons = Buttons {
    labels: &["Image...", "Model..."],
    click: |index| {
        if let Some(&kind) = ADD_KINDS.get(index) {
            browse(kind);
        }
    },
};

/// Opens the file dialog for a reference of `kind`, linked or copied as [`ADD_AS`] says.
fn browse(kind: Kind) {
    let Some(mut state) = state() else {
        return;
    };
    if state.browsing.is_some() {
        return;
    }
    let mode = if ADD_AS.get() == 1 {
        SourceMode::Copy
    } else {
        SourceMode::Link
    };
    match files::browse(kind) {
        Some(dialog) => state.browsing = Some((kind, mode, dialog)),
        None => log!("a file dialog is open already"),
    }
}

static SHOW_HIDDEN: Buttons = Buttons {
    labels: &["Show all"],
    click: |_| {
        let Some(mut state) = state() else {
            return;
        };
        for reference in state.sidecar.references.iter_mut().filter(|r| is_hidden(r)) {
            reference.visible = true;
            if reference.opacity <= 0.0 {
                reference.opacity = 1.0;
            }
        }
        state.refresh_panel = true;
        state.save();
    },
};

/// The "Show references" check box.
struct Shown;

static SHOWN: Shown = Shown;

impl Check for Shown {
    fn checked(&self) -> bool {
        state().is_none_or(|state| state.shown)
    }

    fn set_checked(&self, checked: bool) {
        if let Some(mut state) = state() {
            state.show_all(checked);
        }
    }
}

/// How a change of the first selected reference's values goes to the other selected ones.
#[derive(Clone, Copy)]
enum Together {
    /// Each changed value is set on every reference.
    Same,
    /// Every reference's value changes by as much as the first one's: they move together.
    Moved,
}

/// Numbers of the selected references.
struct Numbers {
    /// A reference's values, if the row fits it.
    get: fn(&Reference) -> Option<Vec<f64>>,
    /// Sets value `index` of a reference the row fits.
    set: fn(&mut Reference, usize, f64),
    together: Together,
    range: (f32, f32),
    unit: Unit,
}

impl Number for Numbers {
    fn values(&self) -> Vec<f32> {
        first_selected(self.get)
            .unwrap_or_default()
            .into_iter()
            .map(|value| value as f32)
            .collect()
    }

    fn set_values(&self, values: &[f32]) {
        let Some(old) = first_selected(self.get) else {
            return;
        };
        // Only the values the panel changed: the others keep their exact values on every reference.
        let changed: Vec<(usize, f64)> = old
            .iter()
            .zip(values)
            .enumerate()
            .filter(|&(_, (&old, &new))| old as f32 != new)
            .map(|(index, (_, &new))| (index, widen(new)))
            .collect();
        if changed.is_empty() {
            return;
        }
        edit_selected(|reference, _| {
            let Some(mine) = (self.get)(reference) else {
                return Ok(());
            };
            for &(index, new) in &changed {
                let (Some(&value), Some(&first)) = (mine.get(index), old.get(index)) else {
                    continue;
                };
                let value = match self.together {
                    Together::Same => new,
                    Together::Moved => value + (new - first),
                };
                (self.set)(reference, index, value);
            }
            Ok(())
        });
    }

    fn range(&self) -> (f32, f32) {
        self.range
    }

    fn unit(&self) -> Unit {
        self.unit
    }
}

/// `value` as the decimal number it shows as (12.3, not 12.300000190734863).
fn widen(value: f32) -> f64 {
    value.to_string().parse().unwrap_or(f64::from(value))
}

static OPACITY: Numbers = Numbers {
    get: |reference| Some(vec![f64::from(reference.opacity)]),
    set: |reference, _, opacity| reference.opacity = (opacity as f32).clamp(0.0, 1.0),
    together: Together::Same,
    range: (0.0, 1.0),
    unit: Unit::Percent,
};

static POSITION: Numbers = Numbers {
    get: |reference| Some(reference.position.to_vec()),
    set: |reference, index, value| {
        if let Some(axis) = reference.position.get_mut(index) {
            *axis = value;
        }
    },
    together: Together::Moved,
    range: POSITION_RANGE,
    unit: Unit::Distance,
};

/// Where the rotation row's values (roll, pitch, yaw: about X, Y, Z, as the game shows bricks')
/// are in the sidecar's `FRotator` (pitch, yaw, roll).
const ROTATION_ORDER: [usize; 3] = [2, 0, 1];

static ROTATION: Numbers = Numbers {
    get: |reference| Some(ROTATION_ORDER.map(|at| reference.rotation[at]).to_vec()),
    set: |reference, index, value| {
        if let Some(&at) = ROTATION_ORDER.get(index) {
            reference.rotation[at] = value;
        }
    },
    together: Together::Same,
    range: (-180.0, 180.0),
    unit: Unit::Angle,
};

static WIDTH: Numbers = Numbers {
    get: |reference| reference.image.as_ref().map(|image| vec![image.width]),
    set: |reference, _, width| {
        if let Some(image) = reference.image.as_mut()
            && width > 0.0
        {
            image.width = width;
        }
    },
    together: Together::Same,
    range: WIDTH_RANGE,
    unit: Unit::Distance,
};

/// One scale for all three axes; scales differing per axis (edited by hand) keep their ratios.
static SCALE: Numbers = Numbers {
    get: |reference| is_model(reference).then(|| vec![reference.scale[0]]),
    set: |reference, _, scale| {
        let old = reference.scale[0];
        if scale <= 0.0 {
            return;
        }
        reference.scale = if old > 0.0 {
            reference.scale.map(|axis| axis * scale / old)
        } else {
            [scale; 3]
        };
    },
    together: Together::Same,
    range: SCALE_RANGE,
    unit: Unit::Percent,
};

/// A list showing and changing something of the selected references.
struct Options {
    items: fn() -> Vec<String>,
    /// A reference's chosen item, if the row fits it.
    chosen: fn(&Reference) -> Option<usize>,
    choose: fn(&mut Reference, usize, &Context) -> Result<(), String>,
}

impl List for Options {
    fn items(&self) -> Vec<String> {
        (self.items)()
    }

    fn chosen(&self) -> usize {
        first_selected(self.chosen).unwrap_or(0)
    }

    fn choose(&self, index: usize) -> bool {
        edit_selected(|reference, context| {
            if (self.chosen)(reference).is_none() {
                return Ok(());
            }
            (self.choose)(reference, index, context)
        })
    }
}

static FILE: Options = Options {
    items: || labels(&["Link", "Copy"]),
    chosen: |reference| Some(usize::from(reference.source.mode == SourceMode::Copy)),
    choose: |reference, index, context| {
        let mode = if index == 1 {
            SourceMode::Copy
        } else {
            SourceMode::Link
        };
        edit::set_mode(reference, context.folder, mode)
    },
};

/// The pivots offered, in order.
const PIVOTS: [Pivot; 3] = [Pivot::Origin, Pivot::Center, Pivot::Bottom];

static PIVOT: Options = Options {
    items: || labels(&["Origin", "Middle", "Bottom"]),
    chosen: |reference| {
        let pivot = reference.import.unwrap_or_default().pivot;
        is_model(reference).then(|| PIVOTS.iter().position(|&known| known == pivot).unwrap_or(0))
    },
    choose: |reference, index, _| {
        let import = reference.import.get_or_insert_with(Default::default);
        import.pivot = *PIVOTS.get(index).ok_or("no such pivot")?;
        Ok(())
    },
};

fn axis_labels() -> Vec<String> {
    Axis::ALL
        .iter()
        .map(|axis| axis.label().to_owned())
        .collect()
}

fn axis_at(index: usize) -> Result<Axis, String> {
    Axis::ALL.get(index).copied().ok_or("no such axis".into())
}

static UP: Options = Options {
    items: axis_labels,
    chosen: |reference| {
        leaves_axes(reference).then(|| reference.import.unwrap_or_default().up as usize)
    },
    choose: |reference, index, _| {
        let import = reference.import.unwrap_or_default();
        reference.import = Some(import.with_up(axis_at(index)?));
        Ok(())
    },
};

static FORWARD: Options = Options {
    items: axis_labels,
    chosen: |reference| {
        leaves_axes(reference).then(|| reference.import.unwrap_or_default().forward as usize)
    },
    choose: |reference, index, _| {
        let import = reference.import.unwrap_or_default();
        reference.import = Some(import.with_forward(axis_at(index)?));
        Ok(())
    },
};

static UNITS: Options = Options {
    items: || {
        Units::ALL
            .iter()
            .map(|units| units.label().to_owned())
            .collect()
    },
    chosen: |reference| {
        let units = reference.import.unwrap_or_default().units;
        leaves_axes(reference).then(|| {
            Units::ALL
                .iter()
                .position(|&known| known == units)
                .unwrap_or(0)
        })
    },
    choose: |reference, index, _| {
        let import = reference.import.get_or_insert_with(Default::default);
        import.units = *Units::ALL.get(index).ok_or("no such units")?;
        Ok(())
    },
};
