//! Copying a reflected property's value from one object to another that has it: only for values
//! that are plain data (numbers, enums, colors, names, plain structs), object references, which
//! need no constructing, freeing or counting (Unreal's own `FProperty::CopyCompleteValue` copies
//! these byte for byte), and bools, bit for bit.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;
use brickrust::ue::fproperty::FProperty;
use brickrust::ue::uclass::UStruct;

use crate::game::reflection::{self, BoolField};

/// `EPropertyFlags::CPF_IsPlainOldData`: the value can be copied with `memcpy`.
const PLAIN_OLD_DATA: u64 = 0x4000_0000;

/// One property of a class, to copy between objects having it.
#[derive(Clone, Copy)]
pub struct CopiedProperty {
    /// The property's name, for change events.
    pub name: FName,
    /// The class declaring it: the objects having it are of it.
    owner: *const UStruct,
    value: Value,
}

/// Where an object holds a [`CopiedProperty`]'s value.
#[derive(Clone, Copy)]
enum Value {
    /// `size` bytes at `offset`.
    Bytes { offset: usize, size: usize },
    /// A bool, maybe one bit of a byte shared with others.
    Bool(BoolField),
}

impl CopiedProperty {
    /// The property `name` of `class` (or its parents), if its values can be copied.
    ///
    /// # Safety
    /// `class` must be a live, linked class.
    pub unsafe fn find(class: *const UStruct, name: &'static str) -> Result<Self, String> {
        // SAFETY: per the contract; the property record lives as long as its class.
        unsafe {
            let property = reflection::find_property(class, name);
            if property.is_null() {
                return Err(format!("no property {name}"));
            }
            Self::of(property)
        }
    }

    /// `property`, a class' own, if its values can be copied.
    ///
    /// # Safety
    /// `property` must be a live property record of a class.
    pub unsafe fn of(property: *const FProperty) -> Result<Self, String> {
        // SAFETY: per the contract.
        unsafe {
            let name = (*property).field.name_private;
            let owner = reflection::declaring_struct(property);
            if owner.is_null() || !reflection::is_class(owner) {
                return Err(format!("{} isn't a class'", reflection::name_text(&name)));
            }
            let value = if reflection::is_kind(property, "BoolProperty") {
                Value::Bool(BoolField::of(property).ok_or_else(|| {
                    format!("{} isn't a bool as expected", reflection::name_text(&name))
                })?)
            } else if (*property).property_flags & PLAIN_OLD_DATA != 0
                || reflection::is_kind(property, "ObjectProperty")
            {
                let size = (*property).element_size * (*property).array_dim;
                Value::Bytes {
                    offset: (*property).offset_internal as usize,
                    size: usize::try_from(size)
                        .map_err(|_| format!("{} has no size", reflection::name_text(&name)))?,
                }
            } else {
                return Err(format!("{} isn't plain data", reflection::name_text(&name)));
            };
            Ok(Self { name, owner, value })
        }
    }

    /// The class declaring the property.
    pub fn owner(&self) -> *const UStruct {
        self.owner
    }

    /// Whether `object` has the property.
    ///
    /// # Safety
    /// `object` must be a live object.
    pub unsafe fn applies_to(&self, object: *const UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { reflection::is_a(object, self.owner) }
    }

    /// Whether `from` and `to` hold different values.
    ///
    /// # Safety
    /// `from` and `to` must be live objects having the property. Game thread only.
    pub unsafe fn differs(&self, from: *const UObjectBase, to: *const UObjectBase) -> bool {
        // SAFETY: per the contract, both objects hold the value where the property says.
        unsafe {
            match self.value {
                Value::Bytes { offset, size } => {
                    let bytes = |object: *const UObjectBase| {
                        core::slice::from_raw_parts(object.cast::<u8>().add(offset), size)
                    };
                    bytes(from) != bytes(to)
                }
                Value::Bool(field) => field.get(from.cast()) != field.get(to.cast()),
            }
        }
    }

    /// Gives `to` the value `from` has. Nothing is told of the change.
    ///
    /// # Safety
    /// `from` and `to` must be live objects having the property. Game thread only.
    pub unsafe fn copy(&self, from: *const UObjectBase, to: *mut UObjectBase) {
        if core::ptr::eq(from, to) {
            return;
        }
        // SAFETY: per the contract, both objects hold the value where the property says, and two
        // objects don't overlap.
        unsafe {
            match self.value {
                Value::Bytes { offset, size } => core::ptr::copy_nonoverlapping(
                    from.cast::<u8>().add(offset),
                    to.cast::<u8>().add(offset),
                    size,
                ),
                Value::Bool(field) => field.set(to.cast(), field.get(from.cast())),
            }
        }
    }
}
