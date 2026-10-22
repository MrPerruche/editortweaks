//! The "Connections" view mode: bricks colored by the part of the vehicle they hold together with,
//! like the game's group view modes color groups; bricks connected to nothing gray. A vehicle cut
//! in two shows two colors, one half built from unconnected pieces shows many.
//!
//! How: the connections are Strength's (`game::brick_connections`, what L uses to select attached
//! bricks), and welded bricks hold together too, connected or not. Parts are colored biggest first,
//! plain colors before the game's two-color patterns (see [`colors`]). Hidden bricks (H) count as
//! not there, as for L: hide bricks to see what falls apart once they're shot off.
//!
//! The tooltip ([`Coloring::describe`]) tells the size of the part under the mouse, and how many
//! parts and loose bricks the vehicle has.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use super::tooltip::Hover;
use super::{Coloring, Colors, Editor, Painting};
use crate::game::brick_connections::{self, Connections};

/// The parts' colors, as the game's group view modes make a group's (see
/// `docs/re/editor-view-modes.md`): each of the two colors' red, green and blue is [`LOW`] or
/// [`HIGH`], picked by a bit of the group's number (1 to 63). Free to change.
const LOW: f32 = 0.2;
const HIGH: f32 = 1.0;

/// The group numbers whose two colors are the same (plain): given to the biggest parts first, in
/// this order (blue, red, green, yellow, cyan, magenta, white).
const PLAIN_FIRST: [u8; 7] = [36, 9, 18, 27, 54, 45, 63];

/// The colors of the `part`th biggest part (from 0): the plain ones, then the other 56 of the
/// game's group colors, then again.
fn colors(part: usize) -> Colors {
    let patterned = (1..64).filter(|group| !PLAIN_FIRST.contains(group));
    let groups: Vec<u8> = PLAIN_FIRST.into_iter().chain(patterned).collect();
    let group = groups[part % groups.len()];
    let channel = |bit: u8| if group & (1 << bit) != 0 { HIGH } else { LOW };
    [
        [channel(3), channel(4), channel(5)],
        [channel(0), channel(1), channel(2)],
    ]
}

struct Parts {
    connections: &'static Connections,
}

/// What the view mode found for the editor as it is, for the tooltip: replaced by each painting.
static FOUND: Mutex<Option<Found>> = Mutex::new(None);

struct Found {
    /// The part each brick holding together with others (by address) is in, from 0, biggest first.
    part: HashMap<usize, usize>,
    /// Each part's bricks.
    sizes: Vec<usize>,
    /// Bricks shown connected to nothing.
    loose: usize,
}

pub fn new() -> Result<Box<dyn Coloring>, String> {
    Ok(Box::new(Parts {
        connections: brick_connections::find()?,
    }))
}

impl Coloring for Parts {
    unsafe fn paint(&self, editor: &Editor) -> Painting {
        // SAFETY: per the contract; the editor's bricks are live, and so are the bricks the finder
        // reports (the editor's own).
        let found = unsafe { self.find(editor) };
        let summary = format!(
            "{} parts (the biggest of {} bricks), {} loose bricks",
            found.sizes.len(),
            found.sizes.first().copied().unwrap_or(0),
            found.loose,
        );
        let colors = found
            .part
            .iter()
            .map(|(&brick, &part)| (brick, colors(part)))
            .collect();
        *FOUND.lock().unwrap_or_else(PoisonError::into_inner) = Some(found);
        Painting { colors, summary }
    }

    unsafe fn describe(&self, editor: &Editor, hover: &Hover) -> Vec<String> {
        // SAFETY: per the contract.
        let Some(point) = (unsafe { hover.point(editor) }) else {
            return Vec::new();
        };
        let found = FOUND.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(found) = found.as_ref() else {
            return Vec::new();
        };
        let this = match found.part.get(&(point.brick as usize)) {
            Some(&part) => format!("This part: {}", bricks(found.sizes[part])),
            None => "This brick: not connected".to_owned(),
        };
        let parts = match found.sizes.len() {
            1 => "1 part".to_owned(),
            count => format!("{count} parts"),
        };
        let vehicle = match found.loose {
            0 => format!("Vehicle: {parts}"),
            loose => format!("Vehicle: {parts}, {} loose", bricks(loose)),
        };
        vec![this, vehicle]
    }
}

impl Parts {
    /// The parts of `editor`'s bricks not hidden: bricks connected, directly or through others, or
    /// welded together.
    ///
    /// # Safety
    /// `editor.editor` must be a live vehicle editor. Game thread only.
    unsafe fn find(&self, editor: &Editor) -> Found {
        // SAFETY: per the contract; the editor's bricks are live, and the finder reports them.
        let (bricks, connections) = unsafe {
            let bricks: Vec<_> = editor
                .brick_list()
                .into_iter()
                .filter(|&brick| !editor.is_hidden(brick))
                .collect();
            (bricks, self.connections.find_all(editor.editor))
        };
        let index: HashMap<usize, usize> = bricks
            .iter()
            .enumerate()
            .map(|(index, &brick)| (brick as usize, index))
            .collect();
        let mut sets = Sets::new(bricks.len());
        for connection in connections {
            let ends =
                [connection.brick, connection.other].map(|brick| index.get(&(brick as usize)));
            if let [Some(&brick), Some(&other)] = ends {
                sets.join(brick, other);
            }
        }
        let mut welded: HashMap<u16, usize> = HashMap::new();
        for (brick, &object) in bricks.iter().enumerate() {
            // SAFETY: a live brick (see above).
            let group = unsafe { self.connections.weld_group(object) };
            if group != 0 {
                let first = *welded.entry(group).or_insert(brick);
                sets.join(first, brick);
            }
        }
        // Each part's bricks, in the editor's order.
        let mut parts: HashMap<usize, Vec<usize>> = HashMap::new();
        for brick in 0..bricks.len() {
            parts.entry(sets.root(brick)).or_default().push(brick);
        }
        let (mut parts, loose): (Vec<_>, Vec<_>) =
            parts.into_values().partition(|part| part.len() > 1);
        // Biggest first; the same size by their first brick, so colors stay put across edits.
        parts.sort_by_key(|part| (core::cmp::Reverse(part.len()), part[0]));
        let mut part_of = HashMap::new();
        for (number, part) in parts.iter().enumerate() {
            part_of.extend(part.iter().map(|&brick| (bricks[brick] as usize, number)));
        }
        Found {
            part: part_of,
            sizes: parts.iter().map(Vec::len).collect(),
            loose: loose.len(),
        }
    }
}

/// `count` bricks, for people.
fn bricks(count: usize) -> String {
    match count {
        1 => "1 brick".to_owned(),
        count => format!("{count} bricks"),
    }
}

/// Bricks (by index) joined into sets, each named by one of its bricks (union-find).
struct Sets {
    /// Each brick's link towards its set's name; the name links to itself.
    parent: Vec<usize>,
}

impl Sets {
    /// `count` bricks, each its own set.
    fn new(count: usize) -> Self {
        Self {
            parent: (0..count).collect(),
        }
    }

    /// The name of `brick`'s set.
    fn root(&mut self, brick: usize) -> usize {
        let mut root = brick;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        // Shortens the way for next time.
        let mut at = brick;
        while self.parent[at] != root {
            at = core::mem::replace(&mut self.parent[at], root);
        }
        root
    }

    /// Joins the sets of `brick` and `other`.
    fn join(&mut self, brick: usize, other: usize) {
        let (brick, other) = (self.root(brick), self.root(other));
        self.parent[brick] = other;
    }
}
