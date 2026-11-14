//! `FolderPath`: a validated folder path such as `Planes/Jets`.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;

/// Longest folder name, in characters.
pub const MAX_NAME_CHARS: usize = 64;
/// Deepest nesting: `A/B/C` is three levels.
pub const MAX_DEPTH: usize = 16;

/// Joins the lowercased names in a path's comparison key. It sorts before every character a name
/// can hold (no control characters), so sorting keys sorts a tree depth-first, parents first.
const KEY_SEPARATOR: &str = "\u{1}";

/// Why a folder name or path was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// The whole path is empty.
    Empty,
    /// A name is empty (after trimming), as in `A//B`.
    EmptyName,
    /// A name is longer than [`MAX_NAME_CHARS`].
    NameTooLong,
    /// A name holds `\`, `/` or a control character.
    ForbiddenChar(char),
    /// More than [`MAX_DEPTH`] levels.
    TooDeep,
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "the folder path is empty"),
            Self::EmptyName => write!(f, "a folder name is empty"),
            Self::NameTooLong => write!(
                f,
                "a folder name is longer than {MAX_NAME_CHARS} characters"
            ),
            Self::ForbiddenChar(c) => write!(f, "a folder name can't contain {c:?}"),
            Self::TooDeep => write!(f, "folders can only be nested {MAX_DEPTH} levels deep"),
        }
    }
}

impl std::error::Error for PathError {}

/// Trims a folder name and checks it. Returns the trimmed name.
pub fn clean_name(name: &str) -> Result<&str, PathError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(PathError::EmptyName);
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(PathError::NameTooLong);
    }
    match name
        .chars()
        .find(|c| matches!(c, '/' | '\\') || c.is_control())
    {
        Some(c) => Err(PathError::ForbiddenChar(c)),
        None => Ok(name),
    }
}

/// A folder's path: one or more valid names, e.g. `Planes/Jets`.
///
/// The user's casing is kept for display, but two paths are **equal when they differ only in
/// case** (`Jets` and `jets` are the same folder). Ordering is depth-first with parents before
/// their children.
#[derive(Debug, Clone)]
pub struct FolderPath {
    names: Vec<String>,
    /// The names lowercased and joined with [`KEY_SEPARATOR`]: what equality and ordering use.
    key: String,
}

impl FolderPath {
    /// Parses a full path like `A/B` (each name trimmed and checked, none empty).
    pub fn new(path: &str) -> Result<Self, PathError> {
        if path.trim().is_empty() {
            return Err(PathError::Empty);
        }
        let names = path
            .split('/')
            .map(|name| clean_name(name).map(str::to_owned))
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_names(names)
    }

    /// A top-level folder.
    pub fn top(name: &str) -> Result<Self, PathError> {
        Self::from_names(vec![clean_name(name)?.to_owned()])
    }

    /// The folder `name` inside this one.
    pub fn child(&self, name: &str) -> Result<Self, PathError> {
        let mut names = self.names.clone();
        names.push(clean_name(name)?.to_owned());
        Self::from_names(names)
    }

    /// The folder this one is in; `None` for a top-level folder (its parent is the root).
    pub fn parent(&self) -> Option<Self> {
        (self.names.len() > 1).then(|| Self::build(self.names[..self.names.len() - 1].to_vec()))
    }

    /// This folder's own name (the last part).
    pub fn name(&self) -> &str {
        // A path always has at least one name.
        self.names.last().map_or("", String::as_str)
    }

    /// The names from the top level down.
    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.names.iter().map(String::as_str)
    }

    /// How many levels deep: 1 for a top-level folder.
    pub fn depth(&self) -> usize {
        self.names.len()
    }

    /// Whether this folder is strictly below `other` (any depth). A folder is not inside itself.
    pub fn is_inside(&self, other: &Self) -> bool {
        self.key.len() > other.key.len()
            && self.key.starts_with(&other.key)
            && self.key[other.key.len()..].starts_with(KEY_SEPARATOR)
    }

    /// The first `depth` levels of this path (1 <= depth <= `self.depth()`).
    pub(crate) fn prefix(&self, depth: usize) -> Self {
        Self::build(self.names[..depth.min(self.names.len())].to_vec())
    }

    /// This folder and every folder above it, outermost first.
    pub(crate) fn ancestors_and_self(&self) -> Vec<Self> {
        (1..=self.names.len())
            .map(|depth| self.prefix(depth))
            .collect()
    }

    /// The comparison key of this path (see the type's docs).
    pub(crate) fn key(&self) -> &str {
        &self.key
    }

    /// The comparison key of the first `depth` levels, without allocating.
    pub(crate) fn key_prefix(&self, depth: usize) -> &str {
        if depth == 0 {
            return "";
        }
        match self.key.match_indices(KEY_SEPARATOR).nth(depth - 1) {
            Some((end, _)) => &self.key[..end],
            None => &self.key,
        }
    }

    /// Replaces the leading `from` (this path, or a folder above it) by `to`; `None` is the
    /// root. `None` comes back when nothing is left, i.e. this path is `from` and `to` the root.
    /// Doesn't check the depth: the caller does.
    pub(crate) fn rebased(&self, from: &Self, to: Option<&Self>) -> Option<Self> {
        let mut names = to.map(|to| to.names.clone()).unwrap_or_default();
        names.extend(self.names[from.names.len()..].iter().cloned());
        (!names.is_empty()).then(|| Self::build(names))
    }

    fn from_names(names: Vec<String>) -> Result<Self, PathError> {
        if names.len() > MAX_DEPTH {
            return Err(PathError::TooDeep);
        }
        Ok(Self::build(names))
    }

    /// `names` must be valid, non-empty and not too many.
    fn build(names: Vec<String>) -> Self {
        let key = names
            .iter()
            .map(|name| name.to_lowercase())
            .collect::<Vec<_>>()
            .join(KEY_SEPARATOR);
        Self { names, key }
    }
}

impl fmt::Display for FolderPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.names.join("/"))
    }
}

impl FromStr for FolderPath {
    type Err = PathError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl PartialEq for FolderPath {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for FolderPath {}

impl Hash for FolderPath {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key.hash(state);
    }
}

impl PartialOrd for FolderPath {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FolderPath {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> FolderPath {
        FolderPath::new(s).unwrap()
    }

    #[test]
    fn parses_and_displays() {
        let p = path("Planes/Jets");
        assert_eq!(p.to_string(), "Planes/Jets");
        assert_eq!(p.name(), "Jets");
        assert_eq!(p.depth(), 2);
        assert_eq!(p.components().collect::<Vec<_>>(), ["Planes", "Jets"]);
    }

    #[test]
    fn names_are_trimmed() {
        assert_eq!(path("  A  /  B ").to_string(), "A/B");
    }

    #[test]
    fn rejects_empty_paths_and_names() {
        assert_eq!(FolderPath::new(""), Err(PathError::Empty));
        assert_eq!(FolderPath::new("   "), Err(PathError::Empty));
        assert_eq!(FolderPath::new("A//B"), Err(PathError::EmptyName));
        assert_eq!(FolderPath::new("/A"), Err(PathError::EmptyName));
        assert_eq!(FolderPath::new("A/"), Err(PathError::EmptyName));
        assert_eq!(FolderPath::top(" "), Err(PathError::EmptyName));
    }

    #[test]
    fn rejects_forbidden_characters() {
        assert_eq!(FolderPath::new("A\\B"), Err(PathError::ForbiddenChar('\\')));
        assert_eq!(FolderPath::new("A\tB"), Err(PathError::ForbiddenChar('\t')));
        assert_eq!(
            FolderPath::new("A\u{1}B"),
            Err(PathError::ForbiddenChar('\u{1}'))
        );
        assert_eq!(FolderPath::top("A/B"), Err(PathError::ForbiddenChar('/')));
        assert_eq!(path("A").child("B/C"), Err(PathError::ForbiddenChar('/')));
    }

    #[test]
    fn name_length_limit() {
        let ok = "x".repeat(MAX_NAME_CHARS);
        assert!(FolderPath::top(&ok).is_ok());
        let long = "x".repeat(MAX_NAME_CHARS + 1);
        assert_eq!(FolderPath::top(&long), Err(PathError::NameTooLong));
        // Characters, not bytes.
        let wide = "é".repeat(MAX_NAME_CHARS);
        assert!(FolderPath::top(&wide).is_ok());
    }

    #[test]
    fn depth_limit() {
        let mut p = FolderPath::top("a").unwrap();
        for _ in 1..MAX_DEPTH {
            p = p.child("a").unwrap();
        }
        assert_eq!(p.depth(), MAX_DEPTH);
        assert_eq!(p.child("a"), Err(PathError::TooDeep));
        let too_deep = vec!["a"; MAX_DEPTH + 1].join("/");
        assert_eq!(FolderPath::new(&too_deep), Err(PathError::TooDeep));
    }

    #[test]
    fn parent_and_child() {
        let p = path("A/B/C");
        assert_eq!(p.parent(), Some(path("A/B")));
        assert_eq!(p.parent().unwrap().parent().unwrap().parent(), None);
        assert_eq!(path("A").child("B").unwrap(), path("A/B"));
    }

    #[test]
    fn equality_ignores_case_but_display_keeps_it() {
        assert_eq!(path("Planes/Jets"), path("planes/JETS"));
        assert_eq!(path("planes/JETS").to_string(), "planes/JETS");
        assert_ne!(path("A/B"), path("A/C"));
        assert_ne!(path("A B"), path("A/B"));
    }

    #[test]
    fn hash_follows_equality() {
        let mut set = std::collections::HashSet::new();
        set.insert(path("Jets"));
        assert!(set.contains(&path("jets")));
    }

    #[test]
    fn is_inside_is_strict_and_whole_names() {
        assert!(path("A/B").is_inside(&path("A")));
        assert!(path("a/b/c").is_inside(&path("A")));
        assert!(!path("A").is_inside(&path("A")));
        assert!(!path("A").is_inside(&path("A/B")));
        assert!(!path("AB").is_inside(&path("A")));
        assert!(!path("A B/C").is_inside(&path("A")));
    }

    #[test]
    fn ordering_is_depth_first() {
        let mut paths = [path("B"), path("A B"), path("A/Z"), path("A")];
        paths.sort();
        let sorted: Vec<_> = paths.iter().map(ToString::to_string).collect();
        assert_eq!(sorted, ["A", "A/Z", "A B", "B"]);
    }

    #[test]
    fn key_prefix_cuts_at_levels() {
        let p = path("Aa/Bb/Cc");
        assert_eq!(p.key_prefix(0), "");
        assert_eq!(p.key_prefix(1), path("aa").key());
        assert_eq!(p.key_prefix(2), path("AA/bb").key());
        assert_eq!(p.key_prefix(3), p.key());
        assert_eq!(p.key_prefix(9), p.key());
    }

    #[test]
    fn rebased_replaces_the_leading_part() {
        let p = path("A/B/C");
        assert_eq!(
            p.rebased(&path("A"), Some(&path("X/Y"))),
            Some(path("X/Y/B/C"))
        );
        assert_eq!(p.rebased(&path("A"), None), Some(path("B/C")));
        assert_eq!(path("A").rebased(&path("A"), None), None);
        assert_eq!(
            path("A").rebased(&path("A"), Some(&path("Q"))),
            Some(path("Q"))
        );
    }

    #[test]
    fn ancestors_and_self_goes_outermost_first() {
        let all: Vec<_> = path("A/B/C")
            .ancestors_and_self()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(all, ["A", "A/B", "A/B/C"]);
    }

    #[test]
    fn from_str_works() {
        assert_eq!("A/B".parse::<FolderPath>(), Ok(path("A/B")));
    }
}
