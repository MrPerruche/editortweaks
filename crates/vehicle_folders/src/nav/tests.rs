use super::*;

fn path(s: &str) -> FolderPath {
    FolderPath::new(s).unwrap()
}

fn folder(s: &str) -> Location {
    Location::Folder(path(s))
}

fn tree_with(folders: &[&str]) -> Tree {
    let mut tree = Tree::new();
    for f in folders {
        let p = path(f);
        match p.parent() {
            Some(parent) => tree.create_folder(Some(&parent), p.name()).unwrap(),
            None => tree.create_folder(None, p.name()).unwrap(),
        };
    }
    tree
}

// ---- breadcrumbs ----

#[test]
fn breadcrumbs_of_the_root() {
    assert_eq!(
        breadcrumbs(&Location::Root),
        [(ROOT_LABEL.to_owned(), Location::Root)]
    );
}

#[test]
fn breadcrumbs_of_a_nested_folder() {
    let crumbs = breadcrumbs(&folder("Planes/Jets/Old"));
    let labels: Vec<_> = crumbs.iter().map(|(label, _)| label.as_str()).collect();
    assert_eq!(labels, [ROOT_LABEL, "Planes", "Jets", "Old"]);
    assert_eq!(crumbs[0].1, Location::Root);
    assert_eq!(crumbs[1].1, folder("Planes"));
    assert_eq!(crumbs[2].1, folder("Planes/Jets"));
    assert_eq!(crumbs[3].1, folder("Planes/Jets/Old"));
}

#[test]
fn breadcrumbs_of_uncategorized() {
    let crumbs = breadcrumbs(&Location::Uncategorized);
    assert_eq!(crumbs.len(), 2);
    assert_eq!(crumbs[0].1, Location::Root);
    assert_eq!(
        crumbs[1],
        (UNCATEGORIZED_LABEL.to_owned(), Location::Uncategorized)
    );
}

#[test]
fn breadcrumbs_keep_the_users_casing() {
    let crumbs = breadcrumbs(&folder("pLANes"));
    assert_eq!(crumbs[1].0, "pLANes");
}

// ---- parent ----

#[test]
fn location_parents() {
    assert_eq!(Location::Root.parent(), None);
    assert_eq!(Location::Uncategorized.parent(), Some(Location::Root));
    assert_eq!(folder("A").parent(), Some(Location::Root));
    assert_eq!(folder("A/B").parent(), Some(folder("A")));
}

// ---- history ----

#[test]
fn starts_at_the_root_with_nowhere_to_go() {
    let history = History::new();
    assert_eq!(history.current(), &Location::Root);
    assert!(!history.can_go_back() && !history.can_go_forward() && !history.can_go_up());
}

#[test]
fn go_back_and_forward() {
    let mut h = History::new();
    h.go(folder("A"));
    h.go(folder("A/B"));
    assert_eq!(h.current(), &folder("A/B"));
    assert!(h.can_go_back() && !h.can_go_forward());

    assert!(h.back());
    assert_eq!(h.current(), &folder("A"));
    assert!(h.back());
    assert_eq!(h.current(), &Location::Root);
    assert!(!h.back(), "nothing before the root");
    assert_eq!(h.current(), &Location::Root);

    assert!(h.forward());
    assert_eq!(h.current(), &folder("A"));
    assert!(h.forward());
    assert_eq!(h.current(), &folder("A/B"));
    assert!(!h.forward());
}

#[test]
fn going_somewhere_forgets_the_forward_list() {
    let mut h = History::new();
    h.go(folder("A"));
    h.go(folder("B"));
    h.back();
    assert!(h.can_go_forward());
    h.go(folder("C"));
    assert!(!h.can_go_forward());
    h.back();
    assert_eq!(h.current(), &folder("A"));
}

#[test]
fn going_where_we_are_does_nothing() {
    let mut h = History::new();
    h.go(Location::Root);
    assert!(!h.can_go_back());
    h.go(folder("A"));
    h.go(folder("a"));
    assert!(h.back());
    assert_eq!(h.current(), &Location::Root);
}

#[test]
fn up_goes_to_the_parent_as_a_step() {
    let mut h = History::new();
    h.go(folder("A/B"));
    assert!(h.can_go_up());
    assert!(h.up());
    assert_eq!(h.current(), &folder("A"));
    assert!(h.up());
    assert_eq!(h.current(), &Location::Root);
    assert!(!h.up());
    assert!(!h.can_go_up());
    // Back returns to where up came from.
    assert!(h.back());
    assert_eq!(h.current(), &folder("A"));
}

#[test]
fn up_from_uncategorized_goes_to_the_root() {
    let mut h = History::new();
    h.go(Location::Uncategorized);
    assert!(h.up());
    assert_eq!(h.current(), &Location::Root);
}

// ---- fix_up ----

#[test]
fn fix_up_changes_nothing_when_all_exists() {
    let tree = tree_with(&["A", "A/B"]);
    let mut h = History::new();
    h.go(folder("A"));
    h.go(folder("A/B"));
    h.go(Location::Uncategorized);
    h.back();
    let before = h.clone();
    h.fix_up(&tree);
    assert_eq!(h, before);
}

#[test]
fn fix_up_moves_a_deleted_current_folder_to_its_nearest_parent() {
    let tree = tree_with(&["A"]);
    let mut h = History::new();
    h.go(folder("A"));
    h.go(folder("A/B/C"));
    h.fix_up(&tree);
    assert_eq!(h.current(), &folder("A"));
    // The step from A to the deleted folder led where we are: dropped.
    assert!(h.back());
    assert_eq!(h.current(), &Location::Root);
    assert!(!h.back());
}

#[test]
fn fix_up_falls_back_to_the_root() {
    let tree = Tree::new();
    let mut h = History::new();
    h.go(folder("Gone"));
    h.fix_up(&tree);
    assert_eq!(h.current(), &Location::Root);
    assert!(!h.can_go_back());
}

#[test]
fn fix_up_drops_missing_folders_from_both_lists() {
    let tree = tree_with(&["Keep"]);
    let mut h = History::new();
    h.go(folder("Gone1"));
    h.go(folder("Keep"));
    h.go(folder("Gone2"));
    h.go(Location::Uncategorized);
    h.back();
    h.back();
    h.back();
    // back list: [Root]; current Gone1; forward: Uncategorized, Gone2, Keep (next last).
    h.fix_up(&tree);
    assert_eq!(h.current(), &Location::Root);
    assert!(!h.can_go_back());
    assert!(h.forward());
    assert_eq!(h.current(), &folder("Keep"));
    assert!(h.forward());
    assert_eq!(h.current(), &Location::Uncategorized);
    assert!(!h.forward());
}

#[test]
fn fix_up_collapses_repeats() {
    let tree = tree_with(&["A"]);
    let mut h = History::new();
    h.go(folder("A"));
    h.go(folder("X"));
    h.go(folder("A"));
    h.go(folder("Y"));
    h.go(folder("A"));
    h.fix_up(&tree);
    // Root, A, [X], A, [Y], A -> the duplicates of the current location at the end go.
    assert_eq!(h.current(), &folder("A"));
    let mut seen = vec![h.current().clone()];
    while h.back() {
        seen.push(h.current().clone());
    }
    assert_eq!(seen, [folder("A"), Location::Root]);
}

#[test]
fn fix_up_adopts_the_trees_spelling() {
    let tree = tree_with(&["Planes"]);
    let mut h = History::new();
    h.go(folder("planes"));
    h.fix_up(&tree);
    assert_eq!(breadcrumbs(h.current())[1].0, "Planes");
}
