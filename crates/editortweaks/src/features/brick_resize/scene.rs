//! The editor's bricks as boxes (and models) in the vehicle, to find what the mouse points at.
//! Gathered when the resize key is pressed (a reflected call per brick, and reading its place) and
//! kept while it's held: nothing else moves bricks meanwhile but this feature, which updates the
//! bricks it changes. Models are added the first time the model key is held meanwhile.

use core::cmp::Ordering;

use super::draw::PixelSize;
use super::geometry::{self, Bounds, Frame, Ray, Vec3};
use super::model::{BrickModel, Element, Pointing};
use super::parts::{self, Part};
use crate::game::weak_object::WeakObject;

/// A brick and its box.
pub struct Placed {
    pub object: WeakObject,
    /// Its own axes in the vehicle.
    pub frame: Frame,
    /// Its box in its own space, as it reports it.
    pub bounds: Bounds,
    /// Whether it has a `BrickSize` to resize.
    pub scalable: bool,
    /// The axes it's mirrored along (its box reported inside out: a negative size).
    pub mirrored: [bool; 3],
    /// Its model, once models are added (see [`Scene::add_models`]), if it has one that fits its
    /// box.
    pub model: Option<BrickModel>,
    /// Whether a move moves it (its frame is then where the move shows it).
    pub moving: bool,
    /// Whether it's the mirror mode counterpart of a brick a move moves: it moves along, mirrored
    /// (its frame is then where the move shows it too).
    pub mirroring: bool,
}

/// What the mouse is doing this frame, in the vehicle.
#[derive(Clone, Copy)]
pub struct View {
    /// Screen position (pixels).
    pub mouse: [f32; 2],
    /// The ray from the camera through it.
    pub ray: Ray,
    pub pixels: PixelSize,
}

/// What the mouse points at on a brick: a part of its box, or of its model.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Piece {
    Box(Part),
    Model(Element),
}

/// A piece of a brick the mouse points at.
pub struct Pointed {
    /// Which brick (see [`Scene::get`]).
    pub index: usize,
    pub piece: Piece,
    /// The point of the brick under the mouse, in the vehicle: where the ray enters its box (or
    /// meets its model), or the piece's point nearest to the ray when the mouse is beside it.
    pub point: Vec3,
}

/// How the mouse points at a brick it may pick.
enum Near {
    /// The ray enters its box through `face`, `distance` along.
    Box { face: Part, distance: f64 },
    /// The mouse is on `piece`, at `point`: on its model, or beside its box or model.
    At { piece: Piece, point: Vec3 },
}

pub struct Scene {
    /// Where the vehicle is in the world: bricks' places are in its space.
    pub vehicle: Frame,
    bricks: Vec<Placed>,
    /// Whether models were added.
    models: bool,
}

impl Scene {
    pub fn new(vehicle: Frame, bricks: Vec<Placed>) -> Self {
        Self {
            vehicle,
            bricks,
            models: false,
        }
    }

    pub fn len(&self) -> usize {
        self.bricks.len()
    }

    pub fn get(&self, index: usize) -> &Placed {
        &self.bricks[index]
    }

    /// Every brick (by index, see [`Self::get`]).
    pub fn bricks(&self) -> &[Placed] {
        &self.bricks
    }

    /// Whether models were added (see [`Self::add_models`]).
    pub fn has_models(&self) -> bool {
        self.models
    }

    /// Gives every brick its model, as `model_of` finds it; how many have one.
    pub fn add_models(&mut self, mut model_of: impl FnMut(&Placed) -> Option<BrickModel>) -> usize {
        self.models = true;
        for placed in &mut self.bricks {
            placed.model = model_of(placed);
        }
        self.bricks
            .iter()
            .filter(|placed| placed.model.is_some())
            .count()
    }

    /// What the mouse points at: a part of a box the ray enters ([`parts::part_under`]), or an
    /// edge or corner of a box the mouse is just beside ([`parts::beside`]); with `models`, the
    /// same on bricks' models instead of their boxes where they have one (see
    /// [`BrickModel::pointed`]). The bricks `preferred` says come first; then the nearest along the
    /// ray, where a brick the ray meets goes ahead of edges beside others less than
    /// [`parts::EDGE_PIXELS`] in front of it (an edge two neighbors share is the one under the
    /// mouse's). `depth` skips that many of them, to reach bricks behind (back to the first past
    /// the last). Bricks `skip` says are left out; `project` shows a point of the vehicle on screen.
    pub fn pick(
        &self,
        view: &View,
        models: bool,
        skip: impl Fn(&Placed) -> bool,
        preferred: impl Fn(&Placed) -> bool,
        depth: usize,
        project: impl Fn(Vec3) -> Option<[f32; 2]>,
    ) -> Option<Pointed> {
        let (ray, mouse, pixels) = (&view.ray, view.mouse, view.pixels);
        // Each brick the mouse may point at: its rank (not preferred, distance along the ray:
        // lowest first; `false` comes before `true`), the brick, how.
        let mut found: Vec<((bool, f64), usize, Near)> = Vec::new();
        let edge = f64::from(parts::EDGE_PIXELS) * pixels.per_distance;
        for (index, placed) in self.bricks.iter().enumerate() {
            if skip(placed) {
                continue;
            }
            let local = placed.frame.local_ray(ray);
            // Boxes the ray passes further than the edge distance from (at their far end) are left
            // out without projecting them.
            let center = placed.frame.point(placed.bounds.point([0; 3]));
            let radius =
                geometry::length(geometry::sub(placed.bounds.max, placed.bounds.min)) / 2.0;
            let far = geometry::length(geometry::sub(center, ray.origin)) + radius;
            let margin = edge * far;
            if geometry::ray_box(&local, &placed.bounds.grown(margin)).is_none() {
                continue;
            }
            let model = placed.model.as_ref().filter(|_| models);
            let (distance, near) = if let Some(model) = model {
                let Some(pointing) = model.pointed(&local, &local_pixels(&placed.frame, &pixels))
                else {
                    continue;
                };
                match pointing {
                    Pointing::On {
                        element,
                        point,
                        distance,
                    } => (
                        distance - edge * distance,
                        Near::At {
                            piece: Piece::Model(element),
                            point: placed.frame.point(point),
                        },
                    ),
                    Pointing::Beside { element, point } => {
                        let point = placed.frame.point(point);
                        let distance =
                            geometry::dot(geometry::sub(point, ray.origin), ray.direction);
                        let piece = Piece::Model(element);
                        (distance, Near::At { piece, point })
                    }
                }
            } else {
                match geometry::ray_box(&local, &placed.bounds) {
                    Some((distance, axis, positive)) => {
                        let ahead = edge * distance;
                        let face = Part::face(axis, positive);
                        (distance - ahead, Near::Box { face, distance })
                    }
                    None => {
                        let Some((part, point)) =
                            parts::beside(&placed.frame, &placed.bounds, ray, mouse, &project)
                        else {
                            continue;
                        };
                        let distance =
                            geometry::dot(geometry::sub(point, ray.origin), ray.direction);
                        let piece = Piece::Box(part);
                        (distance, Near::At { piece, point })
                    }
                }
            };
            found.push(((!preferred(placed), distance), index, near));
        }
        if found.is_empty() {
            return None;
        }
        found.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));
        let (_, index, near) = found.swap_remove(depth % found.len());
        let placed = &self.bricks[index];
        Some(match near {
            Near::Box { face, distance } => Pointed {
                index,
                piece: Piece::Box(parts::part_under(
                    &placed.frame,
                    &placed.bounds,
                    face,
                    mouse,
                    &project,
                )),
                point: geometry::along(ray, distance),
            },
            Near::At { piece, point } => Pointed {
                index,
                piece,
                point,
            },
        })
    }

    /// Replaces what's known of a brick (by its object).
    pub fn update(&mut self, placed: Placed) {
        if let Some(object) = placed.object.get()
            && let Some(known) = self.bricks.iter_mut().find(|known| known.object.is(object))
        {
            *known = placed;
        }
    }
}

/// `pixels` (in the vehicle) in a brick's `frame`.
fn local_pixels(frame: &Frame, pixels: &PixelSize) -> PixelSize {
    PixelSize {
        camera: frame.local(pixels.camera),
        per_distance: pixels.per_distance,
        right: frame.local_direction(pixels.right),
        down: frame.local_direction(pixels.down),
    }
}
