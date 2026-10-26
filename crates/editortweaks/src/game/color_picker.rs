//! The color property's sliders: `WBP_ColorProperty` (a `UColorPropertyWidget`, the vehicle editor's
//! brick color row) holds `HueSlider`, `SaturationSlider`, `ValueSlider` and `AlphaSlider`, plain
//! `UBrickSliderWidget`s. Found in BRMK 1.11.6's `WBP_ColorProperty.uasset` (`et.py blueprint`).
//!
//! Their ranges mean something (a hue past 360 or a saturation past 100% isn't a color), so
//! `property_limits` and `slider_drag` leave them at the game's limits. They're told from other
//! sliders by their outers: a slider's outer is its widget tree, whose outer is the user widget
//! that owns it.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UStruct;

use crate::game::reflection;

/// How many outers to go through to find a slider's user widget (it is the second).
const MAX_OUTERS: usize = 8;

pub struct ColorPickers {
    /// `ColorPropertyWidget`.
    class: *const UStruct,
}

// SAFETY: a class record, alive as long as the game; only used on the game thread.
unsafe impl Send for ColorPickers {}
unsafe impl Sync for ColorPickers {}

pub fn color_pickers() -> Result<ColorPickers, String> {
    let class = reflection::find_class("ColorPropertyWidget")
        .ok_or("class ColorPropertyWidget not found")?;
    Ok(ColorPickers {
        class: class.cast(),
    })
}

impl ColorPickers {
    /// Whether `slider` is one of a color property's.
    ///
    /// # Safety
    /// `slider` must be a live widget. Game thread only.
    pub unsafe fn is_color_slider(&self, slider: *const UObjectBase) -> bool {
        let mut current = slider;
        for _ in 0..MAX_OUTERS {
            if current.is_null() {
                return false;
            }
            // SAFETY: per the contract; an object's outers are live objects (or null).
            unsafe {
                if reflection::is_a(current, self.class) {
                    return true;
                }
                current = (*current).outer_private.cast();
            }
        }
        false
    }
}
