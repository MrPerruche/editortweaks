//! The game's UI color styles (`EBrickUIColorStyle`): every Brick Rigs widget picks its colors by
//! style, not by color. Entries (BRMK 1.11.6 enum params): Default 0, Selected 1, Foreground 2,
//! Highlight 3, Positive 4, Negative 5, Neutral 6, Custom 7. The game marks warnings in its menus
//! with `Highlight` (see `docs/re/property-panel.md`).
//!
//! Buttons (`UButtonWidgetBase`, which sliders derive from) keep theirs in the reflected
//! `ColorStyle` (+0x278 in game 1.11.7, +0x410 in BRMK) and restyle themselves through the
//! reflected `SetColorStyle(EBrickUIColorStyle)` (BRMK 0x44EEB0: stores it, then
//! `UpdateButtonStyleInternal`). A slider passes it on to its fill (`FillBorder`) and text box.

use core::ffi::c_void;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UFunction;

use crate::game::reflection::{self, Parameters};

/// Styles by name, looked up in the reflected enum.
#[derive(Clone, Copy, Debug)]
pub struct ColorStyleValues {
    pub highlight: u8,
    pub negative: u8,
}

pub fn color_style_values() -> Result<ColorStyleValues, String> {
    let value = |entry: &str| -> Result<u8, String> {
        let value = reflection::enum_value("EBrickUIColorStyle", entry)?;
        u8::try_from(value).map_err(|_| format!("{entry} is {value}"))
    };
    Ok(ColorStyleValues {
        highlight: value("EBrickUIColorStyle::Highlight")?,
        negative: value("EBrickUIColorStyle::Negative")?,
    })
}

/// A button's color style: the reflected `ColorStyle` field and `SetColorStyle` function.
pub struct ButtonColorStyle {
    field: usize,
    set: *const UFunction,
    /// Offset of `SetColorStyle`'s parameter.
    parameter: usize,
}

// SAFETY: reflection records live as long as the game; only used on the game thread.
unsafe impl Send for ButtonColorStyle {}
unsafe impl Sync for ButtonColorStyle {}

pub fn button_color_style() -> Result<ButtonColorStyle, String> {
    let class =
        reflection::find_class("ButtonWidgetBase").ok_or("class ButtonWidgetBase not found")?;
    // An enum field is an EnumProperty, or a ByteProperty for the older `TEnumAsByte`.
    let field = reflection::field_offset(class, "ColorStyle", "EnumProperty", 1)
        .or_else(|_| reflection::field_offset(class, "ColorStyle", "ByteProperty", 1))?;
    let set = reflection::find_function(class, "SetColorStyle")?;
    // SAFETY: a live function record; its only parameter is the new style.
    let parameter = unsafe {
        let mut parameters = reflection::properties(set.cast());
        match (parameters.next(), parameters.next()) {
            (Some(style), None)
                if (reflection::is_kind(style, "EnumProperty")
                    || reflection::is_kind(style, "ByteProperty"))
                    && (*style).element_size == 1
                    && Parameters::fit(set) =>
            {
                (*style).offset_internal as usize
            }
            _ => return Err("SetColorStyle: unexpected parameters".into()),
        }
    };
    Ok(ButtonColorStyle {
        field,
        set,
        parameter,
    })
}

impl ButtonColorStyle {
    /// The button's current style.
    ///
    /// # Safety
    /// `button` must be a live `UButtonWidgetBase`.
    pub unsafe fn get(&self, button: *const c_void) -> u8 {
        // SAFETY: per the contract; the offset comes from the class's reflection data.
        unsafe { button.byte_add(self.field).cast::<u8>().read() }
    }

    /// Restyles the button (nothing happens if it already has `style`).
    ///
    /// # Safety
    /// `button` must be a live `UButtonWidgetBase`. Game thread only.
    pub unsafe fn set(&self, button: *mut c_void, style: u8) {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; `SetColorStyle` is a function of the button's class, and its
        // one-byte parameter lies within the checked parameter size.
        unsafe {
            parameters.as_mut_ptr().add(self.parameter).write(style);
            reflection::call(
                button.cast::<UObjectBase>(),
                self.set,
                parameters.as_mut_ptr(),
            );
        }
    }
}
