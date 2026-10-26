//! `UBrickSliderWidget`: the slider + number field used for numeric values in the UI, e.g. the
//! vehicle editor's property panel (one per component: `UNumericPropertyWidget::Sliders`).
//!
//! Found through BRMK 1.11.6's export
//! `?SetValueInternal@UBrickSliderWidget@@AEAA_NM_N0W4EValueChangedEventType@@@Z`, then matched
//! in the game by its code. In game 1.11.7 it's at RVA 0xF393E0 (build 25831956; 0xF39340 in
//! build 25633865):
//!
//! ```text
//! bool SetValueInternal(float NewValue, bool bClampAndSnap, bool bBroadcast, EValueChangedEventType Type)
//! {
//!     if (bClampAndSnap) {
//!         Range = UUIFunctionLibrary::GetDefaultValueRange(ValueType, {MinValue, MaxValue});
//!         if (snap mode)  // usually none; brick-unit distances snap to a grid from 0
//!             NewValue = snap(NewValue, step, origin);
//!         NewValue = Clamp(NewValue, Range.Min, Range.Max);
//!     }
//!     if (NewValue == Value && Type == Interactive) return false;
//!     Value = NewValue; UpdateSliderValue(); if (bBroadcast) OnValueChanged.Broadcast(...);
//! }
//! ```
//!
//! Callers (event type): typed text (0), mouse release re-committing the current value (0 and 1),
//! dragging and arrow keys (2; both already clamp to the range themselves before calling).
//!
//! Addresses below are for game build 25831956 (1.11.7) unless noted otherwise.

use core::ffi::c_void;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UFunction;

use crate::framework::sig::FunctionSig;
use crate::game::reflection::{self, BoolFunction, Parameters};

pub const SET_VALUE_INTERNAL: FunctionSig = FunctionSig {
    name: "UBrickSliderWidget::SetValueInternal",
    // Loading MinValue, MaxValue and ValueType for the GetDefaultValueRange call; field offsets
    // and the stack offset are wildcarded.
    pattern: "f3 0f 10 81 ?? ?? 00 00 4c 8d 40 ?? f3 0f 10 91 ?? ?? 00 00 0f b6 91 ?? ?? 00 00 48 8d 48 08",
};

/// Signature of `SetValueInternal`. The `bool`/enum arguments are declared register-sized: the
/// game leaves their upper bits undefined, so they must be passed through untouched and only
/// their low byte looked at.
pub type SetValueInternalFn = unsafe extern "C" fn(
    this: *mut c_void,
    value: f32,
    clamp_and_snap: usize,
    broadcast: usize,
    event_type: usize,
) -> u8;

/// Byte offsets of the slider's range: the reflected floats `MinValue` and `MaxValue`
/// (0x2BC/0x2C0 in game 1.11.7, 0x454/0x458 in BRMK 1.11.6).
#[derive(Clone, Copy, Debug)]
pub struct RangeFields {
    pub min: usize,
    pub max: usize,
}

pub fn range_fields() -> Result<RangeFields, String> {
    let class =
        reflection::find_class("BrickSliderWidget").ok_or("class BrickSliderWidget not found")?;
    let min = reflection::field_offset(class, "MinValue", "FloatProperty", 4)?;
    let max = reflection::field_offset(class, "MaxValue", "FloatProperty", 4)?;
    // They're declared next to each other; anything else means the reflection data was misread.
    if max != min + 4 {
        return Err(format!(
            "unexpected layout: MinValue at 0x{min:X}, MaxValue at 0x{max:X}"
        ));
    }
    Ok(RangeFields { min, max })
}

impl RangeFields {
    /// The slider's `MinValue` and `MaxValue`.
    ///
    /// # Safety
    /// `slider` must be a live `UBrickSliderWidget`.
    pub unsafe fn get(&self, slider: *const c_void) -> (f32, f32) {
        // SAFETY: per the contract; the offsets come from the class's reflection data.
        unsafe {
            (
                slider.byte_add(self.min).cast::<f32>().read(),
                slider.byte_add(self.max).cast::<f32>().read(),
            )
        }
    }
}

/// `ENumericValueType::TimeOfDay`: typed as `h:m` and shown by `NumberToText` even while editing.
pub const TIME_OF_DAY: u8 = 9;

/// The slider's value: the reflected `Value` (float, stored units) and `ValueType`
/// (`ENumericValueType`, one byte: picks the shown unit, see `game::ui_library`).
#[derive(Clone, Copy, Debug)]
pub struct ValueFields {
    value: usize,
    value_type: usize,
}

pub fn value_fields() -> Result<ValueFields, String> {
    let class =
        reflection::find_class("BrickSliderWidget").ok_or("class BrickSliderWidget not found")?;
    let value = reflection::field_offset(class, "Value", "FloatProperty", 4)?;
    // An enum field is an EnumProperty, or a ByteProperty for the older `TEnumAsByte`.
    let value_type = reflection::field_offset(class, "ValueType", "EnumProperty", 1)
        .or_else(|_| reflection::field_offset(class, "ValueType", "ByteProperty", 1))?;
    Ok(ValueFields { value, value_type })
}

impl ValueFields {
    /// # Safety
    /// `slider` must be a live `UBrickSliderWidget`.
    pub unsafe fn value(&self, slider: *const c_void) -> f32 {
        // SAFETY: per the contract; the offset comes from the class's reflection data.
        unsafe { slider.byte_add(self.value).cast::<f32>().read() }
    }

    /// # Safety
    /// `slider` must be a live `UBrickSliderWidget`.
    pub unsafe fn value_type(&self, slider: *const c_void) -> u8 {
        // SAFETY: as for `value`.
        unsafe { slider.byte_add(self.value_type).cast::<u8>().read() }
    }
}

/// The reflected `SetValueType(ENumericValueType)` (BRMK export 0x451E40): stores the type and, if
/// it changed, shows the value again (`UpdateSliderValue(false, false)`). A numeric row sets each
/// slider's type from its property whenever it updates (BRMK 0x3883F0: the range, the type, then
/// the value), so a type set afterwards lasts until the row's next update.
pub struct SetValueType {
    function: *const UFunction,
    /// Offset of its one parameter.
    value_type: usize,
}

// SAFETY: a reflection record living as long as the game; only called on the game thread.
unsafe impl Send for SetValueType {}
unsafe impl Sync for SetValueType {}

/// Finds `SetValueType`, its one parameter checked to be a one-byte enum. After engine init.
pub fn set_value_type() -> Result<SetValueType, String> {
    let class =
        reflection::find_class("BrickSliderWidget").ok_or("class BrickSliderWidget not found")?;
    let function = reflection::find_function(class, "SetValueType")?;
    // SAFETY: a live function record; its parameters in order.
    unsafe {
        let mut parameters = reflection::properties(function.cast());
        match (parameters.next(), parameters.next()) {
            (Some(value_type), None)
                // An enum is an EnumProperty, or a ByteProperty for the older `TEnumAsByte`.
                if (reflection::is_kind(value_type, "EnumProperty")
                    || reflection::is_kind(value_type, "ByteProperty"))
                    && (*value_type).element_size == 1
                    && Parameters::fit(function) =>
            {
                Ok(SetValueType {
                    function,
                    value_type: (*value_type).offset_internal as usize,
                })
            }
            _ => Err("SetValueType: unexpected parameters".into()),
        }
    }
}

impl SetValueType {
    /// Has `slider` show its value as `value_type` (an `ENumericValueType`).
    ///
    /// # Safety
    /// `slider` must be a live `UBrickSliderWidget`. Game thread only.
    pub unsafe fn call(&self, slider: *mut UObjectBase, value_type: u8) {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the parameter is a byte at its reflected offset (checked).
        unsafe {
            reflection::put(&mut parameters, self.value_type, value_type);
            reflection::call(slider, self.function, parameters.as_mut_ptr());
        }
    }
}

/// `UpdateSliderValue(bool bUseGivenFocus, bool bFocused)` (RVA 0xF45970; BRMK export) writes the
/// value into the slider's text box. While the user types in it (`bFocused`, or the text box's own
/// state), it shows `FText::AsNumber(Value * GetUnitConversionFactor(ValueType))` with default
/// options: 3 decimals and digit grouping. Otherwise, and always for `TIME_OF_DAY`, it shows
/// `UUIFunctionLibrary::NumberToText(Value, ValueType, MaxFractionalDigits, ...)`, put into
/// `CustomTextFormat` if set. Called by `SetValueInternal`, focus changes, typed text, range and
/// type changes.
pub const UPDATE_SLIDER_VALUE: FunctionSig = FunctionSig {
    name: "UBrickSliderWidget::UpdateSliderValue",
    // Reading the text box's typing flag, then skipping the plain number for `TIME_OF_DAY` (9).
    pattern: "48 8b 86 ?? ?? 00 00 0f b6 98 ?? ?? 00 00 c0 eb 04 80 e3 01 84 db 0f 84 ?? ?? 00 00 80 be ?? ?? 00 00 09",
};

pub type UpdateSliderValueFn =
    unsafe extern "C" fn(this: *mut c_void, use_given_focus: usize, focused: usize);

/// `EValueChangedEventType::Committed`: a final value (typed, or a drag's on release).
pub const COMMITTED: u8 = 0;
/// `EValueChangedEventType::Interactive`: a change while still dragging or typing (`Released` is 1).
pub const INTERACTIVE: u8 = 2;

/// The reflected `OnTextChanged(const FText&, EValueChangedEventType)`, which the slider's text box
/// calls (a dynamic delegate, so through `ProcessEvent`) with the typed text: `Interactive` on every
/// edit while typing, `Committed` on Enter or leaving the box. That's its only caller (BRMK: one
/// call, from `execOnTextChanged`; game: `OnTextChanged` 0xF30F70 is only called from the thunk
/// 0xF9CCF0), so hooking the thunk sees every typed value.
///
/// Committed: parses the text (`game::slider_text`), divides by the unit factor and calls
/// `SetValueInternal`. Interactive, while typing: removes every character that isn't a digit, a
/// separator of the user's number format, `:` or one of `1234567890()-+=\/.,*^%` (a lazily
/// built static `FString`, game 0x4979090), and puts the result back into the text box. So letters
/// can't be typed, not even the formula parser's own `sqrt`.
pub struct TextChanged {
    /// The exec thunk.
    pub thunk: *const u8,
    /// Offset of the `EValueChangedEventType` in the parameters.
    event_type: usize,
}

// SAFETY: points into the game's code; only used on the game thread.
unsafe impl Send for TextChanged {}
unsafe impl Sync for TextChanged {}

pub fn on_text_changed() -> Result<TextChanged, String> {
    let class =
        reflection::find_class("BrickSliderWidget").ok_or("class BrickSliderWidget not found")?;
    let function = reflection::find_function(class, "OnTextChanged")?;
    let thunk = reflection::native_thunk(function).map_err(|e| format!("OnTextChanged: {e}"))?;
    // SAFETY: a live function record; its parameters are the text and the event type.
    let event_type = unsafe {
        reflection::properties(function.cast()).find(|&parameter| {
            reflection::is_kind(parameter, "EnumProperty")
                || reflection::is_kind(parameter, "ByteProperty")
        })
    }
    .ok_or("OnTextChanged: event type parameter not found")?;
    Ok(TextChanged {
        thunk,
        // SAFETY: as above.
        event_type: unsafe { (*event_type).offset_internal as usize },
    })
}

impl TextChanged {
    /// The event type of a call to the thunk, if it came through `ProcessEvent` (as the text box's
    /// calls do).
    ///
    /// # Safety
    /// `stack` must be the frame the thunk was called with.
    pub unsafe fn event_type(&self, stack: *const c_void) -> Option<u8> {
        // SAFETY: per the contract; such a call's parameters are laid out as the function's.
        unsafe {
            let parameters = reflection::native_call_parameters(stack)?;
            Some(parameters.add(self.event_type).read())
        }
    }
}

/// The slider's text box: the reflected `TextBox` (a `UBrickTextBoxWidget`) and its reflected
/// `IsTyping()` (bit 4 of a flags byte: +0x260 in the game, +0x3F8 in BRMK; the flag
/// `UpdateSliderValue` and `OnTextChanged` test).
pub struct TextBoxes {
    field: usize,
    is_typing: BoolFunction,
}

pub fn text_boxes() -> Result<TextBoxes, String> {
    let slider =
        reflection::find_class("BrickSliderWidget").ok_or("class BrickSliderWidget not found")?;
    let text_box =
        reflection::find_class("BrickTextBoxWidget").ok_or("class BrickTextBoxWidget not found")?;
    Ok(TextBoxes {
        field: reflection::field_offset(slider, "TextBox", "ObjectProperty", 8)?,
        is_typing: BoolFunction::find(text_box, "IsTyping")?,
    })
}

impl TextBoxes {
    /// Whether the user is typing in `slider`'s text box.
    ///
    /// # Safety
    /// `slider` must be a live `UBrickSliderWidget`. Game thread only.
    pub unsafe fn is_typing(&self, slider: *const c_void) -> bool {
        // SAFETY: per the contract; the field holds null or the slider's UBrickTextBoxWidget.
        unsafe {
            let text_box = slider
                .byte_add(self.field)
                .cast::<*mut UObjectBase>()
                .read();
            !text_box.is_null() && self.is_typing.call(text_box)
        }
    }
}
