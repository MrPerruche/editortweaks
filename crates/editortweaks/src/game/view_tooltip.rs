//! A tooltip of ours over the editor's 3D view: the game's own tooltip (`game::tooltip_widget`) on
//! the canvas the editor's widget keeps over the view (`game::view_canvas`), beside a point (the
//! mouse). Used by the view modes' tooltip and the shot preview's.
//!
//! The game keeps the editor's widget for the next editor (its widget pool), and hides it while a
//! menu shows over the editor. So a tooltip is taken off a canvas it leaves (not just forgotten):
//! one left there would stay on screen in every editor after.

use std::time::{Duration, Instant};

use brickrust::ue::coreuobject::UObjectBase;

use crate::game::kismet::{self, Texts};
use crate::game::tooltip_widget::{self, Tooltip, Tooltips};
use crate::game::view_canvas::{self, ViewCanvas};
use crate::game::viewport::{self, Viewport};
use crate::game::weak_object::WeakObject;

/// How often the editor's canvas is looked for while it can't be found.
const PANEL_RETRY: Duration = Duration::from_secs(1);

/// What a tooltip over the view needs from the game (after engine init, once the game's UI is up).
pub struct ViewTooltipCode {
    pub tooltips: Tooltips,
    pub canvas: ViewCanvas,
    pub viewport: Viewport,
    pub texts: Texts,
}

pub fn find() -> Result<ViewTooltipCode, String> {
    Ok(ViewTooltipCode {
        tooltips: tooltip_widget::find()?,
        canvas: view_canvas::find()?,
        viewport: viewport::find()?,
        texts: kismet::texts()?,
    })
}

/// A tooltip over the view, between frames.
pub struct ViewTooltip {
    /// The canvas it's on, and our tooltip there.
    panel: Option<WeakObject>,
    tooltip: Option<Tooltip>,
    /// When the canvas was last looked for in vain.
    missed: Option<Instant>,
    /// What it says.
    title: String,
    lines: Vec<String>,
    /// Whether it shows.
    visible: bool,
}

// SAFETY: weak pointers to game objects, only used on the game thread.
unsafe impl Send for ViewTooltip {}

impl ViewTooltip {
    pub const fn new() -> Self {
        Self {
            panel: None,
            tooltip: None,
            missed: None,
            title: String::new(),
            lines: Vec::new(),
            visible: false,
        }
    }

    /// Whether it shows.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Gives it `title` over `lines`, shown from now on.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn set(&mut self, code: &ViewTooltipCode, title: String, lines: Vec<String>) {
        if title != self.title || lines != self.lines {
            self.title = title;
            self.lines = lines;
            // SAFETY: per the contract.
            unsafe { self.fill(code) };
        }
    }

    /// Shows it beside `point` (viewport pixels) over the view of the editor `controller` (a player
    /// controller) edits with; why it can't, if it can't.
    ///
    /// # Safety
    /// `controller` must be a live player controller. Game thread only.
    pub unsafe fn show(
        &mut self,
        code: &ViewTooltipCode,
        controller: *mut UObjectBase,
        point: [f32; 2],
    ) -> Result<(), &'static str> {
        // SAFETY: per the contract.
        unsafe {
            let panel = self.panel(code, controller)?;
            let space = code
                .canvas
                .space(panel, controller)
                .ok_or("canvas not laid out")?;
            let [width, height] = code
                .viewport
                .size(controller)
                .ok_or("no viewport size")?
                .map(|pixels| pixels as f32);
            let area = space.units([width, height]);
            let tooltip = self.tooltip.as_mut().ok_or("no tooltip widget")?;
            code.tooltips
                .place(&code.canvas, tooltip, space.units(point), area);
        }
        self.visible = true;
        Ok(())
    }

    /// Hides it.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn hide(&mut self, code: &ViewTooltipCode) {
        if !self.visible {
            return;
        }
        self.visible = false;
        if let Some(tooltip) = &mut self.tooltip {
            // SAFETY: per the contract.
            unsafe { code.tooltips.hide(&code.canvas, tooltip) };
        }
    }

    /// Gives our tooltip what it says, if it's there.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn fill(&self, code: &ViewTooltipCode) {
        if let Some(tooltip) = &self.tooltip {
            // SAFETY: per the contract; new texts, given up by the call.
            unsafe {
                code.tooltips.set_content(
                    tooltip,
                    code.texts.text(&self.title),
                    code.texts.text(&self.lines.join("\n")),
                );
            }
        }
    }

    /// The editor's canvas, with our tooltip on it: found again (looking through every object, at
    /// most every [`PANEL_RETRY`]) if it changed or is hidden, our tooltip taken off the old one.
    ///
    /// # Safety
    /// `controller` must be a live player controller. Game thread only.
    unsafe fn panel(
        &mut self,
        code: &ViewTooltipCode,
        controller: *mut UObjectBase,
    ) -> Result<*mut UObjectBase, &'static str> {
        // SAFETY: per the contract; the panels are live (`WeakObject::get`), and so is the widget
        // removed (`Tooltip::widget`).
        unsafe {
            let known = self.panel.as_ref().and_then(WeakObject::get);
            let panel = match known {
                Some(panel) if code.canvas.shows(panel) => panel,
                _ => {
                    self.hide(code);
                    if self
                        .missed
                        .is_some_and(|missed| missed.elapsed() < PANEL_RETRY)
                    {
                        return Err("no canvas");
                    }
                    let Some(panel) = code.canvas.panel(controller) else {
                        self.missed = Some(Instant::now());
                        return Err("no canvas");
                    };
                    self.missed = None;
                    if known != Some(panel) {
                        if let Some(widget) = self.tooltip.take().and_then(|old| old.widget()) {
                            code.canvas.remove_widget(widget);
                        }
                        self.panel = Some(WeakObject::new(panel));
                    }
                    panel
                }
            };
            if !self
                .tooltip
                .as_ref()
                .is_some_and(|tooltip| code.tooltips.is_alive(tooltip))
            {
                self.tooltip = Some(
                    code.tooltips
                        .add(&code.canvas, panel, controller)
                        .ok_or("no tooltip widget")?,
                );
                self.fill(code);
            }
            Ok(panel)
        }
    }
}
