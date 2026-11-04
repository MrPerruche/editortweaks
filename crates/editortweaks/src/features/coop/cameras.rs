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
/// A name's letters' height, and the gap between the top of its marker and the name (cm).
const LABEL_SIZE: f32 = 14.0;
const LABEL_GAP: f64 = 10.0;

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
    name: String,
    /// Linear red, green, blue.
    color: [f32; 3],
    /// Where it was last put (location, rotation).
    placed: Option<(Vector, Vector)>,
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
        let mut lines = Vec::new();
        // Where each name goes, in the editor's space.
        let mut anchors = Vec::new();
        for (&slot, (shown, _)) in &self.views {
            let color = presence::color_of(&self.slots, slot).1.map(linear);
            let edges = marker(*shown);
            anchors.push((slot, label_anchor(&edges)));
            lines.extend(edges.into_iter().map(|[start, end]| Line {
                start: math::narrow(math::add(origin, turn.rotate(start))),
                end: math::narrow(math::add(origin, turn.rotate(end))),
                color,
                thickness: 0.0,
                on_top: THROUGH_BRICKS.get(),
            }));
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
                unsafe { labels.text.destroy(&label.handle) };
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
                            vacant.insert(Label {
                                handle,
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
                unsafe { labels.text.destroy(&label.handle) };
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

/// Where a marker's name goes in the editor's space: above the marker, over its middle.
fn label_anchor(edges: &[[Vec3; 2]]) -> Vec3 {
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
fn marker(view: View) -> Vec<[Vec3; 2]> {
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
    let mut lines = Vec::with_capacity(10);
    for (i, &corner) in corners.iter().enumerate() {
        lines.push([apex, corner]);
        lines.push([corner, corners[(i + 1) % 4]]);
    }
    let roof = at(0.0, 1.8);
    lines.push([at(-0.5, 1.0), roof]);
    lines.push([at(0.5, 1.0), roof]);
    lines
}
