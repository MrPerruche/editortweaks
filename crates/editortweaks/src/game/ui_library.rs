//! `UUIFunctionLibrary`: Brick Rigs' UI helpers for numbers and units.
//!
//! Addresses are for game build 25831956 (1.11.7); names are BRMK exports.

use core::ffi::c_void;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::{UFunction, UStruct};

use crate::framework::sig::FunctionSig;
use crate::game::kismet::{self, Texts};
use crate::game::reflection::{self, Function, Parameters};
use crate::game::text::Text;

/// `NumberToText(float Value, ENumericValueType, int32 MaxFractionalDigits, bool bShowUnit,
/// bool bAlwaysSign, bool bZeroAsUnlimited)` (RVA 0xEE5D90): the text of slider labels, tooltips
/// and more. Times of day become `h:m`, gears `R1`/`N`/...; otherwise it converts to the shown
/// unit, formats with `FText::AsNumber` (0 to `MaxFractionalDigits` decimals, or a per-type
/// default if that's negative; default rounding, half to even; with 0 decimals it rounds the value
/// itself first) and adds the unit. Non-finite values give "Invalid".
pub const NUMBER_TO_TEXT: FunctionSig = FunctionSig {
    name: "UUIFunctionLibrary::NumberToText",
    // Resolving the value type, then the `h:m` branch for TimeOfDay (9).
    pattern: "41 0f b6 c8 0f 28 f9 e8 ?? ?? ?? ?? 44 0f b6 f0 41 80 fe 09 0f 85 ?? ?? ?? ?? f3 0f 10 0d ?? ?? ?? ?? 48 8d 4d ?? f3 0f 59 3d",
};

/// Returns `out`, the new `FText`. The integer and `bool` arguments are passed through untouched
/// (their upper bits are undefined).
pub type NumberToTextFn = unsafe extern "C" fn(
    out: *mut c_void,
    value: f32,
    value_type: usize,
    max_fractional_digits: usize,
    show_unit: usize,
    always_sign: usize,
    zero_as_unlimited: usize,
) -> *mut c_void;

/// The reflected `ConvertToRealUnits(float Value, ENumericValueType)`: `Value *
/// GetUnitConversionFactor(Type)`, the factor from stored to shown values in the user's units
/// (percent 100, metric distances 0.01 for cm to m, brick units 1/30, ...; see
/// `docs/re/property-panel.md`). Called through reflection, so no signature is needed.
pub struct UnitConversion {
    object: *mut UObjectBase,
    function: *const UFunction,
    value: usize,
    value_type: usize,
    result: usize,
}

// SAFETY: the class default object and the function record live as long as the game; calls only
// happen on the game thread.
unsafe impl Send for UnitConversion {}
unsafe impl Sync for UnitConversion {}

/// The reflected [`NUMBER_TO_TEXT`], called through `ProcessEvent` (no signature needed): a number
/// as the game's UI writes it, in the player's units for the `*Auto` value types.
pub struct NumberText {
    library: *mut UObjectBase,
    function: Function,
    value: usize,
    value_type: usize,
    decimals: usize,
    units: usize,
    result: usize,
    texts: Texts,
}

// SAFETY: the class default object and the reflection records live as long as the game; calls
// only happen on the game thread.
unsafe impl Send for NumberText {}
unsafe impl Sync for NumberText {}

/// Finds [`NumberText`]. After engine init.
pub fn number_text() -> Result<NumberText, String> {
    let class =
        reflection::find_class("UIFunctionLibrary").ok_or("class UIFunctionLibrary not found")?;
    // SAFETY: a live class.
    let library = unsafe { reflection::default_object(class) }
        .ok_or("UIFunctionLibrary has no default object")?;
    let function = Function::find(class, "NumberToText")?;
    Ok(NumberText {
        library,
        value: function.parameter("Value", "FloatProperty", 4)?,
        value_type: function.parameter("ValueType", "EnumProperty", 1)?,
        decimals: function.parameter("MaxFractionalDigits", "IntProperty", 4)?,
        units: function.parameter("bIncludeUnits", "BoolProperty", 1)?,
        result: function.parameter("ReturnValue", "TextProperty", size_of::<Text>() as i32)?,
        function,
        texts: kismet::texts()?,
    })
}

/// The value of `ENumericValueType::<entry>` (e.g. `DistanceAuto`), by its reflected name.
pub fn value_type(entry: &str) -> Result<u8, String> {
    let value =
        reflection::enum_value("ENumericValueType", &format!("ENumericValueType::{entry}"))?;
    u8::try_from(value).map_err(|_| format!("ENumericValueType::{entry} is {value}"))
}

impl NumberText {
    /// `NumberToText(value, value_type, decimals, units)`: `value` (stored, e.g. cm) as shown, with
    /// at most `decimals` decimals, and its unit if `units`.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn text(&self, value: f32, value_type: u8, decimals: i32, units: bool) -> String {
        let mut parameters = Parameters::new();
        // SAFETY: on the game thread; each value goes to its parameter's reflected offset, the
        // others stay zero (false). The result is a new text, ours to release.
        unsafe {
            reflection::put(&mut parameters, self.value, value);
            reflection::put(&mut parameters, self.value_type, value_type);
            reflection::put(&mut parameters, self.decimals, decimals);
            reflection::put(&mut parameters, self.units, units);
            self.function.call(self.library, &mut parameters);
            self.texts
                .string(reflection::take::<Text>(&parameters, self.result))
        }
    }
}

/// Room for `ConvertToRealUnits`' parameters (12 bytes in game 1.11.7).
const PARAMS_SIZE: usize = 32;

pub fn unit_conversion() -> Result<UnitConversion, String> {
    let class =
        reflection::find_class("UIFunctionLibrary").ok_or("class UIFunctionLibrary not found")?;
    let function = reflection::find_function(class, "ConvertToRealUnits")?;
    // SAFETY: `class` and `function` are live reflection records; static functions are called on
    // the class default object, which exists for native classes.
    unsafe {
        let object = (*class).ClassDefaultObject;
        if object.is_null() {
            return Err("UIFunctionLibrary has no default object".into());
        }
        let parameters = function.cast::<UStruct>();
        if (*parameters).properties_size as usize > PARAMS_SIZE {
            return Err("ConvertToRealUnits: unexpected parameters".into());
        }

        // Parameters by type: (float, enum) -> float.
        let (mut value, mut value_type, mut result) = (None, None, None);
        for property in reflection::properties(parameters) {
            let offset = Some((*property).offset_internal as usize);
            if (*property).property_flags & reflection::RETURN_PARAMETER != 0 {
                result = offset;
            } else if reflection::is_kind(property, "FloatProperty") {
                value = offset;
            } else if reflection::is_kind(property, "EnumProperty")
                || reflection::is_kind(property, "ByteProperty")
            {
                value_type = offset;
            }
        }
        match (value, value_type, result) {
            (Some(value), Some(value_type), Some(result)) => Ok(UnitConversion {
                object,
                function,
                value,
                value_type,
                result,
            }),
            _ => Err("ConvertToRealUnits: unexpected parameters".into()),
        }
    }
}

impl UnitConversion {
    /// The factor between stored and shown values of `value_type`: shown = stored * factor.
    /// Game thread only.
    pub fn factor(&self, value_type: u8) -> f32 {
        let mut params = [0u64; PARAMS_SIZE / 8];
        // SAFETY: the offsets come from the function's reflection data and lie within its
        // parameters, which fit in `params` (checked in `unit_conversion`).
        unsafe {
            let base = params.as_mut_ptr().cast::<u8>();
            base.add(self.value).cast::<f32>().write_unaligned(1.0);
            base.add(self.value_type).write(value_type);
            reflection::call(self.object, self.function, base);
            base.add(self.result).cast::<f32>().read_unaligned()
        }
    }
}
