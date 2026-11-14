use super::*;

fn path(s: &str) -> FolderPath {
    FolderPath::new(s).unwrap()
}

fn at(s: &str) -> Location {
    Location::Folder(path(s))
}

/// Planes, Planes/Jets, Trucks; `p1` in Planes, `j1` in Planes/Jets, `z1` in none.
fn sample() -> Tree {
    let mut tree = Tree::new();
    tree.create_folder(None, "Planes").unwrap();
    tree.create_folder(Some(&path("Planes")), "Jets").unwrap();
    tree.create_folder(None, "Trucks").unwrap();
    tree.move_vehicle("p1", Some(&path("Planes"))).unwrap();
    tree.move_vehicle("j1", Some(&path("Planes/Jets"))).unwrap();
    tree
}

fn folders(tree: &Tree) -> Vec<String> {
    tree.folders().map(ToString::to_string).collect()
}

fn vehicle(id: &str) -> Dragged {
    Dragged::Vehicle(id.into())
}

// ---- The gesture ----

/// Feeds `steps` (button down, cursor) in; the events, `Nothing`s left out. A press lands on
/// "row" when the cursor's x is below 100, on nothing otherwise.
fn run(
    gesture: &mut Gesture<&'static str>,
    steps: &[(bool, (i32, i32))],
) -> Vec<Event<&'static str>> {
    steps
        .iter()
        .map(|&(down, cursor)| gesture.update(down, cursor, || (cursor.0 < 100).then_some("row")))
        .filter(|event| *event != Event::Nothing)
        .collect()
}

#[test]
fn a_click_is_no_drag() {
    let mut gesture = Gesture::new();
    let events = run(
        &mut gesture,
        &[
            (false, (10, 10)),
            (true, (10, 10)),
            (true, (12, 13)),
            (false, (12, 13)),
        ],
    );
    assert!(events.is_empty());
    assert_eq!(gesture.dragging(), None);
}

#[test]
fn moving_past_the_threshold_while_held_drags_then_drops() {
    let mut gesture = Gesture::new();
    let events = run(
        &mut gesture,
        &[
            (true, (10, 10)),
            (true, (10, 10 + DRAG_THRESHOLD)),
            (true, (300, 300)),
            (false, (300, 300)),
        ],
    );
    assert_eq!(
        events,
        [
            Event::Started("row"),
            Event::Moved("row"),
            Event::Dropped("row")
        ]
    );
    assert_eq!(gesture.dragging(), None);
}

#[test]
fn the_threshold_counts_on_either_axis() {
    let mut gesture = Gesture::new();
    let events = run(
        &mut gesture,
        &[(true, (50, 50)), (true, (50 - DRAG_THRESHOLD, 50))],
    );
    assert_eq!(events, [Event::Started("row")]);
    assert_eq!(gesture.dragging(), Some(&"row"));
}

#[test]
fn a_press_on_nothing_never_drags_even_over_a_row() {
    let mut gesture = Gesture::new();
    let events = run(
        &mut gesture,
        &[
            (true, (500, 10)),
            (true, (10, 10)),
            (true, (10, 300)),
            (false, (10, 300)),
        ],
    );
    assert!(events.is_empty());
    // The next press counts again.
    let events = run(&mut gesture, &[(true, (10, 10)), (true, (10, 100))]);
    assert_eq!(events, [Event::Started("row")]);
}

#[test]
fn a_button_already_down_when_watching_starts_is_ignored() {
    let mut gesture = Gesture::held();
    let events = run(
        &mut gesture,
        &[(true, (10, 10)), (true, (10, 300)), (false, (10, 300))],
    );
    assert!(events.is_empty());
}

#[test]
fn what_is_pressed_on_is_asked_only_when_the_button_goes_down() {
    let mut gesture = Gesture::new();
    let mut asked = 0;
    for (down, cursor) in [
        (false, (0, 0)),
        (true, (0, 0)),
        (true, (0, 50)),
        (true, (0, 60)),
        (false, (0, 60)),
    ] {
        gesture.update(down, cursor, || {
            asked += 1;
            Some(())
        });
    }
    assert_eq!(asked, 1);
}

#[test]
fn cancelling_gives_back_the_dragged_thing_and_ignores_the_rest_of_the_press() {
    let mut gesture = Gesture::new();
    run(&mut gesture, &[(true, (10, 10)), (true, (10, 100))]);
    assert_eq!(gesture.cancel(), Some("row"));
    let events = run(&mut gesture, &[(true, (10, 200)), (false, (10, 200))]);
    assert!(events.is_empty());
    assert_eq!(gesture.cancel(), None);
    // Idle stays idle: the next press counts.
    let events = run(&mut gesture, &[(true, (10, 10)), (true, (10, 100))]);
    assert_eq!(events, [Event::Started("row")]);
}

#[test]
fn cancelling_a_press_that_is_not_a_drag_yet_ignores_it() {
    let mut gesture = Gesture::new();
    run(&mut gesture, &[(true, (10, 10))]);
    assert_eq!(gesture.cancel(), None);
    let events = run(&mut gesture, &[(true, (10, 100)), (false, (10, 100))]);
    assert!(events.is_empty());
}

// ---- Drops ----

#[test]
fn a_vehicle_dropped_on_a_folder_is_filed_there() {
    let mut tree = sample();
    assert_eq!(
        drop_onto(&mut tree, &vehicle("z1"), &at("Trucks")),
        Ok(true)
    );
    assert_eq!(tree.folder_of("z1"), Some(&path("Trucks")));
    assert_eq!(
        drop_onto(&mut tree, &vehicle("p1"), &at("planes/jets")),
        Ok(true)
    );
    assert_eq!(tree.folder_of("p1"), Some(&path("Planes/Jets")));
}

#[test]
fn a_vehicle_dropped_on_the_top_level_or_uncategorized_leaves_its_folder() {
    let mut tree = sample();
    assert_eq!(
        drop_onto(&mut tree, &vehicle("p1"), &Location::Root),
        Ok(true)
    );
    assert_eq!(tree.folder_of("p1"), None);
    assert_eq!(
        drop_onto(&mut tree, &vehicle("j1"), &Location::Uncategorized),
        Ok(true)
    );
    assert_eq!(tree.folder_of("j1"), None);
}

#[test]
fn dropping_a_vehicle_where_it_is_changes_nothing() {
    let mut tree = sample();
    let before = tree.clone();
    assert_eq!(
        drop_onto(&mut tree, &vehicle("p1"), &at("PLANES")),
        Ok(false)
    );
    assert_eq!(
        drop_onto(&mut tree, &vehicle("z1"), &Location::Root),
        Ok(false)
    );
    assert_eq!(
        drop_onto(&mut tree, &vehicle("z1"), &Location::Uncategorized),
        Ok(false)
    );
    assert_eq!(tree, before);
}

#[test]
fn a_vehicle_dropped_on_a_folder_that_went_away_is_refused() {
    let mut tree = sample();
    let before = tree.clone();
    assert!(drop_onto(&mut tree, &vehicle("z1"), &at("Gone")).is_err());
    assert_eq!(tree, before);
}

#[test]
fn a_folder_dropped_on_a_folder_moves_into_it_with_everything_in_it() {
    let mut tree = sample();
    assert_eq!(
        drop_onto(&mut tree, &Dragged::Folder(path("Planes")), &at("Trucks")),
        Ok(true)
    );
    assert_eq!(
        folders(&tree),
        ["Trucks", "Trucks/Planes", "Trucks/Planes/Jets"]
    );
    assert_eq!(tree.folder_of("j1"), Some(&path("Trucks/Planes/Jets")));
    assert_eq!(tree.folder_of("p1"), Some(&path("Trucks/Planes")));
}

#[test]
fn a_folder_dropped_on_the_top_level_moves_up() {
    let mut tree = sample();
    assert_eq!(
        drop_onto(
            &mut tree,
            &Dragged::Folder(path("Planes/Jets")),
            &Location::Root
        ),
        Ok(true)
    );
    assert_eq!(folders(&tree), ["Jets", "Planes", "Trucks"]);
}

#[test]
fn a_folder_dropped_on_itself_or_where_it_is_changes_nothing() {
    let mut tree = sample();
    let before = tree.clone();
    let jets = Dragged::Folder(path("Planes/Jets"));
    assert_eq!(drop_onto(&mut tree, &jets, &at("Planes/Jets")), Ok(false));
    assert_eq!(drop_onto(&mut tree, &jets, &at("Planes")), Ok(false));
    let planes = Dragged::Folder(path("Planes"));
    assert_eq!(drop_onto(&mut tree, &planes, &Location::Root), Ok(false));
    assert_eq!(
        drop_onto(&mut tree, &planes, &Location::Uncategorized),
        Ok(false)
    );
    assert_eq!(tree, before);
}

#[test]
fn a_folder_dropped_into_its_own_subfolder_is_refused() {
    let mut tree = sample();
    let before = tree.clone();
    assert_eq!(
        drop_onto(
            &mut tree,
            &Dragged::Folder(path("Planes")),
            &at("Planes/Jets")
        ),
        Err(FolderError::IntoOwnSubtree)
    );
    assert_eq!(tree, before);
}

#[test]
fn a_folder_dropped_where_a_sibling_has_its_name_is_refused() {
    let mut tree = sample();
    tree.create_folder(Some(&path("Trucks")), "jets").unwrap();
    let before = tree.clone();
    assert!(matches!(
        drop_onto(
            &mut tree,
            &Dragged::Folder(path("Planes/Jets")),
            &at("Trucks")
        ),
        Err(FolderError::NameTaken(_))
    ));
    assert_eq!(tree, before);
}

#[test]
fn drops_never_lose_vehicles() {
    // Every move in every direction keeps each vehicle filed in a folder that exists, or in none.
    let targets = [
        Location::Root,
        Location::Uncategorized,
        at("Planes"),
        at("Planes/Jets"),
        at("Trucks"),
    ];
    let dragged = [
        vehicle("p1"),
        vehicle("j1"),
        vehicle("z1"),
        Dragged::Folder(path("Planes")),
        Dragged::Folder(path("Planes/Jets")),
        Dragged::Folder(path("Trucks")),
    ];
    for first in &dragged {
        for onto in &targets {
            let mut tree = sample();
            let _ = drop_onto(&mut tree, first, onto);
            for id in ["p1", "j1", "z1"] {
                if let Some(folder) = tree.folder_of(id) {
                    assert!(tree.contains_folder(folder), "{id} in a missing {folder}");
                }
            }
            // Folders only move: none is lost.
            assert_eq!(tree.folders().count(), 3);
        }
    }
}

// ---- Dropping a vehicle onto a vehicle ----

#[test]
fn grouping_two_unfiled_vehicles_makes_a_top_level_folder() {
    let mut tree = sample();
    let made = group_into_new_folder(&mut tree, "z1", "z2").unwrap();
    assert_eq!(made, path("New folder"));
    assert_eq!(tree.folder_of("z1"), Some(&made));
    assert_eq!(tree.folder_of("z2"), Some(&made));
    assert_eq!(
        folders(&tree),
        ["New folder", "Planes", "Planes/Jets", "Trucks"]
    );
}

#[test]
fn grouping_makes_the_folder_where_the_target_is_filed() {
    let mut tree = sample();
    // `z1` (in none) dropped onto `j1` (in Planes/Jets): the folder goes in Planes/Jets.
    let made = group_into_new_folder(&mut tree, "z1", "j1").unwrap();
    assert_eq!(made, path("Planes/Jets/New folder"));
    assert_eq!(tree.folder_of("z1"), Some(&made));
    assert_eq!(tree.folder_of("j1"), Some(&made));
    // The dragged vehicle leaves its own folder.
    let mut tree = sample();
    let made = group_into_new_folder(&mut tree, "p1", "z1").unwrap();
    assert_eq!(made, path("New folder"));
    assert_eq!(tree.folder_of("p1"), Some(&made));
}

#[test]
fn grouping_numbers_the_name_when_taken() {
    let mut tree = sample();
    tree.create_folder(None, "New folder").unwrap();
    tree.create_folder(None, "new folder 2").unwrap();
    let made = group_into_new_folder(&mut tree, "z1", "z2").unwrap();
    assert_eq!(made, path("New folder 3"));
}

#[test]
fn grouping_keeps_pins() {
    let mut tree = sample();
    tree.set_vehicle_pinned("z1", true);
    let made = group_into_new_folder(&mut tree, "z1", "z2").unwrap();
    assert!(tree.is_pinned("z1"));
    assert!(!tree.is_pinned("z2"));
    assert_eq!(tree.folder_of("z1"), Some(&made));
}

#[test]
fn grouping_a_vehicle_with_itself_changes_nothing() {
    let mut tree = sample();
    let before = tree.clone();
    assert_eq!(
        group_into_new_folder(&mut tree, "p1", "p1"),
        Err(FolderError::IntoOwnSubtree)
    );
    assert_eq!(tree, before);
}

#[test]
fn grouping_refuses_a_folder_too_deep_and_changes_nothing() {
    let mut tree = Tree::new();
    let mut parent: Option<FolderPath> = None;
    for level in 0..crate::path::MAX_DEPTH {
        parent = Some(
            tree.create_folder(parent.as_ref(), &format!("L{level}"))
                .unwrap(),
        );
    }
    tree.move_vehicle("deep", parent.as_ref()).unwrap();
    let before = tree.clone();
    assert!(group_into_new_folder(&mut tree, "z1", "deep").is_err());
    assert_eq!(tree, before);
}

#[test]
fn dragging_mut_changes_what_is_dragged() {
    let mut gesture = Gesture::new();
    assert!(gesture.dragging_mut().is_none());
    gesture.update(true, (0, 0), || Some(1));
    assert!(gesture.dragging_mut().is_none(), "a press isn't a drag yet");
    gesture.update(true, (20, 0), || None);
    *gesture.dragging_mut().unwrap() = 2;
    assert_eq!(gesture.update(false, (20, 0), || None), Event::Dropped(2));
}

#[test]
fn page_turns_after_the_delay_then_repeats() {
    let mut timer = HoverTimer::new();
    assert_eq!(timer.update(Some('n'), 1000), None);
    assert_eq!(timer.update(Some('n'), 1000 + PAGE_TURN_DELAY_MS - 1), None);
    assert_eq!(
        timer.update(Some('n'), 1000 + PAGE_TURN_DELAY_MS),
        Some('n')
    );
    let turned = 1000 + PAGE_TURN_DELAY_MS;
    assert_eq!(timer.update(Some('n'), turned + 1), None);
    assert_eq!(
        timer.update(Some('n'), turned + PAGE_TURN_REPEAT_MS - 1),
        None
    );
    assert_eq!(
        timer.update(Some('n'), turned + PAGE_TURN_REPEAT_MS),
        Some('n')
    );
}

#[test]
fn page_turn_waits_again_after_leaving_or_switching() {
    let mut timer = HoverTimer::new();
    timer.update(Some('n'), 0);
    // Left before the delay: starts over.
    assert_eq!(timer.update(None, 500), None);
    assert_eq!(timer.update(Some('n'), 600), None);
    assert_eq!(timer.update(Some('n'), 600 + PAGE_TURN_DELAY_MS - 1), None);
    // Another button: its own delay.
    assert_eq!(timer.update(Some('p'), 600 + PAGE_TURN_DELAY_MS), None);
    assert_eq!(
        timer.update(Some('p'), 600 + 2 * PAGE_TURN_DELAY_MS),
        Some('p')
    );
}

#[test]
fn page_turn_never_fires_without_a_button() {
    let mut timer: HoverTimer<char> = HoverTimer::new();
    for now in (0..5000).step_by(15) {
        assert_eq!(timer.update(None, now), None);
    }
}
