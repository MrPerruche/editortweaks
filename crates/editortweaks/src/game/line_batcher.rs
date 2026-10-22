//! Drawing lines in the 3D view: the world's line batcher (`UWorld::ForegroundLineBatcher`, a
//! reflected property), a component the engine renders every frame from its list of lines. Debug
//! drawing (`DrawDebugLine`) is compiled out of the shipping game, but the batchers and their
//! rendering are not. Found through BRMK 1.11.6's engine module, then in game 1.11.7 (build
//! 25831956); see `docs/re/editor-operations.md`.
//!
//! The batcher's own tick only drops lines with a positive lifetime, and ours have none, yet in the
//! editor lines drawn once are gone a frame later (seen 2026-10-21; what empties the batcher isn't
//! known). A feature that doesn't draw every frame calls [`LineBatcher::keep`] every frame. The
//! batcher holds a single list, so each feature's lines are kept apart here (by the feature's name)
//! and drawn together.

use std::sync::{Mutex, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;

use crate::framework::sig::FunctionSig;
use crate::game::editor_object::Vector;
use crate::game::{reflection, world};

/// `void ULineBatchComponent::DrawLine(const FVector& Start, const FVector& End, const
/// FLinearColor& Color, uint8 DepthPriority, float Thickness, float LifeTime)` (BRMK engine module
/// 0x8A31F0, game RVA 0x247FBA0): appends an `FBatchedLine` (0x34 bytes: start, end, color at
/// +0x18, thickness +0x28, remaining lifetime +0x2C, depth priority +0x30) to the component's
/// `BatchedLines` and marks its render state dirty. Virtual: the last two slots of the component's
/// vtable are `DrawLine` and `DrawPoint`, after the many of `UPrimitiveComponent`.
pub const DRAW_LINE: FunctionSig = FunctionSig {
    name: "ULineBatchComponent::DrawLine",
    // The new line's address in `BatchedLines` (its offset at +7), then its start being copied.
    pattern: "48 6b d7 34 48 03 96 ?? ?? 00 00 74 ?? f2 41 0f 10 06",
};

type DrawLineFn = unsafe extern "C" fn(
    batcher: *mut UObjectBase,
    start: *const Vector,
    end: *const Vector,
    color: *const [f32; 4],
    depth_priority: u8,
    thickness: f32,
    lifetime: f32,
);

/// `ESceneDepthPriorityGroup`: drawn with the world (hidden behind what's in front), or on top.
const SDPG_WORLD: u8 = 0;
const SDPG_FOREGROUND: u8 = 1;

/// One line to draw.
#[derive(Clone, Copy)]
pub struct Line {
    pub start: Vector,
    pub end: Vector,
    /// Linear red, green, blue.
    pub color: [f32; 3],
    /// Width in centimeters; 0 is one pixel wide.
    pub thickness: f32,
    /// Drawn over everything, rather than hidden behind what's in front of it.
    pub on_top: bool,
}

/// A feature's lines, and the batcher they're for (by address).
struct Layer {
    owner: &'static str,
    batcher: usize,
    lines: Vec<Line>,
}

/// Every feature's lines, in the order they first drew.
static LAYERS: Mutex<Vec<Layer>> = Mutex::new(Vec::new());

/// Every feature's lines for `batcher`, in the order the features first drew.
fn layered(layers: &[Layer], batcher: *mut UObjectBase) -> Vec<Line> {
    layers
        .iter()
        .filter(|layer| layer.batcher == batcher as usize)
        .flat_map(|layer| layer.lines.iter().copied())
        .collect()
}

/// The start of a `TArray`: its element count.
#[repr(C)]
struct ArrayHead {
    _data: *mut u8,
    num: i32,
    _max: i32,
}

/// Draws lines through the game's line batcher (after engine init).
pub struct LineBatcher {
    draw_line: DrawLineFn,
    /// Offset of `ULineBatchComponent::BatchedLines` (a `TArray<FBatchedLine>`).
    lines: usize,
    /// Offset of `UWorld::ForegroundLineBatcher`.
    foreground: usize,
}

// SAFETY: game code and offsets living as long as the game; only used on the game thread.
unsafe impl Send for LineBatcher {}
unsafe impl Sync for LineBatcher {}

pub fn find() -> Result<LineBatcher, String> {
    let found = DRAW_LINE.locate()?;
    let world = reflection::find_class("World").ok_or("class World not found")?;
    // SAFETY: the displacement lies inside the matched pattern; the function has DrawLine's
    // signature (see DRAW_LINE).
    unsafe {
        Ok(LineBatcher {
            draw_line: core::mem::transmute::<*const u8, DrawLineFn>(found.function),
            lines: found.address.add(7).cast::<u32>().read_unaligned() as usize,
            foreground: reflection::field_offset(
                world,
                "ForegroundLineBatcher",
                "ObjectProperty",
                8,
            )?,
        })
    }
}

impl LineBatcher {
    /// The current world's foreground line batcher, or null.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn batcher(&self) -> *mut UObjectBase {
        // SAFETY: per the contract; the world is null or a `UWorld` (the property's offset comes
        // from reflection).
        unsafe {
            let world = world::current();
            if world.is_null() {
                return core::ptr::null_mut();
            }
            world
                .cast::<u8>()
                .add(self.foreground)
                .cast::<*mut UObjectBase>()
                .read()
        }
    }

    /// Replaces the lines `owner` (a feature's name) shows through `batcher` with `lines` (none to
    /// clear them). The other features' lines for it stay.
    ///
    /// # Safety
    /// `batcher` must be a live `ULineBatchComponent` (see [`Self::batcher`]). Game thread only.
    pub unsafe fn replace(&self, batcher: *mut UObjectBase, owner: &'static str, lines: &[Line]) {
        let mut layers = LAYERS.lock().unwrap_or_else(PoisonError::into_inner);
        match layers.iter_mut().find(|layer| layer.owner == owner) {
            Some(layer) => {
                layer.batcher = batcher as usize;
                layer.lines = lines.to_vec();
            }
            None => layers.push(Layer {
                owner,
                batcher: batcher as usize,
                lines: lines.to_vec(),
            }),
        }
        // SAFETY: per the contract.
        unsafe { self.draw(batcher, &layered(&layers, batcher)) }
    }

    /// Draws again every feature's lines for `batcher` if it lost some (see the module's doc).
    ///
    /// # Safety
    /// As for [`Self::replace`].
    pub unsafe fn keep(&self, batcher: *mut UObjectBase) {
        let layers = LAYERS.lock().unwrap_or_else(PoisonError::into_inner);
        let lines = layered(&layers, batcher);
        // SAFETY: per the contract; the array's offset comes from `DrawLine`'s own code.
        unsafe {
            let array = batcher.cast::<u8>().add(self.lines).cast::<ArrayHead>();
            if usize::try_from((*array).num) != Ok(lines.len()) {
                self.draw(batcher, &lines);
            }
        }
    }

    /// Replaces every line `batcher` shows with `lines`.
    ///
    /// # Safety
    /// As for [`Self::replace`].
    unsafe fn draw(&self, batcher: *mut UObjectBase, lines: &[Line]) {
        // SAFETY: per the contract; `FBatchedLine`s are plain data, so emptying the array only
        // needs its count reset (the engine copies the lines for rendering when it's marked dirty,
        // on the game thread at the end of the frame). Every call marks it dirty.
        unsafe {
            let array = batcher.cast::<u8>().add(self.lines).cast::<ArrayHead>();
            (*array).num = 0;
            for line in lines {
                let [red, green, blue] = line.color;
                let priority = if line.on_top {
                    SDPG_FOREGROUND
                } else {
                    SDPG_WORLD
                };
                (self.draw_line)(
                    batcher,
                    &line.start,
                    &line.end,
                    &[red, green, blue, 1.0],
                    priority,
                    line.thickness,
                    0.0,
                );
            }
            if lines.is_empty() {
                // Only drawing marks the batcher dirty: draw a line, then drop it.
                let origin = [0.0; 3];
                (self.draw_line)(batcher, &origin, &origin, &[0.0; 4], SDPG_WORLD, 0.0, 0.0);
                (*array).num = 0;
            }
        }
    }
}
