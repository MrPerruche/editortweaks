use super::*;

fn path(s: &str) -> FolderPath {
    FolderPath::new(s).unwrap()
}

fn parse(text: &str) -> Parsed {
    Store::parse(text).unwrap()
}

/// The example file from the plan.
const EXAMPLE: &str = r#"
version = 1

[local]
folders = ["Planes", "Planes/Jets", "Trucks"]
pinned_folders = ["Trucks"]

[local.vehicles."491"]
folder = "Planes/Jets"
pinned = true
created = 1731100000

[subscribed]
folders = ["Tanks"]

[subscribed.vehicles."3123456789"]
folder = "Tanks"
"#;

fn sample_store() -> Store {
    let mut store = Store::default();
    let local = store.tree_mut(Category::Local);
    local.create_folder(None, "Planes").unwrap();
    local.create_folder(Some(&path("Planes")), "Jets").unwrap();
    local.create_folder(None, "Trucks").unwrap();
    local.create_folder(None, "Empty").unwrap();
    local.set_folder_pinned(&path("Trucks"), true).unwrap();
    local
        .adopt(
            "491",
            Some(1731100000),
            crate::Origin::Folder(Some(&path("Planes/Jets"))),
        )
        .unwrap();
    local.set_vehicle_pinned("491", true);
    local.set_vehicle_pinned("12", true);
    local.move_vehicle("7", Some(&path("Trucks"))).unwrap();
    let subscribed = store.tree_mut(Category::Subscribed);
    subscribed.create_folder(None, "Tanks").unwrap();
    subscribed
        .move_vehicle("3123456789", Some(&path("Tanks")))
        .unwrap();
    store
}

#[test]
fn reads_the_example_file() {
    let Parsed { store, warnings } = parse(EXAMPLE);
    assert!(warnings.is_empty(), "{warnings:?}");
    let local = &store.local;
    assert_eq!(
        local.folders().map(ToString::to_string).collect::<Vec<_>>(),
        ["Planes", "Planes/Jets", "Trucks"]
    );
    assert!(local.is_folder_pinned(&path("Trucks")));
    assert_eq!(local.folder_of("491"), Some(&path("Planes/Jets")));
    assert!(local.is_pinned("491"));
    let entry = local.vehicles().next().unwrap().1;
    assert_eq!(entry.created, Some(1731100000));
    assert_eq!(
        store.subscribed.folder_of("3123456789"),
        Some(&path("Tanks"))
    );
    assert_eq!(store.tree(Category::Subscribed), &store.subscribed);
    assert_eq!(store.tree(Category::Local), &store.local);
}

#[test]
fn round_trips() {
    let store = sample_store();
    let text = store.to_toml();
    let parsed = parse(&text);
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    assert_eq!(parsed.store, store);
}

#[test]
fn round_trip_of_the_example_is_stable() {
    let first = parse(EXAMPLE).store.to_toml();
    let second = parse(&first).store.to_toml();
    assert_eq!(first, second);
}

#[test]
fn writing_is_deterministic_and_sorted() {
    let a = sample_store();
    // The same content built in another order.
    let mut b = Store::default();
    b.subscribed.create_folder(None, "Tanks").unwrap();
    b.subscribed
        .move_vehicle("3123456789", Some(&path("Tanks")))
        .unwrap();
    b.local.move_vehicle("7", Some(&path("Trucks"))).ok();
    b.local.create_folder(None, "Empty").unwrap();
    b.local.create_folder(None, "Trucks").unwrap();
    b.local.create_folder(None, "Planes").unwrap();
    b.local
        .create_folder(Some(&path("Planes")), "Jets")
        .unwrap();
    b.local.set_vehicle_pinned("12", true);
    b.local.set_vehicle_pinned("491", true);
    b.local
        .adopt(
            "491",
            Some(1731100000),
            crate::Origin::Folder(Some(&path("Planes/Jets"))),
        )
        .unwrap();
    b.local.set_vehicle_pinned("491", true);
    b.local.move_vehicle("7", Some(&path("Trucks"))).unwrap();
    b.local.set_folder_pinned(&path("Trucks"), true).unwrap();
    assert_eq!(a.to_toml(), b.to_toml());
}

#[test]
fn the_written_file_looks_like_the_plan() {
    let text = sample_store().to_toml();
    assert!(text.starts_with("version = 1\n"), "{text}");
    assert!(
        text.contains("folders = [\"Empty\", \"Planes\", \"Planes/Jets\", \"Trucks\"]"),
        "{text}"
    );
    assert!(text.contains("pinned_folders = [\"Trucks\"]"), "{text}");
    assert!(text.contains("folder = \"Planes/Jets\""), "{text}");
    assert!(text.contains("created = 1731100000"), "{text}");
    // Vehicles come in id order.
    let positions: Vec<usize> = ["12", "491", "7"]
        .iter()
        .map(|id| text.find(&format!("vehicles.{id}]")).unwrap())
        .collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{text}");
}

#[test]
fn an_empty_store_writes_just_the_version() {
    assert_eq!(Store::default().to_toml(), "version = 1\n");
    assert_eq!(parse("").store, Store::default());
    assert_eq!(parse("version = 1\n").store, Store::default());
}

#[test]
fn entries_with_neither_folder_nor_pin_are_dropped() {
    let parsed = parse(
        "[local.vehicles.\"1\"]\ncreated = 5\n[local.vehicles.\"2\"]\npinned = false\n[local.vehicles.\"3\"]\n",
    );
    assert_eq!(parsed.store.local.vehicles().count(), 0);
    assert!(parsed.warnings.is_empty());
}

#[test]
fn missing_parents_are_implied() {
    let parsed = parse("[local]\nfolders = [\"A/B/C\"]\n");
    let folders: Vec<_> = parsed
        .store
        .local
        .folders()
        .map(ToString::to_string)
        .collect();
    assert_eq!(folders, ["A", "A/B", "A/B/C"]);
    assert!(parsed.store.to_toml().contains("\"A/B/C\""));
}

#[test]
fn a_vehicles_unlisted_folder_is_created() {
    let parsed = parse("[local.vehicles.\"1\"]\nfolder = \"X/Y\"\n");
    assert_eq!(parsed.store.local.folder_of("1"), Some(&path("X/Y")));
    assert!(parsed.store.local.contains_folder(&path("X")));
}

#[test]
fn folders_differing_in_case_are_one_folder() {
    let parsed = parse(
        "[local]\nfolders = [\"Jets\", \"jets\"]\n[local.vehicles.\"1\"]\nfolder = \"JETS\"\n",
    );
    let local = &parsed.store.local;
    assert_eq!(
        local.folders().map(ToString::to_string).collect::<Vec<_>>(),
        ["Jets"]
    );
    assert_eq!(local.folder_of("1").unwrap().to_string(), "Jets");
}

// ---- forward compatibility ----

#[test]
fn unknown_fields_are_ignored() {
    let text = r#"
version = 1
future_top_level = "x"
[local]
folders = ["A"]
colors = { A = "red" }
[local.vehicles."1"]
folder = "A"
note = "hello"
tags = ["a", "b"]
[local.extra_section]
whatever = 1
[somewhere_new]
a = 1
"#;
    let parsed = parse(text);
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    assert_eq!(parsed.store.local.folder_of("1"), Some(&path("A")));
}

#[test]
fn a_newer_version_still_loads_what_it_knows() {
    let text = r#"
version = 2
[local]
folders = ["A", "A/B"]
pinned_folders = ["A"]
new_thing = [1, 2, 3]
[local.vehicles."5"]
folder = "A/B"
pinned = true
created = 9
shiny = true
[subscribed.vehicles."77"]
pinned = true
[sync]
enabled = true
"#;
    let parsed = parse(text);
    assert_eq!(parsed.warnings.len(), 1, "{:?}", parsed.warnings);
    assert!(parsed.warnings[0].contains("version 2"));
    let local = &parsed.store.local;
    assert_eq!(local.folder_of("5"), Some(&path("A/B")));
    assert!(local.is_pinned("5"));
    assert!(local.is_folder_pinned(&path("A")));
    assert!(parsed.store.subscribed.is_pinned("77"));
    // It is written as the version this build knows.
    assert!(parsed.store.to_toml().starts_with("version = 1"));
}

#[test]
fn a_missing_version_is_fine() {
    let parsed = parse("[local]\nfolders = [\"A\"]\n");
    assert!(parsed.warnings.is_empty());
    assert_eq!(parsed.store.local.folders().count(), 1);
}

#[test]
fn only_one_category_present() {
    let parsed = parse("[subscribed]\nfolders = [\"T\"]\n");
    assert!(parsed.store.local.is_empty());
    assert_eq!(parsed.store.subscribed.folders().count(), 1);
}

// ---- invalid input ----

#[test]
fn invalid_folders_are_skipped_with_warnings() {
    let text = r#"
[local]
folders = ["Good", "", "A//B", "Back\\slash", 5, "Fine/Sub"]
"#;
    let parsed = parse(text);
    let folders: Vec<_> = parsed
        .store
        .local
        .folders()
        .map(ToString::to_string)
        .collect();
    assert_eq!(folders, ["Fine", "Fine/Sub", "Good"]);
    assert_eq!(parsed.warnings.len(), 4, "{:?}", parsed.warnings);
    assert!(
        parsed
            .warnings
            .iter()
            .all(|w| w.starts_with("[local] folders"))
    );
}

#[test]
fn too_deep_and_too_long_folders_are_skipped() {
    let deep = vec!["a"; crate::MAX_DEPTH + 1].join("/");
    let long = "x".repeat(crate::MAX_NAME_CHARS + 1);
    let parsed = parse(&format!(
        "[local]\nfolders = [\"{deep}\", \"{long}\", \"ok\"]\n"
    ));
    assert_eq!(parsed.store.local.folders().count(), 1);
    assert_eq!(parsed.warnings.len(), 2);
}

#[test]
fn a_vehicle_with_a_bad_folder_keeps_its_pin() {
    let text = "[local.vehicles.\"1\"]\nfolder = \"a//b\"\npinned = true\n";
    let parsed = parse(text);
    assert!(parsed.store.local.is_pinned("1"));
    assert_eq!(parsed.store.local.folder_of("1"), None);
    assert_eq!(parsed.warnings.len(), 1);
}

#[test]
fn a_vehicle_with_a_bad_folder_and_nothing_else_is_dropped() {
    let parsed = parse("[local.vehicles.\"1\"]\nfolder = \"a//b\"\n");
    assert_eq!(parsed.store.local.vehicles().count(), 0);
    assert_eq!(parsed.warnings.len(), 1);
}

#[test]
fn wrong_types_skip_only_that_field() {
    let text = r#"
[local.vehicles."1"]
folder = 5
pinned = "yes"
created = "yesterday"
[local.vehicles."2"]
folder = "Ok"
pinned = true
created = 3
"#;
    let parsed = parse(text);
    assert_eq!(parsed.warnings.len(), 3, "{:?}", parsed.warnings);
    assert_eq!(parsed.store.local.vehicles().count(), 1);
    assert_eq!(parsed.store.local.folder_of("2"), Some(&path("Ok")));
}

#[test]
fn wrong_shaped_sections_are_skipped() {
    let text = r#"
local = 5
[subscribed]
folders = "not a list"
vehicles = 3
"#;
    let parsed = parse(text);
    assert_eq!(parsed.store, Store::default());
    assert_eq!(parsed.warnings.len(), 3, "{:?}", parsed.warnings);
}

#[test]
fn a_vehicle_that_is_not_a_table_is_skipped() {
    let parsed = parse("[local.vehicles]\n\"1\" = 5\n");
    assert_eq!(parsed.store.local.vehicles().count(), 0);
    assert_eq!(parsed.warnings.len(), 1);
}

#[test]
fn an_empty_vehicle_id_is_skipped() {
    let parsed = parse("[local.vehicles.\"\"]\npinned = true\n");
    assert_eq!(parsed.store.local.vehicles().count(), 0);
    assert_eq!(parsed.warnings.len(), 1);
}

#[test]
fn pinning_a_missing_folder_is_skipped() {
    let parsed = parse("[local]\nfolders = [\"A\"]\npinned_folders = [\"A\", \"B\"]\n");
    assert!(parsed.store.local.is_folder_pinned(&path("A")));
    assert_eq!(parsed.store.local.pinned_folders().count(), 1);
    assert_eq!(parsed.warnings.len(), 1);
}

#[test]
fn a_pin_on_a_folder_a_vehicle_implied_works() {
    let parsed =
        parse("[local]\npinned_folders = [\"X\"]\n[local.vehicles.\"1\"]\nfolder = \"X\"\n");
    assert!(parsed.store.local.is_folder_pinned(&path("X")));
    assert!(parsed.warnings.is_empty());
}

#[test]
fn text_that_is_not_toml_is_an_error() {
    let error = Store::parse("this is = = not toml [").unwrap_err();
    assert!(error.to_string().starts_with("not a valid folders file"));
    assert!(Store::parse("{ \"json\": true }").is_err());
}

// ---- categories and pruning ----

#[test]
fn categories_are_separate() {
    let mut store = Store::default();
    store
        .tree_mut(Category::Local)
        .create_folder(None, "A")
        .unwrap();
    assert!(store.tree(Category::Subscribed).is_empty());
    store
        .tree_mut(Category::Subscribed)
        .create_folder(None, "A")
        .unwrap();
    assert_eq!(store.local.folders().count(), 1);
    assert_eq!(store.subscribed.folders().count(), 1);
}

#[test]
fn prune_local_leaves_subscribed_alone() {
    let mut store = sample_store();
    let changed = store.prune_local([("491", Some(1731100000))]);
    assert!(changed);
    // "12" and "7" are gone from local; "491" stays.
    assert_eq!(store.local.vehicles().count(), 1);
    assert!(store.local.is_pinned("491"));
    // The subscribed vehicle isn't in the list, but is never pruned.
    assert_eq!(
        store.subscribed.folder_of("3123456789"),
        Some(&path("Tanks"))
    );
}

#[test]
fn prune_local_with_a_reused_id() {
    let mut store = sample_store();
    assert!(store.prune_local([("491", Some(42)), ("12", None), ("7", None)]));
    assert_eq!(store.local.folder_of("491"), None);
    assert!(!store.local.is_pinned("491"));
}

#[test]
fn error_messages_are_readable() {
    let error = Store::parse("= nope").unwrap_err();
    assert!(!error.to_string().is_empty());
}

// ---- folder creation times ----

/// The local tree's making times as (folder, time) text pairs.
fn local_times(store: &Store) -> Vec<(String, i64)> {
    store
        .local
        .folder_creation_times()
        .map(|(path, time)| (path.to_string(), time))
        .collect()
}

#[test]
fn folder_creation_times_round_trip() {
    let mut store = sample_store();
    store
        .local
        .set_folder_created(&path("Planes"), 1731000000)
        .unwrap();
    store
        .local
        .set_folder_created(&path("Planes/Jets"), 1731000500)
        .unwrap();
    store
        .subscribed
        .set_folder_created(&path("Tanks"), 1731001000)
        .unwrap();

    let text = store.to_toml();
    let parsed = parse(&text);

    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    assert_eq!(parsed.store, store);
    assert_eq!(
        local_times(&parsed.store),
        [
            ("Planes".to_owned(), 1731000000),
            ("Planes/Jets".to_owned(), 1731000500),
        ]
    );
    assert_eq!(
        parsed.store.subscribed.folder_created(&path("Tanks")),
        Some(1731001000)
    );
    // Writing it again gives the same text.
    assert_eq!(parsed.store.to_toml(), text);
}

#[test]
fn the_folder_created_table_is_written_only_with_times() {
    let mut store = sample_store();
    let text = store.to_toml();
    assert!(!text.contains("folder_created"), "{text}");

    store
        .local
        .set_folder_created(&path("Planes"), 1731000000)
        .unwrap();
    let text = store.to_toml();
    assert!(text.contains("[local.folder_created]"), "{text}");
    assert!(text.contains("Planes = 1731000000"), "{text}");
    // Only the category that has times gets the table.
    assert!(!text.contains("[subscribed.folder_created]"), "{text}");
}

#[test]
fn an_older_file_without_the_table_reads_without_times() {
    let parsed = parse(EXAMPLE);
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    assert_eq!(parsed.store.local.folder_creation_times().count(), 0);
    assert_eq!(parsed.store.subscribed.folder_creation_times().count(), 0);
    assert_eq!(parsed.store.local.folder_created(&path("Planes")), None);
    // And it is still written without one.
    assert!(!parsed.store.to_toml().contains("folder_created"));
}

#[test]
fn reads_folder_creation_times() {
    let parsed = parse(
        "[local]\nfolders = [\"Planes\", \"Planes/Jets\"]\n[local.folder_created]\nPlanes = 100\n\"planes/jets\" = 200\n",
    );
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    // Keys are matched ignoring case, and the folders' own spelling is kept.
    assert_eq!(
        local_times(&parsed.store),
        [("Planes".to_owned(), 100), ("Planes/Jets".to_owned(), 200)]
    );
}

#[test]
fn bad_folder_creation_times_are_skipped_with_warnings_and_good_ones_kept() {
    let text = r#"
[local]
folders = ["Good", "Good/Sub", "Text", "Float"]

[local.folder_created]
Good = 100
"Good/Sub" = 200
"a//b" = 300
"" = 350
Text = "yesterday"
Float = 1.5
Nope = 400
"Good/Missing" = 500
"#;
    let parsed = parse(text);

    assert_eq!(
        local_times(&parsed.store),
        [("Good".to_owned(), 100), ("Good/Sub".to_owned(), 200)]
    );
    // Two bad paths, two non-integers, two folders that don't exist.
    assert_eq!(parsed.warnings.len(), 6, "{:?}", parsed.warnings);
    assert!(
        parsed
            .warnings
            .iter()
            .all(|w| w.starts_with("[local] folder_created")),
        "{:?}",
        parsed.warnings
    );
    assert_eq!(
        parsed
            .warnings
            .iter()
            .filter(|w| w.contains("is not a number"))
            .count(),
        2
    );
    assert_eq!(
        parsed
            .warnings
            .iter()
            .filter(|w| w.contains("doesn't exist"))
            .count(),
        2
    );
    // Skipped entries made no folders.
    assert_eq!(parsed.store.local.folders().count(), 4);
}

#[test]
fn a_creation_time_does_not_create_its_folder() {
    let parsed = parse("[local.folder_created]\nGhost = 5\n");
    assert_eq!(parsed.store, Store::default());
    assert_eq!(parsed.warnings.len(), 1, "{:?}", parsed.warnings);
}

#[test]
fn a_folder_created_that_is_not_a_table_is_skipped() {
    let parsed = parse(
        "[local]\nfolders = [\"A\"]\nfolder_created = 5\n[subscribed]\nfolders = [\"B\"]\nfolder_created = [\"B\"]\n",
    );
    assert_eq!(parsed.store.local.folders().count(), 1);
    assert_eq!(parsed.store.local.folder_creation_times().count(), 0);
    assert_eq!(parsed.store.subscribed.folder_creation_times().count(), 0);
    assert_eq!(parsed.warnings.len(), 2, "{:?}", parsed.warnings);
    assert!(parsed.warnings[0].contains("[local] folder_created is not a table"));
    assert!(parsed.warnings[1].contains("[subscribed] folder_created is not a table"));
}

#[test]
fn creation_times_of_a_vehicles_implied_folder_are_kept() {
    // Folders a vehicle implies exist by the time the times are read.
    let parsed = parse("[local.folder_created]\nX = 7\n[local.vehicles.\"1\"]\nfolder = \"X\"\n");
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    assert_eq!(parsed.store.local.folder_created(&path("X")), Some(7));
}
