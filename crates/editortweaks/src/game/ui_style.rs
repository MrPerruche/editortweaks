//! The game's UI color styles (`EBrickUIColorStyle`): every Brick Rigs widget picks its colors by
//! style, not by color. Entries (BRMK 1.11.6 enum params): Default 0, Selected 1, Foreground 2,
//! Highlight 3, Positive 4, Negative 5, Neutral 6, Custom 7. The game marks warnings in its menus
//! with `Highlight` (see `docs/re/property-panel.md`).
//!
//! Buttons (`UButtonWidgetBase`, which sliders derive from) keep theirs in the reflected
//! `ColorStyle` (+0x278 in game 1.11.7, +0x410 in BRMK) and restyle themselves through the
//! reflected `SetColorStyle(EBrickUIColorStyle)` (BRMK 0x44EEB0: stores it, then
//! `UpdateButtonStyleInternal`). A slider passes it on to its fill (`FillBorder`) and text box.
//!
//! The colors themselves come from the UI style the player picked (Options > Interface): a
//! Blueprint class of `UBrickUIStyle` (`BP_UIStyle_Default`, `_Blue`, `_Orange`, ...) named by the
//! reflected `UBrickUserSettings::UIStyle` (BRMK +0x260, read by `GetUIStyle`, 0xB6A10), whose
//! default object holds them in the reflected `ColorStyles` (an `FLinearColor` per style, by
//! `EBrickUIColorStyle`). The default style's (BRMK content): Default (0.065, 0.068, 0.082),
//! Selected 0.3 gray, Foreground 0.847 gray, Highlight (0.791, 0.258, 0.03) orange, Positive green,
//! Negative red, Neutral 0.847 gray.

use core::ffi::c_void;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::{UClass, UFunction, UStruct};

use crate::game::reflection::{self, Getter, Parameters};

/// The default style's `Highlight` color, for when no style is found (linear, with alpha).
pub const DEFAULT_HIGHLIGHT: [f32; 4] = [0.791, 0.258, 0.03, 1.0];

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

/// The colors of the UI style the player picked (after engine init).
pub struct ThemeColors {
    /// `UBrickUserSettings`' default object, to call its static functions on.
    settings_object: *mut UObjectBase,
    /// `static UBrickUserSettings* UBrickUserSettings::GetUserSettings()`.
    settings: Getter<*mut UObjectBase>,
    /// Offset of `UBrickUserSettings::UIStyle` (a `UClass*`).
    style_class: usize,
    /// `BrickUIStyle`, which the picked class must derive from (as `GetUIStyle` checks).
    style: *const UStruct,
    /// Offset of `UBrickUIStyle::ColorStyles`, and how many it holds.
    colors: usize,
    count: usize,
}

// SAFETY: reflection records and the settings' default object live as long as the game; only used
// on the game thread.
unsafe impl Send for ThemeColors {}
unsafe impl Sync for ThemeColors {}

pub fn theme_colors() -> Result<ThemeColors, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let settings = class("BrickUserSettings")?;
    let style = class("BrickUIStyle")?;
    // SAFETY: a class found through reflection; its property records live as long as it.
    let (colors, count) = unsafe {
        let property = reflection::find_property(style.cast(), "ColorStyles");
        if property.is_null()
            || !reflection::is_kind(property, "StructProperty")
            || (*property).element_size != 16
        {
            return Err("BrickUIStyle: field 'ColorStyles' not found".into());
        }
        (
            (*property).offset_internal as usize,
            usize::try_from((*property).array_dim).unwrap_or(0),
        )
    };
    Ok(ThemeColors {
        settings_object: reflection::find_object("Default__BrickUserSettings", "BrickUserSettings")
            .ok_or("BrickUserSettings' default object not found")?,
        settings: Getter::find(settings, "GetUserSettings", "ObjectProperty")?,
        style_class: reflection::field_offset(settings, "UIStyle", "ClassProperty", 8)?,
        style: style.cast(),
        colors,
        count,
    })
}

impl ThemeColors {
    /// The color of `style` (an `EBrickUIColorStyle`) in the player's UI style (linear, with
    /// alpha), if the style is known.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn color(&self, style: u8) -> Option<[f32; 4]> {
        let index = usize::from(style);
        if index >= self.count {
            return None;
        }
        // SAFETY: on the game thread; the settings and the style class's default object are live
        // if not null, and the offsets come from reflection (the index checked against the array).
        unsafe {
            let settings = self.settings.call(self.settings_object);
            if settings.is_null() {
                return None;
            }
            let class = settings
                .cast::<u8>()
                .add(self.style_class)
                .cast::<*const UClass>()
                .read();
            if class.is_null()
                || !reflection::with_parents(class.cast()).any(|parent| parent == self.style)
            {
                return None;
            }
            let style = reflection::default_object(class)?;
            Some(
                style
                    .cast::<u8>()
                    .add(self.colors + index * 16)
                    .cast::<[f32; 4]>()
                    .read_unaligned(),
            )
        }
    }
}
