//! How a gun's shot flies, as the game flies it (no game code here: plain numbers in, plain numbers
//! out). Found in BRMK 1.11.6; see `docs/re/guns.md`, "Trajectories".
//!
//! - The muzzle is the gun's first connector, moved to the farthest barrel connector in front of
//!   it ([`muzzle`], `UBrick::CalcMuzzleTransform`), short of any hidden barrel. Barrels up to
//!   `MaxBarrelLength` long make the shot faster and stronger ([`scaled`],
//!   `UGunBrick::UpdateBarrels`).
//! - A projectile only falls: its movement adds gravity (times its `GravityScale`) and nothing
//!   else, and the engine moves it by `v·t + ½·a·t²` each step, which is exact for a constant
//!   acceleration. So its path is the same parabola at any frame rate ([`fly`]); it stops exactly
//!   at `MaxRange` of distance travelled (`MoveUpdatedComponentImpl` shortens the last move).
//! - The damage it deals depends on the distance it travelled ([`ProjectileParams::damage_at`]),
//!   times the projectiles merged into it: guns of one kind with the same ammo whose muzzles
//!   coincide ([`coincide`]) fire one projectile together (`UProjectileManagerComponent::
//!   SpawnProjectile` merges the first projectile of each shot into one fired by a coinciding gun
//!   the same frame).
//! - What it hits, it damages, then goes through if it has enough penetration left (see
//!   `game::penetration`), turning randomly by up to [`MAX_VEER`] times the share of its
//!   penetration that crossing used, and going on from where it entered with that thing left out
//!   (so it also meets what's in the same place); explosive ammo stops at the first hit.
//! - What it can't go through it ricochets off ([`ricochet`]), unless it hit it within
//!   [`AFTER_EXIT`] of leaving the last thing it went through (`UBrickProjectileMovementComponent::
//!   HandleBlockingHit`, BRMK 0x32D420, then the engine's `HandleImpact` when the projectile
//!   `bShouldBounce`s).

use crate::game::guns::{GunInfo, ProjectileParams};
use crate::math::{self, Vec3};

/// A connector in the world: where it is and the way it faces (length 1).
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub origin: Vec3,
    pub axis: Vec3,
}

/// Where a gun's shots leave from, and the length of barrel in front of its own muzzle (cm).
#[derive(Clone, Copy, Debug)]
pub struct Muzzle {
    pub frame: Frame,
    pub barrel_length: f64,
}

/// How far off the muzzle's axis a barrel connector may be (radians): 5° (BRMK .rdata 0x77648C).
const BARREL_ANGLE: f64 = 0.087_266_46;
/// How far from the muzzle's direction a barrel connector may face: the cosine between them
/// within this of 1 (BRMK .rdata 0x776490).
const BARREL_FACING: f64 = 0.1;

/// The muzzle of a gun whose own muzzle connector is `gun`, given the connectors of the barrel
/// bricks shown (only those of the muzzle's kind count): the farthest in front of it, within
/// [`BARREL_ANGLE`] of its axis and facing its way, as `CalcMuzzleTransform` (lambda BRMK 0x77A20)
/// finds it. The shot leaves from there, along that connector's axis.
///
/// The game counts every barrel, hidden or not, and doesn't need them to touch. Here hiding a
/// barrel takes it off: the connectors of `hidden` barrels in line cut off those from there on.
pub fn muzzle(
    gun: Frame,
    barrels: impl IntoIterator<Item = Frame>,
    hidden: impl IntoIterator<Item = Frame>,
) -> Muzzle {
    let cut = hidden
        .into_iter()
        .filter_map(|barrel| in_line(gun, barrel))
        .fold(f64::INFINITY, f64::min);
    let mut found = Muzzle {
        frame: gun,
        barrel_length: 0.0,
    };
    for barrel in barrels {
        if let Some(ahead) = in_line(gun, barrel)
            && ahead > found.barrel_length
            && ahead < cut
        {
            found = Muzzle {
                frame: barrel,
                barrel_length: ahead,
            };
        }
    }
    found
}

/// How far in front of the gun's own muzzle connector `gun` the barrel connector `barrel` is, if
/// it's one the muzzle may move to (see [`muzzle`]).
pub fn in_line(gun: Frame, barrel: Frame) -> Option<f64> {
    let offset = math::sub(barrel.origin, gun.origin);
    let ahead = math::dot(offset, gun.axis);
    let direction = math::normalized(offset)?;
    let angle = math::dot(direction, gun.axis).clamp(-1.0, 1.0).acos();
    let facing = math::dot(barrel.axis, gun.axis);
    (ahead > 0.0 && angle <= BARREL_ANGLE && (facing - 1.0).abs() <= BARREL_FACING).then_some(ahead)
}

/// How much of the barrels' effect a gun gets: its barrel length's share of `MaxBarrelLength`.
pub fn barrel_share(info: &GunInfo, length: f64) -> f64 {
    if info.max_barrel_length > 0.0 {
        (length / info.max_barrel_length).clamp(0.0, 1.0)
    } else {
        // The game divides by zero there; a modded gun like that gets no barrel effect here.
        0.0
    }
}

/// The projectile params and spread radius a gun fires with, for its barrels' `share`
/// (`UGunBrick::UpdateBarrels`, BRMK 0x18C900).
pub fn scaled(info: &GunInfo, share: f64) -> (ProjectileParams, f64) {
    let lerp = |from: f64, to: f64| from + (to - from) * share;
    let speed = lerp(info.min_speed_scale, 1.0);
    let damage = lerp(info.min_damage_scale, 1.0);
    let p = info.params;
    let params = ProjectileParams {
        initial_speed: p.initial_speed * speed,
        max_damage: p.max_damage * damage,
        min_damage: p.min_damage * damage,
        dropoff_start: p.dropoff_start * damage,
        dropoff_end: p.dropoff_end * damage,
        max_range: p.max_range * damage,
    };
    (
        params,
        info.spread_radius * lerp(1.0, info.min_spread_scale),
    )
}

/// The distance a gun's `SpreadRadius` is the spread cone's radius at, in cm: 50 m
/// (`UFirearmComponent::SetSpreadRadius`, BRMK 0x25CB50, keeps `atan(radius * 0.0002)` as the
/// cone's half angle).
pub const SPREAD_DISTANCE: f64 = 5000.0;

impl ProjectileParams {
    /// The damage of one projectile after travelling `range` cm (BRMK 0x31F650): from
    /// `MaxDamage` until `DropoffStart`, down to `MinDamage` at `DropoffEnd`, then flat.
    pub fn damage_at(&self, range: f64) -> f64 {
        let span = self.dropoff_end - self.dropoff_start;
        let share = if span.abs() > 1e-8 {
            ((range - self.dropoff_start) / span).clamp(0.0, 1.0)
        } else if range < self.dropoff_end {
            0.0
        } else {
            1.0
        };
        self.max_damage + (self.min_damage - self.max_damage) * share
    }
}

/// Whether two guns' own muzzle connectors coincide as `UGunBrick::DoGunsCoincide` (BRMK 0x13B7E0)
/// tells (it also wants the same kind of gun and ammo): within 0.1 cm on each axis, facing the
/// same way within 0.0001 on each.
pub fn coincide(a: Frame, b: Frame) -> bool {
    (0..3).all(|axis| {
        (a.origin[axis] - b.origin[axis]).abs() <= 0.1
            && (a.axis[axis] - b.axis[axis]).abs() <= 1e-4
    })
}

/// Penetration per damage: the game's `BR.Projectiles.PenetrationCoefficient` (100, BRMK .data
/// 0xBD12BC), in cm of density-1 material.
pub const PENETRATION_PER_DAMAGE: f64 = 100.0;
/// The most a projectile turns going through something (degrees), times the share of its
/// penetration that took: `BR.Projectiles.MaxPenetrationSpreadAngle` (45, BRMK .data 0xBD12C0).
pub const MAX_VEER: f64 = 45.0;

/// A shot, as it leaves the muzzle.
#[derive(Clone, Copy, Debug)]
pub struct Shot {
    pub start: Vec3,
    /// cm/s.
    pub velocity: Vec3,
    /// Acceleration along Z (cm/s², negative: down).
    pub gravity: f64,
    pub params: ProjectileParams,
    /// Projectiles merged into one (stacked guns): its damage is that many times one's.
    pub stack: f64,
    pub penetration_coefficient: f64,
    /// Explosive ammo: stops at the first hit.
    pub explodes: bool,
    /// Whether it ricochets off what it can't go through, and how slow a ricochet stops it (cm/s).
    pub bounces: bool,
    pub bounce_stop_speed: f64,
}

/// Something the shot's line meets, as the world tells it.
#[derive(Clone, Copy, Debug)]
pub struct Contact {
    /// How far along the line traced, in cm.
    pub distance: f64,
    /// The surface's normal there (either way; zero if unknown: no ricochet).
    pub normal: Vec3,
    /// What it hit (a component's address), to leave it out once gone through, as the game does.
    pub what: usize,
    /// The vehicle's brick it hit (its address), if it's one.
    pub brick: Option<usize>,
    /// The density of its material (1 unless it's a brick's).
    pub density: f64,
    /// How far through it the line goes from where it entered (cm). Infinite if no far side was
    /// found: the game's thickness sweep missing counts as too thick (FLT_MAX), so it ricochets
    /// off the ground rather than going through.
    pub thickness: f64,
}

/// What the shot did where it hit something.
#[derive(Clone, Copy, Debug)]
pub struct Impact {
    pub at: Vec3,
    /// Where it came out of what it went through (`at` if it didn't go through; where it ended or
    /// turned if that was inside, see [`Self::comes_out`]).
    pub exit: Vec3,
    /// The distance it travelled to there (cm), its time of flight (s) and its velocity there
    /// (cm/s).
    pub range: f64,
    pub time: f64,
    pub velocity: Vec3,
    /// How far from square it hit (degrees off the surface's normal), if the normal is known.
    pub angle: Option<f64>,
    /// Damage dealt there (all merged projectiles).
    pub damage: f64,
    /// How thick what it hit is along its way (cm; infinite if no far side was found), and its
    /// material's density.
    pub thickness: f64,
    pub density: f64,
    /// What it met: thickness times density (cm of density-1 material).
    pub armor: f64,
    /// The penetration it had left as it hit (cm of density-1 material it could go through).
    pub penetration: f64,
    /// The share of its penetration used up after it (by everything gone through so far).
    pub used: f64,
    /// Whether it went on through.
    pub through: bool,
    /// Whether it went through and came out of the far side. Not if it then stopped or ricocheted
    /// inside, at something else there: what it goes through is left out from where it went in,
    /// so it meets what touches or overlaps it from inside.
    pub comes_out: bool,
    /// The most it turned there, in degrees (random up to that).
    pub veer: f64,
    /// If it ricocheted there: how.
    pub ricochet: Option<Ricochet>,
    /// If it stopped there: why.
    pub stop: Option<Stop>,
    /// The vehicle's brick it hit, if it's one (see [`Contact::brick`]).
    pub brick: Option<usize>,
}

/// A ricochet: the share of its speed the shot kept, and the most its new direction is off the
/// mirrored one (degrees, random up to that).
#[derive(Clone, Copy, Debug)]
pub struct Ricochet {
    pub speed_kept: f64,
    pub scatter: f64,
    /// The mirrored direction (length 1): the middle of the cone it leaves in.
    pub direction: Vec3,
    /// The surface's normal (length 1), facing the side the shot came from.
    pub normal: Vec3,
}

/// Why a shot stopped at what it hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stop {
    /// Explosive ammo explodes at the first hit.
    Explodes,
    /// Too thick to go through, and its projectile doesn't ricochet.
    NoRicochet,
    /// Too thick, hit within [`AFTER_EXIT`] of leaving the last thing it went through.
    JustExited,
    /// Too thick, hit within [`RICOCHET_ANGLE`] of square.
    Square,
    /// Too thick, and a ricochet would have left slower than its projectile's stop speed.
    Slow,
    /// Too thick, after more ricochets than this preview follows.
    TooManyRicochets,
}

/// How a flight ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ending {
    /// It travelled `MaxRange`.
    Range,
    /// Something was too thick to go through, and it didn't ricochet.
    Stopped,
    /// Explosive ammo hit something.
    Exploded,
    /// It went on longer than this preview follows.
    Unfinished,
}

/// A point of a shot's path: where, the distance travelled to it (cm), the time of flight to it
/// (s), and the velocity there (cm/s; after a ricochet there, the new one).
#[derive(Clone, Copy, Debug)]
pub struct Point {
    pub at: Vec3,
    pub range: f64,
    pub time: f64,
    pub velocity: Vec3,
}

/// A shot's flight: the points along it, what it hit, and how it ended.
#[derive(Clone, Debug)]
pub struct Flight {
    pub points: Vec<Point>,
    pub impacts: Vec<Impact>,
    pub ending: Ending,
    /// The share of its penetration used up by what it went through.
    pub used: f64,
}

/// How far short of the far side of what it goes through a shot may end or turn and still count as
/// coming out of it (cm): the next thing touching it is met there, give or take rounding.
const OUT_TOLERANCE: f64 = 0.1;

/// Cuts short what the shot was still inside when it stopped or turned `range` cm of distance
/// travelled from the muzzle: it never came out of it.
fn cut_short(impacts: &mut [Impact], range: f64) {
    for impact in impacts.iter_mut().filter(|impact| impact.comes_out) {
        let inside = range - impact.range;
        if inside < impact.thickness - OUT_TOLERANCE {
            if let Some(direction) = math::normalized(impact.velocity) {
                impact.exit = math::add(impact.at, math::scale(direction, inside.max(0.0)));
            }
            impact.comes_out = false;
        }
    }
}

impl Flight {
    /// The path at `range` cm of distance travelled from the muzzle: between the points around it
    /// (past its end: the end).
    pub fn point_at(&self, range: f64) -> Point {
        let fallback = self.points.last().copied().unwrap_or(Point {
            at: [0.0; 3],
            range: 0.0,
            time: 0.0,
            velocity: [0.0; 3],
        });
        let Some(pair) = self
            .points
            .windows(2)
            .find(|pair| pair[0].range <= range && range <= pair[1].range)
        else {
            return fallback;
        };
        let [a, b] = [pair[0], pair[1]];
        let span = b.range - a.range;
        let share = if span > 1e-9 {
            (range - a.range) / span
        } else {
            0.0
        };
        let lerp = |from: f64, to: f64| from + (to - from) * share;
        Point {
            at: core::array::from_fn(|axis| lerp(a.at[axis], b.at[axis])),
            range,
            time: lerp(a.time, b.time),
            velocity: core::array::from_fn(|axis| lerp(a.velocity[axis], b.velocity[axis])),
        }
    }
}

/// The most steps a flight is followed for.
const MAX_STEPS: usize = 4000;
/// How many contacts on one step are looked at (things gone through) at most.
const MAX_CONTACTS_PER_STEP: usize = 64;
/// How far before where it entered the last thing gone through the line is looked at again (cm):
/// the projectile goes on from there, so it meets what's in the same place too.
const BACK: f64 = 0.01;
/// How far off a surface it ricocheted off the shot goes on from (cm, along the surface's normal):
/// the line is traced in single precision, so starting on the surface could start inside it and
/// meet it again. The game's projectile, a sphere, is its radius off it anyway.
const LIFT: f64 = 0.25;
/// How far past a ricochet the surface it ricocheted off is left out (cm of distance travelled), in
/// case the line still starts inside it.
const AFTER_BOUNCE: f64 = 1.0;
/// The most ricochets a flight is followed through.
const MAX_RICOCHETS: usize = 32;
/// How far (cm of distance travelled) past the far side of the last thing it went through a shot
/// stops at what it can't go through rather than ricocheting (BRMK .rdata 0x7764E8).
pub const AFTER_EXIT: f64 = 10.0;
/// Hitting a surface at most this far from square (degrees off its normal), a shot stops; past it,
/// it ricochets (`ComputeBounceResult`, BRMK 0x3113C0: π/4).
pub const RICOCHET_ANGLE: f64 = 45.0;

/// How far from square a shot moving at `velocity` hits a surface facing `normal` (either way), in
/// degrees off the normal; `None` if either is zero.
pub fn incidence(velocity: Vec3, normal: Vec3) -> Option<f64> {
    let direction = math::normalized(velocity)?;
    let normal = math::normalized(normal)?;
    Some(
        math::dot(direction, normal)
            .abs()
            .clamp(0.0, 1.0)
            .acos()
            .to_degrees(),
    )
}

/// Where a shot moving at `velocity` goes once it hit a surface facing `normal` (either way) that
/// it can't go through, as `UBrickProjectileMovementComponent::ComputeBounceResult` (BRMK
/// 0x3113C0) turns it: mirrored off the surface, at its speed times how far past
/// [`RICOCHET_ANGLE`] from square it hit (0 there, all of it grazing), then turned randomly by up to
/// how far it hit from grazing. Its new velocity (the mirrored direction) and the ricochet; why it
/// stops instead: hit too square (or the normal is unknown), or left slower than `stop_speed`
/// (cm/s).
pub fn ricochet(velocity: Vec3, normal: Vec3, stop_speed: f64) -> Result<(Vec3, Ricochet), Stop> {
    let (Some(direction), Some(mut normal)) =
        (math::normalized(velocity), math::normalized(normal))
    else {
        return Err(Stop::Square);
    };
    if math::dot(normal, direction) > 0.0 {
        normal = math::scale(normal, -1.0);
    }
    let along = math::dot(direction, normal);
    let angle = (-along).clamp(-1.0, 1.0).acos().to_degrees();
    let share = ((angle - RICOCHET_ANGLE) / (90.0 - RICOCHET_ANGLE)).clamp(0.0, 1.0);
    if share <= 0.0 {
        return Err(Stop::Square);
    }
    let speed = math::length(velocity) * share;
    if speed < stop_speed || !speed.is_finite() {
        return Err(Stop::Slow);
    }
    let mirrored = math::sub(direction, math::scale(normal, 2.0 * along));
    let mirrored = math::normalized(mirrored).unwrap_or(mirrored);
    Ok((
        math::scale(mirrored, speed),
        Ricochet {
            speed_kept: share,
            scatter: 90.0 - angle,
            direction: mirrored,
            normal,
        },
    ))
}

/// Flies `shot` in steps about `step` cm long (shorter ones follow the curve closer), asking
/// `world` what the line between two points meets first, leaving out the things (by
/// [`Contact::what`]) it's given: those gone through (`None`: nothing, or don't look).
pub fn fly(
    shot: &Shot,
    step: f64,
    mut world: impl FnMut(Vec3, Vec3, &[usize]) -> Option<Contact>,
) -> Flight {
    let gravity = [0.0, 0.0, shot.gravity];
    let mut position = shot.start;
    let mut velocity = shot.velocity;
    let mut range = 0.0;
    let mut elapsed = 0.0;
    let mut flight = Flight {
        points: vec![Point {
            at: position,
            range: 0.0,
            time: 0.0,
            velocity,
        }],
        impacts: Vec::new(),
        ending: Ending::Unfinished,
        used: 0.0,
    };
    // Things gone through, left out from then on (the game ignores the component), and the
    // distance travelled to where the last one was entered: the projectile goes on from there.
    let mut gone_through: Vec<usize> = Vec::new();
    // From just behind the muzzle: what touches it is met too.
    let mut resume = -BACK;
    // Up to this distance travelled, what it can't go through stops it rather than turning it.
    let mut stops_until = 0.0_f64;
    let mut ricochets = 0;
    // The surface it last ricocheted off, and up to what distance travelled it's left out.
    let mut bounced_off: Option<(usize, f64)> = None;
    let max_range = shot.params.max_range;
    'steps: for _ in 0..MAX_STEPS {
        let speed = math::length(velocity);
        if speed < 1e-3 || !speed.is_finite() {
            return flight;
        }
        let mut time = step / speed;
        let mut next = math::add(
            math::add(position, math::scale(velocity, time)),
            math::scale(gravity, 0.5 * time * time),
        );
        let mut chord = math::sub(next, position);
        let mut length = math::length(chord);
        let last = range + length >= max_range;
        if last {
            // The game shortens the last move to end exactly at the range.
            let left = (max_range - range).max(0.0);
            let factor = if length > 0.0 { left / length } else { 0.0 };
            chord = math::scale(chord, factor);
            next = math::add(position, chord);
            length = left;
            time *= factor;
        }
        let next_velocity = math::add(velocity, math::scale(gravity, time));
        if length > 0.0 {
            let first = if range > 0.0 { 0.0 } else { -BACK / length };
            let mut from = ((resume - range) / length).clamp(first, 1.0);
            let mut looked = 0;
            while from < 1.0 && looked < MAX_CONTACTS_PER_STEP {
                looked += 1;
                let start = math::add(position, math::scale(chord, from));
                let Some(contact) = world(start, next, &gone_through) else {
                    break;
                };
                let share = from + contact.distance / length;
                let at = math::add(position, math::scale(chord, share));
                let hit_range = (range + share * length).max(0.0);
                let just_left = bounced_off
                    .is_some_and(|(what, until)| what == contact.what && hit_range <= until);
                if gone_through.contains(&contact.what) || just_left {
                    from = share + 1.0 / length;
                    continue;
                }
                let damage = shot.params.damage_at(hit_range) * shot.stack;
                let density = if contact.density > 0.0 {
                    contact.density
                } else {
                    1.0
                };
                let thickness = contact.thickness.max(0.0);
                let penetration = PENETRATION_PER_DAMAGE * shot.penetration_coefficient * damage;
                let reach = penetration / density;
                // Its velocity and time of flight there (the step's time taken as spread evenly
                // along its chord).
                let hit_velocity = math::add(velocity, math::scale(gravity, share.max(0.0) * time));
                let hit_time = elapsed + share.max(0.0) * time;
                let mut impact = Impact {
                    at,
                    exit: at,
                    range: hit_range,
                    time: hit_time,
                    velocity: hit_velocity,
                    angle: incidence(hit_velocity, contact.normal),
                    damage,
                    thickness,
                    density,
                    armor: thickness * density,
                    penetration: (1.0 - flight.used).max(0.0) * penetration,
                    used: flight.used,
                    through: false,
                    comes_out: false,
                    veer: 0.0,
                    ricochet: None,
                    stop: None,
                    brick: contact.brick,
                };
                let point = |velocity| Point {
                    at,
                    range: hit_range,
                    time: hit_time,
                    velocity,
                };
                if shot.explodes {
                    cut_short(&mut flight.impacts, hit_range);
                    impact.stop = Some(Stop::Explodes);
                    flight.impacts.push(impact);
                    flight.points.push(point(hit_velocity));
                    flight.ending = Ending::Exploded;
                    return flight;
                }
                let goes_through =
                    reach.is_finite() && reach > 0.0 && thickness < (1.0 - flight.used) * reach;
                if !goes_through {
                    let turned = if !shot.bounces {
                        Err(Stop::NoRicochet)
                    } else if hit_range <= stops_until {
                        Err(Stop::JustExited)
                    } else if ricochets >= MAX_RICOCHETS {
                        Err(Stop::TooManyRicochets)
                    } else {
                        ricochet(hit_velocity, contact.normal, shot.bounce_stop_speed)
                    };
                    cut_short(&mut flight.impacts, hit_range);
                    match turned {
                        Ok((turned, how)) => {
                            impact.ricochet = Some(how);
                            flight.impacts.push(impact);
                            flight.points.push(point(turned));
                            // On from just off the surface, the new way.
                            ricochets += 1;
                            position = math::add(at, math::scale(how.normal, LIFT));
                            velocity = turned;
                            range = hit_range;
                            elapsed = hit_time;
                            resume = hit_range;
                            bounced_off = Some((contact.what, hit_range + AFTER_BOUNCE));
                            continue 'steps;
                        }
                        Err(stop) => {
                            impact.stop = Some(stop);
                            flight.impacts.push(impact);
                            flight.points.push(point(hit_velocity));
                            flight.ending = Ending::Stopped;
                            return flight;
                        }
                    }
                }
                flight.used += thickness / reach;
                impact.used = flight.used;
                impact.through = true;
                impact.comes_out = true;
                impact.veer = MAX_VEER * thickness / reach;
                if let Some(direction) = math::normalized(hit_velocity) {
                    impact.exit = math::add(at, math::scale(direction, thickness));
                }
                flight.impacts.push(impact);
                gone_through.push(contact.what);
                stops_until = stops_until.max(hit_range + thickness + AFTER_EXIT);
                resume = hit_range - BACK;
                from = ((resume - range) / length).max(from);
            }
        }
        position = next;
        velocity = next_velocity;
        range += length;
        elapsed += time;
        flight.points.push(Point {
            at: position,
            range,
            time: elapsed,
            velocity,
        });
        if last {
            flight.ending = Ending::Range;
            return flight;
        }
    }
    flight
}
