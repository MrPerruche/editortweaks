//! Copying a reflected property's value from one object to another that has it, of any type
//! (numbers, bools, strings, texts, structs, arrays, ...): the property's own virtual functions
//! compare and copy it, as Unreal's `FProperty::Identical_InContainer` and `CopyCompleteValue` do.
//!
//! UE 4.27 `FProperty` virtual function table slots (`brickrust::ue::fproperty::FPropertyVTable`,
//! the same in BRMK 1.11.6's `FStrProperty` table, `??_7FStrProperty@@6B@`, see
//! `docs/re/editor-operations.md`):
//!
//! - 17 `bool Identical(const void* A, const void* B, uint32 PortFlags) const`: one element.
//! - 23 `void CopyValuesInternal(void* Dest, const void* Src, int32 Count) const`: `Count`
//!   elements, constructing nothing (both hold values already); `CopyCompleteValue` calls it for
//!   all but plain data, which it copies with `memcpy`.

use core::ffi::c_void;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;
use brickrust::ue::fproperty::FProperty;
use brickrust::ue::uclass::UStruct;

use crate::game::reflection;

/// `EPropertyFlags::CPF_IsPlainOldData`: the value can be copied with `memcpy`.
const PLAIN_OLD_DATA: u64 = 0x4000_0000;

/// The virtual function slots used (see the module documentation).
const IDENTICAL_SLOT: usize = 17;
const COPY_VALUES_SLOT: usize = 23;

type IdenticalFn = unsafe extern "C" fn(
    property: *const FProperty,
    a: *const c_void,
    b: *const c_void,
    port_flags: u32,
) -> bool;
type CopyValuesFn = unsafe extern "C" fn(
    property: *const FProperty,
    dest: *mut c_void,
    src: *const c_void,
    count: i32,
);

/// Copies `count` elements of `property`'s type from `source` to `target`, as `CopyCompleteValue`
/// does: `memcpy` for plain data, otherwise the property's own `CopyValuesInternal` (allocating
/// what `target` needs with the game's allocator, only reading `source`).
///
/// # Safety
/// `property` must be a live property record; `source` and `target` must each hold `count` valid
/// values of its type, and not overlap. Game thread only.
pub unsafe fn copy_values(
    property: *const FProperty,
    target: *mut u8,
    source: *const u8,
    count: i32,
) {
    // SAFETY: per the contract; slot 23 is `CopyValuesInternal` (see the module documentation).
    unsafe {
        if (*property).property_flags & PLAIN_OLD_DATA != 0 {
            let size = (*property).element_size as usize * count as usize;
            core::ptr::copy_nonoverlapping(source, target, size);
        } else {
            let slot = *(*property)
                .vtbl
                .cast::<*const c_void>()
                .add(COPY_VALUES_SLOT);
            let copy_values: CopyValuesFn = core::mem::transmute(slot);
            copy_values(property, target.cast(), source.cast(), count);
        }
    }
}

/// One property of a class, to copy between objects having it.
#[derive(Clone, Copy)]
pub struct CopiedProperty {
    /// The property's name, for change events.
    pub name: FName,
    /// The class declaring it: the objects having it are of it.
    owner: *const UStruct,
    property: *const FProperty,
}

impl CopiedProperty {
    /// The property `name` of `class` (or its parents).
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

    /// `property`, a class' own.
    ///
    /// # Safety
    /// `property` must be a live property record of a class, living as long as the game (those of
    /// native classes do).
    pub unsafe fn of(property: *const FProperty) -> Result<Self, String> {
        // SAFETY: per the contract.
        unsafe {
            let name = (*property).field.name_private;
            let owner = reflection::declaring_struct(property);
            if owner.is_null() || !reflection::is_class(owner) {
                return Err(format!("{} isn't a class'", reflection::name_text(&name)));
            }
            if (*property).vtbl.is_null() || (*property).element_size <= 0 {
                return Err(format!("{} isn't a property", reflection::name_text(&name)));
            }
            Ok(Self {
                name,
                owner,
                property,
            })
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

    /// Whether `from` and `to` hold different values (any element of a fixed size array).
    ///
    /// # Safety
    /// `from` and `to` must be live objects having the property. Game thread only.
    pub unsafe fn differs(&self, from: *const UObjectBase, to: *const UObjectBase) -> bool {
        // SAFETY: per the contract, both objects hold the value where the property says; slot 17
        // is `Identical` (see the module documentation).
        unsafe {
            let identical: IdenticalFn = core::mem::transmute(self.slot(IDENTICAL_SLOT));
            let size = (*self.property).element_size as usize;
            (0..(*self.property).array_dim.max(1) as usize).any(|index| {
                let at = self.offset() + index * size;
                !identical(
                    self.property,
                    from.cast::<u8>().add(at).cast(),
                    to.cast::<u8>().add(at).cast(),
                    0,
                )
            })
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
        // objects don't overlap; slot 23 is `CopyValuesInternal` (see the module documentation).
        unsafe {
            let (source, target) = (
                from.cast::<u8>().add(self.offset()),
                to.cast::<u8>().add(self.offset()),
            );
            let count = (*self.property).array_dim.max(1);
            copy_values(self.property, target, source, count);
        }
    }

    /// Where objects hold the value.
    fn offset(&self) -> usize {
        // SAFETY: the record lives as long as the game (see `of`).
        unsafe { (*self.property).offset_internal as usize }
    }

    /// The property's virtual function in `slot`.
    ///
    /// # Safety
    /// `slot` must be one of its table.
    unsafe fn slot(&self, slot: usize) -> *const c_void {
        // SAFETY: per the contract; the record and its table live as long as the game.
        unsafe { *(*self.property).vtbl.cast::<*const c_void>().add(slot) }
    }
}
