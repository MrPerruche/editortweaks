//! A box at the start of the editor's bottom row (before the vehicle's name, size, weight, brick
//! count and price: `game::list_header`) saying what coop does: OFFLINE, HOSTING, JOINING (asking
//! a host to let us in), MEMBER, or, in red, [`BACKGROUND`] while hosting with another vehicle in
//! the editor (the session's goes on without it, until it's brought back or this one is shared).
//!
//! Made once per editor widget, after coop's tick, from the status the coop page shows; its text
//! is only set again when it changes.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use brickrust::ue::coreuobject::UObjectBase;

use super::log::coop_log;
use super::ui::{Doing, Status};
use crate::game::list_header::{self, Field, ListHeader};
use crate::game::weak_object::WeakObject;

/// Hosting, the editor holding another vehicle than the session's. Short: the box is half as wide
/// as the size box.
const BACKGROUND: &str = "NOT SHARED";

/// How often an editor whose row wasn't found (not made yet) is looked through again.
const LOOK_AGAIN: Duration = Duration::from_secs(1);

static KIT: OnceLock<Result<ListHeader, String>> = OnceLock::new();

/// The box in the current editor's row.
#[derive(Default)]
struct Shown {
    /// The editor whose row was looked for, and when.
    editor: Option<WeakObject>,
    looked: Option<Instant>,
    /// Whether adding the box failed in that editor (logged once per editor).
    failed: bool,
    field: Option<Field>,
    /// What the box shows.
    label: Option<(&'static str, bool)>,
}

thread_local! {
    static SHOWN: RefCell<Shown> = RefCell::default();
}

/// What the box says for `status`, and whether in red.
fn label(status: &Status) -> (&'static str, bool) {
    match status.doing {
        Doing::Alone => ("PRIVATE", false), // TODO: Change to OFFLINE after Nov. 6
        Doing::Hosting if status.elsewhere => (BACKGROUND, true),
        Doing::Hosting => ("HOSTING", false),
        Doing::Asking(_) => ("JOINING", false),
        Doing::Member(_) => ("MEMBER", false),
    }
}

/// Shows `status` in `editor`'s row, adding the box first if it has none. After coop's tick, on
/// the game thread.
pub fn show(editor: *mut UObjectBase, status: &Status) {
    let kit = KIT.get_or_init(|| {
        list_header::find().inspect_err(|reason| {
            coop_log!("no coop box in the editor's bottom row: {reason}");
        })
    });
    let Ok(kit) = kit else { return };
    SHOWN.with_borrow_mut(|shown| {
        // A new editor (possibly where a destroyed one was).
        if !shown
            .editor
            .is_some_and(|known| known.is(editor) && known.get_live().is_some())
        {
            *shown = Shown {
                // SAFETY: the ticking editor.
                editor: Some(unsafe { WeakObject::new(editor) }),
                ..Shown::default()
            };
        }
        if shown
            .field
            .as_ref()
            .is_some_and(|field| !kit.is_alive(field))
        {
            shown.field = None;
            shown.label = None;
        }
        if shown.field.is_none() {
            if shown.failed || shown.looked.is_some_and(|at| at.elapsed() < LOOK_AGAIN) {
                return;
            }
            shown.looked = Some(Instant::now());
            // SAFETY: the ticking editor, on the game thread.
            let Some(header) = (unsafe { kit.of(editor) }) else {
                return;
            };
            // SAFETY: the editor's live row, on the game thread.
            match unsafe { kit.add_field(header) } {
                Ok(field) => shown.field = Some(field),
                Err(reason) => {
                    shown.failed = true;
                    coop_log!("no coop box in the editor's bottom row: {reason}");
                    return;
                }
            }
        }
        let Some(field) = &shown.field else { return };
        // SAFETY: on the game thread.
        unsafe { kit.keep_layout(field) };
        let wanted = label(status);
        if shown.label != Some(wanted) {
            shown.label = Some(wanted);
            // SAFETY: on the game thread.
            unsafe { kit.show(field, wanted.0, wanted.1) };
        }
    });
}
