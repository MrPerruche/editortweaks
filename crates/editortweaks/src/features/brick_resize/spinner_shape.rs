//! Cycling a spinner's cross-section with a key while fast editing (`docs/spinner-editing.md`): a
//! press of [`keybinds::NEXT_SPINNER_VARIANT`] over a spinner switches it to the next variant of
//! its shape (inward, outward, upward), a press of [`keybinds::NEXT_SPINNER_SHAPE`] to the next
//! shape (see `model_io::spinner::next_variant` and `next_family`). The spinner's mirror mode
//! counterpart gets the same shape, and the whole change is one undo step.
//!
//! Only the shape is written, and then the brick is told like the editor's panel tells it
//! (`game::spinner::Spinners::write`): the game rebuilds the mesh, connectors and collision itself,
//! and the box, which comes from the cross-section's bounding square, stays as it is.

use brickrust::ue::coreuobject::UObjectBase;
use model_io::spinner as shape;
use std::sync::Once;

use super::{CHANGED, Game, LOG_NO_INTERFACE};
use crate::features::keybinds;
use crate::game::spinner::{Property, Values};
use crate::prelude::*;

static LOG_CYCLED: Once = Once::new();

/// What a press of a cross-section key asks for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    /// The next variant of the spinner's shape.
    Variant,
    /// The next shape.
    Shape,
}

impl Step {
    /// The shape that follows `current`.
    fn next(self, current: u8) -> u8 {
        match self {
            Step::Variant => shape::next_variant(current),
            Step::Shape => shape::next_family(current),
        }
    }
}

/// Which cross-section keys were held last frame, to tell a press from a key held down.
#[derive(Default)]
pub struct Keys {
    variant: bool,
    shape: bool,
}

impl Keys {
    pub const fn new() -> Self {
        Self {
            variant: false,
            shape: false,
        }
    }

    /// The step a key pressed since last frame asks for, if any. The shape key's default holds the
    /// variant key too (Ctrl + R): when it's held, only it counts.
    pub fn pressed(&mut self) -> Option<Step> {
        let variant = keybinds::NEXT_SPINNER_VARIANT.is_held();
        let shape = keybinds::NEXT_SPINNER_SHAPE.is_held();
        let variant_pressed = variant && !self.variant && !shape;
        let shape_pressed = shape && !self.shape;
        self.variant = variant;
        self.shape = shape;
        if shape_pressed {
            Some(Step::Shape)
        } else if variant_pressed {
            Some(Step::Variant)
        } else {
            None
        }
    }
}

/// Switches the spinner `object` (and its mirror mode counterpart) to the shape `step` asks for,
/// as one editor operation. Returns the bricks that changed, whose places in the scene are to be
/// found again: none if the shape stays (a rectangle has no variants) or the bricks can't be told.
///
/// # Safety
/// `editor` must be the live editor and `object` a live spinner in it. Game thread only.
pub unsafe fn cycle(
    game: &Game,
    editor: *mut UObjectBase,
    object: *mut UObjectBase,
    step: Step,
) -> Vec<*mut UObjectBase> {
    let Some((spinners, _)) = &game.spinners else {
        return Vec::new();
    };
    // SAFETY: per the contract; the counterpart is a live brick, checked to be a spinner.
    unsafe {
        let next = step.next(spinners.read(object).shape);
        if next == spinners.read(object).shape {
            return Vec::new();
        }
        let counterpart = game.mirror.as_ref().and_then(|mirror| {
            mirror.axis()?;
            let other = game.code.mirrored_object(object)?;
            (other != object && spinners.is_spinner(other)).then_some(other)
        });
        let mut changed = Vec::new();
        game.editor.operation(editor, CHANGED, || {
            for brick in [Some(object), counterpart].into_iter().flatten() {
                let values = Values {
                    shape: next,
                    ..spinners.read(brick)
                };
                if spinners
                    .write(brick, &values, &[Property::Shape], false)
                    .is_some()
                {
                    changed.push(brick);
                } else {
                    LOG_NO_INTERFACE.call_once(|| {
                        log!("can't change spinners: bricks' property interface not found")
                    });
                }
            }
        });
        if !changed.is_empty() {
            LOG_CYCLED.call_once(|| log!("changed a spinner's cross-section to shape {next}"));
        }
        changed
    }
}
