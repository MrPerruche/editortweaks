use super::*;

fn path(s: &str) -> FolderPath {
    FolderPath::new(s).unwrap()
}

/// Folders as text, in the tree's order.
fn folders(tree: &Tree) -> Vec<String> {
    tree.folders().map(ToString::to_string).collect()
}

/// A tree with Planes, Planes/Jets, Planes/Props, Trucks.
fn sample() -> Tree {
    let mut tree = Tree::new();
    tree.create_folder(None, "Planes").unwrap();
    tree.create_folder(Some(&path("Planes")), "Jets").unwrap();
    tree.create_folder(Some(&path("Planes")), "Props").unwrap();
    tree.create_folder(None, "Trucks").unwrap();
    tree
}

// ---- create ----

#[test]
fn create_folders() {
    let tree = sample();
    assert_eq!(
        folders(&tree),
        ["Planes", "Planes/Jets", "Planes/Props", "Trucks"]
    );
    assert!(tree.contains_folder(&path("planes/jets")));
    assert!(!tree.is_empty());
}

#[test]
fn create_returns_the_new_path() {
    let mut tree = Tree::new();
    let top = tree.create_folder(None, "  Cars ").unwrap();
    assert_eq!(top.to_string(), "Cars");
    assert_eq!(
        tree.create_folder(Some(&top), "Fast").unwrap().to_string(),
        "Cars/Fast"
    );
}

#[test]
fn create_refuses_a_name_clash_ignoring_case() {
    let mut tree = sample();
    let error = tree
        .create_folder(Some(&path("Planes")), "jets")
        .unwrap_err();
    assert_eq!(error, FolderError::NameTaken(path("Planes/Jets")));
    // Same name in another folder is fine.
    assert!(tree.create_folder(Some(&path("Trucks")), "Jets").is_ok());
    assert_eq!(
        tree.create_folder(None, "TRUCKS").unwrap_err(),
        FolderError::NameTaken(path("Trucks"))
    );
}

#[test]
fn create_refuses_missing_parent_and_bad_names() {
    let mut tree = sample();
    assert_eq!(
        tree.create_folder(Some(&path("Nope")), "A").unwrap_err(),
        FolderError::NotFound(path("Nope"))
    );
    assert_eq!(
        tree.create_folder(None, "").unwrap_err(),
        FolderError::InvalidPath(PathError::EmptyName)
    );
    assert_eq!(
        tree.create_folder(None, "A/B").unwrap_err(),
        FolderError::InvalidPath(PathError::ForbiddenChar('/'))
    );
}

#[test]
fn create_refuses_too_deep() {
    let mut tree = Tree::new();
    let mut parent = tree.create_folder(None, "a").unwrap();
    for _ in 1..MAX_DEPTH {
        parent = tree.create_folder(Some(&parent), "a").unwrap();
    }
    assert_eq!(
        tree.create_folder(Some(&parent), "a").unwrap_err(),
        FolderError::InvalidPath(PathError::TooDeep)
    );
}

#[test]
fn subfolders_lists_one_level() {
    let mut tree = sample();
    tree.create_folder(Some(&path("Planes/Jets")), "Old")
        .unwrap();
    let top: Vec<_> = tree
        .subfolders(None)
        .iter()
        .map(|f| f.to_string())
        .collect();
    assert_eq!(top, ["Planes", "Trucks"]);
    let planes: Vec<_> = tree
        .subfolders(Some(&path("Planes")))
        .iter()
        .map(|f| f.to_string())
        .collect();
    assert_eq!(planes, ["Planes/Jets", "Planes/Props"]);
    assert!(tree.subfolders(Some(&path("Trucks"))).is_empty());
}

// ---- rename ----

#[test]
fn rename_moves_the_subtree_and_what_is_in_it() {
    let mut tree = sample();
    tree.move_vehicle("1", Some(&path("Planes"))).unwrap();
    tree.move_vehicle("2", Some(&path("Planes/Jets"))).unwrap();
    tree.set_folder_pinned(&path("Planes/Jets"), true).unwrap();
    tree.set_folder_pinned(&path("Planes"), true).unwrap();

    let new_path = tree.rename_folder(&path("Planes"), "Aircraft").unwrap();

    assert_eq!(new_path.to_string(), "Aircraft");
    assert_eq!(
        folders(&tree),
        ["Aircraft", "Aircraft/Jets", "Aircraft/Props", "Trucks"]
    );
    assert_eq!(tree.folder_of("1").unwrap().to_string(), "Aircraft");
    assert_eq!(tree.folder_of("2").unwrap().to_string(), "Aircraft/Jets");
    assert!(tree.is_folder_pinned(&path("Aircraft")));
    assert!(tree.is_folder_pinned(&path("Aircraft/Jets")));
    assert!(!tree.contains_folder(&path("Planes")));
    assert_eq!(tree.pinned_folders().count(), 2);
}

#[test]
fn rename_a_nested_folder() {
    let mut tree = sample();
    tree.move_vehicle("2", Some(&path("Planes/Jets"))).unwrap();
    tree.rename_folder(&path("Planes/Jets"), "Fast").unwrap();
    assert_eq!(tree.folder_of("2").unwrap().to_string(), "Planes/Fast");
}

#[test]
fn rename_can_change_only_the_casing() {
    let mut tree = sample();
    tree.move_vehicle("1", Some(&path("Planes/Jets"))).unwrap();
    let new_path = tree.rename_folder(&path("planes/jets"), "JETS").unwrap();
    assert_eq!(new_path.to_string(), "Planes/JETS");
    assert_eq!(
        folders(&tree),
        ["Planes", "Planes/JETS", "Planes/Props", "Trucks"]
    );
    assert_eq!(tree.folder_of("1").unwrap().to_string(), "Planes/JETS");
}

#[test]
fn rename_to_the_same_name_changes_nothing() {
    let mut tree = sample();
    let before = tree.clone();
    tree.rename_folder(&path("Planes"), "Planes").unwrap();
    assert_eq!(tree, before);
}

#[test]
fn rename_refuses_clashes_missing_and_bad_names() {
    let mut tree = sample();
    let before = tree.clone();
    assert_eq!(
        tree.rename_folder(&path("Planes/Jets"), "props")
            .unwrap_err(),
        FolderError::NameTaken(path("Planes/Props"))
    );
    assert_eq!(
        tree.rename_folder(&path("Planes"), "TRUCKS").unwrap_err(),
        FolderError::NameTaken(path("Trucks"))
    );
    assert_eq!(
        tree.rename_folder(&path("Nope"), "X").unwrap_err(),
        FolderError::NotFound(path("Nope"))
    );
    assert_eq!(
        tree.rename_folder(&path("Planes"), " ").unwrap_err(),
        FolderError::InvalidPath(PathError::EmptyName)
    );
    assert_eq!(tree, before);
}

// ---- move folder ----

#[test]
fn move_folder_into_another() {
    let mut tree = sample();
    tree.move_vehicle("1", Some(&path("Planes/Jets"))).unwrap();
    tree.set_folder_pinned(&path("Planes/Jets"), true).unwrap();

    let new_path = tree
        .move_folder(&path("Planes/Jets"), Some(&path("Trucks")))
        .unwrap();

    assert_eq!(new_path.to_string(), "Trucks/Jets");
    assert_eq!(
        folders(&tree),
        ["Planes", "Planes/Props", "Trucks", "Trucks/Jets"]
    );
    assert_eq!(tree.folder_of("1").unwrap().to_string(), "Trucks/Jets");
    assert!(tree.is_folder_pinned(&path("Trucks/Jets")));
}

#[test]
fn move_folder_to_the_top_level() {
    let mut tree = sample();
    tree.create_folder(Some(&path("Planes/Jets")), "Old")
        .unwrap();
    let new_path = tree.move_folder(&path("Planes/Jets"), None).unwrap();
    assert_eq!(new_path.to_string(), "Jets");
    assert_eq!(
        folders(&tree),
        ["Jets", "Jets/Old", "Planes", "Planes/Props", "Trucks"]
    );
}

#[test]
fn move_folder_where_it_already_is_is_a_no_op() {
    let mut tree = sample();
    let before = tree.clone();
    assert_eq!(
        tree.move_folder(&path("Planes/Jets"), Some(&path("Planes")))
            .unwrap(),
        path("Planes/Jets")
    );
    assert_eq!(
        tree.move_folder(&path("Trucks"), None).unwrap(),
        path("Trucks")
    );
    assert_eq!(tree, before);
}

#[test]
fn move_folder_not_into_itself_or_its_subtree() {
    let mut tree = sample();
    let before = tree.clone();
    assert_eq!(
        tree.move_folder(&path("Planes"), Some(&path("Planes")))
            .unwrap_err(),
        FolderError::IntoOwnSubtree
    );
    assert_eq!(
        tree.move_folder(&path("Planes"), Some(&path("Planes/Jets")))
            .unwrap_err(),
        FolderError::IntoOwnSubtree
    );
    assert_eq!(
        tree.move_folder(&path("planes"), Some(&path("PLANES/jets")))
            .unwrap_err(),
        FolderError::IntoOwnSubtree
    );
    assert_eq!(tree, before);
}

#[test]
fn move_folder_refuses_a_sibling_name_clash() {
    let mut tree = sample();
    tree.create_folder(Some(&path("Trucks")), "jets").unwrap();
    let before = tree.clone();
    assert_eq!(
        tree.move_folder(&path("Planes/Jets"), Some(&path("Trucks")))
            .unwrap_err(),
        FolderError::NameTaken(path("Trucks/jets"))
    );
    assert_eq!(tree, before);
}

#[test]
fn move_folder_refuses_missing_folders() {
    let mut tree = sample();
    assert_eq!(
        tree.move_folder(&path("Nope"), None).unwrap_err(),
        FolderError::NotFound(path("Nope"))
    );
    assert_eq!(
        tree.move_folder(&path("Trucks"), Some(&path("Nope")))
            .unwrap_err(),
        FolderError::NotFound(path("Nope"))
    );
}

#[test]
fn move_folder_refuses_to_get_too_deep() {
    let mut tree = Tree::new();
    let mut deep = tree.create_folder(None, "a").unwrap();
    for _ in 1..MAX_DEPTH {
        deep = tree.create_folder(Some(&deep), "a").unwrap();
    }
    let other = tree.create_folder(None, "b").unwrap();
    tree.create_folder(Some(&other), "c").unwrap();
    // "b" with its child "c" (2 levels) under the 16-deep folder would be 18 deep.
    let before = tree.clone();
    assert_eq!(
        tree.move_folder(&other, Some(&deep)).unwrap_err(),
        FolderError::InvalidPath(PathError::TooDeep)
    );
    assert_eq!(tree, before);
}

// ---- delete ----

#[test]
fn delete_moves_vehicles_and_subfolders_up() {
    let mut tree = sample();
    tree.move_vehicle("1", Some(&path("Planes"))).unwrap();
    tree.move_vehicle("2", Some(&path("Planes/Jets"))).unwrap();
    tree.set_vehicle_pinned("1", true);
    tree.set_folder_pinned(&path("Planes/Props"), true).unwrap();

    tree.delete_folder(&path("Planes")).unwrap();

    assert_eq!(folders(&tree), ["Jets", "Props", "Trucks"]);
    assert_eq!(tree.folder_of("1"), None);
    assert!(tree.is_pinned("1"), "a pin stays");
    assert_eq!(tree.folder_of("2").unwrap().to_string(), "Jets");
    assert!(tree.is_folder_pinned(&path("Props")));
    assert!(!tree.is_folder_pinned(&path("Planes")));
}

#[test]
fn delete_a_nested_folder_moves_things_into_its_parent() {
    let mut tree = sample();
    tree.create_folder(Some(&path("Planes/Jets")), "Old")
        .unwrap();
    tree.move_vehicle("1", Some(&path("Planes/Jets"))).unwrap();
    tree.move_vehicle("2", Some(&path("Planes/Jets/Old")))
        .unwrap();

    tree.delete_folder(&path("Planes/Jets")).unwrap();

    assert_eq!(
        folders(&tree),
        ["Planes", "Planes/Old", "Planes/Props", "Trucks"]
    );
    assert_eq!(tree.folder_of("1").unwrap().to_string(), "Planes");
    assert_eq!(tree.folder_of("2").unwrap().to_string(), "Planes/Old");
}

#[test]
fn delete_merges_same_named_folders() {
    let mut tree = Tree::new();
    tree.create_folder(None, "Cars").unwrap();
    tree.create_folder(None, "Box").unwrap();
    tree.create_folder(Some(&path("Box")), "cars").unwrap();
    tree.create_folder(Some(&path("Box/cars")), "Deep").unwrap();
    tree.move_vehicle("top", Some(&path("Cars"))).unwrap();
    tree.move_vehicle("inner", Some(&path("Box/cars"))).unwrap();
    tree.move_vehicle("deep", Some(&path("Box/cars/Deep")))
        .unwrap();
    tree.set_folder_pinned(&path("Box/cars"), true).unwrap();

    tree.delete_folder(&path("Box")).unwrap();

    // Box/cars merged into Cars (which keeps its own spelling); its subfolder came along.
    assert_eq!(folders(&tree), ["Cars", "Cars/Deep"]);
    assert_eq!(tree.folder_of("top").unwrap().to_string(), "Cars");
    assert_eq!(tree.folder_of("inner").unwrap().to_string(), "Cars");
    assert_eq!(tree.folder_of("deep").unwrap().to_string(), "Cars/Deep");
    assert!(tree.is_folder_pinned(&path("Cars")), "pinned if either was");
}

#[test]
fn delete_a_folder_holding_a_folder_of_its_own_name() {
    let mut tree = Tree::new();
    tree.create_folder(None, "A").unwrap();
    tree.create_folder(Some(&path("A")), "A").unwrap();
    tree.move_vehicle("1", Some(&path("A/A"))).unwrap();
    tree.delete_folder(&path("A")).unwrap();
    assert_eq!(folders(&tree), ["A"]);
    assert_eq!(tree.folder_of("1").unwrap().to_string(), "A");
}

#[test]
fn delete_an_empty_folder_and_a_missing_one() {
    let mut tree = sample();
    tree.delete_folder(&path("Trucks")).unwrap();
    assert_eq!(folders(&tree), ["Planes", "Planes/Jets", "Planes/Props"]);
    assert_eq!(
        tree.delete_folder(&path("Trucks")).unwrap_err(),
        FolderError::NotFound(path("Trucks"))
    );
}

#[test]
fn delete_never_deletes_vehicles_that_matter() {
    let mut tree = sample();
    tree.move_vehicle("1", Some(&path("Trucks"))).unwrap();
    tree.set_vehicle_pinned("2", true);
    tree.move_vehicle("2", Some(&path("Trucks"))).unwrap();
    tree.delete_folder(&path("Trucks")).unwrap();
    // 1 is now unfiled and unpinned: nothing left to remember. 2 keeps its pin.
    assert_eq!(tree.vehicles().count(), 1);
    assert!(tree.is_pinned("2"));
}

// ---- vehicles and pins ----

#[test]
fn move_vehicle_files_and_unfiles() {
    let mut tree = sample();
    assert_eq!(tree.folder_of("491"), None);
    tree.move_vehicle("491", Some(&path("planes/jets")))
        .unwrap();
    assert_eq!(
        tree.folder_of("491").unwrap().to_string(),
        "Planes/Jets",
        "stored with the tree's spelling"
    );
    tree.move_vehicle("491", Some(&path("Trucks"))).unwrap();
    assert_eq!(tree.folder_of("491").unwrap().to_string(), "Trucks");
    tree.move_vehicle("491", None).unwrap();
    assert_eq!(tree.folder_of("491"), None);
    assert_eq!(
        tree.vehicles().count(),
        0,
        "an entry that says nothing is dropped"
    );
}

#[test]
fn move_vehicle_needs_an_existing_folder() {
    let mut tree = sample();
    assert_eq!(
        tree.move_vehicle("1", Some(&path("Nope"))).unwrap_err(),
        FolderError::NotFound(path("Nope"))
    );
    assert_eq!(tree.vehicles().count(), 0);
}

#[test]
fn moving_an_unknown_vehicle_to_the_top_level_does_nothing() {
    let mut tree = sample();
    tree.move_vehicle("1", None).unwrap();
    assert_eq!(tree.vehicles().count(), 0);
}

#[test]
fn pin_and_unpin_vehicles() {
    let mut tree = Tree::new();
    assert!(!tree.is_pinned("1"));
    tree.set_vehicle_pinned("1", true);
    assert!(tree.is_pinned("1"));
    tree.set_vehicle_pinned("1", false);
    assert!(!tree.is_pinned("1"));
    assert!(tree.is_empty());
    // Unpinning an unknown one is fine.
    tree.set_vehicle_pinned("2", false);
    assert!(tree.is_empty());
}

#[test]
fn unpinning_keeps_the_folder() {
    let mut tree = sample();
    tree.move_vehicle("1", Some(&path("Trucks"))).unwrap();
    tree.set_vehicle_pinned("1", true);
    tree.set_vehicle_pinned("1", false);
    assert_eq!(tree.folder_of("1").unwrap().to_string(), "Trucks");
}

#[test]
fn pin_folders() {
    let mut tree = sample();
    tree.set_folder_pinned(&path("trucks"), true).unwrap();
    assert!(tree.is_folder_pinned(&path("Trucks")));
    assert_eq!(
        tree.pinned_folders()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["Trucks"]
    );
    tree.set_folder_pinned(&path("Trucks"), false).unwrap();
    assert!(!tree.is_folder_pinned(&path("Trucks")));
    assert_eq!(
        tree.set_folder_pinned(&path("Nope"), true).unwrap_err(),
        FolderError::NotFound(path("Nope"))
    );
}

#[test]
fn folder_of_a_missing_folder_is_none() {
    let mut tree = sample();
    tree.move_vehicle("1", Some(&path("Trucks"))).unwrap();
    tree.remove_folder_only(&path("Trucks"));
    assert_eq!(tree.folder_of("1"), None);
    assert_eq!(tree.placement("1"), (None, false));
}

// ---- adopt ----

#[test]
fn adopt_into_the_originals_folder() {
    let mut tree = sample();
    tree.move_vehicle("orig", Some(&path("Planes/Jets")))
        .unwrap();
    tree.set_vehicle_pinned("orig", true);
    tree.adopt("copy", Some(77), Origin::SameFolderAs("orig"))
        .unwrap();
    assert_eq!(tree.folder_of("copy").unwrap().to_string(), "Planes/Jets");
    assert!(!tree.is_pinned("copy"), "a pin isn't copied");
    let entry = tree.vehicles().find(|(id, _)| *id == "copy").unwrap().1;
    assert_eq!(entry.created, Some(77));
}

#[test]
fn adopt_into_the_open_folder() {
    let mut tree = sample();
    tree.adopt("new", Some(5), Origin::Folder(Some(&path("trucks"))))
        .unwrap();
    assert_eq!(tree.folder_of("new").unwrap().to_string(), "Trucks");
}

#[test]
fn adopt_with_nowhere_to_go_files_nothing() {
    let mut tree = sample();
    tree.adopt("a", Some(1), Origin::SameFolderAs("unfiled"))
        .unwrap();
    tree.adopt("b", Some(1), Origin::Folder(None)).unwrap();
    assert_eq!(tree.vehicles().count(), 0);
}

#[test]
fn adopt_into_a_missing_folder_is_an_error() {
    let mut tree = sample();
    assert_eq!(
        tree.adopt("a", None, Origin::Folder(Some(&path("Nope"))))
            .unwrap_err(),
        FolderError::NotFound(path("Nope"))
    );
}

#[test]
fn adopt_replaces_what_was_known_under_that_id() {
    let mut tree = sample();
    tree.move_vehicle("7", Some(&path("Trucks"))).unwrap();
    tree.set_vehicle_pinned("7", true);
    tree.adopt("7", Some(9), Origin::Folder(None)).unwrap();
    assert_eq!(tree.folder_of("7"), None);
    assert!(!tree.is_pinned("7"));
}

// ---- prune ----

fn tree_with_entries() -> Tree {
    let mut tree = sample();
    for (id, created) in [("1", 100), ("2", 200), ("3", 300)] {
        tree.adopt(id, Some(created), Origin::Folder(Some(&path("Trucks"))))
            .unwrap();
    }
    tree
}

#[test]
fn prune_removes_deleted_vehicles() {
    let mut tree = tree_with_entries();
    let changed = tree.prune([("1", Some(100)), ("3", Some(300))]);
    assert!(changed);
    assert_eq!(tree.folder_of("2"), None);
    assert!(tree.folder_of("1").is_some() && tree.folder_of("3").is_some());
}

#[test]
fn prune_removes_a_reused_id() {
    let mut tree = tree_with_entries();
    let changed = tree.prune([("1", Some(100)), ("2", Some(999)), ("3", Some(300))]);
    assert!(changed);
    assert_eq!(
        tree.folder_of("2"),
        None,
        "a new vehicle must not inherit the old one's folder"
    );
    assert!(tree.folder_of("1").is_some());
}

#[test]
fn prune_changes_nothing_when_all_is_current() {
    let mut tree = tree_with_entries();
    let before = tree.clone();
    assert!(!tree.prune([
        ("1", Some(100)),
        ("2", Some(200)),
        ("3", Some(300)),
        ("other", None)
    ]));
    assert_eq!(tree, before);
}

#[test]
fn prune_keeps_folders() {
    let mut tree = tree_with_entries();
    tree.prune(std::iter::empty());
    assert_eq!(folders(&tree).len(), 4);
    assert_eq!(tree.vehicles().count(), 0);
}

#[test]
fn prune_learns_a_missing_creation_time() {
    let mut tree = sample();
    tree.move_vehicle("1", Some(&path("Trucks"))).unwrap();
    assert!(tree.prune([("1", Some(500))]));
    assert_eq!(tree.vehicles().next().unwrap().1.created, Some(500));
    // Later the same id with another time is a different vehicle.
    assert!(tree.prune([("1", Some(501))]));
    assert_eq!(tree.folder_of("1"), None);
}

#[test]
fn prune_trusts_an_unknown_time_on_the_vehicles_side() {
    let mut tree = tree_with_entries();
    let before = tree.clone();
    assert!(!tree.prune([("1", None), ("2", None), ("3", None)]));
    assert_eq!(tree, before);
}

#[test]
fn prune_takes_borrowed_strings_from_anywhere() {
    let mut tree = tree_with_entries();
    let ids = [String::from("1"), String::from("2"), String::from("3")];
    assert!(
        !tree.prune(
            ids.iter()
                .zip([100, 200, 300])
                .map(|(id, c)| (id.as_str(), Some(c)))
        )
    );
}

#[test]
fn errors_have_readable_messages() {
    assert_eq!(
        FolderError::NotFound(path("A/B")).to_string(),
        "the folder \"A/B\" doesn't exist"
    );
    assert!(
        FolderError::NameTaken(path("A"))
            .to_string()
            .contains("already")
    );
    assert!(
        FolderError::IntoOwnSubtree
            .to_string()
            .contains("into itself")
    );
    assert!(
        FolderError::InvalidPath(PathError::Empty)
            .to_string()
            .contains("empty")
    );
}

#[test]
fn forget_vehicle_drops_folder_and_pin_only_for_that_id() {
    let mut tree = sample();
    tree.move_vehicle("491", Some(&path("Planes/Jets")))
        .unwrap();
    tree.set_vehicle_pinned("491", true);
    tree.move_vehicle("12", Some(&path("Trucks"))).unwrap();
    assert!(tree.forget_vehicle("491"));
    assert_eq!(tree.folder_of("491"), None);
    assert!(!tree.is_pinned("491"));
    assert_eq!(tree.folder_of("12"), Some(&path("Trucks")));
    // Folders stay, and an unknown id changes nothing.
    assert!(tree.contains_folder(&path("Planes/Jets")));
    assert!(!tree.forget_vehicle("491"));
}

// ---- free names ----

/// The free name `tree` gives in `parent` for "New folder".
fn free(tree: &Tree, parent: Option<&str>) -> Result<String, PathError> {
    tree.free_folder_name(parent.map(path).as_ref(), "New folder")
}

#[test]
fn free_folder_name_counts_up_ignoring_case() {
    let mut tree = sample();
    assert_eq!(free(&tree, None).as_deref(), Ok("New folder"));
    tree.create_folder(None, "NEW FOLDER").unwrap();
    assert_eq!(free(&tree, None).as_deref(), Ok("New folder 2"));
    tree.create_folder(None, "New folder 2").unwrap();
    assert_eq!(free(&tree, None).as_deref(), Ok("New folder 3"));
}

#[test]
fn free_folder_name_is_per_parent() {
    let mut tree = sample();
    tree.create_folder(None, "New folder").unwrap();
    // Taken at the top level, free inside Planes.
    assert_eq!(free(&tree, Some("Planes")).as_deref(), Ok("New folder"));
    assert_eq!(tree.free_folder_name(None, "Jets").as_deref(), Ok("Jets"));
    assert_eq!(
        tree.free_folder_name(Some(&path("Planes")), "Jets")
            .as_deref(),
        Ok("Jets 2")
    );
}

#[test]
fn free_folder_name_fails_where_no_folder_can_go() {
    let mut tree = Tree::new();
    let mut parent: Option<FolderPath> = None;
    for level in 0..MAX_DEPTH {
        parent = Some(
            tree.create_folder(parent.as_ref(), &format!("L{level}"))
                .unwrap(),
        );
    }
    assert_eq!(
        tree.free_folder_name(parent.as_ref(), "New folder"),
        Err(PathError::TooDeep)
    );
    assert!(tree.free_folder_name(None, "").is_err());
}

// ---- folder creation times ----

/// The making times as (folder, time) text pairs, in the tree's order.
fn creation_times(tree: &Tree) -> Vec<(String, i64)> {
    tree.folder_creation_times()
        .map(|(path, time)| (path.to_string(), time))
        .collect()
}

#[test]
fn folder_creation_time_is_set_and_read_ignoring_case() {
    let mut tree = sample();
    assert_eq!(tree.folder_created(&path("Planes")), None);
    tree.set_folder_created(&path("Planes"), 100).unwrap();
    tree.set_folder_created(&path("planes/jets"), 200).unwrap();
    assert_eq!(tree.folder_created(&path("PLANES")), Some(100));
    assert_eq!(tree.folder_created(&path("Planes/Jets")), Some(200));
    assert_eq!(tree.folder_created(&path("Trucks")), None);
    assert_eq!(
        creation_times(&tree),
        [("Planes".to_owned(), 100), ("Planes/Jets".to_owned(), 200)]
    );
    // Setting it again replaces the time, and the key keeps the tree's spelling.
    tree.set_folder_created(&path("planes"), 50).unwrap();
    assert_eq!(creation_times(&tree)[0], ("Planes".to_owned(), 50));
}

#[test]
fn setting_the_creation_time_of_a_missing_folder_fails() {
    let mut tree = sample();
    assert_eq!(
        tree.set_folder_created(&path("Nope"), 5).unwrap_err(),
        FolderError::NotFound(path("Nope"))
    );
    assert_eq!(tree.folder_created(&path("Nope")), None);
    assert_eq!(tree.folder_creation_times().count(), 0);
}

#[test]
fn rename_carries_the_creation_times() {
    let mut tree = sample();
    tree.set_folder_created(&path("Planes"), 100).unwrap();
    tree.set_folder_created(&path("Planes/Jets"), 200).unwrap();
    tree.set_folder_created(&path("Trucks"), 300).unwrap();

    let new_path = tree.rename_folder(&path("Planes"), "Aircraft").unwrap();

    assert_eq!(new_path.to_string(), "Aircraft");
    assert_eq!(
        creation_times(&tree),
        [
            ("Aircraft".to_owned(), 100),
            ("Aircraft/Jets".to_owned(), 200),
            ("Trucks".to_owned(), 300),
        ]
    );
    assert_eq!(tree.folder_created(&path("Planes")), None);
    assert_eq!(tree.folder_created(&path("Planes/Jets")), None);
}

#[test]
fn rename_of_a_subfolder_carries_its_time() {
    let mut tree = sample();
    tree.set_folder_created(&path("Planes"), 100).unwrap();
    tree.set_folder_created(&path("Planes/Jets"), 200).unwrap();

    tree.rename_folder(&path("Planes/Jets"), "Fast").unwrap();

    assert_eq!(
        creation_times(&tree),
        [("Planes".to_owned(), 100), ("Planes/Fast".to_owned(), 200)]
    );
}

#[test]
fn rename_changing_only_the_casing_keeps_the_times_with_the_new_spelling() {
    let mut tree = sample();
    tree.set_folder_created(&path("Planes"), 100).unwrap();
    tree.set_folder_created(&path("Planes/Jets"), 200).unwrap();

    tree.rename_folder(&path("Planes"), "PLANES").unwrap();

    assert_eq!(
        creation_times(&tree),
        [("PLANES".to_owned(), 100), ("PLANES/Jets".to_owned(), 200)]
    );
    assert_eq!(tree.folder_created(&path("planes")), Some(100));
}

#[test]
fn a_refused_rename_keeps_the_times() {
    let mut tree = sample();
    tree.set_folder_created(&path("Planes"), 100).unwrap();
    tree.rename_folder(&path("Planes"), "trucks").unwrap_err();
    assert_eq!(creation_times(&tree), [("Planes".to_owned(), 100)]);
}

#[test]
fn move_carries_the_times_of_the_whole_subtree() {
    let mut tree = sample();
    tree.create_folder(Some(&path("Planes/Jets")), "Old")
        .unwrap();
    tree.set_folder_created(&path("Planes"), 100).unwrap();
    tree.set_folder_created(&path("Planes/Jets"), 200).unwrap();
    tree.set_folder_created(&path("Planes/Jets/Old"), 300)
        .unwrap();
    tree.set_folder_created(&path("Planes/Props"), 400).unwrap();
    tree.set_folder_created(&path("Trucks"), 500).unwrap();

    let new_path = tree
        .move_folder(&path("Planes/Jets"), Some(&path("Trucks")))
        .unwrap();

    assert_eq!(new_path.to_string(), "Trucks/Jets");
    assert_eq!(
        creation_times(&tree),
        [
            ("Planes".to_owned(), 100),
            ("Planes/Props".to_owned(), 400),
            ("Trucks".to_owned(), 500),
            ("Trucks/Jets".to_owned(), 200),
            ("Trucks/Jets/Old".to_owned(), 300),
        ]
    );
}

#[test]
fn move_to_the_top_level_carries_the_times() {
    let mut tree = sample();
    tree.set_folder_created(&path("Planes/Jets"), 200).unwrap();

    tree.move_folder(&path("Planes/Jets"), None).unwrap();

    assert_eq!(creation_times(&tree), [("Jets".to_owned(), 200)]);
}

#[test]
fn move_where_it_already_is_keeps_the_times() {
    let mut tree = sample();
    tree.set_folder_created(&path("Planes/Jets"), 200).unwrap();
    tree.move_folder(&path("Planes/Jets"), Some(&path("Planes")))
        .unwrap();
    assert_eq!(creation_times(&tree), [("Planes/Jets".to_owned(), 200)]);
}

#[test]
fn delete_drops_the_folders_time_and_keeps_its_subfolders() {
    let mut tree = sample();
    tree.create_folder(Some(&path("Planes/Jets")), "Old")
        .unwrap();
    tree.set_folder_created(&path("Planes"), 100).unwrap();
    tree.set_folder_created(&path("Planes/Jets"), 200).unwrap();
    tree.set_folder_created(&path("Planes/Jets/Old"), 300)
        .unwrap();
    tree.set_folder_created(&path("Planes/Props"), 400).unwrap();

    tree.delete_folder(&path("Planes")).unwrap();

    // Planes' own time is gone; Jets, Old and Props moved up one level keeping theirs.
    assert_eq!(
        creation_times(&tree),
        [
            ("Jets".to_owned(), 200),
            ("Jets/Old".to_owned(), 300),
            ("Props".to_owned(), 400),
        ]
    );
    assert_eq!(tree.folder_created(&path("Planes")), None);
}

#[test]
fn delete_of_a_nested_folder_moves_its_subfolders_times_into_the_parent() {
    let mut tree = sample();
    tree.create_folder(Some(&path("Planes/Jets")), "Old")
        .unwrap();
    tree.set_folder_created(&path("Planes/Jets"), 200).unwrap();
    tree.set_folder_created(&path("Planes/Jets/Old"), 300)
        .unwrap();

    tree.delete_folder(&path("Planes/Jets")).unwrap();

    assert_eq!(creation_times(&tree), [("Planes/Old".to_owned(), 300)]);
}

/// Cars (with `cars_time`) and Box/cars (with `inner_time`), then Box is deleted so Box/cars
/// merges into Cars. Returns the tree.
fn merged_cars(cars_time: Option<i64>, inner_time: Option<i64>) -> Tree {
    let mut tree = Tree::new();
    tree.create_folder(None, "Cars").unwrap();
    tree.create_folder(None, "Box").unwrap();
    tree.create_folder(Some(&path("Box")), "cars").unwrap();
    if let Some(time) = cars_time {
        tree.set_folder_created(&path("Cars"), time).unwrap();
    }
    if let Some(time) = inner_time {
        tree.set_folder_created(&path("Box/cars"), time).unwrap();
    }
    tree.delete_folder(&path("Box")).unwrap();
    tree
}

#[test]
fn merge_keeps_the_earlier_time_when_the_older_folder_moves_in() {
    let tree = merged_cars(Some(200), Some(100));
    assert_eq!(folders(&tree), ["Cars"]);
    assert_eq!(creation_times(&tree), [("Cars".to_owned(), 100)]);
}

#[test]
fn merge_keeps_the_earlier_time_when_the_younger_folder_moves_in() {
    let tree = merged_cars(Some(100), Some(200));
    assert_eq!(folders(&tree), ["Cars"]);
    assert_eq!(creation_times(&tree), [("Cars".to_owned(), 100)]);
}

#[test]
fn merge_with_equal_times_keeps_that_time() {
    let tree = merged_cars(Some(100), Some(100));
    assert_eq!(creation_times(&tree), [("Cars".to_owned(), 100)]);
}

#[test]
fn merge_with_only_one_known_time_keeps_it() {
    // The folder that was there had no time: the moved-in one's time is kept.
    let tree = merged_cars(None, Some(200));
    assert_eq!(creation_times(&tree), [("Cars".to_owned(), 200)]);
    // The moved-in folder had none: the existing one's stays.
    let tree = merged_cars(Some(100), None);
    assert_eq!(creation_times(&tree), [("Cars".to_owned(), 100)]);
    // Neither had one.
    let tree = merged_cars(None, None);
    assert_eq!(tree.folder_creation_times().count(), 0);
}

#[test]
fn merge_keeps_the_existing_folders_spelling_of_the_time_key() {
    // Box/cars is older but lands in Cars, whose spelling stays.
    let tree = merged_cars(Some(200), Some(100));
    let (folder, time) = tree.folder_creation_times().next().unwrap();
    assert_eq!(folder.to_string(), "Cars");
    assert_eq!(time, 100);
}

#[test]
fn merge_keeps_the_earlier_times_of_subfolders_too() {
    let mut tree = Tree::new();
    tree.create_folder(None, "Cars").unwrap();
    tree.create_folder(Some(&path("Cars")), "Deep").unwrap();
    tree.create_folder(None, "Box").unwrap();
    tree.create_folder(Some(&path("Box")), "cars").unwrap();
    tree.create_folder(Some(&path("Box/cars")), "deep").unwrap();
    tree.create_folder(Some(&path("Box/cars")), "Extra")
        .unwrap();
    tree.set_folder_created(&path("Cars"), 500).unwrap();
    tree.set_folder_created(&path("Cars/Deep"), 100).unwrap();
    tree.set_folder_created(&path("Box/cars"), 400).unwrap();
    tree.set_folder_created(&path("Box/cars/deep"), 300)
        .unwrap();
    tree.set_folder_created(&path("Box/cars/Extra"), 600)
        .unwrap();

    tree.delete_folder(&path("Box")).unwrap();

    assert_eq!(
        creation_times(&tree),
        [
            ("Cars".to_owned(), 400),
            ("Cars/Deep".to_owned(), 100),
            ("Cars/Extra".to_owned(), 600),
        ]
    );
}

#[test]
fn a_folder_without_a_time_stays_without() {
    let mut tree = sample();
    tree.set_folder_created(&path("Trucks"), 300).unwrap();

    tree.rename_folder(&path("Planes"), "Aircraft").unwrap();
    assert_eq!(tree.folder_created(&path("Aircraft")), None);
    assert_eq!(tree.folder_created(&path("Aircraft/Jets")), None);

    tree.move_folder(&path("Aircraft/Jets"), Some(&path("Trucks")))
        .unwrap();
    assert_eq!(tree.folder_created(&path("Trucks/Jets")), None);

    tree.delete_folder(&path("Aircraft")).unwrap();
    assert_eq!(tree.folder_created(&path("Props")), None);
    assert_eq!(creation_times(&tree), [("Trucks".to_owned(), 300)]);
}
