//! The limits machinery: which rule applies to an edited value and what range it gives. The rules
//! themselves are in `rules.rs`.

use core::ffi::c_void;
use std::sync::{Mutex, PoisonError};

use brickrust::ue::fproperty::FProperty;
use brickrust::ue::uclass::UStruct;

use super::rules;
use crate::game::numeric_property::{NumericRange, NumericValue};
use crate::game::reflection;
use crate::prelude::*;

/// Largest magnitude any value may reach. The panel shows values times a unit factor of up to 100
/// (percent), and a shown value that overflows to infinity froze the game: Unreal's number
/// formatting loops forever on infinity. A 128th of the float maximum keeps shown values finite.
pub const SAFE_MAX: f32 = f32::MAX / 128.0;

/// Integer property types and the values they can hold, as floats (past 2^24 not every integer is
/// a float: the maximums are the largest floats that still fit).
const INTEGER_RANGES: &[(&str, f32, f32)] = &[
    ("ByteProperty", 0.0, 255.0),
    ("Int8Property", -128.0, 127.0),
    ("Int16Property", -32_768.0, 32_767.0),
    ("UInt16Property", 0.0, 65_535.0),
    ("IntProperty", -2_147_483_648.0, 2_147_483_520.0),
    ("UInt32Property", 0.0, 4_294_967_040.0),
    ("Int64Property", -9.223_372e18, 9.223_371_5e18),
    ("UInt64Property", 0.0, 1.844_674_3e19),
];

/// One side of a rule's range.
#[allow(dead_code)] // building blocks for `rules.rs`, not all in use at any time
#[derive(Clone, Copy)]
pub enum Bound {
    /// What the game allows on this side.
    Vanilla,
    /// No limit of its own: ±[`rules::UNLIMITED`], or less if the value's type can't hold that.
    Unlimited,
    /// The same limit for every component.
    All(f32),
    /// One limit per component (x, y, z).
    Each([f32; 3]),
}

/// Adjusts a value after it was clamped to its rule's range, for limits a plain range can't express
/// (several components at once, other properties). Gets the value's components (unused ones are 0);
/// the result is clamped to the range again.
pub type Adjust = fn(&Edit, &mut [f32; 3]);

/// Most bytes a property may have for rules to depend on it (see [`Rule::depends_on`]): a vector's.
pub const MAX_DEPENDENCY_SIZE: usize = 12;

/// Limits for one property. Start from [`rule`] and narrow them with the methods.
#[derive(Clone, Copy)]
pub struct Rule {
    /// Class or struct of the edited object, without its `U`/`A`/`F` prefix (e.g. `ScalableBrick`);
    /// a class also matches its subclasses. Empty matches any.
    pub class: &'static str,
    /// Property name, as the game's reflection data spells it (e.g. `BrickSize`).
    pub property: &'static str,
    pub min: Bound,
    pub max: Bound,
    /// Where dragging a slider stops on the low side, if above `min` (see [`Rule::drag_min`]).
    pub drag_min: Option<f32>,
    pub adjust: Option<Adjust>,
    /// Other properties of the same object that `adjust` reads (see `dependencies`).
    pub depends_on: &'static [&'static str],
}

/// The rule for `property` of objects of `class` (the first matching one), if any.
///
/// # Safety
/// `class` must be a live class or struct.
pub unsafe fn rule_for(class: *const UStruct, property: &str) -> Option<&'static Rule> {
    rules::RULES
        .iter()
        // SAFETY: per the contract.
        .find(|rule| rule.property == property && unsafe { rule.matches_class(class) })
}

/// A rule for `class`'s `property` without any limit.
pub const fn rule(class: &'static str, property: &'static str) -> Rule {
    Rule {
        class,
        property,
        min: Bound::Unlimited,
        max: Bound::Unlimited,
        drag_min: None,
        adjust: None,
        depends_on: &[],
    }
}

#[allow(dead_code)] // building blocks for `rules.rs`, not all in use at any time
impl Rule {
    /// The same minimum for every component.
    pub const fn min(self, min: f32) -> Self {
        Self {
            min: Bound::All(min),
            ..self
        }
    }

    /// The same maximum for every component.
    pub const fn max(self, max: f32) -> Self {
        Self {
            max: Bound::All(max),
            ..self
        }
    }

    /// One minimum per component (x, y, z).
    pub const fn min_each(self, min: [f32; 3]) -> Self {
        Self {
            min: Bound::Each(min),
            ..self
        }
    }

    /// One maximum per component (x, y, z).
    pub const fn max_each(self, max: [f32; 3]) -> Self {
        Self {
            max: Bound::Each(max),
            ..self
        }
    }

    /// Keep the game's minimum.
    pub const fn vanilla_min(self) -> Self {
        Self {
            min: Bound::Vanilla,
            ..self
        }
    }

    /// Keep the game's maximum.
    pub const fn vanilla_max(self) -> Self {
        Self {
            max: Bound::Vanilla,
            ..self
        }
    }

    /// Keep the game's range: the property isn't unclamped at all.
    pub const fn vanilla(self) -> Self {
        self.vanilla_min().vanilla_max()
    }

    /// Dragging a slider stops at `min` (every component), above the rule's minimum: values below
    /// it can only be typed. A drag starting below it jumps to it.
    pub const fn drag_min(self, min: f32) -> Self {
        Self {
            drag_min: Some(min),
            ..self
        }
    }

    /// Run `adjust` on every value once it's within the range.
    pub const fn adjust(self, adjust: Adjust) -> Self {
        Self {
            adjust: Some(adjust),
            ..self
        }
    }

    /// Check the value again whenever one of `properties` (of the same object, each at most
    /// [`MAX_DEPENDENCY_SIZE`] bytes) is edited, for an `adjust` that reads them: values it allowed
    /// before that edit but not after it are limited then.
    pub const fn depends_on(self, properties: &'static [&'static str]) -> Self {
        Self {
            depends_on: properties,
            ..self
        }
    }

    /// Whether the rule applies to values of objects of `class` (or of structs of that type).
    ///
    /// # Safety
    /// `class` must be a live class or struct.
    pub unsafe fn matches_class(&self, class: *const UStruct) -> bool {
        // SAFETY: per the contract; parents are live as long as their children.
        self.class.is_empty()
            || unsafe { reflection::with_parents(class) }.any(|class| unsafe {
                reflection::name_is(reflection::struct_name(class), self.class)
            })
    }
}

/// Some properties of an object as they were before an edit, for rules to read instead of the
/// current values (see [`Edit::with_previous`]).
#[derive(Clone, Default)]
pub struct Previous {
    values: Vec<(&'static str, [u8; MAX_DEPENDENCY_SIZE])>,
}

impl Previous {
    /// Records `name`'s value: its bytes, padded with zeros.
    pub fn insert(&mut self, name: &'static str, bytes: [u8; MAX_DEPENDENCY_SIZE]) {
        self.values.push((name, bytes));
    }

    fn get(&self, name: &str) -> Option<&[u8; MAX_DEPENDENCY_SIZE]> {
        self.values
            .iter()
            .find(|(recorded, _)| *recorded == name)
            .map(|(_, bytes)| bytes)
    }
}

/// A value being stored into a reflected property, as rules see it.
pub struct Edit {
    property: *const FProperty,
    /// The object or struct holding the value.
    target: *mut c_void,
    /// Where names are looked up: the edited object's class, or the struct declaring the property.
    class: *const UStruct,
    vanilla_min: [f32; 3],
    vanilla_max: [f32; 3],
    components: usize,
    /// Values [`read`](Self::read) returns instead of the current ones.
    previous: Previous,
}

#[allow(dead_code)] // what rules can ask, not all in use at any time
impl Edit {
    /// `None` if the property isn't declared by a class or struct (e.g. it's an array element).
    ///
    /// # Safety
    /// `property` must be null or a live property of `target`'s type (or a parent of it), and
    /// `target` must stay valid while the `Edit` is used.
    pub unsafe fn new(
        property: *const FProperty,
        target: *mut c_void,
        vanilla: &NumericRange,
        components: usize,
    ) -> Option<Self> {
        if property.is_null() || target.is_null() {
            return None;
        }
        // SAFETY: per the contract.
        let class = unsafe {
            let owner = reflection::declaring_struct(property);
            if owner.is_null() {
                return None;
            }
            // Instances of a class are objects, which know their exact class.
            if reflection::is_class(owner) {
                reflection::class_of(target)
            } else {
                owner
            }
        };
        Some(Self {
            property,
            target,
            class,
            vanilla_min: all_components(&vanilla.min),
            vanilla_max: all_components(&vanilla.max),
            components: components.clamp(1, 3),
            previous: Previous::default(),
        })
    }

    /// The same edit as seen before other properties changed: [`read`](Self::read) returns their
    /// `previous` values.
    pub fn with_previous(self, previous: Previous) -> Self {
        Self { previous, ..self }
    }

    /// How many components the value has (1 to 3).
    pub fn components(&self) -> usize {
        self.components
    }

    /// The game's minimum, per component.
    pub fn vanilla_min(&self) -> [f32; 3] {
        self.vanilla_min
    }

    /// The game's maximum, per component.
    pub fn vanilla_max(&self) -> [f32; 3] {
        self.vanilla_max
    }

    /// A property of the edited object by name, as stored right now: while a new value is being
    /// stored, the edited property itself still holds the previous one (and other properties hold
    /// those given to [`with_previous`](Self::with_previous)). `None` if there's no such property
    /// or it isn't `T`-sized.
    pub fn read<T: Copy>(&self, name: &'static str) -> Option<T> {
        // SAFETY: `class` is the type of `target` (see `new`), so its properties lie within
        // `target`; the size check keeps the read inside the property, and inside the recorded
        // bytes of a previous value (which come from a property of that name and size).
        unsafe {
            let property = reflection::find_property(self.class, name);
            if property.is_null() || usize::try_from((*property).element_size) != Ok(size_of::<T>())
            {
                return None;
            }
            if let Some(bytes) = self.previous.get(name)
                && size_of::<T>() <= bytes.len()
            {
                return Some(bytes.as_ptr().cast::<T>().read_unaligned());
            }
            let offset = (*property).offset_internal as usize;
            Some(self.target.byte_add(offset).cast::<T>().read_unaligned())
        }
    }

    fn matches(&self, rule: &Rule) -> bool {
        // SAFETY: `property` and `class` are live (see `new`).
        unsafe {
            reflection::name_is(&(*self.property).field.name_private, rule.property)
                && rule.matches_class(self.class)
        }
    }

    /// The values the property's type can hold.
    fn type_range(&self) -> (f32, f32) {
        INTEGER_RANGES
            .iter()
            // SAFETY: `property` is live (see `new`).
            .find(|(kind, ..)| unsafe { reflection::is_kind(self.property, kind) })
            .map_or((-SAFE_MAX, SAFE_MAX), |&(_, min, max)| (min, max))
    }

    /// E.g. `ScalableBrick.BrickSize`.
    pub fn describe(&self) -> String {
        // SAFETY: `property` and `class` are live (see `new`), and so is the declaring struct.
        unsafe {
            let name = &(*self.property).field.name_private;
            let class = reflection::struct_name(self.class);
            let owner = reflection::declaring_struct(self.property);
            if owner == self.class {
                format!("{class}.{name}")
            } else {
                format!(
                    "{class}.{name} (declared in {})",
                    reflection::struct_name(owner)
                )
            }
        }
    }
}

/// A value's limits, per component.
#[derive(Clone, Copy)]
pub struct Limits {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Limits {
    /// For values whose property can't be identified (e.g. array elements): no limit of their own,
    /// like properties without a rule.
    pub fn unknown() -> Self {
        Self {
            min: [-rules::UNLIMITED; 3],
            max: [rules::UNLIMITED; 3],
        }
    }

    /// Clamps each component into its limits (NaN becomes the minimum).
    pub fn clamp(&self, value: &mut [f32; 3]) {
        for ((component, min), max) in value.iter_mut().zip(self.min).zip(self.max) {
            *component = component.max(min).min(max);
        }
    }
}

/// The limits for `edit`, and the rule they come from (`None`: [`rules::DEFAULT`]). Logs them the
/// first time the property is edited.
pub fn resolve(edit: &Edit) -> (Limits, Option<&'static Rule>) {
    let (limits, rule) = limits_of(edit);
    log_first_edit(edit, &limits, rule.is_some());
    (limits, rule)
}

/// `value` as its rule allows it for `edit`: within its limits, then adjusted. Unused components
/// may change too, compare only the used ones.
pub fn limited(edit: &Edit, mut value: [f32; 3]) -> [f32; 3] {
    let (limits, rule) = limits_of(edit);
    limits.clamp(&mut value);
    adjust(edit, &limits, rule, &mut value);
    value
}

/// [`resolve`] without logging.
fn limits_of(edit: &Edit) -> (Limits, Option<&'static Rule>) {
    let rule = rules::RULES.iter().find(|rule| edit.matches(rule));
    let applied = rule.unwrap_or(&rules::DEFAULT);
    let (type_min, type_max) = edit.type_range();

    let mut limits = Limits {
        min: side(
            applied.min,
            edit.vanilla_min,
            type_min.max(-rules::UNLIMITED),
        ),
        max: side(
            applied.max,
            edit.vanilla_max,
            type_max.min(rules::UNLIMITED),
        ),
    };
    // If a rule's minimum is above its maximum, the minimum wins.
    for (min, max) in limits.min.iter_mut().zip(&mut limits.max) {
        *min = min.max(type_min).min(type_max);
        *max = max.min(type_max).max(*min);
    }
    (limits, rule)
}

/// Runs the rule's adjustment, if any, on a value already within `limits`, keeping it within them.
pub fn adjust(edit: &Edit, limits: &Limits, rule: Option<&Rule>, value: &mut [f32; 3]) {
    if let Some(adjust) = rule.unwrap_or(&rules::DEFAULT).adjust {
        adjust(edit, value);
        limits.clamp(value);
    }
}

/// One side of the limits: `unlimited` is how far [`Bound::Unlimited`] goes.
fn side(bound: Bound, vanilla: [f32; 3], unlimited: f32) -> [f32; 3] {
    match bound {
        Bound::Vanilla => vanilla,
        Bound::Unlimited => [unlimited; 3],
        Bound::All(limit) => [limit; 3],
        Bound::Each(limits) => limits,
    }
}

/// All three components; a component past `num_used` takes the last used one, like the game does.
fn all_components(value: &NumericValue) -> [f32; 3] {
    let last = usize::from(value.num_used).clamp(1, 3) - 1;
    core::array::from_fn(|i| value.data[i.min(last)])
}

/// Logs each property's names and limits the first time it's edited, to help write rules.
fn log_first_edit(edit: &Edit, limits: &Limits, has_rule: bool) {
    static SEEN: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    {
        let mut seen = SEEN.lock().unwrap_or_else(PoisonError::into_inner);
        if seen.contains(&(edit.property as usize)) {
            return;
        }
        seen.push(edit.property as usize);
    }
    let shown = |values: &[f32; 3]| match edit.components {
        1 => format!("{:?}", values[0]),
        n => format!("{:?}", &values[..n]),
    };
    log!(
        "{}: game range {}..{}, now {}..{} ({})",
        edit.describe(),
        shown(&edit.vanilla_min),
        shown(&edit.vanilla_max),
        shown(&limits.min),
        shown(&limits.max),
        if has_rule { "rule" } else { "default" },
    );
}
