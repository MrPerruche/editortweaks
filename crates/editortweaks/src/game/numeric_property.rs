//! Numeric brick properties (`FNumericBrickPropertyBase` and its typed variants): the C++ objects
//! behind numeric rows of property panels, shared by the vehicle editor and the settings menus.
//! They aren't UObjects and BRMK doesn't export them; found in game 1.11.7 (build 25633865) by
//! following `UNumericPropertyWidget::UpdateValue` (BRMK export) to the property's virtual calls.
//!
//! Vtable (21 entries; three variants seen, e.g. at RVA 0x38DC498): [18] typed getter, [19] typed
//! setter (stores the clamped value into the edited object's field), [20] `GetValueRange`.
//! Fields: the edited reflected property (`FProperty*`) at +0x8, see [`reflected_field`].
//! Attributes (each a stored `TOptional` plus an optional delegate taking the edited objects):
//! value range at +0x30 (delegate +0x58), axis lock at +0x68 (+0x70), "don't clamp" bool at +0x98
//! (+0xA0). Function names below are ours; the real ones aren't known.

use core::ffi::c_void;

use brickrust::ue::fname::FName;
use brickrust::ue::fproperty::FProperty;

use crate::framework::sig::FunctionSig;
use crate::game::reflection;

/// Offset of the edited `FProperty*` in a numeric property. The typed setters (e.g. RVA 0xD081B0)
/// store the value at `Container::target() + FProperty::Offset_Internal`, reading it from here.
const REFLECTED_FIELD: usize = 0x8;

/// The reflected property a numeric property edits.
///
/// # Safety
/// `property` must be a live numeric property (e.g. `ClampValue`'s `this`).
pub unsafe fn reflected_field(property: *const c_void) -> *const FProperty {
    // SAFETY: per the contract and the layout above.
    unsafe {
        *property
            .byte_add(REFLECTED_FIELD)
            .cast::<*const FProperty>()
    }
}

/// Offset of a numeric property's name (`FName`): the reflected property's for a row editing one,
/// the row's own type for one that doesn't. Game 0xD68446 stores it after building the thrust force
/// row, and BRMK's base constructor (0x1171A0) starts it out as `None`.
const NAME: usize = 0x10;

/// Whether `property` is the row of type `name`, one editing no reflected property
/// (e.g. `FThrusterForceBrickProperty`, which has its own getter, setter and range).
///
/// # Safety
/// `property` must be a live numeric property (e.g. `ClampValue`'s `this`).
pub unsafe fn is_row_type(property: *const c_void, name: &str) -> bool {
    // SAFETY: per the contract and the layout above.
    unsafe {
        reflected_field(property).is_null()
            && reflection::name_is(&*property.byte_add(NAME).cast::<FName>(), name)
    }
}

/// What a numeric property edits (`ClampValue`'s and the setters' `container` argument): a root
/// object and a path of nested objects or structs; the value lives in the last one.
#[repr(C)]
pub struct Container {
    root: *mut c_void,
    // TArray<void*>
    path: *const *mut c_void,
    path_len: i32,
    path_capacity: i32,
}

impl Container {
    /// The edited object (e.g. a brick), also when the value is in a struct nested in it. The
    /// same container is passed to other properties' code (`FBrickPropertyContainer`).
    pub fn root(&self) -> *mut c_void {
        self.root
    }

    /// The object or struct holding the value: the last path entry, or the root without a path.
    ///
    /// # Safety
    /// `self` must be a live container, as the game passes it.
    pub unsafe fn target(&self) -> *mut c_void {
        match usize::try_from(self.path_len) {
            // SAFETY: per the contract, `path` holds `path_len` entries.
            Ok(len) if len > 0 => unsafe { *self.path.add(len - 1) },
            _ => self.root,
        }
    }
}

/// Vtable slot of the typed getter (vtable +0x90, e.g. RVA 0xCE8BE0 for floats in build 25831956):
/// `bool Get(const Container*, NumericValue* Out)`, reading the field the setters write. A
/// rotator's components come out as roll, pitch, yaw. BRMK's panel reads values with it (0x3826C0).
const GETTER_SLOT: usize = 0x90 / 8;

type GetterFn = unsafe extern "C" fn(
    property: *const c_void,
    container: *const c_void,
    out: *mut NumericValue,
) -> u8;

/// The value `property` holds for `container`'s target, as the panel reads it.
///
/// # Safety
/// `property` must be a live numeric property and `container` a live container for it (e.g. the
/// arguments of a `ClampValue` call).
pub unsafe fn current_value(
    property: *const c_void,
    container: *const c_void,
) -> Option<NumericValue> {
    let mut value = NumericValue {
        data: [0.0; 3],
        num_used: 0,
    };
    // SAFETY: per the contract; every numeric property's vtable has the getter at this slot (see
    // the module docs), and it only writes `value`.
    let found = unsafe {
        let vtable = *property.cast::<*const usize>();
        let getter: GetterFn = core::mem::transmute(*vtable.add(GETTER_SLOT));
        getter(property, container, &mut value)
    };
    (found != 0).then_some(value)
}

/// `FNumericBrickPropertyValue`: up to three components (`num_used` of them are meaningful).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NumericValue {
    pub data: [f32; 3],
    pub num_used: u8,
}

/// `FNumericBrickPropertyRange`: per-component limits (a component index past `num_used` uses
/// the last used one).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NumericRange {
    pub min: NumericValue,
    pub max: NumericValue,
}

/// Clamps a value before every typed setter stores it (RVA 0xD01BD0, 7 callers):
///
/// ```text
/// *Out = In;
/// if (DontClamp(Container)) return Out;
/// Range = this->GetValueRange(Container);          // virtual, vtable +0xA0
/// for (i = 0; i < Out->NumUsed; i++) {
///     Source = RemapAxisLockIndex(AxisLock(Container), i);   // locked axes copy their master
///     Out->Data[i] = Clamp(Out->Data[Source], Range.Min[Source], Range.Max[Source]);
/// }
/// return Out;
/// ```
pub const CLAMP_VALUE: FunctionSig = FunctionSig {
    name: "FNumericBrickPropertyBase::ClampValue",
    // The clamp loop: range component lookup, `comiss` against the minimum.
    pattern: "0f b6 4c 24 ?? 0f b6 54 24 ?? ff ca ff c9 3b c1 0f 4e c8 48 63 c9 f3 0f 10 44 8c ?? 48 63 c8 f3 0f 10 0c 8b 0f 2f c8",
};

pub type ClampValueFn = unsafe extern "C" fn(
    property: *const c_void,
    out: *mut NumericValue,
    container: *const c_void,
    input: *const NumericValue,
) -> *mut NumericValue;

/// The property's range (RVA 0xCE8FC0, vtable +0xA0): the range delegate's result if bound,
/// else the stored range if set, else 0..1. Also what the panel shows on the sliders.
pub const GET_VALUE_RANGE: FunctionSig = FunctionSig {
    name: "FNumericBrickPropertyBase::GetValueRange",
    // Building the default range {0}..{1}, then checking the range delegate on `this`.
    pattern: "48 c7 45 ?? 00 00 80 3f c7 45 ?? 00 00 00 00 c6 45 ?? 01 0f 10 4d ?? 0f 11 45 ?? 0f 11 4d ?? 74 ?? 48 8b 49 ?? 48 85 c9",
};

pub type GetValueRangeFn = unsafe extern "C" fn(
    property: *const c_void,
    out: *mut NumericRange,
    container: *const c_void,
) -> *mut NumericRange;

/// Vtable slot of `GetValueRange`, only used for diagnostics (spotting overrides).
pub const GET_VALUE_RANGE_SLOT: usize = 0xA0 / 8;
