//! The listing: which rows the browser shows at a location, in which order.
//!
//! One pass over the game's items with hash lookups (`O(n)`, plus sorting the folders shown),
//! cheap enough for thousands of vehicles on every page change.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::natural::natural_cmp;
use crate::nav::Location;
use crate::path::FolderPath;
use crate::tree::Tree;

/// A vehicle as the game lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The vehicle's id: its folder name (Local) or Workshop item id (Subscribed).
    pub id: String,
    /// Its title, for ordering folders among vehicles sorted by title.
    pub title: String,
}

impl Item {
    /// An item from its id and title.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
        }
    }
}

/// How folders and vehicles are laid out (the "Vehicle folders" setting).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// The game's flat list, pinned vehicles first. No folders.
    Off,
    /// A level's subfolders and vehicles interleaved in one order.
    Mixed,
    /// A level's subfolders, then its vehicles.
    #[default]
    FoldersFirst,
    /// The top level shows only folders and one Uncategorized folder holding the unfiled vehicles.
    UncategorizedFolder,
}

/// What to list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListOptions {
    /// Where the browser is. Ignored when the list is flat ([`ViewMode::Off`] or a search).
    pub location: Location,
    /// The layout.
    pub mode: ViewMode,
    /// Whether the player is searching: the game already filtered the items, and they are shown
    /// flat, from every folder.
    pub search_active: bool,
    /// Whether the game sorted the items by title (only [`ViewMode::Mixed`] cares).
    pub sort_by_title: bool,
}

/// How many vehicles a folder row previews (its tile's 2x2 thumbnail grid).
pub const PREVIEW_LEN: usize = 4;

/// One row of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A folder.
    Folder {
        /// Where it is.
        path: FolderPath,
        /// Whether it's pinned.
        pinned: bool,
        /// The listed vehicles in it, in its subfolders too.
        vehicle_count: usize,
        /// Up to [`PREVIEW_LEN`] of them to show on its tile (positions in the items given): those
        /// filed in it first (pinned ones first, then the game's order), then from its subfolders.
        preview: Vec<usize>,
    },
    /// The virtual folder of unfiled vehicles.
    Uncategorized {
        /// How many vehicles are in it.
        vehicle_count: usize,
        /// Up to [`PREVIEW_LEN`] of them, as listed inside it.
        preview: Vec<usize>,
    },
    /// A vehicle.
    Vehicle {
        /// Its position in the items given.
        index: usize,
        /// Whether it's pinned.
        pinned: bool,
    },
}

/// The rows to show for the game's `items` (in the game's own sort order), in order.
///
/// Per level, pinned folders come first, then pinned vehicles, then the rest as the mode says. A
/// pinned vehicle only shows in its own folder. A vehicle whose folder doesn't exist (any more),
/// or that the tree doesn't know, is unfiled.
pub fn list(tree: &Tree, items: &[Item], options: &ListOptions) -> Vec<Row> {
    if options.mode == ViewMode::Off || options.search_active {
        return flat(tree, items);
    }

    let view = View::of(tree, options);
    let (here, mut folders) = survey(tree, items, view);

    // Folders by name, naturally; the exact spelling breaks ties so the order is fixed.
    folders.sort_by(|a, b| natural_cmp(a.name(), b.name()).then_with(|| a.name().cmp(b.name())));
    let (pinned_folders, plain_folders): (Vec<Folder>, Vec<Folder>) =
        folders.into_iter().partition(|f| f.pinned);
    let (pinned_here, plain_here): (Vec<Placed>, Vec<Placed>) =
        here.iter().copied().partition(|v| v.pinned);

    let mut rows = Vec::with_capacity(pinned_folders.len() + plain_folders.len() + here.len() + 1);
    match (view, options.mode) {
        (View::Uncategorized, _) => {
            rows.extend(pinned_here.iter().chain(&plain_here).map(Placed::row));
        }
        (View::Root, ViewMode::UncategorizedFolder) => {
            rows.extend(pinned_folders.iter().chain(&plain_folders).map(Folder::row));
            if !here.is_empty() {
                rows.push(Row::Uncategorized {
                    vehicle_count: here.len(),
                    preview: pinned_here
                        .iter()
                        .chain(&plain_here)
                        .take(PREVIEW_LEN)
                        .map(|vehicle| vehicle.index)
                        .collect(),
                });
            }
        }
        (_, ViewMode::Mixed) => {
            rows.extend(pinned_folders.iter().map(Folder::row));
            rows.extend(pinned_here.iter().map(Placed::row));
            rows.extend(interleave(
                plain_folders,
                &plain_here,
                items,
                options.sort_by_title,
            ));
        }
        _ => {
            rows.extend(pinned_folders.iter().map(Folder::row));
            rows.extend(pinned_here.iter().map(Placed::row));
            rows.extend(plain_folders.iter().map(Folder::row));
            rows.extend(plain_here.iter().map(Placed::row));
        }
    }
    rows
}

/// Every item, the pinned ones first (each group in the game's order).
fn flat(tree: &Tree, items: &[Item]) -> Vec<Row> {
    let mut rows = Vec::with_capacity(items.len());
    let mut unpinned = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if tree.is_pinned(&item.id) {
            rows.push(Row::Vehicle {
                index,
                pinned: true,
            });
        } else {
            unpinned.push(Row::Vehicle {
                index,
                pinned: false,
            });
        }
    }
    rows.append(&mut unpinned);
    rows
}

/// The place being listed, resolved against the tree.
#[derive(Clone, Copy)]
enum View<'a> {
    Root,
    Folder(&'a FolderPath),
    Uncategorized,
}

impl<'a> View<'a> {
    /// A folder that doesn't exist is the root; Uncategorized only exists in its own view mode.
    fn of(tree: &'a Tree, options: &ListOptions) -> Self {
        match &options.location {
            Location::Folder(path) => tree.folder(path).map_or(View::Root, View::Folder),
            Location::Uncategorized if options.mode == ViewMode::UncategorizedFolder => {
                View::Uncategorized
            }
            _ => View::Root,
        }
    }

    /// The folder whose subfolders are listed.
    fn folder(&self) -> Option<&'a FolderPath> {
        match self {
            View::Folder(path) => Some(path),
            _ => None,
        }
    }

    /// How a vehicle filed in `folder` (`None`: unfiled) relates to what's being listed.
    fn relation<'f>(&self, folder: Option<&'f FolderPath>) -> Relation<'f> {
        match (self, folder) {
            (View::Root | View::Uncategorized, None) => Relation::Here,
            (View::Root, Some(folder)) => Relation::Below(folder.key_prefix(1)),
            (View::Folder(here), Some(folder)) if folder == *here => Relation::Here,
            (View::Folder(here), Some(folder)) if folder.is_inside(here) => {
                Relation::Below(folder.key_prefix(here.depth() + 1))
            }
            _ => Relation::Elsewhere,
        }
    }
}

enum Relation<'f> {
    /// Listed at this level.
    Here,
    /// In a subfolder of this level; holds that subfolder's key.
    Below(&'f str),
    /// Not part of this level.
    Elsewhere,
}

/// A vehicle listed at the current level.
#[derive(Clone, Copy)]
struct Placed {
    index: usize,
    pinned: bool,
}

impl Placed {
    fn row(&self) -> Row {
        Row::Vehicle {
            index: self.index,
            pinned: self.pinned,
        }
    }
}

/// A subfolder of the current level.
struct Folder<'a> {
    path: &'a FolderPath,
    pinned: bool,
    /// Listed vehicles in its whole subtree.
    vehicle_count: usize,
    /// The first of them in the game's order (an index into the items).
    first_index: Option<usize>,
    preview: Vec<usize>,
}

impl Folder<'_> {
    fn name(&self) -> &str {
        self.path.name()
    }

    fn row(&self) -> Row {
        Row::Folder {
            path: self.path.clone(),
            pinned: self.pinned,
            vehicle_count: self.vehicle_count,
            preview: self.preview.clone(),
        }
    }
}

/// What one pass gathers about a subfolder of the current level.
struct Subtree {
    /// Listed vehicles in it, in its subfolders too.
    count: usize,
    /// The first of them in the game's order.
    first: usize,
    /// The first few filed in it, pinned and not, and the first few deeper.
    pinned: Vec<usize>,
    plain: Vec<usize>,
    deeper: Vec<usize>,
}

impl Subtree {
    fn new(first: usize) -> Self {
        Self {
            count: 0,
            first,
            pinned: Vec::new(),
            plain: Vec::new(),
            deeper: Vec::new(),
        }
    }

    fn add(&mut self, index: usize, filed_here: bool, pinned: bool) {
        self.count += 1;
        let group = match (filed_here, pinned) {
            (true, true) => &mut self.pinned,
            (true, false) => &mut self.plain,
            (false, _) => &mut self.deeper,
        };
        if group.len() < PREVIEW_LEN {
            group.push(index);
        }
    }

    fn preview(&self) -> Vec<usize> {
        self.pinned
            .iter()
            .chain(&self.plain)
            .chain(&self.deeper)
            .take(PREVIEW_LEN)
            .copied()
            .collect()
    }
}

/// One pass over the items: those listed at this level, and the level's subfolders with their
/// counts and previews.
fn survey<'a>(tree: &'a Tree, items: &[Item], view: View<'a>) -> (Vec<Placed>, Vec<Folder<'a>>) {
    let mut here = Vec::new();
    // Subfolder key -> what's in its subtree.
    let mut below: HashMap<&str, Subtree> = HashMap::new();
    for (index, item) in items.iter().enumerate() {
        let (folder, pinned) = tree.placement(&item.id);
        match view.relation(folder) {
            Relation::Here => here.push(Placed { index, pinned }),
            Relation::Below(key) => {
                let filed_here = folder.is_some_and(|folder| folder.key() == key);
                below
                    .entry(key)
                    .or_insert_with(|| Subtree::new(index))
                    .add(index, filed_here, pinned);
            }
            Relation::Elsewhere => {}
        }
    }
    let folders = tree
        .subfolders(view.folder())
        .into_iter()
        .map(|path| {
            let stats = below.get(path.key());
            Folder {
                path,
                pinned: tree.is_folder_pinned(path),
                vehicle_count: stats.map_or(0, |stats| stats.count),
                first_index: stats.map(|stats| stats.first),
                preview: stats.map_or_else(Vec::new, Subtree::preview),
            }
        })
        .collect();
    (here, folders)
}

/// The newest times among a folder's vehicles: when the last one was made, and changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Newest {
    pub created: Option<i64>,
    pub updated: Option<i64>,
}

/// The newest creation and update times among `vehicles` (id, created, updated) filed in `folder`
/// or its subfolders. A vehicle whose folder doesn't exist (any more) is unfiled, as [`list`] has
/// it.
pub fn newest_in<'a>(
    tree: &Tree,
    folder: &FolderPath,
    vehicles: impl IntoIterator<Item = (&'a str, i64, i64)>,
) -> Newest {
    let later = |known: Option<i64>, time: i64| Some(known.map_or(time, |known| known.max(time)));
    let mut newest = Newest::default();
    for (id, created, updated) in vehicles {
        let Some(filed) = tree.folder_of(id) else {
            continue;
        };
        if filed == folder || filed.is_inside(folder) {
            newest.created = later(newest.created, created);
            newest.updated = later(newest.updated, updated);
        }
    }
    newest
}

/// Mixes the unpinned folders (sorted by name) into the unpinned vehicles (in the game's order).
///
/// Sorted by title, a folder goes where its name falls among the titles (before an equal one).
/// Otherwise it goes where the first vehicle inside it would be, and folders without vehicles
/// come last, by name.
fn interleave(
    folders: Vec<Folder<'_>>,
    vehicles: &[Placed],
    items: &[Item],
    by_title: bool,
) -> Vec<Row> {
    let (folders, empty): (Vec<Folder>, Vec<Folder>) = if by_title {
        (folders, Vec::new())
    } else {
        let (mut with, empty): (Vec<Folder>, Vec<Folder>) =
            folders.into_iter().partition(|f| f.first_index.is_some());
        with.sort_by_key(|f| f.first_index);
        (with, empty)
    };

    let folder_goes_first = |folder: &Folder, vehicle: &Placed| -> bool {
        if by_title {
            natural_cmp(folder.name(), &items[vehicle.index].title) != Ordering::Greater
        } else {
            folder
                .first_index
                .is_some_and(|first| first < vehicle.index)
        }
    };

    let mut rows = Vec::with_capacity(folders.len() + vehicles.len() + empty.len());
    let mut folders = folders.into_iter().peekable();
    for vehicle in vehicles {
        while let Some(folder) = folders.next_if(|folder| folder_goes_first(folder, vehicle)) {
            rows.push(folder.row());
        }
        rows.push(vehicle.row());
    }
    rows.extend(folders.map(|folder| folder.row()));
    rows.extend(empty.iter().map(Folder::row));
    rows
}

#[cfg(test)]
mod tests;
