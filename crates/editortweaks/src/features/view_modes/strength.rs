//! The "Strength" view mode: bricks shaded by how hard they are to break off, pale for the weakest
//! in the vehicle, deep blue for the strongest, on a log scale (see [`shade`]); bricks connected to
//! nothing gray.
//!
//! How hard is the game's own measure: a brick takes no damage until a hit deals at least the sum
//! of its connections' strengths, then breaks off (with the rest of the damage going on to its
//! neighbors). Welded bricks are one body: they share the sum of their group's connections. A
//! connection's strength grows with the weaker of the two bricks' materials (`Strength` of their
//! physical materials), the kind of connectors and how many points connect them (studs, at least
//! 9); see `game::brick_connections`.
//!
//! How: the editor's own connection finder (what L uses to select attached bricks) reports every
//! connector pair; pairs of the same two bricks merge as a spawning vehicle merges them (the first
//! kind found stays, more of it adds points), and the game computes each connection's strength.
//! Hidden bricks (H) count as not there, as for L: hide bricks to see how the rest holds once
//! they're shot off. Axles, actuators and the like connect through their own kind of connection in
//! a vehicle: counted here like the others.
//!
//! The tooltip ([`Coloring::describe`]) tells the damage that breaks off the brick under the mouse;
//! if it's selected, the selection's strength: that of all the connections between it and the
//! other bricks; and the strength to pierce through the vehicle along that line: the armor a shot
//! meets (`game::penetration`), the thickness of each brick it crosses times its material's
//! density, added up. Damage and armor are also told in AP, the players' unit: a shot of a Ballistic
//! 2x2x2 gun with two barrels ([`AP_DAMAGE`], [`AP_PENETRATION`]).

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;

use super::tooltip::{Hover, number};
use super::{Coloring, Editor, Painting, blend};
use crate::game::brick_connections::{self, Connections};
use crate::game::editor_object::{self, Selection};
use crate::game::penetration::{self, Densities};
use crate::prelude::*;

/// The colors, free to change (see `Colors`): the weakest bricks in the vehicle and the strongest;
/// [`shade`] blends between them.
const WEAKEST: [f32; 3] = [0.6, 0.75, 1.0];
const STRONGEST: [f32; 3] = [0.0, 0.02, 0.35];

/// The color of a brick needing `damage` to break off, when the vehicle's bricks need from `weakest`
/// to `strongest` (all above 0). Free to change.
fn shade(damage: f32, weakest: f32, strongest: f32) -> [f32; 3] {
    // How far from weakest to strongest, on a log scale: twice as strong is always the same step.
    let share = if strongest > weakest {
        (damage / weakest).ln() / (strongest / weakest).ln()
    } else {
        1.0
    };
    blend(WEAKEST, STRONGEST, share.clamp(0.0, 1.0))
}

struct Strength {
    connections: &'static Connections,
    /// Without it, the tooltip tells no armor (logged).
    densities: Option<Densities>,
    /// Without it, the tooltip tells nothing of the selection (logged).
    selection: Option<Selection>,
}

/// What the view mode found for the editor as it is, for the tooltip: replaced by each painting.
static FOUND: Mutex<Option<Found>> = Mutex::new(None);

struct Found {
    /// The damage each connected brick (by address) needs to break off.
    damage: HashMap<usize, f32>,
    /// Each pair of connected bricks (by address), with the strength of its connection.
    pairs: Vec<(usize, usize, f32)>,
}

pub fn new() -> Result<Box<dyn Coloring>, String> {
    Ok(Box::new(Strength {
        connections: brick_connections::find()?,
        densities: penetration::find()
            .map_err(|reason| log!("the Strength tooltip tells no armor: {reason}"))
            .ok(),
        selection: editor_object::selection()
            .map_err(|reason| log!("the Strength tooltip tells nothing of the selection: {reason}"))
            .ok(),
    }))
}

impl Coloring for Strength {
    unsafe fn paint(&self, editor: &Editor) -> Painting {
        // SAFETY: per the contract; the editor's bricks are live, and so are the bricks the finder
        // reports (the editor's own).
        let (bricks, found) = unsafe {
            let bricks = editor.brick_list();
            (bricks.len(), find(self.connections, editor.editor, &bricks))
        };
        let damage = found.damage.clone();
        *FOUND.lock().unwrap_or_else(PoisonError::into_inner) = Some(found);
        let range = damage
            .values()
            .fold(None, |range: Option<(f32, f32)>, &damage| match range {
                Some((weakest, strongest)) => Some((weakest.min(damage), strongest.max(damage))),
                None => Some((damage, damage)),
            });
        let Some((weakest, strongest)) = range else {
            return Painting {
                colors: HashMap::new(),
                summary: format!("of {bricks} bricks, none connected"),
            };
        };
        let summary = format!(
            "of {bricks} bricks, {} connected, needing from {weakest:.3} to {strongest:.3} damage \
             to break off",
            damage.len(),
        );
        let colors = damage
            .into_iter()
            .map(|(brick, damage)| {
                let color = shade(damage, weakest, strongest);
                (brick, [color, color])
            })
            .collect();
        Painting { colors, summary }
    }

    unsafe fn describe(&self, editor: &Editor, hover: &Hover) -> Vec<String> {
        // SAFETY: per the contract.
        let Some(point) = (unsafe { hover.point(editor) }) else {
            return Vec::new();
        };
        // SAFETY: per the contract.
        let crossed = match self.densities {
            Some(_) => unsafe { hover.line(editor) },
            None => Vec::new(),
        };
        let found = FOUND.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(found) = found.as_ref() else {
            return Vec::new();
        };
        let mut lines = vec![match found.damage.get(&(point.brick as usize)) {
            Some(&damage) => format!("Brick strength: {}", damage_text(damage.into())),
            None => "Brick strength: not connected".to_owned(),
        }];
        if let Some(selection) = &self.selection {
            // SAFETY: the editor's bricks, live (the one pointed at is among them).
            let held = unsafe { selection_strength(editor, selection, &found.pairs, point.brick) };
            if let Some(held) = held {
                lines.push(if held > 0.0 {
                    format!("Selection strength: {}", damage_text(held.into()))
                } else {
                    "Selection strength: not connected to the rest".to_owned()
                });
            }
        }
        if let Some(densities) = &self.densities
            && !crossed.is_empty()
        {
            let (mut armor, mut thickness) = (0.0_f64, 0.0_f64);
            for crossing in &crossed {
                let across = f64::from(crossing.exit - crossing.entry);
                // SAFETY: as above.
                let density = unsafe {
                    densities.density(self.connections.physical_material(crossing.brick))
                };
                armor += across * f64::from(density);
                thickness += across;
            }
            let bricks = match crossed.len() {
                1 => "1 brick".to_owned(),
                count => format!("{count} bricks"),
            };
            lines.push(format!(
                "Strength to pierce through vehicle: {}",
                pierce_text(armor)
            ));
            lines.push(format!("{} cm through {bricks}", number(thickness)));
        }
        lines
    }
}

/// The damage each of `bricks` needs to break off, by address (see [`find`]): for the shot preview,
/// which tells the bricks a shot breaks off.
///
/// # Safety
/// `editor` must be a live vehicle editor, `bricks` its bricks. Game thread only.
pub(crate) unsafe fn break_off_damage(
    editor: *mut UObjectBase,
    bricks: &[*mut UObjectBase],
) -> Result<HashMap<usize, f32>, String> {
    let connections = brick_connections::find()?;
    // SAFETY: per the contract.
    Ok(unsafe { find(connections, editor, bricks) }.damage)
}

/// What the view mode shows of `bricks`: the damage each connected one needs to break off (the
/// strengths of its weld group's connections, its own if not welded; all above 0), and the
/// connections themselves.
///
/// # Safety
/// `editor` must be a live vehicle editor, `bricks` its bricks. Game thread only.
unsafe fn find(
    connections: &Connections,
    editor: *mut UObjectBase,
    bricks: &[*mut UObjectBase],
) -> Found {
    // SAFETY: per the contract; the reported bricks are the editor's, live.
    let found = unsafe { connections.find_all(editor) };
    // The bricks' connections, merged by pair as `CreateBrickConnection` does.
    let mut pairs: HashMap<(usize, usize), Pair> = HashMap::new();
    for connection in found {
        let (brick, other) = (connection.brick as usize, connection.other as usize);
        let pair = pairs
            .entry((brick.min(other), brick.max(other)))
            .or_insert(Pair {
                brick: connection.brick,
                other: connection.other,
                kind: connection.kind,
                points: 0,
            });
        if pair.kind == connection.kind {
            pair.points = pair.points.saturating_add(connection.points);
        }
    }
    // What breaks off together: a weld group, or a brick alone.
    let body = |brick: *mut UObjectBase| {
        // SAFETY: a live brick (see above).
        match unsafe { connections.weld_group(brick) } {
            0 => Body::Brick(brick as usize),
            group => Body::Welded(group),
        }
    };
    let mut bodies: HashMap<Body, f32> = HashMap::new();
    let mut strengths = Vec::with_capacity(pairs.len());
    for pair in pairs.values() {
        // SAFETY: live bricks (see above), on the game thread.
        let strength =
            unsafe { connections.strength(pair.brick, pair.other, pair.kind, pair.points) };
        // The finder doesn't connect bricks of one weld group: these are two bodies.
        for brick in [pair.brick, pair.other] {
            *bodies.entry(body(brick)).or_default() += strength;
        }
        strengths.push((pair.brick as usize, pair.other as usize, strength));
    }
    // Every brick of a connected body, also welded ones with no connection of their own.
    let damage = bricks
        .iter()
        .filter_map(|&brick| {
            let damage = bodies.get(&body(brick)).copied()?;
            (damage > 0.0 && damage.is_finite()).then_some((brick as usize, damage))
        })
        .collect();
    Found {
        damage,
        pairs: strengths,
    }
}

/// If `brick` is selected: the strength of all the connections (`pairs`, as in [`Found`]) between
/// the selected bricks and the others (0 if none).
///
/// # Safety
/// `brick` must be one of `editor`'s bricks. Game thread only.
unsafe fn selection_strength(
    editor: &Editor,
    selection: &Selection,
    pairs: &[(usize, usize, f32)],
    brick: *mut UObjectBase,
) -> Option<f32> {
    // SAFETY: per the contract; the editor's bricks are live.
    unsafe {
        if !selection.is_selected(brick) {
            return None;
        }
        let selected: HashSet<usize> = editor
            .brick_list()
            .into_iter()
            .filter(|&brick| selection.is_selected(brick))
            .map(|brick| brick as usize)
            .collect();
        Some(
            pairs
                .iter()
                .filter(|(brick, other, _)| selected.contains(brick) != selected.contains(other))
                .map(|&(_, _, strength)| strength)
                .sum(),
        )
    }
}

/// The connections of two bricks, merged into one.
struct Pair {
    brick: *mut UObjectBase,
    other: *mut UObjectBase,
    kind: u8,
    points: u16,
}

/// Bricks breaking off together.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Body {
    Brick(usize),
    Welded(u16),
}

/// What 1 AP is, the players' unit of how strong guns are: a shot of the Ballistic 2x2x2 gun brick
/// (`BP_Gun_2x2x2_Ballistic`, "Gun 2x2x2" in game like the plain one, which players don't use; the
/// same at any range; BRMK 1.11.6, see `docs/re/guns.md`) with two barrels in front: the most
/// barrels add (`MaxBarrelLength`), without them it's half. It deals [`AP_DAMAGE`], and goes
/// through [`AP_PENETRATION`] of armor (`100 * PenetrationCoefficient * Damage`: 0.5 * 15).
pub(crate) const AP_DAMAGE: f64 = 15.0;
pub(crate) const AP_PENETRATION: f64 = 750.0;

/// `damage` for the tooltip, also in AP: how many times an AP shot breaks it off.
fn damage_text(damage: f64) -> String {
    format!("{} dmg ({} AP)", number(damage), number(damage / AP_DAMAGE))
}

/// A line's armor (see `game::penetration`) for the tooltip, in AP: how many times what an AP shot
/// goes through.
fn pierce_text(armor: f64) -> String {
    format!("{} AP", number(armor / AP_PENETRATION))
}
