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

#[cfg(test)]
mod tests {
    use super::*;

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
