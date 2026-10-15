//! Unreal's texts (`FText`): number formatting (`FText::AsNumber` for floats) and handing out texts
//! of ours ([`Text`]).
//!
//! `FText::AsNumber(float, const FNumberFormattingOptions*, const FCulturePtr&)` (game build
//! 25831956: RVA 0x108E9F0) only forwards to `FText::AsNumberTemplate<float, float>` (0x107B610),
//! which is hooked instead: one-call wrappers all look alike. Found through the call in
//! `UBrickSliderWidget::UpdateSliderValue`. The template converts the value to double and formats
//! it with `FastDecimalFormat::NumberToString` (0x1062130), which prints at most 18 decimals.

use core::ffi::c_void;
use core::sync::atomic::{AtomicI32, Ordering};

use crate::framework::sig::FunctionSig;

/// `FNumberFormattingOptions` (0x18 bytes). The default constructor gives: no sign for positive
/// numbers, grouping, rounding half to even, 1 to 324 integral digits and 0 to 3 decimals.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct NumberFormattingOptions {
    pub always_sign: bool,
    pub use_grouping: bool,
    /// `ERoundingMode`.
    pub rounding_mode: i32,
    pub minimum_integral_digits: i32,
    pub maximum_integral_digits: i32,
    pub minimum_fractional_digits: i32,
    pub maximum_fractional_digits: i32,
}

const _: () = assert!(size_of::<NumberFormattingOptions>() == 0x18);

pub const AS_NUMBER_FLOAT: FunctionSig = FunctionSig {
    name: "FText::AsNumberTemplate<float>",
    // Converting the float to double right before formatting it (`AsPercent<float>` multiplies by
    // 100 first), then reading the culture pointer.
    pattern: "0f 57 c0 f3 0f 5a c6 f2 0f 11 4c 24 ?? e8 ?? ?? ?? ?? 49 8b 44 24 08 49 8b 0c 24",
};

/// Returns `out`, the new `FText`. `options` may be null (the culture's defaults); `culture` points
/// to a possibly empty `TSharedPtr<FCulture>` (empty: the current culture).
pub type AsNumberFn = unsafe extern "C" fn(
    out: *mut c_void,
    value: f32,
    options: *const NumberFormattingOptions,
    culture: *const c_void,
) -> *mut c_void;

/// An `FText` (UE 4.27, 0x18 bytes): a thread-safe shared reference to its text data, and flags.
/// A value of this type holds one reference, which it only gives back through [`Text::release`]:
/// keep it for the whole game (e.g. in a static) and hand out copies with [`Text::assign_to`], or
/// release it once used.
#[repr(C)]
pub struct Text {
    data: *mut c_void,
    controller: *mut ReferenceController,
    flags: u32,
}

const _: () = assert!(size_of::<Text>() == 0x18);

/// `FReferenceControllerBase`: the reference counts of shared pointers. The shared references hold
/// one weak reference together.
#[repr(C)]
struct ReferenceController {
    vtable: *const ReferenceControllerVTable,
    shared: i32,
    weak: i32,
}

#[repr(C)]
struct ReferenceControllerVTable {
    destroy_object: unsafe extern "C" fn(controller: *mut ReferenceController),
    /// The deleting destructor; flag 1 frees the memory.
    delete: unsafe extern "C" fn(controller: *mut ReferenceController, flags: u32),
}

impl Text {
    /// Makes the live `FText` at `target` another reference to this text, releasing its old one
    /// (how the game copies texts, e.g. game 0xDBF0C8).
    ///
    /// # Safety
    /// `target` must point to a live `FText`. Game thread only.
    pub unsafe fn assign_to(&self, target: *mut Text) {
        // SAFETY: per the contract; this text's own reference keeps its controller alive.
        unsafe {
            if !self.controller.is_null() {
                counter(&raw mut (*self.controller).shared).fetch_add(1, Ordering::AcqRel);
            }
            let old = target.replace(Text {
                data: self.data,
                controller: self.controller,
                flags: self.flags,
            });
            old.release();
        }
    }

    /// Gives up this reference, like `~FText`: the text is destroyed with its last reference.
    ///
    /// # Safety
    /// `self` must hold a live reference. Game thread only.
    pub unsafe fn release(self) {
        let controller = self.controller;
        if controller.is_null() {
            return;
        }
        // SAFETY: per the contract; the controller lives until both counts reach zero.
        unsafe {
            if counter(&raw mut (*controller).shared).fetch_sub(1, Ordering::AcqRel) == 1 {
                ((*(*controller).vtable).destroy_object)(controller);
                if counter(&raw mut (*controller).weak).fetch_sub(1, Ordering::AcqRel) == 1 {
                    ((*(*controller).vtable).delete)(controller, 1);
                }
            }
        }
    }
}

/// One of a reference controller's counts, which the engine changes atomically.
///
/// # Safety
/// `count` must point to a live, aligned count.
unsafe fn counter<'a>(count: *mut i32) -> &'a AtomicI32 {
    // SAFETY: per the contract; the engine only accesses the counts atomically.
    unsafe { AtomicI32::from_ptr(count) }
}
