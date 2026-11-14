//! `Tree`: one category's folders and the vehicles filed in them.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use crate::path::{FolderPath, MAX_DEPTH, PathError};

/// Why a folder operation was refused. The tree is unchanged then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderError {
    /// The name or the resulting path is invalid.
    InvalidPath(PathError),
    /// The folder doesn't exist.
    NotFound(FolderPath),
    /// A sibling already has that name (compared ignoring case). Holds the folder in the way.
    NameTaken(FolderPath),
    /// A folder can't go into itself or one of its own subfolders.
    IntoOwnSubtree,
}

impl fmt::Display for FolderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath(error) => error.fmt(f),
            Self::NotFound(path) => write!(f, "the folder \"{path}\" doesn't exist"),
            Self::NameTaken(path) => write!(f, "there is already a folder \"{path}\""),
            Self::IntoOwnSubtree => write!(f, "a folder can't be moved into itself"),
        }
    }
}

impl std::error::Error for FolderError {}

impl From<PathError> for FolderError {
    fn from(error: PathError) -> Self {
        Self::InvalidPath(error)
    }
}

/// What the tree remembers about one vehicle.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VehicleEntry {
    /// The folder it's filed in; `None` is the top level.
    pub folder: Option<FolderPath>,
    /// Whether the vehicle is pinned.
    pub pinned: bool,
    /// The vehicle's creation time (unix seconds), to tell a reused id from the same vehicle.
    pub created: Option<i64>,
}

impl VehicleEntry {
    /// An entry with neither a folder nor a pin says nothing, so it isn't kept.
    fn is_worth_keeping(&self) -> bool {
        self.folder.is_some() || self.pinned
    }
}

/// Where a new vehicle (a duplicate, a Save As, a new one) goes, for [`Tree::adopt`].
#[derive(Debug, Clone, Copy)]
pub enum Origin<'a> {
    /// Into the folder of this vehicle id (nowhere special if that one is unfiled).
    SameFolderAs(&'a str),
    /// Into this folder (`None`: the top level), e.g. the folder the browser has open.
    Folder(Option<&'a FolderPath>),
}

/// The folders of one category (Local or Subscribed) and which vehicles are filed or pinned.
///
/// Vehicles are only ever mentioned by id; the tree knows nothing about whether they exist
/// ([`Tree::prune`] cleans up). Folder names are unique among siblings ignoring case.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tree {
    /// Every folder, parents included. A set of paths that compare ignoring case, so it also
    /// holds the casing to show.
    folders: BTreeSet<FolderPath>,
    pinned_folders: BTreeSet<FolderPath>,
    /// When folders were made (unix seconds), for those made since the tree remembers it.
    folder_created: BTreeMap<FolderPath, i64>,
    vehicles: HashMap<String, VehicleEntry>,
}

impl Tree {
    /// An empty tree.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether there are no folders and nothing is filed or pinned.
    pub fn is_empty(&self) -> bool {
        self.folders.is_empty() && self.pinned_folders.is_empty() && self.vehicles.is_empty()
    }

    // ---- Reading ----

    /// Every folder, parents before their children, otherwise in name order (ignoring case).
    pub fn folders(&self) -> impl Iterator<Item = &FolderPath> {
        self.folders.iter()
    }

    /// Whether the folder exists.
    pub fn contains_folder(&self, path: &FolderPath) -> bool {
        self.folders.contains(path)
    }

    /// The folder as the tree has it (with the casing to show), if it exists.
    pub fn folder(&self, path: &FolderPath) -> Option<&FolderPath> {
        self.folders.get(path)
    }

    /// The folders directly inside `parent` (`None`: the top level), in no particular order.
    pub fn subfolders(&self, parent: Option<&FolderPath>) -> Vec<&FolderPath> {
        let depth = parent.map_or(0, FolderPath::depth) + 1;
        self.folders
            .iter()
            .filter(|folder| {
                folder.depth() == depth && parent.is_none_or(|parent| folder.is_inside(parent))
            })
            .collect()
    }

    /// Whether the folder is pinned.
    pub fn is_folder_pinned(&self, path: &FolderPath) -> bool {
        self.pinned_folders.contains(path)
    }

    /// The pinned folders.
    pub fn pinned_folders(&self) -> impl Iterator<Item = &FolderPath> {
        self.pinned_folders.iter()
    }

    /// When the folder was made (unix seconds), if known.
    pub fn folder_created(&self, path: &FolderPath) -> Option<i64> {
        self.folder_created.get(path).copied()
    }

    /// Every folder whose making time is known, with it.
    pub fn folder_creation_times(&self) -> impl Iterator<Item = (&FolderPath, i64)> {
        self.folder_created.iter().map(|(path, &time)| (path, time))
    }

    /// Every vehicle the tree has something to say about (a folder or a pin), in no order.
    pub fn vehicles(&self) -> impl Iterator<Item = (&str, &VehicleEntry)> {
        self.vehicles.iter().map(|(id, entry)| (id.as_str(), entry))
    }

    /// The folder a vehicle is filed in; `None` for the top level, also when its folder is gone.
    pub fn folder_of(&self, id: &str) -> Option<&FolderPath> {
        self.vehicles
            .get(id)?
            .folder
            .as_ref()
            .and_then(|folder| self.folders.get(folder))
    }

    /// Whether the vehicle is pinned.
    pub fn is_pinned(&self, id: &str) -> bool {
        self.vehicles.get(id).is_some_and(|entry| entry.pinned)
    }

    /// A vehicle's folder (as [`Tree::folder_of`]) and pin, in one lookup.
    pub(crate) fn placement(&self, id: &str) -> (Option<&FolderPath>, bool) {
        match self.vehicles.get(id) {
            Some(entry) => (
                entry
                    .folder
                    .as_ref()
                    .and_then(|folder| self.folders.get(folder)),
                entry.pinned,
            ),
            None => (None, false),
        }
    }

    // ---- Folders ----

    /// Creates the folder `name` inside `parent` (`None`: the top level) and returns its path.
    pub fn create_folder(
        &mut self,
        parent: Option<&FolderPath>,
        name: &str,
    ) -> Result<FolderPath, FolderError> {
        let path = match parent {
            Some(parent) => self.existing(parent)?.child(name)?,
            None => FolderPath::top(name)?,
        };
        self.ensure_free(&path)?;
        self.folders.insert(path.clone());
        Ok(path)
    }

    /// A name no folder in `parent` (`None`: the top level) has yet (compared ignoring case):
    /// `base`, else `base 2`, `base 3`, ... An error if the names aren't valid there (`parent` as
    /// deep as folders go, `base` invalid or too long once numbered).
    pub fn free_folder_name(
        &self,
        parent: Option<&FolderPath>,
        base: &str,
    ) -> Result<String, PathError> {
        for n in 1.. {
            let name = if n == 1 {
                base.to_owned()
            } else {
                format!("{base} {n}")
            };
            let path = match parent {
                Some(parent) => parent.child(&name)?,
                None => FolderPath::top(&name)?,
            };
            if !self.contains_folder(&path) {
                return Ok(name);
            }
        }
        unreachable!("the folders are finite, so a number is free")
    }

    /// Renames a folder. Its subfolders, vehicles and pins follow. Changing only the casing of a
    /// name is fine. Returns the new path.
    pub fn rename_folder(
        &mut self,
        path: &FolderPath,
        new_name: &str,
    ) -> Result<FolderPath, FolderError> {
        let path = self.existing(path)?.clone();
        let new_path = match path.parent() {
            Some(parent) => parent.child(new_name)?,
            None => FolderPath::top(new_name)?,
        };
        if new_path != path {
            self.ensure_free(&new_path)?;
        }
        self.relocate(&path, &new_path);
        Ok(new_path)
    }

    /// Moves a folder, with everything in it, into `new_parent` (`None`: the top level). Returns
    /// its new path. Moving it where it already is changes nothing.
    pub fn move_folder(
        &mut self,
        path: &FolderPath,
        new_parent: Option<&FolderPath>,
    ) -> Result<FolderPath, FolderError> {
        let path = self.existing(path)?.clone();
        let new_parent = match new_parent {
            Some(parent) => Some(self.existing(parent)?.clone()),
            None => None,
        };
        if let Some(parent) = &new_parent
            && (*parent == path || parent.is_inside(&path))
        {
            return Err(FolderError::IntoOwnSubtree);
        }
        let new_path = match &new_parent {
            Some(parent) => parent.child(path.name())?,
            None => FolderPath::top(path.name())?,
        };
        if new_path == path {
            return Ok(path);
        }
        self.ensure_free(&new_path)?;
        // The deepest folder in the moved subtree must still fit.
        let deepest = self
            .folders
            .iter()
            .filter(|f| **f == path || f.is_inside(&path))
            .map(FolderPath::depth)
            .max();
        let extra_levels = deepest.map_or(0, |deepest| deepest - path.depth());
        if new_path.depth() + extra_levels > MAX_DEPTH {
            return Err(PathError::TooDeep.into());
        }
        self.relocate(&path, &new_path);
        Ok(new_path)
    }

    /// Deletes a folder. Vehicles are never deleted: its vehicles and subfolders move up into its
    /// parent. A subfolder that meets a folder of the same name there is merged into it (and is
    /// pinned if either was).
    pub fn delete_folder(&mut self, path: &FolderPath) -> Result<(), FolderError> {
        let path = self.existing(path)?.clone();
        let parent = path.parent();
        self.folders.remove(&path);
        self.pinned_folders.remove(&path);
        self.folder_created.remove(&path);
        // Everything inside loses one level; the vehicles filed directly in `path` land in `parent`.
        self.rebase_subtree(&path, parent.as_ref());
        self.vehicles.retain(|_, entry| entry.is_worth_keeping());
        Ok(())
    }

    /// Records when a folder was made (unix seconds), e.g. right after [`Tree::create_folder`].
    pub fn set_folder_created(&mut self, path: &FolderPath, time: i64) -> Result<(), FolderError> {
        let path = self.existing(path)?.clone();
        self.folder_created.insert(path, time);
        Ok(())
    }

    /// Pins or unpins a folder.
    pub fn set_folder_pinned(
        &mut self,
        path: &FolderPath,
        pinned: bool,
    ) -> Result<(), FolderError> {
        let path = self.existing(path)?.clone();
        if pinned {
            self.pinned_folders.insert(path);
        } else {
            self.pinned_folders.remove(&path);
        }
        Ok(())
    }

    // ---- Vehicles ----

    /// Files a vehicle into `folder` (`None`: the top level). The folder must exist.
    pub fn move_vehicle(
        &mut self,
        id: &str,
        folder: Option<&FolderPath>,
    ) -> Result<(), FolderError> {
        match folder {
            Some(folder) => {
                let folder = self.existing(folder)?.clone();
                self.vehicles.entry(id.to_owned()).or_default().folder = Some(folder);
            }
            None => {
                if let Some(entry) = self.vehicles.get_mut(id) {
                    entry.folder = None;
                    self.forget_if_pointless(id);
                }
            }
        }
        Ok(())
    }

    /// Pins or unpins a vehicle.
    pub fn set_vehicle_pinned(&mut self, id: &str, pinned: bool) {
        if pinned {
            self.vehicles.entry(id.to_owned()).or_default().pinned = true;
        } else if let Some(entry) = self.vehicles.get_mut(id) {
            entry.pinned = false;
            self.forget_if_pointless(id);
        }
    }

    /// Forgets one vehicle (the game deleted it), so a new vehicle reusing its id starts unfiled.
    /// Returns whether the tree knew it.
    pub fn forget_vehicle(&mut self, id: &str) -> bool {
        self.vehicles.remove(id).is_some()
    }

    /// Registers a new vehicle (a duplicate, a Save As, a new one) with its creation time and
    /// files it per `origin`. Whatever the tree knew under that id is replaced: the id is new.
    /// A pin is not copied from the original.
    pub fn adopt(
        &mut self,
        new_id: &str,
        created: Option<i64>,
        origin: Origin<'_>,
    ) -> Result<(), FolderError> {
        let folder = match origin {
            Origin::SameFolderAs(id) => self.folder_of(id).cloned(),
            Origin::Folder(Some(folder)) => Some(self.existing(folder)?.clone()),
            Origin::Folder(None) => None,
        };
        self.vehicles.remove(new_id);
        if folder.is_some() {
            self.vehicles.insert(
                new_id.to_owned(),
                VehicleEntry {
                    folder,
                    pinned: false,
                    created,
                },
            );
        }
        Ok(())
    }

    // ---- Cleaning up ----

    /// Forgets vehicles that are gone, given the vehicles that exist now as (id, creation time).
    ///
    /// An entry goes when its id isn't there, or when the stored creation time differs from the
    /// vehicle's (the game reused the id for a new vehicle, which mustn't inherit the old one's
    /// folder). An entry without a stored creation time gets the vehicle's. Returns whether
    /// anything changed (the file needs saving).
    ///
    /// Only call it with the complete list: with a partial one it forgets the rest.
    pub fn prune<'a>(
        &mut self,
        existing: impl IntoIterator<Item = (&'a str, Option<i64>)>,
    ) -> bool {
        let existing: HashMap<&str, Option<i64>> = existing.into_iter().collect();
        let mut changed = false;
        self.vehicles.retain(|id, entry| {
            let Some(&current) = existing.get(id.as_str()) else {
                changed = true;
                return false;
            };
            match (entry.created, current) {
                (Some(stored), Some(now)) if stored != now => {
                    changed = true;
                    false
                }
                (None, Some(now)) => {
                    entry.created = Some(now);
                    changed = true;
                    true
                }
                _ => true,
            }
        });
        changed
    }

    // ---- For reading the file ----

    /// Adds a folder and the folders above it, as a file lists them.
    pub(crate) fn insert_with_ancestors(&mut self, path: &FolderPath) {
        for ancestor in path.ancestors_and_self() {
            self.folders.insert(ancestor);
        }
    }

    /// Pins a folder that exists; says whether it did.
    pub(crate) fn pin_existing_folder(&mut self, path: &FolderPath) -> bool {
        match self.folders.get(path) {
            Some(folder) => {
                self.pinned_folders.insert(folder.clone());
                true
            }
            None => false,
        }
    }

    /// Stores a vehicle's entry as read from a file (dropped if it says nothing). The folder is
    /// added to the tree if it isn't there.
    pub(crate) fn insert_vehicle(&mut self, id: &str, mut entry: VehicleEntry) {
        if let Some(folder) = &entry.folder {
            self.insert_with_ancestors(folder);
            entry.folder = self.folders.get(folder).cloned();
        }
        if entry.is_worth_keeping() {
            self.vehicles.insert(id.to_owned(), entry);
        }
    }

    /// Tests only: removes a folder but not what's filed in it, the stale state a vehicle's
    /// entry can be left in.
    #[cfg(test)]
    pub(crate) fn remove_folder_only(&mut self, path: &FolderPath) {
        self.folders.remove(path);
    }

    // ---- Helpers ----

    /// The folder as the tree has it, or `NotFound`.
    fn existing(&self, path: &FolderPath) -> Result<&FolderPath, FolderError> {
        self.folders
            .get(path)
            .ok_or_else(|| FolderError::NotFound(path.clone()))
    }

    /// `NameTaken` unless no folder is at `path` (compared ignoring case).
    fn ensure_free(&self, path: &FolderPath) -> Result<(), FolderError> {
        match self.folders.get(path) {
            Some(taken) => Err(FolderError::NameTaken(taken.clone())),
            None => Ok(()),
        }
    }

    /// Drops a vehicle's entry once it has neither a folder nor a pin.
    fn forget_if_pointless(&mut self, id: &str) {
        if self
            .vehicles
            .get(id)
            .is_some_and(|entry| !entry.is_worth_keeping())
        {
            self.vehicles.remove(id);
        }
    }

    /// Moves the folder `from` and everything in it to `to` (a path that may only differ in
    /// casing from `from`). `to` must be free, or the same folder.
    fn relocate(&mut self, from: &FolderPath, to: &FolderPath) {
        self.rebase_subtree(from, Some(to));
    }

    /// Replaces the leading `from` by `to` (`None`: the root) in every folder at or below `from`,
    /// in their pins, making times and the vehicles filed there. A folder that lands on an
    /// existing one merges into it (keeping the earlier making time).
    fn rebase_subtree(&mut self, from: &FolderPath, to: Option<&FolderPath>) {
        let inside = |folder: &FolderPath| folder == from || folder.is_inside(from);
        let moved: Vec<FolderPath> = self.folders.iter().filter(|f| inside(f)).cloned().collect();
        let moved_pins: Vec<FolderPath> = self
            .pinned_folders
            .iter()
            .filter(|f| inside(f))
            .cloned()
            .collect();
        for folder in &moved {
            self.folders.remove(folder);
        }
        for folder in &moved_pins {
            self.pinned_folders.remove(folder);
        }
        let moved_times: Vec<(FolderPath, i64)> = moved
            .iter()
            .filter_map(|folder| Some((folder.clone(), self.folder_created.remove(folder)?)))
            .collect();
        // Parents come before their children, so a child lands under its parent's final spelling
        // (a merge keeps the spelling of the folder that was already there).
        for folder in moved.iter().filter_map(|folder| folder.rebased(from, to)) {
            let folder = match folder.parent().and_then(|parent| self.folders.get(&parent)) {
                Some(parent) => parent.child(folder.name()).unwrap_or(folder),
                None => folder,
            };
            self.folders.insert(folder);
        }
        for folder in moved_pins
            .iter()
            .filter_map(|folder| folder.rebased(from, to))
        {
            if let Some(folder) = self.folders.get(&folder) {
                self.pinned_folders.insert(folder.clone());
            }
        }
        for (folder, time) in moved_times {
            let Some(folder) = folder
                .rebased(from, to)
                .and_then(|folder| self.folders.get(&folder).cloned())
            else {
                continue;
            };
            // Removed and inserted again: a merge's surviving key keeps its own spelling.
            let earliest = self
                .folder_created
                .remove(&folder)
                .map_or(time, |t| t.min(time));
            self.folder_created.insert(folder, earliest);
        }
        for entry in self.vehicles.values_mut() {
            if let Some(folder) = &entry.folder
                && inside(folder)
            {
                entry.folder = folder
                    .rebased(from, to)
                    .and_then(|folder| self.folders.get(&folder).cloned());
            }
        }
    }
}

#[cfg(test)]
mod tests;
