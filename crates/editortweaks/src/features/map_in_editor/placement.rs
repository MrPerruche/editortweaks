//! Where the game puts a vehicle spawned at a garage, redone from the game's code (see
//! `docs/re/map-in-editor.md`: `AGarage::GetVehicleSpawnBoxRelativeTransform` and
//! `ABrickVehicle::GetVehicleSpawnOffset`).
//!
//! The garage has a spawn box, half the size of `extent`, whose centre is the spawn point. The game
//! puts the centre of the vehicle's bounding box at the spawn point, moved back (-X) and down (-Z)
//! in the box's frame when the vehicle is smaller than the box on that axis, and turns the vehicle
//! like the box. Everything is in `f64`.

use crate::game::garage::Garage;
use crate::math::{self, Quat, Vec3};

/// The smallest and largest corners of the vehicle's bricks, in the vehicle's space.
#[derive(Clone, Copy, Debug)]
pub struct VehicleBox {
    pub min: Vec3,
    pub max: Vec3,
}

impl VehicleBox {
    fn half(&self) -> Vec3 {
        math::scale(math::sub(self.max, self.min), 0.5)
    }

    fn center(&self) -> Vec3 {
        math::scale(math::add(self.min, self.max), 0.5)
    }
}

/// A place in the world.
#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub location: Vec3,
    pub rotation: Quat,
}

/// Where a garage actor is in the world.
#[derive(Clone, Copy, Debug)]
pub struct GaragePlace {
    pub pose: Pose,
    pub scale: Vec3,
}

/// What spawning a vehicle at a garage does.
#[derive(Clone, Copy, Debug)]
pub struct Landing {
    /// Where the vehicle's origin ends up in the world, and how it's turned: where the editor goes
    /// to show the map as it will be around the vehicle.
    pub vehicle: Pose,
    /// The spawn point (the spawn box's centre) in the vehicle's space.
    pub spawn_point: Vec3,
}

/// Whether `garage` has a pinned spawn point of its own (`PinnedVehicleSpawnOrigin` isn't zero):
/// only then does spawning pinned land elsewhere.
pub fn has_pinned_point(garage: &Garage) -> bool {
    garage.pinned_origin != [0.0; 3]
}

/// Where a vehicle with `vehicle`'s bounding box lands when spawned at `garage` (standing at
/// `place`); `pinned` for the pinned spawn point, if the garage has one.
pub fn land(garage: &Garage, place: &GaragePlace, pinned: bool, vehicle: &VehicleBox) -> Landing {
    let extent = math::widen(garage.extent);
    let box_rotation = Quat::from_rotator(math::widen(garage.rotation));
    let pinned_origin = math::widen(garage.pinned_origin);
    let use_pinned = pinned && has_pinned_point(garage);
    let base = if use_pinned {
        pinned_origin
    } else {
        math::widen(garage.offset)
    };
    // In front of the building (along the garage's X), by the box's reach along its own forward.
    let along = if use_pinned {
        0.0
    } else {
        math::dot(extent, box_rotation.rotate([1.0, 0.0, 0.0])).abs()
    };
    let building_origin = math::widen(garage.bounds_origin);
    let building_extent = math::widen(garage.bounds_extent);
    let relative = [
        building_origin[0] + building_extent[0] + along + base[0],
        base[1],
        base[2] + extent[2],
    ];
    let rotation = place.pose.rotation * box_rotation;
    let spawn_point = math::add(
        place.pose.location,
        place.pose.rotation.rotate([
            relative[0] * place.scale[0],
            relative[1] * place.scale[1],
            relative[2] * place.scale[2],
        ]),
    );

    // The vehicle's box centre goes to the spawn point, pushed to the box's back and floor where
    // the vehicle is smaller than the box (`GetVehicleSpawnOffset`; the garage's alignment).
    let (half, center) = (vehicle.half(), vehicle.center());
    let push = if math::length(extent) > 1e-3 {
        [
            -(extent[0] - half[0]).max(0.0),
            0.0,
            -(extent[2] - half[2]).max(0.0),
        ]
    } else {
        [0.0, 0.0, half[2]]
    };
    Landing {
        vehicle: Pose {
            location: math::add(spawn_point, rotation.rotate(math::sub(push, center))),
            rotation,
        },
        spawn_point: math::sub(center, push),
    }
}

/// How the game turns a vehicle spawned at the player's location: `FRotationMatrix::MakeFromZX(
/// normal, forward).Rotator()`, its up along the ground's `normal`, facing `forward` as far as that
/// allows.
pub fn on_surface(normal: Vec3, forward: Vec3) -> Quat {
    let z = math::normalized(normal).unwrap_or([0.0, 0.0, 1.0]);
    let mut x = math::normalized(forward).unwrap_or([1.0, 0.0, 0.0]);
    // Facing along the normal leaves no way to tell forward: the engine picks an axis.
    if math::dot(z, x).abs() > 1.0 - 1e-8 {
        x = if z[2].abs() < 1.0 - 1e-8 {
            [0.0, 0.0, 1.0]
        } else {
            [1.0, 0.0, 0.0]
        };
    }
    let y = math::normalized(math::cross(z, x)).unwrap_or([0.0, 1.0, 0.0]);
    let x = math::cross(y, z);
    // `FMatrix::Rotator`.
    let pitch = x[2].atan2((x[0] * x[0] + x[1] * x[1]).sqrt());
    let yaw = x[1].atan2(x[0]);
    let unrolled_y = [-yaw.sin(), yaw.cos(), 0.0];
    let roll = math::dot(z, unrolled_y).atan2(math::dot(y, unrolled_y));
    Quat::from_rotator([pitch.to_degrees(), yaw.to_degrees(), roll.to_degrees()])
}

/// How much smaller than the vehicle's box the box that's tested for room is, on each side.
const FIT_TOLERANCE: f64 = 5.0;

/// What a sweep of the vehicle's box hit: where its centre was, and whether it started inside.
#[derive(Clone, Copy, Debug)]
pub struct BoxHit {
    pub location: Vec3,
    pub started_inside: bool,
}

/// Where the game puts a vehicle spawned on the ground at `ground`, turned by `rotation`
/// (`ABrickVehicle::GetVehicleSpawnOffset` without a spawn point, for an unpinned vehicle): the
/// bottom of its box on the ground. If `blocked` says its box (centred where given, with the half
/// size given) has no room there, `sweep` (from, to, half size) brings it back from each side in
/// turn: right, left, forward, back, up. The first hit that didn't start inside anything places
/// it, moved back from what it hit. With none, it stays where it has no room.
pub fn fit_on_ground(
    ground: Vec3,
    rotation: Quat,
    vehicle: &VehicleBox,
    blocked: impl FnOnce(Vec3, Vec3) -> bool,
    mut sweep: impl FnMut(Vec3, Vec3, Vec3) -> Option<BoxHit>,
) -> Landing {
    let (half, middle) = (vehicle.half(), vehicle.center());
    let tested = [0, 1, 2].map(|axis| half[axis] - FIT_TOLERANCE);
    let mut center = math::add(ground, rotation.rotate([0.0, 0.0, half[2]]));
    if blocked(center, tested) {
        let size = math::scale(half, 2.0);
        let widest = 2.0 * half[0].max(half[1]).max(half[2]);
        let tries = [
            ([0.0, 1.0, 0.0], size[1] + half[1]),
            ([0.0, -1.0, 0.0], size[1] - half[1]),
            ([1.0, 0.0, 0.0], size[0] + half[0]),
            ([-1.0, 0.0, 0.0], size[0] - half[0]),
            ([0.0, 0.0, 1.0], widest + half[2]),
        ];
        for (direction, distance) in tries {
            if distance < 1e-4 {
                continue;
            }
            let along = rotation.rotate(direction);
            let from = math::add(center, math::scale(along, distance));
            let Some(hit) = sweep(from, center, tested).filter(|hit| !hit.started_inside) else {
                continue;
            };
            // 55 cm back along a side, 5 cm up.
            let back = 5.0 + (1.0 - f64::abs(direction[2])) * 50.0;
            center = math::add(hit.location, math::scale(along, back));
            break;
        }
    }
    Landing {
        vehicle: Pose {
            location: math::sub(center, rotation.rotate(middle)),
            rotation,
        },
        spawn_point: [middle[0], middle[1], middle[2] - half[2]],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VEHICLE: VehicleBox = VehicleBox {
        min: [-100.0, 10.0, 20.0],
        max: [300.0, 50.0, 80.0],
    };

    fn in_world(landing: &Landing, point: Vec3) -> Vec3 {
        math::add(
            landing.vehicle.location,
            landing.vehicle.rotation.rotate(point),
        )
    }

    fn near(a: Vec3, b: Vec3) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-6)
    }

    #[test]
    fn with_room_the_box_stands_on_the_ground() {
        let rotation = Quat::from_rotator([0.0, 90.0, 0.0]);
        let landing = fit_on_ground(
            [1000.0, 2000.0, 500.0],
            rotation,
            &VEHICLE,
            |_, tested| {
                assert!(near(tested, [195.0, 15.0, 25.0]), "{tested:?}");
                false
            },
            |_, _, _| panic!("no sweep with room"),
        );
        assert!(near(
            in_world(&landing, landing.spawn_point),
            [1000.0, 2000.0, 500.0]
        ));
        assert_eq!(landing.spawn_point, [100.0, 30.0, 20.0]);
    }

    #[test]
    fn without_room_it_comes_back_from_the_right_first() {
        let ground = [0.0, 0.0, 0.0];
        let mut sweeps = Vec::new();
        let landing = fit_on_ground(
            ground,
            Quat::from_rotator([0.0; 3]),
            &VEHICLE,
            |_, _| true,
            |from, to, _| {
                sweeps.push((from, to));
                Some(BoxHit {
                    location: [0.0, 40.0, 30.0],
                    started_inside: false,
                })
            },
        );
        // From 1.5 times its width to the right (half 20: 60 cm), down to where it should stand.
        assert_eq!(sweeps, [([0.0, 60.0, 30.0], [0.0, 0.0, 30.0])]);
        // Moved 55 cm further right than the hit, so its box's middle is at 95 cm.
        let middle = in_world(&landing, VEHICLE.center());
        assert!(near(middle, [0.0, 95.0, 30.0]), "{middle:?}");
    }

    #[test]
    fn sweeps_starting_inside_something_are_skipped_in_the_games_order() {
        let mut froms = Vec::new();
        let landing = fit_on_ground(
            [0.0; 3],
            Quat::from_rotator([0.0; 3]),
            &VEHICLE,
            |_, _| true,
            |from, _, _| {
                froms.push(from);
                match froms.len() {
                    5 => Some(BoxHit {
                        location: [0.0, 0.0, 100.0],
                        started_inside: false,
                    }),
                    2 => None,
                    _ => Some(BoxHit {
                        location: [0.0; 3],
                        started_inside: true,
                    }),
                }
            },
        );
        assert_eq!(
            froms,
            [
                [0.0, 60.0, 30.0],   // right: 40 + 20
                [0.0, -20.0, 30.0],  // left: 40 - 20
                [600.0, 0.0, 30.0],  // forward: 400 + 200
                [-200.0, 0.0, 30.0], // back: 400 - 200
                [0.0, 0.0, 460.0],   // up: 2 * 200 + 30
            ]
        );
        // Up only moves 5 cm back.
        assert!(near(
            in_world(&landing, VEHICLE.center()),
            [0.0, 0.0, 105.0]
        ));
    }

    #[test]
    fn nothing_found_leaves_it_where_it_has_no_room() {
        let landing = fit_on_ground(
            [7.0, 8.0, 9.0],
            Quat::from_rotator([0.0; 3]),
            &VEHICLE,
            |_, _| true,
            |_, _, _| None,
        );
        assert!(near(
            in_world(&landing, landing.spawn_point),
            [7.0, 8.0, 9.0]
        ));
    }

    #[test]
    fn on_a_slope_it_tilts_and_keeps_facing_the_players_way() {
        // A slope going up towards +X by 30 degrees; the player faces +X.
        let (sine, cosine) = 30f64.to_radians().sin_cos();
        let rotation = on_surface([-sine, 0.0, cosine], [1.0, 0.0, 0.0]);
        assert!(near(rotation.rotate([0.0, 0.0, 1.0]), [-sine, 0.0, cosine]));
        assert!(near(rotation.rotate([1.0, 0.0, 0.0]), [cosine, 0.0, sine]));
        assert!(near(rotation.rotate([0.0, 1.0, 0.0]), [0.0, 1.0, 0.0]));
    }

    #[test]
    fn on_flat_ground_it_only_turns_about_the_vertical() {
        for yaw in [0.0, 37.0, 90.0, -140.0] {
            let forward = Quat::from_rotator([0.0, yaw, 0.0]).rotate([1.0, 0.0, 0.0]);
            let rotation = on_surface([0.0, 0.0, 1.0], forward);
            assert!(near(rotation.rotate([1.0, 0.0, 0.0]), forward), "{yaw}");
            assert!(near(rotation.rotate([0.0, 0.0, 1.0]), [0.0, 0.0, 1.0]));
        }
    }

    #[test]
    fn a_rolled_surface_keeps_its_normal() {
        let normal = math::normalized([0.3, -0.5, 0.8]).unwrap();
        let rotation = on_surface(normal, [0.2, 1.0, 0.1]);
        assert!(near(rotation.rotate([0.0, 0.0, 1.0]), normal));
    }

    /// City2's central garage (`BP_Editor_Medium_2`, garage 12 of 14 in game 1.11.7), the only
    /// one of the map with a pinned spawn point: its fields from the mod kit's level (the offset is
    /// the C++ default), where it stands in game.
    fn central_garage(pinned_origin: [f32; 3]) -> (Garage, GaragePlace) {
        let garage = Garage {
            actor: core::ptr::null_mut(),
            extent: [2500.0, 1500.0, 2400.0],
            offset: [50.0, 0.0, 0.0],
            rotation: [0.0; 3],
            pinned_origin,
            bounds_origin: [0.0, 0.0, 205.0],
            bounds_extent: [570.0, 780.0, 295.0],
        };
        let place = GaragePlace {
            pose: Pose {
                location: [-17690.1, -5492.7, 0.0],
                rotation: Quat::from_rotator([0.0, 90.0, 0.0]),
            },
            scale: [1.0; 3],
        };
        (garage, place)
    }

    /// A city cover: bricks a kilometre out every way.
    const COVER: VehicleBox = VehicleBox {
        min: [-101500.0, -101500.0, -94212.5],
        max: [101500.0, 101500.0, 100002.5],
    };

    fn close(a: Vec3, b: Vec3) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.5)
    }

    #[test]
    fn a_pinned_cover_lands_25_m_further_out_at_the_central_garage() {
        let (garage, place) = central_garage([5053.0, 0.0, 0.0]);
        let normal = land(&garage, &place, false, &COVER).vehicle.location;
        let pinned = land(&garage, &place, true, &COVER).vehicle.location;
        // Where the mod predicted in game, from the garage's fields then, that it lands unpinned.
        assert!(close(normal, [-17690.1, -2372.7, -495.0]), "{normal:?}");
        // 570 + 5053 in front of the building instead of 570 + 2500 + 50, along the garage's
        // forward (the world's +Y).
        assert!(
            close(math::sub(pinned, normal), [0.0, 2503.0, 0.0]),
            "{pinned:?}"
        );
    }

    #[test]
    fn garages_without_a_pinned_point_ignore_the_setting() {
        let (garage, place) = central_garage([0.0; 3]);
        let normal = land(&garage, &place, false, &COVER).vehicle.location;
        let pinned = land(&garage, &place, true, &COVER).vehicle.location;
        assert!(close(normal, pinned), "{normal:?} {pinned:?}");
    }
}
