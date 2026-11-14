//! Folders and pins for the vehicle browser. Pure logic, no game code.
//!
//! Vehicles stay where the game keeps them; this crate only keeps a mapping from a vehicle's id to
//! a folder (and a pin), per category, in a file the game doesn't know
//! (`EditorTweaks-Folders.toml`, see [`Store`]).
//!
//! - [`FolderPath`]: a validated folder path.
//! - [`Tree`]: one category's folders and what's filed in them (create, rename, move, delete,
//!   pins, pruning).
//! - [`Store`]: both categories and the file's reading and writing.
//! - [`list`]: the rows to show at a [`Location`] for the game's items, per [`ViewMode`].
//! - [`History`], [`breadcrumbs`], [`page_range`]: navigation and paging.
//! - [`Gesture`], [`drop_onto`]: dragging vehicles and folders onto folders.
//! - [`is_vehicle_id`]: whether an id is safe to hand to the game's deletion.

mod drag;
mod item_id;
mod listing;
mod natural;
mod nav;
mod paging;
mod path;
mod store;
mod tree;

pub use drag::{
    DRAG_THRESHOLD, Dragged, Event, Gesture, HoverTimer, NEW_FOLDER_NAME, PAGE_TURN_DELAY_MS,
    PAGE_TURN_REPEAT_MS, destination, drop_onto, group_into_new_folder,
};
pub use item_id::is_vehicle_id;
pub use listing::{Item, ListOptions, Newest, PREVIEW_LEN, Row, ViewMode, list, newest_in};
pub use natural::natural_cmp;
pub use nav::{History, Location, ROOT_LABEL, UNCATEGORIZED_LABEL, breadcrumbs};
pub use paging::page_range;
pub use path::{FolderPath, MAX_DEPTH, MAX_NAME_CHARS, PathError, clean_name};
pub use store::{Category, FILE_VERSION, ParseError, Parsed, Store};
pub use tree::{FolderError, Origin, Tree, VehicleEntry};
