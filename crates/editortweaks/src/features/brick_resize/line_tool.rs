//! The line moved bricks turn about or slide along (see [`super::hinge`]), placed by two points
//! while moving bricks with the mouse free (the gizmo showing): with the resize key held, a click
//! of the line key ([`crate::features::keybinds::LINE_POINT`], the right mouse button by default)
//! places a point where snapping finds one under the mouse (on any brick, moved ones too: see
//! [`super::snap`]); the second point makes the line, replacing the last one. A click on the first
//! point again takes it back; one over no brick clears the line and a first point, and so does a
//! press of the clear key ([`crate::features::keybinds::CLEAR_LINE`], none by default).
//!
//! Only a click counts: a press that moves the mouse further than a few pixels places nothing
//! (started on a moved brick with a line placed, it slides the bricks along the line: see
//! [`LineTool::is_dragging`]). On the right mouse button (the game's camera button), a press is the
//! line key's while a point can be placed, so the camera doesn't start moving (and capture the
//! mouse) under it: it moves once the resize key is let go (to bring the second point into view,
//! say). The line stays where it was placed in the vehicle, shows while moving with the mouse free,
//! and lasts until cleared or replaced, or less long (a setting, see [`LINE_LASTS`]).

use std::sync::Once;

use super::draw::{self, Drawing, Mark};
use super::geometry::{self, Frame, Vec3};
use super::hinge::Line;
use super::scene::{Scene, View};
use super::snap::{self, Snapper};
use crate::framework::settings::{Alternative, Choice, Setting};
use crate::prelude::*;

/// How long a line lasts, a setting ([`SETTINGS`]): the options of [`LineLasts`], in order.
static LINE_LASTS: Choice = Choice::new(
    "LineLasts",
    "Line lasts",
    "How long a line placed to move bricks about lasts. Cleared or replaced, it's gone in any \
     case.",
    &[
        Alternative {
            key: "UntilCleared",
            name: "Until cleared",
        },
        Alternative {
            key: "UntilMoveEnds",
            name: "Until the move ends",
        },
        Alternative {
            key: "ForOneDrag",
            name: "For one drag",
        },
    ],
    0,
);

/// The settings of lines, in the menu's order.
pub static SETTINGS: [Setting; 1] = [Setting::Choice(&LINE_LASTS)];

/// How long a line lasts now.
fn line_lasts() -> LineLasts {
    match LINE_LASTS.get() {
        1 => LineLasts::UntilMoveEnds,
        2 => LineLasts::ForOneDrag,
        _ => LineLasts::UntilCleared,
    }
}

/// How long a line lasts (see [`LINE_LASTS`]). Cleared or replaced, it's gone in any case.
#[derive(PartialEq, Eq)]
enum LineLasts {
    /// Until cleared or replaced: it serves move after move.
    UntilCleared,
    /// Until the move it was placed in ends (the bricks placed, or the move cancelled), with a
    /// first point.
    UntilMoveEnds,
    /// For one drag of the bricks about or along it, or until its move ends if sooner.
    ForOneDrag,
}

/// Logs the first line placed.
static LOG_PLACED: Once = Once::new();

/// How far the mouse may move (pixels) while the line key is down for its release to still be a
/// click.
const CLICK_PIXELS: f32 = 6.0;

/// How close (cm) a point placed may be to the first one to take it back.
const SAME_POINT: f64 = 1e-3;

/// How far past its points the line shows (cm each way).
const SHOWN_PAST: f64 = 5000.0;

/// How wide its points' marks are (pixels), and the first point's while the second is awaited.
const END_MARK_PIXELS: f32 = 12.0;
const FIRST_MARK_PIXELS: f32 = 16.0;

/// The line, its points, and the line key's clicks.
pub struct LineTool {
    /// The line's two points (in the vehicle), once both are placed.
    ends: Option<[Vec3; 2]>,
    /// The first point of the next line, while its second isn't placed.
    first: Option<Vec3>,
    /// Where the vehicle was when a point was last placed: to show them without bricks gathered.
    vehicle: Option<Frame>,
    /// What a click would place, with the mouse where it is (while that can be done). Made when
    /// first needed: the tool lives in a static, made at compile time.
    snapper: Option<Snapper>,
    /// A press of the line key that may turn out to be a click.
    press: Option<Press>,
    /// Whether the line key was down last frame, and whether a press could place a point then.
    held: bool,
    ready: bool,
    /// Whether the editor was in a move last frame.
    in_move: bool,
    /// Whether the clear key was down last frame.
    clear_held: bool,
    /// Whether the right mouse button's press (the game's camera action) went to the line key,
    /// until its release.
    took_button: bool,
}

/// A press of the line key: what it would place (nothing: clear), where the mouse was, how far the
/// cursor has gone from there at most, and the mouse's movement the engine reported meanwhile (it
/// may hold the cursor still), all in pixels.
struct Press {
    pick: Option<Vec3>,
    at: Option<[f32; 2]>,
    moved: f32,
    reported: [f32; 2],
}

impl Press {
    /// Whether the mouse stayed close enough for a click.
    fn is_click(&self) -> bool {
        self.moved.max(self.reported[0].hypot(self.reported[1])) <= CLICK_PIXELS
    }
}

impl LineTool {
    pub const fn new() -> Self {
        Self {
            ends: None,
            first: None,
            vehicle: None,
            snapper: None,
            press: None,
            held: false,
            ready: false,
            in_move: false,
            clear_held: false,
            took_button: false,
        }
    }

    /// The line, if both its points are placed.
    pub fn line(&self) -> Option<Line> {
        let [start, end] = self.ends?;
        Line::through(start, end)
    }

    /// Whether there's nothing to show or follow: no line, no first point, no press.
    pub fn is_idle(&self) -> bool {
        self.ends.is_none() && self.first.is_none() && self.press.is_none()
    }

    /// Where the vehicle was when a point was last placed.
    pub fn vehicle(&self) -> Option<Frame> {
        self.vehicle
    }

    /// The bricks were gathered again: what was found among them is gone (they're numbered anew).
    pub fn forget_bricks(&mut self) {
        self.snapper = None;
    }

    /// Whether the editor is `in_move` now: a move ending takes the line and a first point with it,
    /// unless lines last until cleared ([`LINE_LASTS`]).
    pub fn in_move(&mut self, in_move: bool) {
        if self.in_move && !in_move && line_lasts() != LineLasts::UntilCleared {
            self.ends = None;
            self.first = None;
        }
        self.in_move = in_move;
    }

    /// A drag moved bricks about or along the line: it takes the line with it if lines last for
    /// one ([`LINE_LASTS`]).
    pub fn drag_done(&mut self) {
        if line_lasts() == LineLasts::ForOneDrag {
            self.ends = None;
        }
    }

    /// Follows the clear key, `held` or not this frame: a press clears the line and a first point.
    pub fn clear_key(&mut self, held: bool) {
        if held && !self.clear_held {
            self.clear();
        }
        self.clear_held = held;
    }

    /// Finds what a click would place with the mouse at `view` among `scene`'s bricks (their
    /// models with `models`), to show it.
    pub fn preview(&mut self, scene: &Scene, view: &View, models: bool) {
        let snapper = self.snapper.get_or_insert_with(Snapper::default);
        snapper.snap(&snap::snapping(), scene, view, models, |_| false);
    }

    /// Shows nothing a click would place (it can't be done now).
    pub fn stop_preview(&mut self) {
        if let Some(snapper) = &mut self.snapper {
            snapper.stop();
        }
    }

    /// Follows the line key, `held` or not this frame: a press while a point `can_be_placed`
    /// starts a click (with the mouse at `mouse`), its release places what showed before the press
    /// if the mouse hardly moved meanwhile; `vehicle` is where the vehicle is now. Called before
    /// this frame's [`Self::preview`]: the press may hide the cursor. Whether a press started.
    pub fn key(
        &mut self,
        held: bool,
        can_be_placed: bool,
        mouse: Option<[f32; 2]>,
        vehicle: Option<Frame>,
    ) -> bool {
        // Only a press seen going down while a point could be placed the frame before counts: not
        // a key already down when the resize key was pressed (a camera move going on).
        let started = held && !self.held && can_be_placed && self.ready;
        if started {
            let pick = self
                .snapper
                .as_ref()
                .and_then(Snapper::snapped)
                .map(|snapped| snapped.target.point);
            self.press = Some(Press {
                pick,
                at: mouse,
                moved: 0.0,
                reported: [0.0; 2],
            });
        }
        if let Some(press) = &mut self.press
            && let (Some(at), Some(mouse)) = (press.at, mouse)
        {
            let apart = (mouse[0] - at[0]).hypot(mouse[1] - at[1]);
            press.moved = press.moved.max(apart);
        }
        if !held
            && let Some(press) = self.press.take()
            && press.is_click()
        {
            self.place(press.pick, vehicle);
        }
        self.held = held;
        self.ready = can_be_placed;
        started
    }

    /// Whether the line key is down after a press that moved too far to be a click: a drag, which
    /// slides the bricks along the line when started on one of them (`brick_resize`).
    pub fn is_dragging(&self) -> bool {
        self.press.as_ref().is_some_and(|press| !press.is_click())
    }

    /// The engine reported the mouse moving by `pixels` (right, down), as it does while a button is
    /// held: a press that moves too far isn't a click.
    pub fn mouse_moved(&mut self, pixels: [f32; 2]) {
        if let Some(press) = &mut self.press {
            press.reported[0] += pixels[0];
            press.reported[1] += pixels[1];
        }
    }

    /// The right mouse button went down (`pressed`) or up, the game's camera button: its press is
    /// the line key's if `line_key` (the line key is down with it, while a point can be placed),
    /// and so is its release. Whether this one is.
    pub fn right_button(&mut self, pressed: bool, line_key: bool) -> bool {
        if pressed {
            self.took_button = line_key;
            line_key
        } else {
            core::mem::take(&mut self.took_button)
        }
    }

    /// Whether the right mouse button is down as the line key (see [`Self::right_button`]): the
    /// mouse's movement then neither moves the camera nor the bricks.
    pub fn took_button(&self) -> bool {
        self.took_button
    }

    /// Forgets a press of the line key (a drag started meanwhile).
    pub fn cancel_press(&mut self) {
        self.press = None;
    }

    /// Clears the line and a first point.
    fn clear(&mut self) {
        if self.ends.is_some() || self.first.is_some() {
            log!("line cleared");
        }
        self.ends = None;
        self.first = None;
    }

    /// Places `pick` (nothing: clears), with the vehicle at `vehicle`.
    fn place(&mut self, pick: Option<Vec3>, vehicle: Option<Frame>) {
        let Some(point) = pick else {
            self.clear();
            return;
        };
        if vehicle.is_some() {
            self.vehicle = vehicle;
        }
        match self.first.take() {
            Some(first) if geometry::length(geometry::sub(point, first)) < SAME_POINT => {}
            Some(first) => {
                self.ends = Some([first, point]);
                LOG_PLACED.call_once(|| {
                    log!(
                        "line placed: {:.1} cm long (move bricks about it by fast editing them)",
                        geometry::length(geometry::sub(point, first))
                    )
                });
            }
            None => self.first = Some(point),
        }
    }

    /// Shows the line and its points, a first point, and what a click would place (with a dashed
    /// line from a first point to it).
    pub fn draw(&self, drawing: &mut Drawing) {
        if let (Some([start, end]), Some(line)) = (self.ends, self.line()) {
            let past = geometry::scale(line.direction, SHOWN_PAST);
            drawing.segment(
                geometry::sub(start, past),
                geometry::add(end, past),
                draw::LINE_PAST,
            );
            drawing.segment(start, end, draw::LINE);
            for point in [start, end] {
                drawing.mark(Mark::Circle, point, END_MARK_PIXELS, draw::LINE);
            }
        }
        let shown = self.snapper.as_ref().and_then(Snapper::snapped);
        if let Some(first) = self.first {
            drawing.mark(Mark::Circle, first, FIRST_MARK_PIXELS, draw::LINE);
            if let Some(shown) = shown {
                drawing.dashed(first, shown.target.point, draw::LINE);
            }
        }
        if let Some(shown) = shown {
            let point = shown.target.point;
            match (shown.kind.mark(), shown.target.normal) {
                (Some(mark), _) => drawing.mark(mark, point, draw::MARK_PIXELS, draw::LINE),
                (None, Some(normal)) => {
                    drawing.flat_mark(point, normal, draw::MARK_PIXELS, draw::LINE)
                }
                (None, None) => drawing.mark(Mark::Square, point, draw::MARK_PIXELS, draw::LINE),
            }
        }
    }
}
