//! Proof of life: shows that the mod loads, that engine hooks fire and that signature-resolved game
//! functions can be called. Visible in the log (`brickworks.txt`).

use crate::prelude::*;

pub struct Hello;

impl Feature for Hello {
    fn id(&self) -> &'static str {
        "hello"
    }

    fn on_load(&self) -> LoadResult {
        log!(
            "editortweaks {} loaded, waiting for the engine",
            env!("CARGO_PKG_VERSION")
        );
        Ok(())
    }

    fn on_engine_init(&self) {
        // GetProjectVersion is located by byte signature, so this also proves lookups work.
        // SAFETY: runs after UEngine::Init on the game thread. The returned FString was allocated
        // by the game's allocator and is ours to free (TArray::free uses FMemory::Free).
        let version = unsafe {
            let mut raw = brickrust::br::statics::GetProjectVersion();
            let version = raw.to_string();
            raw.free();
            version
        };
        // FString lengths include the terminating NUL.
        let version = version.trim_end_matches('\0');
        let expected = crate::TARGET_GAME_VERSION.to_str().unwrap_or_default();
        if version == expected {
            log!("game version {version}");
        } else {
            log!("game version {version}, but this build targets {expected}");
        }
    }
}
