//! Limits for property values. Edit this file to change them.
//!
//! Every numeric property edited through a property panel (vehicle editor, settings menus) gets
//! the limits of the first rule below matching its class and name, or [`DEFAULT`]'s. Limits apply
//! when a value is edited; loaded vehicles keep the values they were saved with.
//!
//! To find a property's names, edit it in game, then run `et.py log`: the first edit of each
//! property logs a line like
//! `ScalableBrick.BrickSize: game range [10.0, 10.0, 10.0]..[9000.0, 9000.0, 9000.0], now ...`.
//!
//! A rule starts from `rule("Class", "Property")`, which has no limits, then narrows them:
//! - `.min(x)` / `.max(x)`: the same limit for every component;
//! - `.min_each([x, y, z])` / `.max_each([x, y, z])`: one limit per component;
//! - `.vanilla_min()` / `.vanilla_max()` / `.vanilla()`: keep the game's limits;
//! - `.adjust(function)`: run `function` on each new value, for limits depending on several
//!   components or on other properties (see `brick_size` below);
//! - `.depends_on(&["Other"])`: for an `adjust` reading other properties (floats and vectors
//!   only), also check the value when one of those is edited. Values `adjust` allowed before that
//!   edit but not after are limited; values outside of their limits already are kept. The game's
//!   limits (`.vanilla_*()`) aren't known then and don't apply, except for brick sizes.
//!
//! Whatever the rule, values stay within what their type can hold (integer properties) and within
//! ±`SAFE_MAX` (about 2.66e36), past which the game can't display them. Where nothing limits them,
//! they stop at ±[`UNLIMITED`].

use super::limits::{Edit, Rule, rule};
use crate::game::scalable_brick::{self, DENSEST_CONNECTOR_SPACING};

/// How far values may go where nothing limits them: properties without a rule, and rules without a
/// `.min`/`.max`. The game multiplies values by its own constants, which overflow to infinity past
/// some point: a light's brightness of 2.66e36 (times 650 and its type's intensity) crashed the
/// game when the light was selected (2026-10-09). 1e15 was the highest found safe by trying.
pub const UNLIMITED: f32 = 1.0e15;

/// Limits for properties without a rule below: none of their own (so ±[`UNLIMITED`]).
/// `rule("", "").vanilla()` would keep the game's limits everywhere, unclamping only the
/// properties listed below.
pub const DEFAULT: Rule = rule("", "");

pub const RULES: &[Rule] = &[
    // Brick size (cm): never below the game's minimum; no fixed maximum besides a sanity cap, but a
    // budget of connector points (see `brick_size`), which depends on the connector spacing.
    rule("ScalableBrick", "BrickSize")
        .min(0.0)
        .max(MAX_BRICK_SIZE)
        .adjust(brick_size)
        .depends_on(&["ConnectorSpacing"]),
    // Light brightness (1.0 = 100%): limited while the light shines (see `light_brightness`).
    rule("LightBrick", "Brightness")
        .adjust(light_brightness)
        .depends_on(&["LightDirection"]),
    // Example: up to 100 m (10000 cm) on every axis, keeping the game's minimum.
    // rule("SomeBrick", "SomeRadius").vanilla_min().max(10_000.0),
];

/// Most brightness a shining light may have, either way: 1000% (the game allows 0 to 100%).
const MAX_SHINING_BRIGHTNESS: f32 = 10.0;

/// `LightDirection` of a light brick that doesn't shine, only glows: "Off"
/// (`ELightBrickDirection::None`; the others are Omnidirectional and the six axis directions).
const LIGHT_OFF: u8 = 0;

/// Keeps a shining light's brightness within ±[`MAX_SHINING_BRIGHTNESS`]. One set to Off only
/// glows, which takes far larger values (up to ±[`UNLIMITED`]).
fn light_brightness(edit: &Edit, brightness: &mut [f32; 3]) {
    if edit.read::<u8>("LightDirection") != Some(LIGHT_OFF) {
        brightness[0] = brightness[0].clamp(-MAX_SHINING_BRIGHTNESS, MAX_SHINING_BRIGHTNESS);
    }
}

/// Largest brick size per axis, in cm: a million kilometres.
const MAX_BRICK_SIZE: f32 = 1.0e11;

/// Most connector points one brick may have. Bricks in the billions make vehicles lag when they
/// spawn; the game's own largest brick has under 5 million.
const MAX_CONNECTOR_POINTS: f64 = 50.0e6;

/// Brick sizes shrunk by the connector budget snap down to this grid, in cm.
const BRICK_GRID: f32 = 10.0;

/// Keeps a brick within [`MAX_CONNECTOR_POINTS`]: if its new size has more, the axes that grew
/// shrink together until it fits (all axes if none grew).
fn brick_size(edit: &Edit, size: &mut [f32; 3]) {
    // Faces without connectors don't count. If the spacing can't be read, assume the densest.
    let spacing = edit
        .read::<u16>("ConnectorSpacing")
        .unwrap_or(DENSEST_CONNECTOR_SPACING);
    if scalable_brick::connector_points(*size, spacing) <= MAX_CONNECTOR_POINTS {
        return;
    }

    // While a new size is being stored, the property still holds the previous one.
    let before = edit.read::<[f32; 3]>("BrickSize").unwrap_or(*size);
    let mut grown: [bool; 3] = core::array::from_fn(|axis| size[axis] > before[axis]);
    if !grown.contains(&true) {
        grown = [true; 3];
    }
    let min = edit.vanilla_min();
    let scaled = |scale: f32| -> [f32; 3] {
        core::array::from_fn(|axis| {
            if grown[axis] {
                let snapped = (size[axis] * scale / BRICK_GRID).floor() * BRICK_GRID;
                snapped.max(min[axis])
            } else {
                size[axis]
            }
        })
    };

    // Smaller bricks have fewer points: binary search for the largest scale that fits.
    let (mut fits, mut too_big) = (0.0, 1.0);
    for _ in 0..40 {
        let middle = (fits + too_big) / 2.0;
        if scalable_brick::connector_points(scaled(middle), spacing) <= MAX_CONNECTOR_POINTS {
            fits = middle;
        } else {
            too_big = middle;
        }
    }
    *size = scaled(fits);
}
