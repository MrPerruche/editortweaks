//! `UNumericPropertyWidget`: a numeric row of a property panel, with one slider per component in
//! its reflected `Sliders` array.
//!
//! Its Blueprint creates the sliders (`AddSliderWidget` is a Blueprint event) and binds each one's
//! value change to `OnSliderValueChanged(Value, Type, SliderIndex)`. That replaces component
//! `SliderIndex` (every component when `AreAxesLocked()`) of the first edited object's value and
//! writes the result to every edited object (see `docs/re/property-panel.md`).

use core::ffi::c_void;

use brickrust::ue::coreuobject::{GObjects, UObjectBase};
use brickrust::ue::tarray::TArray;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::framework::sig::FunctionSig;
use crate::game::reflection::{self, BoolFunction};

/// `UNumericPropertyWidget::UpdateValue(const FBrickPropertyChangedEvent*)` (virtual; RVA 0xF03680
/// in build 25831956, BRMK export 0x3FD440): fills the row's sliders from the first edited object
/// whenever the panel shows or refreshes the property (not during a slider drag). In order: reads
/// the value (a helper, game 0xEAE1B0, BRMK 0x3826C0, calling the property's getter), the value
/// type, then the property's `GetValueRange` (vtable +0xA0, see `game::numeric_property`) with
/// that object's container, then the Blueprint event `UpdateNumericProperty` and a helper (BRMK
/// 0x3883F0) setting each slider's range and value. Found in the game as the only other caller of
/// the value helper, which `OnSliderValueChanged` (through its exec thunk) calls first.
pub const UPDATE_VALUE: FunctionSig = FunctionSig {
    name: "UNumericPropertyWidget::UpdateValue",
    // Storing the value type, then calling the property's GetValueRange with the container.
    pattern: "88 45 ?? 4c 8d 45 ?? 48 8b 03 48 8d 55 ?? 48 8b cb ff 90 a0 00 00 00 39 bb ?? 00 00 00",
};

pub type UpdateValueFn = unsafe extern "C" fn(this: *mut c_void, event: *const c_void);

/// The component a slider edits.
#[derive(Clone, Copy, Debug)]
pub struct SliderComponent {
    pub index: usize,
    /// The row edits every component at once.
    pub axes_locked: bool,
}

pub struct PropertyWidgets {
    class: *const UClass,
    sliders: usize,
    are_axes_locked: BoolFunction,
}

// SAFETY: reflection records live as long as the game; only used on the game thread.
unsafe impl Send for PropertyWidgets {}
unsafe impl Sync for PropertyWidgets {}

pub fn property_widgets() -> Result<PropertyWidgets, String> {
    let class = reflection::find_class("NumericPropertyWidget")
        .ok_or("class NumericPropertyWidget not found")?;
    Ok(PropertyWidgets {
        class,
        sliders: reflection::field_offset(class, "Sliders", "ArrayProperty", 16)?,
        are_axes_locked: BoolFunction::find(class, "AreAxesLocked")?,
    })
}

impl PropertyWidgets {
    /// The component `slider` edits, if it belongs to a numeric property row. Looks through every
    /// object, so call it once per edit, not per frame. Game thread only.
    pub fn slider_component(&self, slider: *const c_void) -> Option<SliderComponent> {
        let row = self.row_of(slider)?;
        // SAFETY: `row_of` returns a live NumericPropertyWidget, which has `AreAxesLocked`.
        unsafe {
            Some(SliderComponent {
                index: self.sliders(row).iter().position(|&s| s == slider)?,
                axes_locked: self.are_axes_locked.call(row),
            })
        }
    }

    /// The numeric property row (`UNumericPropertyWidget`) `slider` belongs to, if any. Looks
    /// through every object, so call it once per edit, not per frame. Game thread only.
    pub fn row_of(&self, slider: *const c_void) -> Option<*mut UObjectBase> {
        // SAFETY: on the game thread, GObjects slots hold live objects or null.
        unsafe {
            GObjects().iter().find(|&object| {
                !object.is_null()
                    && self.is_property_widget(object)
                    && self.sliders(object).contains(&slider)
            })
        }
    }

    /// The sliders of `row`, by component.
    ///
    /// # Safety
    /// `row` must be a live `UNumericPropertyWidget`, and the slice is only valid until its sliders
    /// change (don't keep it across frames). Game thread only.
    pub unsafe fn sliders<'a>(&self, row: *const UObjectBase) -> &'a [*const c_void] {
        // SAFETY: per the contract; `sliders` is the offset of a `TArray<UBrickSliderWidget*>` in
        // every NumericPropertyWidget.
        unsafe {
            let sliders = &*row.byte_add(self.sliders).cast::<TArray<*const c_void>>();
            match usize::try_from(sliders.num) {
                Ok(len) if len > 0 && !sliders.data.is_null() => {
                    core::slice::from_raw_parts(sliders.data, len)
                }
                _ => &[],
            }
        }
    }

    /// # Safety
    /// `object` must be a live object.
    unsafe fn is_property_widget(&self, object: *const UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { reflection::with_parents(reflection::class_of(object.cast())) }
            .any(|class| class == self.class.cast::<UStruct>())
    }
}
