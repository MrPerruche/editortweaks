//! Others' cameras, shown in their colors (`coop::presence`): a small pyramid where each one looks
//! from, pointing where it looks, its little roof up, drawn with the line batcher (hidden behind
//! bricks unless [`THROUGH_BRICKS`]). The local camera goes out as it moves.
//!
//! Views travel in the editor's space, the bricks' space, the same for everyone wherever their
//! editor stands or however it's turned (`features/map_in_editor`). The local camera's place is
//! the editor's own camera location, already in that space (`game::editor_camera`; the
//! orthographic view moves only the view made from it); its direction is the camera manager's
//! rotation, in the world's space, brought into the editor's.
//!
//! Each marker also has its player's name above it, a text render component of ours (`game::text_actor`)
//! on the editor's actor, the only one whose components the editor shows, in their color, turned to face the local camera, spawned with the marker and destroyed with it.
//! If the game doesn't offer what those need (logged at start), the markers show without names.
//!
//! Both have a dark outline, so they read on any brick or sky: a marker's lines are thick ones over
//! thicker dark ones, about as wide on screen from any distance; a name stands over dark copies of
//! itself, a little off in each direction. (A text render component has no outline of its own.)

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::time::Instant;

use ::coop::ids::Slot;
use ::coop::message::Member;
use ::coop::presence::{self, View, ViewPace};
use ::coop::session::Session;
use brickrust::ue::coreuobject::UObjectBase;

use super::changes::Object;
use super::log::coop_log;
use super::presence::linear;
use crate::framework::settings::Toggle;
use crate::game::actor::{self, Actors};
use crate::game::brick_editor::{self, BrickEditor};
use crate::game::editor_camera;
use crate::game::editor_object::Vector;
use crate::game::line_batcher::{self, Line, LineBatcher};
use crate::game::reflection::{self, Getter};
use crate::game::text_actor::{self, TextActors, TextLabel};
use crate::math::{self, Quat, Vec3};

/// The line layer's owner.
const LAYER: &str = "coop cameras";
/// How far a marker's pyramid reaches ahead of its camera, and its base's half width and half
/// height (cm).
const DEPTH: f64 = 60.0;
const HALF_WIDTH: f64 = 40.0;
const HALF_HEIGHT: f64 = 25.0;
/// How many lines a marker has.
const MARKER_EDGES: usize = 10;
/// A name's letters' height, and the gap between the top of its marker and the name (cm).
const LABEL_SIZE: f32 = 14.0;
const LABEL_GAP: f64 = 10.0;
/// How wide a marker's lines and their outline are on screen (pixels, about: see
/// [`CM_PER_PIXEL`]).
const LINE_PIXELS: f64 = 2.0;
const LINE_OUTLINE_PIXELS: f64 = 5.0;
/// How wide a pixel is at one centimeter's distance (cm), at the usual field of view on a screen
/// 1920 pixels wide. Near enough for an outline.
const CM_PER_PIXEL: f64 = 1.0 / 960.0;
/// How wide lines may be (cm), whatever the distance: the orthographic view stands far back.
const MAX_LINE_WIDTH: f64 = 8.0;
/// The outline's color.
const OUTLINE: [f32; 3] = [0.0, 0.0, 0.0];
/// How far a name's dark copies are off it, across and up (cm), and behind it.
const LABEL_OUTLINE: f64 = 0.8;
const LABEL_OUTLINE_BEHIND: f64 = 0.5;
/// Which ways a name's dark copies are off it.
const LABEL_OUTLINE_WAYS: [[f64; 2]; 4] = [[-1.0, -1.0], [1.0, -1.0], [-1.0, 1.0], [1.0, 1.0]];

pub static THROUGH_BRICKS: Toggle = Toggle::new(
    "CoopCamerasThroughBricks",
    "Others' cameras through bricks",
    "Other players' camera markers show through bricks. Off: bricks hide them.",
    false,
);

/// The game code showing cameras needs (after engine init).
pub struct Kit {
    actors: Actors,
    editor: BrickEditor,
    lines: LineBatcher,
    /// Offset of the editor's camera location (relative to the editor, in its space).
    camera_location: usize,
    /// Offset of `APlayerController::PlayerCameraManager`.
    camera_manager: usize,
    /// `FRotator APlayerCameraManager::GetCameraRotation() const`.
    camera_rotation: Getter<Vector>,
    /// `AActor* AController::GetViewTarget() const` (the player controller's: its camera
    /// manager's; the camera manager's own isn't reflected).
    view_target: Getter<*mut UObjectBase>,
    /// What shows names, if the game has it.
    labels: Option<LabelKit>,
}

/// What names need besides the markers' code.
struct LabelKit {
    text: TextActors,
    /// `FVector APlayerCameraManager::GetCameraLocation() const`: where the local view is, in the
    /// world.
    viewer: Getter<Vector>,
}

// SAFETY: reflection records and game code living as long as the game; only used on the game
// thread.
unsafe impl Send for Kit {}
unsafe impl Sync for Kit {}

pub fn find() -> Result<Kit, String> {
    let controller =
        reflection::find_class("PlayerController").ok_or("class PlayerController not found")?;
    let manager = reflection::find_class("PlayerCameraManager")
        .ok_or("class PlayerCameraManager not found")?;
    let labels = text_actor::find()
        .and_then(|text| {
            Ok(LabelKit {
                text,
                viewer: Getter::find(manager, "GetCameraLocation", "StructProperty")?,
            })
        })
        .inspect_err(|reason| coop_log!("others' camera names won't show: {reason}"))
        .ok();
    Ok(Kit {
        actors: actor::find()?,
        editor: brick_editor::find()?,
        lines: line_batcher::find()?,
        camera_location: editor_camera::find(editor_camera::code()?)?.camera_location(),
        camera_manager: reflection::field_offset(
            controller,
            "PlayerCameraManager",
            "ObjectProperty",
            8,
        )?,
        camera_rotation: Getter::find(manager, "GetCameraRotation", "StructProperty")?,
        view_target: Getter::find(controller, "GetViewTarget", "ObjectProperty")?,
        labels,
    })
}

impl Kit {
    /// Where the editor stands and how it's turned.
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    unsafe fn frame(&self, editor: Object) -> (Vec3, Quat) {
        // SAFETY: per the contract.
        unsafe {
            (
                math::widen(self.actors.location(editor)),
                Quat::from_rotator(math::widen(self.actors.rotation(editor))),
            )
        }
    }

    /// The camera manager of the player looking through the editor, if any.
    ///
    /// # Safety
    /// As for [`Self::frame`].
    unsafe fn own_manager(&self, editor: Object) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; the offset comes from reflection, and the controller's camera
        // manager is a live object or null.
        unsafe {
            let controller = self.editor.player_controller(editor);
            if controller.is_null() {
                return None;
            }
            let manager = controller
                .cast::<u8>()
                .add(self.camera_manager)
                .cast::<*mut UObjectBase>()
                .read();
            (!manager.is_null() && self.view_target.call(controller) == editor).then_some(manager)
        }
    }

    /// Where the local camera is in the world, if the player looks through the editor and names
    /// can show.
    ///
    /// # Safety
    /// As for [`Self::frame`].
    unsafe fn viewer(&self, editor: Object) -> Option<Vector> {
        let labels = self.labels.as_ref()?;
        // SAFETY: per the contract; the manager is live.
        unsafe { Some(labels.viewer.call(self.own_manager(editor)?)) }
    }

    /// The local camera in the editor's space, if the player looks through the editor.
    ///
    /// # Safety
    /// As for [`Self::frame`].
    unsafe fn own_view(&self, editor: Object) -> Option<View> {
        // SAFETY: per the contract; the offsets come from reflection and the game's code.
        unsafe {
            let manager = self.own_manager(editor)?;
            let location: Vector = editor
                .cast::<u8>()
                .add(self.camera_location)
                .cast::<Vector>()
                .read_unaligned();
            let (_, turn) = self.frame(editor);
            let rotation = Quat::from_rotator(math::widen(self.camera_rotation.call(manager)));
            let forward = turn.inverse().rotate(rotation.rotate([1.0, 0.0, 0.0]));
            Some(View {
                location,
                forward: math::narrow(forward),
            })
            .filter(View::is_sane)
        }
    }
}

/// Others' cameras, and sending ours.
pub struct Cameras {
    own: Option<Slot>,
    /// Every member's slot (for their colors).
    slots: Vec<Slot>,
    /// Per other player: the view shown, and the last one they sent.
    views: BTreeMap<Slot, (View, View)>,
    pace: ViewPace,
    /// The last tick, for gliding.
    ticked: Option<Instant>,
    drawn: bool,
    /// Every member's name.
    names: BTreeMap<Slot, String>,
    /// The names showing, per other player.
    labels: BTreeMap<Slot, Label>,
    /// Spawning a name failed (logged): no more tries.
    labels_failed: bool,
    /// The first name made was logged.
    label_logged: bool,
}

/// A name showing, and what it was last given.
struct Label {
    handle: TextLabel,
    /// Its dark copies, one per [`LABEL_OUTLINE_WAYS`] (fewer if some couldn't be made).
    outline: Vec<TextLabel>,
    name: String,
    /// Linear red, green, blue.
    color: [f32; 3],
    /// Where it was last put (location, rotation).
    placed: Option<(Vector, Vector)>,
}

impl Label {
    /// Removes the name and its outline.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn destroy(&self, text: &TextActors) {
        // SAFETY: per the contract.
        unsafe {
            text.destroy(&self.handle);
            for dark in &self.outline {
                text.destroy(dark);
            }
        }
    }
}

impl Cameras {
    pub fn new(own: Option<Slot>) -> Self {
        Self {
            own,
            slots: Vec::new(),
            views: BTreeMap::new(),
            pace: ViewPace::default(),
            ticked: None,
            drawn: false,
            names: BTreeMap::new(),
            labels: BTreeMap::new(),
            labels_failed: false,
            label_logged: false,
        }
    }

    /// Our slot, once the subhost gave it.
    pub fn joined(&mut self, slot: Slot) {
        self.own = Some(slot);
    }

    /// Another editor shows the vehicle (the player left the last one): the names showing went
    /// with it (lines stay: [`Cameras::hide`] clears them). Where the others look stays.
    pub fn new_editor(&mut self) {
        self.labels.clear();
        self.labels_failed = false;
        self.ticked = None;
    }

    /// The session's members now.
    pub fn members(&mut self, members: &[Member]) {
        self.slots = members.iter().map(|m| m.slot).collect();
        self.names = members.iter().map(|m| (m.slot, m.name.clone())).collect();
        self.views.retain(|slot, _| self.slots.contains(slot));
    }

    /// `slot`'s camera is at `view` now (`None`: it doesn't look through its editor).
    pub fn seen(&mut self, slot: Slot, view: Option<View>) {
        if Some(slot) == self.own {
            return;
        }
        match view {
            Some(view) => {
                self.views
                    .entry(slot)
                    .and_modify(|(_, sent)| *sent = view)
                    .or_insert((view, view));
            }
            None => {
                self.views.remove(&slot);
            }
        }
    }

    /// Sends the local camera when due, and shows the others'.
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    pub unsafe fn tick(&mut self, kit: &Kit, session: &mut Session, editor: Object, now: u64) {
        // SAFETY: per the contract.
        let view = unsafe { kit.own_view(editor) };
        if self.pace.due(now, view) {
            session.show_view(view);
        }
        let seconds = self
            .ticked
            .map_or(0.0, |at| at.elapsed().as_secs_f32().min(1.0));
        self.ticked = Some(Instant::now());
        for (shown, sent) in self.views.values_mut() {
            *shown = presence::glide(*shown, *sent, seconds);
        }
        // SAFETY: game thread.
        let batcher = unsafe { kit.lines.batcher() };
        if batcher.is_null() {
            // No markers can show, so no names.
            // SAFETY: per the contract.
            unsafe { self.hide_labels(kit) };
            return;
        }
        if self.views.is_empty() && !self.drawn && self.labels.is_empty() {
            return;
        }
        // SAFETY: per the contract.
        let (origin, turn) = unsafe { kit.frame(editor) };
        // SAFETY: per the contract.
        let viewer = unsafe { kit.viewer(editor) }.map(math::widen);
        let mut lines = Vec::new();
        // Where each name goes, in the editor's space.
        let mut anchors = Vec::new();
        for (&slot, (shown, _)) in &self.views {
            let color = presence::color_of(&self.slots, slot).1.map(linear);
            let edges = marker(*shown);
            anchors.push((slot, label_anchor(&edges)));
            let in_world = edges.map(|[start, end]| {
                [
                    math::add(origin, turn.rotate(start)),
                    math::add(origin, turn.rotate(end)),
                ]
            });
            lines.extend(outlined_lines(&in_world, viewer, color));
        }
        // SAFETY: the current world's batcher.
        unsafe { kit.lines.replace(batcher, LAYER, &lines) };
        self.drawn = !lines.is_empty();
        // SAFETY: per the contract.
        unsafe { self.show_labels(kit, editor, origin, turn, &anchors) };
    }

    /// Shows each of `anchors` (a player's slot and the place of their name in the editor's
    /// space) as a name facing the local camera, and removes the other names.
    ///
    /// # Safety
    /// As for [`Self::tick`].
    unsafe fn show_labels(
        &mut self,
        kit: &Kit,
        editor: Object,
        origin: Vec3,
        turn: Quat,
        anchors: &[(Slot, Vec3)],
    ) {
        let Some(labels) = &kit.labels else { return };
        self.labels.retain(|slot, label| {
            let keep = label.handle.is_alive() && anchors.iter().any(|(shown, _)| shown == slot);
            if !keep {
                // SAFETY: per the contract.
                unsafe { label.destroy(&labels.text) };
            }
            keep
        });
        if self.labels_failed {
            return;
        }
        // SAFETY: per the contract.
        let viewer = unsafe { kit.viewer(editor) };
        for &(slot, anchor) in anchors {
            let at = math::narrow(math::add(origin, turn.rotate(anchor)));
            let name = self.names.get(&slot).cloned().unwrap_or_default();
            let color = presence::color_of(&self.slots, slot).1.map(linear);
            let label = match self.labels.entry(slot) {
                Entry::Occupied(label) => label.into_mut(),
                Entry::Vacant(vacant) => {
                    // SAFETY: per the contract; the editor is a live actor, which owns the label.
                    match unsafe { labels.text.spawn(editor, &name, color, LABEL_SIZE) } {
                        Ok(handle) => {
                            if !self.label_logged {
                                coop_log!("camera name label made for {name}");
                                self.label_logged = true;
                            }
                            // Without its outline a name still shows.
                            // SAFETY: as above.
                            let outline = LABEL_OUTLINE_WAYS
                                .iter()
                                .filter_map(|_| unsafe {
                                    labels.text.spawn(editor, &name, OUTLINE, LABEL_SIZE).ok()
                                })
                                .collect();
                            vacant.insert(Label {
                                handle,
                                outline,
                                name: name.clone(),
                                color,
                                placed: None,
                            })
                        }
                        Err(reason) => {
                            coop_log!("others' camera names won't show: {reason}");
                            self.labels_failed = true;
                            return;
                        }
                    }
                }
            };
            // SAFETY: per the contract; the label is alive (checked above or just made).
            unsafe {
                if label.name != name {
                    labels.text.set_text(&label.handle, &name);
                    for dark in &label.outline {
                        labels.text.set_text(dark, &name);
                    }
                    label.name = name;
                }
                if label.color != color {
                    labels.text.set_color(&label.handle, color);
                    label.color = color;
                }
                // Turned to the local camera; as it was if that's not looking through the editor.
                let rotation = viewer
                    .and_then(|viewer| text_actor::facing(at, viewer))
                    .or(label.placed.map(|(_, rotation)| rotation))
                    .unwrap_or_default();
                if label.placed != Some((at, rotation)) {
                    labels.text.place(&label.handle, at, rotation);
                    for (dark, way) in label.outline.iter().zip(LABEL_OUTLINE_WAYS) {
                        labels
                            .text
                            .place(dark, outline_place(at, rotation, way), rotation);
                    }
                    label.placed = Some((at, rotation));
                }
            }
        }
    }

    /// Removes every name.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn hide_labels(&mut self, kit: &Kit) {
        if let Some(labels) = &kit.labels {
            for label in self.labels.values() {
                // SAFETY: per the contract.
                unsafe { label.destroy(&labels.text) };
            }
        }
        self.labels.clear();
    }

    /// Clears the markers and their names.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn hide(&mut self, kit: &Kit) {
        // SAFETY: per the contract.
        unsafe { self.hide_labels(kit) };
        if self.drawn {
            // SAFETY: per the contract.
            let batcher = unsafe { kit.lines.batcher() };
            if !batcher.is_null() {
                // SAFETY: the current world's batcher.
                unsafe { kit.lines.replace(batcher, LAYER, &[]) };
            }
            self.drawn = false;
        }
    }
}

/// Where a name's dark copy goes (`way`: across and up) for a name at `at` turned to `rotation`
/// (its X axis toward the viewer, Y across, Z up): a little behind it, seen from the viewer.
fn outline_place(at: Vector, rotation: Vector, way: [f64; 2]) -> Vector {
    let turn = Quat::from_rotator(math::widen(rotation));
    let off = [
        -LABEL_OUTLINE_BEHIND,
        way[0] * LABEL_OUTLINE,
        way[1] * LABEL_OUTLINE,
    ];
    math::narrow(math::add(math::widen(at), turn.rotate(off)))
}

/// A marker's lines, each of `edges` given in the world: over dark ones if the local camera is known
/// (`viewer`), about as wide on screen from any distance; without it, one pixel wide. The dark ones
/// come first and stand a little further from the viewer, so they stay behind.
fn outlined_lines(edges: &[[Vec3; 2]], viewer: Option<Vec3>, color: [f32; 3]) -> Vec<Line> {
    let on_top = THROUGH_BRICKS.get();
    let line = |[start, end]: [Vec3; 2], color, thickness: f64| Line {
        start: math::narrow(start),
        end: math::narrow(end),
        color,
        thickness: thickness as f32,
        on_top,
    };
    let Some(viewer) = viewer else {
        return edges.iter().map(|&edge| line(edge, color, 0.0)).collect();
    };
    let width_at = |point: Vec3, pixels: f64| {
        let distance = math::length(math::sub(point, viewer));
        (distance * CM_PER_PIXEL * pixels).min(MAX_LINE_WIDTH)
    };
    let mut lines = Vec::with_capacity(edges.len() * 2);
    let mut front = Vec::with_capacity(edges.len());
    for &edge in edges {
        let middle = math::scale(math::add(edge[0], edge[1]), 0.5);
        let dark = width_at(middle, LINE_OUTLINE_PIXELS);
        let away = math::normalized(math::sub(middle, viewer)).unwrap_or([0.0; 3]);
        let behind = math::scale(away, dark);
        lines.push(line(
            [math::add(edge[0], behind), math::add(edge[1], behind)],
            OUTLINE,
            dark,
        ));
        front.push(line(edge, color, width_at(middle, LINE_PIXELS)));
    }
    lines.append(&mut front);
    lines
}

/// Where a marker's name goes in the editor's space: above the marker, over its middle.
fn label_anchor(edges: &[[Vec3; 2]; MARKER_EDGES]) -> Vec3 {
    let points = edges.iter().flatten();
    let count = (edges.len() * 2).max(1) as f64;
    let (x, y) = points
        .clone()
        .fold((0.0, 0.0), |(x, y), point| (x + point[0], y + point[1]));
    let top = points.map(|point| point[2]).fold(f64::MIN, f64::max);
    [x / count, y / count, top + LABEL_GAP]
}

/// A camera's marker in the editor's space: a pyramid from the camera along where it looks, and a
/// little roof on its base's top edge, so which way is up shows.
fn marker(view: View) -> [[Vec3; 2]; MARKER_EDGES] {
    let apex = math::widen(view.location);
    let forward = math::normalized(math::widen(view.forward)).unwrap_or([1.0, 0.0, 0.0]);
    // Unreal's axes: X forward, Y right, Z up. Looking straight up or down, right is the editor's.
    let right = math::normalized(math::cross([0.0, 0.0, 1.0], forward)).unwrap_or([0.0, 1.0, 0.0]);
    let up = math::cross(forward, right);
    let center = math::add(apex, math::scale(forward, DEPTH));
    let at = |x: f64, y: f64| {
        math::add(
            center,
            math::add(
                math::scale(right, x * HALF_WIDTH),
                math::scale(up, y * HALF_HEIGHT),
            ),
        )
    };
    let corners = [at(-1.0, -1.0), at(1.0, -1.0), at(1.0, 1.0), at(-1.0, 1.0)];
    let roof = at(0.0, 1.8);
    [
        [apex, corners[0]],
        [corners[0], corners[1]],
        [apex, corners[1]],
        [corners[1], corners[2]],
        [apex, corners[2]],
        [corners[2], corners[3]],
        [apex, corners[3]],
        [corners[3], corners[0]],
        [at(-0.5, 1.0), roof],
        [at(0.5, 1.0), roof],
    ]
}
