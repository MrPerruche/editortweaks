//! Rules whose `adjust` reads other properties of the object (`Rule::depends_on`, e.g. a light's
//! brightness depending on its direction) are applied again when one of those properties is edited.
//! Otherwise loosening the limit (light Off), raising the value and tightening the limit again
//! (light on) would keep a value the rule doesn't allow.
//!
//! Only values the rule allowed before that edit are limited: one that was outside of its limits
//! already (e.g. edited into the saved vehicle) stays until it's edited itself, as the game does
//! with its own limits.
//!
//! The editor checks it may modify an object right before writing a property to it
//! (`game::brick_property::CAN_MODIFY`): that's when the properties rules depend on are recorded.
//! Once written, it tells the object (`PostModifyBrickProperty`, hooked for scalable bricks, which
//! every brick with such a rule is). If a recorded property changed, the values depending on it
//! are limited where needed before the game handles the change, then the game is told they changed
//! too, as if they had been edited.

use core::ffi::c_void;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;
use brickrust::ue::fproperty::FProperty;
use brickrust::ue::uclass::UStruct;

use super::limits::{self, Edit, MAX_DEPENDENCY_SIZE, Previous, SAFE_MAX};
use super::{brick_size, rules};
use crate::framework::hook;
use crate::game::brick_property::{self, CanModifyFn, ChangedEvent, PostModifyFn};
use crate::game::numeric_property::{Container, NumericRange, NumericValue};
use crate::game::reflection;
use crate::game::scalable_brick;
use crate::game::weak_object::WeakObject;
use crate::prelude::*;

static CAN_MODIFY: OnceLock<CanModifyFn> = OnceLock::new();
static POST_MODIFY: OnceLock<PostModifyFn> = OnceLock::new();

/// How many objects may be recorded before those destroyed since are first forgotten.
const RECORDED_CLEANUP: usize = 4096;

/// The game's range of a value whose range isn't known here: the widest, so rules keeping the
/// game's limits (`.vanilla_min()` etc.) don't limit it when it's checked again.
const UNKNOWN_RANGE: NumericRange = NumericRange {
    min: NumericValue {
        data: [-SAFE_MAX; 3],
        num_used: 3,
    },
    max: NumericValue {
        data: [SAFE_MAX; 3],
        num_used: 3,
    },
};

thread_local! {
    /// Per object (by address): the properties rules depend on, as last recorded.
    static RECORDED: RefCell<HashMap<usize, Recorded>> = RefCell::new(HashMap::new());
    /// How many objects may be recorded before those destroyed since are forgotten again.
    static NEXT_CLEANUP: Cell<usize> = const { Cell::new(RECORDED_CLEANUP) };
    /// Per class (by address): its values whose rule depends on other properties.
    static CLASSES: RefCell<HashMap<usize, Class>> = RefCell::new(HashMap::new());
    /// Set while telling an object about values limited here, so they aren't checked again.
    static REPORTING: Cell<bool> = const { Cell::new(false) };
}

struct Recorded {
    object: WeakObject,
    values: Previous,
}

struct Class {
    class: WeakObject,
    dependents: Rc<[Dependent]>,
}

/// A property of one class whose rule depends on other properties.
struct Dependent {
    property: *const FProperty,
    /// Its number of floats: 1 for a float, 3 for a vector.
    components: usize,
    /// The properties its rule depends on.
    depends_on: Vec<(&'static str, *const FProperty)>,
}

pub(super) struct Targets {
    can_modify: *const u8,
    post_modify: *const u8,
}

pub(super) fn resolve() -> Result<Targets, String> {
    Ok(Targets {
        can_modify: brick_property::CAN_MODIFY.resolve()?,
        post_modify: scalable_brick::POST_MODIFY.resolve()?,
    })
}

/// # Safety
/// `targets` must come from [`resolve`].
pub(super) unsafe fn install(targets: Targets) -> Result<(), String> {
    // SAFETY: each detour matches the hooked function's signature (`CanModifyFn`, `PostModifyFn`).
    unsafe {
        hook::install_typed(
            brick_property::CAN_MODIFY.name,
            targets.can_modify,
            can_modify_detour as CanModifyFn,
            &CAN_MODIFY,
        )?;
        hook::install_typed(
            scalable_brick::POST_MODIFY.name,
            targets.post_modify,
            post_modify_detour as PostModifyFn,
            &POST_MODIFY,
        )
    }
}

unsafe extern "C" fn can_modify_detour(
    edit_info: *const c_void,
    container: *const Container,
) -> u8 {
    let original = *CAN_MODIFY.get().expect("hook recorded before use");
    if super::slider::active() && !container.is_null() {
        // SAFETY: the editor passes a live container, whose root is a live object or null.
        unsafe { record((*container).root().cast()) };
    }
    // SAFETY: forwards the caller's arguments unchanged.
    unsafe { original(edit_info, container) }
}

unsafe extern "C" fn post_modify_detour(interface: *mut c_void, event: *mut ChangedEvent) {
    let original = *POST_MODIFY.get().expect("hook recorded before use");
    if !super::slider::active() || REPORTING.get() || interface.is_null() || event.is_null() {
        // SAFETY: forwards the caller's arguments unchanged.
        return unsafe { original(interface, event) };
    }
    // SAFETY: the game passes a live brick's `IBrickPropertyInterface` and an event that stays
    // valid during the call. UI code only runs on the game thread.
    unsafe {
        let object = brick_property::object_of(interface);
        let limited = limit_dependents(object, &*event);
        original(interface, event);
        for property in limited {
            report(interface, &mut *event, property);
        }
        record(object);
    }
}

/// Records the properties rules depend on, as `object` holds them now.
///
/// # Safety
/// `object` must be null or a live object. Game thread only.
unsafe fn record(object: *mut UObjectBase) {
    if object.is_null() {
        return;
    }
    // SAFETY: per the contract.
    let dependents = unsafe { dependents(reflection::class_of(object.cast())) };
    if dependents.is_empty() {
        return;
    }
    let mut values = Previous::default();
    for &(name, property) in dependents.iter().flat_map(|d| &d.depends_on) {
        let mut bytes = [0; MAX_DEPENDENCY_SIZE];
        // SAFETY: `property` belongs to the object's class and has at most MAX_DEPENDENCY_SIZE
        // bytes (checked in `find_dependents`).
        unsafe {
            let size = (*property).element_size as usize;
            let field = object
                .byte_add((*property).offset_internal as usize)
                .cast::<u8>();
            core::ptr::copy_nonoverlapping(field, bytes.as_mut_ptr(), size);
        }
        values.insert(name, bytes);
    }
    // SAFETY: per the contract.
    let object_ref = unsafe { WeakObject::new(object) };
    RECORDED.with_borrow_mut(|recorded| {
        if recorded.len() >= NEXT_CLEANUP.get() {
            recorded.retain(|_, entry| entry.object.get().is_some());
            NEXT_CLEANUP.set((2 * recorded.len()).max(RECORDED_CLEANUP));
        }
        recorded.insert(
            object as usize,
            Recorded {
                object: object_ref,
                values,
            },
        );
    });
}

/// Limits the values whose rules depend on the property `event` names, where its change made
/// them break their rule. Returns the properties limited.
///
/// # Safety
/// `object` must be a live object. Game thread only.
unsafe fn limit_dependents(
    object: *mut UObjectBase,
    event: &ChangedEvent,
) -> Vec<*const FProperty> {
    let Some(changed) = event.property() else {
        return Vec::new(); // everything changed (e.g. undo): nothing in particular
    };
    static SEEN: AtomicBool = AtomicBool::new(false);
    if !SEEN.swap(true, Ordering::Relaxed) {
        log!("checking limits on property changes (first: {changed})");
    }
    // SAFETY: per the contract.
    let dependents = unsafe { dependents(reflection::class_of(object.cast())) };
    let is_changed = |name: &'static str| {
        // SAFETY: names in the game's name table live as long as the game.
        unsafe { changed.equals_str(name) }
    };
    let affected: Vec<&Dependent> = dependents
        .iter()
        .filter(|d| d.depends_on.iter().any(|&(name, _)| is_changed(name)))
        .collect();
    if affected.is_empty() {
        return Vec::new();
    }

    let previous = RECORDED.with_borrow(|recorded| {
        recorded
            .get(&(object as usize))
            .filter(|entry| entry.object.get() == Some(object))
            .map(|entry| entry.values.clone())
    });
    let Some(previous) = previous else {
        static LOGGED: AtomicBool = AtomicBool::new(false);
        if !LOGGED.swap(true, Ordering::Relaxed) {
            log!(
                "{changed} changed without its previous value recorded: values depending on it are kept"
            );
        }
        return Vec::new();
    };
    affected
        .into_iter()
        // SAFETY: per the contract, and each dependent belongs to the object's class.
        .filter(|dependent| unsafe { limit(object.cast(), dependent, &previous, changed) })
        .map(|dependent| dependent.property)
        .collect()
}

/// Limits `dependent`'s value if its rule allowed it with the `previous` values of the properties
/// it depends on, but doesn't now that `changed` changed. Returns whether it did.
///
/// # Safety
/// `object` must be a live object of the class `dependent` belongs to. Game thread only.
unsafe fn limit(
    object: *mut c_void,
    dependent: &Dependent,
    previous: &Previous,
    changed: FName,
) -> bool {
    let used = dependent.components;
    // SAFETY: per the contract, the property holds `used` floats (see `float_count`).
    let field = unsafe {
        object
            .byte_add((*dependent.property).offset_internal as usize)
            .cast::<f32>()
    };
    let mut value = [0.0; 3];
    // SAFETY: as above.
    unsafe { core::ptr::copy_nonoverlapping(field, value.as_mut_ptr(), used) };

    // SAFETY: per the contract.
    let vanilla =
        unsafe { brick_size::game_range_of(object, dependent.property) }.unwrap_or(UNKNOWN_RANGE);
    // SAFETY: per the contract.
    let edit = || unsafe { Edit::new(dependent.property, object, &vanilla, used) };
    let (Some(before), Some(now)) = (edit(), edit()) else {
        return false;
    };
    let before = before.with_previous(previous.clone());

    let limited = limits::limited(&now, value);
    if limited[..used] == value[..used] {
        return false;
    }
    if limits::limited(&before, value)[..used] != value[..used] {
        log!(
            "{}: kept {:?}, outside of its limits before {changed} changed too",
            now.describe(),
            &value[..used]
        );
        return false;
    }
    // SAFETY: as above.
    unsafe { core::ptr::copy_nonoverlapping(limited.as_ptr(), field, used) };
    log!(
        "{}: {:?} limited to {:?} ({changed} changed)",
        now.describe(),
        &value[..used],
        &limited[..used]
    );
    true
}

/// Tells the object behind `interface` that `property` changed, with `event` (which named the
/// property edited) naming it instead.
///
/// # Safety
/// `interface` must point to a live object's `IBrickPropertyInterface` and `property` be one of
/// its class's properties. Game thread only.
unsafe fn report(interface: *mut c_void, event: &mut ChangedEvent, property: *const FProperty) {
    // SAFETY: per the contract.
    let name = unsafe { (*property).field.name_private };
    event.renamed(name, |event| {
        REPORTING.set(true);
        // SAFETY: per the contract.
        unsafe { brick_property::post_modify(interface, event) };
        REPORTING.set(false);
    });
}

/// The values of `class` whose rules depend on other properties (found once per class).
///
/// # Safety
/// `class` must be a live class. Game thread only.
unsafe fn dependents(class: *const UStruct) -> Rc<[Dependent]> {
    let cached = CLASSES.with_borrow(|classes| {
        classes
            .get(&(class as usize))
            .filter(|entry| entry.class.get().is_some())
            .map(|entry| entry.dependents.clone())
    });
    if let Some(dependents) = cached {
        return dependents;
    }
    // SAFETY: per the contract.
    let dependents: Rc<[Dependent]> = unsafe { find_dependents(class) }.into();
    // SAFETY: per the contract; classes are objects.
    let class_ref = unsafe { WeakObject::new(class.cast_mut().cast()) };
    CLASSES.with_borrow_mut(|classes| {
        classes.insert(
            class as usize,
            Class {
                class: class_ref,
                dependents: dependents.clone(),
            },
        )
    });
    dependents
}

/// # Safety
/// `class` must be a live class.
unsafe fn find_dependents(class: *const UStruct) -> Vec<Dependent> {
    let mut found: Vec<Dependent> = Vec::new();
    for name in rules::RULES
        .iter()
        .filter(|rule| !rule.depends_on.is_empty())
        .map(|rule| rule.property)
    {
        // SAFETY: per the contract; property records live as long as their class.
        unsafe {
            // Only the first rule matching a property applies, so that's the one to follow.
            let Some(rule) = limits::rule_for(class, name).filter(|r| !r.depends_on.is_empty())
            else {
                continue;
            };
            let property = reflection::find_property(class, rule.property);
            if property.is_null() || found.iter().any(|d| d.property == property) {
                continue;
            }
            let class_name = reflection::struct_name(class);
            let Some(components) = float_count(property) else {
                log!(
                    "{class_name}.{}: only floats and vectors can depend on other properties",
                    rule.property
                );
                continue;
            };
            let mut depends_on = Vec::new();
            for &name in rule.depends_on {
                let other = reflection::find_property(class, name);
                let size = if other.is_null() {
                    0
                } else {
                    (*other).element_size
                };
                if (1..=MAX_DEPENDENCY_SIZE as i32).contains(&size) {
                    depends_on.push((name, other));
                } else {
                    log!(
                        "{class_name}.{}: can't depend on {name} (no such property of at most {MAX_DEPENDENCY_SIZE} bytes)",
                        rule.property
                    );
                }
            }
            let names: Vec<&str> = depends_on.iter().map(|&(name, _)| name).collect();
            log!(
                "{class_name}.{}: limits follow {}",
                rule.property,
                names.join(", ")
            );
            found.push(Dependent {
                property,
                components,
                depends_on,
            });
        }
    }
    found
}

/// How many floats `property` holds, if it's a float or a vector.
///
/// # Safety
/// `property` must be a live property record.
unsafe fn float_count(property: *const FProperty) -> Option<usize> {
    // SAFETY: per the contract; a struct property's struct lives as long as the property.
    unsafe {
        if reflection::is_kind(property, "FloatProperty") {
            return Some(1);
        }
        let ustruct = reflection::struct_type(property);
        (!ustruct.is_null() && reflection::struct_name(ustruct).equals_str("Vector")).then_some(3)
    }
}
