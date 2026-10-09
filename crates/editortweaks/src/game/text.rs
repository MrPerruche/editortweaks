//! Unreal's number formatting: `FText::AsNumber` for floats.
//!
//! `FText::AsNumber(float, const FNumberFormattingOptions*, const FCulturePtr&)` (game build
//! 25831956: RVA 0x108E9F0) only forwards to `FText::AsNumberTemplate<float, float>` (0x107B610),
//! which is hooked instead: one-call wrappers all look alike. Found through the call in
//! `UBrickSliderWidget::UpdateSliderValue`. The template converts the value to double and formats
//! it with `FastDecimalFormat::NumberToString` (0x1062130), which prints at most 18 decimals.

use core::ffi::c_void;

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
