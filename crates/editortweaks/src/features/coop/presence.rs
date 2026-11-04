//! Others' selections, shown in their colors (`coop::presence`): the bricks outlined by the
//! editor's own outline, and a box around each player's selection. The local selection goes out
//! as it changes.
//!
//! The outline is the editor's own, in colors of ours: `game::outline_cases` holds the mechanism
//! (others' bricks get the material's free cases, and the material those cases' colors; outside of
//! the default mode, and as soon as the game itself uses those cases, the game gets its looks and
//! colors back). Coop outranks the shot preview there.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ::coop::ids::{CoopId, Slot};
use ::coop::message::Member;
use ::coop::presence::{self, Pace};
use ::coop::session::Session;

use super::changes::{self, Object};
use crate::game::component_bounds::{self, ComponentBounds};
use crate::game::editor_object::{self, Selection, Vector};
use crate::game::line_batcher::{self, Line, LineBatcher};
use crate::game::outline_cases::{self, Highlights, OutlineCases};

// The free cases are the ones the coop crate shares out among the players.
const _: () = assert!(presence::OUTLINED == outline_cases::CASES);

/// Whether a box is drawn around each other player's selection too: off, the outline alone looked
/// better in game (2026-10-28). Without boxes, others past the outlined few don't show.
const BOXES: bool = false;
/// How often the boxes are worked out again (others' bricks rarely move).
const BOXES_EVERY: Duration = Duration::from_millis(250);
/// Bricks of one selection measured for its box, at most.
const BOXED: usize = 1024;
/// How far a box stands off the bricks (cm), so it doesn't hide in their faces.
const BOX_MARGIN: f32 = 2.0;
/// The line layer's owner.
const LAYER: &str = "coop";

/// The game code showing selections needs (after engine init).
pub struct Kit {
    selection: Selection,
    bounds: ComponentBounds,
    lines: LineBatcher,
    cases: OutlineCases,
}

// SAFETY: reflection records and game code living as long as the game; only used on the game
// thread.
unsafe impl Send for Kit {}
unsafe impl Sync for Kit {}

pub fn find() -> Result<Kit, String> {
    Ok(Kit {
        selection: editor_object::selection()?,
        bounds: component_bounds::find()?,
        lines: line_batcher::find()?,
        cases: outline_cases::find()?,
    })
}

/// What the others selected, and what of it shows.
pub struct Shown {
    own: Option<Slot>,
    members: Vec<Member>,
    selections: BTreeMap<Slot, Vec<CoopId>>,
    pace: Pace,
    /// The others' bricks outlined in their colors.
    highlights: Highlights,
    boxes_at: Option<Instant>,
    drawn: bool,
}

impl Shown {
    pub fn new(own: Option<Slot>) -> Self {
        Self {
            own,
            members: Vec::new(),
            selections: BTreeMap::new(),
            pace: Pace::default(),
            highlights: Highlights::new("coop", 1),
            boxes_at: None,
            drawn: false,
        }
    }

    /// Our slot, once the subhost gave it.
    pub fn joined(&mut self, slot: Slot) {
        self.own = Some(slot);
    }

    /// Another editor shows the vehicle (the player left the last one): what showed there went
    /// with it (lines stay: [`Shown::hide`] clears them). Who selected what stays.
    pub fn new_editor(&mut self) {
        self.highlights.forget();
        self.boxes_at = None;
    }

    /// The session's members now (who left or came is shown on the coop page, not announced).
    pub fn members(&mut self, members: Vec<Member>) {
        let slots: Vec<Slot> = members.iter().map(|m| m.slot).collect();
        self.selections.retain(|slot, _| slots.contains(slot));
        self.members = members;
        self.boxes_at = None;
    }

    /// `slot` selected `ids`.
    pub fn selected(&mut self, slot: Slot, ids: Vec<CoopId>) {
        if Some(slot) == self.own {
            return;
        }
        if ids.is_empty() {
            self.selections.remove(&slot);
        } else {
            self.selections.insert(slot, ids);
        }
        self.boxes_at = None;
    }

    /// Sends the local selection when due, and shows the others' (if `showing`: the editor is in
    /// its default mode, else the game gets its looks back).
    ///
    /// # Safety
    /// `editor` must be the live editor, its objects those `changes` knows. Game thread only.
    pub unsafe fn tick(
        &mut self,
        kit: &Kit,
        session: &mut Session,
        editor: Object,
        now: u64,
        showing: bool,
    ) {
        let mut ids: Vec<CoopId> = changes::known()
            .into_iter()
            // SAFETY: per the contract.
            .filter(|&(object, _)| unsafe { kit.selection.is_selected(object) })
            .map(|(_, id)| id)
            .collect();
        ids.sort_unstable();
        if self.pace.due(now, &ids) {
            session.select(ids);
        }
        if !showing || self.selections.is_empty() {
            // SAFETY: game thread.
            unsafe { self.hide(kit) };
            return;
        }
        // SAFETY: per the contract.
        if !unsafe { self.outline(kit, editor) } {
            // SAFETY: game thread.
            unsafe { self.hide(kit) };
            return;
        }
        // SAFETY: per the contract.
        unsafe {
            if BOXES {
                self.draw_boxes(kit);
            }
        }
    }

    /// Outlines the outlined others' bricks in their colors; `false` if the game itself uses those
    /// cases now.
    ///
    /// # Safety
    /// As for [`Self::tick`].
    unsafe fn outline(&mut self, kit: &Kit, editor: Object) -> bool {
        let slots: Vec<Slot> = self.members.iter().map(|m| m.slot).collect();
        let outlined = presence::outlined(self.own, self.selections.keys().copied());
        let mut colors = [None; presence::OUTLINED];
        let mut wanted = Vec::new();
        for (index, &slot) in outlined.iter().enumerate() {
            colors[index] = Some(presence::color_of(&slots, slot).1);
            for &id in &self.selections[&slot] {
                let Some(object) = changes::object(id) else {
                    continue;
                };
                // SAFETY: per the contract (`changes` knows only live objects).
                let component = unsafe {
                    if kit.selection.is_selected(object) {
                        // Ours shows as the game shows it.
                        continue;
                    }
                    kit.cases.outlines().component_of(object)
                };
                if let Some(component) = component {
                    wanted.push((component, index));
                }
            }
        }
        // SAFETY: per the contract.
        unsafe { self.highlights.show(&kit.cases, editor, &wanted, colors) }
    }

    /// Draws a box around each other's selection, in their color.
    ///
    /// # Safety
    /// As for [`Self::tick`].
    unsafe fn draw_boxes(&mut self, kit: &Kit) {
        // SAFETY: game thread.
        let batcher = unsafe { kit.lines.batcher() };
        if batcher.is_null() {
            return;
        }
        if self.boxes_at.is_some_and(|at| at.elapsed() < BOXES_EVERY) {
            // SAFETY: the current world's batcher.
            unsafe { kit.lines.keep(batcher) };
            return;
        }
        self.boxes_at = Some(Instant::now());
        let slots: Vec<Slot> = self.members.iter().map(|m| m.slot).collect();
        let mut lines = Vec::new();
        for (&slot, ids) in &self.selections {
            let mut bounds: Option<[Vector; 2]> = None;
            for object in ids.iter().filter_map(|&id| changes::object(id)).take(BOXED) {
                // SAFETY: per the contract.
                let Some(component) = (unsafe { kit.cases.outlines().component_of(object) }) else {
                    continue;
                };
                // SAFETY: the component of a live object.
                let [low, high] = unsafe { kit.bounds.of(component) };
                bounds = Some(match bounds {
                    None => [low, high],
                    Some([l, h]) => [
                        std::array::from_fn(|a| l[a].min(low[a])),
                        std::array::from_fn(|a| h[a].max(high[a])),
                    ],
                });
            }
            if let Some(bounds) = bounds {
                let color = presence::color_of(&slots, slot).1.map(linear);
                lines.extend(box_lines(bounds, color));
            }
        }
        // SAFETY: as above.
        unsafe { kit.lines.replace(batcher, LAYER, &lines) };
        self.drawn = !lines.is_empty();
    }

    /// Gives the game back every look and color, and clears the boxes.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn hide(&mut self, kit: &Kit) {
        // SAFETY: per the contract.
        unsafe { self.highlights.hide(&kit.cases) };
        if self.drawn {
            // SAFETY: per the contract.
            let batcher = unsafe { kit.lines.batcher() };
            if !batcher.is_null() {
                // SAFETY: the current world's batcher.
                unsafe { kit.lines.replace(batcher, LAYER, &[]) };
            }
            self.drawn = false;
        }
        self.boxes_at = None;
    }
}

/// An sRGB channel made linear (lines are drawn in linear color).
pub(super) fn linear(channel: f32) -> f32 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

/// The twelve edges of the box `[low, high]`, a little bigger.
fn box_lines([low, high]: [Vector; 2], color: [f32; 3]) -> Vec<Line> {
    let low = low.map(|v| v - BOX_MARGIN);
    let high = high.map(|v| v + BOX_MARGIN);
    let corner = |i: usize| -> Vector {
        std::array::from_fn(|axis| {
            if i >> axis & 1 == 0 {
                low[axis]
            } else {
                high[axis]
            }
        })
    };
    let mut lines = Vec::with_capacity(12);
    for i in 0..8 {
        for axis in 0..3 {
            if i >> axis & 1 == 0 {
                lines.push(Line {
                    start: corner(i),
                    end: corner(i | 1 << axis),
                    color,
                    thickness: 0.0,
                    on_top: false,
                });
            }
        }
    }
    lines
}
