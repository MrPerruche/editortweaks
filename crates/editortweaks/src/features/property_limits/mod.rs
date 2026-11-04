//! Our own limits on the values the vehicle editor edits, instead of the game's: by default none
//! besides a sanity cap (`rules::UNLIMITED`), so values typed into sliders aren't limited to the
//! slider's range; [`rules`] sets limits for specific properties. Everything else keeps the game's
//! limits ([`scope`]).
//!
//! The game limits numeric values in up to three places, each handled by a submodule:
//! - [`slider`]: the slider widget clamps typed values to its range.
//! - [`property`]: the numeric property clamps them again when storing them; the rules apply here.
//! - [`brick_size`]: scalable bricks constrain their size to a per-type range; the rules apply
//!   here too.
//!
//! Rules can depend on other properties of the object (a light's brightness on its direction):
//! [`dependencies`] applies them again when those change.
//!
//! Nothing else re-clamps, so saved vehicles keep their values, also for players without the mod
//! (until they edit that value). Arrow keys still stop at the slider's ends, and so does dragging
//! unless `slider_drag` lifts that; a rule's `drag_min` then stops drags before typed values.
//!
//! [`highlight`] colors sliders showing values outside of the game's range, and differently those
//! outside of the rules too.

mod brick_size;
mod dependencies;
mod highlight;
mod limits;
mod property;
mod rules;
mod scope;
mod slider;

use crate::prelude::*;

pub struct PropertyLimits;

impl Feature for PropertyLimits {
    fn id(&self) -> &'static str {
        "property_limits"
    }

    fn on_load(&self) -> LoadResult {
        // Resolve the required patterns before hooking anything, so a missing one leaves the
        // game untouched. The brick size and dependency parts are optional: the rest still works
        // without them.
        let slider_target = slider::resolve()?;
        let property_targets = property::resolve()?;
        let brick_size_target = brick_size::resolve();
        let dependencies_targets = dependencies::resolve();

        // SAFETY: each target is the start of the function its pattern identifies, and each
        // submodule's detours have exactly the signatures declared next to those patterns.
        unsafe {
            slider::install(slider_target)?;
            property::install(property_targets)?;
            match brick_size_target {
                Ok(target) => brick_size::install(target)?,
                Err(reason) => log!("brick sizes keep the game's limits: {reason}"),
            }
            match dependencies_targets {
                Ok(targets) => dependencies::install(targets)?,
                Err(reason) => log!(
                    "limits depending on other properties only apply when their value is edited: {reason}"
                ),
            }
        }
        if let Err(reason) = scope::install() {
            log!("every value keeps the game's limits: {reason}");
        }
        if let Err(reason) = highlight::install() {
            log!("values outside of the game's range aren't highlighted: {reason}");
        }
        Ok(())
    }

    fn on_engine_init(&self) {
        scope::on_engine_init();
        slider::on_engine_init();
        brick_size::on_engine_init();
        highlight::on_engine_init();
    }
}

/// Whether sliders keep values set outside of their range (typed, or dragged by `slider_drag`).
/// Known from engine init on.
pub(crate) fn keeps_slider_values() -> bool {
    slider::active()
}

/// Whether the values numeric properties store can be rewritten per edited object
/// (`slider_formulas::per_object_value`, `axis_permutations::per_object_value`), from the hook of
/// `ClampValue`. Known from load on.
pub(crate) fn rewrites_stored_values() -> bool {
    property::hooked()
}

/// How far values go where nothing limits them (see `rules::UNLIMITED`), for rows of ours that take
/// typed values past their sliders' ends as bricks' do.
pub(crate) const UNLIMITED: f32 = rules::UNLIMITED;

/// How far values of `slider` may go, if past its ends (see [`scope`]); `None` keeps them within
/// its range. Game thread only.
pub(crate) fn slider_limit(slider: *const core::ffi::c_void) -> Option<(f32, f32)> {
    scope::limit_of(slider)
}
