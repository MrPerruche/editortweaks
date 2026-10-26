//! `UNumericPropertyWidget`: a numeric row of a property panel, with one slider per component in
//! its reflected `Sliders` array.
//!
//! Its Blueprint creates the sliders (`AddSliderWidget` is a Blueprint event) and binds each one's
//! value change to `OnSliderValueChanged(Value, Type, SliderIndex)`. That replaces component
//! `SliderIndex` (every component when `AreAxesLocked()`) of the first edited object's value and
//! writes the result to every edited object (see `docs/re/property-panel.md`).
//!
//! The row sits in a `UPropertyContainerWidget` (its name, then the row, then a panel of small
//! buttons). Its Blueprint adds the lock button (2 or 3 sliders) and the units button (distances)
//! to that panel when the row updates ([`UPDATE_NUMERIC_PROPERTY`]), and the container then adds
//! its context menu button.

use core::ffi::c_void;

use brickrust::ue::coreuobject::{GObjects, UObjectBase};
use brickrust::ue::fname::FName;
use brickrust::ue::tarray::TArray;
use brickrust::ue::uclass::{UClass, UFunction, UStruct};

use crate::framework::sig::{self, CodeShape, FunctionSig};
use crate::game::reflection::{self, BoolFunction, Getter, Parameters};

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

/// `UNumericPropertyWidget::UpdateNumericProperty(ENumericValueType ValueType, int32 NumAxes)`
/// (game 0xFE5D60 in build 25831956, BRMK export 0x57B2F0): the native side of a Blueprint event,
/// `this->ProcessEvent(FindFunctionChecked(NAME_UpdateNumericProperty), {ValueType, NumAxes})`.
/// `UpdateValue` calls it once it read the value and its range, before setting the sliders; the
/// event (`WBP_NumericProperty`) then adds the lock button if `NumAxes > 1`, then the units button
/// for `DistanceAutoOrBrickUnits` (18), to the container's button panel, which the container
/// emptied before updating the row (`UPropertyContainerWidget::UpdateProperty`, BRMK 0x3FAF80:
/// the panel's used count set to 0, the row updated, then `PostAddPropertyButtons` adds the context
/// menu button). The shape, with the name's address (rip-relative, at +0x17) as its hole; it's the
/// shape of every such event stub with a byte and an int, so [`update_numeric_property`] also
/// checks the name.
pub const UPDATE_NUMERIC_PROPERTY: CodeShape = CodeShape {
    name: "UNumericPropertyWidget::UpdateNumericProperty",
    shape: "48 89 5c 24 10 57 48 83 ec 20 48 8b 01 48 8b f9 88 54 24 30 48 8b 15 ?? ?? ?? ?? 44 89 44 24 34 48 8b 98 20 02 00 00 e8",
};

/// Where the name's displacement is in [`UPDATE_NUMERIC_PROPERTY`], and where the instruction
/// using it ends.
const EVENT_NAME_DISPLACEMENT: usize = 0x17;
const EVENT_NAME_INSTRUCTION_END: usize = 0x1B;

pub type UpdateNumericPropertyFn =
    unsafe extern "C" fn(this: *mut c_void, value_type: u8, num_axes: i32);

/// Finds `UpdateNumericProperty` among `UpdateValue`'s calls: the one with its shape that looks up
/// the event by its name. After engine init (the name is made when the game starts).
pub fn update_numeric_property() -> Result<*const u8, String> {
    let update_value = UPDATE_VALUE.resolve()?;
    sig::direct_calls_in_all_parts(update_value)
        .into_iter()
        .filter(|&target| sig::is_function_start(target))
        .find(|&target| {
            // SAFETY: the shape matched, so the code at `target` holds a `mov rdx, [rip + disp32]`
            // whose displacement is at its offset; the static name it loads is in the exe's data.
            UPDATE_NUMERIC_PROPERTY.matches(target)
                && unsafe {
                    let displacement = target
                        .add(EVENT_NAME_DISPLACEMENT)
                        .cast::<i32>()
                        .read_unaligned();
                    let name = target
                        .add(EVENT_NAME_INSTRUCTION_END)
                        .offset(displacement as isize)
                        .cast::<FName>();
                    reflection::name_is(&name.read_unaligned(), "UpdateNumericProperty")
                }
        })
        .ok_or_else(|| {
            format!(
                "{}: not called by UpdateValue",
                UPDATE_NUMERIC_PROPERTY.name
            )
        })
}

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

/// Around a numeric row: its container and the container's button panel, and the row's own way to
/// change a component's value.
pub struct RowParts {
    /// `UPropertyWidget::GetPropertyContainerWidget()`.
    container: Getter<*mut UObjectBase>,
    /// `UPropertyContainerWidget::GetMenuButtonPanelWidget()`.
    button_panel: Getter<*mut UObjectBase>,
    /// Offset of `UPropertyContainerWidget::PropertyWidget`, the row it shows now.
    shown_row: usize,
    /// `UNumericPropertyWidget::OnSliderValueChanged(float Value, EValueChangedEventType Type,
    /// int32 SliderIndex)`, and its parameters' offsets.
    value_changed: *const UFunction,
    value: usize,
    event_type: usize,
    slider_index: usize,
}

// SAFETY: reflection records live as long as the game; only used on the game thread.
unsafe impl Send for RowParts {}
unsafe impl Sync for RowParts {}

pub fn row_parts() -> Result<RowParts, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let row = class("PropertyWidget")?;
    let container = class("PropertyContainerWidget")?;
    let numeric = class("NumericPropertyWidget")?;
    let value_changed = reflection::find_function(numeric, "OnSliderValueChanged")?;
    // SAFETY: a live function record; its parameters in order.
    let (value, event_type, slider_index) = unsafe {
        let parameters: Vec<_> = reflection::properties(value_changed.cast()).collect();
        match parameters[..] {
            [value, event_type, index]
                if reflection::is_kind(value, "FloatProperty")
                    // An enum is an EnumProperty, or a ByteProperty for the older `TEnumAsByte`.
                    && (reflection::is_kind(event_type, "EnumProperty")
                        || reflection::is_kind(event_type, "ByteProperty"))
                    && (*event_type).element_size == 1
                    && reflection::is_kind(index, "IntProperty")
                    && Parameters::fit(value_changed) =>
            {
                (
                    (*value).offset_internal as usize,
                    (*event_type).offset_internal as usize,
                    (*index).offset_internal as usize,
                )
            }
            _ => return Err("OnSliderValueChanged: unexpected parameters".into()),
        }
    };
    Ok(RowParts {
        container: Getter::find(row, "GetPropertyContainerWidget", "ObjectProperty")?,
        button_panel: Getter::find(container, "GetMenuButtonPanelWidget", "ObjectProperty")?,
        shown_row: reflection::field_offset(container, "PropertyWidget", "ObjectProperty", 8)?,
        value_changed,
        value,
        event_type,
        slider_index,
    })
}

impl RowParts {
    /// The button panel of the container showing `row`, if it still shows it (containers are
    /// reused for other rows).
    ///
    /// # Safety
    /// `row` must be a live `UNumericPropertyWidget`. Game thread only.
    pub unsafe fn button_panel(&self, row: *mut UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; a live container's `PropertyWidget` is an object pointer.
        unsafe {
            let container = self.container.call(row);
            if container.is_null()
                || container
                    .cast::<u8>()
                    .add(self.shown_row)
                    .cast::<*mut UObjectBase>()
                    .read()
                    != row
            {
                return None;
            }
            let panel = self.button_panel.call(container);
            (!panel.is_null()).then_some(panel)
        }
    }

    /// Sets component `index` of the value `row` edits to `value`, as its slider does: the first
    /// edited object's value with that component replaced (every component if the row's axes are
    /// locked) goes to every edited object, as one step to undo; `event_type` as a slider sends it
    /// (`game::slider::COMMITTED` for a final value).
    ///
    /// # Safety
    /// `row` must be a live `UNumericPropertyWidget` with a slider `index`. Game thread only.
    pub unsafe fn set_component(
        &self,
        row: *mut UObjectBase,
        index: usize,
        value: f32,
        event_type: u8,
    ) {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the parameters are written at their reflected offsets with
        // their types (checked in `row_parts`).
        unsafe {
            reflection::put(&mut parameters, self.value, value);
            reflection::put(&mut parameters, self.event_type, event_type);
            reflection::put(&mut parameters, self.slider_index, index as i32);
            reflection::call(row, self.value_changed, parameters.as_mut_ptr());
        }
    }
}
