//! Exporting the vehicle open in the editor as a 3D model: "Export 3D model" in the editor's
//! Editor tweaks menu (`editor_menu`), or its key ([`keybinds::EXPORT_MODEL`], none by default),
//! opens a dialog in the game's menu ([`DIALOG`], see `settings_menu`) with the export's settings.
//! Its Export button writes the model (`model_io::export`) to
//! `SavedRemastered\EditorTweaks\Exports`, named after the vehicle, and shows it in Explorer if
//! [`SHOW_EXPORTED`] is on.
//!
//! What's in it: the bricks [`BRICKS`] picks, each an object of its own named like the editor
//! names it (or one object per material, [`JOIN`]), with its shape as [`SHAPES`] says (see
//! [`shapes`]) and a material for its colour and brick material, textured like the game draws it
//! if [`EXPORT_TEXTURES`] is on (see [`looks`]), with its pattern; wheels' tyres in rubber
//! (`game::tyres`); text bricks' text (see [`text`]).
//! Collision has no materials or text. The model's origin is the middle of the vehicle's bottom.
//! glTF (`.glb`) and OBJ files are +Y up with the vehicle's front towards +Z, STL files +Z up with
//! its front towards +X; glTF counts in metres, OBJ and STL in [`UNITS`].
//!
//! The bricks are read on the game thread at the editor's next tick; the files are written on a
//! thread of their own, which reports back through a channel read at the following ticks.

mod looks;
mod shapes;
mod text;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Instant;

use brickrust::ue::coreuobject::UObjectBase;

use model_io::export::{Export, Object, glb, obj, stl};
use model_io::model::SMOOTH_ANGLE;

use super::editor_menu;
use super::keybinds;
use super::settings_menu::{self, Dialog};
use crate::framework::settings::{Alternative, Choice, Section, Setting, Toggle};
use crate::game::brick_editor;
use crate::game::brick_looks::{BrickLooks, BrickPatterns};
use crate::game::brick_model::BrickModels;
use crate::game::collision::Collision;
use crate::game::display_name::{self, DisplayNames};
use crate::game::editor_file::EditorFile;
use crate::game::editor_object::{self, LocalBounds, Selection, WorldTransforms};
use crate::game::editor_view::{self, Bricks, EditorCode};
use crate::game::kismet::{self, Texts};
use crate::game::spinner::Spinners;
use crate::game::spinner_profiles::SpinnerProfiles;
use crate::game::text_render::TextRenders;
use crate::game::texture_readback::Readback;
use crate::game::tyres::Tyres;
use crate::math;
use crate::prelude::*;
use looks::{Materials, Projection};
use shapes::{Kind, Shapes, Source, SpinnerLooks};
use text::TextParts;

/// Whether Explorer opens on the exported file, a setting.
pub static SHOW_EXPORTED: Toggle = Toggle::new(
    "ShowExportedModel",
    "Show exported models",
    "After exporting the vehicle as a 3D model, open its folder in Explorer with the file selected.",
    true,
);

/// Whether exported models get the brick materials' textures and text bricks' text, a setting.
/// Reading the textures makes the first export of a game take a moment longer.
pub static EXPORT_TEXTURES: Toggle = Toggle::new(
    "ExportModelTextures",
    "Export textures",
    "Give exported models the brick materials' textures (wood grain, metal, normal maps), bricks' \
     patterns and text bricks' text. The first export of a game takes a moment longer while \
     they're read.",
    true,
);

/// Which bricks go in the model, a setting: [`Scope`]'s options, in order.
pub static BRICKS: Choice = Choice::new(
    "ExportModelBricks",
    "Bricks",
    "Which bricks go in the model: the ones the editor shows (hidden ones left out), the selected \
     ones, or every brick, hidden ones too.",
    &[
        Alternative {
            key: "Shown",
            name: "Shown bricks",
        },
        Alternative {
            key: "Selected",
            name: "Selected bricks",
        },
        Alternative {
            key: "All",
            name: "Every brick",
        },
    ],
    0,
);

/// What each brick is in the model, a setting: its model at a level of detail, or its collision
/// ([`chosen_source`]).
pub static SHAPES: Choice = Choice::new(
    "ExportModelShapes",
    "Shapes",
    "What each brick is: its model as the game draws it up close, a less detailed version of it \
     (the game's levels of detail, drawn further away; bricks without them keep their model), or \
     its collision, the shapes physics uses (without materials).",
    &[
        Alternative {
            key: "Full",
            name: "Full detail",
        },
        Alternative {
            key: "Medium",
            name: "Less detail",
        },
        Alternative {
            key: "Low",
            name: "Low detail",
        },
        Alternative {
            key: "Lowest",
            name: "Lowest detail",
        },
        Alternative {
            key: "Collision",
            name: "Collision",
        },
    ],
    0,
);

/// Whether bricks are joined into one object per material, a setting.
pub static JOIN: Toggle = Toggle::new(
    "ExportModelJoin",
    "One object per material",
    "Join the bricks into one object per material instead of one object per brick: fewer, bigger \
     objects, quicker to work with in other programs.",
    false,
);

/// The file format, a setting: [`Format`]'s options, in order.
pub static FORMAT: Choice = Choice::new(
    "ExportModelFormat",
    "File format",
    "glTF (.glb) keeps the materials and textures and opens in Blender, Windows' 3D Viewer and \
     most engines. OBJ (.obj, with a .mtl and the textures next to it) is for programs without \
     glTF. STL (.stl) is shapes only, for 3D printing.",
    &[
        Alternative {
            key: "Glb",
            name: "glTF (.glb)",
        },
        Alternative {
            key: "Obj",
            name: "OBJ (.obj)",
        },
        Alternative {
            key: "Stl",
            name: "STL (.stl)",
        },
    ],
    0,
);

/// The unit OBJ and STL files count in, a setting ([`units_per_centimetre`]).
pub static UNITS: Choice = Choice::new(
    "ExportModelUnits",
    "Units (OBJ and STL)",
    "The unit OBJ and STL files count in (glTF files are always in metres). 3D printing slicers \
     expect millimetres.",
    &[
        Alternative {
            key: "Metres",
            name: "Metres",
        },
        Alternative {
            key: "Centimetres",
            name: "Centimetres",
        },
        Alternative {
            key: "Millimetres",
            name: "Millimetres",
        },
    ],
    0,
);

/// The export's settings, saved with the others but shown in [`DIALOG`] rather than the options.
pub static SETTINGS: [Section; 1] = [Section {
    title: "Model export",
    settings: &[],
    unlisted: &[
        Setting::Choice(&BRICKS),
        Setting::Choice(&SHAPES),
        Setting::Toggle(&JOIN),
        Setting::Toggle(&EXPORT_TEXTURES),
        Setting::Choice(&FORMAT),
        Setting::Choice(&UNITS),
        Setting::Toggle(&SHOW_EXPORTED),
    ],
}];

/// The dialog "Export 3D model" opens: the settings, and a button exporting with them.
static DIALOG: Dialog = Dialog {
    context: "ExportModel",
    title: "Export 3D model",
    category: "Properties.Settings.EditorTweaks.ModelExport",
    sections: &[
        Section {
            title: "Model",
            settings: &[
                Setting::Choice(&BRICKS),
                Setting::Choice(&SHAPES),
                Setting::Toggle(&JOIN),
                Setting::Toggle(&EXPORT_TEXTURES),
            ],
            unlisted: &[],
        },
        Section {
            title: "File",
            settings: &[
                Setting::Choice(&FORMAT),
                Setting::Choice(&UNITS),
                Setting::Toggle(&SHOW_EXPORTED),
            ],
            unlisted: &[],
        },
    ],
    button: "Export",
    icon: "/Game/BrickRigs/UI/Textures/Icons/T_SaveFileAs.T_SaveFileAs",
    action: || EXPORT.store(true, Ordering::Relaxed),
};

/// Which bricks are exported ([`BRICKS`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Shown,
    Selected,
    All,
}

impl Scope {
    fn chosen() -> Self {
        match BRICKS.get() {
            1 => Self::Selected,
            2 => Self::All,
            _ => Self::Shown,
        }
    }
}

/// What bricks are exported as ([`SHAPES`]).
fn chosen_source() -> Source {
    match SHAPES.get() {
        1 => Source::Model { level: 1 },
        2 => Source::Model { level: 2 },
        3 => Source::Model { level: usize::MAX },
        4 => Source::Collision,
        _ => Source::Model { level: 0 },
    }
}

/// The file format ([`FORMAT`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Format {
    Glb,
    Obj,
    Stl,
}

impl Format {
    fn chosen() -> Self {
        match FORMAT.get() {
            1 => Self::Obj,
            2 => Self::Stl,
            _ => Self::Glb,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Glb => "glb",
            Self::Obj => "obj",
            Self::Stl => "stl",
        }
    }
}

/// How many of [`UNITS`]' unit make a centimetre (the editor's unit).
fn units_per_centimetre() -> f32 {
    match UNITS.get() {
        1 => 1.0,
        2 => 10.0,
        _ => 0.01,
    }
}

pub struct ModelExport;

/// Set once everything is found; until then (or if that fails) nothing happens.
static GAME: OnceLock<Game> = OnceLock::new();
/// The menu entry was clicked: open the dialog at the editor's next tick.
static REQUESTED: AtomicBool = AtomicBool::new(false);
/// The dialog's Export button was clicked: export at the editor's next tick.
static EXPORT: AtomicBool = AtomicBool::new(false);
/// Whether the key was held at the last tick (a press exports once).
static KEY_HELD: AtomicBool = AtomicBool::new(false);
/// The export being written, if any.
static WRITING: Mutex<Option<Receiver<Result<Written, String>>>> = Mutex::new(None);

/// The game code this feature uses.
struct Game {
    bricks: Bricks,
    code: EditorCode,
    bounds: LocalBounds,
    world: WorldTransforms,
    looks: BrickLooks,
    /// Bricks' patterns (else they're left out).
    patterns: Option<BrickPatterns>,
    /// Bricks' meshes (else bricks are boxes).
    models: Option<BrickModels>,
    /// Spinners' collision pieces (else they're boxes), for those without [`Self::spinners`].
    collision: Option<Collision>,
    /// Spinners' values and cross-sections (else they're their collision pieces).
    spinners: Option<SpinnerLooks>,
    /// Textures read back (else looks have none, and there's no text).
    readback: Option<Readback>,
    /// Text bricks' text (else it's left out).
    texts: Option<TextRenders>,
    /// Wheels' tyre slots (else tyres show their wheel's look).
    tyres: Option<Tyres>,
    /// The vehicle's name, for the file's (else "Vehicle").
    file: Option<EditorFile>,
    /// Bricks' names, for the nodes' (else their kind and number).
    names: Option<(DisplayNames, Texts)>,
    /// Which bricks are selected (else the selected bricks can't be exported alone).
    selection: Option<Selection>,
}

impl Feature for ModelExport {
    fn id(&self) -> &'static str {
        "model_export"
    }

    fn on_load(&self) -> LoadResult {
        editor_menu::on_click(&keybinds::EXPORT_MODEL, || {
            REQUESTED.store(true, Ordering::Relaxed)
        });
        settings_menu::add_dialog(&DIALOG);
        brick_editor::after_tick(after_tick)
    }

    fn on_engine_init(&self) {
        match engine_init() {
            Ok(()) => log!("active: Export 3D model in the editor's Editor tweaks menu"),
            Err(reason) => log!("inactive: {reason}"),
        }
    }
}

fn engine_init() -> Result<(), String> {
    let names = display_name::find().and_then(|names| Ok((names, kismet::texts()?)));
    let game = Game {
        bricks: editor_view::bricks()?,
        code: editor_view::editor_code()?,
        bounds: LocalBounds::find()?,
        world: WorldTransforms::find()?,
        looks: BrickLooks::find()?,
        patterns: optional(
            BrickPatterns::find(),
            "models are exported without patterns",
        ),
        models: optional(BrickModels::find(), "bricks are exported as boxes"),
        collision: optional(Collision::find(), "spinners are exported as boxes"),
        spinners: optional(
            Spinners::find().and_then(|spinners| Ok((spinners, SpinnerProfiles::find()?))),
            "spinners are exported by their collision",
        ),
        readback: optional(Readback::find(), "models are exported without textures"),
        texts: optional(TextRenders::find(), "models are exported without text"),
        tyres: optional(Tyres::find(), "tyres are exported in their wheel's look"),
        file: optional(EditorFile::find(), "exports aren't named after the vehicle"),
        names: optional(
            names,
            "exported bricks aren't named like the editor names them",
        ),
        selection: optional(
            editor_object::selection(),
            "the selected bricks can't be exported alone",
        ),
    };
    let _ = GAME.set(game);
    Ok(())
}

/// `found`, or `None` after logging what goes `without` it.
fn optional<T>(found: Result<T, String>, without: &str) -> Option<T> {
    match found {
        Ok(found) => Some(found),
        Err(reason) => {
            log!("{without}: {reason}");
            None
        }
    }
}

fn after_tick(editor: *mut UObjectBase, _delta: f32) {
    let Some(game) = GAME.get() else {
        return;
    };
    report_written();
    let held = keybinds::EXPORT_MODEL.is_held();
    let pressed = held && !KEY_HELD.swap(held, Ordering::Relaxed);
    if REQUESTED.swap(false, Ordering::Relaxed) || pressed {
        settings_menu::open(&DIALOG);
    }
    if !EXPORT.swap(false, Ordering::Relaxed) {
        return;
    }
    let mut writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    if writing.is_some() {
        log!("an export is still being written");
        return;
    }
    let started = Instant::now();
    // SAFETY: the live editor ticking, on the game thread.
    let (mut export, summary) = unsafe { gather(game, editor) };
    if export.objects.is_empty() {
        log!("nothing to export: {summary}");
        return;
    }
    if JOIN.get() {
        export.join_by_look();
    }
    let format = Format::chosen();
    let Some(path) = export_path(&export.name, format.extension()) else {
        log!("no folder to export to (LOCALAPPDATA isn't set)");
        return;
    };
    log!(
        "exporting {}: {summary}, {} triangles, {} materials (read in {} ms)",
        path.display(),
        export.triangle_count(),
        export.looks.len(),
        started.elapsed().as_millis()
    );
    let scale = units_per_centimetre();
    let (sender, receiver) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("editortweaks model export".into())
        .spawn(move || {
            let started = Instant::now();
            let written = write(&export, format, scale, &path).map(|bytes| Written {
                path,
                bytes,
                millis: started.elapsed().as_millis(),
            });
            let _ = sender.send(written);
        });
    match spawned {
        Ok(_) => *writing = Some(receiver),
        Err(error) => log!("export not written: {error}"),
    }
}

/// Writes `export` to `path` as `format` (OBJ and STL counting `scale` units a centimetre), with
/// the files an OBJ needs next to it (its materials and textures): how many bytes in all.
fn write(export: &Export, format: Format, scale: f32, path: &Path) -> Result<usize, String> {
    let save = |path: &Path, bytes: &[u8]| {
        std::fs::write(path, bytes).map_err(|error| format!("{}: {error}", path.display()))
    };
    match format {
        Format::Glb => {
            let bytes = glb::write(export)?;
            save(path, &bytes)?;
            Ok(bytes.len())
        }
        Format::Stl => {
            let bytes = stl::write(export, scale);
            save(path, &bytes)?;
            Ok(bytes.len())
        }
        Format::Obj => {
            let library = path.with_extension("mtl");
            let library_name = library
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let files = obj::write(export, scale, &library_name)?;
            save(path, files.obj.as_bytes())?;
            save(&library, files.mtl.as_bytes())?;
            let mut bytes = files.obj.len() + files.mtl.len();
            for (name, image) in &files.images {
                save(&path.with_file_name(name), image)?;
                bytes += image.len();
            }
            Ok(bytes)
        }
    }
}

/// A file the export thread wrote.
struct Written {
    path: PathBuf,
    bytes: usize,
    millis: u128,
}

/// Logs how the export being written went, once it's done.
fn report_written() {
    let mut writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(receiver) = writing.as_ref() else {
        return;
    };
    let result = match receiver.try_recv() {
        Ok(result) => result,
        Err(mpsc::TryRecvError::Empty) => return,
        Err(mpsc::TryRecvError::Disconnected) => Err("the export thread stopped".into()),
    };
    *writing = None;
    match result {
        Ok(written) => {
            log!(
                "exported {} ({} KB, written in {} ms)",
                written.path.display(),
                written.bytes / 1024,
                written.millis
            );
            if SHOW_EXPORTED.get() {
                show_in_explorer(&written.path);
            }
        }
        Err(reason) => log!("export not written: {reason}"),
    }
}

/// Opens Explorer on `path`'s folder with it selected.
fn show_in_explorer(path: &std::path::Path) {
    use std::os::windows::process::CommandExt;

    // Explorer wants `/select,"path"`: given the whole argument quoted (`arg` quotes it when the
    // path has spaces), it opens Documents. Windows paths can't hold quotes.
    let mut select = std::ffi::OsString::from("/select,\"");
    select.push(path.as_os_str());
    select.push("\"");
    if let Err(error) = std::process::Command::new("explorer.exe")
        .raw_arg(select)
        .spawn()
    {
        log!("Explorer didn't open: {error}");
    }
}

/// Where the export of the vehicle `name` goes: the exports' folder (made if it isn't there),
/// a file named after it, with `extension`.
fn export_path(name: &str, extension: &str) -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    let folder = PathBuf::from(local).join(r"BrickRigs\SavedRemastered\EditorTweaks\Exports");
    // Fails if it exists already: either way, it's there or can't be.
    let _ = std::fs::create_dir_all(&folder);
    Some(folder.join(format!("{}.{extension}", file_name(name))))
}

/// `name` as a file name: characters Windows doesn't allow become `_`; "Vehicle" if empty.
fn file_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim_end_matches(['.', ' ']);
    if cleaned.is_empty() {
        "Vehicle".into()
    } else {
        cleaned.into()
    }
}

/// The editor's bricks [`BRICKS`] picks as a model, and a summary of how their shapes were found.
///
/// # Safety
/// `editor` must be the live editor. Game thread only.
unsafe fn gather(game: &Game, editor: *mut UObjectBase) -> (Export, String) {
    let mut export = Export {
        name: game
            .file
            .as_ref()
            // SAFETY: per the contract.
            .map(|file| unsafe { file.read(editor) }.title)
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| "Vehicle".into()),
        ..Export::default()
    };
    let source = chosen_source();
    let scope = Scope::chosen();
    let mut shapes = Shapes::new(
        source,
        game.models.as_ref(),
        game.collision.as_ref(),
        game.spinners.as_ref(),
    );
    // Collision has no looks.
    let looks = source != Source::Collision;
    let readback = game
        .readback
        .as_ref()
        .filter(|_| looks && EXPORT_TEXTURES.get());
    let mut materials = Materials::new(readback, editor);
    let mut texts = game
        .texts
        .as_ref()
        .filter(|_| readback.is_some())
        .map(TextParts::new);
    let (mut meshes, mut swept, mut pieces, mut boxes, mut with_text) = (0, 0, 0, 0, 0);
    let (mut with_tyres, mut patterned) = (0, 0);
    // The rubber texture, found with the first tyre.
    let mut rubber = None;
    let (mut collision, mut without_collision) = (0, 0);
    // SAFETY: per the contract; the editor's objects are live (entries may be null).
    unsafe {
        let objects = game.bricks.editor_objects(editor).iter().copied();
        for (index, brick) in objects
            .filter(|&object| {
                !object.is_null()
                    && game.bricks.is_brick(object)
                    && match scope {
                        Scope::Shown => !game.code.is_hidden(object),
                        Scope::Selected => game
                            .selection
                            .as_ref()
                            .is_some_and(|selection| selection.is_selected(object)),
                        Scope::All => true,
                    }
            })
            .enumerate()
        {
            let Some(bounds) = game.bounds.of(brick) else {
                continue;
            };
            let place = game.world.of(brick);
            let (mut part, kind, faces) = shapes.placed(brick, &place, bounds.map(math::widen));
            match kind {
                Kind::Mesh => meshes += 1,
                Kind::Swept => swept += 1,
                Kind::Pieces => pieces += 1,
                Kind::Box => boxes += 1,
                Kind::Collision => collision += 1,
                Kind::Nothing => {
                    without_collision += 1;
                    continue;
                }
            }
            // A wheel's tyres have a look of their own (rubber), whatever the brick's.
            let tyre_slots = match &game.tyres {
                Some(tyres) if looks && !faces.slots.is_empty() => tyres.slots(brick),
                _ => Vec::new(),
            };
            let mesh_uvs = !faces.uvs.is_empty() && faces.uvs.len() == faces.slots.len();
            let mut tyres = Vec::new();
            // Which of the mesh's triangles each of the part's is, as tyres are split off.
            let mut remaining: Vec<usize> = (0..part.corners.len()).collect();
            for (slot, tyre) in tyre_slots.iter().enumerate() {
                let Some(tyre) = *tyre else {
                    continue;
                };
                let of_slot = |triangle: usize| {
                    faces.slots.get(triangle).map_or(0, |&slot| slot as usize) == slot
                };
                let mut piece = part.split_off(|index| of_slot(remaining[index]));
                let (taken, kept): (Vec<usize>, Vec<usize>) =
                    remaining.iter().partition(|&&triangle| of_slot(triangle));
                remaining = kept;
                if piece.corners.is_empty() {
                    continue;
                }
                if mesh_uvs {
                    let uvs: Vec<_> = taken.iter().map(|&triangle| faces.uvs[triangle]).collect();
                    piece.set_corner_uvs(&uvs);
                }
                tyres.push((piece, tyre));
            }
            part.split_sharp_edges(SMOOTH_ANGLE);
            if looks {
                let pattern = game
                    .patterns
                    .as_ref()
                    .and_then(|patterns| patterns.of(brick));
                let (look, projection) =
                    materials.brick(&mut export, &game.looks.of(brick), pattern.as_ref());
                if let Some(projection) = projection {
                    patterned += usize::from(matches!(projection, Projection::Pattern { .. }));
                    projection.apply(&mut part);
                }
                part.material = Some(export.look_index(look));
            }
            let mut parts = Vec::new();
            if !part.corners.is_empty() {
                parts.push(part);
            }
            with_tyres += usize::from(!tyres.is_empty());
            for (mut piece, tyre) in tyres {
                piece.split_sharp_edges(SMOOTH_ANGLE);
                let rubber =
                    *rubber.get_or_insert_with(|| Tyres::rubber().unwrap_or(core::ptr::null_mut()));
                let (look, tiling) = materials.tyre(&mut export, rubber, tyre, mesh_uvs);
                if let Some(tiling) = tiling {
                    model_io::export::box_uvs(&mut piece, tiling);
                }
                piece.material = Some(export.look_index(look));
                parts.push(piece);
            }
            if let Some(texts) = &mut texts {
                let text = texts.parts(&mut materials, &mut export, brick, bounds, &place);
                with_text += usize::from(!text.is_empty());
                parts.extend(text);
            }
            let name = game
                .names
                .as_ref()
                .and_then(|(names, texts)| names.of(texts, brick))
                .unwrap_or_else(|| "Brick".into());
            export.objects.push(Object {
                name: format!("{name} {}", index + 1),
                parts,
            });
        }
    }
    put_on_origin(&mut export);
    let shapes = match source {
        Source::Collision => format!(
            "their collision ({collision} bricks, {without_collision} without collision left out)"
        ),
        Source::Model { level } => format!(
            "{meshes} by their mesh{}, {swept} spinners swept, {pieces} spinners by their \
             collision, {boxes} as boxes",
            match level {
                0 => String::new(),
                usize::MAX => " (lowest detail)".into(),
                level => format!(" (LOD {level})"),
            }
        ),
    };
    let (refitted, misplaced) = texts.map_or((0, 0), |texts| (texts.refitted, texts.misplaced));
    let summary = format!(
        "{} bricks ({shapes}), {patterned} with patterns, {with_tyres} with tyres, {with_text} \
         with text{}{}, {} images{}",
        export.objects.len(),
        if refitted > 0 {
            format!(" ({refitted} sized unlike their text component)")
        } else {
            String::new()
        },
        if misplaced > 0 {
            format!(" ({misplaced} placed away from their text component)")
        } else {
            String::new()
        },
        export.images.len(),
        if materials.unread > 0 {
            format!(" ({} textures couldn't be read)", materials.unread)
        } else {
            String::new()
        }
    );
    (export, summary)
}

/// Moves `export` so the middle of its bottom is the origin.
fn put_on_origin(export: &mut Export) {
    let positions = || {
        export
            .objects
            .iter()
            .flat_map(|object| &object.parts)
            .flat_map(|part| &part.positions)
    };
    let Some(&first) = positions().next() else {
        return;
    };
    let (low, high) = positions().fold((first, first), |(low, high), p| {
        (
            core::array::from_fn(|axis| low[axis].min(p[axis])),
            core::array::from_fn(|axis| high[axis].max(p[axis])),
        )
    });
    let origin: [f32; 3] = [(low[0] + high[0]) / 2.0, (low[1] + high[1]) / 2.0, low[2]];
    for object in &mut export.objects {
        for part in &mut object.parts {
            for position in &mut part.positions {
                for axis in 0..3 {
                    position[axis] -= origin[axis];
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_windows_safe() {
        assert_eq!(file_name("My car: v2/final?"), "My car_ v2_final_");
        assert_eq!(file_name("  "), "Vehicle");
        assert_eq!(file_name("Truck. "), "Truck");
    }
}
