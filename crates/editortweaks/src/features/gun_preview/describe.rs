//! What the shot preview's tooltip says about what the mouse is on ([`super::hover::Target`]): the
//! essentials, or everything with the extended tooltip key held (`keybinds::EXTENDED_TOOLTIP`).
//! Numbers in the player's units (metric or imperial, as the game's settings say), strength and
//! penetration also in AP (as the Strength view mode tells them, `view_modes::strength`).
//!
//! Strength is the damage a shot deals: it only drops with the distance travelled (drop-off).
//! Penetration is how much armor it can go through (thickness times density): it drops with the
//! damage, and each thing pierced uses up a share of it for the rest of the flight.

use super::ballistics::{
    self, AFTER_EXIT, Ending, Impact, PENETRATION_PER_DAMAGE, Point, RICOCHET_ANGLE,
    SPREAD_DISTANCE, Stop,
};
use super::hover::Target;
use super::{Preview, Vehicle};
use crate::features::view_modes::{AP_DAMAGE, AP_PENETRATION};
use crate::math;

/// What a tooltip says: a title over lines.
pub struct Text {
    pub title: String,
    pub lines: Vec<String>,
}

/// How numbers are shown: in the player's units.
#[derive(Clone, Copy)]
pub struct Units {
    pub imperial: bool,
}

/// What describing needs besides the shots.
pub struct Context<'a> {
    pub units: Units,
    /// Everything, or the essentials.
    pub extended: bool,
    /// The vehicle's bricks as shots hit them, if they do.
    pub vehicle: Option<&'a Vehicle>,
    /// What the editor calls a brick (by address).
    pub name: &'a dyn Fn(usize) -> Option<String>,
    /// The extended tooltip key, for people ("Shift").
    pub more_key: &'a str,
}

/// The tooltip for `target` among `shots`; the essentials say which key tells more.
pub fn describe(shots: &[Preview], target: Target, context: &Context) -> Option<Text> {
    let mut text = describe_target(shots, target, context)?;
    if !context.extended {
        text.title += &format!(" - {} for more", context.more_key);
    }
    Some(text)
}

fn describe_target(shots: &[Preview], target: Target, context: &Context) -> Option<Text> {
    match target {
        Target::Gun { shot } => Some(gun(shots.get(shot)?, context)),
        Target::Path { shot, range } => Some(path(shots.get(shot)?, range, context)),
        Target::Impact { shot, impact } => {
            let shot = shots.get(shot)?;
            Some(hit(shot, shot.flight.impacts.get(impact)?, impact, context))
        }
        Target::End { shot } => Some(end(shots.get(shot)?, context)),
        Target::Weakest { shot } => {
            let shot = shots.get(shot)?;
            Some(weakest(shot, shot.weakest()?, context))
        }
        Target::Brick { brick } => Some(brick_hits(shots, brick, context)),
    }
}

/// A gun (or its barrels): what it fires.
fn gun(shot: &Preview, context: &Context) -> Text {
    let units = context.units;
    let stack = shot.stack as f64;
    let info = &shot.info;
    let params = &shot.params;
    let (base, base_spread) = ballistics::scaled(info, 0.0);
    let muzzle = params.damage_at(0.0) * stack;
    let max_barrel = info.max_barrel_length;
    let barrel = shot.barrel_length.min(max_barrel);
    let mut title = shot_title(shot, context);
    let mut lines = Vec::new();
    // The least damage, and where it's reached (within the range).
    let lowest_at = params.dropoff_end.min(params.max_range).max(0.0);
    let lowest = params.damage_at(lowest_at) * stack;
    let drops = lowest < muzzle * 0.999;
    if !context.extended {
        let mut strength = format!("Strength: {}", ap(muzzle));
        if drops {
            strength += &format!(", {} @ {}", ap(lowest), units.length(lowest_at));
        }
        lines.push(strength);
        if max_barrel > 0.0 {
            lines.push(format!("Barrels: {}", units.short_pair(barrel, max_barrel)));
        }
        if let Some(explosion) = explosion(shot, units) {
            lines.push(format!("Explosion: {explosion}"));
        }
        return Text { title, lines };
    }
    if max_barrel > 0.0 {
        title += &format!(" with {} barrel", units.short_pair(barrel, max_barrel));
    }
    let base_damage = base.damage_at(0.0) * stack;
    let mut strength = format!(
        "Strength: {} ({})",
        gained(base_damage, muzzle, Unit::DAMAGE),
        ap(muzzle)
    );
    if drops && params.dropoff_start > 0.0 && params.dropoff_start < params.max_range {
        strength += &format!(" < {}", units.length(params.dropoff_start));
    }
    if drops {
        strength += &format!(
            ", {} dmg ({}) @ {}",
            num(lowest),
            ap(lowest),
            units.length(lowest_at)
        );
    }
    lines.push(strength);
    let mut penetration = format!("Penetration: {}", armor(pierce(shot, muzzle)));
    if shot.explosive {
        penetration = format!("Penetration: none, {EXPLODES_FIRST}");
    } else if drops {
        penetration += &format!(
            ", {} @ {}",
            armor(pierce(shot, lowest)),
            units.length(lowest_at)
        );
    }
    if !shot.projectile.bounces && !shot.explosive {
        penetration += ", no ricochets";
    }
    lines.push(penetration);
    let max_speed = shot.projectile.max_speed;
    let speed = |speed: f64| {
        if max_speed > 0.0 {
            speed.min(max_speed)
        } else {
            speed
        }
    };
    let (slowest, fastest) = (speed(base.initial_speed), speed(params.initial_speed));
    let mut speed_line = format!("Speed: {}", gained(slowest, fastest, units.speed_unit()));
    if max_speed > 0.0 && params.initial_speed > max_speed {
        speed_line += " (capped by its projectile)";
    }
    lines.push(speed_line);
    let range = gained(
        base.max_range,
        params.max_range,
        units.length_unit(params.max_range),
    );
    lines.push(format!("Range: {range}"));
    if let Some(rate) = fire_rate(shot) {
        lines.push(format!("Fire rate: {rate}"));
    }
    let angle = (shot.spread_radius / SPREAD_DISTANCE).atan().to_degrees();
    lines.push(format!(
        "Spread: {} @ {} (±{}°)",
        gained(base_spread, shot.spread_radius, units.short_unit()),
        units.length(SPREAD_DISTANCE),
        num(angle)
    ));
    if let Some(explosion) = explosion(shot, units) {
        lines.push(format!("Explosion: {explosion}"));
    }
    // What it touches and measures armor with: a ball this wide, not a line.
    if let Some(radius) = shot.projectile.radius.filter(|radius| *radius > 0.0) {
        lines.push(format!(
            "Projectile size: {} wide",
            units.short(2.0 * radius)
        ));
    }
    let gravity = shot.projectile.gravity_scale;
    if (gravity - 1.0).abs() > 1e-3 {
        lines.push(format!("Gravity: ×{}", num(gravity)));
    }
    notes(shot, &mut lines);
    Text { title, lines }
}

/// A point of a shot's path, `range` cm from the muzzle.
fn path(shot: &Preview, range: f64, context: &Context) -> Text {
    let units = context.units;
    let point = shot.flight.point_at(range);
    let damage = shot.params.damage_at(range) * shot.stack as f64;
    let full = pierce(shot, damage);
    let left = full * (1.0 - used_before(shot, range)).max(0.0);
    let title = shot_title(shot, context);
    if !context.extended {
        return Text {
            title,
            lines: vec![
                format!("Strength: {}", ap(damage)),
                penetration_left(shot, left, full),
            ],
        };
    }
    let mut lines = vec![
        since_fired(point.time, range, units),
        format!("Strength: {} dmg ({})", num(damage), ap(damage)),
        penetration_left(shot, left, full),
    ];
    lines.push(format!(
        "Speed: {}",
        units.speed(math::length(point.velocity))
    ));
    let before: Vec<(usize, &Impact)> = shot
        .flight
        .impacts
        .iter()
        .enumerate()
        .filter(|(_, impact)| impact.range < range)
        .collect();
    if !before.iter().any(|(_, impact)| impact.ricochet.is_some())
        && let Some(drop) = drop(shot, point.at)
        && drop.abs() >= 1.0
    {
        let way = if drop > 0.0 { "below" } else { "above" };
        lines.push(format!(
            "Drop: {} {way} the aim line",
            units.length(drop.abs())
        ));
    }
    let pierced: Vec<&(usize, &Impact)> =
        before.iter().filter(|(_, impact)| impact.through).collect();
    if !pierced.is_empty() {
        let bricks = pierced
            .iter()
            .filter(|(_, impact)| impact.brick.is_some())
            .count();
        let broken = pierced
            .iter()
            .filter(|(index, _)| shot.breaks.get(*index).copied().unwrap_or(false))
            .count();
        let mut parts = Vec::new();
        if bricks > 0 {
            parts.push(format!(
                "{}, {broken} broken",
                count(bricks, "brick", "bricks")
            ));
        }
        if pierced.len() > bricks {
            parts.push(count(pierced.len() - bricks, "map object", "map objects"));
        }
        lines.push(format!("Pierced: {}", parts.join("; ")));
    }
    let ricochets = before
        .iter()
        .filter(|(_, impact)| impact.ricochet.is_some())
        .count();
    if ricochets > 0 {
        lines.push(format!("Ricocheted: {}", count(ricochets, "time", "times")));
    }
    Text { title, lines }
}

/// Where a shot hit something (its impact `index`).
fn hit(shot: &Preview, impact: &Impact, index: usize, context: &Context) -> Text {
    let units = context.units;
    let what = hit_thing(impact, context);
    let title = match impact.stop {
        Some(Stop::Explodes) => format!("Explodes on {what}"),
        Some(_) => format!("Stops in {what}"),
        None if impact.ricochet.is_some() => format!("Ricochets off {what}"),
        None if !impact.comes_out => format!("Goes into {what}"),
        None => format!("Pierces {what}"),
    };
    let mut lines = Vec::new();
    let mut strength = format!("Strength: {}", ap(impact.damage));
    if context.extended {
        strength = format!(
            "Strength: {} dmg ({})",
            num(impact.damage),
            ap(impact.damage)
        );
    }
    if let Some(needed) = break_off(context.vehicle, impact.brick) {
        let breaks = shot.breaks.get(index).copied().unwrap_or(false);
        let verb = if breaks { "breaks it off" } else { "it holds" };
        strength += &format!(", {verb} (needs {})", ap(needed));
    }
    if context.extended {
        lines.push(since_fired(impact.time, impact.range, units));
    }
    lines.push(strength);
    let left = pierce(shot, impact.damage) * (1.0 - impact.used).max(0.0);
    let full = pierce(shot, impact.damage);
    match impact.stop {
        Some(Stop::Explodes) => {
            lines.insert(0, format!("Ends here: {EXPLODES_FIRST}"));
            if let Some(explosion) = explosion(shot, units) {
                lines.push(format!("Explosion: {explosion}"));
            }
        }
        Some(stop) => {
            let why = no_ricochet(stop, units);
            lines.insert(0, format!("Ends here: can't go through, and {why}"));
            let needs = if impact.armor.is_finite() {
                format!("needs {}", armor(impact.armor))
            } else {
                "no far side to come out of".to_owned()
            };
            lines.push(format!(
                "{}, {needs}",
                penetration_left(shot, impact.penetration, full)
            ));
        }
        None if impact.through => lines.push(format!(
            "Penetration: {} → {}",
            armor(impact.penetration),
            armor(left)
        )),
        None => {}
    }
    if impact.through && !impact.comes_out {
        // What it ended at or turned off inside.
        let next = shot.flight.impacts[index + 1..]
            .iter()
            .find(|later| later.stop.is_some() || later.ricochet.is_some());
        if let Some(next) = next {
            let how = if next.ricochet.is_some() {
                "ricochets off"
            } else {
                "ends at"
            };
            lines.push(format!(
                "Doesn't come out: {how} {} {} in",
                hit_thing(next, context),
                units.short(next.range - impact.range)
            ));
        }
    }
    if let Some(ricochet) = impact.ricochet {
        lines.push(format!(
            "Ricochet: keeps {}% of its speed, scatters up to {}°",
            num(ricochet.speed_kept * 100.0),
            num(ricochet.scatter)
        ));
    }
    if !context.extended {
        return Text { title, lines };
    }
    if impact.thickness > 0.0 && impact.thickness.is_finite() {
        let density = if (impact.density - 1.0).abs() > 1e-3 {
            format!(" × density {}", num(impact.density))
        } else {
            String::new()
        };
        lines.push(format!(
            "Armor: {}{density} = {}",
            units.short(impact.thickness),
            armor(impact.armor)
        ));
    }
    if let Some(angle) = impact.angle {
        lines.push(format!("Angle: {}° off square", num(angle)));
    }
    lines.push(format!(
        "Speed: {}",
        units.speed(math::length(impact.velocity))
    ));
    if impact.through && impact.veer > 0.0 {
        lines.push(format!("Turns: up to {}°", num(impact.veer)));
    }
    if impact.brick.is_some() {
        lines.push("In game, this vehicle's own shots pass through it".to_owned());
    }
    Text { title, lines }
}

/// What a shot hit, for people: the brick's name, or the map.
fn hit_thing(impact: &Impact, context: &Context) -> String {
    match impact.brick {
        Some(brick) => (context.name)(brick).unwrap_or_else(|| "a brick".to_owned()),
        None => "the map".to_owned(),
    }
}

/// Where a shot ran out of range (or past what the preview follows).
fn end(shot: &Preview, context: &Context) -> Text {
    let units = context.units;
    let last = shot.flight.points.last().copied().unwrap_or(Point {
        at: [0.0; 3],
        range: 0.0,
        time: 0.0,
        velocity: [0.0; 3],
    });
    let damage = shot.params.damage_at(last.range) * shot.stack as f64;
    let left = pierce(shot, damage) * (1.0 - shot.flight.used).max(0.0);
    let ending = if shot.flight.ending == Ending::Range {
        format!("Ends here: out of range, {}", units.length(last.range))
    } else {
        format!(
            "Drawn this far: {}, the preview follows it no further",
            units.length(last.range)
        )
    };
    let mut lines = vec![ending];
    if context.extended {
        lines.push(since_fired(last.time, last.range, units));
        lines.push(format!("Strength: {} dmg ({})", num(damage), ap(damage)));
        lines.push(penetration_left(shot, left, pierce(shot, damage)));
        lines.push(format!(
            "Speed: {}",
            units.speed(math::length(last.velocity))
        ));
    } else {
        lines.push(format!("Strength: {}", ap(damage)));
    }
    Text {
        title: shot_title(shot, context),
        lines,
    }
}

/// Where a shot's damage stops dropping (`point`): it's at its weakest from there on.
fn weakest(shot: &Preview, point: Point, context: &Context) -> Text {
    let units = context.units;
    let damage = shot.params.damage_at(point.range) * shot.stack as f64;
    let full = shot.params.damage_at(0.0) * shot.stack as f64;
    let left = pierce(shot, damage) * (1.0 - used_before(shot, point.range)).max(0.0);
    let mut lines = vec![format!("Drop-off ends: {}", units.length(point.range))];
    if context.extended {
        lines.push(since_fired(point.time, point.range, units));
        lines.push(format!(
            "Strength from here on: {} dmg ({}), {}% of {}",
            num(damage),
            ap(damage),
            num(damage / full * 100.0),
            ap(full)
        ));
        lines.push(penetration_left(shot, left, pierce(shot, damage)));
        lines.push(format!(
            "Speed: {}",
            units.speed(math::length(point.velocity))
        ));
    } else {
        lines.push(format!("Strength from here on: {}", ap(damage)));
    }
    Text {
        title: shot_title(shot, context),
        lines,
    }
}

/// A brick of the vehicle shots hit: how hard, and whether it breaks off.
fn brick_hits(shots: &[Preview], brick: usize, context: &Context) -> Text {
    let title = (context.name)(brick).unwrap_or_else(|| "Brick".to_owned());
    let hits: Vec<(&Preview, &Impact, bool)> = shots
        .iter()
        .flat_map(|shot| {
            shot.flight
                .impacts
                .iter()
                .enumerate()
                .filter(|(_, impact)| impact.brick == Some(brick))
                .map(move |(index, impact)| {
                    (
                        shot,
                        impact,
                        shot.breaks.get(index).copied().unwrap_or(false),
                    )
                })
        })
        .collect();
    let strongest = hits
        .iter()
        .map(|(_, impact, _)| impact.damage)
        .fold(0.0, f64::max);
    let breaks = hits.iter().any(|&(_, _, breaks)| breaks);
    let mut summary = format!(
        "Hit by {}, {}",
        count(hits.len(), "shot", "shots"),
        if breaks { "breaks off" } else { "holds" }
    );
    if let Some(needed) = break_off(context.vehicle, Some(brick)) {
        summary += &format!(": {} hit, needs {}", ap(strongest), ap(needed));
    }
    let mut lines = vec![summary];
    if context.extended {
        for (shot, impact, _) in hits.iter().take(6) {
            let what = match impact.stop {
                Some(Stop::Explodes) => "explodes",
                Some(_) => "stops",
                None if impact.ricochet.is_some() => "ricochets",
                None => "pierces",
            };
            let thickness = if impact.thickness.is_finite() {
                format!("{} through", context.units.short(impact.thickness))
            } else {
                "no far side".to_owned()
            };
            lines.push(format!(
                "{}: {} dmg ({}), {what}, {thickness}",
                shot_title(shot, context),
                num(impact.damage),
                ap(impact.damage),
            ));
        }
        if hits.len() > 6 {
            lines.push(format!("and {} more", hits.len() - 6));
        }
        if let Some(density) = context.vehicle.and_then(|vehicle| {
            vehicle
                .targets
                .iter()
                .find(|target| target.brick == brick)
                .map(|target| target.density)
        }) {
            lines.push(format!("Density: {}", num(density)));
        }
    }
    Text { title, lines }
}

/// The shot's title: its guns (how many stacked), and its ammo unless it's the default.
fn shot_title(shot: &Preview, context: &Context) -> String {
    let name = shot
        .guns
        .first()
        .and_then(|&gun| (context.name)(gun))
        .unwrap_or_else(|| "Gun".to_owned());
    let mut title = if shot.stack > 1 {
        format!("{}x {name}", shot.stack)
    } else {
        name
    };
    if shot.ammo != "Default" {
        title += &format!(" ({})", ammo_name(&shot.ammo));
    }
    title
}

/// An `EAmmoType` name for people: "HighExplosive" is "High explosive".
fn ammo_name(ammo: &str) -> String {
    let mut name = String::new();
    for (index, letter) in ammo.chars().enumerate() {
        if index > 0 && letter.is_uppercase() {
            name.push(' ');
            name.extend(letter.to_lowercase());
        } else {
            name.push(letter);
        }
    }
    name
}

/// How fast the shot's guns fire, if that can be told: "10 shots/s, 100-round magazine, 3 s
/// reload", or with a 1-round magazine, which reloads after every shot, "a shot every 1.5 s
/// (reloads each)" (the longer of the reload and the least time between shots).
fn fire_rate(shot: &Preview) -> Option<String> {
    let fire = &shot.fire;
    let cycle = fire.cycle.filter(|&cycle| cycle > 0.0);
    let reload = fire.reload.filter(|&reload| reload > 0.0);
    let capacity = fire.capacity.filter(|&capacity| capacity > 0);
    let mut parts = Vec::new();
    match (capacity, reload) {
        (Some(1), Some(reload)) => {
            let every = cycle.map_or(reload, |cycle| cycle.max(reload));
            parts.push(format!("{}/shot", time(every)));
        }
        _ => {
            parts.push(format!("{} shots/s", num(1.0 / cycle?)));
            if let Some(burst) = fire.burst.filter(|&burst| burst > 0) {
                parts.push(format!("bursts of {burst}"));
            }
            if let Some(capacity) = capacity {
                parts.push(format!("{capacity}-round magazine"));
            }
            if let Some(reload) = reload {
                parts.push(format!("{} reload", time(reload)));
            }
        }
    }
    if shot.info.per_shot > 1 {
        parts.insert(1, format!("{} projectiles each", shot.info.per_shot));
    }
    Some(parts.join(", "))
}

/// The explosive the shot carries, for explosive ammo: "eq. 2 L of C4".
fn explosion(shot: &Preview, units: Units) -> Option<String> {
    if !shot.explosive {
        return None;
    }
    let ratio = if shot.homing {
        shot.projectile.homing_explosive_ratio
    } else {
        1.0
    };
    let liters = shot.projectile.explosive_volume * shot.stack as f64 * ratio;
    let filler = if shot.ammo == "Incendiary" {
        "petrol"
    } else {
        "C4"
    };
    Some(format!("eq. {} of {filler}", units.volume(liters)))
}

/// Things the preview doesn't show as they'll be.
fn notes(shot: &Preview, lines: &mut Vec<String>) {
    if shot.homing {
        lines.push("Drawn unguided".to_owned());
    }
}

/// "0.85 s / 520 m since fired".
fn since_fired(seconds: f64, range: f64, units: Units) -> String {
    format!("{} / {} since fired", time(seconds), units.length(range))
}

/// The damage that breaks off `brick`, if it's the vehicle's and that can be told.
fn break_off(vehicle: Option<&Vehicle>, brick: Option<usize>) -> Option<f64> {
    vehicle?
        .break_off
        .get(&brick?)
        .map(|&needed| f64::from(needed))
}

/// What explosive ammo does instead of going through things (`ABrickProjectile::ApplyDamage`, BRMK
/// 0x30D9B0, returns true for it on any hit, and the projectile's movement then stops).
const EXPLODES_FIRST: &str = "explodes at the first thing it hits";

/// "Penetration left: 3 AP of 5 AP" (`left` of `full`, as armor), or that explosive ammo never goes
/// through anything.
fn penetration_left(shot: &Preview, left: f64, full: f64) -> String {
    if shot.explosive {
        return format!("Penetration: none, {EXPLODES_FIRST}");
    }
    if left < full * 0.999 {
        format!("Penetration left: {} of {}", armor(left), armor(full))
    } else {
        format!("Penetration left: {}", armor(left))
    }
}

/// Why a shot that can't go through something stops in it rather than ricocheting off it.
fn no_ricochet(stop: Stop, units: Units) -> String {
    match stop {
        Stop::NoRicochet => "its projectile never ricochets".to_owned(),
        Stop::JustExited => format!(
            "too soon after piercing (within {}) to ricochet",
            units.short(AFTER_EXIT)
        ),
        Stop::Square => format!("too square to ricochet (within {}°)", num(RICOCHET_ANGLE)),
        Stop::Slow => "too slow to ricochet".to_owned(),
        Stop::TooManyRicochets => "ricocheted more than the preview follows".to_owned(),
        Stop::Explodes => EXPLODES_FIRST.to_owned(),
    }
}

/// The penetration `damage` gives the shot's projectile (cm of density-1 material).
fn pierce(shot: &Preview, damage: f64) -> f64 {
    PENETRATION_PER_DAMAGE * shot.projectile.penetration_coefficient * damage
}

/// The share of its penetration the shot used up before `range` cm.
fn used_before(shot: &Preview, range: f64) -> f64 {
    shot.flight
        .impacts
        .iter()
        .filter(|impact| impact.through && impact.range < range)
        .map(|impact| impact.used)
        .fold(0.0, f64::max)
}

/// How far below the line the muzzle aims along `at` is (cm; negative: above).
fn drop(shot: &Preview, at: math::Vec3) -> Option<f64> {
    let first = shot.flight.points.first()?;
    let aim = math::normalized(first.velocity)?;
    let along = math::dot(math::sub(at, first.at), aim);
    let on_line = math::add(first.at, math::scale(aim, along));
    Some(on_line[2] - at[2])
}

/// The space around the signs of sums: thin (U+2009). If the game's font lacks it, `' '`.
const SUM_SPACE: char = '\u{2009}';

/// "7.5 + 7.5 = 15 dmg" (no barrels, what barrels add, with them), all in `unit`, or just the value
/// if barrels change nothing. What barrels add is told from the other two as shown, so the sum
/// adds up.
fn gained(base: f64, value: f64, unit: Unit) -> String {
    if (value - base).abs() <= value.abs() * 1e-3 {
        return unit.show(value);
    }
    let change = shown(value / unit.size) - shown(base / unit.size);
    let (sign, change) = if change >= 0.0 {
        ('+', change)
    } else {
        ('−', -change)
    };
    let space = SUM_SPACE;
    format!(
        "{}{space}{sign}{space}{}{space}={space}{}",
        unit.bare(base),
        num(change),
        unit.show(value)
    )
}

/// `value` as [`num`] shows it.
fn shown(value: f64) -> f64 {
    num(value).replace(',', "").parse().unwrap_or(value)
}

/// `damage` in AP.
fn ap(damage: f64) -> String {
    format!("{} AP", num(damage / AP_DAMAGE))
}

/// Armor or penetration (cm of density-1 material) in AP.
fn armor(armor: f64) -> String {
    format!("{} AP", num(armor / AP_PENETRATION))
}

/// `count` of a thing, named in the singular or the plural.
fn count(count: usize, one: &str, many: &str) -> String {
    match count {
        1 => format!("1 {one}"),
        count => format!("{count} {many}"),
    }
}

/// A time: in ms under a second.
fn time(seconds: f64) -> String {
    if seconds < 1.0 {
        format!("{} ms", num(seconds * 1000.0))
    } else {
        format!("{} s", num(seconds))
    }
}

/// A unit values are shown in: how many of the game's (cm, cm/s, damage) one is, and its symbol.
#[derive(Clone, Copy)]
pub struct Unit {
    size: f64,
    symbol: &'static str,
}

impl Unit {
    const DAMAGE: Self = Self::new(1.0, "dmg");

    const fn new(size: f64, symbol: &'static str) -> Self {
        Self { size, symbol }
    }

    /// `value` (in the game's unit) in this one, without the symbol.
    fn bare(self, value: f64) -> String {
        num(value / self.size)
    }

    /// `value` (in the game's unit) in this one: "12 m".
    fn show(self, value: f64) -> String {
        format!("{} {}", self.bare(value), self.symbol)
    }
}

impl Units {
    /// The unit a distance (cm) reads best in: cm, m or km; in, ft or mi.
    pub fn length_unit(self, cm: f64) -> Unit {
        let cm = cm.abs();
        if self.imperial {
            let feet = cm / 30.48;
            if feet < 3.0 {
                Unit::new(2.54, "in")
            } else if feet < 5280.0 {
                Unit::new(30.48, "ft")
            } else {
                Unit::new(30.48 * 5280.0, "mi")
            }
        } else if cm < 100.0 {
            Unit::new(1.0, "cm")
        } else if cm < 100_000.0 {
            Unit::new(100.0, "m")
        } else {
            Unit::new(100_000.0, "km")
        }
    }

    /// A distance (cm), in [`Self::length_unit`].
    pub fn length(self, cm: f64) -> String {
        self.length_unit(cm).show(cm)
    }

    /// The unit of small lengths: cm or in.
    pub fn short_unit(self) -> Unit {
        if self.imperial {
            Unit::new(2.54, "in")
        } else {
            Unit::new(1.0, "cm")
        }
    }

    /// A small length (cm): cm or in.
    pub fn short(self, cm: f64) -> String {
        self.short_unit().show(cm)
    }

    /// Two small lengths (cm), as "120/240 cm".
    pub fn short_pair(self, a: f64, b: f64) -> String {
        let unit = self.short_unit();
        format!("{}/{}", unit.bare(a), unit.show(b))
    }

    /// The unit of speeds: m/s or ft/s.
    pub fn speed_unit(self) -> Unit {
        if self.imperial {
            Unit::new(30.48, "ft/s")
        } else {
            Unit::new(100.0, "m/s")
        }
    }

    /// A speed (cm/s): m/s or ft/s.
    pub fn speed(self, cm_per_second: f64) -> String {
        self.speed_unit().show(cm_per_second)
    }

    /// A volume (liters): L or gal (US).
    pub fn volume(self, liters: f64) -> String {
        if self.imperial {
            format!("{} gal", num(liters * 0.264_172))
        } else {
            format!("{} L", num(liters))
        }
    }
}

/// `value` for people: three significant digits or so (none after the point from 100 up), its
/// thousands grouped.
fn num(value: f64) -> String {
    if !value.is_finite() {
        return "?".to_owned();
    }
    let size = value.abs();
    let decimals = if size >= 100.0 || size == 0.0 {
        0
    } else if size >= 10.0 {
        1
    } else if size >= 1.0 {
        2
    } else {
        (1 - size.log10().floor() as i32).clamp(2, 6) as usize
    };
    let text = format!("{size:.decimals$}");
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, ""));
    let fraction = fraction.trim_end_matches('0');
    let mut grouped = String::new();
    for (index, digit) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    let sign = if value < 0.0 && (grouped != "0" || !fraction.is_empty()) {
        "-"
    } else {
        ""
    };
    if fraction.is_empty() {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}.{fraction}")
    }
}
