//! Smooth curves built from spinners (`docs/fast-editing-next.md`, "Curves from spinners"): a
//! chain of circular arcs, each starting where and the way the one before ends, so the curve has no
//! kink. One arc is one round spinner ([`Arc::spinner`], placed by [`Arc::axes`] about its hub).
//!
//! The curve goes through nodes, each a point and a handle (a direction, and a length weighing
//! where the arcs meet, like a Bezier curve's handles). Between two nodes there are always exactly
//! two arcs, a biarc ([`biarc`]): two points and their directions can always be joined so (but for
//! one case, below). So moving a node or its handle only changes existing spinners, and spinners
//! are only made for a new node.
//!
//! Nothing may blow up, or bricks shake (their places are stored in `f32`): an arc's radius is
//! computed from its angle, which is at least [`MIN_ANGLE`] (a straight span is a slight arc), and
//! at most [`MAX_RADIUS`]; a direction too close to tell (an arc's plane when it's nearly straight,
//! two directions nearly the same) is decided by what was there before or made exact. Every result
//! is checked finite. Where no good answer exists, the functions say so (`None`) and the caller
//! keeps the last shape.

use crate::spinner::{MIN_ANGLE, Spinner, Vec3};

/// The editor panel's largest radius (cm): the arcs stay within it, so a vanilla player can still
/// edit the spinners (the game's mesh doesn't clamp it).
pub const MAX_RADIUS: f64 = 9000.0;
/// The largest angle of an arc (degrees): a spinner at 360 degrees is a closed ring without caps.
pub const MAX_ARC: f64 = 359.0;
/// The shortest chord an arc may have (cm): shorter, there's no arc to make.
pub const MIN_CHORD: f64 = 1e-3;

/// Below this angle between an arc's start direction and its chord (radians, a quarter of
/// [`MIN_ANGLE`]), the arc is too straight to tell its own plane, and keeps the one it's given.
/// Above it, it takes its own: between the two, a curve passing straight keeps bending the way it
/// did, rather than flipping from frame to frame.
const STRAIGHT: f64 = MIN_ANGLE.to_radians() / 4.0;
/// How close two directions' dot product must be to 1 to count as the same (about 1.4e-6 radians
/// apart): directions read from `f32` places differ by noise of about 1e-7.
const SAME_DIRECTION: f64 = 1e-12;
/// The handles' lengths weigh where a span's arcs meet: their ratio is kept within this and its
/// inverse.
const MOST_RATIO: f64 = 10.0;
/// How far from its span's chord the point where two arcs meet may go, in chords: further, the
/// arcs would be huge loops (two directions pointing nearly back at each other's way).
const FARTHEST_JOINT: f64 = 10.0;
/// The least of a direction left along a surface for it to be laid onto it (see
/// [`along_surface`]): less, it goes nearly straight into it, and which way along is noise.
const MIN_ALONG: f64 = 0.05;
/// How many points [`nearest_allowed`] tries at most.
const MOST_TRIES: usize = 600;
/// How far [`nearest_allowed`] tilts its steps from straight toward the target (degrees), each
/// tilt all the way round in [`AROUND`] steps: tilted, they slide along what isn't allowed.
const TILTS: [f64; 2] = [45.0, 80.0];
const AROUND: usize = 8;

/// A point the curve goes through, and its handle: the way the curve goes there (its direction),
/// and how far it reaches (its length, cm, more than zero).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Node {
    pub point: Vec3,
    pub handle: Vec3,
}

/// A circular arc (one spinner's centre line): from `start` to `end`, turning by `angle` about its
/// plane's `normal` round its `hub`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arc {
    pub start: Vec3,
    pub end: Vec3,
    pub hub: Vec3,
    /// Its plane's normal (length 1), about which it turns from the start to the end
    /// (right-handed: the start's direction is `normal x (start - hub)`).
    pub normal: Vec3,
    /// cm, from [`MIN_CHORD`] / 2 to [`MAX_RADIUS`].
    pub radius: f64,
    /// How far it turns (radians), from [`MIN_ANGLE`] to [`MAX_ARC`].
    pub angle: f64,
}

impl Arc {
    /// The point `along` the arc (0 at its start, 1 at its end).
    pub fn point(&self, along: f64) -> Vec3 {
        let (sin, cos) = (self.angle * along).sin_cos();
        let out = scale(sub(self.start, self.hub), 1.0 / self.radius);
        let side = cross(self.normal, out);
        add(
            self.hub,
            scale(add(scale(out, cos), scale(side, sin)), self.radius),
        )
    }

    /// The way the arc goes `along` it (0 at its start, 1 at its end; length 1).
    pub fn direction(&self, along: f64) -> Vec3 {
        let (sin, cos) = (self.angle * along).sin_cos();
        let out = scale(sub(self.start, self.hub), 1.0 / self.radius);
        let side = cross(self.normal, out);
        add(scale(side, cos), scale(out, -sin))
    }

    /// Its length along the centre line (cm).
    pub fn length(&self) -> f64 {
        self.radius * self.angle
    }

    /// The axes (length 1: X, Y, Z) of a spinner brick built along it, whose origin is the hub
    /// (see [`Self::spinner`]): its start on the +Y axis, its arc turning about +Z (a positive
    /// angle; [`Self::placement`] may turn it the other way).
    pub fn axes(&self) -> [Vec3; 3] {
        self.placement(self.normal).axes
    }

    /// A round spinner along it with thickness and height `size` (cm), to place at [`Self::hub`]
    /// turned by [`Self::axes`].
    pub fn spinner(&self, size: [f64; 2]) -> Spinner {
        Spinner {
            radius: [self.radius; 2],
            size,
            angle: self.angle.to_degrees(),
        }
    }

    /// The same arc gone the other way: from its end to its start.
    pub fn reversed(&self) -> Arc {
        Arc {
            start: self.end,
            end: self.start,
            normal: scale(self.normal, -1.0),
            ..*self
        }
    }

    /// How far its start is from where its tangent lines meet (cm): `radius tan(angle / 2)`, the
    /// length of the handles it would have in a span (see [`biarc`]). `None` from half a turn on,
    /// where they meet behind it or not at all.
    pub fn reach(&self) -> Option<f64> {
        let half = self.angle / 2.0;
        (half < core::f64::consts::FRAC_PI_2 - 1e-6).then(|| self.radius * half.tan())
    }

    /// How a spinner brick goes along it with its own Z axis as near `up` as can be: the arc turns
    /// about its plane's normal, or the other way about the opposite normal with a negative angle
    /// (so a curve's cross-sections keep facing up the way they did as its arcs' planes change).
    pub fn placement(&self, up: Vec3) -> Placement {
        let y = scale(sub(self.start, self.hub), 1.0 / self.radius);
        let (z, angle) = if dot(self.normal, up) >= 0.0 {
            (self.normal, self.angle)
        } else {
            (scale(self.normal, -1.0), -self.angle)
        };
        Placement {
            axes: [cross(y, z), y, z],
            angle: angle.to_degrees(),
        }
    }

    /// Whether a spinner along it shows a thickness `thickness` in full: the game's mesh keeps
    /// the inner surface from going past the hub (see [`Spinner::thickness`]).
    pub fn fits(&self, thickness: f64) -> bool {
        thickness <= 2.0 * self.radius
    }

    /// The arc of a round spinner (see [`Spinner::is_round`]) whose brick has its origin at `hub`
    /// and the axes `axes` (as [`Self::axes`]); `None` for an elliptic one, a closed ring or values
    /// that make no arc.
    pub fn of_spinner(hub: Vec3, axes: [Vec3; 3], spinner: &Spinner) -> Option<Arc> {
        if !spinner.is_round() || spinner.is_full_turn() {
            return None;
        }
        let radius = spinner.radius[0];
        let angle = spinner.radians();
        if !(radius > 0.0 && angle.abs() > 0.0 && radius.is_finite() && angle.is_finite()) {
            return None;
        }
        let [_, y, z] = axes;
        let normal = normalized(if angle < 0.0 { scale(z, -1.0) } else { z })?;
        let mut arc = Arc {
            start: add(hub, scale(y, radius)),
            end: [0.0; 3],
            hub,
            normal,
            radius,
            angle: angle.abs(),
        };
        arc.end = arc.point(1.0);
        Some(arc)
    }
}

/// How a spinner brick is placed along an arc (see [`Arc::placement`]): its axes (X, Y, Z, length
/// 1, square to each other, right-handed), its origin being the arc's hub, and its angle (degrees,
/// signed). Its radius is the arc's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub axes: [Vec3; 3],
    pub angle: f64,
}

/// The rotation turning the unit axes onto `axes` (X, Y, Z: length 1, square to each other,
/// right-handed), as a quaternion `[x, y, z, w]` (an `FQuat`'s components).
pub fn quaternion([x, y, z]: [Vec3; 3]) -> [f64; 4] {
    // The rotation matrix has the axes as columns. Shepperd's way: from its largest diagonal
    // term, so nothing divides by a number near zero.
    let (m00, m11, m22) = (x[0], y[1], z[2]);
    let trace = m00 + m11 + m22;
    let quat = if trace > 0.0 {
        let s = 2.0 * (1.0 + trace).sqrt();
        [
            (y[2] - z[1]) / s,
            (z[0] - x[2]) / s,
            (x[1] - y[0]) / s,
            s / 4.0,
        ]
    } else if m00 >= m11 && m00 >= m22 {
        let s = 2.0 * (1.0 + m00 - m11 - m22).sqrt();
        [
            s / 4.0,
            (y[0] + x[1]) / s,
            (z[0] + x[2]) / s,
            (y[2] - z[1]) / s,
        ]
    } else if m11 >= m22 {
        let s = 2.0 * (1.0 + m11 - m00 - m22).sqrt();
        [
            (y[0] + x[1]) / s,
            s / 4.0,
            (z[1] + y[2]) / s,
            (z[0] - x[2]) / s,
        ]
    } else {
        let s = 2.0 * (1.0 + m22 - m00 - m11).sqrt();
        [
            (z[0] + x[2]) / s,
            (z[1] + y[2]) / s,
            s / 4.0,
            (x[1] - y[0]) / s,
        ]
    };
    let length = quat.iter().map(|part| part * part).sum::<f64>().sqrt();
    quat.map(|part| part / length)
}

/// The arc from `start` to `end` leaving `start` along `direction`, as near as the game's limits
/// let it: at least [`MIN_ANGLE`] (a straight line is a slight arc), at most [`MAX_ARC`] and
/// [`MAX_RADIUS`]. Where they don't, it leaves a little off `direction`; it always ends at `end`.
/// `plane` is the normal to keep for an arc too straight to tell its own (the last one it had,
/// so it doesn't flip).
///
/// `None` for a chord shorter than [`MIN_CHORD`] or longer than the largest radius allows, no
/// direction, or `end` straight behind `start`.
pub fn arc_through(start: Vec3, direction: Vec3, end: Vec3, plane: Vec3) -> Option<Arc> {
    let chord = sub(end, start);
    let length = norm(chord);
    let way = normalized(direction)?;
    if !(MIN_CHORD..=2.0 * MAX_RADIUS).contains(&length) {
        return None;
    }
    let ahead = dot(way, chord);
    let aside = norm(sub(chord, scale(way, ahead)));
    // Half the angle the arc turns by: between its direction and its chord.
    let half = aside.atan2(ahead);
    if half > core::f64::consts::PI - STRAIGHT {
        // Straight behind: only a full circle would come back to it.
        return None;
    }
    let normal = if half > STRAIGHT {
        normalized(cross(way, chord))?
    } else {
        across(plane, chord)?
    };
    let limits = MIN_ANGLE.to_radians() / 2.0..=MAX_ARC.to_radians() / 2.0;
    let mut half = half.clamp(*limits.start(), *limits.end());
    // The radius is the chord over twice the half angle's sine: at most the largest radius.
    let least_sine = length / (2.0 * MAX_RADIUS);
    if half.sin() < least_sine {
        let small = least_sine.min(1.0).asin();
        half = if half < core::f64::consts::FRAC_PI_2 {
            small
        } else {
            core::f64::consts::PI - small
        };
    }
    let radius = length / (2.0 * half.sin());
    // The hub is off the chord's middle, across it toward the side the arc turns to.
    let middle = scale(add(start, end), 0.5);
    let inward = cross(normal, scale(chord, 1.0 / length));
    let hub = add(middle, scale(inward, radius * half.cos()));
    let arc = Arc {
        start,
        end,
        hub,
        normal,
        radius,
        angle: 2.0 * half,
    };
    is_finite(&arc).then_some(arc)
}

/// The two arcs joining node `from` to node `to`: the first leaving `from` along its handle, the
/// second arriving at `to` along its handle, meeting the same way (no kink). The handles' lengths
/// weigh where they meet: a longer handle at `from` makes its arc the longer one. `planes` are
/// the two arcs' last normals (see [`arc_through`]).
///
/// `None` where they can't be joined well: points too close together or too far apart, the same
/// direction pointing back from `to` to `from` (it takes a loop), directions nearly so (huge
/// loops), or a handle of no length. The caller keeps what it had and could ask for a node more.
pub fn biarc(from: Node, to: Node, planes: [Vec3; 2]) -> Option<[Arc; 2]> {
    let (reach0, reach1) = (norm(from.handle), norm(to.handle));
    let t0 = normalized(from.handle)?;
    let mut t1 = normalized(to.handle)?;
    let ratio = (reach1 / reach0).clamp(1.0 / MOST_RATIO, MOST_RATIO);
    let chord = sub(to.point, from.point);
    let squared = dot(chord, chord);
    let length = squared.sqrt();
    if length < MIN_CHORD {
        return None;
    }
    let mut same = dot(t0, t1);
    if 1.0 - same < SAME_DIRECTION {
        // The same direction, but for noise: exactly, or the meeting point below would swing about
        // with the noise.
        t1 = t0;
        same = 1.0;
    }
    // The arcs' tangent lines from the nodes meet at `from + d0 t0` and `to - d1 t1`, with
    // `d1 = ratio d0` and the two points `d0 + d1` apart:
    // `2 ratio (1 - same) d0^2 + 2 (chord . w) d0 - |chord|^2 = 0`, `w = t0 + ratio t1`. Its
    // positive root written so it never subtracts nearly equal numbers, nor divides by
    // `1 - same`.
    let w = add(t0, scale(t1, ratio));
    let toward = dot(chord, w);
    let spread = 2.0 * ratio * (1.0 - same) * squared;
    let below = toward + (toward * toward + spread.max(0.0)).sqrt();
    let weighted = scale(add(scale(from.point, ratio), to.point), 1.0 / (1.0 + ratio));
    let joint = if below > 1e-9 * length {
        let d0 = squared / below;
        // On the line between the tangent lines' points, `d0` from the first: written so the
        // `d0 (t0 - t1)` part, bounded as `d0` grows when the directions near each other, comes
        // last.
        add(weighted, scale(sub(t0, t1), ratio * d0 / (1.0 + ratio)))
    } else if dot(chord, t0).abs() <= 1e-9 * length {
        // The same direction across the chord: an S of two half circles, meeting in the middle
        // (weighed by the handles).
        weighted
    } else {
        return None;
    };
    if norm(sub(joint, weighted)) > FARTHEST_JOINT * length {
        return None;
    }
    let first = arc_through(from.point, t0, joint, planes[0])?;
    let second = arc_through(joint, first.direction(1.0), to.point, planes[1])?;
    Some([first, second])
}

/// The arcs of the curve through `nodes`, two per span between neighbours (see [`biarc`]);
/// `planes` the arcs' last normals, in order (missing ones: +Z). `None` if a span can't be made.
pub fn chain(nodes: &[Node], planes: &[Vec3]) -> Option<Vec<Arc>> {
    let plane = |index: usize| planes.get(index).copied().unwrap_or([0.0, 0.0, 1.0]);
    let mut arcs = Vec::with_capacity(2 * nodes.len().saturating_sub(1));
    for (span, pair) in nodes.windows(2).enumerate() {
        arcs.extend(biarc(
            pair[0],
            pair[1],
            [plane(2 * span), plane(2 * span + 1)],
        )?);
    }
    Some(arcs)
}

/// How near two arcs' ends must be to be joined in a chain (cm): spinners placed cap to cap by
/// their connectors are within `f32` noise of each other.
pub const JOINED: f64 = 0.5;
/// The shortest handle (cm).
pub const MIN_REACH: f64 = 1.0;

/// The order of `arcs` along the one open chain they make, end to end: each arc's place in
/// `arcs`, and whether it goes the other way in the chain (from its end to its start). The chain
/// starts at the first arc (in `arcs`' order) with a free end. `None` if they don't make one open
/// chain: apart, branching (an end near two others), or closed in a ring.
pub fn order(arcs: &[Arc]) -> Option<Vec<(usize, bool)>> {
    let end = |index: usize, last: bool| {
        let arc = &arcs[index];
        if last { arc.end } else { arc.start }
    };
    // The end joined to each end, if any: (arc, its end or start).
    let joined = |index: usize, last: bool| -> Result<Option<(usize, bool)>, ()> {
        let at = end(index, last);
        let mut found = None;
        for other in (0..arcs.len()).filter(|&other| other != index) {
            for other_last in [false, true] {
                if norm(sub(end(other, other_last), at)) <= JOINED {
                    if found.is_some() {
                        return Err(());
                    }
                    found = Some((other, other_last));
                }
            }
        }
        Ok(found)
    };
    let mut links = Vec::with_capacity(arcs.len());
    for index in 0..arcs.len() {
        links.push([joined(index, false).ok()?, joined(index, true).ok()?]);
    }
    // Going from its start unless only its end is free.
    let (first, from_end) = (0..arcs.len()).find_map(|index| match links[index] {
        [None, _] => Some((index, false)),
        [_, None] => Some((index, true)),
        _ => None,
    })?;
    let mut chain = vec![(first, from_end)];
    let (mut index, mut reversed) = (first, from_end);
    // From the end it leaves by to the next arc's joined end.
    while let Some((next, next_last)) = links[index][usize::from(!reversed)] {
        if chain.iter().any(|&(seen, _)| seen == next) || chain.len() > arcs.len() {
            return None;
        }
        // Joined by its end, it goes the other way.
        (index, reversed) = (next, next_last);
        chain.push((index, reversed));
    }
    (chain.len() == arcs.len()).then_some(chain)
}

/// A node of a [`Curve`]: a point it goes through, the way it goes there, and its handles' lengths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurveNode {
    pub point: Vec3,
    /// Length 1.
    pub direction: Vec3,
    /// Toward the span before it and the one after (cm, at least [`MIN_REACH`]): they weigh where
    /// the spans' arcs meet (see [`biarc`]).
    pub reach: [f64; 2],
}

/// Which of a node's handles: toward the span before it, or the one after.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Back,
    Ahead,
}

impl Side {
    fn index(self) -> usize {
        match self {
            Side::Back => 0,
            Side::Ahead => 1,
        }
    }
}

/// A chain of arcs (one per spinner) shaped by its nodes: the start of every other arc, and the
/// last arc's end. Between two nodes, a span of two arcs (see [`biarc`]), or, at the end of a chain
/// of an odd number of arcs, of one: that arc then leaves its first node the node's way and ends
/// at the last node however it comes (the last node has no handles: one arc alone can't hold both
/// directions). Changing a node or a handle only changes the arcs of the spans next to it, and
/// never how many arcs there are.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    nodes: Vec<CurveNode>,
    arcs: Vec<Arc>,
}

impl Curve {
    /// The curve of `arcs` (in order, each going on from where the last ends: see [`order`]), its
    /// nodes as they are. A node where the arcs meet at an angle takes the way between: the next
    /// change smooths the kink. Handles reach where their arc's tangent lines meet, or half its
    /// chord for an arc of half a turn or more. `None` for no arcs.
    pub fn of_arcs(arcs: Vec<Arc>) -> Option<Curve> {
        let last = arcs.last()?;
        let spans = arcs.len().div_ceil(2);
        let reach = |arc: &Arc| {
            arc.reach()
                .unwrap_or_else(|| norm(sub(arc.end, arc.start)) / 2.0)
                .max(MIN_REACH)
        };
        let mut nodes = Vec::with_capacity(spans + 1);
        for span in 0..spans {
            let arc = &arcs[2 * span];
            let direction = match span.checked_sub(1) {
                None => arc.direction(0.0),
                Some(_) => {
                    let before = arcs[2 * span - 1].direction(1.0);
                    normalized(add(before, arc.direction(0.0))).unwrap_or(arc.direction(0.0))
                }
            };
            let back = if span == 0 {
                MIN_REACH
            } else {
                reach(&arcs[2 * span - 1])
            };
            nodes.push(CurveNode {
                point: arc.start,
                direction,
                reach: [back, reach(arc)],
            });
        }
        let back = if arcs.len().is_multiple_of(2) {
            reach(last)
        } else {
            MIN_REACH
        };
        nodes.push(CurveNode {
            point: last.end,
            direction: last.direction(1.0),
            reach: [back, MIN_REACH],
        });
        Some(Curve { nodes, arcs })
    }

    pub fn nodes(&self) -> &[CurveNode] {
        &self.nodes
    }

    /// Its arcs, in order.
    pub fn arcs(&self) -> &[Arc] {
        &self.arcs
    }

    /// Where the handle of node `node` on `side` ends, if it has one there: not before the first
    /// node nor after the last, nor at the last node of a span of one arc.
    pub fn handle(&self, node: usize, side: Side) -> Option<Vec3> {
        let at = self.nodes.get(node)?;
        let has = match side {
            Side::Back => node > 0 && self.arcs_of(node - 1).len() == 2,
            Side::Ahead => node + 1 < self.nodes.len(),
        };
        let way = match side {
            Side::Back => -1.0,
            Side::Ahead => 1.0,
        };
        has.then(|| add(at.point, scale(at.direction, way * at.reach[side.index()])))
    }

    /// The curve with node `node` at `point`; `None` if a span next to it can't be made (see
    /// [`biarc`] and [`arc_through`]): the caller keeps the curve it had.
    pub fn with_node(&self, node: usize, point: Vec3) -> Option<Curve> {
        let direction = self.nodes.get(node)?.direction;
        self.with_node_along(node, point, direction)
    }

    /// The curve with node `node` at `point`, going `direction` there (its handles turn with it,
    /// keeping their lengths; the last node of a span of one arc still goes the way its arc
    /// arrives); `None` as for [`Self::with_node`], or for no direction.
    pub fn with_node_along(&self, node: usize, point: Vec3, direction: Vec3) -> Option<Curve> {
        let direction = normalized(direction)?;
        let mut curve = self.clone();
        let at = curve.nodes.get_mut(node)?;
        at.point = point;
        at.direction = direction;
        curve.solve_around(node)?;
        Some(curve)
    }

    /// The curve with the handle of node `node` on `side` ending at `tip` (its direction and
    /// length, at least [`MIN_REACH`]; the node's other handle turns with it); `None` if it has no
    /// handle there, or a span next to it can't be made.
    pub fn with_handle(&self, node: usize, side: Side, tip: Vec3) -> Option<Curve> {
        self.handle(node, side)?;
        let mut curve = self.clone();
        let at = &mut curve.nodes[node];
        let off = match side {
            Side::Back => sub(at.point, tip),
            Side::Ahead => sub(tip, at.point),
        };
        let reach = norm(off);
        if reach < MIN_REACH {
            return None;
        }
        at.direction = normalized(off)?;
        at.reach[side.index()] = reach;
        curve.solve_around(node)?;
        Some(curve)
    }

    /// Where the arcs of span `span` are in [`Self::arcs`].
    fn arcs_of(&self, span: usize) -> core::ops::Range<usize> {
        let start = 2 * span;
        start..(start + 2).min(self.arcs.len())
    }

    /// Makes the spans before and after node `node` again.
    fn solve_around(&mut self, node: usize) -> Option<()> {
        for span in [node.checked_sub(1), Some(node)].into_iter().flatten() {
            if span + 1 < self.nodes.len() {
                self.solve(span)?;
            }
        }
        Some(())
    }

    /// Makes the arcs of span `span` again from its nodes.
    fn solve(&mut self, span: usize) -> Option<()> {
        let range = self.arcs_of(span);
        let (from, to) = (self.nodes[span], self.nodes[span + 1]);
        let planes = |index: usize| self.arcs[index].normal;
        if range.len() == 2 {
            let arcs = biarc(
                Node {
                    point: from.point,
                    handle: scale(from.direction, from.reach[1]),
                },
                Node {
                    point: to.point,
                    handle: scale(to.direction, to.reach[0]),
                },
                [planes(range.start), planes(range.start + 1)],
            )?;
            self.arcs[range].copy_from_slice(&arcs);
        } else {
            let arc = arc_through(from.point, from.direction, to.point, planes(range.start))?;
            self.arcs[range.start] = arc;
            // The last node goes the way its one arc arrives.
            self.nodes[span + 1].direction = arc.direction(1.0);
        }
        Some(())
    }
}

/// `direction` laid onto a surface facing `normal`: its part across the surface taken out (length
/// 1), so a curve going that way through a point on the surface touches it rather than going into
/// it. `None` if it goes nearly straight into the surface (less than [`MIN_ALONG`] of it left).
pub fn along_surface(direction: Vec3, normal: Vec3) -> Option<Vec3> {
    let normal = normalized(normal)?;
    let flat = sub(direction, scale(normal, dot(direction, normal)));
    (norm(flat) >= MIN_ALONG * norm(direction)).then(|| normalized(flat))?
}

/// The point nearest `target` that `allowed` lets through, looked for from `from`: `target` itself
/// if it can, else as far along the straight line toward it as goes, then sliding on round what
/// isn't allowed while that gets nearer, by steps down to `tolerance` (cm). It's a local search:
/// it doesn't jump over what isn't allowed, and only gets as near as it can from where it went (at
/// most [`MOST_TRIES`] tries). `None` if `from` isn't let through. Within `tolerance` of `from`,
/// it gives `from` itself, so the same target again changes nothing.
pub fn nearest_allowed(
    from: Vec3,
    target: Vec3,
    tolerance: f64,
    mut allowed: impl FnMut(Vec3) -> bool,
) -> Option<Vec3> {
    if allowed(target) {
        return Some(target);
    }
    if !allowed(from) {
        return None;
    }
    let tolerance = tolerance.max(1e-9);
    let mut search = Search {
        target,
        allowed,
        tries: 2,
    };
    let mut best = search.toward_target(from, tolerance, f64::INFINITY);
    let mut distance = norm(sub(target, best));
    let mut step = distance / 2.0;
    // The way the last step across went, while that's how it gets nearer.
    let mut sliding: Option<Vec3> = None;
    while step > tolerance && search.tries < MOST_TRIES {
        let Some(toward) = normalized(sub(target, best)) else {
            break;
        };
        // The nearest of the points a step away, straight on or tilted every way, that's let
        // through and nearer; else a step across, then on toward the target as far as goes (along
        // what isn't allowed where it's nearly square to the target, or curves round it); else a
        // smaller step. While sliding, across first. Near the nearest point, going along what
        // isn't allowed barely changes the distance (by the step times how far off it is, over
        // the distance): the way on toward the target is found finely enough to tell.
        let fine = (step * tolerance / distance).max(1e-9);
        let found = if let Some(way) = sliding {
            let slid = search.slid(best, distance, toward, step, fine, way);
            sliding = slid.map(|(_, _, across)| across);
            if let Some((_, _, across)) = slid
                && dot(across, way) < 0.0
            {
                // Back the other way: it went past the nearest point, a smaller step next.
                step /= 2.0;
            }
            slid.map(|(nearer, point, _)| (nearer, point))
                .or_else(|| search.stepped(best, distance, toward, step))
        } else {
            search.stepped(best, distance, toward, step).or_else(|| {
                let slid = search.slid(best, distance, toward, step, fine, toward);
                sliding = slid.map(|(_, _, way)| way);
                slid.map(|(nearer, point, _)| (nearer, point))
            })
        };
        match found {
            // Going on well: the same step next (no further than half the way left).
            Some((nearer, point)) => {
                (distance, best) = (nearer, point);
                step = step.min(distance / 2.0);
            }
            None => step /= 2.0,
        }
    }
    Some(if norm(sub(best, from)) <= tolerance {
        from
    } else {
        best
    })
}

/// The state of [`nearest_allowed`]'s search.
struct Search<F> {
    target: Vec3,
    allowed: F,
    /// How many points it tried.
    tries: usize,
}

impl<F: FnMut(Vec3) -> bool> Search<F> {
    /// Whether `point` is let through; false once out of tries.
    fn allows(&mut self, point: Vec3) -> bool {
        if self.tries >= MOST_TRIES {
            return false;
        }
        self.tries += 1;
        (self.allowed)(point)
    }

    /// As far as goes from `from` (let through) straight toward the target, to within `tolerance`:
    /// first tried `first` (cm) along, then twice as far each time while let through.
    fn toward_target(&mut self, from: Vec3, tolerance: f64, first: f64) -> Vec3 {
        let way = sub(self.target, from);
        let length = norm(way);
        if length <= tolerance {
            return from;
        }
        // Parts of the way: `low` is let through, `high` isn't (the target itself isn't).
        let mut low = 0.0;
        let mut high = (first / length).min(1.0);
        while high < 1.0 && self.allows(add(from, scale(way, high))) {
            low = high;
            high = (high * 2.0).min(1.0);
        }
        while (high - low) * length > tolerance && self.tries < MOST_TRIES {
            let middle = (low + high) / 2.0;
            if self.allows(add(from, scale(way, middle))) {
                low = middle;
            } else {
                high = middle;
            }
        }
        add(from, scale(way, low))
    }

    /// The nearest point `step` from `best` (`distance` from the target), straight `toward` it or
    /// tilted by [`TILTS`], that's nearer and let through, and its distance.
    fn stepped(
        &mut self,
        best: Vec3,
        distance: f64,
        toward: Vec3,
        step: f64,
    ) -> Option<(f64, Vec3)> {
        let ahead = core::iter::once(toward).chain(tilted(toward, &TILTS));
        let mut tried: Vec<(f64, Vec3)> = ahead
            .map(|direction| add(best, scale(direction, step)))
            .map(|point| (norm(sub(self.target, point)), point))
            .filter(|&(nearer, _)| nearer < distance)
            .collect();
        tried.sort_by(|a, b| a.0.total_cmp(&b.0));
        tried.into_iter().find(|&(_, point)| self.allows(point))
    }

    /// A point reached going `step` from `best` square to the way `toward` the target, then on
    /// toward it as far as goes (to within `tolerance`), that's nearer than `distance`: its
    /// distance, and the way across. Across along `way` (less its part toward the target) if that
    /// gets nearer, else the nearest of every way round.
    fn slid(
        &mut self,
        best: Vec3,
        distance: f64,
        toward: Vec3,
        step: f64,
        tolerance: f64,
        way: Vec3,
    ) -> Option<(f64, Vec3, Vec3)> {
        let kept = normalized(sub(way, scale(toward, dot(way, toward))));
        let mut found: Option<(f64, Vec3, Vec3)> = None;
        for (index, direction) in kept.into_iter().chain(tilted(toward, &[90.0])).enumerate() {
            let across = add(best, scale(direction, step));
            if !self.allows(across) {
                continue;
            }
            let point = self.toward_target(across, tolerance, step);
            let nearer = norm(sub(self.target, point));
            if nearer < found.map_or(distance, |(least, _, _)| least) {
                found = Some((nearer, point, direction));
                if index == 0 && kept.is_some() {
                    // The same way as before still gets nearer.
                    break;
                }
            }
        }
        found
    }
}

/// `toward` (length 1) tilted by each of `tilts` (degrees), all the way round in [`AROUND`] steps.
fn tilted(toward: Vec3, tilts: &'static [f64]) -> impl Iterator<Item = Vec3> {
    // Two directions square to it and each other: across the axis it's least along.
    let axis = (0..3)
        .min_by(|&a, &b| toward[a].abs().total_cmp(&toward[b].abs()))
        .unwrap_or(2);
    let mut other = [0.0; 3];
    other[axis] = 1.0;
    let side = normalized(cross(toward, other)).unwrap_or(other);
    let up = cross(toward, side);
    tilts.iter().flat_map(move |tilt| {
        let (sin, cos) = tilt.to_radians().sin_cos();
        (0..AROUND).map(move |turn| {
            let round = core::f64::consts::TAU * turn as f64 / AROUND as f64;
            let off = add(scale(side, round.cos()), scale(up, round.sin()));
            add(scale(toward, cos), scale(off, sin))
        })
    })
}

/// The direction across `chord` nearest `plane` (length 1): `plane` less its part along the
/// chord, or, if that's nothing, whichever axis is most across it.
fn across(plane: Vec3, chord: Vec3) -> Option<Vec3> {
    let way = normalized(chord)?;
    let off = |normal: Vec3| normalized(sub(normal, scale(way, dot(normal, way))));
    let kept = normalized(plane).and_then(|normal| {
        // Nearly along the chord, its part across is noise.
        (dot(normal, way).abs() < 1.0 - 1e-6).then(|| off(normal))?
    });
    kept.or_else(|| {
        let axis = (0..3)
            .min_by(|&a, &b| way[a].abs().total_cmp(&way[b].abs()))
            .unwrap_or(2);
        let mut normal = [0.0; 3];
        normal[axis] = 1.0;
        off(normal)
    })
}

fn is_finite(arc: &Arc) -> bool {
    [arc.start, arc.end, arc.hub, arc.normal]
        .iter()
        .flatten()
        .chain([&arc.radius, &arc.angle])
        .all(|value| value.is_finite())
}

fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: Vec3, by: f64) -> Vec3 {
    a.map(|axis| axis * by)
}

fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn norm(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}

fn normalized(a: Vec3) -> Option<Vec3> {
    let length = norm(a);
    (length > 1e-12 && length.is_finite()).then(|| scale(a, 1.0 / length))
}

#[cfg(test)]
mod tests {
    use super::*;

    const UP: Vec3 = [0.0, 0.0, 1.0];

    fn node(point: Vec3, handle: Vec3) -> Node {
        Node { point, handle }
    }

    fn near(a: Vec3, b: Vec3, tolerance: f64) -> bool {
        norm(sub(a, b)) <= tolerance
    }

    /// A small random number generator (xorshift), the same numbers every run.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }

        /// From -1 to 1.
        fn signed(&mut self) -> f64 {
            2.0 * self.next() - 1.0
        }

        fn vector(&mut self, reach: f64) -> Vec3 {
            [self.signed(), self.signed(), self.signed()].map(|axis| axis * reach)
        }
    }

    /// Every point of `arc` is its radius from its hub, in its plane, and it ends at its end.
    fn check_circle(arc: &Arc) {
        for step in 0..=16 {
            let point = arc.point(f64::from(step) / 16.0);
            let off = sub(point, arc.hub);
            assert!((norm(off) - arc.radius).abs() <= 1e-6 * arc.radius.max(1.0));
            assert!(dot(off, arc.normal).abs() <= 1e-6 * arc.radius.max(1.0));
        }
        assert!(near(arc.point(1.0), arc.end, 1e-6 * arc.radius.max(1.0)));
        assert!((norm(arc.normal) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_quarter_circle() {
        let arc = arc_through([0.0; 3], [1.0, 0.0, 0.0], [100.0, 100.0, 0.0], UP).unwrap();
        assert!((arc.radius - 100.0).abs() < 1e-9);
        assert!((arc.angle.to_degrees() - 90.0).abs() < 1e-9);
        assert!(near(arc.hub, [0.0, 100.0, 0.0], 1e-9));
        assert!(near(arc.normal, UP, 1e-12));
        assert!(near(arc.direction(0.0), [1.0, 0.0, 0.0], 1e-12));
        assert!(near(arc.direction(1.0), [0.0, 1.0, 0.0], 1e-12));
        assert!((arc.length() - 50.0 * core::f64::consts::PI).abs() < 1e-9);
        check_circle(&arc);
    }

    #[test]
    fn more_than_a_half_circle() {
        // Leaving along +X, ending behind and to the side: three quarters of a circle.
        let arc = arc_through([0.0; 3], [1.0, 0.0, 0.0], [-100.0, 100.0, 0.0], UP).unwrap();
        assert!((arc.angle.to_degrees() - 270.0).abs() < 1e-9);
        assert!((arc.radius - 100.0).abs() < 1e-9);
        assert!(near(arc.hub, [0.0, 100.0, 0.0], 1e-9));
        check_circle(&arc);
    }

    #[test]
    fn a_straight_line_is_a_slight_arc_ending_where_asked() {
        let arc = arc_through([0.0; 3], [1.0, 0.0, 0.0], [100.0, 0.0, 0.0], UP).unwrap();
        assert!((arc.angle.to_degrees() - MIN_ANGLE).abs() < 1e-9);
        assert!(arc.radius.is_finite() && arc.radius <= MAX_RADIUS);
        assert!(near(arc.end, [100.0, 0.0, 0.0], 1e-12));
        // In the plane it was given, a hair off its direction.
        assert!(near(arc.normal, UP, 1e-12));
        let off = dot(arc.direction(0.0), [1.0, 0.0, 0.0]).acos().to_degrees();
        assert!((off - MIN_ANGLE / 2.0).abs() < 1e-9);
        check_circle(&arc);
    }

    #[test]
    fn a_long_straight_line_keeps_within_the_largest_radius() {
        let arc = arc_through([0.0; 3], [1.0, 0.0, 0.0], [10_000.0, 0.0, 0.0], UP).unwrap();
        assert!(arc.radius <= MAX_RADIUS * (1.0 + 1e-12));
        assert!(near(arc.end, [10_000.0, 0.0, 0.0], 1e-9));
        check_circle(&arc);
        // Past twice the largest radius, no arc ends there.
        assert!(arc_through([0.0; 3], [1.0, 0.0, 0.0], [20_000.0, 0.0, 0.0], UP).is_none());
    }

    #[test]
    fn nearly_straight_arcs_keep_their_plane_until_clearly_bent() {
        let plane = [0.0, 1.0, 0.0];
        // Bent a hair toward +Y: too little to tell, the given plane stays (the arc bends toward
        // -Z, across it).
        let tiny = arc_through([0.0; 3], [1.0, 1e-5, 0.0], [100.0, 0.0, 0.0], plane).unwrap();
        assert!(near(tiny.normal, plane, 1e-9));
        // Clearly bent toward +Y: its own plane.
        let bent = arc_through([0.0; 3], [1.0, -0.05, 0.0], [100.0, 0.0, 0.0], plane).unwrap();
        assert!(near(bent.normal, UP, 1e-9));
        // A plane along the chord can't be kept: some plane across it is taken.
        let along = arc_through(
            [0.0; 3],
            [1.0, 0.0, 0.0],
            [100.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
        );
        assert!(dot(along.unwrap().normal, [1.0, 0.0, 0.0]).abs() < 1e-9);
    }

    #[test]
    fn arcs_that_cant_be_made() {
        let way = [1.0, 0.0, 0.0];
        assert!(arc_through([0.0; 3], way, [-100.0, 0.0, 0.0], UP).is_none());
        assert!(arc_through([0.0; 3], way, [1e-4, 0.0, 0.0], UP).is_none());
        assert!(arc_through([0.0; 3], [0.0; 3], [100.0, 0.0, 0.0], UP).is_none());
        assert!(arc_through([0.0; 3], [f64::NAN; 3], [100.0, 0.0, 0.0], UP).is_none());
        assert!(arc_through([0.0; 3], way, [f64::INFINITY, 0.0, 0.0], UP).is_none());
    }

    /// The span's two arcs start at `from` along its handle, end at `to` along its handle, and meet
    /// the same way.
    fn check_span(arcs: &[Arc; 2], from: Node, to: Node, tolerance: f64) {
        let [first, second] = arcs;
        check_circle(first);
        check_circle(second);
        assert!(near(first.start, from.point, 1e-9));
        assert!(near(second.end, to.point, 1e-9));
        assert!(near(first.end, second.start, 1e-9));
        assert!(near(first.direction(1.0), second.direction(0.0), tolerance));
        assert!(near(
            first.direction(0.0),
            normalized(from.handle).unwrap(),
            tolerance
        ));
        assert!(near(
            second.direction(1.0),
            normalized(to.handle).unwrap(),
            tolerance
        ));
    }

    #[test]
    fn a_c_shaped_span() {
        let from = node([0.0; 3], [100.0, 0.0, 0.0]);
        let to = node([0.0, 200.0, 0.0], [-100.0, 0.0, 0.0]);
        let arcs = biarc(from, to, [UP; 2]).unwrap();
        check_span(&arcs, from, to, 1e-9);
        // A half circle, in two quarters.
        for arc in &arcs {
            assert!((arc.radius - 100.0).abs() < 1e-6);
            assert!((arc.angle.to_degrees() - 90.0).abs() < 1e-6);
        }
    }

    #[test]
    fn an_s_shaped_span_meets_in_the_middle() {
        // The same direction across the chord: two half circles.
        let from = node([0.0; 3], [0.0, 50.0, 0.0]);
        let to = node([200.0, 0.0, 0.0], [0.0, 50.0, 0.0]);
        let arcs = biarc(from, to, [UP; 2]).unwrap();
        check_span(&arcs, from, to, 1e-9);
        assert!(near(arcs[0].end, [100.0, 0.0, 0.0], 1e-9));
        for arc in &arcs {
            assert!((arc.angle.to_degrees() - 180.0).abs() < 1e-6);
            assert!((arc.radius - 50.0).abs() < 1e-6);
        }
    }

    #[test]
    fn the_same_direction_with_noise_doesnt_swing_the_joint() {
        // As read from `f32` places: the same direction but for about 1e-7 radians.
        let from = node([0.0; 3], [0.0, 50.0, 0.0]);
        let mut joints = Vec::new();
        for noise in [
            [5e-6, 0.0, 0.0],
            [-5e-6, 0.0, 0.0],
            [0.0, 0.0, 5e-6],
            [0.0; 3],
        ] {
            let to = node([200.0, 0.0, 0.0], add([0.0, 50.0, 0.0], noise));
            joints.push(biarc(from, to, [UP; 2]).unwrap()[0].end);
        }
        for joint in &joints {
            assert!(near(*joint, joints[0], 1e-3), "{joints:?}");
        }
    }

    #[test]
    fn a_straight_span_is_two_slight_arcs() {
        let from = node([0.0; 3], [50.0, 0.0, 0.0]);
        let to = node([300.0, 0.0, 0.0], [50.0, 0.0, 0.0]);
        let arcs = biarc(from, to, [UP; 2]).unwrap();
        // Off the handles by at most half the least angle.
        check_span(&arcs, from, to, (MIN_ANGLE.to_radians() / 2.0).sin() * 1.01);
        for arc in &arcs {
            assert!(arc.radius <= MAX_RADIUS);
            assert!((arc.angle.to_degrees() - MIN_ANGLE).abs() < 1e-9);
        }
    }

    #[test]
    fn handles_weigh_where_the_arcs_meet() {
        let from = node([0.0; 3], [100.0, 0.0, 0.0]);
        let short = node([200.0, 200.0, 0.0], [0.0, 30.0, 0.0]);
        let long = node([200.0, 200.0, 0.0], [0.0, 300.0, 0.0]);
        let even = biarc(from, node(short.point, [0.0, 100.0, 0.0]), [UP; 2]).unwrap();
        let toward = biarc(from, short, [UP; 2]).unwrap();
        let away = biarc(from, long, [UP; 2]).unwrap();
        check_span(&toward, from, short, 1e-9);
        check_span(&away, from, long, 1e-9);
        // A longer handle at the end makes the second arc the longer one.
        assert!(away[1].length() > even[1].length());
        assert!(toward[1].length() < even[1].length());
    }

    #[test]
    fn spans_that_cant_be_joined() {
        // The same direction pointing back from one to the other: a loop.
        let from = node([0.0; 3], [100.0, 0.0, 0.0]);
        let behind = node([-200.0, 0.0, 0.0], [100.0, 0.0, 0.0]);
        assert!(biarc(from, behind, [UP; 2]).is_none());
        // Nearly so, slightly off: huge loops, refused too.
        let off = node([-200.0, 1.0, 0.0], [100.0, 0.1, 0.0]);
        assert!(biarc(from, off, [UP; 2]).is_none());
        // On top of each other, or a handle of no length.
        assert!(biarc(from, node([0.0; 3], [0.0, 1.0, 0.0]), [UP; 2]).is_none());
        assert!(biarc(from, node([100.0, 0.0, 0.0], [0.0; 3]), [UP; 2]).is_none());
    }

    /// How far off its given direction `arc` may leave (radians): none, or as far as the least
    /// angle or the largest radius hold it.
    fn held(arc: &Arc) -> f64 {
        let chord = norm(sub(arc.end, arc.start));
        let least = MIN_ANGLE.to_radians() / 2.0;
        let widest = (chord / (2.0 * MAX_RADIUS)).min(1.0).asin();
        if arc.angle <= 2.0 * least * (1.0 + 1e-6) || arc.radius >= MAX_RADIUS * (1.0 - 1e-9) {
            least.max(widest)
        } else {
            0.0
        }
    }

    /// The angle between two directions (radians).
    fn between(a: Vec3, b: Vec3) -> f64 {
        dot(a, b).clamp(-1.0, 1.0).acos()
    }

    #[test]
    fn random_spans_in_3d_are_smooth_and_finite() {
        let mut random = Random(0x9E37_79B9_7F4A_7C15);
        let (mut made, mut tried) = (0, 0);
        for _ in 0..20_000 {
            tried += 1;
            let reach = 10f64.powf(3.0 * random.next()); // 1 cm to 10 m
            let from = node(random.vector(reach), random.vector(1.0));
            let to = node(random.vector(reach), random.vector(1.0));
            let planes = [random.vector(1.0), random.vector(1.0)];
            let Some(arcs) = biarc(from, to, planes) else {
                continue;
            };
            made += 1;
            for arc in &arcs {
                assert!(is_finite(arc));
                assert!(arc.radius <= MAX_RADIUS * (1.0 + 1e-9));
                assert!(arc.angle >= MIN_ANGLE.to_radians() * (1.0 - 1e-9));
                assert!(arc.angle <= MAX_ARC.to_radians() * (1.0 + 1e-9));
            }
            // An arc held to the least angle or the largest radius leaves off the direction it
            // was given, by at most its `held` angle (the second arc then also arrives off by as
            // much as the first left): a slight kink where the arcs meet. The points stay exact.
            let [first, second] = arcs;
            let (start, joint) = (held(&first), held(&second));
            assert!(near(first.start, from.point, 1e-9));
            assert!(near(first.end, second.start, 1e-9));
            assert!(near(second.end, to.point, 1e-9));
            let leaving = normalized(from.handle).unwrap();
            let arriving = normalized(to.handle).unwrap();
            assert!(between(first.direction(0.0), leaving) <= start + 1e-6);
            assert!(between(first.direction(1.0), second.direction(0.0)) <= joint + 1e-6);
            assert!(between(second.direction(1.0), arriving) <= start + joint + 1e-6);
        }
        // Nearly all random spans can be joined.
        assert!(made * 100 > tried * 95, "{made} of {tried}");
    }

    #[test]
    fn moving_a_node_a_little_moves_the_arcs_a_little() {
        // No jumps: the curve follows a node dragged in small steps.
        let mut random = Random(42);
        for _ in 0..200 {
            let from = node(random.vector(300.0), random.vector(1.0));
            let mut to = node(random.vector(300.0), random.vector(1.0));
            let step = random.vector(0.01);
            let mut last: Option<[Arc; 2]> = None;
            for _ in 0..100 {
                to.point = add(to.point, step);
                let planes = last.map_or([UP; 2], |arcs| arcs.map(|arc| arc.normal));
                let Some(arcs) = biarc(from, to, planes) else {
                    last = None;
                    continue;
                };
                if let Some(previous) = last {
                    let chord = norm(sub(to.point, from.point));
                    // Only near the cases refused does the joint move fast.
                    let joint = norm(sub(arcs[0].end, previous[0].end));
                    assert!(joint < 0.5 + 0.01 * chord, "{joint}");
                }
                last = Some(arcs);
            }
        }
    }

    #[test]
    fn arcs_are_spinners_and_back() {
        let arc = arc_through([10.0, 20.0, 30.0], [0.0, 1.0, 1.0], [80.0, 50.0, 60.0], UP).unwrap();
        let spinner = arc.spinner([10.0, 5.0]);
        assert!(spinner.is_round());
        let axes = arc.axes();
        // A frame: unit axes square to each other, right-handed.
        for (a, axis) in axes.iter().enumerate() {
            assert!((norm(*axis) - 1.0).abs() < 1e-12);
            for other in &axes[a + 1..] {
                assert!(dot(*axis, *other).abs() < 1e-12);
            }
        }
        assert!(near(cross(axes[0], axes[1]), axes[2], 1e-12));
        // The spinner's own centre line, placed by the axes, is the arc.
        let placed = |local: Vec3| {
            add(
                arc.hub,
                add(
                    add(scale(axes[0], local[0]), scale(axes[1], local[1])),
                    scale(axes[2], local[2]),
                ),
            )
        };
        for step in 0..=8 {
            let along = f64::from(step) / 8.0;
            let local = spinner.point(spinner.radians() * along, 0.0, 0.0);
            assert!(near(placed(local), arc.point(along), 1e-9));
        }
        let back = Arc::of_spinner(arc.hub, axes, &spinner).unwrap();
        assert!(near(back.start, arc.start, 1e-9));
        assert!(near(back.end, arc.end, 1e-9));
        assert!(near(back.normal, arc.normal, 1e-12));
        assert!((back.angle - arc.angle).abs() < 1e-12);
    }

    /// `vector` turned by the quaternion `[x, y, z, w]` (as `FQuat` does).
    fn rotate([x, y, z, w]: [f64; 4], vector: Vec3) -> Vec3 {
        let axis = [x, y, z];
        let twice = scale(cross(axis, vector), 2.0);
        add(add(vector, scale(twice, w)), cross(axis, twice))
    }

    #[test]
    fn quaternions_turn_the_unit_axes_onto_the_axes() {
        let mut random = Random(7);
        let unit = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], UP];
        // Random frames, and half turns about each axis (Shepperd's other cases).
        let mut frames = vec![
            unit,
            [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]],
            [[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]],
            [[-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], UP],
        ];
        for _ in 0..1000 {
            let x = normalized(random.vector(1.0)).unwrap();
            let y = normalized(cross(random.vector(1.0), x)).unwrap();
            frames.push([x, y, cross(x, y)]);
        }
        for axes in frames {
            let quat = quaternion(axes);
            for (axis, turned) in unit.iter().zip(axes) {
                assert!(near(rotate(quat, *axis), turned, 1e-12), "{axes:?}");
            }
        }
    }

    #[test]
    fn placed_spinners_keep_facing_up() {
        let arc = arc_through([0.0; 3], [1.0, 0.0, 0.0], [100.0, 100.0, 0.0], UP).unwrap();
        for up in [UP, [0.0, 0.0, -1.0]] {
            let placement = arc.placement(up);
            assert!(dot(placement.axes[2], up) > 0.0);
            let spinner = Spinner {
                angle: placement.angle,
                ..arc.spinner([10.0, 10.0])
            };
            // The spinner's centre line, placed, is the arc either way.
            let [x, y, z] = placement.axes;
            for step in 0..=8 {
                let along = f64::from(step) / 8.0;
                let local = spinner.point(spinner.radians() * along, 0.0, 0.0);
                let placed = add(
                    arc.hub,
                    add(
                        add(scale(x, local[0]), scale(y, local[1])),
                        scale(z, local[2]),
                    ),
                );
                assert!(near(placed, arc.point(along), 1e-9));
            }
            // And its arc read back is the same.
            let back = Arc::of_spinner(arc.hub, placement.axes, &spinner).unwrap();
            assert!(near(back.end, arc.end, 1e-9));
            assert!(near(back.normal, arc.normal, 1e-12));
        }
        assert!(arc.placement([0.0, 0.0, -1.0]).angle < 0.0);
    }

    #[test]
    fn reversed_arcs_and_reaches() {
        let arc = arc_through([0.0; 3], [1.0, 0.0, 0.0], [100.0, 100.0, 0.0], UP).unwrap();
        let back = arc.reversed();
        check_circle(&back);
        assert!(near(back.point(0.25), arc.point(0.75), 1e-9));
        assert!(near(
            back.direction(0.0),
            scale(arc.direction(1.0), -1.0),
            1e-12
        ));
        // A quarter circle's tangents meet one radius from its ends.
        assert!((arc.reach().unwrap() - 100.0).abs() < 1e-9);
        let wide = arc_through([0.0; 3], [1.0, 0.0, 0.0], [-100.0, 100.0, 0.0], UP).unwrap();
        assert!(wide.reach().is_none());
        // A span's handles at its arcs' reaches give it back.
        let from = node([0.0; 3], [100.0, 0.0, 0.0]);
        let to = node([300.0, 200.0, 40.0], [0.0, 30.0, 10.0]);
        let [first, second] = biarc(from, to, [UP; 2]).unwrap();
        let again = biarc(
            node(
                from.point,
                scale(first.direction(0.0), first.reach().unwrap()),
            ),
            node(
                to.point,
                scale(second.direction(1.0), second.reach().unwrap()),
            ),
            [first.normal, second.normal],
        )
        .unwrap();
        assert!(near(again[0].end, first.end, 1e-6));
    }

    /// A chain of four arcs (two spans) through three nodes.
    fn four_arcs() -> Vec<Arc> {
        let nodes = [
            node([0.0; 3], [100.0, 0.0, 0.0]),
            node([300.0, 100.0, 50.0], [0.0, 100.0, 0.0]),
            node([300.0, 400.0, 0.0], [-100.0, 0.0, 100.0]),
        ];
        chain(&nodes, &[]).unwrap()
    }

    #[test]
    fn arcs_in_any_order_and_way_are_put_in_order() {
        let arcs = four_arcs();
        // Shuffled, two of them the other way.
        let given = [arcs[2], arcs[0].reversed(), arcs[3], arcs[1].reversed()];
        let chain = order(&given).unwrap();
        let ordered: Vec<Arc> = chain
            .iter()
            .map(|&(index, reversed)| {
                if reversed {
                    given[index].reversed()
                } else {
                    given[index]
                }
            })
            .collect();
        // From the first with a free end (the first arc gone the other way: its end is free).
        assert_eq!(chain[0], (1, true));
        for pair in ordered.windows(2) {
            assert!(near(pair[0].end, pair[1].start, 1e-9));
        }
        assert!(near(ordered[0].start, arcs[0].start, 1e-9));
        // One arc alone is a chain.
        assert_eq!(order(&arcs[..1]), Some(vec![(0, false)]));
    }

    #[test]
    fn arcs_that_make_no_chain() {
        let arcs = four_arcs();
        // Apart.
        assert!(order(&[arcs[0], arcs[2]]).is_none());
        // Branching: two arcs from the same end.
        let branch = arc_through(arcs[0].end, [0.0, 0.0, 1.0], [0.0, 0.0, 500.0], UP).unwrap();
        assert!(order(&[arcs[0], arcs[1], branch]).is_none());
        // Closed: a ring of two half circles.
        let half = arc_through([0.0; 3], [1.0, 0.0, 0.0], [0.0, 100.0, 0.0], UP).unwrap();
        let other = arc_through([0.0, 100.0, 0.0], [-1.0, 0.0, 0.0], [0.0; 3], UP).unwrap();
        assert!(order(&[half, other]).is_none());
        assert!(order(&[]).is_none());
    }

    #[test]
    fn a_curve_of_its_own_arcs_is_unchanged_until_moved() {
        let arcs = four_arcs();
        let curve = Curve::of_arcs(arcs.clone()).unwrap();
        assert_eq!(curve.nodes().len(), 3);
        // Moving a node to where it is gives the same arcs back.
        let same = curve.with_node(1, curve.nodes()[1].point).unwrap();
        for (arc, again) in arcs.iter().zip(same.arcs()) {
            assert!(near(arc.end, again.end, 1e-6));
            assert!(near(arc.hub, again.hub, 1e-6));
        }
    }

    #[test]
    fn moving_a_node_reshapes_the_spans_next_to_it() {
        let curve = Curve::of_arcs(four_arcs()).unwrap();
        let moved = curve.with_node(2, [350.0, 450.0, 20.0]).unwrap();
        // The first span stays, the second ends at the node, still smooth at the middle node.
        assert_eq!(moved.arcs()[..2], curve.arcs()[..2]);
        assert!(near(moved.arcs()[3].end, [350.0, 450.0, 20.0], 1e-9));
        assert!(near(
            moved.arcs()[1].direction(1.0),
            moved.arcs()[2].direction(0.0),
            1e-9
        ));
        // Moving the middle node changes both spans, the ends stay.
        let middle = curve.with_node(1, [250.0, 150.0, 0.0]).unwrap();
        assert!(near(middle.arcs()[0].start, [0.0; 3], 1e-9));
        assert!(near(middle.arcs()[3].end, curve.arcs()[3].end, 1e-9));
        assert!(near(middle.arcs()[1].end, [250.0, 150.0, 0.0], 1e-9));
        // Onto another node: no span; the curve stays as it was (`None`).
        assert!(curve.with_node(1, [0.0; 3]).is_none());
    }

    #[test]
    fn handles_turn_and_weigh_the_curve() {
        let curve = Curve::of_arcs(four_arcs()).unwrap();
        assert!(curve.handle(0, Side::Back).is_none());
        assert!(curve.handle(2, Side::Ahead).is_none());
        let tip = add(curve.nodes()[1].point, [0.0, 0.0, 80.0]);
        let turned = curve.with_handle(1, Side::Ahead, tip).unwrap();
        let node = turned.nodes()[1];
        assert!(near(node.direction, UP, 1e-12));
        assert!((node.reach[1] - 80.0).abs() < 1e-9);
        assert!(near(turned.handle(1, Side::Ahead).unwrap(), tip, 1e-9));
        // Its back handle turned with it, keeping its length.
        let back = turned.handle(1, Side::Back).unwrap();
        assert!(near(back, sub(node.point, scale(UP, node.reach[0])), 1e-9));
        assert!(near(turned.arcs()[1].direction(1.0), UP, 1e-9));
        assert!(near(turned.arcs()[2].direction(0.0), UP, 1e-9));
        // Too short a handle changes nothing.
        let short = add(curve.nodes()[1].point, [0.0, 0.0, 0.5]);
        assert!(curve.with_handle(1, Side::Ahead, short).is_none());
    }

    #[test]
    fn an_odd_chain_ends_in_one_arc() {
        // Three arcs: a span of two, then one alone.
        let mut arcs = four_arcs();
        arcs.pop();
        let curve = Curve::of_arcs(arcs).unwrap();
        assert_eq!(curve.nodes().len(), 3);
        assert!(curve.handle(1, Side::Ahead).is_some());
        assert!(curve.handle(2, Side::Back).is_none());
        // Its end goes anywhere, the arc bending to it from the middle node's way.
        let bent = curve.with_node(2, [400.0, 300.0, 0.0]).unwrap();
        let last = bent.arcs()[2];
        assert!(near(last.end, [400.0, 300.0, 0.0], 1e-9));
        assert!(near(last.direction(0.0), bent.nodes()[1].direction, 1e-9));
        assert!(near(bent.nodes()[2].direction, last.direction(1.0), 1e-12));
        // One spinner alone: its start's handle turns it, its end bends it.
        let one = Curve::of_arcs(vec![four_arcs()[0]]).unwrap();
        assert_eq!(one.nodes().len(), 2);
        let tip = add(one.nodes()[0].point, [0.0, 50.0, 0.0]);
        let turned = one.with_handle(0, Side::Ahead, tip).unwrap();
        assert!(near(turned.arcs()[0].direction(0.0), [0.0, 1.0, 0.0], 1e-9));
        assert!(near(turned.arcs()[0].end, one.arcs()[0].end, 1e-9));
    }

    #[test]
    fn directions_laid_onto_a_surface() {
        let laid = along_surface([1.0, 0.0, -1.0], UP).unwrap();
        assert!(near(laid, [1.0, 0.0, 0.0], 1e-12));
        // Already along it: the same.
        assert!(near(
            along_surface([0.0, 1.0, 0.0], UP).unwrap(),
            [0.0, 1.0, 0.0],
            1e-12
        ));
        // Nearly straight into it: no way along to tell.
        assert!(along_surface([0.01, 0.0, -1.0], UP).is_none());
        assert!(along_surface([1.0, 0.0, 0.0], [0.0; 3]).is_none());
    }

    #[test]
    fn a_node_going_another_way() {
        let curve = Curve::of_arcs(four_arcs()).unwrap();
        let point = curve.nodes()[1].point;
        let turned = curve.with_node_along(1, point, [0.0, 0.0, 3.0]).unwrap();
        let node = turned.nodes()[1];
        assert!(near(node.direction, UP, 1e-12));
        assert_eq!(node.reach, curve.nodes()[1].reach);
        assert!(near(turned.arcs()[1].direction(1.0), UP, 1e-9));
        assert!(near(turned.arcs()[2].direction(0.0), UP, 1e-9));
        assert!(curve.with_node_along(1, point, [0.0; 3]).is_none());
        // Its own way: the same as moving it.
        let moved = [250.0, 150.0, 0.0];
        let along = curve.with_node_along(1, moved, curve.nodes()[1].direction);
        assert_eq!(along, curve.with_node(1, moved));
    }

    /// Outside a ball of radius 10 about the origin.
    fn outside_ball(point: Vec3) -> bool {
        norm(point) >= 10.0
    }

    #[test]
    fn the_target_itself_when_allowed() {
        let target = [20.0, 0.0, 0.0];
        assert_eq!(
            nearest_allowed([0.0, 30.0, 0.0], target, 0.01, outside_ball),
            Some(target)
        );
        // From where it isn't allowed: nothing to go from.
        assert_eq!(
            nearest_allowed([0.0; 3], [1.0, 0.0, 0.0], 0.01, outside_ball),
            None
        );
    }

    #[test]
    fn the_nearest_point_slides_round_what_isnt_allowed() {
        // In the ball, near its side: the nearest allowed point is on that side, though the line
        // from where it starts meets the ball elsewhere.
        let target = [1.0, 0.0, 0.0];
        let reached = nearest_allowed([0.0, 20.0, 0.0], target, 0.01, outside_ball).unwrap();
        assert!(outside_ball(reached));
        assert!(norm(sub(reached, target)) < 9.0 + 0.01, "{reached:?}");
        // Allowed on one side of a slanted plane, the target far off the other side: the point
        // nearest it on the plane.
        let normal = normalized([1.0, 1.0, 0.0]).unwrap();
        let in_front = |point: Vec3| dot(point, normal) >= 0.0;
        let target = [-100.0, 50.0, 30.0];
        let reached = nearest_allowed([0.0; 3], target, 0.01, in_front).unwrap();
        let nearest = sub(target, scale(normal, dot(target, normal)));
        assert!(in_front(reached));
        assert!(near(reached, nearest, 0.5), "{reached:?} {nearest:?}");
    }

    #[test]
    fn the_nearest_point_is_allowed_never_further_and_settles() {
        let mut random = Random(11);
        for _ in 0..2000 {
            // Some balls not allowed in.
            let balls: Vec<(Vec3, f64)> = (0..3)
                .map(|_| (random.vector(50.0), 5.0 + 20.0 * random.next()))
                .collect();
            let allowed = |point: Vec3| {
                balls
                    .iter()
                    .all(|&(center, radius)| norm(sub(point, center)) >= radius)
            };
            let from = random.vector(100.0);
            let target = random.vector(100.0);
            let mut tries = 0;
            let counted = |point: Vec3| {
                tries += 1;
                allowed(point)
            };
            let Some(reached) = nearest_allowed(from, target, 0.01, counted) else {
                assert!(!allowed(from));
                continue;
            };
            assert!(tries <= MOST_TRIES);
            assert!(allowed(reached));
            let distance = norm(sub(target, reached));
            assert!(distance <= norm(sub(target, from)) + 1e-9);
            // Looked for again from there, it barely moves: no creeping from frame to frame.
            let again = nearest_allowed(reached, target, 0.01, allowed).unwrap();
            assert!(norm(sub(target, again)) <= distance + 1e-9);
            let settled = nearest_allowed(again, target, 0.01, allowed).unwrap();
            assert!(near(settled, again, 0.5), "{again:?} {settled:?}");
        }
    }

    #[test]
    fn a_handle_held_back_by_a_bend_too_tight() {
        let curve = Curve::of_arcs(four_arcs()).unwrap();
        // The arcs may bend no tighter than a radius of 40 cm.
        let fits = |curve: &Curve| curve.arcs().iter().all(|arc| arc.radius >= 40.0);
        assert!(fits(&curve));
        let tip = curve.handle(1, Side::Ahead).unwrap();
        // Turned hard to the side and short: too tight.
        let target = add(curve.nodes()[1].point, [-5.0, 0.0, -5.0]);
        let shaped = |at: Vec3| curve.with_handle(1, Side::Ahead, at).filter(fits);
        assert!(shaped(target).is_none());
        let reached = nearest_allowed(tip, target, 0.01, |at| shaped(at).is_some()).unwrap();
        let held = shaped(reached).unwrap();
        assert!(norm(sub(target, reached)) < norm(sub(target, tip)));
        // As far as it goes: the tightest arc is about as tight as allowed.
        let tightest = held
            .arcs()
            .iter()
            .map(|arc| arc.radius)
            .fold(f64::MAX, f64::min);
        assert!(tightest < 41.0, "{tightest}");
    }

    #[test]
    fn a_spinner_turning_the_other_way() {
        let spinner = Spinner {
            radius: [100.0; 2],
            size: [10.0; 2],
            angle: -90.0,
        };
        let axes = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], UP];
        let arc = Arc::of_spinner([0.0; 3], axes, &spinner).unwrap();
        // Its end is the spinner's: from +Y toward +X for a negative angle.
        let end = spinner.point(spinner.radians(), 0.0, 0.0);
        assert!(near(arc.end, end, 1e-9));
        assert!(near(arc.normal, [0.0, 0.0, -1.0], 1e-12));
        // Elliptic ones and closed rings aren't arcs.
        let elliptic = Spinner {
            radius: [100.0, 50.0],
            ..spinner
        };
        assert!(Arc::of_spinner([0.0; 3], axes, &elliptic).is_none());
        let ring = Spinner {
            angle: 360.0,
            ..spinner
        };
        assert!(Arc::of_spinner([0.0; 3], axes, &ring).is_none());
    }

    #[test]
    fn a_chain_has_two_arcs_per_span() {
        let nodes = [
            node([0.0; 3], [100.0, 0.0, 0.0]),
            node([300.0, 100.0, 50.0], [0.0, 100.0, 0.0]),
            node([300.0, 400.0, 0.0], [-100.0, 0.0, 100.0]),
        ];
        let arcs = chain(&nodes, &[]).unwrap();
        assert_eq!(arcs.len(), 4);
        // The spans meet at the middle node, the same way.
        assert!(near(arcs[1].end, arcs[2].start, 1e-9));
        assert!(near(arcs[1].direction(1.0), arcs[2].direction(0.0), 1e-9));
        assert!(chain(&nodes[..1], &[]).unwrap().is_empty());
    }
}
