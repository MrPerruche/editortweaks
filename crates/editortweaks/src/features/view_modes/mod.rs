//! Our view modes, cycled with Z or picked in the editor panel's view mode list (nothing selected),
//! each showing something the game doesn't:
//!
//! - "Connections" ([`connections`]): bricks colored by the part of the vehicle they hold together
//!   with;
//! - "Mirror" ([`mirror`]): what each brick has at its mirror image, as mirror mode sees it;
//! - "Duplicates" ([`duplicates`]): bricks sharing their place with copies of themselves;
//! - "Strength" ([`strength`]): bricks shaded by the damage it takes to break them off;
//! - "Collision" ([`collision`]): bricks' collision shapes, with their own materials;
//! - "Reference fit" ([`fit`]): bricks colored by how far they are from the reference models.
//!
//! How (`game::editor_view`): each one is a new entry of the game's view mode enum, which the view
//! mode cycle reads. In the coloring ones, bricks get the material of the game's group view modes
//! (hooking `GetEditorMaterialOverride`) and our colors in its custom primitive data (after the
//! function writing the game's). Collision keeps the bricks' materials and adds meshes; Reference fit
//! gives bricks the hidden material and adds meshes. A view mode works out every brick's colors at once ([`Coloring`]), kept
//! until the editor changes: after every change the editor re-pairs mirrored bricks
//! (`UpdateMirroredObjects`), and then bricks whose colors changed are repainted. The game names
//! view modes from a fixed list, so ours are added to it: the panel's list shows them, and their
//! names after Z aren't "Invalid". Hiding bricks or showing them again (H, Alt + H) isn't an edit:
//! a coloring view mode is worked out again when the hidden bricks change too (checked every frame),
//! since Strength and Connections leave them out.
//!
//! Strength, Connections and Reference fit also have a tooltip next to the mouse ([`tooltip`],
//! [`describe`]).
//!
//! The game's own Physics view mode gets more too ([`physics`]): the centers of lift and thrust
//! next to its center of mass, and the tooltip over the three.

mod collision;
mod connections;
mod duplicates;
mod fit;
mod mirror;
mod physics;
mod strength;
mod tooltip;

/// For the shot preview's tooltip: the AP unit and how numbers read.
pub(crate) use strength::{AP_DAMAGE, AP_PENETRATION, break_off_damage};

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, Once, OnceLock, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;
use core::ffi::c_void;

use crate::framework::hook;
use crate::framework::settings::{Section, Setting};
use crate::game::brick_editor;
use crate::game::editor_view::{
    self, Bricks, EditorCode, MaterialOverrideFn, PrimitiveDataFn, UpdateMirroredObjectsFn,
    ViewModeItems, ViewModeItemsFn,
};
use crate::game::kismet;
use crate::game::text::Text;
use crate::prelude::*;

/// A brick's two colors, mixed in a pattern (the same color twice shows plain). Each is red, green,
/// blue as linear values from 0 to 1, not the 0 to 255 of color pickers (about
/// `(value / 255)^2.2`: 128 is 0.22). The group view modes use 0.2 and 1. Lighting still shades
/// them.
type Colors = [[f32; 3]; 2];

/// Bricks with nothing to show, in each of our view modes. Free to change, like the colors in each
/// view mode's module.
const GRAY: [f32; 3] = [0.2, 0.2, 0.2];
const PLAIN: Colors = [GRAY, GRAY];

/// The view modes' settings, in the options' Editor tweaks menu.
pub static SETTINGS: [Section; 1] = [Section {
    title: "View modes",
    settings: &[
        Setting::Toggle(&tooltip::TOOLTIP),
        Setting::Toggle(&physics::CENTERS),
    ],
}];

/// Our view modes, in the order Z cycles through them: each one's name and how to make it.
/// A view mode's value is its place here, kept in the user's settings: reordering them switches a
/// saved one to another, once.
const MODES: [(&str, NewLook); 6] = [
    ("Connections", || connections::new().map(Look::Colors)),
    ("Mirror", || mirror::new().map(Look::Colors)),
    ("Duplicates", || duplicates::new().map(Look::Colors)),
    ("Strength", || strength::new().map(Look::Colors)),
    ("Collision", || collision::find().map(|()| Look::Collision)),
    ("Reference fit", || fit::find().map(|()| Look::Fit)),
];

/// The color `share` of the way from `from` to `to` (0 to 1), blended as a color picker's values
/// (sRGB), where halfway looks halfway; blending the linear values themselves keeps most shades
/// close to the paler end.
fn blend(from: [f32; 3], to: [f32; 3], share: f32) -> [f32; 3] {
    core::array::from_fn(|channel| {
        let [from, to] = [from[channel], to[channel]].map(to_srgb);
        to_linear(from + (to - from) * share)
    })
}

/// A linear color value as a color picker's (sRGB, 0 to 1): the standard sRGB curve.
fn to_srgb(linear: f32) -> f32 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

/// The inverse of [`to_srgb`].
fn to_linear(srgb: f32) -> f32 {
    if srgb <= 0.040_45 {
        srgb / 12.92
    } else {
        ((srgb + 0.055) / 1.055).powf(2.4)
    }
}

/// Makes a view mode's [`Look`] (after engine init), or tells why it can't.
type NewLook = fn() -> Result<Look, String>;

/// How one of our view modes shows bricks.
enum Look {
    /// Painted with colors, through the group view modes' material.
    Colors(Box<dyn Coloring>),
    /// Their collision shapes, with their own materials ([`collision`]).
    Collision,
    /// Colored by their distance to the reference models, over the hidden material ([`fit`]).
    Fit,
}

/// How one of our view modes colors bricks.
trait Coloring: Send + Sync {
    /// What the view mode shows of `editor`'s bricks.
    ///
    /// # Safety
    /// `editor.editor` must be a live `ABrickEditor`. Game thread only.
    unsafe fn paint(&self, editor: &Editor) -> Painting;

    /// The lines of the tooltip over what `hover` points at; none by default.
    ///
    /// # Safety
    /// As for [`Self::paint`].
    unsafe fn describe(&self, _editor: &Editor, _hover: &tooltip::Hover) -> Vec<String> {
        Vec::new()
    }
}

/// What a view mode shows.
struct Painting {
    /// Bricks' colors, by address; the others show [`PLAIN`].
    colors: HashMap<usize, Colors>,
    /// What it found, logged the first time it's shown.
    summary: String,
}

/// An editor whose bricks a view mode colors, and the game code to look at them.
struct Editor<'a> {
    code: &'a EditorCode,
    bricks: &'a Bricks,
    /// A live `ABrickEditor`.
    editor: *mut UObjectBase,
}

impl Editor<'_> {
    /// The editor's objects (bricks and others; entries may be null), only valid until they change.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn objects(&self) -> &[*mut UObjectBase] {
        // SAFETY: per the contract; `editor` is live.
        unsafe { self.bricks.editor_objects(self.editor) }
    }

    /// The editor's bricks.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn brick_list(&self) -> Vec<*mut UObjectBase> {
        // SAFETY: per the contract; the editor's objects are live.
        unsafe {
            self.objects()
                .iter()
                .copied()
                .filter(|&object| !object.is_null() && self.bricks.is_brick(object))
                .collect()
        }
    }

    /// Whether `object` (one of the editor's) is hidden (H, or by the view mode).
    ///
    /// # Safety
    /// `object` must be a live editor object. Game thread only.
    unsafe fn is_hidden(&self, object: *const UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { self.code.is_hidden(object) }
    }
}

pub struct ViewModes;

static CODE: OnceLock<EditorCode> = OnceLock::new();
static MATERIAL_OVERRIDE: OnceLock<MaterialOverrideFn> = OnceLock::new();
static PRIMITIVE_DATA: OnceLock<PrimitiveDataFn> = OnceLock::new();
static UPDATE_MIRRORED_OBJECTS: OnceLock<UpdateMirroredObjectsFn> = OnceLock::new();
static ITEMS: OnceLock<ViewModeItems> = OnceLock::new();
static VIEW_MODE_ITEMS: OnceLock<ViewModeItemsFn> = OnceLock::new();
/// Set once our view modes exist; until then (or if that fails) the hooks change nothing.
static VIEWS: OnceLock<Views> = OnceLock::new();
/// How each brick (by address) was last painted.
static PAINTED: Mutex<BTreeMap<usize, Colors>> = Mutex::new(BTreeMap::new());
/// What the view mode shown shows, for the editor as it is; `None` once it changed.
static SHOWN: Mutex<Option<Shown>> = Mutex::new(None);
/// Counts the editor's changes (edits, and changes of the hidden bricks): what the tooltip shows
/// depends on them.
static GENERATION: AtomicU64 = AtomicU64::new(0);

struct Views {
    bricks: Bricks,
    modes: Vec<View>,
}

/// One of our view modes.
struct View {
    name: &'static str,
    /// Its value in the game's enum.
    value: u8,
    look: Look,
    /// Its name for the game's lists, kept for the whole game (the lists get copies); without it,
    /// it isn't listed.
    label: Option<Label>,
    /// Logs its first summary.
    logged: Once,
}

struct Label(Text);

// SAFETY: a game text living as long as the game; only used on the game thread.
unsafe impl Send for Label {}
unsafe impl Sync for Label {}

/// A view mode's colors, for the editor as it is.
struct Shown {
    value: u8,
    colors: HashMap<usize, Colors>,
}

impl Feature for ViewModes {
    fn id(&self) -> &'static str {
        "view_modes"
    }

    fn on_load(&self) -> LoadResult {
        let code = editor_view::editor_code()?;
        // SAFETY: each target is the start of the function its signature names, and each detour
        // has that function's signature.
        unsafe {
            hook::install_typed(
                editor_view::MATERIAL_OVERRIDE.name,
                code.material_override,
                material_override as MaterialOverrideFn,
                &MATERIAL_OVERRIDE,
            )?;
            hook::install_typed(
                editor_view::PRIMITIVE_DATA.name,
                code.primitive_data,
                primitive_data as PrimitiveDataFn,
                &PRIMITIVE_DATA,
            )?;
            hook::install_typed(
                editor_view::UPDATE_MIRRORED_OBJECTS.name,
                code.update_mirrored_objects,
                update_mirrored_objects as UpdateMirroredObjectsFn,
                &UPDATE_MIRRORED_OBJECTS,
            )?;
        }
        let _ = CODE.set(code);
        // Optional: without it the view modes work with Z, but aren't in the panel's list and
        // their names show as "Invalid".
        if let Err(reason) = install_items_hook() {
            log!("the view modes won't be listed: {reason}");
        }
        // Optional: without it, collision shapes don't show the selection outline.
        if let Err(reason) = brick_editor::after_tick(collision::after_tick) {
            log!("collision shapes won't be outlined: {reason}");
        }
        // Without it, the reference fit view mode shows no colors.
        if let Err(reason) = brick_editor::after_tick(fit::after_tick) {
            log!("the reference fit won't show: {reason}");
        }
        if let Err(reason) = brick_editor::after_tick(follow_hidden) {
            log!("the coloring view modes won't follow hidden bricks: {reason}");
        }
        // Before the tooltip, which reads the centers.
        if let Err(reason) = brick_editor::after_tick(physics::after_tick) {
            log!("no centers of lift and thrust: {reason}");
        }
        if let Err(reason) = brick_editor::after_tick(tooltip::after_tick) {
            log!("the view modes show no tooltip: {reason}");
        }
        Ok(())
    }

    fn on_engine_init(&self) {
        if let Err(reason) = engine_init() {
            log!("inactive: {reason}");
        }
    }
}

fn install_items_hook() -> Result<(), String> {
    let items = editor_view::view_mode_items()?;
    // SAFETY: `items.function` is the start of the function the signature names, and the detour
    // has its signature.
    unsafe {
        hook::install_typed(
            editor_view::VIEW_MODE_ITEMS.name,
            items.function,
            view_mode_items as ViewModeItemsFn,
            &VIEW_MODE_ITEMS,
        )?;
    }
    let _ = ITEMS.set(items);
    Ok(())
}

fn engine_init() -> Result<(), String> {
    let bricks = editor_view::bricks()?;
    let mut modes = Vec::new();
    for (name, new) in MODES {
        let look = match new() {
            Ok(look) => look,
            Err(reason) => {
                log!("view mode '{name}' inactive: {reason}");
                continue;
            }
        };
        let label = match ITEMS.get().map(|_| kismet::string_to_text(name)) {
            Some(Ok(text)) => Some(Label(text)),
            Some(Err(reason)) => {
                log!("view mode '{name}' won't be listed: {reason}");
                None
            }
            None => None,
        };
        // Last: the view mode only becomes selectable once it has everything it needs.
        let value = match editor_view::add_view_mode(name) {
            Ok(value) => value,
            Err(reason) => {
                log!("view mode '{name}' inactive: {reason}");
                continue;
            }
        };
        log!("active: view mode '{name}' ({value}), cycle with Z");
        modes.push(View {
            name,
            value,
            look,
            label,
            logged: Once::new(),
        });
    }
    if modes.is_empty() {
        return Err("no view mode added".into());
    }
    let _ = VIEWS.set(Views { bricks, modes });
    Ok(())
}

/// The game code, our view modes and the one `brick` shows, if it's a brick in the editor shown
/// in one of ours.
///
/// # Safety
/// `brick` must be a live brick. Game thread only.
unsafe fn shown(
    brick: *const UObjectBase,
) -> Option<(&'static EditorCode, &'static Views, &'static View)> {
    let (code, views) = (CODE.get()?, VIEWS.get()?);
    // SAFETY: per the contract.
    let value = unsafe { code.view_mode(brick) }?;
    Some((code, views, views.view(value)?))
}

unsafe extern "C" fn material_override(brick: *mut UObjectBase) -> *mut UObjectBase {
    let original = *MATERIAL_OVERRIDE.get().expect("hook recorded before use");
    // SAFETY: the game's call, passed on; then `brick` is the live brick it asked about.
    unsafe {
        let material = original(brick);
        // Hidden bricks keep the game's material for them.
        if !material.is_null() {
            return material;
        }
        match shown(brick) {
            Some((code, views, view)) => match view.look {
                Look::Colors(_) => views.bricks.color_material(code, brick),
                Look::Fit => views.bricks.hidden_material(code, brick),
                Look::Collision => material,
            },
            None => material,
        }
    }
}

unsafe extern "C" fn primitive_data(
    brick: *mut UObjectBase,
    component: *const c_void,
    transform: *const c_void,
    relative: *const c_void,
) {
    let original = *PRIMITIVE_DATA.get().expect("hook recorded before use");
    // SAFETY: the game's call, passed on; then `brick` is the live brick it updated.
    unsafe {
        original(brick, component, transform, relative);
        match shown(brick) {
            Some((code, views, view)) => match view.look {
                Look::Colors(_) => {
                    collision::remove(brick);
                    fit::remove(brick);
                    views.paint(code, view, brick);
                }
                Look::Collision => {
                    fit::remove(brick);
                    collision::show(code, &views.bricks, brick);
                }
                Look::Fit => {
                    collision::remove(brick);
                    fit::show(code, brick);
                }
            },
            None => {
                collision::remove(brick);
                fit::remove(brick);
            }
        }
    }
}

/// Lists our view modes with the game's: in the editor panel's view mode list, and for their names
/// after Z (which would be "Invalid").
unsafe extern "C" fn view_mode_items(container: *const c_void, items: *mut c_void) {
    let original = *VIEW_MODE_ITEMS.get().expect("hook recorded before use");
    // SAFETY: the game's call, passed on; then `items` holds the game's list.
    unsafe {
        original(container, items);
        let (Some(views), Some(list)) = (VIEWS.get(), ITEMS.get()) else {
            return;
        };
        for view in &views.modes {
            if let Some(Label(name)) = &view.label {
                list.append(items, view.value, name);
            }
        }
    }
}

unsafe extern "C" fn update_mirrored_objects(editor: *mut UObjectBase) {
    let original = *UPDATE_MIRRORED_OBJECTS
        .get()
        .expect("hook recorded before use");
    // SAFETY: the game's call, passed on; then `editor` is the live editor that changed.
    unsafe {
        original(editor);
        // The editor changed, and so may every brick's colors.
        changed();
        if let (Some(code), Some(views)) = (CODE.get(), VIEWS.get()) {
            views.repaint_changed(code, editor);
        }
    }
}

/// How many times the editor changed (edits; changes of the hidden bricks only while a coloring
/// view mode shows): what depends on the vehicle is worked out again when it moves on.
pub(crate) fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// The editor changed: what view modes show is worked out again.
fn changed() {
    if let Ok(mut shown) = SHOWN.lock() {
        *shown = None;
    }
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// Every frame: repaints the coloring view mode shown if the hidden bricks changed.
fn follow_hidden(editor: *mut UObjectBase, _delta: f32) {
    /// The editor and its hidden objects, by address, hashed.
    static HIDDEN: Mutex<Option<(usize, u64)>> = Mutex::new(None);
    let (Some(code), Some(views)) = (CODE.get(), VIEWS.get()) else {
        return;
    };
    let editor_view = Editor {
        code,
        bricks: &views.bricks,
        editor,
    };
    // SAFETY: on the game thread (the editor's tick), `editor` the live editor ticking; its
    // objects are live.
    unsafe {
        let Some((view, _)) = shown_view(editor) else {
            return;
        };
        if !matches!(
            views.view(view).map(|view| &view.look),
            Some(Look::Colors(_))
        ) {
            return;
        }
        let mut hasher = DefaultHasher::new();
        for &object in editor_view.objects() {
            if !object.is_null() && code.is_hidden(object) {
                (object as usize).hash(&mut hasher);
            }
        }
        let now = (editor as usize, hasher.finish());
        let before = HIDDEN
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(now);
        if before.is_some_and(|before| before != now) {
            changed();
            views.repaint_changed(code, editor);
        }
    }
}

/// Which of our view modes `editor` shows (its value), and the editor's [`GENERATION`].
///
/// # Safety
/// `editor` must be a live `ABrickEditor`. Game thread only.
unsafe fn shown_view(editor: *mut UObjectBase) -> Option<(u8, u64)> {
    let views = VIEWS.get()?;
    // SAFETY: per the contract.
    let view = views.view(unsafe { view_of(editor) }?)?;
    Some((view.value, GENERATION.load(Ordering::Relaxed)))
}

/// The view mode `editor` shows (any, the game's too), as its first brick tells; `None` without
/// bricks.
///
/// # Safety
/// `editor` must be a live `ABrickEditor`. Game thread only.
unsafe fn view_of(editor: *mut UObjectBase) -> Option<u8> {
    let (code, views) = (CODE.get()?, VIEWS.get()?);
    // SAFETY: per the contract; the editor's objects are live.
    unsafe {
        let first = views
            .bricks
            .editor_objects(editor)
            .iter()
            .copied()
            .find(|&object| !object.is_null() && views.bricks.is_brick(object))?;
        code.view_mode(first)
    }
}

/// The view mode whose tooltip `editor` shows (one of ours, or the game's Physics), and what the
/// tooltip depends on besides the mouse: a count that moves on when the vehicle does.
///
/// # Safety
/// `editor` must be a live `ABrickEditor`. Game thread only.
unsafe fn tooltip_view(editor: *mut UObjectBase) -> Option<(u8, u64)> {
    // SAFETY: per the contract.
    unsafe { shown_view(editor).or_else(|| physics::shown(editor)) }
}

/// The tooltip of the view mode `value` ([`tooltip_view`]) over `editor`, at what `hover` points
/// at: ours titled with the view mode's name, the Physics view mode's with the center under the
/// mouse.
///
/// # Safety
/// `editor` must be a live `ABrickEditor`. Game thread only.
unsafe fn describe(
    editor: *mut UObjectBase,
    value: u8,
    hover: &tooltip::Hover,
) -> tooltip::Description {
    if physics::is_physics(value) {
        // SAFETY: per the contract.
        return unsafe { physics::describe(editor, hover) };
    }
    let (Some(code), Some(views)) = (CODE.get(), VIEWS.get()) else {
        return tooltip::Description::default();
    };
    let Some(view) = views.view(value) else {
        return tooltip::Description::default();
    };
    let editor = Editor {
        code,
        bricks: &views.bricks,
        editor,
    };
    // SAFETY: per the contract.
    let lines = unsafe {
        match &view.look {
            Look::Colors(coloring) => coloring.describe(&editor, hover),
            Look::Fit => fit::describe(&editor, hover),
            Look::Collision => Vec::new(),
        }
    };
    tooltip::Description {
        title: view.name.to_owned(),
        lines,
    }
}

impl Views {
    /// Our view mode with the value `value`, if it is one of ours.
    fn view(&self, value: u8) -> Option<&View> {
        self.modes.iter().find(|view| view.value == value)
    }

    /// Paints `brick` as `view` shows it.
    ///
    /// # Safety
    /// `brick` must be a live brick in the editor. Game thread only.
    unsafe fn paint(&self, code: &EditorCode, view: &View, brick: *mut UObjectBase) {
        // SAFETY: per the contract.
        unsafe { self.paint_as(brick, self.colors(code, view, brick)) }
    }

    /// # Safety
    /// `brick` must be a live brick in the editor. Game thread only.
    unsafe fn paint_as(&self, brick: *mut UObjectBase, colors: Colors) {
        // SAFETY: per the contract.
        unsafe { self.bricks.paint(brick, colors) };
        if let Ok(mut painted) = PAINTED.lock() {
            painted.insert(brick as usize, colors);
        }
    }

    /// `brick`'s colors in `view`, working out every brick's for its editor if not known yet.
    ///
    /// # Safety
    /// `brick` must be a live brick in the editor. Game thread only.
    unsafe fn colors(&self, code: &EditorCode, view: &View, brick: *mut UObjectBase) -> Colors {
        let key = brick as usize;
        if let Some(colors) = known(view, key) {
            return colors;
        }
        // SAFETY: per the contract; the brick's editor is live while the brick is in it.
        unsafe {
            let editor = self.bricks.editor_of(brick);
            if !editor.is_null() {
                self.show(code, view, editor);
            }
        }
        known(view, key).unwrap_or(PLAIN)
    }

    /// Works out what `view` shows of `editor`'s bricks, kept until the editor changes.
    ///
    /// # Safety
    /// `editor` must be a live `ABrickEditor`. Game thread only.
    unsafe fn show(&self, code: &EditorCode, view: &View, editor: *mut UObjectBase) {
        let editor = Editor {
            code,
            bricks: &self.bricks,
            editor,
        };
        let Look::Colors(coloring) = &view.look else {
            return;
        };
        // SAFETY: per the contract.
        let painting = unsafe { coloring.paint(&editor) };
        view.logged
            .call_once(|| log!("{}: {}", view.name, painting.summary));
        if let Ok(mut shown) = SHOWN.lock() {
            *shown = Some(Shown {
                value: view.value,
                colors: painting.colors,
            });
        }
    }

    /// Repaints the bricks of `editor` whose colors changed since they were painted, if it shows
    /// one of our view modes.
    ///
    /// # Safety
    /// `editor` must be a live `ABrickEditor`. Game thread only.
    unsafe fn repaint_changed(&self, code: &EditorCode, editor: *mut UObjectBase) {
        // SAFETY: per the contract; the editor's bricks are live, and painting doesn't change them.
        unsafe {
            let bricks = Editor {
                code,
                bricks: &self.bricks,
                editor,
            }
            .brick_list();
            // Every brick shows the same view mode.
            let view = bricks
                .first()
                .and_then(|&first| code.view_mode(first))
                .and_then(|value| self.view(value));
            match view.map(|view| &view.look) {
                Some(Look::Collision) => {
                    // Deleted bricks lose their shapes, changed ones get new ones.
                    fit::keep_only(&[]);
                    collision::keep_only(&bricks);
                    collision::refresh(code, &self.bricks, &bricks);
                    return;
                }
                Some(Look::Fit) => {
                    // Deleted bricks lose their colors, moved ones are measured again.
                    collision::keep_only(&[]);
                    fit::keep_only(&bricks);
                    fit::refresh(code, &bricks);
                    return;
                }
                _ => {}
            }
            collision::keep_only(&[]);
            fit::keep_only(&[]);
            let Some(view) = view else {
                return;
            };
            self.show(code, view, editor);
            for brick in bricks {
                let colors = self.colors(code, view, brick);
                let painted = PAINTED
                    .lock()
                    .ok()
                    .and_then(|painted| painted.get(&(brick as usize)).copied());
                if painted != Some(colors) {
                    self.paint_as(brick, colors);
                }
            }
        }
    }
}

/// The colors of `brick` (by address) in `view`, if worked out for the editor as it is.
fn known(view: &View, brick: usize) -> Option<Colors> {
    let shown = SHOWN.lock().ok()?;
    let shown = shown.as_ref().filter(|shown| shown.value == view.value)?;
    Some(shown.colors.get(&brick).copied().unwrap_or(PLAIN))
}
