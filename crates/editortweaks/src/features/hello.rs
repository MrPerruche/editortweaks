//! Proof of life: shows that the mod loads, that engine hooks fire and that signature-resolved game
//! functions can be called. Visible in-game as a suffix on the window title.

use crate::platform::window;
use crate::prelude::*;

pub struct Hello;

const TITLE_SUFFIX: &str = " + editortweaks";

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

    fn on_map_load(&self) {
        let Some(hwnd) = window::find_game_window() else {
            log!("game window not found");
            return;
        };
        let title = window::title(hwnd);
        if title.ends_with(TITLE_SUFFIX) {
            return;
        }
        let title = title.trim_end(); // the game's title has trailing spaces
        let new_title = format!("{title}{TITLE_SUFFIX}");
        if window::set_title(hwnd, &new_title) {
            log!("window title set to '{new_title}'");
        } else {
            log!("failed to set window title");
        }
    }
}
