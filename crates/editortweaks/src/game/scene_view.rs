//! The renderer's view of each frame (`FSceneView`, engine, not reflected): made on the game thread
//! from an `FSceneViewInitOptions` (the camera's place, angle, projection and screen rectangle) for
//! the player's view, scene captures and the like, then copied into the renderer's `FViewInfo`.
//! Found in game 1.11.8 (build 26488256); see `docs/re/editor-camera.md`, "Temporal AA".
//!
//! Its `bAllowTemporalJitter` is what lets the renderer jitter the view for temporal AA: the
//! constructor sets it, nothing else of the engine's does (the only other writes are the copies
//! into `FViewInfo`), and the renderer checks it last before jittering.

use crate::framework::sig::FunctionSig;

/// `FSceneView::FSceneView(const FSceneViewInitOptions& InitOptions)` (game RVA 0x2871620).
pub const CONSTRUCTOR: FunctionSig = FunctionSig {
    name: "FSceneView::FSceneView",
    // `bAllowTemporalJitter = true` (the offset at +2), then the feature level's shading path
    // looked up for the next field.
    pattern: "c6 83 ?? ?? 00 00 01 b8 08 01 00 00",
};

/// [`CONSTRUCTOR`]'s signature: returns `view`.
pub type ConstructorFn = unsafe extern "C" fn(view: *mut u8, options: *const u8) -> *mut u8;

/// Where `ProjectionMatrix.M[3][3]` is in `FSceneViewInitOptions`, which starts with its base
/// `FSceneViewProjectionData`: `FVector ViewOrigin` (0x0), `FMatrix ViewRotationMatrix` (0x10, as
/// `FMatrix` is 16-aligned), `FMatrix ProjectionMatrix` (0x50), `FIntRect ViewRect` (0x90, which
/// the constructor copies from there). An engine layout, the same for every UE 4.27 game.
const PROJECTION_WW: usize = 0x50 + 0x3C;

/// What the constructor tells about the view.
#[derive(Clone, Copy)]
pub struct SceneViewCode {
    /// The constructor's start.
    pub constructor: *const u8,
    /// Where `bAllowTemporalJitter` is in `FSceneView`.
    allow_jitter: usize,
}

// SAFETY: an address in the game exe and an offset, both living as long as the game.
unsafe impl Send for SceneViewCode {}
unsafe impl Sync for SceneViewCode {}

pub fn code() -> Result<SceneViewCode, String> {
    let found = CONSTRUCTOR.locate()?;
    // SAFETY: the match is 12 bytes of the game's code; the instruction's displacement is at +2.
    let allow_jitter = unsafe { found.address.add(2).cast::<u32>().read_unaligned() } as usize;
    if allow_jitter >= 0x10000 {
        return Err(format!(
            "{}: bAllowTemporalJitter at 0x{allow_jitter:X} is past any FSceneView",
            CONSTRUCTOR.name
        ));
    }
    Ok(SceneViewCode {
        constructor: found.function,
        allow_jitter,
    })
}

/// Whether the view made from `options` is orthographic (as the engine tells,
/// `FViewMatrices::IsPerspectiveProjection`: `M[3][3]` is 1, not 0).
///
/// # Safety
/// `options` must point to a live `FSceneViewInitOptions`.
pub unsafe fn is_orthographic(options: *const u8) -> bool {
    // SAFETY: per the contract; the offset is the engine's layout (see `PROJECTION_WW`).
    unsafe { options.add(PROJECTION_WW).cast::<f32>().read_unaligned() >= 1.0 }
}

impl SceneViewCode {
    /// Stops temporal AA from jittering `view`.
    ///
    /// # Safety
    /// `view` must be an `FSceneView` just constructed, not yet given to the renderer.
    pub unsafe fn stop_jitter(&self, view: *mut u8) {
        // SAFETY: per the contract; the offset is the constructor's own write's.
        unsafe { view.add(self.allow_jitter).write(0) }
    }
}
