//! Lengths written over the 3D view: the length of the line tool's purple lines (see
//! [`super::line_tool`]), beside the middle of each (the line placed, and the dashed one from a
//! first point to what a click would place), in light purple, and so is the angle of a turn about
//! the line, past the grabbed point (see `super::hinge`); and a brick's face size while it's
//! dragged or can be typed (see `brick_resize`), in light blue, past the face. In the editor's unit
//! for sizes, as its panel shows them: brick units, or metric or imperial
//! (`ENumericValueType::DistanceAutoOrBrickUnits`, which the panel's units button switches),
//! written by the game's own `NumberToText` (`game::ui_library`).
//!
//! Each length is a text over the 3D view (`game::view_text`), drawn by the game's UI on the
//! editor's widget, where its line shows on the screen (`ProjectWorldLocationToScreen`): so it
//! draws over the bricks like the line, and stays readable and steady while the camera turns.
//! A line's stands [`GAP`] off its middle, on its upper side (its right if the line is upright),
//! so it doesn't cover it ([`Spot::Beside`]). A face's stands [`GAP`] past the face's middle, on
//! the side it's dragged to, so it follows the face and only turns round it as the camera does
//! ([`Spot::Past`]). A length not needed is hidden rather than removed, so one that comes and goes
//! as snapping finds and loses points isn't made again each time.
//!
//! The game keeps the editor's widget for the next editor and hides it while a menu shows over the
//! editor, so the texts are taken off a canvas they leave (as `game::view_tooltip` does).
//!
//! A text can also be why a length can't be typed, or an angle ([`Segment::text`]). While a length is typed
//! (see [`super::typed_length`]), a text box (`game::view_edit`) shows instead of its text, holding
//! the length at full precision and its unit ([`edit_text`]), all selected. Enter or a click
//! elsewhere commits it (the text box loses the keyboard), Escape cancels; the keyboard then goes
//! back to the 3D view (unless a click took it). What's typed is read in the same unit
//! ([`typed_length`]).
//!
//! What it needs is looked for the first time a length shows; if the game doesn't offer it, that's
//! logged once and lines show without lengths (or without the text box: lengths can't be typed).

use std::sync::{Once, OnceLock};
use std::time::{Duration, Instant};

use brickrust::ue::coreuobject::UObjectBase;

use super::geometry::{self, Frame, Vec3};
use super::typed_length::Closed;
use crate::game::game_focus;
use crate::game::slider_text::ExactText;
use crate::game::ui_library::{self, NumberText, UnitConversion};
use crate::game::view_canvas::{self, PanelSpace, ViewCanvas};
use crate::game::view_edit::{self, ViewEdit, ViewEdits};
use crate::game::view_text::{self, Style, ViewText, ViewTexts};
use crate::game::viewport::Viewport;
use crate::game::weak_object::WeakObject;
use crate::math;
use crate::platform::keyboard;
use crate::prelude::*;

/// How lines' lengths look: light purple letters (the lines are purple) with a dark outline.
const LINE_STYLE: Style = Style {
    color: [0.8, 0.55, 1.0, 1.0],
    outline_color: [0.0, 0.0, 0.0, 1.0],
    size: 22,
    outline: 2,
};
/// How faces' sizes look: light blue letters (as the resized brick's box), a dark outline.
const FACE_STYLE: Style = Style {
    color: [0.45, 0.75, 1.0, 1.0],
    ..LINE_STYLE
};
/// How far a length stands off its anchor (the canvas's units).
const GAP: f32 = 8.0;
/// At most how many decimals a length has (trailing zeros are left out: 1.5 m stays "1.5 m").
const DECIMALS: i32 = 3;
/// The shortest length written (cm).
const SHORTEST: f64 = 1e-3;
/// How often the editor's canvas is looked for while it can't be found.
const PANEL_RETRY: Duration = Duration::from_secs(1);
/// How long a text box opened may take to get the keyboard before typing gives up.
const FOCUS_WAIT: Duration = Duration::from_secs(1);
/// Up the screen (its y goes down).
const UP: [f32; 2] = [0.0, -1.0];

static LOG_NO_EDIT: Once = Once::new();
static LOG_NO_FOCUS: Once = Once::new();

/// What lengths need from the game.
struct Kit {
    canvas: ViewCanvas,
    texts: ViewTexts,
    /// For typing lengths; without it, they can't be typed.
    edits: Option<ViewEdits>,
    numbers: NumberText,
    /// For typed lengths: the factor from cm to the shown unit.
    units: UnitConversion,
    /// `ENumericValueType::DistanceAutoOrBrickUnits`: what the size panel's rows show.
    unit: u8,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for Kit {}
unsafe impl Sync for Kit {}

static KIT: OnceLock<Option<Kit>> = OnceLock::new();

/// What lengths need, looked for once (after engine init).
fn kit() -> Option<&'static Kit> {
    KIT.get_or_init(|| {
        let found = (|| {
            Ok::<_, String>(Kit {
                canvas: view_canvas::find()?,
                texts: view_text::find()?,
                edits: view_edit::find()
                    .map_err(|reason| log!("lengths can't be typed: {reason}"))
                    .ok(),
                numbers: ui_library::number_text()?,
                units: ui_library::unit_conversion()?,
                unit: ui_library::value_type("DistanceAutoOrBrickUnits")?,
            })
        })();
        found
            .map_err(|reason| log!("line lengths won't show: {reason}"))
            .ok()
    })
    .as_ref()
}

/// What a length is of, which sets its color.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tint {
    /// A line of the line tool's: purple.
    Line,
    /// A brick's face: blue.
    Face,
}

impl Tint {
    fn index(self) -> usize {
        match self {
            Tint::Line => 0,
            Tint::Face => 1,
        }
    }

    fn style(self) -> &'static Style {
        match self {
            Tint::Line => &LINE_STYLE,
            Tint::Face => &FACE_STYLE,
        }
    }
}

/// Where a length goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Spot {
    /// Beside the middle (see [`beside`]): a line's.
    Beside,
    /// Past the second end, away from the first (see [`past`]): a face's size, the second end on
    /// the face.
    Past,
}

/// A stretch whose length shows, from one end to the other (in the vehicle).
pub struct Segment {
    pub ends: [Vec3; 2],
    /// What shows instead of its length, if anything.
    pub text: Option<String>,
    pub tint: Tint,
    pub spot: Spot,
    /// While its length is typed: what the text box opens with (see [`edit_text`]).
    pub edit: Option<String>,
}

impl Segment {
    /// A line's length (or `text`), beside its middle.
    pub fn line(ends: [Vec3; 2], text: Option<String>) -> Self {
        Self {
            ends,
            text,
            tint: Tint::Line,
            spot: Spot::Beside,
            edit: None,
        }
    }

    /// The angle of a turn about the line (`text`, see `super::hinge`), from the middle of the
    /// circle the grabbed point goes round on to the point: past the point, outside the circle.
    pub fn angle(ends: [Vec3; 2], text: String, edit: Option<String>) -> Self {
        Self {
            ends,
            text: Some(text),
            tint: Tint::Line,
            spot: Spot::Past,
            edit,
        }
    }

    /// A face's size, from the opposite side's middle to the face's: past the face.
    pub fn face(ends: [Vec3; 2], edit: Option<String>) -> Self {
        Self {
            ends,
            text: None,
            tint: Tint::Face,
            spot: Spot::Past,
            edit,
        }
    }
}

/// The length in cm `text` (typed in the shown unit, see [`super::typed_length`]) asks for, the
/// length being `now` (cm), in the unit's exact size (see [`typed_formula::cm_per_unit`]); `None`
/// if it isn't a length, or what lengths need isn't there.
///
/// # Safety
/// Game thread only.
pub unsafe fn typed_length(text: &str, now: f64) -> Option<f64> {
    let kit = kit()?;
    let cm = typed_formula::cm_per_unit(kit.units.factor(kit.unit));
    if !(cm.is_finite() && cm > 0.0) {
        return None;
    }
    // SAFETY: per the contract.
    let unit = unsafe { unit_text(kit, now) };
    let length = typed_formula::typed_length(text, &unit, now / cm)? * cm;
    (length.is_finite() && length >= 0.0).then_some(length)
}

/// What the text box opens with for the length `now` (cm): the number in the shown unit, as short
/// as it can be written while reading back as that length (as the sliders write theirs), then
/// the unit.
///
/// # Safety
/// Game thread only.
pub unsafe fn edit_text(now: f64) -> String {
    let Some(kit) = kit() else {
        return format!("{now}");
    };
    let factor = kit.units.factor(kit.unit);
    let number = ExactText::find(now as f32, factor)
        .map_or_else(|| format!("{}", now as f32 * factor), |exact| exact.text());
    // SAFETY: per the contract.
    let unit = unsafe { unit_text(kit, now) };
    format!("{number} {unit}").trim_end().to_owned()
}

/// The unit the length `now` (cm) shows in (e.g. "m"; empty in brick units): the text of its
/// number with the unit, past the text without.
///
/// # Safety
/// Game thread only.
unsafe fn unit_text(kit: &Kit, now: f64) -> String {
    // SAFETY: per the contract.
    let (with, without) = unsafe {
        (
            kit.numbers.text(now as f32, kit.unit, DECIMALS, true),
            kit.numbers.text(now as f32, kit.unit, DECIMALS, false),
        )
    };
    with.strip_prefix(&without).unwrap_or("").trim().to_owned()
}

/// Where a length goes beside a line showing from `a` to `b` on the screen (if both ends show),
/// its middle at `middle`: [`GAP`] away from the middle on the line's upper side (its right if
/// it's upright), and the point of the text put there (its alignment: 0 to 1 across and down)
/// so that the text is all on that side.
fn beside(middle: [f32; 2], ends: Option<([f32; 2], [f32; 2])>) -> ([f32; 2], [f32; 2]) {
    let away = ends
        .and_then(|(a, b)| {
            let along = [b[0] - a[0], b[1] - a[1]];
            let length = along[0].hypot(along[1]);
            // Across the line, on its upper side (the screen's y goes down).
            let mut across = [along[1] / length, -along[0] / length];
            if across[1] > 0.0 || (across[1] == 0.0 && across[0] < 0.0) {
                across = [-across[0], -across[1]];
            }
            (length > 1.0).then_some(across)
        })
        .unwrap_or(UP);
    off(middle, away)
}

/// Where a length goes past the end `b` of a line showing from `a` to `b` on the screen: [`GAP`]
/// further along it, the text all past `b` (its edge or corner facing `b`). Above `b` if the line
/// shows as a dot (seen end on). It turns round `b` smoothly as the camera does: no jumps.
fn past(a: [f32; 2], b: [f32; 2]) -> ([f32; 2], [f32; 2]) {
    let along = [b[0] - a[0], b[1] - a[1]];
    let length = along[0].hypot(along[1]);
    let away = if length > 1.0 {
        [along[0] / length, along[1] / length]
    } else {
        UP
    };
    off(b, away)
}

/// [`GAP`] from `anchor` in the direction `away` (of length 1), and the text's alignment there so
/// that it's all on that side: the text's edge or corner facing `anchor`.
fn off(anchor: [f32; 2], away: [f32; 2]) -> ([f32; 2], [f32; 2]) {
    // Scaled so the larger component is 1: a side's middle, or a corner.
    let reach = away[0].abs().max(away[1].abs());
    let alignment = away.map(|component| 0.5 - 0.5 * component / reach);
    let position = [0, 1].map(|axis| anchor[axis] + away[axis] * GAP);
    (position, alignment)
}

/// The text box while a length is typed.
struct Editing {
    tint: Tint,
    /// What it opened with: committed unchanged, it changes nothing.
    opened_with: String,
    /// Whether it got the keyboard yet, and when it was opened.
    focused: bool,
    since: Instant,
}

/// The text box asked for this frame: its tint, what it opens with, and where it goes (if its
/// spot shows).
type EditAsked<'a> = (Tint, &'a str, Option<([f32; 2], [f32; 2])>);

/// The lengths showing.
pub struct LineLabels {
    /// The canvas they're on, and our texts there (for each [`Tint`], one per length, in order).
    panel: Option<WeakObject>,
    labels: [Vec<ViewText>; 2],
    /// Our text boxes there (for each [`Tint`]), and the one being typed into.
    edits: [Option<ViewEdit>; 2],
    editing: Option<Editing>,
    /// When the canvas was last looked for in vain.
    missed: Option<Instant>,
    /// Making one failed (logged): no more tries.
    failed: bool,
}

// SAFETY: weak pointers to game objects, only used on the game thread.
unsafe impl Send for LineLabels {}

impl LineLabels {
    pub const fn new() -> Self {
        Self {
            panel: None,
            labels: [Vec::new(), Vec::new()],
            edits: [None, None],
            editing: None,
            missed: None,
            failed: false,
        }
    }

    /// Shows the length of each of `segments` (in the vehicle, at `vehicle` in the world), its
    /// text, or the text box typing it, over the view of the editor `controller` (a player
    /// controller, or null) edits with, and hides the others. With no `vehicle` or controller, or
    /// no segments, hides them all. `escape`: Escape was pressed.
    ///
    /// How the text box closed, if it did this frame (or couldn't show: cancelled).
    ///
    /// # Safety
    /// `controller` must be a live player controller or null. Game thread only.
    pub unsafe fn show(
        &mut self,
        viewport: &Viewport,
        controller: *mut UObjectBase,
        segments: &[Segment],
        vehicle: Option<&Frame>,
        escape: bool,
    ) -> Option<Closed> {
        let asks_edit = segments.iter().any(|segment| segment.edit.is_some());
        let cancelled = asks_edit.then_some(Closed::Cancelled);
        let wanted: Vec<(&Segment, f64)> = match vehicle {
            Some(_) if !controller.is_null() => segments
                .iter()
                .map(|segment| {
                    let [start, end] = segment.ends;
                    (segment, geometry::length(geometry::sub(end, start)))
                })
                .filter(|&(segment, length)| {
                    length >= SHORTEST || segment.text.is_some() || segment.edit.is_some()
                })
                .collect(),
            _ => Vec::new(),
        };
        let visible = self.labels.iter().flatten().any(ViewText::is_visible);
        if wanted.is_empty() && !visible && self.editing.is_none() && !asks_edit {
            return None;
        }
        let Some(kit) = kit() else {
            return cancelled;
        };
        // SAFETY: per the contract.
        unsafe {
            let (Some(vehicle), false) = (vehicle, wanted.is_empty()) else {
                self.hide_all(kit);
                return cancelled;
            };
            let Some(panel) = self.panel(kit, controller) else {
                return cancelled;
            };
            let Some(space) = kit.canvas.space(panel, controller) else {
                // Not laid out yet.
                self.hide_all(kit);
                return cancelled;
            };
            for labels in &mut self.labels {
                labels.retain(|label| kit.texts.is_alive(label));
            }
            // For each tint, one label per length shown (in order), each made when first needed.
            let mut used = [0; 2];
            let mut edit_asked: Option<EditAsked> = None;
            for &(segment, length) in &wanted {
                let place = spot(viewport, controller, vehicle, &space, segment);
                if let Some(opens_with) = &segment.edit {
                    edit_asked.get_or_insert((segment.tint, opens_with, place));
                    continue;
                }
                let Some((position, alignment)) = place else {
                    // Behind the camera.
                    continue;
                };
                let tint = segment.tint.index();
                if used[tint] == self.labels[tint].len() {
                    match self.add(kit, panel, segment.tint) {
                        Some(label) => self.labels[tint].push(label),
                        None => break,
                    }
                }
                let label = &mut self.labels[tint][used[tint]];
                used[tint] += 1;
                let text = match &segment.text {
                    Some(text) => text.clone(),
                    None => kit.numbers.text(length as f32, kit.unit, DECIMALS, true),
                };
                kit.texts.write(label, &text);
                kit.texts.show(&kit.canvas, label, position, alignment);
            }
            for (labels, used) in self.labels.iter_mut().zip(used) {
                for label in labels.iter_mut().skip(used) {
                    kit.texts.hide(&kit.canvas, label);
                }
            }
            match edit_asked {
                Some(asked) => self.edit(kit, panel, asked, escape),
                None => {
                    // The box was open for something no longer shown (a face forgotten): it
                    // closes as cancelled, or the keys would go on being taken for it.
                    let was_editing = self.editing.is_some();
                    self.stop_editing(kit);
                    was_editing.then_some(Closed::Cancelled)
                }
            }
        }
    }

    /// Shows the text box `asked`, opening it if it isn't yet, and tells if it closed: the
    /// keyboard left it (Enter, a click elsewhere: committed) or Escape was pressed (cancelled).
    ///
    /// # Safety
    /// `panel` must be the live canvas of the editor's widget. Game thread only.
    unsafe fn edit(
        &mut self,
        kit: &Kit,
        panel: *mut UObjectBase,
        (tint, opens_with, place): EditAsked,
        escape: bool,
    ) -> Option<Closed> {
        let Some(edits) = &kit.edits else {
            return Some(Closed::Cancelled);
        };
        // SAFETY: per the contract.
        unsafe {
            if self
                .editing
                .as_ref()
                .is_some_and(|editing| editing.tint != tint)
            {
                self.stop_editing(kit);
            }
            let slot = &mut self.edits[tint.index()];
            if slot.as_ref().is_some_and(|edit| !edits.is_alive(edit)) {
                *slot = None;
                self.editing = None;
            }
            if slot.is_none() {
                *slot = edits.add(&kit.canvas, panel, tint.style());
            }
            let Some(edit) = slot.as_mut() else {
                LOG_NO_EDIT.call_once(|| log!("lengths can't be typed: no text box could be made"));
                return Some(Closed::Cancelled);
            };
            if let Some((position, alignment)) = place {
                edits.show(&kit.canvas, edit, position, alignment);
            }
            let Some(editing) = &mut self.editing else {
                edits.write(edit, opens_with);
                edits.focus(edit);
                self.editing = Some(Editing {
                    tint,
                    opened_with: opens_with.to_owned(),
                    focused: false,
                    since: Instant::now(),
                });
                return None;
            };
            if !editing.focused {
                if edits.has_focus(edit) {
                    editing.focused = true;
                } else if editing.since.elapsed() > FOCUS_WAIT {
                    LOG_NO_FOCUS.call_once(|| {
                        log!("a length can't be typed: its text box didn't get the keyboard")
                    });
                    self.stop_editing(kit);
                    return Some(Closed::Cancelled);
                } else {
                    edits.focus(edit);
                }
                return None;
            }
            if escape {
                self.stop_editing(kit);
                return Some(Closed::Cancelled);
            }
            if edits.has_focus(edit) {
                return None;
            }
            // Committed: Enter (the text box then lets go of the keyboard) or a click elsewhere
            // (which took it).
            let text = edits.read(edit);
            let unchanged = text.trim() == editing.opened_with.trim();
            edits.hide(&kit.canvas, edit);
            self.editing = None;
            if !keyboard::is_mouse_button_down() {
                game_focus::give_to_view();
            }
            Some(if unchanged {
                Closed::Cancelled
            } else {
                Closed::Committed(text)
            })
        }
    }

    /// Closes the text box if it's open, giving the keyboard back to the 3D view if it had it.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn stop_editing(&mut self, kit: &Kit) {
        let (Some(editing), Some(edits)) = (self.editing.take(), &kit.edits) else {
            return;
        };
        if let Some(edit) = &mut self.edits[editing.tint.index()] {
            // SAFETY: per the contract.
            unsafe {
                let had_focus = edits.has_focus(edit);
                edits.hide(&kit.canvas, edit);
                if had_focus {
                    game_focus::give_to_view();
                }
            }
        }
    }

    /// Gives the keyboard to the 3D view, taking it from any widget hovered before (whose Enter
    /// would press it), if the editor's widget shows (not under a menu).
    ///
    /// # Safety
    /// `controller` must be a live player controller or null. Game thread only.
    pub unsafe fn give_keyboard_to_view(&mut self, controller: *mut UObjectBase) {
        if controller.is_null() || self.editing.is_some() {
            return;
        }
        let Some(kit) = kit() else { return };
        // SAFETY: per the contract.
        unsafe {
            if self.panel(kit, controller).is_some() {
                game_focus::give_to_view();
            }
        }
    }

    /// Hides every length, and the text box.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn hide_all(&mut self, kit: &Kit) {
        // SAFETY: per the contract.
        unsafe {
            for label in self.labels.iter_mut().flatten() {
                kit.texts.hide(&kit.canvas, label);
            }
            self.stop_editing(kit);
        }
    }

    /// The editor's canvas: found again (looking through every object, at most every
    /// [`PANEL_RETRY`]) if it changed or is hidden, our widgets taken off the old one.
    ///
    /// # Safety
    /// `controller` must be a live player controller. Game thread only.
    unsafe fn panel(
        &mut self,
        kit: &Kit,
        controller: *mut UObjectBase,
    ) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; the panels are live (`WeakObject::get`), and so are the
        // widgets removed (`ViewText::widget`, `ViewEdit::widget`).
        unsafe {
            let known = self.panel.as_ref().and_then(WeakObject::get);
            if let Some(panel) = known
                && kit.canvas.shows(panel)
            {
                return Some(panel);
            }
            self.hide_all(kit);
            if self
                .missed
                .is_some_and(|missed| missed.elapsed() < PANEL_RETRY)
            {
                return None;
            }
            let Some(panel) = kit.canvas.panel(controller) else {
                self.missed = Some(Instant::now());
                return None;
            };
            self.missed = None;
            if known != Some(panel) {
                let labels = self.labels.iter_mut().flat_map(|labels| labels.drain(..));
                let widgets: Vec<_> = labels
                    .filter_map(|label| label.widget())
                    .chain(
                        self.edits
                            .iter_mut()
                            .filter_map(|edit| edit.take()?.widget()),
                    )
                    .collect();
                for widget in widgets {
                    kit.canvas.remove_widget(widget);
                }
                self.panel = Some(WeakObject::new(panel));
            }
            Some(panel)
        }
    }

    /// A new, hidden label of `tint` on `panel`, unless making one failed before.
    ///
    /// # Safety
    /// `panel` must be a live canvas panel of an editor widget. Game thread only.
    unsafe fn add(&mut self, kit: &Kit, panel: *mut UObjectBase, tint: Tint) -> Option<ViewText> {
        if self.failed {
            return None;
        }
        // SAFETY: per the contract.
        let label = unsafe { kit.texts.add(&kit.canvas, panel, tint.style()) };
        if label.is_none() {
            log!("line lengths won't show: no text widget could be made");
            self.failed = true;
        }
        label
    }
}

/// Where the length of `segment` (in the vehicle, at `vehicle` in the world) goes on the screen of
/// `controller`'s viewport, in the canvas's units (`space`), and its alignment (see [`beside`] and
/// [`past`]); `None` if its anchor is behind the camera.
///
/// # Safety
/// `controller` must be a live player controller. Game thread only.
unsafe fn spot(
    viewport: &Viewport,
    controller: *mut UObjectBase,
    vehicle: &Frame,
    space: &PanelSpace,
    segment: &Segment,
) -> Option<([f32; 2], [f32; 2])> {
    let on_screen = |point: Vec3| {
        // SAFETY: per the contract.
        let pixel = unsafe { viewport.project(controller, math::narrow(vehicle.point(point))) }?;
        Some(space.units(pixel))
    };
    let ends = segment.ends;
    match segment.spot {
        Spot::Beside => {
            let middle = on_screen(geometry::lerp(ends, 0.5))?;
            let shown_ends = on_screen(ends[0]).zip(on_screen(ends[1]));
            Some(beside(middle, shown_ends))
        }
        Spot::Past => {
            let end = on_screen(ends[1])?;
            // The direction past the face: towards it from the box's middle, which is more often
            // in front of the camera than the opposite side.
            let middle = on_screen(geometry::lerp(ends, 0.5)).unwrap_or(end);
            Some(past(middle, end))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5
    }

    #[test]
    fn level_line_puts_the_text_above() {
        for ends in [([0.0, 50.0], [100.0, 50.0]), ([100.0, 50.0], [0.0, 50.0])] {
            let (position, alignment) = beside([50.0, 50.0], Some(ends));
            assert!(near(position, [50.0, 50.0 - GAP]));
            assert!(near(alignment, [0.5, 1.0]));
        }
    }

    #[test]
    fn upright_line_puts_the_text_right() {
        let (position, alignment) = beside([50.0, 50.0], Some(([50.0, 0.0], [50.0, 100.0])));
        assert!(near(position, [50.0 + GAP, 50.0]));
        assert!(near(alignment, [0.0, 0.5]));
    }

    #[test]
    fn slanted_line_puts_a_corner_at_the_gap() {
        // Rising to the right on the screen: the text goes up and left of it.
        let (_, alignment) = beside([50.0, 50.0], Some(([0.0, 100.0], [100.0, 0.0])));
        assert!(near(alignment, [1.0, 1.0]));
    }

    #[test]
    fn no_ends_or_a_dot_puts_the_text_above() {
        for ends in [None, Some(([50.0, 50.0], [50.2, 50.0]))] {
            let (position, alignment) = beside([50.0, 50.0], ends);
            assert!(near(position, [50.0, 50.0 - GAP]));
            assert!(near(alignment, [0.5, 1.0]));
        }
    }

    #[test]
    fn past_a_face_dragged_right_puts_the_text_right_of_it() {
        let (position, alignment) = past([0.0, 50.0], [100.0, 50.0]);
        assert!(near(position, [100.0 + GAP, 50.0]));
        assert!(near(alignment, [0.0, 0.5]));
    }

    #[test]
    fn past_a_face_dragged_down_puts_the_text_below_it() {
        let (position, alignment) = past([50.0, 0.0], [50.0, 100.0]);
        assert!(near(position, [50.0, 100.0 + GAP]));
        assert!(near(alignment, [0.5, 0.0]));
    }

    #[test]
    fn past_a_face_seen_end_on_puts_the_text_above() {
        let (position, alignment) = past([50.0, 50.0], [50.3, 50.0]);
        assert!(near(position, [50.0, 50.0 - GAP]));
        assert!(near(alignment, [0.5, 1.0]));
    }

    #[test]
    fn past_turns_smoothly() {
        // Nearly the same direction, nearly the same alignment: no jump.
        let (_, a) = past([0.0, 0.0], [100.0, 1.0]);
        let (_, b) = past([0.0, 0.0], [100.0, -1.0]);
        assert!((a[1] - b[1]).abs() < 0.02);
    }
}
