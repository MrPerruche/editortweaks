//! How a slider turns typed text into its value, and how to write a value so it reads back exactly.
//!
//! On commit, `OnTextChanged` (see `game::slider`) parses the text with Unreal's
//! `TDefaultNumericTypeInterface<float>::FromString`: a number in the user's culture, else
//! culture-invariant, else a formula (`FBasicMathExpressionEvaluator`: `+ - * / % ^`, `sqrt`,
//! parentheses; the result clamped to +-FLT_MAX), converted from double to float. Digit grouping
//! isn't accepted. It then divides by the unit factor, in single precision:
//! `stored = typed / factor` (`game::ui_library`).
//!
//! `FromString`'s last argument is the slider's stored value, used by the operators `+=`, `-=`, `*=`
//! and `/=`. That value isn't converted to the shown unit: `+=10` on a slider showing 50 % gives
//! 10.5 % (0.5 + 10, then / 100).
//!
//! Addresses are for game build 25831956 (1.11.7).

use core::ffi::c_void;

use crate::framework::sig::FunctionSig;

/// `TDefaultNumericTypeInterface<float>::FromString(const FString&, const float& ExistingValue)`
/// (RVA 0xF0A250; BRMK 0x40A7A0): a virtual function, called on a static object. `OnTextChanged`
/// is its only caller (once, or three times for `h:m`).
pub const FROM_STRING: FunctionSig = FunctionSig {
    name: "TDefaultNumericTypeInterface<float>::FromString",
    // Converting the existing value to double for the formula evaluator, then calling it.
    pattern: "f3 41 0f 10 1e 0f 5a db 44 39 7f 08 74 03 48 8b 37 4c 8b c6 48 8d 55 ?? 48 8d 0d ?? ?? ?? ?? e8",
};

pub type FromStringFn = unsafe extern "C" fn(
    interface: *const c_void,
    out: *mut OptionalF32,
    text: *const FString,
    existing: *const f32,
) -> *mut OptionalF32;

/// `TOptional<float>`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct OptionalF32 {
    pub value: f32,
    pub is_set: bool,
}

/// `FString`: UTF-16 characters and a terminating zero, which `num` counts.
#[repr(C)]
pub struct FString {
    data: *const u16,
    num: i32,
    max: i32,
}

impl FString {
    /// # Safety
    /// `self` must be a live string, as the game passes it.
    pub unsafe fn to_string(&self) -> String {
        let len = usize::try_from(self.num).unwrap_or(0).saturating_sub(1);
        if self.data.is_null() || len == 0 {
            return String::new();
        }
        // SAFETY: per the contract, `data` holds `num` characters.
        String::from_utf16_lossy(unsafe { core::slice::from_raw_parts(self.data, len) })
    }
}

/// A string of ours in `FString` layout, for game functions that only read their string argument
/// (they don't resize or free it).
pub struct OwnedFString {
    /// The characters `header` points to; moving the `Vec` doesn't move them.
    _chars: Vec<u16>,
    header: FString,
}

impl OwnedFString {
    pub fn new(text: &str) -> Self {
        let chars: Vec<u16> = text.encode_utf16().chain([0]).collect();
        let num = i32::try_from(chars.len()).expect("typed text is short");
        let header = FString {
            data: chars.as_ptr(),
            num,
            max: num,
        };
        Self {
            _chars: chars,
            header,
        }
    }

    pub fn as_ptr(&self) -> *const FString {
        &self.header
    }
}

/// Unreal prints at most 18 decimals (`FastDecimalFormat`); stay below that.
const MAX_DIGITS: usize = 17;

/// The stored value a slider gets from the plain number `text`, like `OnTextChanged` computes it.
pub fn typed_value(text: &str, factor: f32) -> Option<f32> {
    let typed = text.parse::<f64>().ok()? as f32;
    Some(typed / factor)
}

/// The shortest decimal text, in the shown unit, that a slider reads back as a given stored value:
/// `shown` printed with `digits` decimals.
///
/// Without a unit conversion (factor 1) it reads back exactly. With one, it may also read back as a
/// float right next to the stored value: the conversion is off by that much itself. 30 cm in brick
/// units (factor 1/30, not exact as a float) shows as "1" rather than "1.0000001", though typing
/// "1" gives 29.999998 cm (then snapped to 30 by the brick grid).
#[derive(Clone, Copy, Debug)]
pub struct ExactText {
    /// The number to print: the shown value (stored * factor, a float like in the game), or one of
    /// its neighbours when the shown value itself doesn't divide back.
    pub shown: f32,
    pub digits: usize,
}

impl ExactText {
    /// `None` if there's no such text with at most 17 decimals (e.g. tiny values).
    pub fn find(stored: f32, factor: f32) -> Option<Self> {
        if !factor.is_finite() || factor == 0.0 {
            return None;
        }
        let shown = stored * factor;
        if !shown.is_finite() {
            return None;
        }
        let reads_back = |typed: f32| {
            typed == stored
                || (factor != 1.0 && (typed == stored.next_up() || typed == stored.next_down()))
        };
        let candidates = [shown, shown.next_down(), shown.next_up()];
        (0..=MAX_DIGITS).find_map(|digits| {
            candidates
                .into_iter()
                .map(|shown| Self { shown, digits })
                .find(|text| typed_value(&text.text(), factor).is_some_and(reads_back))
        })
    }

    /// The text, as Unreal prints it with this many decimals (and no grouping).
    pub fn text(&self) -> String {
        format!("{:.*}", self.digits, f64::from(self.shown))
    }
}

/// `stored` written for use inside a formula, in the shown unit, so that a formula of just this
/// number gives back `stored` (see [`ExactText`]). Very large or tiny numbers are written as
/// `m*(10^e)` or `m/(10^e)`: Unreal's formula parser reads a number's digits into an integer.
pub fn formula_number(stored: f32, factor: f32) -> String {
    match ExactText::find(stored, factor) {
        Some(exact) if exact.shown.abs() < 1e15 => exact.text(),
        exact => {
            let shown = exact.map_or(stored * factor, |exact| exact.shown);
            // The shortest digits that read back as `shown`, e.g. `2.6584558e36`.
            let scientific = format!("{shown:e}");
            match scientific.split_once('e') {
                Some((mantissa, "0")) => mantissa.to_owned(),
                Some((mantissa, exponent)) => match exponent.strip_prefix('-') {
                    Some(exponent) => format!("({mantissa}/(10^{exponent}))"),
                    None => format!("({mantissa}*(10^{exponent}))"),
                },
                None => scientific,
            }
        }
    }
}
