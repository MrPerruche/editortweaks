//! Reading reference models on worker threads: the file, its move into the editor's space, its
//! cutting into pieces small enough to build in one frame, and its textures as PNG for the engine.
//! The game thread only gets the result.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use model_io::readers::Format;
use model_io::space::ImportSettings;
use model_io::{Part, readers};

/// The most triangles in one piece of a model: the game thread builds pieces one by one, a few per
/// frame (see `scene::BUILD_BUDGET`).
const PIECE_TRIANGLES: usize = 2000;

/// What a reference model needs to be built.
pub struct LoadedModel {
    /// Its parts cut into pieces, in the editor's space around the pivot.
    pub pieces: Vec<Part>,
    pub materials: Vec<LoadedMaterial>,
    pub triangles: usize,
    pub read_time: Duration,
}

/// A model material: its color, its texture as a PNG file's contents, and the color it shows
/// see-through in a lit material, which can't show the texture (its color times the texture's
/// average).
pub struct LoadedMaterial {
    pub color: [f32; 4],
    pub texture: Option<Vec<u8>>,
    pub see_through_color: [f32; 4],
}

/// A finished read, for the reference `id` as it was when asked (`key`).
pub struct Loaded {
    pub id: String,
    pub key: String,
    pub result: Result<LoadedModel, String>,
}

/// Reads the model at `path` on a new thread; the result goes to `done`.
pub fn start(
    id: String,
    key: String,
    path: PathBuf,
    settings: ImportSettings,
    done: Sender<Loaded>,
) {
    let spawned = std::thread::Builder::new()
        .name("editortweaks reference".into())
        .spawn(move || {
            let result =
                read(&path, &settings).map_err(|reason| format!("{}: {reason}", path.display()));
            // The game thread may have moved on and dropped the receiver.
            let _ = done.send(Loaded { id, key, result });
        });
    if let Err(error) = spawned {
        crate::prelude::log!("no thread to read a model: {error}");
    }
}

fn read(path: &std::path::Path, settings: &ImportSettings) -> Result<LoadedModel, String> {
    let started = Instant::now();
    let read = readers::read(path)?;
    // What a coop host sent is in the editor's space already.
    let model = if Format::of(path) == Some(Format::Shared) {
        read
    } else {
        model_io::space::to_unreal(read, settings)?
    };
    let triangles = model.triangle_count();
    let pieces = model
        .parts
        .iter()
        .flat_map(|part| part.split(PIECE_TRIANGLES))
        .collect();
    let materials = model
        .materials
        .iter()
        .map(|material| {
            // A texture the engine can't take is left out: the part keeps its color.
            let texture = material
                .texture
                .as_ref()
                .and_then(|image| Some((image, image.to_png().ok()?)));
            let average = texture
                .as_ref()
                .map_or([1.0; 4], |(image, _)| image.average_color());
            LoadedMaterial {
                color: material.color,
                see_through_color: core::array::from_fn(|channel| {
                    material.color[channel] * average[channel]
                }),
                texture: texture.map(|(_, png)| png),
            }
        })
        .collect();
    Ok(LoadedModel {
        pieces,
        materials,
        triangles,
        read_time: started.elapsed(),
    })
}
