//! Workshop item ids as the game holds them: a row's `OnlineItemId` (`FFluUGCItemIdWrapper`, a
//! reflected field of `FUGCFileInfo`) is a shared reference `{object, controller}` to an
//! `FFluUGCItemId`, whose object is `{vtable, PublishedFileId_t}` for Steam. See
//! `docs/re/ugc-steam-queries.md` (read in BRMK 1.11.6: FluUGC 0x2990, FluUGCSteam 0xD0A0).
//!
//! Ids are made the way the game reads them from text: the property's `ImportText` (the
//! `FProperty` vtable, as brickrust lays it out) with `"Steam:<id>"`, which the struct's own
//! `ImportTextItem` hands to `FFluUGCItemIdWrapper::FromString` (FluUGC 0x4010 -> 0x3830). Each id
//! made is checked by reading it back, so a layout that doesn't match only means no ids.

use core::ffi::c_void;
use std::cell::RefCell;
use std::collections::HashMap;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fproperty::FProperty;
use brickrust::ue::uclass::UStruct;

use crate::game::reflection;

/// `FFluUGCItemIdWrapper` (and `TSharedRef<FFluUGCItemId>`, laid out the same): the id object and
/// its reference controller.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ItemId {
    object: *mut u8,
    controller: *mut u8,
}

const _: () = assert!(size_of::<ItemId>() == 16);

impl ItemId {
    const NONE: Self = Self {
        object: core::ptr::null_mut(),
        controller: core::ptr::null_mut(),
    };

    /// The Workshop item id it holds, if any.
    ///
    /// # Safety
    /// The id must be a live one of the game's (or `NONE`). Game thread only.
    unsafe fn value(self) -> Option<u64> {
        if self.object.is_null() {
            return None;
        }
        // SAFETY: per the contract; a Steam id object keeps its `PublishedFileId_t` at +8
        // (FluUGCSteam 0xD0A0 reads it there).
        Some(unsafe { self.object.add(8).cast::<u64>().read_unaligned() })
    }
}

/// `const TCHAR* FProperty::ImportText_Internal(const TCHAR* Buffer, void* Data, int32 PortFlags,
/// UObject* OwnerObject, FOutputDevice* ErrorText) const`: null if the text wasn't understood.
type ImportTextFn = unsafe extern "C" fn(
    property: *const FProperty,
    buffer: *const u16,
    data: *mut u8,
    port_flags: i32,
    owner: *mut UObjectBase,
    error: *mut c_void,
) -> *const u16;

/// An `FOutputDevice` that ignores what it's told: the import's error messages, if it has any
/// (`Logf` calls its virtual `Serialize`). Every slot returns 0.
#[repr(C)]
struct QuietDevice {
    vtable: *const QuietTable,
    _flags: [u8; 8],
}

#[repr(C)]
struct QuietTable([unsafe extern "C" fn() -> usize; 32]);

unsafe extern "C" fn ignore() -> usize {
    0
}

static QUIET_TABLE: QuietTable = QuietTable([ignore; 32]);

/// Reads and makes the ids of rows.
pub struct ItemIds {
    property: *const FProperty,
    offset: usize,
}

// SAFETY: a reflection record living as long as the game; only used on the game thread.
unsafe impl Send for ItemIds {}
unsafe impl Sync for ItemIds {}

thread_local! {
    /// The ids made so far, by value: each keeps one reference of ours for the whole game, so the
    /// game never frees one we hand out. Game thread only.
    static MADE: RefCell<HashMap<u64, ItemId>> = RefCell::new(HashMap::new());
}

impl ItemIds {
    /// Finds `OnlineItemId` in the rows' struct `ustruct`.
    pub fn find(ustruct: *const UStruct) -> Result<Self, String> {
        let property = reflection::struct_field(
            ustruct,
            "OnlineItemId",
            "StructProperty",
            size_of::<ItemId>() as i32,
        )?;
        // SAFETY: a live StructProperty record; its struct lives as long as it.
        let wrapper = unsafe {
            let inner = reflection::struct_type(property);
            !inner.is_null()
                && reflection::name_is(&(*inner).ufield.uobject.name_private, "FluUGCItemIdWrapper")
        };
        if !wrapper {
            return Err("OnlineItemId isn't an FFluUGCItemIdWrapper".into());
        }
        Ok(Self {
            property,
            // SAFETY: as above.
            offset: unsafe { (*property).offset_internal as usize },
        })
    }

    /// The Workshop item id of `row`, if it has one (a Steam result's; local vehicles have none).
    ///
    /// # Safety
    /// `row` must be a live row (`FUGCQueryResult`). Game thread only.
    pub unsafe fn of_row(&self, row: *const u8) -> Option<u64> {
        // SAFETY: per the contract; the field is a wrapper (checked at start).
        unsafe {
            row.add(self.offset)
                .cast::<ItemId>()
                .read_unaligned()
                .value()
        }
    }

    /// The game's id for the Workshop item `id`, made the first time (kept for the game). The
    /// caller may copy it into an array the game reads, without a reference of its own, as long as
    /// the game doesn't destroy that array.
    ///
    /// # Safety
    /// Game thread only, after engine init (the Steam UGC subsystem makes the id).
    pub unsafe fn make(&self, id: u64) -> Result<ItemId, String> {
        if let Some(made) = MADE.with_borrow(|made| made.get(&id).copied()) {
            return Ok(made);
        }
        // Quoted: unquoted, the token would end at ':'. The plain number takes the default
        // subsystem, Steam's.
        for text in [format!("\"Steam:{id}\""), id.to_string()] {
            // SAFETY: per the contract.
            let made = unsafe { self.import(&text) };
            // SAFETY: what the import made is the game's own id, or none.
            if unsafe { made.value() } == Some(id) && !made.controller.is_null() {
                MADE.with_borrow_mut(|known| known.insert(id, made));
                return Ok(made);
            }
        }
        Err(format!("the game made no id of {id} from text"))
    }

    /// `text` imported into a new wrapper (`NONE` if the import made nothing).
    ///
    /// # Safety
    /// Game thread only, after engine init.
    unsafe fn import(&self, text: &str) -> ItemId {
        let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
        let mut made = ItemId::NONE;
        let mut device = QuietDevice {
            vtable: &QUIET_TABLE,
            _flags: [0; 8],
        };
        // SAFETY: the property's own `ImportText_Internal` (slot 22 of brickrust's
        // `FPropertyVTable`, UE 4.27's order), on a zeroed wrapper as the struct's import expects,
        // NUL-terminated text, no flags or owner, and an output device that ignores everything.
        unsafe {
            let import: ImportTextFn =
                core::mem::transmute((*(*self.property).vtbl).ImportText_Internal);
            import(
                self.property,
                wide.as_ptr(),
                (&raw mut made).cast(),
                0,
                core::ptr::null_mut(),
                (&raw mut device).cast(),
            );
        }
        made
    }
}
