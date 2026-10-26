//! Color rows of ours: the game's color picker (`WBP_ColorProperty`, a `UColorPropertyWidget`: hue
//! wheel, palette, hex code and sliders), as a brick's color shows.
//!
//! Unlike the other brick properties, `FColorBrickProperty` has no virtual getter or setter (its
//! table holds only the base's 18 slots): the widget's value helper and the color write helper read
//! and write the `FColor` through the property's `FProperty` themselves. So besides a copy of its
//! table, both helpers are hooked: for a row of ours (told by its table) they read and write our
//! [`Color`], for every other row they go on to the game's code.
//!
//! Found in game 1.11.7 (build 25831956); see "Color rows of our own" in
//! `docs/re/property-panel.md`.

use core::ffi::c_void;
use std::sync::OnceLock;

use super::{FIELDS, Property, RawArray, Value};
use crate::framework::hook;
use crate::framework::sig::{CodeShape, FunctionSig, VtableSig, direct_calls};

/// A color shown as the game's color picker. Its functions run on the game thread, whenever a
/// panel lists the row or changes it: while a slider or the wheel is dragged, every frame.
pub trait Color: Sync {
    /// The color shown: red, green, blue and alpha, as the picker shows them (sRGB, 0 to 255).
    fn rgba(&self) -> [u8; 4];
    /// Sets the color picked.
    fn set_rgba(&self, rgba: [u8; 4]);
    /// Whether the picker shows alpha too (asked once, when the row is made); without, it's 255.
    fn has_alpha(&self) -> bool {
        false
    }
}

/// Where a color property keeps whether it has alpha, an attribute like a number's: the stored
/// value, a flag saying it's set, then a delegate (unbound when zero, as ours). Offsets from the
/// start of [`Property::fields`].
pub(super) const ALPHA: usize = 0x18 - FIELDS;
pub(super) const ALPHA_SET: usize = 0x19 - FIELDS;

const COLOR_PROPERTY: VtableSig = VtableSig {
    name: "FColorBrickProperty vtable",
    type_name: "FColorBrickProperty",
    length: 18,
};

/// The color table's [`super::slot::COMPARE_VALUES`] (game 0xCD29D0): two `FColor`s compared as
/// one `uint32`.
const COLOR_COMPARE: CodeShape = CodeShape {
    name: "FColorBrickProperty compare",
    shape: "41 8b 00 39 02 0f 94 c0 c3",
};

/// The color table's own slots that reach the value through the `FProperty` (null for ours).
mod slot {
    /// `(this, FString* out, container) -> out`: the value as text (game 0xCDA280).
    pub const EXPORT_TEXT: usize = 14;
    /// `(this, container, const TCHAR* text) -> bool`: the value from text such as
    /// `(R=..,G=..,B=..,A=..)` (game 0xCE9E20).
    pub const IMPORT_TEXT: usize = 16;
}

/// The color widget's value helper (game 0xEAE060), `bool (const FBrickPropertyEditInfo*, FColor*
/// out)`, called by `UColorPropertyWidget::UpdateValue` (0xF02EA0) with the widget's edit info
/// (+0x268). It casts the edit info's property to `FColorBrickProperty` (0xEAD840, the call at
/// [`CAST_CALL`]), then reads the color inline (the cast's result, its `FProperty` at +8, the
/// `FProperty`'s offset at +0x4C), alpha 255 unless the property has alpha.
const GET_VALUE: FunctionSig = FunctionSig {
    name: "UColorPropertyWidget value helper",
    pattern: "48 8b cf 48 89 5c 24 50 e8 ?? ?? ?? ?? 48 8b d8 48 85 c0 0f 84 ?? ?? ?? ?? 48 8b 54 24 20 48 8b cf e8 ?? ?? ?? ?? 84 c0 0f 84 ?? ?? ?? ?? 48 8b 44 24 30 4c 8b 43 08",
};

/// Where [`GET_VALUE`]'s pattern calls the cast to `FColorBrickProperty`.
const CAST_CALL: usize = 8;

/// The color write helper (game 0xEB43D0), `void (const FBrickPropertyEditInfo*,
/// ABasePlayerController*, EValueChangedEventType, const FColor*)`, called by
/// `UColorPropertyWidget::SetColorPropertyValue`: one of the typed write helpers (see "Before and
/// after a brick property is written"), with the setter inlined. Matched on that setter's alpha (255
/// unless the property has alpha), and checked to call the same cast as [`GET_VALUE`].
const WRITE_VALUE: FunctionSig = FunctionSig {
    name: "color property write helper",
    pattern: "41 0f 44 cd 84 c9 b9 ff 00 00 00 0f 44 c1 88 44 24",
};

type GetValueFn = unsafe extern "C" fn(info: *const EditInfo, out: *mut FColor) -> bool;
type WriteValueFn = unsafe extern "C" fn(
    info: *const EditInfo,
    controller: *mut c_void,
    event: u8,
    value: *const FColor,
);

/// `FBrickPropertyEditInfo`'s start: its property (the object of a `TSharedRef`).
#[repr(C)]
struct EditInfo {
    property: *const Property,
}

/// `FColor` as it lies in memory on Windows (little endian).
#[repr(C)]
#[derive(Clone, Copy)]
struct FColor {
    b: u8,
    g: u8,
    r: u8,
    a: u8,
}

/// Our color table, as an address: rows of ours are told by it.
static TABLE: OnceLock<usize> = OnceLock::new();
static ORIGINAL_GET: OnceLock<GetValueFn> = OnceLock::new();
static ORIGINAL_WRITE: OnceLock<WriteValueFn> = OnceLock::new();

/// Copies the game's color table with `shared` and ours in place of the game's functions, and hooks
/// the helpers reading and writing colors. Works before engine init.
pub(super) fn table(shared: &[(usize, *const c_void)]) -> Result<&'static [*const c_void], String> {
    let get = GET_VALUE.locate()?;
    // SAFETY: the call lies within the matched pattern.
    let cast = unsafe { get.call_target(CAST_CALL) }
        .ok_or_else(|| format!("{}: no call to the cast", GET_VALUE.name))?;
    let write = WRITE_VALUE.resolve()?;
    if !direct_calls(write).contains(&cast) {
        return Err(format!(
            "{}: doesn't cast to a color property",
            WRITE_VALUE.name
        ));
    }
    let table = super::table(
        &COLOR_PROPERTY,
        None,
        (super::slot::COMPARE_VALUES, &COLOR_COMPARE),
        &[
            (super::slot::COMPARE_VALUES, compare_colors as *const c_void),
            (slot::EXPORT_TEXT, export_text as *const c_void),
            (slot::IMPORT_TEXT, import_text as *const c_void),
        ],
        shared,
    )?;
    let _ = TABLE.set(table.as_ptr() as usize);
    // SAFETY: both are function starts with these signatures (see their declarations).
    unsafe {
        hook::install_typed(
            GET_VALUE.name,
            get.function,
            get_value as GetValueFn,
            &ORIGINAL_GET,
        )?;
        hook::install_typed(
            WRITE_VALUE.name,
            write,
            write_value as WriteValueFn,
            &ORIGINAL_WRITE,
        )?;
    }
    Ok(table)
}

/// The property of the edit info `info` if it's a color row of ours, with its color.
///
/// # Safety
/// `info` must be null or a live edit info. Game thread only.
unsafe fn ours(info: *const EditInfo) -> Option<(&'static Property, &'static dyn Color)> {
    let &table = TABLE.get()?;
    // SAFETY: per the contract; a brick property starts with its table. Ours are never freed.
    unsafe {
        let property = info.as_ref()?.property.as_ref()?;
        if property.vtable as usize != table {
            return None;
        }
        match property.value {
            Value::Color(color) => Some((property, color)),
            _ => None,
        }
    }
}

unsafe extern "C" fn get_value(info: *const EditInfo, out: *mut FColor) -> bool {
    // SAFETY: the game passes its edit info and room for a color.
    unsafe {
        let Some((_, color)) = ours(info) else {
            return ORIGINAL_GET
                .get()
                .is_some_and(|original| original(info, out));
        };
        let [r, g, b, a] = color.rgba();
        let a = if color.has_alpha() { a } else { u8::MAX };
        out.write(FColor { b, g, r, a });
    }
    true
}

unsafe extern "C" fn write_value(
    info: *const EditInfo,
    controller: *mut c_void,
    event: u8,
    value: *const FColor,
) {
    // SAFETY: the game passes its edit info and the color picked.
    unsafe {
        let Some((property, color)) = ours(info) else {
            if let Some(original) = ORIGINAL_WRITE.get() {
                original(info, controller, event, value);
            }
            return;
        };
        let Some(&FColor { b, g, r, a }) = value.as_ref() else {
            return;
        };
        color.set_rgba([r, g, b, a]);
        (property.changed)();
    }
}

unsafe extern "C" fn compare_colors(
    _this: *const Property,
    a: *const FColor,
    b: *const FColor,
) -> bool {
    // SAFETY: the game passes two colors.
    let (a, b) = unsafe { (*a, *b) };
    [a.r, a.g, a.b, a.a] == [b.r, b.g, b.b, b.a]
}

unsafe extern "C" fn export_text(
    _this: *const Property,
    out: *mut RawArray,
    _container: *const c_void,
) -> *mut RawArray {
    // SAFETY: the caller passes room for an `FString`; an empty one owns no memory.
    unsafe {
        out.write(RawArray {
            data: core::ptr::null_mut(),
            num: 0,
            max: 0,
        });
    }
    out
}

unsafe extern "C" fn import_text(
    _this: *const Property,
    _container: *const c_void,
    _text: *const u16,
) -> bool {
    false
}
