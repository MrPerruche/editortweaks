use std::time::{Duration, Instant};

use super::*;

fn path(s: &str) -> FolderPath {
    FolderPath::new(s).unwrap()
}

fn at(s: &str) -> Location {
    Location::Folder(path(s))
}

fn opts(mode: ViewMode, location: Location) -> ListOptions {
    ListOptions {
        location,
        mode,
        ..ListOptions::default()
    }
}

/// Rows as short text: `F:Name(count)`, `U(count)`, `V:id`; a `*` after the letter means pinned.
fn show(rows: &[Row], items: &[Item]) -> Vec<String> {
    rows.iter()
        .map(|row| match row {
            Row::Folder {
                path,
                pinned,
                vehicle_count,
                ..
            } => {
                format!(
                    "F{}:{path}({vehicle_count})",
                    if *pinned { "*" } else { "" }
                )
            }
            Row::Uncategorized { vehicle_count, .. } => format!("U({vehicle_count})"),
            Row::Vehicle { index, pinned } => {
                format!("V{}:{}", if *pinned { "*" } else { "" }, items[*index].id)
            }
        })
        .collect()
}

/// The game's items, in its order:
/// 0 a1 Alpha (unfiled), 1 j1 "Jet 1" (Planes/Jets), 2 t1 Truck (Trucks), 3 p1 Prop (Planes),
/// 4 z1 Zed (unfiled), 5 j2 "Jet 2" (Planes/Jets).
fn items() -> Vec<Item> {
    [
        ("a1", "Alpha"),
        ("j1", "Jet 1"),
        ("t1", "Truck"),
        ("p1", "Prop"),
        ("z1", "Zed"),
        ("j2", "Jet 2"),
    ]
    .into_iter()
    .map(|(id, title)| Item::new(id, title))
    .collect()
}

/// Folders Planes, Planes/Jets, Trucks, Empty and the filing above.
fn tree() -> Tree {
    let mut tree = Tree::new();
    tree.create_folder(None, "Planes").unwrap();
    tree.create_folder(Some(&path("Planes")), "Jets").unwrap();
    tree.create_folder(None, "Trucks").unwrap();
    tree.create_folder(None, "Empty").unwrap();
    for (id, folder) in [
        ("j1", "Planes/Jets"),
        ("t1", "Trucks"),
        ("p1", "Planes"),
        ("j2", "Planes/Jets"),
    ] {
        tree.move_vehicle(id, Some(&path(folder))).unwrap();
    }
    tree
}

fn listing(tree: &Tree, options: &ListOptions) -> Vec<String> {
    let items = items();
    show(&list(tree, &items, options), &items)
}

// ---- Off and search: flat ----

#[test]
fn off_is_the_flat_list() {
    let rows = listing(&tree(), &opts(ViewMode::Off, at("Planes")));
    assert_eq!(rows, ["V:a1", "V:j1", "V:t1", "V:p1", "V:z1", "V:j2"]);
}

#[test]
fn off_puts_pinned_vehicles_first_in_the_games_order() {
    let mut tree = tree();
    tree.set_vehicle_pinned("z1", true);
    tree.set_vehicle_pinned("j1", true);
    let rows = listing(&tree, &opts(ViewMode::Off, Location::Root));
    assert_eq!(rows, ["V*:j1", "V*:z1", "V:a1", "V:t1", "V:p1", "V:j2"]);
}

#[test]
fn search_is_flat_in_every_mode_and_ignores_the_location() {
    let mut tree = tree();
    tree.set_vehicle_pinned("t1", true);
    for mode in [
        ViewMode::Mixed,
        ViewMode::FoldersFirst,
        ViewMode::UncategorizedFolder,
        ViewMode::Off,
    ] {
        let options = ListOptions {
            search_active: true,
            ..opts(mode, at("Planes/Jets"))
        };
        assert_eq!(
            listing(&tree, &options),
            ["V*:t1", "V:a1", "V:j1", "V:p1", "V:z1", "V:j2"],
            "{mode:?}"
        );
    }
}

#[test]
fn no_items_no_rows_without_folders() {
    let rows = list(&Tree::new(), &[], &ListOptions::default());
    assert!(rows.is_empty());
}

// ---- FoldersFirst ----

#[test]
fn folders_first_at_the_root() {
    let rows = listing(&tree(), &opts(ViewMode::FoldersFirst, Location::Root));
    assert_eq!(
        rows,
        ["F:Empty(0)", "F:Planes(3)", "F:Trucks(1)", "V:a1", "V:z1"]
    );
}

#[test]
fn folders_first_is_the_default_mode() {
    assert_eq!(ViewMode::default(), ViewMode::FoldersFirst);
    assert_eq!(ListOptions::default().location, Location::Root);
}

#[test]
fn folders_first_inside_a_folder() {
    let rows = listing(&tree(), &opts(ViewMode::FoldersFirst, at("Planes")));
    assert_eq!(rows, ["F:Planes/Jets(2)", "V:p1"]);
}

#[test]
fn folders_first_inside_a_leaf_folder() {
    let rows = listing(&tree(), &opts(ViewMode::FoldersFirst, at("planes/jets")));
    assert_eq!(rows, ["V:j1", "V:j2"], "vehicles in the game's order");
}

#[test]
fn an_empty_folder_shows_nothing() {
    assert!(listing(&tree(), &opts(ViewMode::FoldersFirst, at("Empty"))).is_empty());
}

#[test]
fn a_missing_location_shows_the_root() {
    let root = listing(&tree(), &opts(ViewMode::FoldersFirst, Location::Root));
    assert_eq!(
        listing(&tree(), &opts(ViewMode::FoldersFirst, at("Nope/Gone"))),
        root
    );
}

#[test]
fn uncategorized_location_outside_its_mode_shows_the_root() {
    let root = listing(&tree(), &opts(ViewMode::FoldersFirst, Location::Root));
    assert_eq!(
        listing(
            &tree(),
            &opts(ViewMode::FoldersFirst, Location::Uncategorized)
        ),
        root
    );
    let root = listing(&tree(), &opts(ViewMode::Mixed, Location::Root));
    assert_eq!(
        listing(&tree(), &opts(ViewMode::Mixed, Location::Uncategorized)),
        root
    );
}

#[test]
fn folders_are_sorted_naturally_ignoring_case() {
    let mut tree = Tree::new();
    for name in ["Car 10", "car 2", "Car 1", "boats", "Zebra"] {
        tree.create_folder(None, name).unwrap();
    }
    let rows = show(
        &list(&tree, &[], &opts(ViewMode::FoldersFirst, Location::Root)),
        &[],
    );
    assert_eq!(
        rows,
        [
            "F:boats(0)",
            "F:Car 1(0)",
            "F:car 2(0)",
            "F:Car 10(0)",
            "F:Zebra(0)"
        ]
    );
}

// ---- pins ----

#[test]
fn pins_come_first_at_each_level() {
    let mut tree = tree();
    tree.set_folder_pinned(&path("Trucks"), true).unwrap();
    tree.set_vehicle_pinned("z1", true);
    let rows = listing(&tree, &opts(ViewMode::FoldersFirst, Location::Root));
    assert_eq!(
        rows,
        ["F*:Trucks(1)", "V*:z1", "F:Empty(0)", "F:Planes(3)", "V:a1"]
    );
}

#[test]
fn pinned_folders_sort_among_themselves_by_name() {
    let mut tree = tree();
    tree.set_folder_pinned(&path("Trucks"), true).unwrap();
    tree.set_folder_pinned(&path("Empty"), true).unwrap();
    let rows = listing(&tree, &opts(ViewMode::FoldersFirst, Location::Root));
    assert_eq!(&rows[..3], ["F*:Empty(0)", "F*:Trucks(1)", "F:Planes(3)"]);
}

#[test]
fn a_pinned_vehicle_only_shows_in_its_own_folder() {
    let mut tree = tree();
    tree.set_vehicle_pinned("j1", true);
    let root = listing(&tree, &opts(ViewMode::FoldersFirst, Location::Root));
    assert!(!root.iter().any(|row| row.contains("j1")), "{root:?}");
    let planes = listing(&tree, &opts(ViewMode::FoldersFirst, at("Planes")));
    assert!(!planes.iter().any(|row| row.contains("j1")), "{planes:?}");
    let jets = listing(&tree, &opts(ViewMode::FoldersFirst, at("Planes/Jets")));
    assert_eq!(jets, ["V*:j1", "V:j2"]);
}

#[test]
fn pinned_vehicles_keep_the_games_order_among_themselves() {
    let mut tree = tree();
    tree.set_vehicle_pinned("z1", true);
    tree.set_vehicle_pinned("a1", true);
    let rows = listing(&tree, &opts(ViewMode::FoldersFirst, Location::Root));
    assert_eq!(&rows[..2], ["V*:a1", "V*:z1"]);
}

#[test]
fn pins_apply_in_mixed_and_uncategorized_modes() {
    let mut tree = tree();
    tree.set_folder_pinned(&path("Trucks"), true).unwrap();
    tree.set_vehicle_pinned("z1", true);
    let mixed = listing(&tree, &opts(ViewMode::Mixed, Location::Root));
    assert_eq!(&mixed[..2], ["F*:Trucks(1)", "V*:z1"]);
    let uncategorized = listing(&tree, &opts(ViewMode::UncategorizedFolder, Location::Root));
    assert_eq!(uncategorized.first().unwrap(), "F*:Trucks(1)");
    let inside = listing(
        &tree,
        &opts(ViewMode::UncategorizedFolder, Location::Uncategorized),
    );
    assert_eq!(inside, ["V*:z1", "V:a1"]);
}

// ---- Mixed ----

#[test]
fn mixed_without_title_sort_puts_a_folder_where_its_first_vehicle_is() {
    // Root vehicles: a1 (0), z1 (4). Planes' first is j1 (1), Trucks' is t1 (2).
    let rows = listing(&tree(), &opts(ViewMode::Mixed, Location::Root));
    assert_eq!(
        rows,
        ["V:a1", "F:Planes(3)", "F:Trucks(1)", "V:z1", "F:Empty(0)"]
    );
}

#[test]
fn mixed_folder_position_looks_through_the_whole_subtree() {
    // Planes holds nothing directly except p1 (3), but j1 (1) is in Planes/Jets: Planes sits at 1.
    let items = items();
    let rows = show(
        &list(&tree(), &items, &opts(ViewMode::Mixed, Location::Root)),
        &items,
    );
    let planes = rows.iter().position(|r| r.starts_with("F:Planes")).unwrap();
    let alpha = rows.iter().position(|r| r == "V:a1").unwrap();
    assert_eq!(planes, alpha + 1);
}

#[test]
fn mixed_inside_a_folder() {
    // In Planes: subfolder Jets (first j1 = 1), vehicle p1 (3).
    let rows = listing(&tree(), &opts(ViewMode::Mixed, at("Planes")));
    assert_eq!(rows, ["F:Planes/Jets(2)", "V:p1"]);
}

#[test]
fn mixed_empty_folders_go_last_by_name() {
    let mut tree = tree();
    tree.create_folder(None, "Aaa").unwrap();
    let rows = listing(&tree, &opts(ViewMode::Mixed, Location::Root));
    assert_eq!(&rows[rows.len() - 2..], ["F:Aaa(0)", "F:Empty(0)"]);
}

#[test]
fn mixed_with_title_sort_puts_folders_among_the_titles() {
    // Titles sorted: Alpha, Zed at the root. Folders Empty, Planes, Trucks fall between.
    let mut tree = Tree::new();
    for name in ["Beta", "Zulu"] {
        tree.create_folder(None, name).unwrap();
    }
    let items: Vec<Item> = ["Alpha", "Carrot", "Zed"]
        .iter()
        .map(|t| Item::new(*t, *t))
        .collect();
    let options = ListOptions {
        sort_by_title: true,
        ..opts(ViewMode::Mixed, Location::Root)
    };
    let rows = show(&list(&tree, &items, &options), &items);
    assert_eq!(
        rows,
        ["V:Alpha", "F:Beta(0)", "V:Carrot", "V:Zed", "F:Zulu(0)"]
    );
}

#[test]
fn mixed_with_title_sort_is_natural() {
    let mut tree = Tree::new();
    tree.create_folder(None, "Car 10").unwrap();
    let items: Vec<Item> = ["Car 2", "Car 9", "Car 11"]
        .iter()
        .map(|t| Item::new(*t, *t))
        .collect();
    let options = ListOptions {
        sort_by_title: true,
        ..opts(ViewMode::Mixed, Location::Root)
    };
    let rows = show(&list(&tree, &items, &options), &items);
    assert_eq!(rows, ["V:Car 2", "V:Car 9", "F:Car 10(0)", "V:Car 11"]);
}

#[test]
fn mixed_with_title_sort_puts_a_folder_before_an_equal_title() {
    let mut tree = Tree::new();
    tree.create_folder(None, "Same").unwrap();
    let items = vec![Item::new("v", "same")];
    let options = ListOptions {
        sort_by_title: true,
        ..opts(ViewMode::Mixed, Location::Root)
    };
    assert_eq!(
        show(&list(&tree, &items, &options), &items),
        ["F:Same(0)", "V:v"]
    );
}

#[test]
fn mixed_with_title_sort_keeps_the_games_order_of_vehicles() {
    // The game's order isn't ours; we never reorder vehicles, only place folders.
    let mut tree = Tree::new();
    tree.create_folder(None, "M").unwrap();
    let items: Vec<Item> = ["b", "a", "z"].iter().map(|t| Item::new(*t, *t)).collect();
    let options = ListOptions {
        sort_by_title: true,
        ..opts(ViewMode::Mixed, Location::Root)
    };
    let rows = show(&list(&tree, &items, &options), &items);
    let order: Vec<_> = rows.iter().filter(|r| r.starts_with("V:")).collect();
    assert_eq!(order, ["V:b", "V:a", "V:z"]);
}

#[test]
fn mixed_with_title_sort_places_only_the_unpinned_ones_among_titles() {
    let mut tree = Tree::new();
    tree.create_folder(None, "M").unwrap();
    tree.set_vehicle_pinned("z", true);
    let items: Vec<Item> = ["a", "z"].iter().map(|t| Item::new(*t, *t)).collect();
    let options = ListOptions {
        sort_by_title: true,
        ..opts(ViewMode::Mixed, Location::Root)
    };
    assert_eq!(
        show(&list(&tree, &items, &options), &items),
        ["V*:z", "V:a", "F:M(0)"]
    );
}

// ---- UncategorizedFolder ----

#[test]
fn uncategorized_mode_at_the_root_shows_folders_and_one_row() {
    let rows = listing(
        &tree(),
        &opts(ViewMode::UncategorizedFolder, Location::Root),
    );
    assert_eq!(rows, ["F:Empty(0)", "F:Planes(3)", "F:Trucks(1)", "U(2)"]);
}

#[test]
fn uncategorized_mode_hides_the_row_when_nothing_is_unfiled() {
    let mut tree = tree();
    tree.move_vehicle("a1", Some(&path("Trucks"))).unwrap();
    tree.move_vehicle("z1", Some(&path("Trucks"))).unwrap();
    let rows = listing(&tree, &opts(ViewMode::UncategorizedFolder, Location::Root));
    assert_eq!(rows, ["F:Empty(0)", "F:Planes(3)", "F:Trucks(3)"]);
}

#[test]
fn uncategorized_mode_with_no_folders_shows_only_the_row() {
    let rows = listing(
        &Tree::new(),
        &opts(ViewMode::UncategorizedFolder, Location::Root),
    );
    assert_eq!(rows, ["U(6)"]);
}

#[test]
fn inside_uncategorized_the_unfiled_vehicles() {
    let rows = listing(
        &tree(),
        &opts(ViewMode::UncategorizedFolder, Location::Uncategorized),
    );
    assert_eq!(rows, ["V:a1", "V:z1"]);
}

#[test]
fn uncategorized_mode_inside_a_folder_is_like_folders_first() {
    let a = listing(&tree(), &opts(ViewMode::UncategorizedFolder, at("Planes")));
    let b = listing(&tree(), &opts(ViewMode::FoldersFirst, at("Planes")));
    assert_eq!(a, b);
}

// ---- unfiled and unknown ----

#[test]
fn vehicles_with_a_vanished_folder_count_as_unfiled() {
    let mut tree = tree();
    tree.remove_folder_only(&path("Trucks"));
    let rows = listing(&tree, &opts(ViewMode::FoldersFirst, Location::Root));
    assert_eq!(rows, ["F:Empty(0)", "F:Planes(3)", "V:a1", "V:t1", "V:z1"]);
}

#[test]
fn unknown_vehicles_are_unfiled_and_unpinned() {
    let rows = listing(&Tree::new(), &opts(ViewMode::FoldersFirst, Location::Root));
    assert_eq!(rows, ["V:a1", "V:j1", "V:t1", "V:p1", "V:z1", "V:j2"]);
}

#[test]
fn listed_subtree_counts_only_count_listed_items() {
    // With only two of the six items given, counts follow what's listed.
    let tree = tree();
    let items = vec![Item::new("j1", "Jet 1"), Item::new("a1", "Alpha")];
    let rows = show(
        &list(&tree, &items, &opts(ViewMode::FoldersFirst, Location::Root)),
        &items,
    );
    assert_eq!(rows, ["F:Empty(0)", "F:Planes(1)", "F:Trucks(0)", "V:a1"]);
}

#[test]
fn rows_carry_the_trees_spelling_of_folders() {
    let items = items();
    let rows = list(&tree(), &items, &opts(ViewMode::FoldersFirst, at("pLANES")));
    match &rows[0] {
        Row::Folder { path, .. } => assert_eq!(path.to_string(), "Planes/Jets"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn vehicle_rows_point_into_the_input() {
    let items = items();
    let rows = list(
        &tree(),
        &items,
        &opts(ViewMode::FoldersFirst, at("Planes/Jets")),
    );
    assert_eq!(
        rows,
        [
            Row::Vehicle {
                index: 1,
                pinned: false
            },
            Row::Vehicle {
                index: 5,
                pinned: false
            }
        ]
    );
}

#[test]
fn deep_folder_counts_reach_every_ancestor() {
    let mut tree = Tree::new();
    let a = tree.create_folder(None, "A").unwrap();
    let b = tree.create_folder(Some(&a), "B").unwrap();
    let c = tree.create_folder(Some(&b), "C").unwrap();
    tree.move_vehicle("x", Some(&c)).unwrap();
    tree.move_vehicle("y", Some(&b)).unwrap();
    let items = vec![Item::new("x", "x"), Item::new("y", "y")];
    let at_root = show(
        &list(&tree, &items, &opts(ViewMode::FoldersFirst, Location::Root)),
        &items,
    );
    assert_eq!(at_root, ["F:A(2)"]);
    let at_a = show(
        &list(
            &tree,
            &items,
            &opts(ViewMode::FoldersFirst, Location::Folder(a)),
        ),
        &items,
    );
    assert_eq!(at_a, ["F:A/B(2)"]);
    let at_b = show(
        &list(
            &tree,
            &items,
            &opts(ViewMode::FoldersFirst, Location::Folder(b)),
        ),
        &items,
    );
    assert_eq!(at_b, ["F:A/B/C(1)", "V:y"]);
}

#[test]
fn same_named_subfolders_in_different_parents_dont_mix_up_counts() {
    let mut tree = Tree::new();
    let a = tree.create_folder(None, "A").unwrap();
    let b = tree.create_folder(None, "B").unwrap();
    let a_x = tree.create_folder(Some(&a), "X").unwrap();
    let b_x = tree.create_folder(Some(&b), "X").unwrap();
    tree.move_vehicle("1", Some(&a_x)).unwrap();
    tree.move_vehicle("2", Some(&b_x)).unwrap();
    tree.move_vehicle("3", Some(&b_x)).unwrap();
    let items: Vec<Item> = ["1", "2", "3"].iter().map(|t| Item::new(*t, *t)).collect();
    let rows = show(
        &list(&tree, &items, &opts(ViewMode::FoldersFirst, Location::Root)),
        &items,
    );
    assert_eq!(rows, ["F:A(1)", "F:B(2)"]);
    let rows = show(
        &list(
            &tree,
            &items,
            &opts(ViewMode::FoldersFirst, Location::Folder(b)),
        ),
        &items,
    );
    assert_eq!(rows, ["F:B/X(2)"]);
}

// ---- every mode keeps every vehicle reachable ----

#[test]
fn every_vehicle_is_listed_exactly_once_across_the_folders() {
    // Walk the folder tree in FoldersFirst and collect every vehicle row.
    let tree = tree();
    let items = items();
    let mut seen = Vec::new();
    let mut to_visit = vec![Location::Root];
    while let Some(location) = to_visit.pop() {
        for row in list(&tree, &items, &opts(ViewMode::FoldersFirst, location)) {
            match row {
                Row::Folder { path, .. } => to_visit.push(Location::Folder(path)),
                Row::Vehicle { index, .. } => seen.push(index),
                Row::Uncategorized { .. } => unreachable!(),
            }
        }
    }
    seen.sort_unstable();
    assert_eq!(seen, [0, 1, 2, 3, 4, 5]);
}

#[test]
fn uncategorized_mode_keeps_every_vehicle_reachable() {
    let tree = tree();
    let items = items();
    let mut seen = Vec::new();
    let mut to_visit = vec![Location::Root];
    while let Some(location) = to_visit.pop() {
        for row in list(
            &tree,
            &items,
            &opts(ViewMode::UncategorizedFolder, location),
        ) {
            match row {
                Row::Folder { path, .. } => to_visit.push(Location::Folder(path)),
                Row::Uncategorized { .. } => to_visit.push(Location::Uncategorized),
                Row::Vehicle { index, .. } => seen.push(index),
            }
        }
    }
    seen.sort_unstable();
    assert_eq!(seen, [0, 1, 2, 3, 4, 5]);
}

// ---- speed ----

#[test]
fn ten_thousand_items_list_quickly() {
    let mut tree = Tree::new();
    let mut folders = Vec::new();
    for top in 0..20 {
        let parent = tree.create_folder(None, &format!("Top {top}")).unwrap();
        for sub in 0..10 {
            folders.push(
                tree.create_folder(Some(&parent), &format!("Sub {sub}"))
                    .unwrap(),
            );
        }
        folders.push(parent);
    }
    let items: Vec<Item> = (0..10_000)
        .map(|n| Item::new(n.to_string(), format!("Vehicle {n}")))
        .collect();
    for (n, item) in items.iter().enumerate() {
        if n % 3 != 0 {
            tree.move_vehicle(&item.id, Some(&folders[n % folders.len()]))
                .unwrap();
        }
        if n % 50 == 0 {
            tree.set_vehicle_pinned(&item.id, true);
        }
    }

    let start = Instant::now();
    let mut rows = 0;
    for mode in [
        ViewMode::Mixed,
        ViewMode::FoldersFirst,
        ViewMode::UncategorizedFolder,
        ViewMode::Off,
    ] {
        for location in [
            Location::Root,
            at("Top 3"),
            at("Top 3/Sub 4"),
            Location::Uncategorized,
        ] {
            for sort_by_title in [false, true] {
                let options = ListOptions {
                    location: location.clone(),
                    mode,
                    search_active: false,
                    sort_by_title,
                };
                rows += list(&tree, &items, &options).len();
            }
        }
    }
    let elapsed = start.elapsed();
    assert!(rows > 0);
    // 32 listings of 10 000 items; a generous bound that only catches quadratic behaviour.
    assert!(elapsed < Duration::from_secs(10), "took {elapsed:?}");
}

#[test]
fn ten_thousand_unfiled_items_in_folders_first_lists_all_of_them() {
    let tree = Tree::new();
    let items: Vec<Item> = (0..10_000)
        .map(|n| Item::new(n.to_string(), n.to_string()))
        .collect();
    let rows = list(&tree, &items, &ListOptions::default());
    assert_eq!(rows.len(), 10_000);
    assert_eq!(
        rows[9_999],
        Row::Vehicle {
            index: 9_999,
            pinned: false
        }
    );
}

// ---- Previews: the vehicles a folder tile shows ----

/// Each folder row's preview as ids, `name: id id ...` (`U` for Uncategorized).
fn previews(tree: &Tree, items: &[Item], options: &ListOptions) -> Vec<String> {
    let ids = |preview: &[usize]| {
        preview
            .iter()
            .map(|&index| items[index].id.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };
    list(tree, items, options)
        .iter()
        .filter_map(|row| match row {
            Row::Folder { path, preview, .. } => Some(format!("{path}: {}", ids(preview))),
            Row::Uncategorized { preview, .. } => Some(format!("U: {}", ids(preview))),
            Row::Vehicle { .. } => None,
        })
        .collect()
}

#[test]
fn a_preview_shows_vehicles_filed_in_the_folder_before_deeper_ones() {
    // Planes holds p1 itself and j1, j2 in Jets: p1 first though j1 comes first in the game's order.
    assert_eq!(
        previews(
            &tree(),
            &items(),
            &opts(ViewMode::FoldersFirst, Location::Root)
        ),
        ["Empty: ", "Planes: p1 j1 j2", "Trucks: t1"]
    );
    assert_eq!(
        previews(
            &tree(),
            &items(),
            &opts(ViewMode::FoldersFirst, at("Planes"))
        ),
        ["Planes/Jets: j1 j2"]
    );
}

#[test]
fn a_preview_holds_at_most_four_pinned_ones_first() {
    let mut tree = Tree::new();
    tree.create_folder(None, "Many").unwrap();
    tree.create_folder(Some(&path("Many")), "Sub").unwrap();
    let items: Vec<Item> = (0..8).map(|n| Item::new(format!("v{n}"), "")).collect();
    // v0 deeper, v1..v7 filed in Many, v5 pinned.
    tree.move_vehicle("v0", Some(&path("Many/Sub"))).unwrap();
    for n in 1..8 {
        tree.move_vehicle(&format!("v{n}"), Some(&path("Many")))
            .unwrap();
    }
    tree.set_vehicle_pinned("v5", true);
    assert_eq!(
        previews(&tree, &items, &opts(ViewMode::Mixed, Location::Root)),
        ["Many: v5 v1 v2 v3"]
    );
}

#[test]
fn a_preview_takes_deeper_vehicles_when_few_are_filed_in_it() {
    let mut tree = Tree::new();
    tree.create_folder(None, "Top").unwrap();
    tree.create_folder(Some(&path("Top")), "Sub").unwrap();
    let items: Vec<Item> = (0..6).map(|n| Item::new(format!("v{n}"), "")).collect();
    for n in 0..5 {
        tree.move_vehicle(&format!("v{n}"), Some(&path("Top/Sub")))
            .unwrap();
    }
    tree.move_vehicle("v5", Some(&path("Top"))).unwrap();
    assert_eq!(
        previews(&tree, &items, &opts(ViewMode::FoldersFirst, Location::Root)),
        ["Top: v5 v0 v1 v2"]
    );
}

#[test]
fn the_uncategorized_preview_is_its_first_rows() {
    let mut tree = tree();
    tree.set_vehicle_pinned("z1", true);
    assert_eq!(
        previews(
            &tree,
            &items(),
            &opts(ViewMode::UncategorizedFolder, Location::Root)
        ),
        ["Empty: ", "Planes: p1 j1 j2", "Trucks: t1", "U: z1 a1"]
    );
}

#[test]
fn previews_point_into_the_input_and_count_no_more_than_the_folder() {
    let items = items();
    for row in list(
        &tree(),
        &items,
        &opts(ViewMode::FoldersFirst, Location::Root),
    ) {
        if let Row::Folder {
            preview,
            vehicle_count,
            ..
        } = row
        {
            assert!(preview.len() <= vehicle_count.min(PREVIEW_LEN));
            assert!(preview.iter().all(|&index| index < items.len()));
        }
    }
}

// ---- newest_in ----

#[test]
fn newest_in_takes_the_subtree_and_nothing_else() {
    let mut tree = Tree::new();
    tree.create_folder(None, "Planes").unwrap();
    tree.create_folder(Some(&path("Planes")), "Jets").unwrap();
    tree.create_folder(None, "Trucks").unwrap();
    tree.move_vehicle("1", Some(&path("Planes"))).unwrap();
    tree.move_vehicle("2", Some(&path("Planes/Jets"))).unwrap();
    tree.move_vehicle("3", Some(&path("Trucks"))).unwrap();
    let vehicles = [
        ("1", 100, 500),
        ("2", 300, 400),
        ("3", 900, 900),
        ("4", 950, 950),
    ];
    assert_eq!(
        newest_in(&tree, &path("Planes"), vehicles),
        Newest {
            created: Some(300),
            updated: Some(500),
        }
    );
    assert_eq!(
        newest_in(&tree, &path("planes/jets"), vehicles),
        Newest {
            created: Some(300),
            updated: Some(400),
        }
    );
}

#[test]
fn newest_in_an_empty_folder_is_unknown() {
    let mut tree = Tree::new();
    tree.create_folder(None, "Empty").unwrap();
    tree.create_folder(None, "Full").unwrap();
    tree.move_vehicle("1", Some(&path("Full"))).unwrap();
    assert_eq!(
        newest_in(&tree, &path("Empty"), [("1", 5, 6)]),
        Newest::default()
    );
}

#[test]
fn newest_in_ignores_vehicles_of_a_deleted_folder() {
    let mut tree = Tree::new();
    tree.create_folder(None, "A").unwrap();
    tree.create_folder(Some(&path("A")), "B").unwrap();
    tree.move_vehicle("1", Some(&path("A/B"))).unwrap();
    tree.remove_folder_only(&path("A/B"));
    assert_eq!(
        newest_in(&tree, &path("A"), [("1", 5, 6)]),
        Newest::default()
    );
}
