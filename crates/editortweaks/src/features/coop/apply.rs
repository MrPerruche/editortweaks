//! The session's edits, made in the editor: a whole vehicle (joining, a resync) or someone
//! else's edit. Each runs as one editor operation that takes no undo step, as undo does itself, so
//! the game's own handling (mirrored bricks, the vehicle's properties) follows. Someone else's
//! bricks leave the selection as it was (deleted ones drop out of it).
//!
//! Creates, deletes, moves, property sets, reorders and groups (milestone 4,
//! `docs/re/coop/protocol.md`). A created brick is the vehicle file the author's game saved of it, loaded as an import: the
//! editor's own format, with every property. A move is written as the move mode commits one
//! (`SpawnTransforms::set`), with the move flags on the operation so the editor updates what
//! depends on places; its numbers were checked finite and in range when the message was decoded.
//! A property set is written as the panel writes one, with the property flag, through
//! `game::property_values`: only properties this game finds by reflection, every field checked
//! first; bricks a value refers to are found by their coop ids. A reorder is made as the panel
//! makes one (`game::object_order`), one object at a time, the fewest moves (`coop::order`). A
//! group is written as the game's grouping writes one, with the groups flag
//! (`game::brick_groups`); any number is a valid group. The game's loader gives loaded bricks new
//! group numbers, so created bricks start in no group (their groups follow as ops), and a whole
//! vehicle comes with its bricks' groups. The objects a load makes are matched to the op's coop ids
//! in order; another count (e.g. the game adding mirror counterparts) means the editor drifted.

use std::collections::{HashMap, HashSet};

use ::coop::vehicle::Vehicle;
use ::coop::{CoopId, GroupKind, Groups, Op, Transform, Value};

use super::changes::{self, Object};
use super::log::coop_log;
use super::{Game, adopt, objects, place_of, remember_all_groups, remember_all_values};
use crate::features::property_limits;
use crate::game::brick_editor::{MOVED, NO_UNDO_STEP, PROPERTY_CHANGED, UNMODIFIED};
use crate::game::brick_groups::{BrickGroups, GROUPS_CHANGED};
use crate::game::object_order::{ORDER_CHANGED, ObjectOrder};
use crate::game::vehicle_changes;
use crate::game::{property_values, reflection};

/// How applying went, beyond what the editor shows.
#[derive(Default)]
pub struct Outcome {
    /// The editor made other objects than the session expects: ask for a snapshot.
    pub drifted: bool,
    /// Ops refused as impossible here (a property this game doesn't have, a value it can't hold,
    /// a reorder without the game's reorder, a group without knowing where groups are kept).
    pub refused: usize,
    /// Bricks this game can't make (a mod's): the loader skipped them, and the rest are in order.
    pub skipped: Vec<CoopId>,
}

/// Where coop edits.
pub struct Target<'a> {
    pub game: &'a Game,
    /// A live `AVehicleEditor`, and its interface component.
    pub editor: Object,
    pub component: Object,
}

impl Target<'_> {
    /// Runs `change` as one editor operation without an undo step.
    fn operation(&self, change: impl FnOnce()) {
        // SAFETY: the editor is live (it's ticking); game thread.
        unsafe {
            self.game
                .editor
                .operation_with(self.editor, 0, NO_UNDO_STEP, change)
        };
    }

    /// Replaces the vehicle with `vehicle` (a [`Vehicle`]), whose objects have `ids`. `first`:
    /// joining, when the game may move the camera to it and drop the undo steps of the vehicle it
    /// replaces.
    pub fn load(&self, ids: &[CoopId], vehicle: &[u8], first: bool) -> Outcome {
        let drifted = Outcome {
            drifted: true,
            ..Outcome::default()
        };
        let vehicle = match Vehicle::decode(vehicle) {
            Ok(vehicle) => vehicle,
            Err(error) => {
                coop_log!("the session's vehicle is unreadable: {error}");
                return drifted;
            }
        };
        if let Some(reason) = unfit(&vehicle.archive, ids.len()) {
            coop_log!("refused the session's vehicle: {reason}");
            return drifted;
        }
        changes::forget_all();
        let mut result = Ok(());
        let created = changes::applying(|| {
            self.operation(|| {
                // SAFETY: the component is the live editor's; game thread.
                result = unsafe {
                    self.game
                        .archive
                        .load(self.component, &vehicle.archive, false, !first)
                };
            })
        });
        if let Err(reason) = result {
            coop_log!("couldn't load the session's vehicle: {reason}");
            return drifted;
        }
        // SAFETY: the objects the load just made; game thread.
        let made = unsafe { made_of(&vehicle.archive, &created, ids) };
        let ids_here = ids_of(ids, made.as_deref());
        // SAFETY: as above. The editor is live.
        unsafe {
            adopt(self.game, &created, &ids_here);
            changes::remember_order(&objects(self.game, self.editor));
        }
        if let Some(made) = made.as_ref().filter(|_| vehicle.groups.len() == ids.len()) {
            let groups: Vec<Groups> = made.iter().map(|&object| vehicle.groups[object]).collect();
            changes::applying(|| self.operation(|| self.regroup_all(&created, &groups)));
            remember_all_groups(self.game);
        } else if vehicle.groups.is_empty() {
            coop_log!("the session's vehicle came without its groups: they may differ here");
        }
        coop_log!(
            "loaded the session's vehicle: {} objects for {} ids",
            created.len(),
            ids.len()
        );
        if created.len() != ids.len() {
            log_classes(&vehicle.archive);
        }
        Outcome {
            drifted: made.is_none(),
            skipped: skipped_of(ids, made.as_deref()),
            ..Outcome::default()
        }
    }

    /// Makes the bricks `ids` the selection, as a click would (those that are here). Nothing for no
    /// bricks, nor outside the default mode: coop only loads in it.
    pub fn select(&self, ids: &[CoopId]) {
        if ids.iter().any(|&id| changes::object(id).is_some()) {
            self.set_selection(ids);
        }
    }

    /// Makes the bricks `ids` (those that are here) the selection, as a click would: none selects
    /// nothing. Not outside the default mode (undo only runs in it).
    pub fn set_selection(&self, ids: &[CoopId]) {
        let bricks: Vec<Object> = ids.iter().filter_map(|&id| changes::object(id)).collect();
        let Ok(selector) = &self.game.selector else {
            return;
        };
        // SAFETY: the editor is live (it's ticking); game thread.
        let Some(mode) = (unsafe { self.game.modes.default_mode(self.editor) }) else {
            return;
        };
        // SAFETY: the editor's default mode, and its live objects (they have coop ids).
        unsafe { selector.select(mode, &bricks, false) };
    }

    /// Finds the session's vehicle in this editor, new since the player left the last one (to
    /// test the vehicle, say), whose bricks were `known`: the editor's objects must be as many,
    /// each where one of them was (property values hold pointers, so they're not compared: audits
    /// find what else differs). The order may differ: the game's loader puts seats first. Bricks
    /// at one place are paired in the order they come. They then get their coop ids again, and
    /// their groups (the game's loader renumbers them). `Ok(true)`: in another order than the
    /// session's (members are to get this one). `Err` says what differs.
    pub fn refit(&self, known: &[changes::Known]) -> Result<bool, String> {
        // SAFETY: the editor is live (it's ticking); game thread.
        let objects = unsafe { objects(self.game, self.editor) };
        if objects.len() != known.len() {
            return Err(format!(
                "{} objects here, {} in the session",
                objects.len(),
                known.len()
            ));
        }
        // The known bricks by rounded location, in reverse: the last fitting one is the first.
        let mut at: HashMap<[i64; 3], Vec<&changes::Known>> = HashMap::new();
        for brick in known.iter().rev() {
            if let Some(place) = &brick.place {
                at.entry(rounded(place)).or_default().push(brick);
            }
        }
        let mut found = Vec::with_capacity(objects.len());
        for (index, &object) in objects.iter().enumerate() {
            // SAFETY: the editor's live objects; game thread.
            let place = unsafe { place_of(self.game, object) }
                .ok_or_else(|| format!("object {index} has no place"))?;
            let bricks = at.get_mut(&rounded(&place));
            let brick = bricks.and_then(|bricks| {
                let at = bricks
                    .iter()
                    .rposition(|brick| brick.place.is_some_and(|was| same_place(&was, &place)))?;
                Some(bricks.remove(at))
            });
            found.push(brick.ok_or_else(|| format!("object {index} is elsewhere"))?);
        }
        let absent = changes::absent();
        changes::watch(self.editor, self.component);
        changes::mark_absent(absent);
        let ids: Vec<CoopId> = found.iter().map(|brick| brick.id).collect();
        let reordered = !ids.iter().eq(known.iter().map(|brick| &brick.id));
        // SAFETY: as above.
        unsafe {
            adopt(self.game, &objects, &ids);
            changes::remember_order(&objects);
        }
        let groups: Vec<Groups> = found.iter().map(|brick| brick.groups).collect();
        changes::applying(|| self.operation(|| self.regroup_all(&objects, &groups)));
        remember_all_groups(self.game);
        Ok(reordered)
    }

    /// Makes an edit not made in this editor. `theirs`: a peer's, which must be one the panel
    /// could make (`checks`); not the subhost's object order mending ours, nor our own steps back.
    pub fn apply(&self, ops: &[Op], theirs: bool) -> Outcome {
        let mut outcome = Outcome::default();
        // Bricks the edit makes may go anywhere in the order (undoing a delete puts them back).
        let made: Vec<CoopId> = ops
            .iter()
            .flat_map(|op| match op {
                Op::Create { ids, .. } => ids.as_slice(),
                _ => &[],
            })
            .copied()
            .collect();
        let created = changes::applying(|| {
            self.operation(|| {
                let mut doomed = Vec::new();
                for op in ops {
                    match op {
                        Op::Create { ids, archive } => {
                            self.destroy(&mut doomed);
                            self.create(ids, archive, &mut outcome);
                        }
                        Op::Delete { id } => doomed.extend(changes::take_object(*id)),
                        Op::Move { id, transform } => {
                            outcome.drifted |= !self.place(*id, transform)
                        }
                        Op::Set {
                            id,
                            property,
                            value,
                        } => {
                            let value = match value {
                                Value::Raw(bytes) => property_values::Value::Data(bytes.clone()),
                                Value::Text(text) => property_values::Value::Text(text.clone()),
                                // Bricks deleted here meanwhile drop out, as the game drops
                                // references to deleted bricks.
                                Value::Bricks(ids) => property_values::Value::Bricks(
                                    ids.iter().filter_map(|&id| changes::object(id)).collect(),
                                ),
                            };
                            outcome.refused += usize::from(!self.set(*id, property, &value));
                        }
                        Op::Order { ids } if theirs && !self.panel_reorders(ids, &made) => {
                            outcome.refused += 1;
                        }
                        Op::Order { ids } => match &self.game.order {
                            Ok(order) => outcome.drifted |= !self.reorder(order, ids),
                            Err(reason) => {
                                coop_log!("refused a reorder: {reason}");
                                outcome.refused += 1;
                            }
                        },
                        Op::Group { id, kind, group } => match &self.game.groups {
                            Ok(groups) => {
                                outcome.refused +=
                                    usize::from(!self.group(groups, *id, *kind, *group))
                            }
                            Err(reason) => {
                                coop_log!("refused a group: {reason}");
                                outcome.refused += 1;
                            }
                        },
                    }
                }
                self.destroy(&mut doomed);
                // Loads that keep the selection set it (see `create`): the vehicle did change.
                // SAFETY: as for `operation`; the operation is this one.
                unsafe { self.game.editor.clear_open_bits(self.editor, UNMODIFIED) };
            })
        });
        debug_assert!(created.is_empty(), "every load's objects are taken");
        if ops.iter().any(|op| matches!(op, Op::Set { .. })) {
            remember_all_values(self.game);
        }
        if ops
            .iter()
            .any(|op| matches!(op, Op::Create { .. } | Op::Group { .. }))
        {
            remember_all_groups(self.game);
        }
        // SAFETY: the editor is live (it's ticking); game thread.
        changes::remember_order(&unsafe { objects(self.game, self.editor) });
        outcome
    }

    /// Loads one created op's objects, telling `outcome` if they aren't what its ids say.
    fn create(&self, ids: &[CoopId], archive: &[u8], outcome: &mut Outcome) {
        if let Some(reason) = unfit(archive, ids.len()) {
            coop_log!("refused to create {} objects: {reason}", ids.len());
            outcome.drifted = true;
            return;
        }
        let mut result = Ok(());
        // Nested: only this load's objects. Imported, they would replace the selection.
        let created = changes::applying(|| {
            result = vehicle_changes::keeping_selection(|| {
                // SAFETY: the component is the live editor's; game thread.
                unsafe { self.game.archive.load(self.component, archive, true, true) }
            });
        });
        if let Err(reason) = result {
            coop_log!("couldn't create {} objects: {reason}", ids.len());
            outcome.drifted = true;
            return;
        }
        // SAFETY: the objects the load just made; game thread.
        let made = unsafe { made_of(archive, &created, ids) };
        // SAFETY: as above.
        unsafe { adopt(self.game, &created, &ids_of(ids, made.as_deref())) };
        // Their groups are numbers the loader picked here: the author's follow as ops.
        self.regroup_all(&created, &vec![[0; 2]; created.len()]);
        if created.len() != ids.len() {
            coop_log!("creating {} objects made {}", ids.len(), created.len());
            log_classes(archive);
        }
        outcome.drifted |= made.is_none();
        outcome.skipped.extend(skipped_of(ids, made.as_deref()));
    }

    /// Moves the brick with coop id `id`; false if it couldn't (unknown here, or the editor's
    /// places not found yet).
    fn place(&self, id: CoopId, transform: &Transform) -> bool {
        let Some(object) = changes::object(id) else {
            // Deleted here meanwhile by an edit the session ordered first: nothing to move.
            return true;
        };
        let mut placed = false;
        // SAFETY: the editor is live and the object is one of its live objects (it has a coop
        // id); game thread. The nested operation joins ours, adding the move flags.
        unsafe {
            self.game.spawn.learn([object]);
            self.game.editor.operation(self.editor, MOVED, || {
                placed = self
                    .game
                    .spawn
                    .set(object, transform.location, transform.rotation);
            });
        }
        if placed {
            changes::place(id, *transform);
        } else {
            coop_log!("couldn't move a brick: where the editor keeps places isn't known yet");
        }
        placed
    }

    /// Gives the brick with coop id `id` the value `value` of its property `property`; false if
    /// this game refuses it (logged). Nothing to do for an unknown id.
    fn set(&self, id: CoopId, property: &str, value: &property_values::Value) -> bool {
        let Some(object) = changes::object(id) else {
            // Deleted here meanwhile by an edit the session ordered first.
            return true;
        };
        if let property_values::Value::Data(bytes) = value
            // SAFETY: a live brick of the editor (it has a coop id); game thread.
            && let Some(reason) = unsafe { self.past_limits(object, property, bytes) }
        {
            coop_log!("refused a property value: {property}: {reason}");
            return false;
        }
        if property == "OwningSeat"
            && let property_values::Value::Bricks(bricks) = value
            && !self.camera_seat(bricks)
        {
            coop_log!("refused a property value: {property}: not a seat");
            return false;
        }
        if let Some(channel) = property.strip_suffix(".SourceBricks")
            && let property_values::Value::Bricks(bricks) = value
            // SAFETY: a live brick of the editor (it has a coop id), the bricks live (just
            // looked up by coop id); game thread.
            && let Some(reason) = unsafe { self.source_refusal(object, channel, bricks) }
        {
            coop_log!("refused a property value: {property}: {reason}");
            return false;
        }
        let mut result = Ok(());
        // SAFETY: the editor is live and the object is one of its live objects (it has a coop
        // id); game thread. The nested operation joins ours, adding the property flag.
        unsafe {
            self.game
                .editor
                .operation(self.editor, PROPERTY_CHANGED, || {
                    result = self.game.values.set(object, property, value);
                });
        }
        if let Err(reason) = &result {
            coop_log!("refused a property value: {reason}");
        }
        result.is_ok()
    }

    /// Why `bytes`, a value of `object`'s property `property`, is past what any panel lets a
    /// player type, if it is: a float changed to past ±[`property_limits::UNLIMITED`] (past which
    /// the game's own sums overflow; a light that bright crashed it). Fields it doesn't change
    /// aren't checked, nor the rules' narrower limits: values loaded from files may be past those
    /// and must keep working, also when an undo brings them back. Every member refuses it alike,
    /// so only its sender's editor differs, which its audits then mend.
    ///
    /// # Safety
    /// `object` must be a live brick of the editor. Game thread only.
    unsafe fn past_limits(&self, object: Object, property: &str, bytes: &[u8]) -> Option<String> {
        let unlimited = f64::from(property_limits::UNLIMITED);
        // SAFETY: per the contract.
        unsafe { self.game.values.changed_floats(object, property, bytes) }
            .into_iter()
            .find(|f| f.abs() > unlimited)
            .map(|far| format!("{far} is past ±{unlimited}"))
    }

    /// Whether the panel could reorder the bricks so: `ids` first, the others after them in the
    /// order they have (`coop::order::reordered`), but for the bricks `made` (by the same edit),
    /// which may go anywhere. Logs why not.
    fn panel_reorders(&self, ids: &[CoopId], made: &[CoopId]) -> bool {
        let checks = match &self.game.checks {
            Ok(checks) => checks,
            Err(reason) => {
                coop_log!("refused a reorder: {reason}");
                return false;
            }
        };
        // SAFETY: the editor is live (it's ticking); copied at once.
        let current = unsafe { self.game.bricks.editor_objects(self.editor) }.to_vec();
        if current.iter().any(|object| object.is_null()) {
            coop_log!("refused a reorder: the editor's object list has gaps");
            return false;
        }
        let listed: Vec<Object> = ids.iter().filter_map(|&id| changes::object(id)).collect();
        let target = ::coop::order::reordered(&current, &listed);
        let made: Vec<Object> = made.iter().filter_map(|&id| changes::object(id)).collect();
        let old = |objects: Vec<Object>| -> Vec<Object> {
            objects.into_iter().filter(|o| !made.contains(o)).collect()
        };
        // SAFETY: the editor's live objects (none null, checked above); game thread.
        let could = unsafe { checks.reorders(&old(current), &old(target)) };
        if !could {
            coop_log!("refused a reorder no panel makes (only seats or cameras among themselves)");
        }
        could
    }

    /// Why the panel couldn't give `object`'s input channel `channel` (a property of it) the
    /// sources `bricks`, if so (`game::input_sources`, the game's own check). An unknown channel
    /// is left to the write, which refuses it.
    ///
    /// # Safety
    /// `object` must be a live brick of the editor and `bricks` live objects. Game thread only.
    unsafe fn source_refusal(
        &self,
        object: Object,
        channel: &str,
        bricks: &[Object],
    ) -> Option<String> {
        let sources = match &self.game.sources {
            Ok(sources) => sources,
            Err(reason) => return Some(reason.clone()),
        };
        // SAFETY: per the contract; the channel is a struct property of the brick's class, so it
        // lies within the brick.
        unsafe {
            let class = reflection::class_of(object.cast());
            let field = reflection::properties(class)
                .find(|&field| reflection::name_is(&(*field).field.name_private, channel))?;
            let kind = reflection::struct_type(field);
            if kind.is_null()
                || !reflection::name_is(reflection::struct_name(kind), "VehicleInputChannel")
            {
                return Some("not an input channel".into());
            }
            let at = object.cast::<u8>().add((*field).offset_internal as usize);
            sources.refusal(at, bricks)
        }
    }

    /// Whether `bricks` may be a camera's seat (`checks`).
    fn camera_seat(&self, bricks: &[Object]) -> bool {
        match &self.game.checks {
            // SAFETY: bricks of the editor, live (just looked up by coop id); game thread.
            Ok(checks) => unsafe { checks.camera_seat(bricks) },
            Err(_) => false,
        }
    }

    /// Puts the bricks with coop ids `ids` first in the object order, in that order, the others
    /// after them in the order they had (`coop::order::reordered`). False if the order didn't
    /// come out so (logged).
    fn reorder(&self, order: &ObjectOrder, ids: &[CoopId]) -> bool {
        // SAFETY: the editor is live (it's ticking); copied at once.
        let current = unsafe { self.game.bricks.editor_objects(self.editor) }.to_vec();
        if current.iter().any(|object| object.is_null()) {
            // The game's reorder reads every object's class.
            coop_log!("couldn't reorder: the editor's object list has gaps");
            return false;
        }
        let listed: Vec<Object> = ids.iter().filter_map(|&id| changes::object(id)).collect();
        let target = ::coop::order::reordered(&current, &listed);
        let moves = ::coop::order::moves(&current, &target);
        if moves.is_empty() {
            return true;
        }
        let mut refused = 0;
        // SAFETY: the editor is live, and its interface component holds the objects, none null
        // (checked above; moves only shuffle them); game thread. The nested operation joins ours,
        // adding the reorder flag as the panel does.
        unsafe {
            self.game.editor.operation(self.editor, ORDER_CHANGED, || {
                for &(object, index) in &moves {
                    refused += usize::from(!order.move_to(self.component, object, index));
                }
            });
        }
        // SAFETY: as above.
        let reached = unsafe { self.game.bricks.editor_objects(self.editor) } == target;
        if !reached {
            coop_log!(
                "reordering by {} moves didn't give the session's order ({refused} refused)",
                moves.len()
            );
        }
        reached
    }

    /// Puts the brick with coop id `id` in group `group` of `kind`; false if it can't be in one
    /// (logged). Nothing to do for an unknown id.
    fn group(&self, groups: &BrickGroups, id: CoopId, kind: GroupKind, group: u16) -> bool {
        let Some(object) = changes::object(id) else {
            // Deleted here meanwhile by an edit the session ordered first.
            return true;
        };
        let mut written = false;
        // SAFETY: the editor is live and the object is one of its live objects (it has a coop
        // id); game thread. The nested operation joins ours, adding the groups flag.
        unsafe {
            self.game.editor.operation(self.editor, GROUPS_CHANGED, || {
                written = groups.write(object, kind as usize, group);
            });
        }
        if !written {
            coop_log!("couldn't group an object: it has no editor parameters");
        }
        written
    }

    /// Puts `objects` (live, just made) in the groups `all` lists for them, in order, if where
    /// groups are kept is known. Inside an operation.
    fn regroup_all(&self, objects: &[Object], all: &[Groups]) {
        let Ok(groups) = &self.game.groups else {
            return;
        };
        // SAFETY: the editor is live and the objects are its live objects; game thread. The nested
        // operation joins the caller's, adding the groups flag.
        unsafe {
            self.game.editor.operation(self.editor, GROUPS_CHANGED, || {
                for (&object, now) in objects.iter().zip(all) {
                    for kind in GroupKind::ALL {
                        groups.write(object, kind as usize, now[kind as usize]);
                    }
                }
            });
        }
    }

    /// Deletes `doomed` (their coop ids already forgotten), if any.
    fn destroy(&self, doomed: &mut Vec<Object>) {
        if !doomed.is_empty() {
            // SAFETY: they're the editor's live objects (each still had its coop id); game thread.
            unsafe { self.game.archive.destroy(self.component, doomed) };
            doomed.clear();
        }
    }
}

/// Why `archive` mustn't reach the game's loader as `count` objects, if so: read with our own
/// reader first (`coop::archive`), it must be a vehicle file the game's current loader takes,
/// of exactly that many objects.
fn unfit(archive: &[u8], count: usize) -> Option<String> {
    match ::coop::archive::read_header(archive) {
        Err(error) => Some(error.to_string()),
        Ok(header) if header.object_count() != count => {
            Some(format!("{} objects for {count} ids", header.object_count()))
        }
        Ok(_) => None,
    }
}

/// Which of `archive`'s objects, whose ids are `ids`, a load of it made as `created` (the game's
/// loader skips classes it doesn't have, such as a brick of a mod not installed here); `None` if
/// that can't be told.
///
/// # Safety
/// `created` must be live objects. Game thread only.
unsafe fn made_of(archive: &[u8], created: &[Object], ids: &[CoopId]) -> Option<Vec<usize>> {
    if created.len() == ids.len() {
        return Some((0..ids.len()).collect());
    }
    let header = ::coop::archive::read_header(archive).ok()?;
    let classes: Vec<String> = created
        .iter()
        // SAFETY: per the contract. A live object's class is a live class with a name.
        .map(|&object| unsafe {
            reflection::name_text(reflection::struct_name(reflection::class_of(object.cast())))
        })
        .collect();
    header.made_objects(&classes)
}

/// The ids of the objects made: `made` of `ids` (`None`, it can't be told: in order, as far as they
/// go, and the session's vehicle is asked for).
fn ids_of(ids: &[CoopId], made: Option<&[usize]>) -> Vec<CoopId> {
    match made {
        Some(made) => made.iter().map(|&object| ids[object]).collect(),
        None => ids.to_vec(),
    }
}

/// The ids of the objects not made.
fn skipped_of(ids: &[CoopId], made: Option<&[usize]>) -> Vec<CoopId> {
    let Some(made) = made else {
        return Vec::new();
    };
    let made: HashSet<usize> = made.iter().copied().collect();
    ids.iter()
        .enumerate()
        .filter(|(object, _)| !made.contains(object))
        .map(|(_, &id)| id)
        .collect()
}

/// Logs what classes `archive` asks for, after the game's loader skipped some.
fn log_classes(archive: &[u8]) {
    let classes = match ::coop::archive::read_header(archive) {
        Ok(header) => header.class_names.join(", "),
        Err(error) => error.to_string(),
    };
    coop_log!("  the game here lacks some of these classes: {classes}");
}

/// A place's location to a tenth of a centimetre, to find the bricks that may be there.
fn rounded(place: &Transform) -> [i64; 3] {
    place.location.map(|cm| (cm * 10.0).round() as i64)
}

/// Whether two places are one, but for float rounding (a vehicle saved and loaded again).
fn same_place(a: &Transform, b: &Transform) -> bool {
    let close = |a: [f32; 3], b: [f32; 3], within: f32| {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() <= within)
    };
    close(a.location, b.location, 0.05) && close(a.rotation, b.rotation, 0.05)
}
