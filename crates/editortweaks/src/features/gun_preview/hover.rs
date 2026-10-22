//! What the mouse is on, for the shot preview's tooltip ([`super::label`]): the most specific thing
//! within a few pixels of it wins. A marker (where a shot hit something, came out of it, ended, or
//! its damage stopped dropping) first, then a stretch it pierced, then its path; else the brick under the mouse, if it's one of a shot's guns
//! or barrels, or a brick it hits. Nothing near: no tooltip.
//!
//! The shots' points are projected onto the screen once per camera and shots ([`Screen`]), so
//! moving the mouse only measures distances.

use super::Preview;
use super::ballistics::Ending;

/// How near the mouse a marker must be, in pixels.
const MARKER_REACH: f32 = 12.0;
/// How many pixels nearer than it is a marker counts per step up in importance (as drawn, see
/// `super::Layer`), so where a shot ends wins over a marker under it.
const MARKER_RANK: f32 = 4.0;
/// How near the mouse a path or a pierced stretch must be, in pixels.
const LINE_REACH: f32 = 6.0;

/// What the mouse is on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    /// A gun or barrel firing shot `shot` (an index into the shots).
    Gun { shot: usize },
    /// The point of shot `shot`'s path `range` cm of distance travelled from the muzzle.
    Path { shot: usize, range: f64 },
    /// Where shot `shot` hit something (its impact `impact`): its markers, or the stretch pierced.
    Impact { shot: usize, impact: usize },
    /// The end of shot `shot`, where it ran out of range (or past what the preview follows).
    End { shot: usize },
    /// Where shot `shot`'s damage stops dropping ([`Preview::weakest`]).
    Weakest { shot: usize },
    /// A brick of the vehicle (its address) that shots hit.
    Brick { brick: usize },
}

/// The shots on the screen: each point of their paths, where they hit things and came out (viewport
/// pixels; `None` behind the camera).
pub struct Screen {
    shots: Vec<ShotOnScreen>,
}

struct ShotOnScreen {
    points: Vec<Option<[f32; 2]>>,
    impacts: Vec<[Option<[f32; 2]>; 2]>,
    weakest: Option<[f32; 2]>,
}

impl Screen {
    /// `shots` on the screen, each world point placed by `project`.
    pub fn new(
        shots: &[Preview],
        mut project: impl FnMut(crate::math::Vec3) -> Option<[f32; 2]>,
    ) -> Self {
        Self {
            shots: shots
                .iter()
                .map(|shot| ShotOnScreen {
                    points: shot
                        .flight
                        .points
                        .iter()
                        .map(|point| project(point.at))
                        .collect(),
                    impacts: shot
                        .flight
                        .impacts
                        .iter()
                        .map(|impact| [project(impact.at), project(impact.exit)])
                        .collect(),
                    weakest: shot.weakest().and_then(|point| project(point.at)),
                })
                .collect(),
        }
    }

    /// What drawn of `shots` (those this was made for) `mouse` is on, if anything.
    pub fn target(&self, shots: &[Preview], mouse: [f32; 2]) -> Option<Target> {
        self.marker(shots, mouse)
            .or_else(|| self.pierced(shots, mouse))
            .or_else(|| self.path(shots, mouse))
    }

    /// The marker nearest `mouse`, within [`MARKER_REACH`].
    fn marker(&self, shots: &[Preview], mouse: [f32; 2]) -> Option<Target> {
        let mut nearest: Option<(f32, Target)> = None;
        // Ranks: 0 where it goes through, 1 where it ricochets or breaks a brick off, 2 where its
        // damage stops dropping, 3 where it ends.
        let mut consider = |point: Option<[f32; 2]>, target: Target, rank: u8| {
            let Some(point) = point else { return };
            let distance = distance(point, mouse);
            let score = distance - MARKER_RANK * f32::from(rank);
            if distance <= MARKER_REACH && nearest.is_none_or(|(best, _)| score < best) {
                nearest = Some((score, target));
            }
        };
        for (index, (shot, screen)) in shots.iter().zip(&self.shots).enumerate() {
            let impacts = &shot.flight.impacts;
            for (impact_index, (impact, [at, exit])) in
                impacts.iter().zip(&screen.impacts).enumerate()
            {
                // As drawn: where it goes through (in and out), ricochets, breaks a brick off or
                // ends.
                let target = Target::Impact {
                    shot: index,
                    impact: impact_index,
                };
                let breaks = shot.breaks.get(impact_index).copied().unwrap_or(false);
                let rank = if impact.stop.is_some() {
                    3
                } else if impact.ricochet.is_some() || breaks {
                    1
                } else {
                    0
                };
                if impact.through || rank > 0 {
                    consider(*at, target, rank);
                }
                if impact.comes_out {
                    consider(*exit, target, 0);
                }
            }
            if matches!(shot.flight.ending, Ending::Range | Ending::Unfinished) {
                consider(
                    screen.points.last().copied().flatten(),
                    Target::End { shot: index },
                    3,
                );
            }
            consider(screen.weakest, Target::Weakest { shot: index }, 2);
        }
        nearest.map(|(_, target)| target)
    }

    /// The stretch pierced nearest `mouse`, within [`LINE_REACH`].
    fn pierced(&self, shots: &[Preview], mouse: [f32; 2]) -> Option<Target> {
        let mut nearest: Option<(f32, Target)> = None;
        for (index, (shot, screen)) in shots.iter().zip(&self.shots).enumerate() {
            for (impact_index, (impact, ends)) in
                shot.flight.impacts.iter().zip(&screen.impacts).enumerate()
            {
                let [Some(at), Some(exit)] = *ends else {
                    continue;
                };
                if !impact.through {
                    continue;
                }
                let (distance, _) = to_segment(at, exit, mouse);
                if distance <= LINE_REACH && nearest.is_none_or(|(best, _)| distance < best) {
                    let target = Target::Impact {
                        shot: index,
                        impact: impact_index,
                    };
                    nearest = Some((distance, target));
                }
            }
        }
        nearest.map(|(_, target)| target)
    }

    /// The point of a path nearest `mouse`, within [`LINE_REACH`].
    fn path(&self, shots: &[Preview], mouse: [f32; 2]) -> Option<Target> {
        let mut nearest: Option<(f32, Target)> = None;
        for (index, (shot, screen)) in shots.iter().zip(&self.shots).enumerate() {
            let points = &shot.flight.points;
            for (pair, on_screen) in points.windows(2).zip(screen.points.windows(2)) {
                let [Some(start), Some(end)] = [on_screen[0], on_screen[1]] else {
                    continue;
                };
                let (distance, along) = to_segment(start, end, mouse);
                if distance <= LINE_REACH && nearest.is_none_or(|(best, _)| distance < best) {
                    let range = pair[0].range + (pair[1].range - pair[0].range) * f64::from(along);
                    nearest = Some((distance, Target::Path { shot: index, range }));
                }
            }
        }
        nearest.map(|(_, target)| target)
    }
}

/// What the brick `brick` (an address) under the mouse is to `shots`: one of a shot's guns or
/// barrels, or a brick they hit.
pub fn brick_target(shots: &[Preview], brick: usize) -> Option<Target> {
    let firing = shots
        .iter()
        .position(|shot| shot.guns.contains(&brick) || shot.barrels.contains(&brick));
    if let Some(shot) = firing {
        return Some(Target::Gun { shot });
    }
    shots
        .iter()
        .any(|shot| {
            shot.flight
                .impacts
                .iter()
                .any(|impact| impact.brick == Some(brick))
        })
        .then_some(Target::Brick { brick })
}

/// How far apart two screen points are, in pixels.
fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// How far `point` is from the segment from `start` to `end`, and how far along it (0 to 1) its
/// nearest point is.
fn to_segment(start: [f32; 2], end: [f32; 2], point: [f32; 2]) -> (f32, f32) {
    let segment = [end[0] - start[0], end[1] - start[1]];
    let length = segment[0] * segment[0] + segment[1] * segment[1];
    let along = if length > 1e-6 {
        (((point[0] - start[0]) * segment[0] + (point[1] - start[1]) * segment[1]) / length)
            .clamp(0.0, 1.0)
    } else {
        0.0
    };
    let nearest = [start[0] + segment[0] * along, start[1] + segment[1] * along];
    (distance(nearest, point), along)
}
