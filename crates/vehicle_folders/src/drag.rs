//! Dragging a vehicle or folder onto a folder: the mouse gesture ([`Gesture`]) and what a drop does
//! ([`drop_onto`]). The game side polls the mouse button and the cursor, finds what's under it, and
//! feeds both in.

use crate::nav::Location;
use crate::path::FolderPath;
use crate::tree::{FolderError, Tree};

/// How far the cursor moves, in pixels, while the button is held before a press becomes a drag
/// (Slate's own drag threshold is 5).
pub const DRAG_THRESHOLD: i32 = 6;

/// What is dragged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dragged {
    /// A vehicle, by its id.
    Vehicle(String),
    Folder(FolderPath),
}

/// A press on something draggable becoming a drag. `T` is what was pressed on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gesture<T> {
    state: State<T>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum State<T> {
    /// The button is up.
    Idle,
    /// The button is down, but not on something draggable (or it already was when we started
    /// watching): nothing happens until it's released.
    Held,
    /// Pressed on `what` at `at`, not moved far yet: still a click.
    Pressed {
        what: T,
        at: (i32, i32),
    },
    Dragging {
        what: T,
    },
}

/// What an update of [`Gesture`] tells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event<T> {
    Nothing,
    /// A drag of this began.
    Started(T),
    /// The drag continues (show what's under the cursor).
    Moved(T),
    /// The button was released while dragging this: drop it where the cursor is.
    Dropped(T),
}

impl<T: Clone> Default for Gesture<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> Gesture<T> {
    /// Watching from now on with the button up.
    pub fn new() -> Self {
        Self { state: State::Idle }
    }

    /// Watching from now on with the button already down: that press is never a drag.
    pub fn held() -> Self {
        Self { state: State::Held }
    }

    /// What is being dragged, if a drag is on.
    pub fn dragging(&self) -> Option<&T> {
        match &self.state {
            State::Dragging { what } => Some(what),
            _ => None,
        }
    }

    /// What is being dragged, to change (e.g. which row shows it after the page changed).
    pub fn dragging_mut(&mut self) -> Option<&mut T> {
        match &mut self.state {
            State::Dragging { what } => Some(what),
            _ => None,
        }
    }

    /// The next look at the mouse: whether the button is `down` and where the cursor is.
    /// `pressed_on` tells what a new press landed on (asked only when the button goes down).
    pub fn update(
        &mut self,
        down: bool,
        cursor: (i32, i32),
        pressed_on: impl FnOnce() -> Option<T>,
    ) -> Event<T> {
        let state = core::mem::replace(&mut self.state, State::Idle);
        let (next, event) = match (state, down) {
            (State::Idle, false) | (State::Held, false) | (State::Pressed { .. }, false) => {
                (State::Idle, Event::Nothing)
            }
            (State::Idle, true) => match pressed_on() {
                Some(what) => (State::Pressed { what, at: cursor }, Event::Nothing),
                None => (State::Held, Event::Nothing),
            },
            (State::Held, true) => (State::Held, Event::Nothing),
            (State::Pressed { what, at }, true) => {
                if moved_far(at, cursor) {
                    (State::Dragging { what: what.clone() }, Event::Started(what))
                } else {
                    (State::Pressed { what, at }, Event::Nothing)
                }
            }
            (State::Dragging { what }, true) => {
                (State::Dragging { what: what.clone() }, Event::Moved(what))
            }
            (State::Dragging { what }, false) => (State::Idle, Event::Dropped(what)),
        };
        self.state = next;
        event
    }

    /// Stops a drag without dropping (the list went away): what was dragged, if anything. The rest
    /// of the press is ignored.
    pub fn cancel(&mut self) -> Option<T> {
        let state = core::mem::replace(&mut self.state, State::Held);
        match state {
            State::Dragging { what } => Some(what),
            State::Idle => {
                self.state = State::Idle;
                None
            }
            _ => None,
        }
    }
}

/// How long something dragged stays over a page button before the page turns, in milliseconds.
pub const PAGE_TURN_DELAY_MS: u64 = 600;
/// How often the page turns again while it stays there.
pub const PAGE_TURN_REPEAT_MS: u64 = 800;

/// Turning pages while dragging, by holding over the previous or next page button: the first turn
/// after [`PAGE_TURN_DELAY_MS`] over the same button, then one every [`PAGE_TURN_REPEAT_MS`] while
/// it stays. `K` tells the buttons apart.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HoverTimer<K> {
    /// The button under the cursor, and when the next turn is due (milliseconds, any clock).
    over: Option<(K, u64)>,
}

impl<K: PartialEq + Clone> HoverTimer<K> {
    pub fn new() -> Self {
        Self { over: None }
    }

    /// The next look: the button under the cursor (`None`: none) at `now_ms`. The button to turn
    /// the page with, when a turn is due.
    pub fn update(&mut self, over: Option<K>, now_ms: u64) -> Option<K> {
        let Some(button) = over else {
            self.over = None;
            return None;
        };
        match &mut self.over {
            Some((known, due)) if *known == button => {
                if now_ms < *due {
                    return None;
                }
                *due = now_ms + PAGE_TURN_REPEAT_MS;
                Some(button)
            }
            _ => {
                self.over = Some((button, now_ms + PAGE_TURN_DELAY_MS));
                None
            }
        }
    }
}

/// Whether the cursor went past the drag threshold from `from`, on either axis.
fn moved_far(from: (i32, i32), to: (i32, i32)) -> bool {
    (to.0 - from.0).abs() >= DRAG_THRESHOLD || (to.1 - from.1).abs() >= DRAG_THRESHOLD
}

/// The folder a drop onto `target` files things into: the folder itself, or the top level (`None`)
/// for the top level and Uncategorized (the vehicles in no folder).
pub fn destination(target: &Location) -> Option<FolderPath> {
    match target {
        Location::Folder(path) => Some(path.clone()),
        Location::Root | Location::Uncategorized => None,
    }
}

/// Drops `dragged` onto `target`: files the vehicle there, or moves the folder (with everything in
/// it) into it. Whether anything changed: dropping something where it already is changes nothing.
pub fn drop_onto(
    tree: &mut Tree,
    dragged: &Dragged,
    target: &Location,
) -> Result<bool, FolderError> {
    let into = destination(target);
    match dragged {
        Dragged::Vehicle(id) => {
            if tree.folder_of(id) == into.as_ref() {
                return Ok(false);
            }
            tree.move_vehicle(id, into.as_ref())?;
            Ok(true)
        }
        Dragged::Folder(path) => {
            if into.as_ref() == Some(path) || path.parent() == into {
                return Ok(false);
            }
            tree.move_folder(path, into.as_ref())?;
            Ok(true)
        }
    }
}

/// The name a folder made by dropping a vehicle onto another one starts with (then "New folder 2",
/// ... if taken).
pub const NEW_FOLDER_NAME: &str = "New folder";

/// Drops the vehicle `dragged` onto the vehicle `target`, as phones group apps: a new folder (named
/// [`NEW_FOLDER_NAME`], or numbered if taken) is made where `target` is filed, and both go into it.
/// Their pins stay. The new folder's path; nothing changes on an error (or if both are the same
/// vehicle, refused as moving a folder into itself would be).
pub fn group_into_new_folder(
    tree: &mut Tree,
    dragged: &str,
    target: &str,
) -> Result<FolderPath, FolderError> {
    if dragged == target {
        return Err(FolderError::IntoOwnSubtree);
    }
    let parent = tree.folder_of(target).cloned();
    let name = tree.free_folder_name(parent.as_ref(), NEW_FOLDER_NAME)?;
    let folder = tree.create_folder(parent.as_ref(), &name)?;
    // Both can't fail: the folder exists now.
    tree.move_vehicle(target, Some(&folder))?;
    tree.move_vehicle(dragged, Some(&folder))?;
    Ok(folder)
}

#[cfg(test)]
mod tests;
