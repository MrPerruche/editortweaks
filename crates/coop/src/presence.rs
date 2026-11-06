//! Showing who works where: each player's color, who of the others the editor's outline can show
//! (it has a few spare colors), when the local selection and camera go out again, and others'
//! cameras gliding between the views they send.

use crate::ids::{CoopId, ORIGINAL_SLOT, Slot};
use crate::ops::Transform;

/// The players' colors, with their names (sRGB, 0 to 1: the outline draws after tone mapping).
/// Away from the game's own outline colors (orange selection, green and red validity) and the line
/// tool's purple. As many as a session is likely to hold, the most distinct first.
pub const COLORS: [(&str, [f32; 3]); 10] = [
    ("cyan", [0.0, 0.85, 1.0]),
    ("magenta", [1.0, 0.2, 0.85]),
    ("lime", [0.6, 1.0, 0.1]),
    ("blue", [0.25, 0.4, 1.0]),
    ("yellow", [1.0, 0.92, 0.15]),
    ("white", [1.0, 1.0, 1.0]),
    ("pink", [1.0, 0.6, 0.75]),
    ("mint", [0.45, 1.0, 0.75]),
    ("tan", [0.8, 0.65, 0.42]),
    ("teal", [0.0, 0.55, 0.55]),
];

/// How many others the outline can show at once (the material's spare colors).
pub const OUTLINED: usize = 3;

/// `slot`'s color among `members` (every member's slot, the subhost's too): by its place among
/// them, so everyone shows everyone in the same color, and up to [`COLORS`]' count all differ.
pub fn color_of(members: &[Slot], slot: Slot) -> (&'static str, [f32; 3]) {
    let mut slots: Vec<Slot> = members
        .iter()
        .copied()
        .filter(|&s| s != ORIGINAL_SLOT)
        .collect();
    slots.sort_unstable();
    slots.dedup();
    let place = slots.iter().position(|&s| s == slot).unwrap_or(slots.len());
    COLORS[place % COLORS.len()]
}

/// Whose selections the outline shows: the others (not `own`) selecting something, by slot,
/// at most [`OUTLINED`]. A brick several of them selected shows in the first one's.
pub fn outlined(own: Option<Slot>, selecting: impl IntoIterator<Item = Slot>) -> Vec<Slot> {
    let mut slots: Vec<Slot> = selecting.into_iter().filter(|&s| Some(s) != own).collect();
    slots.sort_unstable();
    slots.dedup();
    slots.truncate(OUTLINED);
    slots
}

/// When the local selection goes out: soon after it changes, and now and then anyway, so who
/// joined since sees it too.
#[derive(Default)]
pub struct Pace {
    /// The selection last sent, and when (ms).
    sent: Option<(Vec<CoopId>, u64)>,
}

impl Pace {
    /// The least time between two selections sent (ms; the session's allowance is 10 a second).
    pub const SOONEST: u64 = 200;
    /// How often an unchanged selection goes out again (ms).
    pub const AGAIN: u64 = 5_000;

    /// Whether `ids` (the selection now, sorted) go out at `now` (ms). Nothing selected goes out
    /// only to clear what was.
    pub fn due(&mut self, now: u64, ids: &[CoopId]) -> bool {
        let due = match &self.sent {
            None => !ids.is_empty(),
            Some((sent, at)) => {
                let since = now.saturating_sub(*at);
                if sent != ids {
                    since >= Self::SOONEST
                } else {
                    !ids.is_empty() && since >= Self::AGAIN
                }
            }
        };
        if due {
            self.sent = Some((ids.to_vec(), now));
        }
        due
    }
}

/// Where a player's camera is and which way it looks, in the editor's space: the bricks' space,
/// the same for everyone wherever their editor stands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub location: [f32; 3],
    /// Which way it looks: a unit vector.
    pub forward: [f32; 3],
}

impl View {
    /// Whether every number is finite, the location within a brick's caps and `forward` about a
    /// unit long. Peers' views are refused otherwise.
    pub fn is_sane(&self) -> bool {
        let length = self.forward.iter().map(|v| v * v).sum::<f32>().sqrt();
        self.location
            .iter()
            .all(|v| v.abs() <= Transform::MAX_LOCATION)
            && (0.5..=1.5).contains(&length)
    }

    /// Whether it differs from `other` enough to send: moved a centimeter, or turned a little.
    fn differs(&self, other: &View) -> bool {
        let apart = |a: [f32; 3], b: [f32; 3]| {
            a.iter()
                .zip(b)
                .map(|(a, b)| (a - b) * (a - b))
                .sum::<f32>()
                .sqrt()
        };
        apart(self.location, other.location) > 1.0 || apart(self.forward, other.forward) > 0.005
    }
}

/// When the local camera goes out: soon after it moves, now and then anyway, so who joined since
/// sees it too. `None` (not looking at the editor) goes out once.
#[derive(Default)]
pub struct ViewPace {
    /// The view last sent, and when (ms).
    sent: Option<(Option<View>, u64)>,
}

impl ViewPace {
    /// The least time between two views sent (ms; the session's allowance is 20 a second).
    pub const SOONEST: u64 = 100;
    /// How often an unchanged view goes out again (ms).
    pub const AGAIN: u64 = 5_000;

    /// Whether `view` (the camera now) goes out at `now` (ms).
    pub fn due(&mut self, now: u64, view: Option<View>) -> bool {
        let due = match (&self.sent, view) {
            (None, view) => view.is_some(),
            (Some((sent, at)), view) => {
                let since = now.saturating_sub(*at);
                match (sent, view) {
                    (Some(sent), Some(view)) if !sent.differs(&view) => since >= Self::AGAIN,
                    (None, None) => false,
                    _ => since >= Self::SOONEST,
                }
            }
        };
        if due {
            self.sent = Some((view, now));
        }
        due
    }
}

/// How fast a shown camera catches up with the last view sent: the share of the way left it
/// covers per second, as a rate (views come about every [`ViewPace::SOONEST`]).
const GLIDE_RATE: f32 = 15.0;
/// Farther than this (cm) a shown camera jumps rather than glides.
const GLIDE_JUMP: f32 = 5_000.0;

/// `shown` moved toward `target` over `seconds`, so others' cameras don't jump between the views
/// they send.
pub fn glide(shown: View, target: View, seconds: f32) -> View {
    let far = shown
        .location
        .iter()
        .zip(target.location)
        .any(|(a, b)| (a - b).abs() > GLIDE_JUMP);
    if far {
        return target;
    }
    let share = 1.0 - (-GLIDE_RATE * seconds.max(0.0)).exp();
    let toward = |a: [f32; 3], b: [f32; 3]| std::array::from_fn(|i| a[i] + (b[i] - a[i]) * share);
    let forward: [f32; 3] = toward(shown.forward, target.forward);
    let length = forward.iter().map(|v| v * v).sum::<f32>().sqrt();
    View {
        location: toward(shown.location, target.location),
        // Turning half a turn passes near zero length: then it takes the target's at once.
        forward: if length > 0.1 {
            forward.map(|v| v / length)
        } else {
            target.forward
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::SUBHOST_SLOT;

    fn id(counter: u32) -> CoopId {
        CoopId::new(2, counter).unwrap()
    }

    #[test]
    fn everyone_sees_the_same_distinct_colors() {
        let members = [SUBHOST_SLOT, 2, 5, 9];
        let colors: Vec<_> = members.iter().map(|&s| color_of(&members, s).0).collect();
        assert_eq!(colors, ["cyan", "magenta", "lime", "blue"]);
        // The order the list comes in doesn't matter, nor the original slot.
        assert_eq!(color_of(&[9, ORIGINAL_SLOT, 5, 2, 1], 5).0, "lime");
    }

    #[test]
    fn colors_cycle_past_the_palette() {
        let count = COLORS.len() as Slot;
        let members: Vec<Slot> = (1..=count + 2).collect();
        assert_eq!(color_of(&members, count + 1).0, color_of(&members, 1).0);
    }

    #[test]
    fn a_full_palette_of_players_all_differ() {
        let members: Vec<Slot> = (1..=COLORS.len() as Slot).collect();
        let mut names: Vec<_> = members.iter().map(|&s| color_of(&members, s).0).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), COLORS.len());
    }

    #[test]
    fn the_outline_shows_a_few_others() {
        assert_eq!(outlined(Some(2), [5, 2, 1, 1]), vec![1, 5]);
        assert_eq!(outlined(None, [7, 3, 4, 6]), vec![3, 4, 6]);
        assert!(outlined(Some(1), [1]).is_empty());
    }

    #[test]
    fn a_selection_goes_out_soon_after_it_changes() {
        let mut pace = Pace::default();
        assert!(!pace.due(0, &[]), "nothing selected yet: nothing to clear");
        let a = [id(1), id(2)];
        assert!(pace.due(10, &a));
        assert!(!pace.due(20, &a), "unchanged");
        let b = [id(3)];
        assert!(!pace.due(100, &b), "too soon");
        assert!(pace.due(10 + Pace::SOONEST, &b));
        assert!(!pace.due(300 + Pace::SOONEST, &b));
        assert!(
            pace.due(10 + Pace::SOONEST + Pace::AGAIN, &b),
            "again for newcomers"
        );
    }

    fn view(x: f32, forward: [f32; 3]) -> View {
        View {
            location: [x, 0.0, 0.0],
            forward,
        }
    }

    #[test]
    fn views_are_checked() {
        assert!(view(10.0, [1.0, 0.0, 0.0]).is_sane());
        assert!(!view(f32::NAN, [1.0, 0.0, 0.0]).is_sane());
        assert!(!view(1e9, [1.0, 0.0, 0.0]).is_sane());
        assert!(!view(0.0, [0.0, 0.0, 0.0]).is_sane());
        assert!(!view(0.0, [f32::INFINITY, 0.0, 0.0]).is_sane());
        assert!(!view(0.0, [f32::NAN, 1.0, 0.0]).is_sane());
    }

    #[test]
    fn a_view_goes_out_soon_after_it_moves() {
        let mut pace = ViewPace::default();
        assert!(!pace.due(0, None), "not looking yet: nothing to clear");
        let a = view(0.0, [1.0, 0.0, 0.0]);
        assert!(pace.due(10, Some(a)));
        assert!(
            !pace.due(500, Some(view(0.5, [1.0, 0.0, 0.0]))),
            "barely moved"
        );
        let b = view(20.0, [1.0, 0.0, 0.0]);
        assert!(!pace.due(50, Some(b)), "too soon");
        assert!(pace.due(10 + ViewPace::SOONEST, Some(b)));
        let turned = view(20.0, [0.99, 0.14, 0.0]);
        assert!(pace.due(10 + 2 * ViewPace::SOONEST, Some(turned)));
        assert!(!pace.due(3_000, Some(turned)));
        assert!(
            pace.due(10 + 2 * ViewPace::SOONEST + ViewPace::AGAIN, Some(turned)),
            "again for newcomers"
        );
        assert!(pace.due(20_000, None), "stopped looking");
        assert!(!pace.due(40_000, None), "said once");
        assert!(pace.due(40_001, Some(a)));
    }

    #[test]
    fn cameras_glide_to_their_views() {
        let from = view(0.0, [1.0, 0.0, 0.0]);
        let to = view(100.0, [0.0, 1.0, 0.0]);
        assert_eq!(glide(from, to, 0.0), from);
        let half = glide(from, to, 0.05);
        assert!(half.location[0] > 30.0 && half.location[0] < 70.0);
        let length: f32 = half.forward.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((length - 1.0).abs() < 1e-5, "still a direction");
        let later = glide(from, to, 1.0);
        assert!((later.location[0] - 100.0).abs() < 0.01);
        assert_eq!(
            glide(from, view(1e5, [1.0, 0.0, 0.0]), 0.01).location[0],
            1e5
        );
        let back = view(0.0, [-1.0, 0.0, 0.0]);
        assert!(
            glide(from, back, 0.046).is_sane(),
            "a half turn passes zero"
        );
    }

    #[test]
    fn clearing_goes_out_once() {
        let mut pace = Pace::default();
        assert!(pace.due(0, &[id(1)]));
        assert!(pace.due(Pace::SOONEST, &[]));
        assert!(!pace.due(Pace::SOONEST + Pace::AGAIN * 2, &[]));
    }
}
