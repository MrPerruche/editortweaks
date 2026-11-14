//! Where the browser is, and getting around: locations, back/forward/up, breadcrumbs.

use crate::path::FolderPath;
use crate::tree::Tree;

/// The label of the top level in [`breadcrumbs`].
pub const ROOT_LABEL: &str = "Home";
/// The label of the [`Location::Uncategorized`] row and breadcrumb.
pub const UNCATEGORIZED_LABEL: &str = "Uncategorized";

/// A place the browser can show.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Location {
    /// The top level.
    #[default]
    Root,
    /// A folder.
    Folder(FolderPath),
    /// The virtual folder of vehicles that are in no folder (only in the
    /// [`ViewMode::UncategorizedFolder`](crate::ViewMode::UncategorizedFolder) view).
    Uncategorized,
}

impl Location {
    /// The location one step up: a folder's parent, the root from a top-level folder or from
    /// Uncategorized. `None` at the root.
    pub fn parent(&self) -> Option<Location> {
        match self {
            Location::Root => None,
            Location::Uncategorized => Some(Location::Root),
            Location::Folder(path) => Some(path.parent().map_or(Location::Root, Location::Folder)),
        }
    }
}

/// The clickable path above the list: each part's label and where it leads, from the top level
/// down to `location` itself.
pub fn breadcrumbs(location: &Location) -> Vec<(String, Location)> {
    let mut crumbs = vec![(ROOT_LABEL.to_owned(), Location::Root)];
    match location {
        Location::Root => {}
        Location::Uncategorized => {
            crumbs.push((UNCATEGORIZED_LABEL.to_owned(), Location::Uncategorized))
        }
        Location::Folder(path) => {
            crumbs.extend(
                path.ancestors_and_self()
                    .into_iter()
                    .map(|ancestor| (ancestor.name().to_owned(), Location::Folder(ancestor))),
            );
        }
    }
    crumbs
}

/// The current location with back and forward, like a file manager's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    current: Location,
    /// Older locations, the most recent last.
    back: Vec<Location>,
    /// Locations left by going back, the next one last.
    forward: Vec<Location>,
}

impl History {
    /// A history at the top level.
    pub fn new() -> Self {
        Self::default()
    }

    /// Where the browser is.
    pub fn current(&self) -> &Location {
        &self.current
    }

    /// Whether [`History::back`] has somewhere to go.
    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    /// Whether [`History::forward`] has somewhere to go.
    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    /// Whether [`History::up`] has somewhere to go (not at the top level).
    pub fn can_go_up(&self) -> bool {
        self.current != Location::Root
    }

    /// Goes to a location, forgetting the forward list. Going where it already is does nothing.
    pub fn go(&mut self, location: Location) {
        if location == self.current {
            return;
        }
        let left = std::mem::replace(&mut self.current, location);
        self.back.push(left);
        self.forward.clear();
    }

    /// Goes back one location. Returns whether it moved.
    pub fn back(&mut self) -> bool {
        match self.back.pop() {
            Some(previous) => {
                let left = std::mem::replace(&mut self.current, previous);
                self.forward.push(left);
                true
            }
            None => false,
        }
    }

    /// Goes forward again after going back. Returns whether it moved.
    pub fn forward(&mut self) -> bool {
        match self.forward.pop() {
            Some(next) => {
                let left = std::mem::replace(&mut self.current, next);
                self.back.push(left);
                true
            }
            None => false,
        }
    }

    /// Goes up to the parent folder (or the top level, from Uncategorized), as a new step in the
    /// history. Returns whether it moved.
    pub fn up(&mut self) -> bool {
        match self.current.parent() {
            Some(parent) => {
                self.go(parent);
                true
            }
            None => false,
        }
    }

    /// Repairs the history after folders were renamed, moved or deleted: a folder that's gone is
    /// dropped from the lists, and if the current location is gone the nearest folder above it
    /// that exists is current (else the top level). Folders are shown as the tree spells them.
    pub fn fix_up(&mut self, tree: &Tree) {
        self.current = nearest_surviving(tree, &self.current);
        for list in [&mut self.back, &mut self.forward] {
            let kept: Vec<Location> = list
                .drain(..)
                .filter_map(|location| surviving(tree, &location))
                .collect();
            *list = kept;
            list.dedup();
        }
        // Steps that now lead where we already are are pointless.
        while self.back.last() == Some(&self.current) {
            self.back.pop();
        }
        while self.forward.last() == Some(&self.current) {
            self.forward.pop();
        }
    }
}

/// The location as the tree spells it, if it still exists. Uncategorized and the root always do.
fn surviving(tree: &Tree, location: &Location) -> Option<Location> {
    match location {
        Location::Folder(path) => tree.folder(path).cloned().map(Location::Folder),
        other => Some(other.clone()),
    }
}

/// The location if it exists, else (for a folder) the nearest folder above it that does, else the
/// top level.
fn nearest_surviving(tree: &Tree, location: &Location) -> Location {
    match location {
        Location::Folder(path) => path
            .ancestors_and_self()
            .into_iter()
            .rev()
            .find_map(|candidate| tree.folder(&candidate).cloned())
            .map_or(Location::Root, Location::Folder),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests;
