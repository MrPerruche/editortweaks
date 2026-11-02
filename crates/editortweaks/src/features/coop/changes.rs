//! The vehicle as coop knows it: each editor object's coop id, and the local edits the game's
//! hooks saw (`game::vehicle_changes`), until the session takes them.
//!
//! The hooks fire inside the game's own code, also while coop applies a remote edit, so this state
//! is kept apart from the session's (which is borrowed then) and only borrowed briefly.
//!
//! Which edits are final: objects created for a load, an import or a mirror are, at the end of
//! their operation. New bricks and duplicates are still being placed (they follow the mouse) until
//! a click commits the move (an operation with the move flag), or Escape deletes them again: they
//! are only sent when put down.
//!
//! Moves and properties alike: the place and the syncable property values of every brick with a
//! coop id are remembered; after an operation that commits a move or changes a property, the
//! bricks whose place or values differ are the changed ones (the editor doesn't say which). The
//! same for the object order (seat order, camera order, ...): the coop ids in object order are
//! remembered, and compared after an operation that reorders. And each brick's groups, compared
//! after an operation that changes groups. What was remembered is what undoes the change.
//!
//! Bricks deleted by a local edit are saved as they go (`super::bury`), for undoing the delete.
//! The game takes the player's undo and redo to coop while it watches an editor (see
//! [`step_back_asked`]).

use std::cell::RefCell;
use std::collections::HashMap;

use ::coop::{CoopId, Groups, Op, Transform};
use brickrust::ue::coreuobject::UObjectBase;

use crate::game::brick_editor::PROPERTY_CHANGED;
use crate::game::brick_groups::GROUPS_CHANGED;
use crate::game::object_order::ORDER_CHANGED;
use crate::game::vehicle_changes;

pub type Object = *mut UObjectBase;

/// The flag of an operation that commits a move (and so puts down placed bricks): `EndMode` sets
/// it, and nothing else in the editor does (`docs/re/coop/change-hooks.md`, section 5).
const MOVE_COMMITTED: u32 = 0x10;

/// Bricks a local edit deleted, as they were just before: what undoing the delete takes.
pub struct Buried {
    pub ids: Vec<CoopId>,
    /// The ops making them again as they were, with the same ids (none if they couldn't be saved).
    pub undo: Vec<Op>,
}

/// A local edit, final: what it created and deleted, and whether it moved bricks.
pub struct Edit {
    /// Live objects, without coop ids yet.
    pub created: Vec<Object>,
    pub deleted: Vec<CoopId>,
    /// The deleted bricks, by the game's deletes (each of `deleted` is in one).
    pub buried: Vec<Buried>,
    /// Some bricks may have moved: compare places (see [`moved`]).
    pub moved: bool,
    /// Some bricks' properties may have changed: compare values (see [`values_changed`]).
    pub properties: bool,
    /// The object order may have changed: compare it (see [`reordered`]).
    pub ordered: bool,
    /// Some bricks' groups may have changed: compare them (see [`groups_changed`]).
    pub grouped: bool,
}

#[derive(Clone, Copy)]
struct Watched {
    editor: Object,
    component: Object,
}

#[derive(Default)]
struct Changes {
    /// Only this editor (and its interface component) is watched; nothing while `None`.
    watched: Option<Watched>,
    ids: HashMap<Object, CoopId>,
    objects: HashMap<CoopId, Object>,
    /// Where each brick with a coop id was last seen or put.
    places: HashMap<CoopId, Transform>,
    /// Each brick's syncable property values, as last seen or set
    /// (`property_values::ClassValues::read`).
    values: HashMap<CoopId, Vec<u8>>,
    /// Each brick's groups, as last seen or set.
    groups: HashMap<CoopId, Groups>,
    /// The coop ids in object order, as last seen or set; new ones appended (where the editor
    /// puts them), deleted ones not taken out.
    order: Vec<CoopId>,
    /// Created, still being placed.
    placing: Vec<Object>,
    /// Created and final, deleted: waiting for their operation to end.
    created: Vec<Object>,
    deleted: Vec<CoopId>,
    buried: Vec<Buried>,
    /// The coop ids of the objects deleted since the edits were last sent, to name bricks that
    /// property values held before (to undo them).
    graves: HashMap<Object, CoopId>,
    finished: Vec<Edit>,
    /// Undo (false) and redo (true) asked for by the player, not made yet.
    step_backs: Vec<bool>,
    /// While coop applies a remote edit: what's created is recorded in `applied`, and nothing is
    /// an edit of ours.
    applying: bool,
    applied: Vec<Object>,
}

thread_local! {
    // The game thread's: the editor and its hooks run there.
    static CHANGES: RefCell<Changes> = RefCell::new(Changes::default());
}

fn with<R>(f: impl FnOnce(&mut Changes) -> R) -> R {
    CHANGES.with_borrow_mut(f)
}

/// Starts watching `editor` (with its interface component), knowing none of its objects yet.
pub fn watch(editor: Object, component: Object) {
    with(|changes| {
        *changes = Changes {
            watched: Some(Watched { editor, component }),
            ..Changes::default()
        }
    });
}

/// Stops watching and forgets everything.
pub fn unwatch() {
    with(|changes| *changes = Changes::default());
}

/// Stops watching the editor (the player left it: its objects are gone), but remembers every
/// brick, to find them again in the next editor ([`remembered`]). Nothing that names its objects
/// may be used until then.
pub fn detach() {
    with(|changes| changes.watched = None);
}

/// Watches `editor` (with its interface component) again, after [`detach`]: the same editor, which
/// only paused, its objects as they were.
pub fn rewatch(editor: Object, component: Object) {
    with(|changes| changes.watched = Some(Watched { editor, component }));
}

/// A brick as coop last knew it.
pub struct Known {
    pub id: CoopId,
    pub place: Option<Transform>,
    pub groups: Groups,
}

/// Every brick with a coop id as last remembered, in object order.
pub fn remembered() -> Vec<Known> {
    with(|changes| {
        changes
            .order
            .iter()
            .filter(|id| changes.objects.contains_key(id))
            .map(|&id| Known {
                id,
                place: changes.places.get(&id).copied(),
                groups: changes.groups.get(&id).copied().unwrap_or_default(),
            })
            .collect()
    })
}

/// Gives `objects` the coop `ids`, in order (their places: [`place`]). They're new, so last in
/// the object order.
pub fn assign(objects: &[Object], ids: &[CoopId]) {
    with(|changes| {
        for (&object, &id) in objects.iter().zip(ids) {
            changes.ids.insert(object, id);
            changes.objects.insert(id, object);
            changes.order.push(id);
        }
    });
}

/// Forgets every object's coop id (before the vehicle is replaced).
pub fn forget_all() {
    with(|changes| {
        changes.ids.clear();
        changes.objects.clear();
        changes.places.clear();
        changes.values.clear();
        changes.groups.clear();
        changes.order.clear();
        changes.placing.clear();
        changes.created.clear();
        changes.deleted.clear();
        changes.buried.clear();
        changes.graves.clear();
        changes.finished.clear();
        changes.step_backs.clear();
    });
}

pub fn id_of(object: Object) -> Option<CoopId> {
    with(|changes| changes.ids.get(&object).copied())
}

/// The coop id `object` had, if it's one deleted by a local edit not sent yet (only compared, never
/// followed: it's gone).
pub fn id_of_buried(object: Object) -> Option<CoopId> {
    with(|changes| changes.graves.get(&object).copied())
}

/// Forgets the deleted objects' ids (once their edits are sent).
pub fn forget_graves() {
    with(|changes| changes.graves.clear());
}

/// The object with coop id `id`.
pub fn object(id: CoopId) -> Option<Object> {
    with(|changes| changes.objects.get(&id).copied())
}

/// The object with coop id `id`, which forgets it (it's about to be deleted).
pub fn take_object(id: CoopId) -> Option<Object> {
    with(|changes| {
        let object = changes.objects.remove(&id)?;
        changes.ids.remove(&object);
        changes.places.remove(&id);
        changes.values.remove(&id);
        changes.groups.remove(&id);
        Some(object)
    })
}

/// Every object with a coop id (all live: deleted ones are forgotten as they go).
pub fn known() -> Vec<(Object, CoopId)> {
    with(|changes| changes.ids.iter().map(|(&o, &id)| (o, id)).collect())
}

/// Remembers where the brick with coop id `id` is now (it was made, or coop moved it).
pub fn place(id: CoopId, transform: Transform) {
    with(|changes| {
        changes.places.insert(id, transform);
    });
}

/// Where the brick with coop id `id` was if it moved since its place was last remembered
/// (`Some(None)` if that wasn't known), remembering `now`. Exact: any change counts, as places
/// travel bit for bit.
pub fn moved(id: CoopId, now: Transform) -> Option<Option<Transform>> {
    with(|changes| {
        let was = changes.places.insert(id, now);
        (was != Some(now)).then_some(was)
    })
}

/// Remembers the property values the brick with coop id `id` has now (it was made, or coop set
/// them).
pub fn remember_values(id: CoopId, values: Vec<u8>) {
    with(|changes| {
        changes.values.insert(id, values);
    });
}

/// The values the brick with coop id `id` had if they differ from `now`, remembering `now`.
pub fn values_changed(id: CoopId, now: Vec<u8>) -> Option<Vec<u8>> {
    with(|changes| match changes.values.insert(id, now) {
        Some(was) if was == changes.values[&id] => None,
        was => Some(was.unwrap_or_default()),
    })
}

/// Remembers the groups the brick with coop id `id` is in now (it was made, or coop set them).
pub fn remember_groups(id: CoopId, groups: Groups) {
    with(|changes| {
        changes.groups.insert(id, groups);
    });
}

/// The groups the brick with coop id `id` was in if they differ from `now`, remembering `now`.
pub fn groups_changed(id: CoopId, now: Groups) -> Option<Groups> {
    with(|changes| {
        let was = changes.groups.insert(id, now).unwrap_or_default();
        (was != now).then_some(was)
    })
}

/// Remembers `objects` (the editor's, in its order) as the object order now (coop loaded or
/// changed the vehicle). Those without a coop id are left out.
pub fn remember_order(objects: &[Object]) {
    with(|changes| {
        changes.order = objects
            .iter()
            .filter_map(|object| changes.ids.get(object).copied())
            .collect();
    });
}

/// The coop ids in object order, as last remembered (with bricks deleted since).
pub fn order() -> Vec<CoopId> {
    with(|changes| changes.order.clone())
}

/// The order list (`Op::Order`) that brings the objects with coop ids from their last remembered
/// order into the order of `objects` (the editor's, in its order), if they were reordered, and the
/// one bringing them back; remembering the new order.
pub fn reordered(objects: &[Object]) -> Option<(Vec<CoopId>, Vec<CoopId>)> {
    with(|changes| {
        let now: Vec<CoopId> = objects
            .iter()
            .filter_map(|object| changes.ids.get(object).copied())
            .collect();
        let was = std::mem::replace(&mut changes.order, now);
        let list = ::coop::order::change(&was, &changes.order)?;
        let back = ::coop::order::change(&changes.order, &was)?;
        Some((list, back))
    })
}

/// The finished local edits, oldest first.
pub fn take_edits() -> Vec<Edit> {
    with(|changes| std::mem::take(&mut changes.finished))
}

/// `undo_redo::take_over`'s callback: the player asked to undo (or redo) in `editor`. Coop makes
/// it if it watches that editor (see [`take_step_backs`]); the game's own mustn't run then.
pub fn step_back_asked(editor: Object, redo: bool) -> bool {
    with(|changes| {
        let ours = changes.watched.is_some_and(|w| w.editor == editor);
        if ours {
            changes.step_backs.push(redo);
        }
        ours
    })
}

/// The undos (false) and redos (true) the player asked for, oldest first.
pub fn take_step_backs() -> Vec<bool> {
    with(|changes| std::mem::take(&mut changes.step_backs))
}

/// Runs `apply` as coop applying a remote edit: its creations aren't local edits, and come back
/// here, in order. May be nested: each gets its own creations.
pub fn applying(apply: impl FnOnce()) -> Vec<Object> {
    let (was_applying, outer) = with(|changes| {
        (
            std::mem::replace(&mut changes.applying, true),
            std::mem::take(&mut changes.applied),
        )
    });
    apply();
    with(|changes| {
        changes.applying = was_applying;
        std::mem::replace(&mut changes.applied, outer)
    })
}

/// `vehicle_changes::on_created`'s callback.
pub fn created(editor: Object, objects: &[Object], context: u8) {
    with(|changes| {
        if changes.watched.is_none_or(|w| w.editor != editor) {
            return;
        }
        if changes.applying {
            changes.applied.extend_from_slice(objects);
        } else if vehicle_changes::is_placing(context) {
            changes.placing.extend_from_slice(objects);
        } else {
            changes.created.extend_from_slice(objects);
        }
    });
}

/// `vehicle_changes::on_destroyed`'s callback: objects still live, about to go.
pub fn destroyed(component: Object, objects: &[Object]) {
    let ours: Vec<(Object, CoopId)> = with(|changes| {
        if changes.watched.is_none_or(|w| w.component != component) || changes.applying {
            // Coop's own deletes forget their ids themselves.
            return Vec::new();
        }
        for object in objects {
            // Not sent yet: dropped from what would be (a brick being placed, cancelled).
            changes.placing.retain(|o| o != object);
            changes.created.retain(|o| o != object);
            for edit in &mut changes.finished {
                edit.created.retain(|o| o != object);
            }
        }
        objects
            .iter()
            .filter_map(|&object| Some((object, *changes.ids.get(&object)?)))
            .collect()
    });
    if ours.is_empty() {
        return;
    }
    // Saved while they're whole, with every coop id still known.
    let buried = super::bury(component, &ours);
    with(|changes| {
        for &(object, id) in &ours {
            changes.ids.remove(&object);
            changes.objects.remove(&id);
            changes.places.remove(&id);
            changes.values.remove(&id);
            changes.groups.remove(&id);
            changes.graves.insert(object, id);
            changes.deleted.push(id);
        }
        changes.buried.push(buried);
    });
}

/// `BrickEditor::after_modify`'s callback: an operation ended, with `flags` (what changed).
pub fn modified(flags: u32) {
    with(|changes| {
        if changes.watched.is_none() || changes.applying {
            return;
        }
        let moved = flags & MOVE_COMMITTED != 0;
        let properties = flags & PROPERTY_CHANGED != 0;
        let ordered = flags & ORDER_CHANGED != 0;
        let grouped = flags & GROUPS_CHANGED != 0;
        if moved {
            let placed = std::mem::take(&mut changes.placing);
            changes.created.extend(placed);
        }
        if moved
            || properties
            || ordered
            || grouped
            || !changes.created.is_empty()
            || !changes.deleted.is_empty()
        {
            let edit = Edit {
                created: std::mem::take(&mut changes.created),
                deleted: std::mem::take(&mut changes.deleted),
                buried: std::mem::take(&mut changes.buried),
                moved,
                properties,
                ordered,
                grouped,
            };
            changes.finished.push(edit);
        }
    });
}
