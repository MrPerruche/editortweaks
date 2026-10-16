//! Keeping the editor and the world apart while the map shows, so that moving the editor into the
//! map changes nothing for anyone else, whoever hosts (`docs/re/map-in-editor.md`, "Multiplayer").
//!
//! The editor's components collide with the world like any other: its bricks block vehicles,
//! characters and sensors, and the editor's clicks hit the map. While the map shows, every editor
//! component that collides gets the [`isolated`] collision: its object type a channel no game
//! system uses, ignoring every other channel. The editor's own queries go to that channel and hit
//! only objects of its type (`game::editor_trace`). Each component's collision is kept and put back
//! when the map hides.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use brickrust::ue::coreuobject::UObjectBase;

use crate::game::component_collision::{
    self, BLOCK, CHANNELS, Collision, ComponentCollision, IGNORE, NO_COLLISION, VISIBILITY,
};
use crate::game::editor_trace;
use crate::game::weak_object::WeakObject;
use crate::prelude::*;

/// The game code and reflection it uses, and the channel.
pub struct Isolation {
    collision: ComponentCollision,
    channel: u8,
}

/// The editor's components isolated, and the collision each had.
pub struct Isolated {
    /// By component address.
    saved: BTreeMap<usize, (WeakObject, Collision)>,
    /// Whether the editor's queries are isolated.
    active: bool,
}

// SAFETY: weak object references, only used on the game thread (the feature's mutex just
// satisfies statics).
unsafe impl Send for Isolated {}

impl Isolated {
    pub const fn new() -> Self {
        Self {
            saved: BTreeMap::new(),
            active: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.active && self.saved.is_empty()
    }

    /// How many components are isolated.
    pub fn len(&self) -> usize {
        self.saved.len()
    }
}

/// Finds what it needs and hooks the editor's queries. After engine init.
pub fn find() -> Result<Isolation, String> {
    let isolation = Isolation {
        collision: component_collision::find()?,
        channel: component_collision::unused_channel()?,
    };
    editor_trace::install()?;
    Ok(isolation)
}

/// The collision an editor component gets while the map shows: of the channel's type, ignoring
/// every other channel; it blocks the channel if the editor's clicks could hit it (it blocked
/// visibility).
///
/// Whether it collides at all (`CollisionEnabled`) stays as it was. Turning its physics off or on
/// makes the engine rebuild the component's physics state (`FBodyInstance::SetCollisionEnabled`),
/// which a brick's component only allows from its brick's own `UBrick::CreatePhysicsState`: it
/// lost its physics state for good, and clicks found no brick any more (2026-10-15, see
/// `docs/re/map-in-editor.md`). Ignoring every other channel is enough: the engine's queries and its
/// physics only let two things meet when each blocks the other's type.
pub fn isolated(original: &Collision, channel: u8) -> Collision {
    let mut responses = [IGNORE; CHANNELS];
    let clickable = original.has_queries() && original.responses[usize::from(VISIBILITY)] == BLOCK;
    if clickable {
        responses[usize::from(channel)] = BLOCK;
    }
    Collision {
        enabled: original.enabled,
        object_type: channel,
        responses,
    }
}

impl Isolation {
    /// The channel of the editor's queries while the map shows.
    pub fn channel(&self) -> u8 {
        self.channel
    }

    /// How many of `components` still collide with the world, not isolated: [`Self::apply`] works
    /// from the editor's own list of its components, and this checks it covered every brick.
    ///
    /// # Safety
    /// `components` must be live primitive components. Game thread only.
    pub unsafe fn missed(&self, components: impl IntoIterator<Item = *mut UObjectBase>) -> usize {
        components
            .into_iter()
            // SAFETY: per the contract.
            .map(|component| unsafe { self.collision.read(component) })
            .filter(|now| now.enabled != NO_COLLISION && now.object_type != self.channel)
            .count()
    }

    /// Isolates `editor`: each of its components that collides and isn't isolated yet (new, or
    /// given its collision again by the game, e.g. taken back from the editor's pool), then its
    /// queries. Run every tick while the map shows, before the editor moves into the map.
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    pub unsafe fn apply(&self, state: &mut Isolated, editor: *mut UObjectBase) {
        static LOG_FIRST: std::sync::Once = std::sync::Once::new();
        static LOG_CHECK: std::sync::Once = std::sync::Once::new();
        let started = Instant::now();
        let (mut changed, mut calls) = (0, 0);
        // SAFETY: per the contract; the components are the editor's live ones.
        unsafe {
            let components = self.collision.primitive_components(editor);
            let count = components.len();
            for component in components {
                let now = self.collision.read(component);
                if now.object_type == self.channel || now.enabled == NO_COLLISION {
                    continue;
                }
                LOG_FIRST.call_once(|| log_collision("collided", &now));
                state
                    .saved
                    .insert(component as usize, (WeakObject::new(component), now));
                calls += self.collision.set(component, &isolated(&now, self.channel));
                changed += 1;
            }
            if changed == 0 {
                // What every tick with the map shown costs.
                let took = started.elapsed();
                LOG_CHECK
                    .call_once(|| log!("checking the editor's {count} components took {took:.2?}"));
            }
        }
        if !state.active {
            editor_trace::isolate(Some((editor, self.channel)));
            state.active = true;
        }
        report("isolated", changed, calls, started.elapsed());
    }

    /// Puts back the collision of the components [`Self::apply`] isolated (those that still have
    /// it, and aren't being destroyed with a closed editor), and the editor's queries. Run once the
    /// editor is back out of the map.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn restore(&self, state: &mut Isolated) {
        if state.is_empty() {
            return;
        }
        editor_trace::isolate(None);
        state.active = false;
        let started = Instant::now();
        let (mut changed, mut calls) = (0, 0);
        for (component, saved) in std::mem::take(&mut state.saved).into_values() {
            let Some(component) = component.get_live() else {
                continue;
            };
            // SAFETY: a live component (just checked).
            unsafe {
                if self.collision.read(component).object_type == self.channel {
                    calls += self.collision.set(component, &saved);
                    changed += 1;
                }
            }
        }
        report("put back", changed, calls, started.elapsed());
    }
}

/// Logs how many components changed, if any, and how long it took.
fn report(what: &str, changed: usize, calls: usize, took: Duration) {
    if changed > 0 {
        log!(
            "the collision of {changed} editor components {what} ({calls} calls, {took:.1?}{})",
            if took > Duration::from_millis(20) {
                ", slow"
            } else {
                ""
            }
        );
    }
}

/// Logs a component's collision, for the record.
fn log_collision(what: &str, collision: &Collision) {
    let blocks: Vec<usize> = (0..CHANNELS)
        .filter(|&channel| collision.responses[channel] == BLOCK)
        .collect();
    log!(
        "the editor's first component {what}: collision {} (1 queries, 2 physics), type {}, blocking channels {blocks:?}",
        collision.enabled,
        collision.object_type
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::component_collision::{Change, changes};

    const CHANNEL: u8 = 31;

    fn collision(enabled: u8, responses: [u8; CHANNELS]) -> Collision {
        Collision {
            enabled,
            object_type: 15,
            responses,
        }
    }

    #[test]
    fn a_clickable_brick_blocks_only_the_editors_channel() {
        let brick = collision(3, [BLOCK; CHANNELS]);
        let isolated = isolated(&brick, CHANNEL);
        assert_eq!(isolated.object_type, CHANNEL);
        for channel in 0..CHANNELS {
            let expected = if channel == usize::from(CHANNEL) {
                BLOCK
            } else {
                IGNORE
            };
            assert_eq!(isolated.responses[channel], expected, "channel {channel}");
        }
    }

    #[test]
    fn what_clicks_went_through_stays_unclickable() {
        let mut responses = [BLOCK; CHANNELS];
        responses[usize::from(VISIBILITY)] = IGNORE;
        let isolated = isolated(&collision(1, responses), CHANNEL);
        assert_eq!(isolated.responses, [IGNORE; CHANNELS]);
    }

    #[test]
    fn physics_only_blocks_nothing() {
        let isolated = isolated(&collision(2, [BLOCK; CHANNELS]), CHANNEL);
        assert_eq!(isolated.responses, [IGNORE; CHANNELS]);
    }

    #[test]
    fn whether_it_collides_never_changes() {
        // Only the type and the responses change, both ways: never `CollisionEnabled`, which would
        // rebuild the physics state.
        for enabled in 1..=3 {
            let original = collision(enabled, [BLOCK; CHANNELS]);
            let isolated = isolated(&original, CHANNEL);
            assert_eq!(isolated.enabled, enabled);
            for change in changes(&original, &isolated)
                .into_iter()
                .chain(changes(&isolated, &original))
            {
                assert!(
                    !matches!(change, Change::Enabled(_)),
                    "{enabled}: {change:?}"
                );
            }
        }
    }
}
