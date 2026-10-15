//! The box on the screen while dragging, drawn with the game's UI over the editor's 3D view
//! (`game::view_canvas`): see-through inside, with a solid edge, in the UI style's highlight color
//! (the selection outline's). Without the editor's widget, the box is drawn as lines in the 3D view
//! instead (see `super::State::draw`).

use brickrust::ue::coreuobject::UObjectBase;

use crate::game::view_canvas::{Border, ViewCanvas};
use crate::game::weak_object::WeakObject;

/// How opaque the inside is (the color's alpha).
const FILL_ALPHA: f32 = 0.15;
/// How thick the edge is, in UI units (pixels at 100% UI scale).
const EDGE: f32 = 2.0;

/// The borders making the box, kept between drags: its inside, then its four edges.
pub struct Marquee {
    /// The canvas they're on.
    panel: Option<WeakObject>,
    borders: Vec<Border>,
    /// The color they show.
    color: [f32; 4],
}

impl Marquee {
    pub const fn new() -> Self {
        Self {
            panel: None,
            borders: Vec::new(),
            color: [0.0; 4],
        }
    }

    /// Gets ready to show a box over the editor `controller` (a player controller) edits with, in
    /// `color` (linear): finds the editor's canvas again if it changed (looking through every
    /// object), and makes the borders on it. Whether the box can show.
    ///
    /// # Safety
    /// `controller` must be a live player controller. Game thread only.
    pub unsafe fn begin(
        &mut self,
        canvas: &ViewCanvas,
        controller: *mut UObjectBase,
        color: [f32; 3],
    ) -> bool {
        // SAFETY: per the contract; the panel is live (`WeakObject::get`).
        unsafe {
            let kept = self
                .panel
                .as_ref()
                .and_then(WeakObject::get)
                .filter(|&panel| canvas.shows(panel));
            let panel = match kept {
                Some(panel) => panel,
                None => {
                    // A new editor widget: the old borders go with the old one.
                    self.panel = None;
                    self.borders.clear();
                    let Some(panel) = canvas.panel(controller) else {
                        return false;
                    };
                    self.panel = Some(WeakObject::new(panel));
                    panel
                }
            };
            if self.borders.len() != 5 || !self.borders.iter().all(|b| canvas.is_alive(b)) {
                self.borders = (0..5)
                    .map_while(|_| canvas.add_border(panel, [0.0; 4]))
                    .collect();
                self.color = [0.0; 4];
                if self.borders.len() != 5 {
                    return false;
                }
            }
            let [red, green, blue] = color;
            if self.color != [red, green, blue, 1.0] {
                self.color = [red, green, blue, 1.0];
                for (index, border) in self.borders.iter().enumerate() {
                    let alpha = if index == 0 { FILL_ALPHA } else { 1.0 };
                    canvas.set_color(border, [red, green, blue, alpha]);
                }
            }
        }
        true
    }

    /// Shows the box from `start` to `end` (viewport pixels); false if it can't (see
    /// [`Self::begin`]).
    ///
    /// # Safety
    /// `controller` must be a live player controller. Game thread only.
    pub unsafe fn show(
        &self,
        canvas: &ViewCanvas,
        controller: *mut UObjectBase,
        start: [f32; 2],
        end: [f32; 2],
    ) -> bool {
        let Some(panel) = self.panel.as_ref().and_then(WeakObject::get) else {
            return false;
        };
        if self.borders.len() != 5 {
            return false;
        }
        // SAFETY: per the contract; the panel is live (`WeakObject::get`).
        let Some(space) = (unsafe { canvas.space(panel, controller) }) else {
            return false;
        };
        let [a, b] = [start, end].map(|pixel| space.units(pixel));
        let (left, top) = (a[0].min(b[0]), a[1].min(b[1]));
        let (width, height) = ((a[0] - b[0]).abs(), (a[1] - b[1]).abs());
        let edge_x = EDGE.min(width / 2.0);
        let edge_y = EDGE.min(height / 2.0);
        let inner = (height - 2.0 * edge_y).max(0.0);
        let places = [
            ([left, top], [width, height]),
            ([left, top], [width, edge_y]),
            ([left, top + height - edge_y], [width, edge_y]),
            ([left, top + edge_y], [edge_x, inner]),
            ([left + width - edge_x, top + edge_y], [edge_x, inner]),
        ];
        for (border, (position, size)) in self.borders.iter().zip(places) {
            // SAFETY: per the contract.
            unsafe { canvas.place(border, position, size) };
        }
        true
    }

    /// Hides the box.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn hide(&self, canvas: &ViewCanvas) {
        for border in &self.borders {
            // SAFETY: per the contract.
            unsafe { canvas.hide(border) };
        }
    }
}
