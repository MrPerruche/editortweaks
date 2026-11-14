//! `Store`: both categories' trees and their file, `EditorTweaks-Folders.toml`.
//!
//! The file is kept readable by every build, past and future: a `version`, every field optional,
//! unknown fields ignored, and bad entries skipped with a warning instead of failing the file.
//! Reading goes through `toml::Table` by hand for that: a type serde can't turn into ours (a
//! field some later version changed) costs that field, not the whole file.

use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;
use toml::{Table, Value};

use crate::path::FolderPath;
use crate::tree::{Tree, VehicleEntry};

/// The version this build writes.
pub const FILE_VERSION: u32 = 1;

/// The two kinds of vehicles that have their own folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    /// The player's own vehicles (id: the vehicle's folder name).
    Local,
    /// Subscribed Workshop vehicles (id: the Workshop item id).
    Subscribed,
}

/// Both categories' folders. Each category has its own tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Store {
    /// The local vehicles' tree.
    pub local: Tree,
    /// The subscribed vehicles' tree.
    pub subscribed: Tree,
}

/// A file that isn't TOML at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not a valid folders file: {}", self.0)
    }
}

impl std::error::Error for ParseError {}

/// A read file: what could be understood, and what was skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// The folders and pins that were read.
    pub store: Store,
    /// One line per skipped or odd part, for the log.
    pub warnings: Vec<String>,
}

impl Store {
    /// The tree of a category.
    pub fn tree(&self, category: Category) -> &Tree {
        match category {
            Category::Local => &self.local,
            Category::Subscribed => &self.subscribed,
        }
    }

    /// The tree of a category, to change.
    pub fn tree_mut(&mut self, category: Category) -> &mut Tree {
        match category {
            Category::Local => &mut self.local,
            Category::Subscribed => &mut self.subscribed,
        }
    }

    /// Forgets local vehicles that are gone or whose id was reused ([`Tree::prune`]). Subscribed
    /// ones are never pruned this way: an item unsubscribed and subscribed again keeps its folder.
    pub fn prune_local<'a>(
        &mut self,
        existing: impl IntoIterator<Item = (&'a str, Option<i64>)>,
    ) -> bool {
        self.local.prune(existing)
    }

    /// Reads a file's text. Only text that isn't TOML is an error; anything in it that is
    /// invalid or unknown is skipped, with a warning for what was lost.
    pub fn parse(text: &str) -> Result<Parsed, ParseError> {
        let table: Table = text
            .parse()
            .map_err(|error: toml::de::Error| ParseError(error.message().to_owned()))?;
        let mut warnings = Vec::new();
        if let Some(version) = table.get("version").and_then(Value::as_integer)
            && version > i64::from(FILE_VERSION)
        {
            warnings.push(format!(
                "version {version} is newer than this build's {FILE_VERSION}: reading what it knows, \
                 and saving will drop the rest"
            ));
        }
        let store = Store {
            local: read_tree("local", table.get("local"), &mut warnings),
            subscribed: read_tree("subscribed", table.get("subscribed"), &mut warnings),
        };
        Ok(Parsed { store, warnings })
    }

    /// The file's text. Sorted, so the same folders always give the same text.
    pub fn to_toml(&self) -> String {
        let file = FileOut {
            version: FILE_VERSION,
            local: TreeOut::of(&self.local),
            subscribed: TreeOut::of(&self.subscribed),
        };
        // Our own plain data always serializes.
        toml::to_string(&file).unwrap_or_default()
    }
}

// ---- Reading ----

fn read_tree(section: &str, value: Option<&Value>, warnings: &mut Vec<String>) -> Tree {
    let mut tree = Tree::new();
    let Some(value) = value else {
        return tree;
    };
    let Some(table) = value.as_table() else {
        warnings.push(format!("[{section}] is not a table, skipped"));
        return tree;
    };

    for path in read_paths(section, "folders", table, warnings) {
        tree.insert_with_ancestors(&path);
    }

    if let Some(vehicles) = table.get("vehicles") {
        match vehicles.as_table() {
            Some(vehicles) => {
                for (id, entry) in vehicles {
                    read_vehicle(section, id, entry, &mut tree, warnings);
                }
            }
            None => warnings.push(format!("[{section}] vehicles is not a table, skipped")),
        }
    }

    // After the vehicles, whose folders may have added some.
    for path in read_paths(section, "pinned_folders", table, warnings) {
        if !tree.pin_existing_folder(&path) {
            warnings.push(format!(
                "[{section}] pinned folder \"{path}\" doesn't exist, skipped"
            ));
        }
    }
    read_creation_times(section, table, &mut tree, warnings);
    tree
}

/// The folders' making times (`folder_created`, a table of path = unix seconds), for folders that
/// exist; anything else is skipped with a warning.
fn read_creation_times(section: &str, table: &Table, tree: &mut Tree, warnings: &mut Vec<String>) {
    let Some(value) = table.get("folder_created") else {
        return;
    };
    let Some(times) = value.as_table() else {
        warnings.push(format!(
            "[{section}] folder_created is not a table, skipped"
        ));
        return;
    };
    for (path, time) in times {
        let path = match FolderPath::new(path) {
            Ok(path) => path,
            Err(error) => {
                warnings.push(format!("[{section}] folder_created: {error}: {path}"));
                continue;
            }
        };
        let Some(time) = time.as_integer() else {
            warnings.push(format!(
                "[{section}] folder_created \"{path}\" is not a number, skipped"
            ));
            continue;
        };
        if tree.set_folder_created(&path, time).is_err() {
            warnings.push(format!(
                "[{section}] folder_created \"{path}\" doesn't exist, skipped"
            ));
        }
    }
}

/// A list of folder paths in a table's field; invalid ones are skipped with a warning.
fn read_paths(
    section: &str,
    key: &str,
    table: &Table,
    warnings: &mut Vec<String>,
) -> Vec<FolderPath> {
    let Some(value) = table.get(key) else {
        return Vec::new();
    };
    let Some(list) = value.as_array() else {
        warnings.push(format!("[{section}] {key} is not a list, skipped"));
        return Vec::new();
    };
    let mut paths = Vec::new();
    for item in list {
        match item.as_str().map(FolderPath::new) {
            Some(Ok(path)) => paths.push(path),
            Some(Err(error)) => warnings.push(format!("[{section}] {key}: {error}: {item}")),
            None => warnings.push(format!("[{section}] {key}: {item} is not text, skipped")),
        }
    }
    paths
}

fn read_vehicle(
    section: &str,
    id: &str,
    value: &Value,
    tree: &mut Tree,
    warnings: &mut Vec<String>,
) {
    let Some(table) = value.as_table() else {
        warnings.push(format!(
            "[{section}] vehicle \"{id}\" is not a table, skipped"
        ));
        return;
    };
    if id.trim().is_empty() {
        warnings.push(format!("[{section}] a vehicle has an empty id, skipped"));
        return;
    }
    let mut entry = VehicleEntry::default();
    if let Some(folder) = table.get("folder") {
        match folder.as_str().map(FolderPath::new) {
            Some(Ok(path)) => entry.folder = Some(path),
            Some(Err(error)) => warnings.push(format!(
                "[{section}] vehicle \"{id}\" folder: {error}: {folder}"
            )),
            None => warnings.push(format!(
                "[{section}] vehicle \"{id}\" folder is not text, skipped"
            )),
        }
    }
    if let Some(pinned) = table.get("pinned") {
        match pinned.as_bool() {
            Some(pinned) => entry.pinned = pinned,
            None => warnings.push(format!(
                "[{section}] vehicle \"{id}\" pinned is not true/false, skipped"
            )),
        }
    }
    if let Some(created) = table.get("created") {
        match created.as_integer() {
            Some(created) => entry.created = Some(created),
            None => warnings.push(format!(
                "[{section}] vehicle \"{id}\" created is not a number, skipped"
            )),
        }
    }
    tree.insert_vehicle(id, entry);
}

// ---- Writing ----

#[derive(Serialize)]
struct FileOut {
    version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    local: Option<TreeOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subscribed: Option<TreeOut>,
}

#[derive(Serialize)]
struct TreeOut {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    folders: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pinned_folders: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    folder_created: BTreeMap<String, i64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    vehicles: BTreeMap<String, VehicleOut>,
}

#[derive(Serialize)]
struct VehicleOut {
    #[serde(skip_serializing_if = "Option::is_none")]
    folder: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pinned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    created: Option<i64>,
}

impl TreeOut {
    /// `None` for a tree with nothing to write.
    fn of(tree: &Tree) -> Option<Self> {
        if tree.is_empty() {
            return None;
        }
        Some(Self {
            folders: tree.folders().map(ToString::to_string).collect(),
            pinned_folders: tree.pinned_folders().map(ToString::to_string).collect(),
            folder_created: tree
                .folder_creation_times()
                .map(|(path, time)| (path.to_string(), time))
                .collect(),
            vehicles: tree
                .vehicles()
                .map(|(id, entry)| {
                    let out = VehicleOut {
                        folder: entry.folder.as_ref().map(ToString::to_string),
                        pinned: entry.pinned,
                        created: entry.created,
                    };
                    (id.to_owned(), out)
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests;
