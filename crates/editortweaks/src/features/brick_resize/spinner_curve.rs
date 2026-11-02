//! Selected spinners shaped as one smooth curve (see `model_io::curve`). With fast editing on in
//! the editor's default mode, and the selected spinners a chain of round ones joined cap to cap
//! (open, not a ring; any order, each either way along), the curve's controls show: its nodes (the
//! start of every other spinner, and the chain's end) and their handles. Dragging a node moves it,
//! dragging a handle's tip turns the curve there (both handles of a node turn together) and its
//! length weighs where the span's two arcs meet. With the snap key, the control snaps like any
//! drag (a handle then points at what it snaps to); a node snapped onto a face lies along it: the
//! curve touches the face there rather than going into it.
//!
//! Only the spinners of the spans next to the dragged control change: their radius, angle and
//! place. Their size and cross-section stay, and their cross-sections keep facing up as before
//! ([`model_io::curve::Arc::placement`]). No brick is ever made or deleted. Where the spinners
//! can't take the shape the mouse asks for (two nodes on top of each other, a loop, a bend too
//! tight for a spinner's thickness), the control goes as near it as they can
//! ([`curve::nearest_allowed`]), a dashed line on to where the mouse is. With mirror mode on, the
//! counterparts follow, mirrored (but those in the chain themselves: an arch across the mirror
//! plane is edited as one curve).

use brickrust::ue::coreuobject::UObjectBase;
use model_io::curve::{self, Arc, Curve, Side};

use super::draw::{self, Drawing, Mark};
use super::geometry::{self, Vec3};
use super::parts;
use super::scene::{Scene, View};
use super::{Game, LOG_NO_INTERFACE};
use crate::game::spinner::{Property, Values};
use crate::game::weak_object::WeakObject;
use crate::math::{self, Quat};
use crate::prelude::*;

/// What a curve's change tells each spinner changed.
const CHANGES: &[Property] = &[Property::Radius, Property::Angle];
/// How many segments each arc of the curve is drawn with, at most (fewer for small angles).
const ARC_SEGMENTS: f64 = 24.0;
/// The smallest cosine between the camera's ray and the plane a control moves in.
const MIN_COSINE: f64 = 0.05;
/// How near (cm) a held back control gets to the nearest point the spinners can take.
const NEAREST: f64 = 0.01;
/// How far (pixels) a held back control must be from where the mouse asks for it to show the way
/// on (see [`draw::CURVE_HELD`]).
const HELD_PIXELS: f64 = 3.0;

/// One of a curve's controls.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Control {
    /// A node (see [`Curve::nodes`]).
    Node(usize),
    /// The tip of a node's handle.
    Handle(usize, Side),
}

/// A spinner of a chain.
#[derive(Clone)]
struct Link {
    object: WeakObject,
    /// Its values as they were: its cross-section and size stay.
    stored: Values,
    /// The thickness it shows (cm, see `model_io::spinner::Spinner::thickness`): its arc's radius
    /// stays at least half of it.
    thickness: f64,
    /// Whether it goes from its end to its start along the chain.
    reversed: bool,
    /// Its own Z axis as last placed: its cross-section keeps facing it as near as can be.
    up: Vec3,
}

/// The selected spinners as a curve.
#[derive(Clone)]
pub struct Chain {
    links: Vec<Link>,
    curve: Curve,
}

impl Chain {
    /// The chain the scene's spinners `selected` (by index) make, if they're all round, not
    /// closed rings, and joined cap to cap into one open chain.
    pub fn of(scene: &Scene, selected: &[usize]) -> Option<Chain> {
        let mut arcs = Vec::with_capacity(selected.len());
        let mut bricks = Vec::with_capacity(selected.len());
        for &index in selected {
            let placed = scene.get(index);
            let brick = placed.spinner.as_ref()?;
            let frame = &placed.frame;
            arcs.push(Arc::of_spinner(frame.origin, frame.axes, &brick.spinner)?);
            let thickness = brick.spinner.thickness();
            bricks.push((placed.object, brick.stored, thickness, frame.axes[2]));
        }
        let order = curve::order(&arcs)?;
        let mut links = Vec::with_capacity(order.len());
        let mut ordered = Vec::with_capacity(order.len());
        for (index, reversed) in order {
            let (object, stored, thickness, up) = bricks[index];
            links.push(Link {
                object,
                stored,
                thickness,
                reversed,
                up,
            });
            let arc = arcs[index];
            ordered.push(if reversed { arc.reversed() } else { arc });
        }
        let curve = Curve::of_arcs(ordered)?;
        Some(Chain { links, curve })
    }

    /// How many spinners it has.
    pub fn len(&self) -> usize {
        self.links.len()
    }

    /// The control under the mouse (at `view`): the nearest on the screen within
    /// [`parts::CORNER_PIXELS`], in front of the camera.
    pub fn control_under(&self, view: &View) -> Option<Control> {
        controls(&self.curve)
            .filter_map(|(control, point)| {
                let ahead =
                    geometry::dot(geometry::sub(point, view.ray.origin), view.ray.direction);
                let pixels = geometry::ray_distance(&view.ray, point) / view.pixels.at(point);
                (ahead > 0.0 && pixels <= f64::from(parts::CORNER_PIXELS))
                    .then_some((control, pixels))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(control, _)| control)
    }

    /// Shows the curve: its centre line, its nodes and handles; `lit` the one under the mouse.
    pub fn draw(&self, drawing: &mut Drawing, lit: Option<Control>) {
        draw_curve(drawing, &self.curve, lit);
    }
}

/// A curve's control being dragged.
pub struct Editing {
    pub control: Control,
    chain: Chain,
    /// The curve as last asked for (and written).
    curve: Curve,
    /// The arc each spinner was given last (none yet: as grabbed).
    written: Vec<Option<Arc>>,
    /// The links' mirror mode counterparts (none for those in the chain), and the mirror axis.
    counterparts: Vec<Option<WeakObject>>,
    axis: Option<u8>,
    /// The plane the control moves in when not snapped: through where it was grabbed, facing the
    /// camera as it was then.
    plane: (Vec3, Vec3),
    /// Where the mouse was pressed (screen pixels), until it moves away: a click changes nothing.
    pub pressed_at: Option<[f32; 2]>,
    /// Where the mouse last asked for the control, when the spinners couldn't take it there.
    held_back: Option<Vec3>,
}

impl Editing {
    /// Starts dragging `control` of `chain` with the mouse at `view`; `None` if it has no such
    /// control.
    ///
    /// # Safety
    /// The chain's spinners must be the live editor's (or gone). Game thread only.
    pub unsafe fn new(game: &Game, chain: &Chain, control: Control, view: &View) -> Option<Self> {
        let grabbed = point_of(&chain.curve, control)?;
        let axis = game.mirror.as_ref().and_then(|mirror| {
            // SAFETY: game thread.
            unsafe { mirror.axis() }
        });
        let counterparts = chain
            .links
            .iter()
            .map(|link| {
                let object = link.object.get()?;
                axis?;
                // SAFETY: per the contract; `object` is a live spinner.
                let other = unsafe { game.code.mirrored_object(object)? };
                let in_chain = chain.links.iter().any(|link| link.object.is(other));
                // SAFETY: `other` is the live counterpart.
                (!in_chain).then(|| unsafe { WeakObject::new(other) })
            })
            .collect();
        Some(Editing {
            control,
            written: vec![None; chain.links.len()],
            chain: chain.clone(),
            curve: chain.curve.clone(),
            counterparts,
            axis,
            plane: (grabbed, view.ray.direction),
            pressed_at: Some(view.mouse),
            held_back: None,
        })
    }

    /// Where the dragged control is now.
    pub fn grabbed(&self) -> Vec3 {
        point_of(&self.curve, self.control).unwrap_or(self.plane.0)
    }

    /// Where the control goes for the mouse at `view`, unsnapped: in its plane.
    pub fn followed(&self, view: &View) -> Option<Vec3> {
        let (point, normal) = self.plane;
        let distance = geometry::ray_plane(&view.ray, point, normal, MIN_COSINE)?;
        Some(geometry::along(&view.ray, distance))
    }

    /// Takes the control to `point`, or as near it as the spinners can take, from where it is; a
    /// node `on` a face facing that way lies along it (if that leaves it anywhere to go). Whether
    /// the curve changed.
    pub fn aim(&mut self, point: Vec3, on: Option<Vec3>) -> bool {
        let from = self.grabbed();
        let (laid, unlaid) = (self.laid(on), self.laid(None));
        let others = (laid != unlaid).then_some(unlaid);
        let reached = core::iter::once(laid).chain(others).find_map(|direction| {
            let shaped = |at: Vec3| self.shaped(at, direction).is_some();
            let at = curve::nearest_allowed(from, point, NEAREST, shaped)?;
            Some((at, self.shaped(at, direction)?))
        });
        let Some((at, curve)) = reached else {
            self.held_back = Some(point);
            return false;
        };
        self.held_back = (at != point).then_some(point);
        if curve == self.curve {
            return false;
        }
        self.curve = curve;
        true
    }

    /// The way a dragged node goes: the way it went when grabbed, laid along a face facing `on`
    /// (unless it goes nearly straight into it); `None` for a handle.
    fn laid(&self, on: Option<Vec3>) -> Option<Vec3> {
        let Control::Node(node) = self.control else {
            return None;
        };
        let grabbed = self.chain.curve.nodes()[node].direction;
        Some(
            on.and_then(|normal| curve::along_surface(grabbed, normal))
                .unwrap_or(grabbed),
        )
    }

    /// The curve with the control at `at` (a node going `direction`), if the spinners can take it.
    fn shaped(&self, at: Vec3, direction: Option<Vec3>) -> Option<Curve> {
        let curve = match (self.control, direction) {
            (Control::Node(node), Some(direction)) => {
                self.curve.with_node_along(node, at, direction)
            }
            (Control::Node(node), None) => self.curve.with_node(node, at),
            (Control::Handle(node, side), _) => self.curve.with_handle(node, side, at),
        };
        curve.filter(|curve| self.fits(curve))
    }

    /// Whether every spinner still shows the thickness it showed along its arc of `curve`: else a
    /// final change would make the game take it down (see `game::spinner`).
    fn fits(&self, curve: &Curve) -> bool {
        let thicknesses = self.chain.links.iter().map(|link| link.thickness);
        curve
            .arcs()
            .iter()
            .zip(thicknesses)
            .all(|(arc, thickness)| arc.fits(thickness))
    }

    /// Whether the curve differs from the one grabbed.
    pub fn changed(&self) -> bool {
        self.curve != self.chain.curve
    }

    /// The chain's spinners and their counterparts.
    pub fn objects(&self) -> Vec<*mut UObjectBase> {
        let links = self.chain.links.iter().map(|link| &link.object);
        links
            .chain(self.counterparts.iter().flatten())
            .filter_map(WeakObject::get)
            .collect()
    }

    /// Whether `object` is one of the chain's spinners or their counterparts.
    pub fn has(&self, object: &WeakObject) -> bool {
        let links = self.chain.links.iter().map(|link| &link.object);
        // The same object in the same slot: nothing to resolve.
        links
            .chain(self.counterparts.iter().flatten())
            .any(|ours| ours == object)
    }

    /// Gives the spinners their arcs of the curve, and their counterparts too: while dragging
    /// (`interactive`), those whose arcs changed since last given (or since grabbed); finally,
    /// all those given one, telling them it's final. False if a spinner is gone or can't be told.
    ///
    /// # Safety
    /// The chain's spinners must be the live editor's (or gone). Game thread only.
    pub unsafe fn write(&mut self, game: &Game, interactive: bool) -> bool {
        let Some((spinners, _)) = &game.spinners else {
            return false;
        };
        let arcs = self.curve.arcs().iter().zip(self.chain.curve.arcs());
        for (index, (arc, grabbed)) in arcs.enumerate() {
            let written = self.written[index];
            let due = if interactive {
                written.map_or(arc != grabbed, |written| written != *arc)
            } else {
                written.is_some()
            };
            if !due {
                continue;
            }
            self.written[index] = Some(*arc);
            let link = &mut self.chain.links[index];
            let Some(object) = link.object.get() else {
                return false;
            };
            let arc = if link.reversed { arc.reversed() } else { *arc };
            let placement = arc.placement(link.up);
            link.up = placement.axes[2];
            let values = Values {
                radius: [arc.radius as f32; 2],
                angle: placement.angle as f32,
                ..link.stored
            };
            let [x, y, z, w] = curve::quaternion(placement.axes);
            let rotation = math::narrow(Quat { x, y, z, w }.to_rotator());
            let location = math::narrow(arc.hub);
            // SAFETY: per the contract; `object` is a live spinner.
            unsafe {
                let Some(kept) = spinners.write(object, &values, CHANGES, interactive) else {
                    LOG_NO_INTERFACE.call_once(|| {
                        log!("can't change spinners: bricks' property interface not found")
                    });
                    return false;
                };
                game.spawn.set(object, location, rotation);
                if let (Some(other), Some(axis), Some(mirror)) = (
                    self.counterparts[index].as_ref().and_then(WeakObject::get),
                    self.axis,
                    &game.mirror,
                ) {
                    let mirrored = Values {
                        angle: -kept.angle,
                        ..kept
                    };
                    spinners.write(other, &mirrored, CHANGES, interactive);
                    let (location, turned) =
                        mirror.mirrored_place(object, location, rotation, axis);
                    let rotation = math::narrow(Quat::widen(turned).to_rotator());
                    game.spawn.set(other, location, rotation);
                }
            }
        }
        true
    }

    /// Shows the curve as dragged, the dragged control lit, and the way on to where the mouse asks
    /// for it while the spinners hold it back.
    pub fn draw(&self, drawing: &mut Drawing) {
        draw_curve(drawing, &self.curve, Some(self.control));
        if let Some(wanted) = self.held_back {
            let at = self.grabbed();
            if drawing.pixels_between(at, wanted) >= HELD_PIXELS {
                drawing.dashed(at, wanted, draw::CURVE_HELD);
            }
        }
    }
}

/// Every control of `curve` and where it is: its nodes, then its handles' tips.
fn controls(curve: &Curve) -> impl Iterator<Item = (Control, Vec3)> + '_ {
    let nodes = curve.nodes().iter().enumerate();
    let handles = nodes.clone().flat_map(move |(node, _)| {
        [Side::Back, Side::Ahead]
            .into_iter()
            .filter_map(move |side| Some((Control::Handle(node, side), curve.handle(node, side)?)))
    });
    nodes
        .map(|(node, at)| (Control::Node(node), at.point))
        .chain(handles)
}

/// Where `control` of `curve` is.
fn point_of(curve: &Curve, control: Control) -> Option<Vec3> {
    match control {
        Control::Node(node) => curve.nodes().get(node).map(|node| node.point),
        Control::Handle(node, side) => curve.handle(node, side),
    }
}

/// Shows `curve`: its arcs' centre line, its handles (from their node to their tip), its nodes;
/// `lit` the control under the mouse or dragged.
fn draw_curve(drawing: &mut Drawing, curve: &Curve, lit: Option<Control>) {
    for arc in curve.arcs() {
        let segments = (arc.angle.to_degrees() / 360.0 * ARC_SEGMENTS * 4.0)
            .ceil()
            .clamp(1.0, ARC_SEGMENTS);
        let steps = segments as usize;
        for step in 0..steps {
            let along = |step: usize| step as f64 / segments;
            drawing.segment(
                arc.point(along(step)),
                arc.point(along(step + 1)),
                draw::CURVE,
            );
        }
    }
    let pen = |control: Control| {
        if lit == Some(control) {
            draw::PART
        } else {
            draw::CURVE_CONTROL
        }
    };
    for (control, point) in controls(curve) {
        match control {
            Control::Node(_) => {
                drawing.mark(Mark::Circle, point, draw::END_MARK_PIXELS, pen(control));
            }
            Control::Handle(node, _) => {
                let from = curve.nodes()[node].point;
                drawing.segment(from, point, draw::CURVE_HANDLE);
                let size = draw::END_MARK_PIXELS * 0.8;
                drawing.mark(Mark::Square, point, size, pen(control));
            }
        }
    }
}
