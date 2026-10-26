//! A more compact "Editor settings" section in the editor's side panel (nothing selected), a setting
//! ([`COMPACT_PANEL`]):
//!
//! - **Semi-compact:** automatic mirroring and its axis in one row (Off, X, Y, Z); the time of day
//!   and the atmosphere in one row of two sliders; the sun's height and direction in another.
//! - **Compact:** that, and the view mode list as a dropdown instead of rows of buttons.
//!
//! How (`settings_list::rework`): as the game lists that section's rows, the first row of each pair
//! gives its place to a row of ours, under its name, and the second one goes. So the panel's event
//! after a change names the setting as the game's row would (`EditorMirrorAxis`, or the world's
//! struct), and the game reacts as to its own row: saving, updating the mirrored bricks or the sky.
//! Our rows read and write the game's settings themselves (`game::editor_settings`). The setting
//! takes effect the next time the panel lists its rows (e.g. after selecting something).
//!
//! Only the panel changes: the settings are the game's, saved as the game saves them.

use std::sync::OnceLock;

use super::settings_list::{self, Rework};
use crate::framework::settings::{Alternative, Choice};
use crate::game::editor_settings::{self, EditorSettings, World};
use crate::game::property_reflection::{List, Number, Row, Unit, Value};
use crate::prelude::*;

/// How compact the side panel's editor settings are, a setting: the options of [`Compactness`],
/// in order.
pub static COMPACT_PANEL: Choice = Choice::new(
    "CompactSidePanel",
    "Compact side panel",
    "The editor settings in the vehicle editor's side panel (nothing selected), in fewer rows. \
     Semi-compact: mirroring and its axis in one row, time of day and atmosphere in one, the sun's \
     height and direction in one. Compact: also the view mode as a dropdown.",
    &[
        Alternative {
            key: "Off",
            name: "Off",
        },
        Alternative {
            key: "Semi",
            name: "Semi-compact",
        },
        Alternative {
            key: "Full",
            name: "Compact",
        },
    ],
    0,
);

/// The options of [`COMPACT_PANEL`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Compactness {
    Off,
    Semi,
    Full,
}

impl Compactness {
    fn chosen() -> Self {
        match COMPACT_PANEL.get() {
            1 => Self::Semi,
            2 => Self::Full,
            _ => Self::Off,
        }
    }
}

/// What the side panel asks the settings list for, with nothing selected.
const EDITOR_SETTINGS: &str = "Properties.Editor";

/// The game's rows reworked, by the fields they edit: the first of each pair is replaced, the
/// second merged into it (see [`rework`]).
const MIRROR_AXIS: &str = "EditorMirrorAxis";
const MIRROR_ENABLED: &str = "bEditorMirrorAxisEnabled";
const TIME_OF_DAY: &str = "TimeOfDay";
const ATMOSPHERE: &str = "AtmosphereDensity";
const SUN_ANGLE: &str = "SunAngle";
const SUN_YAW: &str = "SunYaw";
const VIEW_MODE: &str = "EditorViewMode";

/// Our rows, made after engine init.
struct Rows {
    mirror: Row,
    time: Row,
    sun: Row,
}

static ROWS: OnceLock<Rows> = OnceLock::new();
static SETTINGS: OnceLock<EditorSettings> = OnceLock::new();

pub struct CompactPanel;

impl Feature for CompactPanel {
    fn id(&self) -> &'static str {
        "compact_panel"
    }

    fn on_engine_init(&self) {
        match start() {
            Ok(()) => log!("active: the side panel's editor settings can be compact"),
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

fn start() -> Result<(), String> {
    let _ = SETTINGS.set(editor_settings::find()?);
    let rows = Rows {
        mirror: settings_list::row_named(
            MIRROR_AXIS,
            "Mirroring",
            "Automatic mirroring: off, or across the X, Y or Z axis.",
            Value::List(&MIRROR),
        )?,
        time: settings_list::row_named(
            TIME_OF_DAY,
            "Time and atmosphere",
            "The editor's time of day, then the density of its atmosphere.",
            Value::Number(&TIME),
        )?,
        sun: settings_list::row_named(
            SUN_ANGLE,
            "Sun elevation and direction",
            "The sun's height above the horizon, then its direction.",
            Value::Number(&SUN),
        )?,
    };
    let _ = ROWS.set(rows);
    settings_list::rework(EDITOR_SETTINGS, rework)
}

/// What becomes of the game's row editing `field`, as compact as chosen.
fn rework(field: &str) -> Rework {
    let compactness = Compactness::chosen();
    let Some(rows) = ROWS.get() else {
        return Rework::Keep;
    };
    if compactness == Compactness::Off {
        return Rework::Keep;
    }
    match field {
        MIRROR_AXIS => Rework::Replace(&rows.mirror),
        MIRROR_ENABLED => Rework::MergeInto(&rows.mirror),
        TIME_OF_DAY => Rework::Replace(&rows.time),
        ATMOSPHERE => Rework::MergeInto(&rows.time),
        SUN_ANGLE => Rework::Replace(&rows.sun),
        SUN_YAW => Rework::MergeInto(&rows.sun),
        VIEW_MODE if compactness == Compactness::Full => Rework::Dropdown,
        _ => Rework::Keep,
    }
}

/// Automatic mirroring and its axis: Off, X, Y, Z.
struct Mirror;

static MIRROR: Mirror = Mirror;

impl List for Mirror {
    fn items(&self) -> Vec<String> {
        ["Off", "X", "Y", "Z"].map(str::to_owned).to_vec()
    }

    fn chosen(&self) -> usize {
        // SAFETY: the panel lists rows on the game thread.
        SETTINGS
            .get()
            .and_then(|settings| unsafe { settings.mirror() })
            .map_or(0, usize::from)
    }

    fn choose(&self, index: usize) -> bool {
        let Some(settings) = SETTINGS.get() else {
            return false;
        };
        let axis = match index {
            0 => None,
            1..=3 => Some(index as u8),
            _ => return false,
        };
        // SAFETY: the panel changes rows on the game thread.
        unsafe { settings.set_mirror(axis) };
        true
    }
}

/// Two floats of the editor's world in one row, each with its own range and unit.
struct WorldPair {
    fields: [World; 2],
    ranges: [(f32, f32); 2],
    units: [Unit; 2],
}

/// The time of day (h:m), then the atmosphere's density (a percentage), as the game's rows show
/// them.
static TIME: WorldPair = WorldPair {
    fields: [World::TimeOfDay, World::AtmosphereDensity],
    ranges: [(0.0, 24.0), (0.0, 1.0)],
    units: [Unit::TimeOfDay, Unit::Percent],
};

/// The sun's height (0° to 90°), then its direction (±180°), as the game's rows show them.
static SUN: WorldPair = WorldPair {
    fields: [World::SunAngle, World::SunYaw],
    ranges: [(0.0, 90.0), (-180.0, 180.0)],
    units: [Unit::Angle, Unit::Angle],
};

impl Number for WorldPair {
    fn values(&self) -> Vec<f32> {
        let Some(settings) = SETTINGS.get() else {
            return vec![0.0; 2];
        };
        // SAFETY: the panel lists rows on the game thread.
        self.fields
            .iter()
            .map(|&field| unsafe { settings.world(field) })
            .collect()
    }

    fn set_values(&self, values: &[f32]) {
        let Some(settings) = SETTINGS.get() else {
            return;
        };
        for ((&field, &(min, max)), &value) in self.fields.iter().zip(&self.ranges).zip(values) {
            // SAFETY: the panel changes rows on the game thread.
            unsafe { settings.set_world(field, value.clamp(min, max)) };
        }
    }

    /// Both values' ranges: each is clamped to its own when set.
    fn range(&self) -> (f32, f32) {
        let [(min_a, max_a), (min_b, max_b)] = self.ranges;
        (min_a.min(min_b), max_a.max(max_b))
    }

    fn component_range(&self, index: usize) -> (f32, f32) {
        self.ranges.get(index).copied().unwrap_or(self.range())
    }

    fn unit(&self) -> Unit {
        self.units[0]
    }

    fn component_unit(&self, index: usize) -> Unit {
        self.units.get(index).copied().unwrap_or(self.unit())
    }

    fn alike(&self) -> bool {
        false
    }
}
