//! The "Mirror" view mode: colors each brick by what is at its mirror image, as mirror mode
//! (automatic mirroring) sees it (see [`Pairing`]):
//!
//! - green: its counterpart, which mirror mode pairs it with and edits along with it (same type,
//!   mirrored place, rotation and properties, so the same color too);
//! - gray and green pattern: no counterpart, as its mirror image is itself (a symmetric brick on
//!   the mirror plane): a mirrored copy would be a duplicate;
//! - orange: a brick of the mirrored type with the mirrored rotation, but mirror mode doesn't pair
//!   them: some property doesn't mirror (color, size, ...), or both are on the mirror plane, where
//!   mirror mode pairs nothing (e.g. two spinners facing away from each other);
//! - gray: nothing.
//!
//! With mirror mode off nothing is paired: all gray.
//!
//! How (`game::mirror_mode`): pairs are the editor's own; mirror images it didn't pair are found
//! with the editor's own checks but the property one, so no brick type's mirroring rules are
//! copied here.

use std::collections::HashMap;

use brickrust::ue::coreuobject::UObjectBase;

use super::{Coloring, Colors, Editor, GRAY, Painting};
use crate::game::mirror_mode::{self, MirrorMode};
use crate::prelude::*;

/// The colors, free to change (see [`Colors`]); [`Pairing::colors`] picks them for each state, and
/// bricks with nothing at their mirror image show plain [`GRAY`].
const GREEN: [f32; 3] = [0.2, 1.0, 0.2];
const ORANGE: [f32; 3] = [1.0, 0.3, 0.02];

/// What a brick has at its mirror image, from best to worst: a brick with several things there (an
/// overlapping brick) shows the best.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Pairing {
    /// Its counterpart: mirror mode pairs them, selects and edits them together.
    Paired,
    /// No counterpart, as its mirror image is itself (a symmetric brick on the mirror plane).
    OwnImage,
    /// A brick of the mirrored type with the mirrored rotation that mirror mode doesn't pair it
    /// with: some property doesn't mirror, or both are on the mirror plane, where mirror mode pairs
    /// nothing (their properties are unknown then).
    Unlinked,
}

impl Pairing {
    fn colors(self) -> Colors {
        match self {
            Self::Paired => [GREEN, GREEN],
            Self::OwnImage => [GRAY, GREEN],
            Self::Unlinked => [ORANGE, ORANGE],
        }
    }
}

struct Mirror {
    /// For mirror images mirror mode doesn't pair; without it, they show as nothing.
    mirror: Option<MirrorMode>,
}

pub fn new() -> Result<Box<dyn Coloring>, String> {
    let mirror = match mirror_mode::find() {
        Ok(mirror) => Some(mirror),
        Err(reason) => {
            log!("unlinked mirror images will show gray: {reason}");
            None
        }
    };
    Ok(Box::new(Mirror { mirror }))
}

impl Coloring for Mirror {
    unsafe fn paint(&self, editor: &Editor) -> Painting {
        // SAFETY: per the contract; the editor's objects are live.
        unsafe {
            let paired = |object: *mut UObjectBase| editor.code.mirrored_object(object).is_some();
            let mut pairings = self.images(editor, paired);
            for &object in editor.objects() {
                if !object.is_null() && paired(object) {
                    pairings.insert(object as usize, Pairing::Paired);
                }
            }
            let count = |wanted| {
                pairings
                    .values()
                    .filter(|&&pairing| pairing == wanted)
                    .count()
            };
            let summary = format!(
                "of {} bricks: {} paired, {} their own mirror image, {} unlinked",
                editor.brick_list().len(),
                count(Pairing::Paired),
                count(Pairing::OwnImage),
                count(Pairing::Unlinked),
            );
            Painting {
                colors: pairings
                    .into_iter()
                    .map(|(brick, pairing)| (brick, pairing.colors()))
                    .collect(),
                summary,
            }
        }
    }
}

impl Mirror {
    /// What the unpaired bricks (per `paired`) have at their mirror image (see
    /// `MirrorMode::images`), by address; bricks with nothing are left out.
    ///
    /// # Safety
    /// `editor.editor` must be a live `ABrickEditor`. Game thread only.
    unsafe fn images(
        &self,
        editor: &Editor,
        paired: impl Fn(*mut UObjectBase) -> bool,
    ) -> HashMap<usize, Pairing> {
        let mut found = HashMap::new();
        // SAFETY: per the contract; the interface component and objects are the editor's, live.
        unsafe {
            let Some(mirror) = &self.mirror else {
                return found;
            };
            let Some(axis) = mirror.axis() else {
                return found;
            };
            let interface = editor.bricks.interface(editor.editor);
            if interface.is_null() {
                return found;
            }
            for (object, image) in mirror.images(interface, editor.objects(), axis, paired) {
                let pairing = if image == object {
                    Pairing::OwnImage
                } else {
                    Pairing::Unlinked
                };
                for brick in [object, image] {
                    let best = found.entry(brick as usize).or_insert(pairing);
                    *best = pairing.min(*best);
                }
            }
        }
        found
    }
}
