//! The layout of a properties panel's rows (`WBP_PropertyContainer`, `docs/re/menu-body.md`). A row
//! laid out horizontally, as a settings page's are, is its icon, its name and its value's grid side
//! by side in a horizontal box: the name's slot fills 1.0 of the width, the value's grid 1.5 (set by
//! the row's Blueprint, `AddPropertyWidget`, each time the row gets a property to show).
//! [`RowLayout::widen`] gives a row's name the whole width, centred: a row that is only its text.
//! All through reflection.
//!
//! Only the name's slot is changed: a row gets its value widget a frame after it's listed (loaded
//! asynchronously), and `AddPropertyWidget` then sets the grid's size again, but not the name's.
//!
//! Rows are reused: a row showing the same property again is only updated, keeping its layout, so
//! what `widen` changed must be put back ([`RowLayout::restore`]) before its panel lists rows again.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::tarray::TArray;
use brickrust::ue::uclass::UStruct;

use crate::game::kismet::{self, Texts};
use crate::game::reflection::{self, Function, Parameters, SingleCall};
use crate::game::text::Text;
use crate::game::weak_object::WeakObject;

/// The name's share of a widened row, against the value grid's 1.5: nearly all of it.
const WIDE: f32 = 1000.0;

pub struct RowLayout {
    /// Offsets of `UMenuSettingsPageWidget::PropertiesPanel` and
    /// `UPropertiesPanelWidget::PropertyContainerWidgets` (its rows).
    page_panel: usize,
    panel_rows: usize,
    /// Offset of `UWidget::Slot`.
    slot: usize,
    /// `UHorizontalBoxSlot`, the offsets of its `Size` (an `FSlateChildSize`: `Value`, `SizeRule`)
    /// and `HorizontalAlignment`, and their setters.
    box_slot: *const UStruct,
    size: usize,
    size_value: usize,
    size_rule: usize,
    alignment: usize,
    set_size: Function,
    set_size_input: usize,
    set_alignment: SingleCall,
    /// `ESlateSizeRule::Fill` and `HAlign_Center`.
    fill: u8,
    center: u8,
    /// `UTextBlock` and the offset of its `Text`.
    text_block: *const UStruct,
    text: usize,
    texts: Texts,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for RowLayout {}
unsafe impl Sync for RowLayout {}

/// What [`RowLayout::widen`] changed in a row's name slot, to put back.
pub struct Widened {
    name_slot: WeakObject,
    value: f32,
    rule: u8,
    alignment: u8,
}

/// Finds the rows' reflection. After engine init.
pub fn find() -> Result<RowLayout, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let object = |class, name| reflection::field_offset(class, name, "ObjectProperty", 8);
    let enum_byte = |name, entry| -> Result<u8, String> {
        let value = reflection::enum_value(name, entry)?;
        u8::try_from(value).map_err(|_| format!("{entry} is {value}"))
    };
    let box_slot = class("HorizontalBoxSlot")?;
    let size = reflection::field(box_slot, "Size", "StructProperty", 8)?;
    // SAFETY: a live StructProperty record.
    let (size, child_size) = unsafe {
        (
            (*size).offset_internal as usize,
            reflection::struct_type(size),
        )
    };
    if child_size.is_null() {
        return Err("HorizontalBoxSlot::Size holds no struct".into());
    }
    let set_size = Function::find(box_slot, "SetSize")?;
    let text_block = class("TextBlock")?;
    Ok(RowLayout {
        page_panel: object(class("MenuSettingsPageWidget")?, "PropertiesPanel")?,
        panel_rows: reflection::field_offset(
            class("PropertiesPanelWidget")?,
            "PropertyContainerWidgets",
            "ArrayProperty",
            0x10,
        )?,
        slot: object(class("Widget")?, "Slot")?,
        box_slot: box_slot.cast(),
        size,
        size_value: reflection::struct_field_offset(child_size, "Value", "FloatProperty", 4)?,
        size_rule: reflection::struct_field_offset(child_size, "SizeRule", "ByteProperty", 1)?,
        alignment: reflection::field_offset(box_slot, "HorizontalAlignment", "ByteProperty", 1)?,
        set_size_input: set_size.parameter("InSize", "StructProperty", 8)?,
        set_size,
        set_alignment: SingleCall::find_only(
            box_slot,
            "SetHorizontalAlignment",
            &["ByteProperty", "EnumProperty"],
            1,
        )?,
        fill: enum_byte("ESlateSizeRule", "ESlateSizeRule::Fill")?,
        center: enum_byte("EHorizontalAlignment", "HAlign_Center")?,
        text_block: text_block.cast(),
        text: reflection::field_offset(text_block, "Text", "TextProperty", 0x18)?,
        texts: kismet::texts()?,
    })
}

/// The object pointer at `offset` in `object`, if not null.
///
/// # Safety
/// `object` must be a live object with an object pointer field at `offset`.
unsafe fn pointer_at(object: *mut UObjectBase, offset: usize) -> Option<*mut UObjectBase> {
    // SAFETY: per the contract.
    let pointer = unsafe {
        object
            .cast::<u8>()
            .add(offset)
            .cast::<*mut UObjectBase>()
            .read()
    };
    (!pointer.is_null()).then_some(pointer)
}

impl RowLayout {
    /// The rows of the settings page `page` (a `UMenuSettingsPageWidget`), in order.
    ///
    /// # Safety
    /// `page` must be a live settings page. Game thread only.
    pub unsafe fn rows(&self, page: *mut UObjectBase) -> Vec<*mut UObjectBase> {
        // SAFETY: per the contract; the panel's rows are a `TArray` of live widgets' pointers.
        unsafe {
            let Some(panel) = pointer_at(page, self.page_panel) else {
                return Vec::new();
            };
            let rows = &*panel
                .cast::<u8>()
                .add(self.panel_rows)
                .cast::<TArray<*mut UObjectBase>>();
            match usize::try_from(rows.num) {
                Ok(count) if count > 0 && !rows.data.is_null() => {
                    core::slice::from_raw_parts(rows.data, count)
                        .iter()
                        .copied()
                        .filter(|row| !row.is_null())
                        .collect()
                }
                _ => Vec::new(),
            }
        }
    }

    /// The text block showing `row`'s name (its `NameTextBlock`, a field of its Blueprint class).
    ///
    /// # Safety
    /// `row` must be a live row (`UPropertyContainerWidget`). Game thread only.
    unsafe fn name_block(&self, row: *mut UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; the field is checked to be an object pointer, and what it
        // points to to be a text block.
        unsafe {
            let field =
                reflection::find_property(reflection::class_of(row.cast()), "NameTextBlock");
            if field.is_null() || !reflection::is_kind(field, "ObjectProperty") {
                return None;
            }
            let block = pointer_at(row, (*field).offset_internal as usize)?;
            reflection::is_a(block, self.text_block).then_some(block)
        }
    }

    /// The name `row` shows.
    ///
    /// # Safety
    /// As for [`Self::name_block`].
    pub unsafe fn name(&self, row: *mut UObjectBase) -> Option<String> {
        // SAFETY: per the contract; a text block's `Text` is a live `FText`, copied (counted)
        // before the conversion gives the copy back.
        unsafe {
            let block = self.name_block(row)?;
            let text = Text::copy_of(block.cast::<u8>().add(self.text).cast());
            Some(self.texts.string(text))
        }
    }

    /// Gives `row`'s name the whole width, centred: its slot fills [`WIDE`] against the value
    /// grid's 1.5 (a fill slot gets its share of the width whatever its content asks for). What it
    /// changed, if it could (a row laid out horizontally).
    ///
    /// # Safety
    /// As for [`Self::name_block`].
    pub unsafe fn widen(&self, row: *mut UObjectBase) -> Option<Widened> {
        // SAFETY: per the contract; the name block's slot is a live object, checked to be a
        // horizontal box slot before its fields are read.
        unsafe {
            let name_slot = pointer_at(self.name_block(row)?, self.slot)?;
            if !reflection::is_a(name_slot, self.box_slot) {
                return None;
            }
            let size = name_slot.cast::<u8>().add(self.size);
            let widened = Widened {
                name_slot: WeakObject::new(name_slot),
                value: size.add(self.size_value).cast::<f32>().read_unaligned(),
                rule: size.add(self.size_rule).read(),
                alignment: name_slot.cast::<u8>().add(self.alignment).read(),
            };
            self.resize(name_slot, WIDE, self.fill);
            self.set_alignment.call(name_slot, self.center);
            Some(widened)
        }
    }

    /// Whether [`Self::widen`] already widened `row` (its name fills [`WIDE`]).
    ///
    /// # Safety
    /// As for [`Self::name_block`].
    pub unsafe fn is_wide(&self, row: *mut UObjectBase) -> bool {
        // SAFETY: per the contract; as in `widen`.
        unsafe {
            let Some(name_slot) = self
                .name_block(row)
                .and_then(|block| pointer_at(block, self.slot))
            else {
                return false;
            };
            if !reflection::is_a(name_slot, self.box_slot) {
                return false;
            }
            let size = name_slot.cast::<u8>().add(self.size);
            size.add(self.size_value).cast::<f32>().read_unaligned() == WIDE
                && size.add(self.size_rule).read() == self.fill
        }
    }

    /// Puts back what [`Self::widen`] changed, if the slot is still alive.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn restore(&self, widened: &Widened) {
        // SAFETY: per the contract; the slot is alive (checked) and a horizontal box slot.
        unsafe {
            if let Some(slot) = widened.name_slot.get() {
                self.resize(slot, widened.value, widened.rule);
                self.set_alignment.call(slot, widened.alignment);
            }
        }
    }

    /// `SetSize` on the horizontal box slot `slot`.
    ///
    /// # Safety
    /// `slot` must be a live `UHorizontalBoxSlot`. Game thread only.
    unsafe fn resize(&self, slot: *mut UObjectBase, value: f32, rule: u8) {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the struct's fields at their reflected offsets.
        unsafe {
            reflection::put(
                &mut parameters,
                self.set_size_input + self.size_value,
                value,
            );
            reflection::put(&mut parameters, self.set_size_input + self.size_rule, rule);
            self.set_size.call(slot, &mut parameters);
        }
    }
}
