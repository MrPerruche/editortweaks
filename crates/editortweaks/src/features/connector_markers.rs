//! The connector markers a selected scalable brick shows (see `game::connectors_ism`):
//!
//! - They stay on their connectors on very large bricks. The game draws a face's points as one
//!   instance per rectangle of them, scaled to its size, and the material tiles one marker per cell
//!   by that scale. The engine keeps instance scales as half floats (11 significant bits): past
//!   2048 cells, or 683 at 15 and 30 cm spacing, they round, and the markers slide off their
//!   connectors toward the far end. Rectangles longer than [`MOST_CELLS`] are cut into pieces at
//!   whole cells, which tile the same markers with exact scales.
//! - With [`AXIS_COLORS`], they're colored by the axis they face: red X, green Y, blue Z, paler
//!   where hovering a size slider changes them (the game: gray, and yellow). The material takes two
//!   colors, so each axis gets a component of its own, attached to the game's, which is emptied.
//!
//! How: after the game adds a brick's instances ([`ADD_CONNECTOR_FIELDS`]), they're read back,
//! cut where needed and added again, to the brick's component or to its axes'. Every frame, axis
//! components whose brick's component no longer shows are destroyed: when a brick stops showing
//! markers, the game destroys its component, or detaches it and keeps it for another brick. Only
//! what the editor shows changes.
//!
//! [`ADD_CONNECTOR_FIELDS`]: connectors_ism::ADD_CONNECTOR_FIELDS

use core::ffi::c_void;
use std::sync::{Mutex, Once, OnceLock, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;

use crate::framework::hook;
use crate::framework::settings::Toggle;
use crate::game::brick_editor;
use crate::game::connectors_ism::{
    self, AddConnectorFieldsFn, ConnectorsIsm, Instance, KIND_SPACING_CM, MESH_CM,
};
use crate::game::editor_object::Vector;
use crate::game::mesh_component::MeshComponents;
use crate::game::reflection;
use crate::game::textures::{LinearColor, Textures};
use crate::game::weak_object::WeakObject;
use crate::math::{self, Quat};
use crate::prelude::*;

pub struct ConnectorMarkers;

pub static AXIS_COLORS: Toggle = Toggle::new(
    "ConnectorAxisColors",
    "Connector colors by axis",
    "A selected scalable brick's connector markers are colored by the axis they face: red X, \
     green Y, blue Z. Off: the game's gray and yellow.",
    true,
);

/// The most cells a marker instance spans along a side, when it lies along the brick's axes: its
/// scale (cells times 1, 1.5 or 3) is then exact as a half float (whole up to 2048, halves up to
/// 1024).
const MOST_CELLS: u32 = 512;
/// The same on a slanted face, whose rotation isn't exact as half floats: there, every piece is a
/// little off in proportion to its size, a 64th of a cell at most.
const MOST_SLANTED_CELLS: u32 = 64;
/// The most pieces one instance is cut into; past that (a face hundreds of thousands of cells
/// long), it's left whole rather than refilled with that many calls.
const MOST_PIECES: usize = 4096;
/// How close to 1 a rotated axis must be to count as one of the brick's.
const ALIGNED: f64 = 1.0 - 1e-5;

/// Each axis' colors (X, Y, Z), as linear values (see `view_modes`): unfocused, and where a hovered
/// size slider changes them. Free to change.
const COLORS: [[LinearColor; 2]; 3] = [
    [[0.85, 0.06, 0.05, 1.0], [1.0, 0.55, 0.5, 1.0]],
    [[0.08, 0.65, 0.05, 1.0], [0.6, 1.0, 0.5, 1.0]],
    [[0.06, 0.25, 1.0, 1.0], [0.55, 0.75, 1.0, 1.0]],
];

static ORIGINAL: OnceLock<AddConnectorFieldsFn> = OnceLock::new();
/// Set at engine init; until then (or if that fails) the hook changes nothing.
static CODE: OnceLock<Code> = OnceLock::new();
/// The game's components whose markers were moved to axis components.
static SPLIT: Mutex<Vec<Split>> = Mutex::new(Vec::new());
/// Logs the first time markers are rearranged, and the first time they can't be read.
static FIRST_REARRANGED: Once = Once::new();
static FIRST_UNREAD: Once = Once::new();

struct Code {
    connectors: ConnectorsIsm,
    components: MeshComponents,
    textures: Textures,
}

/// A brick's marker component, emptied, and its axes' (X, Y, Z), once made.
struct Split {
    original: WeakObject,
    axes: [Option<WeakObject>; 3],
}

// SAFETY: weak pointers to game objects, only resolved on the game thread.
unsafe impl Send for Split {}

impl Feature for ConnectorMarkers {
    fn id(&self) -> &'static str {
        "connector_markers"
    }

    fn on_load(&self) -> LoadResult {
        let add = connectors_ism::ADD_CONNECTOR_FIELDS.resolve()?;
        // SAFETY: the target is the start of the function its signature names, and the detour has
        // that function's signature.
        unsafe {
            hook::install_typed(
                connectors_ism::ADD_CONNECTOR_FIELDS.name,
                add,
                add_detour as AddConnectorFieldsFn,
                &ORIGINAL,
            )?;
        }
        brick_editor::after_tick(after_tick)
    }

    fn on_engine_init(&self) {
        let found = (|| {
            Ok::<_, String>(Code {
                connectors: ConnectorsIsm::find()?,
                components: MeshComponents::find()?,
                textures: Textures::find()?,
            })
        })();
        match found {
            Ok(code) => {
                let _ = CODE.set(code);
                log!("active");
            }
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

/// Lets the game add a brick's markers to `component`, then rearranges them.
unsafe extern "C" fn add_detour(
    component: *mut UObjectBase,
    fields: *const c_void,
    focused_axes: u8,
    brick_size: *const Vector,
) {
    let original = *ORIGINAL.get().expect("hook recorded before use");
    // SAFETY: the game's call, passed on; `component` is the live marker component it filled, on
    // the game thread.
    unsafe {
        original(component, fields, focused_axes, brick_size);
        if let Some(code) = CODE.get() {
            code.rearrange(component);
        }
    }
}

/// Destroys the axis components of game components that no longer show (see [`Code::shown`]),
/// and of all of them once [`AXIS_COLORS`] is off (their markers back in the game's component).
fn after_tick(_editor: *mut UObjectBase, _delta: f32) {
    let Some(code) = CODE.get() else { return };
    let colored = AXIS_COLORS.get();
    let done: Vec<Split> = {
        let mut split = SPLIT.lock().unwrap_or_else(PoisonError::into_inner);
        let (done, kept) = split.drain(..).partition(|split| {
            // SAFETY: on the game thread (the editor's tick).
            !colored || unsafe { code.shown(split) }.is_none()
        });
        *split = kept;
        done
    };
    for split in done {
        // SAFETY: on the game thread (the editor's tick); every component is checked alive
        // through its weak pointer first.
        unsafe { code.unsplit(split, true) };
    }
}

impl Code {
    /// Cuts `component`'s long instances into pieces, and moves them all to its axes' components
    /// with [`AXIS_COLORS`]. Leaves it as it is if its instances can't be read.
    ///
    /// # Safety
    /// `component` must be a live marker component the game just filled. Game thread only.
    unsafe fn rearrange(&self, component: *mut UObjectBase) {
        // SAFETY: per the contract.
        unsafe {
            if !self.connectors.is_connectors(component) {
                return;
            }
            let Some(instances) = self.connectors.instances(component) else {
                FIRST_UNREAD.call_once(|| log!("markers left as they are: unexpected instances"));
                return;
            };
            let pieces: Vec<Instance> = instances.iter().flat_map(|&one| pieces(one)).collect();
            if !AXIS_COLORS.get() {
                if pieces.len() != instances.len() {
                    self.connectors.clear(component);
                    self.connectors.add(component, &pieces);
                }
                return;
            }
            let mut split = take_split(component).unwrap_or_else(|| Split {
                // SAFETY: a live component.
                original: WeakObject::new(component),
                axes: [None; 3],
            });
            let mut by_axis: [Vec<Instance>; 3] = Default::default();
            for piece in pieces.iter() {
                by_axis[facing(piece)].push(*piece);
            }
            let mut placed = true;
            for (axis, instances) in by_axis.iter().enumerate() {
                let Some(target) = self.axis_component(component, &mut split, axis) else {
                    placed = false;
                    break;
                };
                self.connectors.clear(target);
                placed &= self.connectors.add(target, instances);
                self.color(target, axis);
            }
            if placed {
                self.connectors.clear(component);
                FIRST_REARRANGED.call_once(|| {
                    let counts = by_axis.each_ref().map(Vec::len);
                    log!(
                        "markers by axis: {} instances, {} pieces (X, Y, Z: {counts:?})",
                        instances.len(),
                        pieces.len()
                    );
                });
                put_split(split);
            } else {
                // Back to the game's component (which still has the game's instances), colors and
                // all.
                log!("markers stay in one component: an axis component couldn't be made");
                self.unsplit(split, false);
                if pieces.len() != instances.len() {
                    self.connectors.clear(component);
                    self.connectors.add(component, &pieces);
                }
            }
        }
    }

    /// The component of `component`'s markers facing `axis`, made if needed: of the same class
    /// (so the same mesh, material and settings), attached to it.
    ///
    /// # Safety
    /// `component` must be a live marker component. Game thread only.
    unsafe fn axis_component(
        &self,
        component: *mut UObjectBase,
        split: &mut Split,
        axis: usize,
    ) -> Option<*mut UObjectBase> {
        if let Some(made) = split.axes[axis].and_then(|made| made.get_live()) {
            return Some(made);
        }
        // SAFETY: per the contract; the class is a live component class.
        unsafe {
            let class = reflection::class_of(component.cast()).cast();
            let made = self.components.create_of(class, component, [1.0; 3])?;
            if !self.connectors.is_connectors(made) {
                self.components.destroy(made);
                return None;
            }
            split.axes[axis] = Some(WeakObject::new(made));
            Some(made)
        }
    }

    /// Gives `component`'s markers the colors of `axis`.
    ///
    /// # Safety
    /// `component` must be a live marker component. Game thread only.
    unsafe fn color(&self, component: *mut UObjectBase, axis: usize) {
        // SAFETY: per the contract; the material is null or the component's live dynamic
        // material instance.
        unsafe {
            let material = self.connectors.material(component);
            if material.is_null() {
                return;
            }
            let [color, focused] = COLORS[axis];
            self.textures.set_color(material, "Color", color);
            self.textures.set_color(material, "FocusedColor", focused);
        }
    }

    /// The game's component of `split`, while it shows its brick's markers: live and attached. A
    /// brick that stops showing them gets its component destroyed, or (in the editor) detached and
    /// kept for another brick; either way, its axis components (attached to it) would stay where
    /// it was.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn shown(&self, split: &Split) -> Option<*mut UObjectBase> {
        split
            .original
            .get_live()
            // SAFETY: per the contract; a live marker component.
            .filter(|&original| unsafe { self.connectors.is_attached(original) })
    }

    /// Destroys `split`'s axis components, first putting their markers `back` in the game's
    /// component (if it still shows them).
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn unsplit(&self, split: Split, back: bool) {
        // SAFETY: per the contract.
        let original = unsafe { self.shown(&split) }.filter(|_| back);
        for made in split.axes.iter().flatten().filter_map(WeakObject::get_live) {
            // SAFETY: per the contract; both are live marker components.
            unsafe {
                if let Some(original) = original
                    && let Some(instances) = self.connectors.instances(made)
                {
                    self.connectors.add(original, &instances);
                }
                self.components.destroy(made);
            }
        }
    }
}

/// Takes the split of `component` out of [`SPLIT`], if it has one.
fn take_split(component: *mut UObjectBase) -> Option<Split> {
    let mut split = SPLIT.lock().unwrap_or_else(PoisonError::into_inner);
    let index = split
        .iter()
        .position(|split| split.original.is(component) && split.original.get_live().is_some())?;
    Some(split.swap_remove(index))
}

fn put_split(split: Split) {
    SPLIT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(split);
}

/// The axis a marker instance faces (0 to 2: X, Y, Z of its brick): its quad's normal, its Z.
fn facing(instance: &Instance) -> usize {
    let normal = Quat::widen(instance.transform.rotation).rotate([0.0, 0.0, 1.0]);
    let size = normal.map(f64::abs);
    (0..3)
        .max_by(|&a, &b| size[a].total_cmp(&size[b]))
        .unwrap_or(2)
}

/// `instance` cut into pieces at whole cells, so that no piece spans more than [`MOST_CELLS`] (or
/// [`MOST_SLANTED_CELLS`]) along a side; itself if it doesn't.
fn pieces(instance: Instance) -> Vec<Instance> {
    let spacing = KIND_SPACING_CM / f64::from(instance.data[0]);
    if !spacing.is_finite() || spacing <= 0.0 {
        return vec![instance];
    }
    let transform = instance.transform;
    let rotation = Quat::widen(transform.rotation);
    let sides = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]].map(|side| rotation.rotate(side));
    let aligned = [[0.0, 0.0, 1.0]]
        .into_iter()
        .chain(sides)
        .all(|axis| axis.iter().any(|value| value.abs() >= ALIGNED));
    let most = if aligned {
        MOST_CELLS
    } else {
        MOST_SLANTED_CELLS
    };
    let scale = [0, 1].map(|side| f64::from(transform.scale[side]));
    let cells = scale.map(|scale| (scale.abs() * MESH_CM / spacing).round().max(1.0) as u32);
    if cells.iter().all(|&cells| cells <= most) {
        return vec![instance];
    }
    let [runs_x, runs_y] = cells.map(|cells| runs(cells, most));
    if runs_x.len() * runs_y.len() > MOST_PIECES {
        return vec![instance];
    }
    let middle = math::widen([
        transform.translation[0],
        transform.translation[1],
        transform.translation[2],
    ]);
    let mut pieces = Vec::with_capacity(runs_x.len() * runs_y.len());
    for &(offset_x, cells_x) in &runs_x {
        for &(offset_y, cells_y) in &runs_y {
            let offset = math::add(
                math::scale(sides[0], offset_x * spacing),
                math::scale(sides[1], offset_y * spacing),
            );
            let [x, y, z] = math::narrow(math::add(middle, offset));
            let side_scale = |side: usize, cells: u32| {
                (scale[side].signum() * f64::from(cells) * spacing / MESH_CM) as f32
            };
            let mut piece = instance;
            piece.transform.translation = [x, y, z, 0.0];
            piece.transform.scale[0] = side_scale(0, cells_x);
            piece.transform.scale[1] = side_scale(1, cells_y);
            pieces.push(piece);
        }
    }
    pieces
}

/// `cells` in a row cut into the fewest runs of at most `most`, as even as whole cells allow: each
/// run's middle (in cells from the row's middle) and length.
fn runs(cells: u32, most: u32) -> Vec<(f64, u32)> {
    let (cells, most) = (u64::from(cells), u64::from(most.max(1)));
    let count = cells.div_ceil(most).max(1);
    (0..count)
        .map(|run| {
            let start = cells * run / count;
            let end = cells * (run + 1) / count;
            let middle = (start + end) as f64 / 2.0 - cells as f64 / 2.0;
            (middle, (end - start) as u32)
        })
        .collect()
}
