//! The references shown in the editor: mesh components of the editor's own actor, attached to its
//! root (the vehicle's origin), one per image and one per piece of a model.
//!
//! [`Scene::sync`] makes what's shown match the sidecar: a reference whose file or settings changed
//! is built again; one that only moved, was resized (an image's width) or changed its opacity keeps
//! its meshes, and its components are moved or get other materials (as when the setting
//! [`looks::SEE_THROUGH_LOOK`] changes, or the Hide key hides it). Images are built right away (a
//! texture import and two triangles); models are read on a worker thread ([`loading`]), then built a
//! few pieces per frame ([`Scene::pump`]), so a big model appears over several frames instead of
//! freezing one.
//!
//! Each reference keeps its triangles ([`Shape`]) for clicks ([`Scene::pick`]), and the selected ones
//! are outlined like selected bricks ([`Scene::select`]). See-through materials don't draw into the
//! custom depth the outline comes from (translucent ones), or draw a dotted one (dithered ones), so a
//! selected see-through reference gets a copy of each of its meshes drawing nothing but that
//! ([`Shown::update_outline`]), and its pieces are outlined only while opaque.
//!
//! A reference's color multiplies its picture or its parts' colors. Most materials take it as a
//! parameter, changed in place; lit textured parts have none, so their texture is multiplied and
//! imported again ([`TEXTURE_INTERVAL`] apart at the most, while a color is dragged).
//!
//! The models' triangles, placed, are also what the "Reference fit" view mode measures bricks
//! against ([`Scene::fit_triangles`]).

use std::borrow::Cow;
use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use brickrust::ue::coreuobject::UObjectBase;
use model_io::Part;
use model_io::image::{self, srgb_to_linear};
use model_io::pick::{self, Ray, Shape};
use model_io::plane::{self, PlaneLayout};
use model_io::sidecar::{Kind, Reference};

use super::Code;
use super::loading::{self, Loaded};
use super::looks::{self, Looks};
use super::vehicle::Stamp;
use crate::game::editor_object::Transform;
use crate::game::mesh_builder::Triangles;
use crate::game::outline::{Look, OutlineState};
use crate::game::weak_object::WeakObject;
use crate::math::Quat;
use crate::prelude::*;

/// How long model pieces may take to build per frame (at least one is built each frame).
pub const BUILD_BUDGET: Duration = Duration::from_millis(4);

/// The least time between two multiplications of a model's textures by a changing color: each
/// decodes and imports them again.
const TEXTURE_INTERVAL: Duration = Duration::from_millis(250);

/// The color of model parts without a material.
const DEFAULT_COLOR: [f32; 4] = [0.6, 0.6, 0.6, 1.0];

/// How components look to the outline when not selected: not outlined.
const NOT_OUTLINED: Look = Look {
    outlined: false,
    stencil: 0,
};

/// A reference as the sidecar wants it, with its file found.
pub struct Wanted {
    pub reference: Reference,
    pub file: Option<(PathBuf, Stamp)>,
    /// Hidden by the Hide key: shown like hidden bricks, and can't be clicked.
    pub hidden: bool,
    /// False while all references are hidden (Show references): not shown at all.
    pub shown: bool,
}

impl Wanted {
    /// Everything that needs a new build when it changes (the file, its settings), as text. Not an
    /// image's width (a scale) or the opacity (materials).
    fn key(&self) -> String {
        let reference = &self.reference;
        let image = reference.image.clone().map(|mut image| {
            image.width = 0.0;
            image
        });
        format!(
            "{:?} {:?} {:?} {:?}",
            reference.kind, image, reference.import, self.file
        )
    }

    fn placement(&self, built_width: f64) -> Placement {
        let reference = &self.reference;
        let size = match &reference.image {
            Some(image) if built_width > 0.0 => image.width / built_width,
            _ => 1.0,
        };
        Placement {
            position: reference.position,
            rotation: reference.rotation,
            scale: reference.scale,
            mirror: reference.mirror,
            size,
            visible: reference.visible && self.opacity() > 0.0 && self.shown,
        }
    }

    /// The color that multiplies the reference's colors: linear RGB, each 0 to 1.
    fn tint(&self) -> [f32; 3] {
        self.reference.color.map(|value| {
            if value.is_nan() {
                1.0
            } else {
                srgb_to_linear(value.clamp(0.0, 1.0))
            }
        })
    }

    /// The opacity, 0 to 1.
    fn opacity(&self) -> f32 {
        let opacity = self.reference.opacity;
        if opacity.is_nan() {
            1.0
        } else {
            opacity.clamp(0.0, 1.0)
        }
    }
}

/// Where a reference is and whether it's shown: changes that don't need a new build.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Placement {
    position: [f64; 3],
    rotation: [f64; 3],
    scale: [f64; 3],
    mirror: [bool; 3],
    /// An image's width over the width it was built at.
    size: f64,
    visible: bool,
}

impl Placement {
    /// The scale along each axis: mirrored axes scale by -1 (the engine flips the triangles'
    /// winding itself for that).
    fn full_scale(&self) -> [f64; 3] {
        core::array::from_fn(|axis| {
            let sign = if self.mirror[axis] { -1.0 } else { 1.0 };
            self.scale[axis] * sign * self.size
        })
    }

    /// The components' transform relative to the editor's root.
    fn transform(&self) -> Transform {
        let rotation = Quat::from_rotator(self.rotation);
        let [sx, sy, sz] = self.full_scale().map(|value| value as f32);
        let [x, y, z] = self.position.map(|value| value as f32);
        Transform {
            rotation: [rotation.x, rotation.y, rotation.z, rotation.w].map(|value| value as f32),
            translation: [x, y, z, 0.0],
            scale: [sx, sy, sz, 0.0],
        }
    }
}

/// A component of a reference, its mesh, and the index of its material in [`Shown::materials`]
/// (images: always 0, their one material).
struct Piece {
    component: WeakObject,
    mesh: WeakObject,
    material: usize,
}

/// A reference shown (or being built).
struct Shown {
    id: String,
    name: String,
    key: String,
    kind: Kind,
    placement: Placement,
    opacity: f32,
    /// The color multiplying the reference's colors ([`Wanted::tint`]).
    tint: [f32; 3],
    /// When a model's textures were last multiplied by a color ([`TEXTURE_INTERVAL`]).
    textured_at: Option<Instant>,
    /// Hidden by the Hide key ([`Wanted::hidden`]).
    hidden: bool,
    /// Shown by its own settings, whatever the Show references key says.
    own_visible: bool,
    /// An image's width when built: its size scales from it.
    built_width: f64,
    pieces: Vec<Piece>,
    /// For a model: its materials, then one for parts without. For an image: its picture.
    materials: Vec<Material>,
    /// Its triangles, for clicks.
    shape: Shape,
    selected: bool,
    /// While selected and see-through: components drawing only its outline, one per piece, and
    /// how many pieces there were when they were made.
    outline: Vec<WeakObject>,
    outlined_pieces: Option<usize>,
    /// For a model being built: when its read arrived, and how many pieces are left.
    building: Option<(Instant, usize)>,
}

/// A material, made when a piece first needs it, and again when it needs another kind of material
/// ([`Appearance`]). Weak: nothing holds a material until a mesh uses it, so the engine may free it
/// in between, and it's made again.
struct Material {
    color: [f32; 4],
    /// A model part's color in the lit see-through material (its texture's average color).
    see_through_color: [f32; 4],
    /// A model part's texture (PNG bytes).
    texture: Option<Vec<u8>>,
    /// An image's picture, imported once.
    picture: Option<WeakObject>,
    /// The material made, and its kind.
    made: Option<(WeakObject, Appearance)>,
    /// The color the material made had its texture multiplied by (it has no color parameter if
    /// lit and opaque).
    textured_with: [f32; 3],
}

/// The kind of material a reference's part shows with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Appearance {
    /// Opaque: an image's picture, a model part's color or texture.
    Opaque,
    /// See-through: an image's picture, a model part's lit see-through color.
    SeeThrough,
    /// A textured model part see-through, its texture unlit (the setting
    /// [`looks::SEE_THROUGH_LOOK`]).
    UnlitTexture,
    /// A model part see-through, dithered (the setting [`looks::SEE_THROUGH_LOOK`]).
    Dithered,
    /// Hidden by the Hide key: the hidden bricks' look.
    Hidden,
}

/// A model piece waiting to be built.
struct Build {
    id: String,
    key: String,
    triangles: Triangles,
    /// Index into its reference's [`Shown::materials`].
    material: usize,
}

pub struct Scene {
    shown: Vec<Shown>,
    builds: VecDeque<Build>,
    /// [`looks::see_through_look`] when the materials were last made.
    see_through_look: looks::SeeThroughLook,
}

// SAFETY: weak pointers and materials kept alive by the meshes using them; only used on the game
// thread.
unsafe impl Send for Scene {}

impl Scene {
    pub const fn new() -> Self {
        Self {
            shown: Vec::new(),
            builds: VecDeque::new(),
            see_through_look: looks::SeeThroughLook::Smooth,
        }
    }

    /// Removes everything shown.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn clear(&mut self, code: &Code) {
        for shown in self.shown.drain(..) {
            // SAFETY: per the contract.
            unsafe { destroy(code, &shown) };
        }
        self.builds.clear();
    }

    /// Makes what's shown match `wanted` (see the module's comment); model reads report to `loads`.
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    pub unsafe fn sync(
        &mut self,
        code: &Code,
        looks: &Looks,
        editor: *mut UObjectBase,
        wanted: &[Wanted],
        loads: &Sender<Loaded>,
    ) {
        let keys: Vec<(&str, String)> = wanted
            .iter()
            .map(|wanted| (wanted.reference.id.as_str(), wanted.key()))
            .collect();
        let still_wanted = |id: &str, key: &str| {
            keys.iter()
                .any(|(other, other_key)| *other == id && other_key == key)
        };
        self.shown.retain(|shown| {
            let keep = still_wanted(&shown.id, &shown.key);
            if !keep {
                // SAFETY: per the contract.
                unsafe { destroy(code, shown) };
            }
            keep
        });
        self.builds
            .retain(|build| still_wanted(&build.id, &build.key));
        if self.see_through_look != looks::see_through_look() {
            self.see_through_look = looks::see_through_look();
            for shown in &mut self.shown {
                // SAFETY: per the contract.
                unsafe { shown.update_materials(code, looks, editor) };
            }
        }

        for (wanted, (_, key)) in wanted.iter().zip(&keys) {
            if let Some(shown) = self
                .shown
                .iter_mut()
                .find(|shown| shown.id == wanted.reference.id)
            {
                let placement = wanted.placement(shown.built_width);
                if shown.placement != placement {
                    shown.placement = placement;
                    // SAFETY: per the contract.
                    unsafe { place_all(code, shown) };
                }
                if shown.opacity != wanted.opacity() {
                    // SAFETY: per the contract.
                    unsafe { shown.set_opacity(code, looks, editor, wanted.opacity()) };
                }
                if shown.tint != wanted.tint() {
                    // SAFETY: per the contract.
                    unsafe { shown.set_tint(code, looks, editor, wanted.tint()) };
                }
                if shown.hidden != wanted.hidden {
                    shown.hidden = wanted.hidden;
                    // SAFETY: per the contract.
                    unsafe { shown.update_materials(code, looks, editor) };
                }
                shown.own_visible = wanted.reference.visible;
                continue;
            }
            let mut shown = Shown {
                id: wanted.reference.id.clone(),
                name: wanted.reference.name.clone(),
                key: key.clone(),
                kind: wanted.reference.kind,
                placement: wanted.placement(0.0),
                opacity: wanted.opacity(),
                tint: wanted.tint(),
                textured_at: None,
                hidden: wanted.hidden,
                own_visible: wanted.reference.visible,
                built_width: 0.0,
                pieces: Vec::new(),
                materials: Vec::new(),
                shape: Shape::new(),
                selected: false,
                outline: Vec::new(),
                outlined_pieces: None,
                building: None,
            };
            match (&wanted.file, wanted.reference.kind) {
                (None, _) => log!(
                    "{}: file not found ({})",
                    shown.name,
                    wanted.reference.source.path
                ),
                (Some((path, _)), Kind::Image) => {
                    // SAFETY: per the contract.
                    unsafe { shown.show_image(code, looks, editor, &wanted.reference, path) };
                    shown.placement = wanted.placement(shown.built_width);
                    // SAFETY: per the contract.
                    unsafe { place_all(code, &shown) };
                }
                (Some((path, _)), Kind::Model) => loading::start(
                    shown.id.clone(),
                    key.clone(),
                    path.clone(),
                    wanted.reference.import.unwrap_or_default(),
                    loads.clone(),
                ),
            }
            self.shown.push(shown);
        }
    }

    /// Takes a finished model read: queues its pieces.
    pub fn loaded(&mut self, loaded: Loaded) {
        let Some(shown) = self
            .shown
            .iter_mut()
            .find(|shown| shown.id == loaded.id && shown.key == loaded.key)
        else {
            return;
        };
        let model = match loaded.result {
            Ok(model) => model,
            Err(reason) => {
                log!("{}: {reason}", shown.name);
                return;
            }
        };
        log!(
            "{}: {} triangles in {} pieces, read in {:?}",
            shown.name,
            model.triangles,
            model.pieces.len(),
            model.read_time
        );
        shown.materials = model
            .materials
            .into_iter()
            .map(|material| Material {
                see_through_color: material.see_through_color,
                ..Material::new(material.color, material.texture)
            })
            .chain([Material::new(DEFAULT_COLOR, None)])
            .collect();
        let default = shown.materials.len() - 1;
        shown.building = Some((Instant::now(), model.pieces.len()));
        for piece in &model.pieces {
            let material = piece
                .material
                .filter(|&index| index < default)
                .unwrap_or(default);
            self.builds.push_back(Build {
                id: shown.id.clone(),
                key: shown.key.clone(),
                triangles: triangles(piece),
                material,
            });
        }
    }

    /// Builds queued model pieces for up to [`BUILD_BUDGET`] (at least one). Whether a model
    /// finished building (its size is known now).
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    pub unsafe fn pump(&mut self, code: &Code, looks: &Looks, editor: *mut UObjectBase) -> bool {
        let mut finished = false;
        for shown in &mut self.shown {
            if shown.textures_outdated() {
                // SAFETY: per the contract.
                unsafe { shown.update_materials(code, looks, editor) };
            }
        }
        let started = Instant::now();
        while let Some(build) = self.builds.pop_front() {
            let Some(shown) = self
                .shown
                .iter_mut()
                .find(|shown| shown.id == build.id && shown.key == build.key)
            else {
                continue;
            };
            // SAFETY: per the contract; the material is used right away.
            let material = unsafe { shown.material(code, looks, editor, build.material) };
            // SAFETY: per the contract; the material is null or live (just checked or made).
            if let Some((component, mesh)) =
                unsafe { place(code, editor, &build.triangles, material) }
            {
                // SAFETY: a live component and mesh, just made.
                unsafe { shown.add_piece(code, component, mesh, build.material) };
                shown
                    .shape
                    .add(&build.triangles.positions, &build.triangles.corners);
            }
            if let Some((since, left)) = &mut shown.building {
                *left -= 1;
                if *left == 0 {
                    log!("{}: built in {:?}", shown.name, since.elapsed());
                    shown.building = None;
                    finished = true;
                }
            }
            if started.elapsed() >= BUILD_BUDGET {
                break;
            }
        }
        finished
    }

    /// The size of the box around the reference `id`'s triangles along its own axes, unscaled (its
    /// size at scale 1), in cm: once it's fully built.
    pub fn local_size(&self, id: &str) -> Option<[f64; 3]> {
        let shown = self.shown.iter().find(|shown| shown.id == id)?;
        if shown.building.is_some() {
            return None;
        }
        let [min, max] = shown.shape.bounds()?;
        Some(core::array::from_fn(|axis| {
            f64::from(max[axis]) - f64::from(min[axis])
        }))
    }

    /// A number that changes whenever [`Self::fit_triangles`] would.
    pub fn fit_key(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        for shown in self.shown.iter().filter(|shown| shown.counts_for_fit()) {
            shown.id.hash(&mut hasher);
            shown.key.hash(&mut hasher);
            shown.shape.triangle_count().hash(&mut hasher);
            let placement = &shown.placement;
            for value in placement
                .position
                .iter()
                .chain(&placement.rotation)
                .chain(&placement.full_scale())
            {
                value.to_bits().hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    /// The triangles of the models the "Reference fit" view mode measures bricks against
    /// ([`Shown::counts_for_fit`]), where they're shown: in the editor root's space.
    pub fn fit_triangles(&self) -> Vec<[[f32; 3]; 3]> {
        let mut triangles = Vec::new();
        for shown in self.shown.iter().filter(|shown| shown.counts_for_fit()) {
            let placement = &shown.placement;
            let rotation = Quat::from_rotator(placement.rotation);
            let scale = placement.full_scale();
            // As the engine places the components: `parent = Rotate(scale * local) + position`.
            let place = |point: [f32; 3]| {
                let scaled = core::array::from_fn(|axis| f64::from(point[axis]) * scale[axis]);
                let turned = rotation.rotate(scaled);
                core::array::from_fn(|axis| (turned[axis] + placement.position[axis]) as f32)
            };
            triangles.extend(shown.shape.triangles().map(|triangle| triangle.map(place)));
        }
        triangles
    }

    /// The shown reference `ray` (in the space of the editor's root) meets first, and how far along
    /// it (in units of its direction's length). Hidden references are left out.
    pub fn pick(&self, ray: &Ray) -> Option<(&str, f64)> {
        self.first_hit(ray, |shown| shown.placement.visible && !shown.hidden)
    }

    /// The reference among those `counts` keeps that `ray` meets first, and how far along it.
    fn first_hit(&self, ray: &Ray, counts: impl Fn(&Shown) -> bool) -> Option<(&str, f64)> {
        self.shown
            .iter()
            .filter(|shown| counts(shown))
            .filter_map(|shown| {
                let placement = &shown.placement;
                let local = pick::local_ray(
                    ray,
                    placement.position,
                    placement.rotation,
                    placement.full_scale(),
                )?;
                Some((shown.id.as_str(), shown.shape.hit(&local)?))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// How wide the references `ids` are together along the vehicle's Y axis, roughly (their boxes,
    /// ignoring their rotations), if any is built.
    pub fn width_of(&self, ids: &[String]) -> Option<f64> {
        let (min, max) = self
            .shown
            .iter()
            .filter(|shown| ids.contains(&shown.id))
            .filter_map(|shown| {
                let [min, max] = shown.shape.bounds()?;
                let scale = shown.placement.full_scale()[1];
                let ends = [min[1], max[1]].map(|end| f64::from(end) * scale);
                let middle = shown.placement.position[1];
                Some((middle + ends[0].min(ends[1]), middle + ends[0].max(ends[1])))
            })
            .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))?;
        Some(max - min)
    }

    /// Outlines the references `selected` like selected bricks, and no others.
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    pub unsafe fn select(
        &mut self,
        code: &Code,
        looks: &Looks,
        editor: *mut UObjectBase,
        selected: &[String],
    ) {
        for shown in &mut self.shown {
            let selected = selected.contains(&shown.id);
            // SAFETY: per the contract.
            unsafe {
                if shown.selected != selected {
                    shown.selected = selected;
                    shown.outline_all(code);
                }
                shown.update_outline(code, looks, editor);
            }
        }
    }
}

impl Shown {
    /// Shows the image of `reference` at `path`: its texture on a rectangle.
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    unsafe fn show_image(
        &mut self,
        code: &Code,
        looks: &Looks,
        editor: *mut UObjectBase,
        reference: &Reference,
        path: &std::path::Path,
    ) {
        let settings = reference.image.clone().unwrap_or_default();
        let text = path.to_string_lossy().replace('\\', "/");
        // SAFETY: per the contract.
        unsafe {
            let started = Instant::now();
            let texture = code.textures.import_file(editor, &text);
            if texture.is_null() {
                log!("{}: the game can't read {text}", reference.name);
                return;
            }
            let [width, height] = code.textures.size(texture);
            log!(
                "{}: {width}x{height} image read in {:?}",
                reference.name,
                started.elapsed()
            );
            let part = plane::image_plane(&PlaneLayout {
                width: settings.width as f32,
                aspect: width as f32 / height.max(1) as f32,
                flip_x: settings.flip_x,
                flip_y: settings.flip_y,
                pivot: settings.pivot,
                double_sided: settings.double_sided,
            });
            self.built_width = settings.width;
            self.materials = vec![Material {
                picture: Some(WeakObject::new(texture)),
                ..Material::new([1.0; 4], None)
            }];
            let material = self.material(code, looks, editor, 0);
            let triangles = triangles(&part);
            if let Some((component, mesh)) = place(code, editor, &triangles, material) {
                self.add_piece(code, component, mesh, 0);
                self.shape.add(&triangles.positions, &triangles.corners);
            }
        }
    }

    /// The material for this reference's pieces using material `index` at its opacity, made now if
    /// needed (see [`Material`]); null if the engine makes none.
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    unsafe fn material(
        &mut self,
        code: &Code,
        looks: &Looks,
        editor: *mut UObjectBase,
        index: usize,
    ) -> *mut UObjectBase {
        let (kind, opacity, hidden, tint) = (self.kind, self.opacity, self.hidden, self.tint);
        let Some(material) = self.materials.get_mut(index) else {
            return core::ptr::null_mut();
        };
        let wanted = material.appearance(kind, opacity, hidden);
        if let Some(made) = material.made_as(wanted) {
            return made;
        }
        // SAFETY: per the contract.
        let made = unsafe {
            match kind {
                _ if wanted == Appearance::Hidden => looks.hidden(),
                Kind::Image => {
                    let picture = material.picture.and_then(|picture| picture.get());
                    match picture {
                        Some(picture) => {
                            looks.image(&code.textures, editor, picture, tint, opacity)
                        }
                        None => core::ptr::null_mut(),
                    }
                }
                Kind::Model => material.make_for_model(code, looks, editor, wanted, opacity, tint),
            }
        };
        material.textured_with = tint;
        material.made = (!made.is_null()).then(|| {
            // SAFETY: a live material, just made.
            (unsafe { WeakObject::new(made) }, wanted)
        });
        made
    }

    /// Keeps `component` (just made, showing `mesh`) as a piece using material `index`, placed,
    /// outlined and with the custom data a see-through look reads.
    ///
    /// # Safety
    /// `component` must be a live component of ours, `mesh` its live mesh. Game thread only.
    unsafe fn add_piece(
        &mut self,
        code: &Code,
        component: *mut UObjectBase,
        mesh: *mut UObjectBase,
        index: usize,
    ) {
        // SAFETY: per the contract.
        unsafe {
            if self.kind == Kind::Model
                && let Some(material) = self.materials.get(index)
            {
                code.components.set_custom_values(
                    component,
                    &looks::see_through_data(tinted(material.see_through_color, self.tint)),
                );
            }
            place_one(code, &self.placement, component);
            outline_one(code, self.outlines_pieces(), component);
            self.pieces.push(Piece {
                component: WeakObject::new(component),
                mesh: WeakObject::new(mesh),
                material: index,
            });
        }
    }

    /// Makes or removes the components drawing only the outline (see the module's comment): wanted
    /// while selected and see-through, one per piece (made again when pieces were added since).
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    unsafe fn update_outline(&mut self, code: &Code, looks: &Looks, editor: *mut UObjectBase) {
        let wanted = self.selected
            && !looks::is_opaque(self.opacity)
            && code.outlines.is_some()
            && code.components.can_outline_only();
        if wanted && self.outlined_pieces == Some(self.pieces.len()) {
            return;
        }
        if !wanted && self.outlined_pieces.is_none() {
            return;
        }
        // SAFETY: per the contract; every component and mesh used is live (just checked or made).
        unsafe {
            for copy in self.outline.drain(..) {
                if let Some(copy) = copy.get() {
                    code.components.destroy(copy);
                }
            }
            self.outlined_pieces = None;
            if !wanted {
                return;
            }
            self.outlined_pieces = Some(self.pieces.len());
            let root = code.root.call(editor);
            if root.is_null() {
                return;
            }
            // Any opaque material draws custom depth; an image's keeps its cut-outs.
            let material = match (self.kind, self.materials.first()) {
                (Kind::Image, Some(Material { picture, .. })) => {
                    match picture.and_then(|picture| picture.get()) {
                        Some(picture) => {
                            looks.image(&code.textures, editor, picture, [1.0; 3], 1.0)
                        }
                        None => core::ptr::null_mut(),
                    }
                }
                _ => looks.plain(&code.textures, editor, DEFAULT_COLOR),
            };
            for piece in &self.pieces {
                let Some(mesh) = piece.mesh.get() else {
                    continue;
                };
                let Some(copy) = code.components.create(root, mesh, [1.0; 3]) else {
                    continue;
                };
                code.components.set_material(copy, 0, material);
                code.components.outline_only(copy);
                place_one(code, &self.placement, copy);
                outline_one(code, true, copy);
                self.outline.push(WeakObject::new(copy));
            }
        }
    }

    /// Shows the reference at `opacity` ([`Self::update_materials`]).
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    unsafe fn set_opacity(
        &mut self,
        code: &Code,
        looks: &Looks,
        editor: *mut UObjectBase,
        opacity: f32,
    ) {
        let was_opaque = looks::is_opaque(self.opacity);
        self.opacity = opacity;
        // SAFETY: per the contract.
        unsafe {
            self.update_materials(code, looks, editor);
            if was_opaque != looks::is_opaque(opacity) {
                self.outline_all(code);
            }
        }
    }

    /// Shows the reference multiplied by the color `tint` ([`Self::update_materials`]).
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    unsafe fn set_tint(
        &mut self,
        code: &Code,
        looks: &Looks,
        editor: *mut UObjectBase,
        tint: [f32; 3],
    ) {
        self.tint = tint;
        // SAFETY: per the contract; the components used are live (just checked).
        unsafe {
            if self.kind == Kind::Model {
                for piece in &self.pieces {
                    if let (Some(component), Some(material)) =
                        (piece.component.get(), self.materials.get(piece.material))
                    {
                        let data =
                            looks::see_through_data(tinted(material.see_through_color, tint));
                        code.components.set_custom_values(component, &data);
                    }
                }
            }
            self.update_materials(code, looks, editor);
        }
    }

    /// Whether some lit textured part still shows its texture multiplied by an older color, to be
    /// made again once [`TEXTURE_INTERVAL`] has passed ([`Self::update_materials`]).
    fn textures_outdated(&self) -> bool {
        self.kind == Kind::Model
            && self.materials.iter().any(|material| {
                material.texture.is_some()
                    && material.textured_with != self.tint
                    && material.made_as(Appearance::Opaque).is_some()
            })
    }

    /// Gives the pieces the materials the reference's opacity and the settings ask for: a material
    /// whose kind changed (or whose texture needs its color) is made again and given to its
    /// pieces, the others get the opacity and color.
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    unsafe fn update_materials(&mut self, code: &Code, looks: &Looks, editor: *mut UObjectBase) {
        let (kind, opacity, hidden, tint) = (self.kind, self.opacity, self.hidden, self.tint);
        for index in 0..self.materials.len() {
            // Unused yet: made when a piece needs it.
            if !self.pieces.iter().any(|piece| piece.material == index) {
                continue;
            }
            let wanted = self.materials[index].appearance(kind, opacity, hidden);
            let color = tinted(self.materials[index].color, tint);
            let textured = self.materials[index].texture.is_some();
            // SAFETY: per the contract; the materials and components used are live (just checked
            // or made).
            unsafe {
                if let Some(made) = self.materials[index].made_as(wanted) {
                    match (kind, wanted) {
                        (_, Appearance::Hidden) => continue,
                        (Kind::Image, _) | (Kind::Model, Appearance::UnlitTexture) => {
                            looks.set_image_tint(&code.textures, made, tint, opacity);
                            continue;
                        }
                        (Kind::Model, Appearance::Opaque) if !textured => {
                            Looks::set_plain_color(&code.textures, made, color);
                            continue;
                        }
                        (Kind::Model, Appearance::Opaque) => {
                            // The texture is multiplied, which takes a while: not at every step of
                            // a color being dragged.
                            let recent = self
                                .textured_at
                                .is_some_and(|at| at.elapsed() < TEXTURE_INTERVAL);
                            if self.materials[index].textured_with == tint || recent {
                                continue;
                            }
                            self.textured_at = Some(Instant::now());
                        }
                        (Kind::Model, Appearance::SeeThrough) => {
                            Looks::set_see_through_opacity(&code.textures, made, opacity);
                            continue;
                        }
                        (Kind::Model, Appearance::Dithered) => {
                            Looks::set_dithered_opacity(&code.textures, made, opacity);
                            Looks::set_dithered_color(&code.textures, made, color);
                            continue;
                        }
                    }
                    self.materials[index].made = None;
                }
                let made = self.material(code, looks, editor, index);
                for piece in self.pieces.iter().filter(|piece| piece.material == index) {
                    if let Some(component) = piece.component.get() {
                        code.components.set_material(component, 0, made);
                    }
                }
            }
        }
    }

    /// Whether the "Reference fit" view mode measures against it: a model, fully built, shown by
    /// its own settings (whatever the Show references key says) and not hidden by the Hide key.
    fn counts_for_fit(&self) -> bool {
        matches!(self.kind, Kind::Model)
            && self.own_visible
            && self.opacity > 0.0
            && !self.hidden
            && self.building.is_none()
            && !self.shape.is_empty()
    }

    /// Whether the pieces themselves are outlined: while selected and opaque (see the module's
    /// comment).
    fn outlines_pieces(&self) -> bool {
        self.selected && looks::is_opaque(self.opacity)
    }

    /// Gives every piece the outline look its selection and opacity ask for.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn outline_all(&self, code: &Code) {
        for piece in &self.pieces {
            if let Some(component) = piece.component.get() {
                // SAFETY: per the contract; a live component of ours.
                unsafe { outline_one(code, self.outlines_pieces(), component) };
            }
        }
    }
}

impl Material {
    fn new(color: [f32; 4], texture: Option<Vec<u8>>) -> Self {
        Self {
            color,
            see_through_color: color,
            texture,
            picture: None,
            made: None,
            textured_with: [1.0; 3],
        }
    }

    /// The kind of material this one should be for a reference of `kind` at `opacity`, `hidden` by
    /// the Hide key or not.
    fn appearance(&self, kind: Kind, opacity: f32, hidden: bool) -> Appearance {
        use looks::SeeThroughLook;
        if hidden {
            return Appearance::Hidden;
        }
        if looks::is_opaque(opacity) {
            return Appearance::Opaque;
        }
        match (kind, looks::see_through_look()) {
            (Kind::Image, _) => Appearance::SeeThrough,
            (Kind::Model, SeeThroughLook::Dithered) => Appearance::Dithered,
            (Kind::Model, SeeThroughLook::SmoothUnlit) if self.texture.is_some() => {
                Appearance::UnlitTexture
            }
            (Kind::Model, _) => Appearance::SeeThrough,
        }
    }

    /// The material made, if it's of the kind `wanted` and still alive.
    fn made_as(&self, wanted: Appearance) -> Option<*mut UObjectBase> {
        let (made, appearance) = self.made?;
        (appearance == wanted).then(|| made.get()).flatten()
    }

    /// A new material for a model part, of the kind `wanted`, at `opacity` and multiplied by the
    /// color `tint`: lit, its color or texture when opaque; see-through dithered, lit in its color,
    /// or its texture unlit (the opaque look if those can't be had).
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    unsafe fn make_for_model(
        &self,
        code: &Code,
        looks: &Looks,
        editor: *mut UObjectBase,
        wanted: Appearance,
        opacity: f32,
        tint: [f32; 3],
    ) -> *mut UObjectBase {
        let color = tinted(self.color, tint);
        // SAFETY: per the contract.
        unsafe {
            // The looks that take the color as a parameter show the texture as it is.
            let texture = match (&self.texture, wanted) {
                (Some(png), Appearance::Dithered | Appearance::UnlitTexture) => {
                    code.textures.import_buffer(editor, png)
                }
                _ => core::ptr::null_mut(),
            };
            let made = match wanted {
                Appearance::Opaque | Appearance::Hidden => core::ptr::null_mut(),
                Appearance::Dithered => {
                    looks.dithered(&code.textures, editor, texture, color, opacity)
                }
                Appearance::SeeThrough => looks.see_through(&code.textures, editor, opacity),
                Appearance::UnlitTexture if texture.is_null() => core::ptr::null_mut(),
                Appearance::UnlitTexture => {
                    looks.image(&code.textures, editor, texture, tint, opacity)
                }
            };
            if !made.is_null() {
                return made;
            }
            let texture = match &self.texture {
                Some(png) => code.textures.import_buffer(editor, &tinted_png(png, tint)),
                None => core::ptr::null_mut(),
            };
            if texture.is_null() {
                looks.plain(&code.textures, editor, color)
            } else {
                looks.textured(&code.textures, editor, texture)
            }
        }
    }
}

/// `color` (linear RGBA) multiplied by `tint` (linear RGB); alpha kept.
fn tinted(color: [f32; 4], tint: [f32; 3]) -> [f32; 4] {
    [
        color[0] * tint[0],
        color[1] * tint[1],
        color[2] * tint[2],
        color[3],
    ]
}

/// The PNG file `png` with its colors multiplied by `tint` (linear RGB): as it is for white or if
/// it can't be read.
fn tinted_png(png: &[u8], tint: [f32; 3]) -> Cow<'_, [u8]> {
    if tint == [1.0; 3] {
        return Cow::Borrowed(png);
    }
    image::decode(png)
        .and_then(|picture| picture.tinted(tint).to_png())
        .map_or(Cow::Borrowed(png), Cow::Owned)
}

/// The builder's triangles for `part`, which wants a UV per position.
fn triangles(part: &Part) -> Triangles {
    Triangles {
        positions: part.positions.clone(),
        uvs: if part.uvs.len() == part.positions.len() {
            part.uvs.clone()
        } else {
            vec![[0.0; 2]; part.positions.len()]
        },
        corners: part.corners.clone(),
    }
}

/// A new component of the editor showing `triangles` with `material`, at the vehicle's origin.
///
/// # Safety
/// `editor` must be the live editor, `material` null or a live material. Game thread only.
unsafe fn place(
    code: &Code,
    editor: *mut UObjectBase,
    triangles: &Triangles,
    material: *mut UObjectBase,
) -> Option<(*mut UObjectBase, *mut UObjectBase)> {
    // SAFETY: per the contract.
    unsafe {
        let root = code.root.call(editor);
        if root.is_null() {
            log!("the editor has no root component");
            return None;
        }
        let mesh = code.builder.build(outermost(editor), triangles, material)?;
        Some((code.components.create(root, mesh, [1.0; 3])?, mesh))
    }
}

/// The components of `shown` still alive: its pieces, then those drawing its outline.
fn components(shown: &Shown) -> impl Iterator<Item = *mut UObjectBase> + '_ {
    let pieces = shown.pieces.iter().map(|piece| piece.component);
    pieces
        .chain(shown.outline.iter().copied())
        .filter_map(|component| component.get())
}

/// Puts every component of `shown` where its placement says.
///
/// # Safety
/// Game thread only.
unsafe fn place_all(code: &Code, shown: &Shown) {
    for component in components(shown) {
        // SAFETY: per the contract; a live component of ours.
        unsafe { place_one(code, &shown.placement, component) };
    }
}

/// # Safety
/// `component` must be a live component of ours. Game thread only.
unsafe fn place_one(code: &Code, placement: &Placement, component: *mut UObjectBase) {
    // SAFETY: per the contract.
    unsafe {
        code.components
            .set_transform(component, placement.transform());
        code.components.set_visible(component, placement.visible);
    }
}

/// Outlines `component` like a selected brick, or not at all.
///
/// # Safety
/// `component` must be a live component of ours. Game thread only.
unsafe fn outline_one(code: &Code, selected: bool, component: *mut UObjectBase) {
    let Some(outlines) = &code.outlines else {
        return;
    };
    let look = if selected {
        NOT_OUTLINED.in_state(OutlineState::Selected)
    } else {
        NOT_OUTLINED
    };
    // SAFETY: per the contract.
    unsafe { outlines.set_look(component, look) };
}

/// Destroys the components of `shown` still alive.
///
/// # Safety
/// Game thread only.
unsafe fn destroy(code: &Code, shown: &Shown) {
    for component in components(shown) {
        // SAFETY: per the contract; a live component of ours.
        unsafe { code.components.destroy(component) };
    }
}

/// The package `object` is in.
///
/// # Safety
/// `object` must be a live object. Game thread only.
unsafe fn outermost(object: *mut UObjectBase) -> *mut UObjectBase {
    let mut current = object;
    // SAFETY: per the contract; a live object's outers are live.
    unsafe {
        while !(*current).outer_private.is_null() {
            current = (*current).outer_private.cast_mut();
        }
    }
    current
}
