//! Drives features through the mod's lifetime: load, then engine init, then every map load.

use std::sync::OnceLock;

use crate::framework::settings::{self, Section};
use crate::prelude::*;

/// Features whose `on_load` succeeded. Set once by [`start`].
static ACTIVE: OnceLock<Vec<&'static dyn Feature>> = OnceLock::new();

/// Entry point from `mod_init`: loads the settings of `sections` (see `framework::settings`),
/// initializes brickrust, loads every feature and registers the engine hooks that drive the later
/// stages.
pub fn start(features: &'static [&'static dyn Feature], sections: &[&[Section]]) {
    if ACTIVE.get().is_some() {
        log!("start() called twice, ignoring");
        return;
    }

    match settings::load(sections) {
        Ok(ignored) if ignored.is_empty() => {}
        Ok(ignored) => log!("EditorTweaks.ini: ignored {}", ignored.join(", ")),
        Err(reason) => log!("settings not loaded, the defaults apply: {reason}"),
    }

    // SAFETY: called once from `mod_init`, before the engine starts, which is when brickrust
    // expects to resolve its signatures and install its engine hooks.
    unsafe { brickrust::init() };

    let active: Vec<_> = features
        .iter()
        .copied()
        .filter(|feature| match feature.on_load() {
            Ok(()) => true,
            Err(reason) => {
                log!("feature '{}' disabled: {reason}", feature.id());
                false
            }
        })
        .collect();
    log!("{} of {} features active", active.len(), features.len());
    let _ = ACTIVE.set(active);

    // SAFETY: brickrust::init() above resolved the engine functions these hooks attach to.
    unsafe {
        brickrust::hook_post_engine_init(on_engine_init);
        brickrust::hook_post_load_map(on_map_load);
    }
}

fn active() -> &'static [&'static dyn Feature] {
    ACTIVE.get().map(Vec::as_slice).unwrap_or_default()
}

fn on_engine_init() {
    log!("engine initialized");
    for feature in active() {
        feature.on_engine_init();
    }
}

fn on_map_load() {
    for feature in active() {
        feature.on_map_load();
    }
}
