//! Reference images and models in the vehicle editor (`docs/model-io.md`): pictures and 3D models
//! from files, shown next to the vehicle to build after. They are never bricks: components of the
//! editor's own actor, so nothing about the vehicle changes.
//!
//! Each vehicle lists its references in a sidecar file next to it (`editortweaks.toml`, see
//! `model_io::sidecar`). While the editor is open, the open vehicle's folder is looked up from the
//! editor's file info ([`vehicle`]) and its sidecar shown ([`scene`]). Both are looked at again
//! every second, as are the referenced files: editing the sidecar or a file by hand shows right
//! away. References are selected by clicking them, like bricks ([`select`]); the editor's side
//! panel adds them (in the References section with the map's rows) and, while some are selected,
//! shows and edits them instead of the editor's settings ([`panel`]), each change shown at once and
//! written to the sidecar a moment later ([`edit`], [`State::save`]). Like bricks, selected
//! references hide with the editor's Hide key (H) until Unhide all (Alt + H): they then look like
//! hidden bricks and can't be clicked. All of them disappear with the Show references row or key
//! ([`keybinds::SHOW_REFERENCES`]). Neither is saved. An
//! unsaved vehicle keeps its references in memory until its first save, and a Save As carries them
//! over to the new folder ([`carry_over`]). The game autosaves unsaved vehicles into one folder
//! (see [`vehicle`]), and the vehicle is that folder's from then on: its references are written
//! there with each autosave, marked with it ([`write_autosave`], [`follow_autosave`]). Recovering
//! the autosave shows them; once another vehicle is autosaved there they're stale and not shown.
//!
//! The "Reference fit" view mode (`view_modes`) measures bricks against the model references
//! ([`fit_key`], [`fit_triangles`]), and makes its material with [`looks`].

mod edit;
mod files;
mod loading;
pub(crate) mod looks;
mod panel;
mod scene;
mod select;
mod vehicle;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use brickrust::ue::coreuobject::UObjectBase;
use model_io::fit::Surface;
use model_io::sidecar::{Kind, Sidecar, SourceMode};

use self::loading::Loaded;
use self::looks::Looks;
use self::scene::{Scene, Wanted};
use self::vehicle::Stamp;
use crate::features::keybinds;
use crate::framework::settings::Section;
use crate::game::brick_editor;
use crate::game::editor_file::{EditorFile, FileInfo};
use crate::game::editor_panel::{self, EditorPanel};
use crate::game::mesh_builder::MeshBuilder;
use crate::game::mesh_component::MeshComponents;
use crate::game::outline::{self, Outlines};
use crate::game::reflection::{self, Getter};
use crate::game::textures::Textures;
use crate::game::vehicle_box::VehicleBoxes;
use crate::game::weak_object::WeakObject;
use crate::math;
use crate::prelude::*;

/// How often the open vehicle, its sidecar and the referenced files are looked at.
const POLL: Duration = Duration::from_secs(1);

/// How long after the last change the sidecar is written (a slider drag changes it every frame).
const WRITE_DELAY: Duration = Duration::from_millis(400);

pub struct References;

/// The feature's settings, in the options' Editor tweaks menu.
pub static SETTINGS: [Section; 1] = [Section {
    title: "References",
    settings: &[
        crate::framework::settings::Setting::Choice(&panel::ADD_AS),
        crate::framework::settings::Setting::Choice(&looks::SEE_THROUGH_LOOK),
    ],
    // The side panel's "Show references" row sets it.
    unlisted: &[crate::framework::settings::Setting::Toggle(
        &select::SELECTABLE,
    )],
}];

/// The game code this feature uses, found at engine init.
struct Code {
    builder: MeshBuilder,
    components: MeshComponents,
    textures: Textures,
    file: EditorFile,
    boxes: VehicleBoxes,
    /// The side panel, to list its rows again; `None` (logged) if it wasn't found.
    panels: Option<EditorPanel>,
    /// `USceneComponent* AActor::K2_GetRootComponent() const`.
    root: Getter<*mut UObjectBase>,
    /// The selected reference's outline; `None` (logged) if not found.
    outlines: Option<Outlines>,
    /// Clicks and keys; `None` (logged) if not found: references can't be selected.
    input: Option<select::Input>,
}

// SAFETY: reflection records and code living as long as the game; only used on the game thread.
unsafe impl Send for Code {}
unsafe impl Sync for Code {}

static CODE: OnceLock<Code> = OnceLock::new();
static STATE: Mutex<State> = Mutex::new(State::new());

/// What's kept between editor ticks.
struct State {
    /// The editor everything was shown in; another one starts over.
    editor: Option<WeakObject>,
    /// Found at the first editor tick (`None` with the reason logged if missing).
    looks: Option<Result<Looks, ()>>,
    last_poll: Option<Instant>,
    /// The editor's file info last seen, for the log.
    file_info: Option<FileInfo>,
    /// The open vehicle's folder, if it's saved.
    folder: Option<PathBuf>,
    /// The references shown: the sidecar's, or an unsaved vehicle's in memory.
    sidecar: Sidecar,
    /// The sidecar's stamp when last read or written (`None` until read for this folder).
    sidecar_stamp: Option<Option<Stamp>>,
    /// The sidecar on disk couldn't be read: it's not written over (the user may fix it).
    unreadable: bool,
    /// The references last shown, to carry them over to a Save As or a first save.
    last_seen: Option<Seen>,
    /// The stamp of the vehicle in the autosave folder at the last poll (`None` before the first):
    /// when it changes, the game autosaved there.
    autosave: Option<Option<Stamp>>,
    scene: Scene,
    /// Model reads report here.
    loads: Option<(Sender<Loaded>, Receiver<Loaded>)>,
    /// The selected references, which the panel edits: the first one's values show.
    selected: Vec<String>,
    /// The reference a click pressed on, selected when it's released, and whether the click adds
    /// to the selection (Shift).
    pressed: Option<(String, bool)>,
    /// References hidden by the Hide key until Unhide all (not saved).
    hidden: Vec<String>,
    /// Whether references show at all (the Show references row and key; not saved, but kept
    /// from one editor to the next).
    shown: bool,
    /// Whether [`keybinds::SHOW_REFERENCES`] was held at the last tick.
    key_held: bool,
    /// Changes not written to the sidecar yet: since when.
    unwritten: Option<Instant>,
    /// An open file dialog, the kind of reference it adds and whether it links or copies the file.
    browsing: Option<(Kind, SourceMode, Receiver<Option<PathBuf>>)>,
    /// The panel lists different rows now: have it list them again.
    refresh_panel: bool,
    /// The editor's side panel, once found.
    panel: Option<WeakObject>,
}

// SAFETY: weak pointers and the scene, only used on the game thread.
unsafe impl Send for State {}

impl State {
    const fn new() -> Self {
        Self {
            editor: None,
            looks: None,
            last_poll: None,
            file_info: None,
            folder: None,
            sidecar: Sidecar {
                version: model_io::sidecar::CURRENT_VERSION,
                vehicle: None,
                references: Vec::new(),
            },
            sidecar_stamp: None,
            unreadable: false,
            last_seen: None,
            autosave: None,
            scene: Scene::new(),
            loads: None,
            selected: Vec::new(),
            pressed: None,
            hidden: Vec::new(),
            shown: true,
            key_held: false,
            unwritten: None,
            browsing: None,
            refresh_panel: false,
            panel: None,
        }
    }

    /// Selects the references `ids` (the first one's values show), and has the panel show their
    /// rows.
    fn select(&mut self, ids: Vec<String>) {
        if self.selected != ids {
            log!("selected: {ids:?}");
            self.selected = ids;
            self.refresh_panel = true;
        }
    }

    /// Adds the reference `id` to the selection, or takes it out if it's selected already.
    fn toggle(&mut self, id: String) {
        let mut ids = self.selected.clone();
        match ids.iter().position(|known| *known == id) {
            Some(index) => {
                ids.remove(index);
            }
            None => ids.push(id),
        }
        self.select(ids);
    }

    /// Shows or hides all references (the Show references row and key).
    fn show_all(&mut self, shown: bool) {
        if self.shown != shown {
            log!("references {}", if shown { "shown" } else { "hidden" });
            self.shown = shown;
            self.refresh_panel = true;
            self.last_poll = None;
        }
    }

    /// Hides the selected references until Unhide all (the Hide key). Whether any was selected.
    fn hide_selected(&mut self) -> bool {
        if self.selected.is_empty() {
            return false;
        }
        let selected = core::mem::take(&mut self.selected);
        log!("hidden: {selected:?}");
        self.hidden.extend(selected);
        self.refresh_panel = true;
        self.last_poll = None;
        true
    }

    /// Shows the references the Hide key hid (Unhide all).
    fn unhide_all(&mut self) {
        if !self.hidden.is_empty() {
            log!("unhidden: {:?}", self.hidden);
            self.hidden.clear();
            self.last_poll = None;
        }
    }

    /// Whether the reference `id` is hidden for now (not by its own settings): by the Hide key, or
    /// with all of them.
    fn hides(&self, id: &str) -> bool {
        !self.shown || self.hidden.iter().any(|hidden| hidden == id)
    }

    /// After a change: has the references shown at the next tick, and written to the sidecar once
    /// nothing changed for [`WRITE_DELAY`] (an unsaved vehicle's stay in memory).
    fn save(&mut self) {
        self.last_poll = None;
        self.unwritten = Some(Instant::now());
    }

    /// Writes changes not written yet, now.
    fn flush(&mut self) {
        if self.unwritten.take().is_some() {
            self.write();
        }
    }

    /// Writes the references to the sidecar (an unsaved vehicle's stay in memory).
    fn write(&mut self) {
        let Some(folder) = self.folder.clone() else {
            return;
        };
        if self.unreadable {
            log!(
                "{}: not written, it couldn't be read (fix or delete it)",
                vehicle::sidecar_path(&folder).display()
            );
            return;
        }
        for problem in edit::make_missing_copies(&mut self.sidecar, &folder) {
            log!("{problem}");
        }
        self.sidecar.vehicle = vehicle::mark_for(&folder);
        match self.sidecar.write(&folder) {
            Ok(()) => {
                self.sidecar_stamp = Some(vehicle::stamp(&vehicle::sidecar_path(&folder)));
            }
            Err(reason) => log!("{reason}"),
        }
    }
}

/// The feature's state for the panel's rows, which run on the game thread while the panel lists
/// or changes them, never inside a tick of ours: `None` if it's busy anyway (logged once).
fn state() -> Option<MutexGuard<'static, State>> {
    static BUSY: std::sync::Once = std::sync::Once::new();
    match STATE.try_lock() {
        Ok(state) => Some(state),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => {
            BUSY.call_once(|| log!("the panel asked while the references were busy"));
            None
        }
    }
}

/// A number that changes whenever [`fit_triangles`] would; `None` while the references are busy
/// (in one of our ticks). Game thread only.
pub fn fit_key() -> Option<u64> {
    let state = STATE.try_lock().ok()?;
    Some(state.scene.fit_key())
}

/// The triangles of the model references that bricks are measured against in the "Reference fit"
/// view mode, in the editor root's space: those shown by their own settings and not hidden by the
/// Hide key. `None` while the references are busy. Game thread only.
pub fn fit_triangles() -> Option<Vec<[[f32; 3]; 3]>> {
    let state = STATE.try_lock().ok()?;
    Some(state.scene.fit_triangles())
}

/// How far `point` (in the world) is from the nearest point of the models the "Reference fit" view
/// mode measures against ([`fit_triangles`]), in cm, as that view mode measures bricks. `None` if
/// none counts, or while the references are busy.
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
pub unsafe fn fit_distance(editor: *mut UObjectBase, point: [f32; 3]) -> Option<f64> {
    /// The references' triangles ready for the measure, and their [`fit_key`].
    static NEAREST: Mutex<Option<(u64, Surface)>> = Mutex::new(None);
    let input = CODE.get()?.input.as_ref()?;
    // SAFETY: per the contract. Only the origin is used.
    let local = unsafe { select::root_ray(input, editor, point, [1.0, 0.0, 0.0]) }?;
    let key = fit_key()?;
    let mut nearest = NEAREST.lock().unwrap_or_else(PoisonError::into_inner);
    if nearest.as_ref().is_none_or(|(built, _)| *built != key) {
        *nearest = Some((key, Surface::new(&fit_triangles()?)));
    }
    let (_, surface) = nearest.as_ref()?;
    // The root isn't scaled: this is in cm.
    surface.distance(math::narrow(local.origin)).map(f64::from)
}

impl Feature for References {
    fn id(&self) -> &'static str {
        "references"
    }

    fn on_load(&self) -> LoadResult {
        brick_editor::after_tick(after_tick)
    }

    fn on_engine_init(&self) {
        match find() {
            Ok(code) => {
                let clickable = code.input.is_some();
                let _ = CODE.set(code);
                if let Err(reason) = panel::add() {
                    log!("no rows in the side panel: {reason}");
                }
                if clickable && let Err(reason) = select::listen() {
                    log!("references can't be selected: {reason}");
                }
            }
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

fn find() -> Result<Code, String> {
    let actor = reflection::find_class("Actor").ok_or("class Actor not found")?;
    Ok(Code {
        builder: MeshBuilder::find()?,
        components: MeshComponents::find()?,
        textures: Textures::find()?,
        file: EditorFile::find()?,
        boxes: VehicleBoxes::find()?,
        panels: editor_panel::find()
            .map_err(|reason| log!("the side panel won't update by itself: {reason}"))
            .ok(),
        root: Getter::find(actor, "K2_GetRootComponent", "ObjectProperty")?,
        outlines: outline::find()
            .map_err(|reason| log!("the selected reference isn't outlined: {reason}"))
            .ok(),
        input: select::find()
            .map_err(|reason| log!("references can't be selected: {reason}"))
            .ok(),
    })
}

fn after_tick(editor: *mut UObjectBase, _delta: f32) {
    let Some(code) = CODE.get() else { return };
    let mut guard = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    let state = &mut *guard;

    if !state.editor.as_ref().is_some_and(|known| known.is(editor)) {
        // A new editor: what was shown went with the old one.
        state.flush();
        // SAFETY: on the game thread; only components still alive are destroyed.
        unsafe { state.scene.clear(code) };
        *state = State {
            // SAFETY: the live editor, ticking.
            editor: Some(unsafe { WeakObject::new(editor) }),
            looks: state.looks.take(),
            shown: state.shown,
            ..State::new()
        };
    }
    // Taken out for the tick (`poll` changes the rest of the state), put back below.
    let looks = state
        .looks
        .take()
        .unwrap_or_else(|| Looks::find().map_err(|reason| log!("inactive: {reason}")));
    let loads = state.loads.take().unwrap_or_else(mpsc::channel);
    if let Ok(looks) = &looks {
        // SAFETY: the live editor, on the game thread.
        unsafe { tick(code, editor, state, looks, &loads) };
    }
    state.looks = Some(looks);
    state.loads = Some(loads);

    let refresh = core::mem::take(&mut state.refresh_panel);
    let known_panel = state.panel.and_then(|panel| panel.get());
    drop(guard);
    if refresh {
        // SAFETY: the live editor, on the game thread, holding no lock (the panel lists our rows).
        let found = unsafe { refresh_panel(code, editor, known_panel) };
        if let Some(found) = found {
            let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
            // SAFETY: a live panel, just used.
            state.panel = Some(unsafe { WeakObject::new(found) });
        }
    }
}

/// An editor tick: the file dialog's answer, the vehicle and its sidecar every [`POLL`], and the
/// models' building.
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
unsafe fn tick(
    code: &Code,
    editor: *mut UObjectBase,
    state: &mut State,
    looks: &Looks,
    (sender, receiver): &(Sender<Loaded>, Receiver<Loaded>),
) {
    let held = keybinds::SHOW_REFERENCES.is_held();
    if held && !state.key_held {
        state.show_all(!state.shown);
    }
    state.key_held = held;

    if let Some((kind, mode, dialog)) = &state.browsing {
        let (kind, mode) = (*kind, *mode);
        match dialog.try_recv() {
            Ok(chosen) => {
                state.browsing = None;
                if let Some(path) = chosen {
                    panel::add_file(state, kind, &path, mode);
                }
            }
            Err(TryRecvError::Disconnected) => state.browsing = None,
            Err(TryRecvError::Empty) => {}
        }
    }

    if state.last_poll.is_none_or(|last| last.elapsed() >= POLL) {
        state.last_poll = Some(Instant::now());
        // SAFETY: the live editor, on the game thread.
        let wanted = unsafe { poll(code, editor, state) };
        // SAFETY: as above.
        unsafe { state.scene.sync(code, looks, editor, &wanted, sender) };
    }

    while let Ok(loaded) = receiver.try_recv() {
        state.scene.loaded(loaded);
    }
    // SAFETY: the live editor, on the game thread.
    unsafe { state.scene.pump(code, looks, editor) };

    if state
        .unwritten
        .is_some_and(|since| since.elapsed() >= WRITE_DELAY)
    {
        state.flush();
    }

    // The selection: references go with their removal or hiding, all of them when bricks got
    // selected.
    if !state.selected.is_empty() {
        // SAFETY: the live editor, on the game thread.
        let bricks = code
            .input
            .as_ref()
            .is_some_and(|input| unsafe { select::bricks_selected(input, editor) });
        let mut kept = state.selected.clone();
        kept.retain(|id| {
            !bricks
                && !state.hides(id)
                && state
                    .sidecar
                    .references
                    .iter()
                    .any(|reference| reference.id == *id)
        });
        state.select(kept);
    }
    // SAFETY: the live editor, on the game thread.
    unsafe { state.scene.select(code, looks, editor, &state.selected) };
}

/// Has the side panel list its rows again; the panel, if it was found.
///
/// # Safety
/// `editor` must be the live editor, `known` null or its live panel. Game thread only, holding no
/// lock of the feature's.
unsafe fn refresh_panel(
    code: &Code,
    editor: *mut UObjectBase,
    known: Option<*mut UObjectBase>,
) -> Option<*mut UObjectBase> {
    let panels = code.panels.as_ref()?;
    // SAFETY: per the contract.
    unsafe {
        let panel = known.or_else(|| panels.of(editor))?;
        panels.refresh(panel);
        Some(panel)
    }
}

/// A vehicle's folder (none while unsaved) and references, as shown at the last poll.
struct Seen {
    folder: Option<PathBuf>,
    sidecar: Sidecar,
    at: SystemTime,
}

/// Looks at the open vehicle and its sidecar again: the references that should be shown.
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
unsafe fn poll(code: &Code, editor: *mut UObjectBase, state: &mut State) -> Vec<Wanted> {
    // SAFETY: per the contract.
    let info = unsafe { code.file.read(editor) };
    let info_changed = state.file_info.as_ref() != Some(&info);
    if info_changed {
        log!(
            "editor file: type {}/{}, local id {:?}, title {:?}",
            info.file_type,
            info.ugc_type,
            info.local_id,
            info.title
        );
        state.file_info = Some(info.clone());
    }

    let now = SystemTime::now();
    let folder = vehicle::folder_of(&info);
    let autosave = vehicle::autosave_stamp();
    let autosaved = state.autosave.is_some_and(|seen| seen != autosave);
    state.autosave = Some(autosave);
    if state.folder != folder || (folder.is_none() && info_changed) {
        match &folder {
            Some(folder) => log!("vehicle folder: {}", folder.display()),
            None => log!("no vehicle folder (not saved yet?)"),
        }
        if autosaved && state.folder.as_deref().is_some_and(vehicle::is_autosave) {
            // The autosave folder's vehicle, autosaved as it's left.
            follow_autosave(state);
        }
        state.flush();
        // The game's first autosave of an unsaved vehicle makes it the autosave folder's vehicle
        // (its file info then names that folder): the same vehicle, its references go along.
        // Recovering the autosave from the game's browser doesn't write it.
        let first_autosave = autosaved
            && state.folder.is_none()
            && folder.as_deref().is_some_and(vehicle::is_autosave);
        if let (true, Some(to)) = (first_autosave, &folder) {
            write_autosave(&state.sidecar, to);
        } else if let (Some(to), Some(seen)) = (&folder, state.last_seen.as_ref()) {
            carry_over(seen, to);
        }
        // Another vehicle: an unsaved one starts without references.
        state.last_seen = None;
        state.folder = folder;
        state.sidecar = Sidecar::default();
        state.sidecar_stamp = None;
        state.unreadable = false;
        if !first_autosave {
            state.selected.clear();
            state.hidden.clear();
        }
        state.refresh_panel = true;
    } else if autosaved {
        follow_autosave(state);
    }

    if let Some(folder) = state.folder.clone() {
        read_sidecar(state, &folder);
    }
    state.last_seen = Some(Seen {
        folder: state.folder.clone(),
        sidecar: state.sidecar.clone(),
        at: now,
    });

    state
        .sidecar
        .references
        .iter()
        .map(|reference| Wanted {
            reference: reference.clone(),
            file: vehicle::find_file(reference, state.folder.as_deref()),
            hidden: state.hidden.contains(&reference.id),
            shown: state.shown,
        })
        .collect()
}

/// Reads the sidecar in `folder` again if it changed on disk.
fn read_sidecar(state: &mut State, folder: &Path) {
    let path = vehicle::sidecar_path(folder);
    let stamp = vehicle::stamp(&path);
    if state.sidecar_stamp == Some(stamp) {
        return;
    }
    state.sidecar_stamp = Some(stamp);
    state.refresh_panel = true;
    match Sidecar::read(folder) {
        Ok(Some(sidecar)) if !vehicle::belongs(&sidecar, folder) => {
            log!(
                "{}: written with an earlier autosave, not shown",
                path.display()
            );
            state.sidecar = Sidecar::default();
            state.unreadable = false;
        }
        Ok(Some(mut sidecar)) => {
            let mut ids = HashSet::new();
            sidecar.references.retain(|reference| {
                let new = ids.insert(reference.id.clone());
                if !new {
                    log!(
                        "{}: id {:?} used twice, left out",
                        path.display(),
                        reference.id
                    );
                }
                new
            });
            if sidecar.is_newer() {
                log!(
                    "{}: written by a newer version of the mod, shown as far as known",
                    path.display()
                );
            }
            log!(
                "{}: {} references",
                path.display(),
                sidecar.references.len()
            );
            state.sidecar = sidecar;
            state.unreadable = false;
        }
        Ok(None) => {
            state.sidecar = Sidecar::default();
            state.unreadable = false;
        }
        // Keeps showing what was read before.
        Err(reason) => {
            log!("{reason}");
            state.unreadable = true;
        }
    }
}

/// After a Save As or an unsaved vehicle's first save (the open vehicle moved to `to`, a folder the
/// game made since `seen` was last shown), writes `seen`'s references there too, as the vehicle
/// itself was saved there. Opening another saved vehicle doesn't qualify: its folder is older.
fn carry_over(seen: &Seen, to: &Path) {
    if seen.sidecar.references.is_empty()
        || seen.folder.as_deref() == Some(to)
        || !vehicle::created_after(to, seen.at)
        || vehicle::stamp(&vehicle::sidecar_path(to)).is_some()
    {
        return;
    }
    // An unsaved vehicle's references are links to full paths: nothing is relative to a folder.
    let from = seen.folder.as_deref().unwrap_or(to);
    let result = seen.sidecar.carried_over(from, to).and_then(|mut sidecar| {
        for problem in edit::make_missing_copies(&mut sidecar, to) {
            log!("{problem}");
        }
        sidecar.vehicle = vehicle::mark_for(to);
        sidecar.write(to)
    });
    let from = seen
        .folder
        .as_ref()
        .map_or("the unsaved vehicle".to_owned(), |from| {
            from.display().to_string()
        });
    match result {
        Ok(()) => log!(
            "saved as {}: {} references carried over from {from}",
            to.display(),
            seen.sidecar.references.len(),
        ),
        Err(reason) => log!(
            "saved as {}: references not carried over: {reason}",
            to.display()
        ),
    }
}

/// After the game autosaved into its autosave folder, the open vehicle staying the same: if that's
/// the vehicle autosaved there, writes its references along, marked with this autosave. Another
/// vehicle open (one saved in its own folder), the sidecar there is left: it no longer belongs.
fn follow_autosave(state: &mut State) {
    match state.folder.clone() {
        Some(folder) if vehicle::is_autosave(&folder) => {
            if state.unreadable {
                // Read again: it may belong to no vehicle now.
                state.sidecar_stamp = None;
            } else {
                state.unwritten = None;
                state.write();
            }
        }
        Some(_) => {}
        // Still unsaved: the game names the autosave folder as soon as it autosaved, so this is
        // a moment's gap at most.
        None => {
            if let Some(folder) = vehicle::autosave_folder() {
                write_autosave(&state.sidecar.clone(), &folder);
            }
        }
    }
}

/// Writes the references of an unsaved vehicle the game just autosaved into its autosave `folder`,
/// marked with that autosave (none: the sidecar there, another vehicle's, is deleted). Links stay
/// links; copies are made there once it's the open vehicle's folder.
fn write_autosave(sidecar: &Sidecar, folder: &Path) {
    if let Err(reason) = Sidecar::read(folder) {
        log!("autosaved: references not written along: {reason}");
        return;
    }
    let sidecar = Sidecar {
        vehicle: vehicle::mark_for(folder),
        ..sidecar.clone()
    };
    if sidecar.vehicle.is_none() {
        log!("autosaved: references not written along: the autosave can't be read");
        return;
    }
    match sidecar.write(folder) {
        Ok(()) => log!(
            "autosaved: {} references written along",
            sidecar.references.len()
        ),
        Err(reason) => log!("autosaved: references not written along: {reason}"),
    }
}
