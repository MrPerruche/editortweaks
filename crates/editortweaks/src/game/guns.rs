//! Gun bricks, the projectiles they fire and the barrels in front of them, all through reflection.
//! Found in BRMK 1.11.6 (exports, the reflection tables UHT generates, disassembly); see
//! `docs/re/guns.md`, "Trajectories".
//!
//! - A gun brick (`UGunBrick`) has an `AmmoType` (its property in the editor). Its static info
//!   (`UGunBrickStaticInfo`) has `FirearmProperties` (`FFirearmProperties`: `ProjectileClass`,
//!   `ProjectileParams`, `NumProjectilesPerShot`, `SpreadRadius`) and the barrel scales
//!   (`MaxBarrelLength`, `MinMuzzleVelocityScale`, `MinDamageScale`, `MinSpreadRadiusScale`), and
//!   how fast it fires ([`FireRate`]: `FirearmProperties`' `BoltCycleTime`, `AmmoCapacity` and
//!   `BurstRounds`, and its own `ReloadTime`; each optional).
//! - The projectile class's default object (`ABrickProjectile`) has `GravityScale` (copied to its
//!   movement component's `ProjectileGravityScale` in `PreRegisterAllComponents`),
//!   `PenetrationCoefficient`, `ExplosiveVolume`, `HomingExplosiveRatio` and
//!   `CollisionProfileName`, and its movement component's template whether it bounces
//!   (`bShouldBounce`, set by `UBrickProjectileMovementComponent`'s constructor) and how slow a
//!   bounce stops it (`BounceVelocityStopSimulatingThreshold`).
//! - A brick's connectors are its static info's `Connectors` (`TArray<FConnectorField>`, what
//!   `UBrick::GetBrickConnectors` returns; scalable bricks make their own instead). The first one
//!   of a gun is its muzzle (`UBrick::CalcMuzzleTransform`).
//!
//! Everything read is checked (null objects, classes, finite numbers), so a modded gun or
//! projectile with odd values is left out rather than trusted.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::tarray::TArray;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::game::reflection::{self, BoolField};
use crate::math::{self, Quat, Vec3};

/// `FProjectileParams`: what a gun gives its projectiles (ranges in cm, speed in cm/s).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectileParams {
    pub initial_speed: f64,
    pub max_damage: f64,
    pub min_damage: f64,
    pub dropoff_start: f64,
    pub dropoff_end: f64,
    pub max_range: f64,
}

/// What a gun brick's static info says about its shots.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GunInfo {
    pub params: ProjectileParams,
    pub per_shot: u8,
    /// `SpreadRadius`, scaled down by barrels (see [`GunInfo::min_spread_scale`]).
    pub spread_radius: f64,
    /// Barrels up to this long (cm) make shots faster and stronger.
    pub max_barrel_length: f64,
    /// Speed, damage and spread factors without barrels (full barrels: 1).
    pub min_speed_scale: f64,
    pub min_damage_scale: f64,
    pub min_spread_scale: f64,
}

/// What a projectile's default object says about its flight.
#[derive(Clone, Copy, Debug)]
pub struct ProjectileInfo {
    /// Times the world's gravity.
    pub gravity_scale: f64,
    pub penetration_coefficient: f64,
    /// Liters of explosive per projectile, for explosive ammo.
    pub explosive_volume: f64,
    /// Times [`Self::explosive_volume`] for homing ammo (1 if it can't be read).
    pub homing_explosive_ratio: f64,
    /// `UProjectileMovementComponent::MaxSpeed` (0: no limit).
    pub max_speed: f64,
    /// The collision profile it moves with (an `FName`), if it has one.
    pub profile: Option<[u8; 8]>,
    /// Whether it ricochets off what it can't go through, and the speed (cm/s) under which a
    /// ricochet stops it instead.
    pub bounces: bool,
    pub bounce_stop_speed: f64,
}

/// How fast a gun fires, as far as its static info tells (`None`: not reflected, or not a number).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FireRate {
    /// The least time between shots (s).
    pub cycle: Option<f64>,
    /// Rounds in a magazine.
    pub capacity: Option<i64>,
    /// How long a reload takes (s).
    pub reload: Option<f64>,
    /// Rounds in a burst (0: no burst mode).
    pub burst: Option<i64>,
}

/// One connector of a brick, in the brick's space (cm): where it is and the way it faces (its Z
/// axis, length 1).
#[derive(Clone, Copy, Debug)]
pub struct Connector {
    pub kind: u8,
    pub origin: Vec3,
    pub axis: Vec3,
}

/// Offsets inside `FConnectorField`.
struct ConnectorLayout {
    size: usize,
    kind: usize,
    direction: usize,
    origin: usize,
    rotation: usize,
}

/// Offsets inside `FProjectileParams`, from the start of `FFirearmProperties`.
struct ParamsLayout {
    initial_speed: usize,
    max_damage: usize,
    min_damage: usize,
    dropoff_start: usize,
    dropoff_end: usize,
    max_range: usize,
}

/// Reads guns, barrels and projectiles (after engine init).
pub struct Guns {
    gun: *const UStruct,
    barrel: *const UStruct,
    gun_info: *const UStruct,
    projectile: *const UStruct,
    /// Offset of `UGunBrick::AmmoType`.
    ammo_type: usize,
    /// Offset of `UBrickStaticInfo::Connectors`.
    connectors: usize,
    connector: ConnectorLayout,
    /// Offset of `UGunBrickStaticInfo::FirearmProperties`, and of its fields in it.
    firearm: usize,
    projectile_class: usize,
    params: ParamsLayout,
    per_shot: usize,
    spread_radius: usize,
    /// Offsets of the barrel fields of `UGunBrickStaticInfo`.
    max_barrel_length: usize,
    min_speed_scale: usize,
    min_damage_scale: usize,
    min_spread_scale: usize,
    /// Offsets of the fire rate's fields, if reflected: `BoltCycleTime`, `AmmoCapacity` and
    /// `BurstRounds` in `FFirearmProperties` (with their size), `ReloadTime` in the static info.
    bolt_cycle: Option<usize>,
    capacity: Option<(usize, usize)>,
    burst: Option<(usize, usize)>,
    reload: Option<usize>,
    /// Offsets of `ABrickProjectile` fields.
    gravity_scale: usize,
    penetration_coefficient: usize,
    explosive_volume: usize,
    /// `HomingExplosiveRatio`, if reflected.
    homing_explosive_ratio: Option<usize>,
    collision_profile: usize,
    movement: usize,
    /// `ProjectileMovementComponent`, and its `MaxSpeed`, `bShouldBounce` and
    /// `BounceVelocityStopSimulatingThreshold`.
    movement_class: *const UStruct,
    max_speed: usize,
    should_bounce: BoolField,
    bounce_stop_speed: usize,
}

// SAFETY: reflection records and offsets living as long as the game; only used on the game thread.
unsafe impl Send for Guns {}
unsafe impl Sync for Guns {}

pub fn find() -> Result<Guns, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let gun = class("GunBrick")?;
    let info = class("BrickStaticInfo")?;
    let gun_info = class("GunBrickStaticInfo")?;
    let projectile = class("BrickProjectile")?;
    let movement_class = class("ProjectileMovementComponent")?;
    let float =
        |owner: *const UClass, name| reflection::field_offset(owner, name, "FloatProperty", 4);

    let connectors_property = reflection::field(info, "Connectors", "ArrayProperty", 0x10)?;
    // SAFETY: live property records from reflection.
    let field_struct =
        unsafe { reflection::struct_type(reflection::array_inner(connectors_property)) };
    if field_struct.is_null() {
        return Err("BrickStaticInfo::Connectors isn't an array of structs".into());
    }
    let connector_field = |name, kind, size| {
        reflection::struct_field_offset(field_struct, name, kind, size)
            .map_err(|reason| format!("ConnectorField: {reason}"))
    };
    let connector = ConnectorLayout {
        // SAFETY: a live struct from reflection.
        size: unsafe { reflection::struct_size(field_struct) },
        kind: connector_field("Type", "EnumProperty", 1)?,
        direction: connector_field("Direction", "EnumProperty", 1)?,
        origin: connector_field("Origin", "StructProperty", 12)?,
        rotation: connector_field("Rotation", "StructProperty", 12)?,
    };

    let (firearm, firearm_struct) = struct_field(gun_info.cast(), "FirearmProperties")?;
    let firearm_field = |name, kind, size| {
        reflection::struct_field_offset(firearm_struct, name, kind, size)
            .map_err(|reason| format!("FirearmProperties: {reason}"))
    };
    // An integer field of `FFirearmProperties`, of whichever size it is.
    let integer = |name| {
        [
            ("IntProperty", 4),
            ("ByteProperty", 1),
            ("Int64Property", 8),
        ]
        .iter()
        .find_map(|&(kind, size)| {
            reflection::struct_field_offset(firearm_struct, name, kind, size as i32)
                .ok()
                .map(|offset| (offset, size))
        })
    };
    let (params, params_struct) = struct_field(firearm_struct, "ProjectileParams")?;
    let param = |name| {
        reflection::struct_field_offset(params_struct, name, "FloatProperty", 4)
            .map(|offset| params + offset)
            .map_err(|reason| format!("ProjectileParams: {reason}"))
    };

    // SAFETY: a live property record from reflection.
    let collision_profile = unsafe {
        let property = reflection::find_property(projectile.cast(), "CollisionProfileName");
        if property.is_null() || !reflection::is_kind(property, "NameProperty") {
            return Err("BrickProjectile::CollisionProfileName not found".into());
        }
        if (*property).element_size != 8 {
            return Err("BrickProjectile::CollisionProfileName isn't 8 bytes".into());
        }
        (*property).offset_internal as usize
    };

    Ok(Guns {
        gun: gun.cast(),
        barrel: class("BarrelBrick")?.cast(),
        gun_info: gun_info.cast(),
        projectile: projectile.cast(),
        ammo_type: reflection::field_offset(gun, "AmmoType", "EnumProperty", 1)?,
        connectors: reflection::field_offset(info, "Connectors", "ArrayProperty", 0x10)?,
        connector,
        firearm,
        projectile_class: firearm_field("ProjectileClass", "ClassProperty", 8)?,
        params: ParamsLayout {
            initial_speed: param("InitialSpeed")?,
            max_damage: param("MaxDamage")?,
            min_damage: param("MinDamage")?,
            dropoff_start: param("DropoffStart")?,
            dropoff_end: param("DropoffEnd")?,
            max_range: param("MaxRange")?,
        },
        per_shot: firearm_field("NumProjectilesPerShot", "ByteProperty", 1)?,
        spread_radius: firearm_field("SpreadRadius", "FloatProperty", 4)?,
        max_barrel_length: float(gun_info, "MaxBarrelLength")?,
        min_speed_scale: float(gun_info, "MinMuzzleVelocityScale")?,
        min_damage_scale: float(gun_info, "MinDamageScale")?,
        min_spread_scale: float(gun_info, "MinSpreadRadiusScale")?,
        bolt_cycle: firearm_field("BoltCycleTime", "FloatProperty", 4).ok(),
        capacity: integer("AmmoCapacity"),
        burst: integer("BurstRounds"),
        reload: float(gun_info, "ReloadTime").ok(),
        gravity_scale: float(projectile, "GravityScale")?,
        penetration_coefficient: float(projectile, "PenetrationCoefficient")?,
        explosive_volume: float(projectile, "ExplosiveVolume")?,
        homing_explosive_ratio: float(projectile, "HomingExplosiveRatio").ok(),
        collision_profile,
        movement: reflection::field_offset(
            projectile,
            "ProjectileMovementComponent",
            "ObjectProperty",
            8,
        )?,
        movement_class: movement_class.cast(),
        max_speed: float(movement_class, "MaxSpeed")?,
        should_bounce: reflection::bool_field(movement_class.cast(), "bShouldBounce")?,
        bounce_stop_speed: float(movement_class, "BounceVelocityStopSimulatingThreshold")?,
    })
}

/// The offset of the struct field `name` of `owner` (a class or struct), and its struct.
fn struct_field(
    owner: *const UStruct,
    name: &'static str,
) -> Result<(usize, *const UStruct), String> {
    // SAFETY: `owner` is a live class or struct from reflection.
    unsafe {
        let property = reflection::find_property(owner, name);
        let inner = if property.is_null() {
            core::ptr::null()
        } else {
            reflection::struct_type(property)
        };
        if inner.is_null() {
            return Err(format!("struct field '{name}' not found"));
        }
        Ok(((*property).offset_internal as usize, inner))
    }
}

impl Guns {
    /// Whether `object` is a gun brick.
    ///
    /// # Safety
    /// `object` must be a live object.
    pub unsafe fn is_gun(&self, object: *const UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { reflection::is_a(object, self.gun) }
    }

    /// Whether `object` is a barrel brick.
    ///
    /// # Safety
    /// `object` must be a live object.
    pub unsafe fn is_barrel(&self, object: *const UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { reflection::is_a(object, self.barrel) }
    }

    /// The ammo type chosen for `gun` (an `EAmmoType` value).
    ///
    /// # Safety
    /// `gun` must be a live gun brick.
    pub unsafe fn ammo_type(&self, gun: *const UObjectBase) -> u8 {
        // SAFETY: per the contract; the offset comes from reflection.
        unsafe { gun.cast::<u8>().add(self.ammo_type).read() }
    }

    /// What a gun's static info (`info`, from `GetBrickStaticInfo`) says about its shots, if it is
    /// a gun's and its numbers make sense.
    ///
    /// # Safety
    /// `info` must be null or a live brick static info.
    pub unsafe fn gun_info(&self, info: *const u8) -> Option<GunInfo> {
        // SAFETY: per the contract; every field is read at its reflected offset in a live
        // `UGunBrickStaticInfo`.
        unsafe {
            if info.is_null() || !reflection::is_a(info.cast(), self.gun_info) {
                return None;
            }
            let float = |offset: usize| f64::from(info.add(offset).cast::<f32>().read_unaligned());
            let firearm = |offset: usize| float(self.firearm + offset);
            let p = &self.params;
            let found = GunInfo {
                params: ProjectileParams {
                    initial_speed: firearm(p.initial_speed),
                    max_damage: firearm(p.max_damage),
                    min_damage: firearm(p.min_damage),
                    dropoff_start: firearm(p.dropoff_start),
                    dropoff_end: firearm(p.dropoff_end),
                    max_range: firearm(p.max_range),
                },
                per_shot: info.add(self.firearm + self.per_shot).read(),
                spread_radius: firearm(self.spread_radius),
                max_barrel_length: float(self.max_barrel_length),
                min_speed_scale: float(self.min_speed_scale),
                min_damage_scale: float(self.min_damage_scale),
                min_spread_scale: float(self.min_spread_scale),
            };
            found.is_sound().then_some(found)
        }
    }

    /// How fast the gun whose static info is `info` fires (see [`Self::gun_info`]).
    ///
    /// # Safety
    /// `info` must be a live gun static info.
    pub unsafe fn fire_rate(&self, info: *const u8) -> FireRate {
        // SAFETY: per the contract; each field is read at its reflected offset and size.
        unsafe {
            let float = |offset: usize| {
                let value = f64::from(info.add(offset).cast::<f32>().read_unaligned());
                value.is_finite().then_some(value)
            };
            let integer = |(offset, size): (usize, usize)| {
                let field = info.add(self.firearm + offset);
                match size {
                    1 => i64::from(field.read()),
                    4 => i64::from(field.cast::<i32>().read_unaligned()),
                    _ => field.cast::<i64>().read_unaligned(),
                }
            };
            FireRate {
                cycle: self
                    .bolt_cycle
                    .and_then(|offset| float(self.firearm + offset)),
                capacity: self.capacity.map(integer),
                reload: self.reload.and_then(float),
                burst: self.burst.map(integer),
            }
        }
    }

    /// The default object of the projectile class a gun's static info fires, and what it says,
    /// if it's a brick projectile.
    ///
    /// # Safety
    /// `info` must be a live gun static info (see [`Self::gun_info`]).
    pub unsafe fn projectile_info(&self, info: *const u8) -> Option<ProjectileInfo> {
        // SAFETY: per the contract; the class is null or live (the static info holds it), and its
        // default object's fields are read at their reflected offsets.
        unsafe {
            let class = info
                .add(self.firearm + self.projectile_class)
                .cast::<*const UClass>()
                .read_unaligned();
            if class.is_null()
                || !reflection::with_parents(class.cast()).any(|parent| parent == self.projectile)
            {
                return None;
            }
            let defaults = reflection::default_object(class)?.cast_const().cast::<u8>();
            let float =
                |offset: usize| f64::from(defaults.add(offset).cast::<f32>().read_unaligned());
            let movement = defaults
                .add(self.movement)
                .cast::<*const UObjectBase>()
                .read_unaligned();
            let movement = (!movement.is_null() && reflection::is_a(movement, self.movement_class))
                .then_some(movement.cast::<u8>());
            let movement_float = |offset: usize| {
                movement.map_or(0.0, |movement| {
                    f64::from(movement.add(offset).cast::<f32>().read_unaligned())
                })
            };
            let profile = defaults
                .add(self.collision_profile)
                .cast::<[u8; 8]>()
                .read_unaligned();
            let found = ProjectileInfo {
                gravity_scale: float(self.gravity_scale),
                penetration_coefficient: float(self.penetration_coefficient),
                explosive_volume: float(self.explosive_volume),
                homing_explosive_ratio: self.homing_explosive_ratio.map_or(1.0, float),
                max_speed: movement_float(self.max_speed),
                // `NAME_None` is all zeros.
                profile: (profile != [0; 8]).then_some(profile),
                bounces: movement.is_some_and(|movement| self.should_bounce.get(movement)),
                bounce_stop_speed: movement_float(self.bounce_stop_speed),
            };
            [
                found.gravity_scale,
                found.penetration_coefficient,
                found.explosive_volume,
                found.homing_explosive_ratio,
                found.max_speed,
                found.bounce_stop_speed,
            ]
            .iter()
            .all(|value| value.is_finite())
            .then_some(found)
        }
    }

    /// The connectors of a brick whose static info is `info`, in the brick's space; those whose
    /// direction can't be read are left out (the game would assert on them).
    ///
    /// # Safety
    /// `info` must be null or a live brick static info.
    pub unsafe fn connectors(&self, info: *const u8) -> Vec<Connector> {
        // SAFETY: per the contract; the array is read at its reflected offset, each element at the
        // reflected size of `FConnectorField`.
        unsafe {
            if info.is_null() {
                return Vec::new();
            }
            let array = &*info.add(self.connectors).cast::<TArray<u8>>();
            let Ok(count) = usize::try_from(array.num) else {
                return Vec::new();
            };
            if array.data.is_null() || count > MAX_CONNECTORS {
                return Vec::new();
            }
            let layout = &self.connector;
            (0..count)
                .filter_map(|index| {
                    let field = array.data.cast_const().add(index * layout.size);
                    let vector = |offset: usize| {
                        math::widen(field.add(offset).cast::<[f32; 3]>().read_unaligned())
                    };
                    let base = base_axis(field.add(layout.direction).read())?;
                    let axis = rotated(vector(layout.rotation), base);
                    let origin = vector(layout.origin);
                    origin
                        .iter()
                        .chain(&axis)
                        .all(|value| value.is_finite())
                        .then_some(Connector {
                            kind: field.add(layout.kind).read(),
                            origin,
                            axis,
                        })
                })
                .collect()
        }
    }
}

/// More connectors than any brick has: an array this long is garbage.
const MAX_CONNECTORS: usize = 100_000;

impl GunInfo {
    /// Whether the numbers are ones the game could fly a projectile with.
    fn is_sound(&self) -> bool {
        let p = &self.params;
        let all_finite = [
            p.initial_speed,
            p.max_damage,
            p.min_damage,
            p.dropoff_start,
            p.dropoff_end,
            p.max_range,
            self.spread_radius,
            self.max_barrel_length,
            self.min_speed_scale,
            self.min_damage_scale,
            self.min_spread_scale,
        ]
        .iter()
        .all(|value| value.is_finite());
        all_finite && p.initial_speed > 0.0 && p.max_range > 0.0
    }
}

/// A connector's Z axis before its `Rotation`, by its `Direction` (`EConnectorDirection`, the
/// jump table of BRMK 0xAA790: 1 to 6 are +X, +Y, +Z, -X, -Y, -Z).
fn base_axis(direction: u8) -> Option<Vec3> {
    Some(match direction {
        1 => [1.0, 0.0, 0.0],
        2 => [0.0, 1.0, 0.0],
        3 => [0.0, 0.0, 1.0],
        4 => [-1.0, 0.0, 0.0],
        5 => [0.0, -1.0, 0.0],
        6 => [0.0, 0.0, -1.0],
        _ => return None,
    })
}

/// `axis` turned by a connector's `Rotation` (pitch, yaw, roll in degrees), which the game only
/// applies when one of them isn't a whole number of turns.
fn rotated(rotation: Vec3, axis: Vec3) -> Vec3 {
    if rotation.iter().all(|degrees| degrees % 360.0 == 0.0) {
        return axis;
    }
    Quat::from_rotator(rotation).rotate(axis)
}
