//! The vehicle editor's archive: some of its objects (or all of them) saved to bytes and loaded
//! back, the way its files, undo steps and duplicates are; and objects deleted. Coop sends
//! vehicles and new bricks this way (`docs/re/coop/`).
//!
//! Found statically in game 1.11.7 (build 25831956) against BRMK 1.11.6
//! (`docs/re/coop/archive-addresses.md`). A `.brv` file is exactly what `SaveObjectsToArchive`
//! writes for all of a vehicle's objects. The game builds its memory archives inline wherever it
//! uses one, so [`VehicleArchive`] does what its undo (reading) and `PostModify` (writing) do,
//! with every function, vtable and offset read from those two places:
//!
//! ```text
//! FMemoryReader Reader(Bytes);   // FArchive(); vtable; Offset = 0; Bytes = &Bytes;
//!                                // LimitSize = INT64_MAX; SetIsLoading(true); SetIsPersistent(false)
//! FSafeArchive Safe{&Reader};    // the game's wrapper: only the archive's address
//! Component->LoadFromArchive(Safe, Bytes, bAppend, bFromUndo);
//! Reader.~FArchive();
//! ```
//!
//! The writer is the same with `FMemoryWriter`'s vtable, `ArchiveName = NAME_None` instead of
//! the limit, and `SetIsSaving(true)`.

use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::tarray::TArray;

use crate::framework::sig::{CodeShape, FunctionSig, SigMatch};
use crate::game::reflection;

/// Undo building its reader and loading the step's vehicle (in `ABrickEditor::UndoRedo`, BRMK
/// 0xF4740, game RVA 0xD0E300; the match is at 0xD0E466).
pub const READER: FunctionSig = FunctionSig {
    name: "ABrickEditor::UndoRedo memory reader",
    // FArchive() (+0), the vtable (+5), Offset (+12), the vtable stored (+16: the archive's
    // place), Bytes (+31), LimitSize (+45), SetIsLoading (+49), SetIsPersistent's vtable offset
    // (+66), then LoadFromArchive (+108) and the destructor (+117).
    pattern: "e8 ?? ?? ?? ?? 48 8d 05 ?? ?? ?? ?? 48 89 75 ?? 48 89 45 ?? 48 8d 4d ?? 48 8d 44 24 ?? b2 01 48 89 45 ?? 48 b8 ff ff ff ff ff ff ff 7f 48 89 45 ?? e8 ?? ?? ?? ?? 48 8b 45 ?? 48 8d 4d ?? 33 d2 ff 90 ?? ?? 00 00 49 8b 8f ?? ?? 00 00 48 8d 45 ?? 45 33 c9 48 89 85 ?? ?? 00 00 4c 8d 44 24 ?? c6 44 24 20 01 48 8d 95 ?? ?? 00 00 e8 ?? ?? ?? ?? 48 8d 4d ?? e8 ?? ?? ?? ??",
};

/// `PostModify` building its writer for the undo snapshot (`ABrickEditor::PostModify`, BRMK
/// 0xC6A90, game RVA 0xCF3840; the match is at 0xCF3960).
pub const WRITER: FunctionSig = FunctionSig {
    name: "ABrickEditor::PostModify memory writer",
    // FArchive() (+0), the vtable (+5), Offset (+12), the vtable stored (+16), ArchiveName
    // (+29), Bytes (+33), SetIsSaving (+42), SetIsPersistent's vtable offset (+59), then the
    // component's SaveToArchive (vtable offset at +94).
    pattern: "e8 ?? ?? ?? ?? 48 8d 05 ?? ?? ?? ?? 48 89 5d ?? 48 89 45 ?? 48 8d 4d ?? 48 8d 44 24 ?? 48 89 5d ?? 48 89 45 ?? b2 01 48 8b c3 e8 ?? ?? ?? ?? 48 8b 45 ?? 48 8d 4d ?? 33 d2 ff 90 ?? ?? 00 00 48 8b 8f ?? ?? 00 00 48 8d 45 ?? 48 89 45 ?? 4c 8d 45 ?? 48 8d 95 ?? ?? 00 00 48 8b 01 ff 90 ?? ?? 00 00",
};

/// `UBrickEditorInterfaceComponent::SaveToArchive(FSafeArchive&)` (BRMK 0xE3890, game RVA
/// 0xD046B0), whole: `SaveObjectsToArchive` of every object with a zeroed header.
pub const SAVE_TO_ARCHIVE: FunctionSig = FunctionSig {
    name: "UBrickEditorInterfaceComponent::SaveToArchive",
    // The object array's offset at +27, `SaveObjectsToArchive` called at +49.
    pattern: "40 53 48 83 ec 30 33 c0 c6 44 24 40 00 88 44 24 28 4d 8b c8 89 44 24 42 4c 8d 81 ?? ?? 00 00 66 89 44 24 46 48 8b da 48 8d 44 24 40 48 89 44 24 20 e8 ?? ?? ?? ?? 48 8b c3 48 83 c4 30 5b c3",
};

/// `bool UBrickEditorInterfaceComponent::DestroyBrickEditorObjects(const TArray<UBrickEditorObject*>&,
/// bool)` (BRMK 0x93C80, game RVA 0xCD4C00): what deleting, undo and loads destroy objects with.
/// It runs as an editor operation (flag 8, joining an open one) and updates the selection.
pub const DESTROY_OBJECTS: FunctionSig = FunctionSig {
    name: "UBrickEditorInterfaceComponent::DestroyBrickEditorObjects",
    // Its start: saving the flag, then returning false for no objects.
    pattern: "44 88 44 24 18 53 41 55 41 57 48 83 ec 70 44 8b 6a 08 4c 8b fa 48 8b d9 45 85 ed 75 0c 32 c0",
};

/// `FArchiveState::SetIsLoading(bool)` (game RVA 0x1159DA0), called by [`READER`]: a leaf
/// function setting the flags' bit 0, so it has no unwind entry to check it by.
const SET_IS_LOADING: CodeShape = CodeShape {
    name: "FArchiveState::SetIsLoading",
    // The flags' offset at +2 and +6.
    shape: "80 61 ?? fe 08 51 ?? c3",
};

/// `FArchiveState::SetIsSaving(bool)` (game RVA 0x1159DC0), called by [`WRITER`]: bit 1.
const SET_IS_SAVING: CodeShape = CodeShape {
    name: "FArchiveState::SetIsSaving",
    // The flags' offset at +2 and +8.
    shape: "80 61 ?? fd 02 d2 08 51 ?? c3",
};

pub type DestroyObjectsFn = unsafe extern "C" fn(
    component: *mut UObjectBase,
    objects: *const TArray<*mut UObjectBase>,
    all: bool,
) -> bool;

/// [`DESTROY_OBJECTS`], found once: `vehicle_changes` hooks it at load, which overwrites the
/// start its pattern matches, so it can't be searched for after that.
pub fn destroy_objects() -> Result<*const u8, String> {
    static FOUND: OnceLock<Result<usize, String>> = OnceLock::new();
    FOUND
        .get_or_init(|| DESTROY_OBJECTS.resolve().map(|target| target as usize))
        .clone()
        .map(|target| target as *const u8)
}

/// The game's `FSafeArchive`: only the archive's address (both places above build it so).
#[repr(C)]
struct SafeArchive {
    archive: *mut u8,
}

/// Room for an `FMemoryReader` or `FMemoryWriter` (0xB0 bytes in game 1.11.7; [`find`] checks
/// the fields it found lie within).
const ARCHIVE_SIZE: usize = 0x100;

#[repr(C, align(16))]
struct ArchiveMemory([u8; ARCHIVE_SIZE]);

/// What `SaveObjectsToArchive` returns (in a slot the caller passes): a code, 0 when saved, then
/// a "checked" byte. Padded.
#[repr(C)]
struct SaveResult {
    code: u32,
    _checked: u8,
    _rest: [u8; 11],
}

type ConstructFn = unsafe extern "C" fn(archive: *mut u8) -> *mut u8;
type DestructFn = unsafe extern "C" fn(archive: *mut u8);
type SetFlagFn = unsafe extern "C" fn(archive: *mut u8, value: bool);
type LoadFromArchiveFn = unsafe extern "C" fn(
    component: *mut UObjectBase,
    archive: *const SafeArchive,
    bytes: *mut TArray<u8>,
    append: bool,
    from_undo: bool,
) -> u32;
type SaveObjectsFn = unsafe extern "C" fn(
    component: *mut UObjectBase,
    result: *mut SaveResult,
    objects: *const TArray<*mut UObjectBase>,
    archive: *const SafeArchive,
    header: *mut u8,
    skip_header: bool,
) -> *mut SaveResult;

/// One kind of memory archive, as the game builds it.
struct Kind {
    vtable: *const u8,
    /// `SetIsLoading` or `SetIsSaving`.
    set_direction: SetFlagFn,
    /// The reader's `LimitSize` or the writer's `ArchiveName`.
    last_field: usize,
}

pub struct VehicleArchive {
    construct: ConstructFn,
    destruct: DestructFn,
    /// Offset of `SetIsPersistent` in archives' vtables.
    set_persistent: usize,
    /// Offsets of `FMemoryArchive::Offset` and of the archive's `Bytes` (a `TArray<uint8>*`).
    offset: usize,
    bytes: usize,
    reader: Kind,
    writer: Kind,
    load_from_archive: LoadFromArchiveFn,
    save_objects: SaveObjectsFn,
    destroy_objects: DestroyObjectsFn,
}

// SAFETY: game code and offsets living as long as the game; only used on the game thread.
unsafe impl Send for VehicleArchive {}
unsafe impl Sync for VehicleArchive {}

/// The signed `[rbp+disp8]` of a store `48 89 xx disp8` starting at `at` in the match.
///
/// # Safety
/// `at + 4` must lie within the matched pattern.
unsafe fn displacement8(found: &SigMatch, at: usize) -> isize {
    // SAFETY: per the contract; the displacement is the instruction's last byte.
    isize::from(unsafe { found.address.add(at + 3).cast::<i8>().read() })
}

/// The `u32` displacement at `at` in the match (a vtable offset).
///
/// # Safety
/// `at + 4` must lie within the matched pattern.
unsafe fn displacement32(found: &SigMatch, at: usize) -> usize {
    // SAFETY: per the contract.
    unsafe { found.address.add(at).cast::<u32>().read_unaligned() as usize }
}

/// The address a `lea reg, [rip+disp32]` (7 bytes) at `at` in the match loads.
///
/// # Safety
/// `at + 7` must lie within the matched pattern.
unsafe fn rip_target(found: &SigMatch, at: usize) -> *const u8 {
    // SAFETY: per the contract.
    unsafe {
        let disp = found.address.add(at + 3).cast::<i32>().read_unaligned() as isize;
        found.address.add(at + 7).wrapping_offset(disp)
    }
}

/// A field's offset in the archive: its store's displacement minus the vtable store's (`base`).
///
/// # Safety
/// Both must lie within the matched pattern.
unsafe fn field(found: &SigMatch, base: usize, at: usize) -> Result<usize, String> {
    // SAFETY: per the contract.
    let offset = unsafe { displacement8(found, at) - displacement8(found, base) };
    usize::try_from(offset)
        .ok()
        .filter(|&offset| offset >= 8 && offset + 8 <= ARCHIVE_SIZE)
        .ok_or_else(|| format!("an archive field at {offset}, outside the archive"))
}

/// The function a `call rel32` at `at` in the match calls.
///
/// # Safety
/// `at + 5` must lie within the matched pattern.
unsafe fn call(found: &SigMatch, at: usize, what: &str) -> Result<*const u8, String> {
    // SAFETY: per the contract.
    unsafe { found.call_target(at) }.ok_or_else(|| format!("{what} isn't a call to a function"))
}

/// The leaf function a `call rel32` at `at` in the match calls, checked against its `shape`
/// (leaf functions have no unwind entry, which [`call`] checks).
///
/// # Safety
/// `at + 5` must lie within the matched pattern.
unsafe fn leaf_call(found: &SigMatch, at: usize, shape: &CodeShape) -> Result<*const u8, String> {
    // SAFETY: per the contract.
    let target = unsafe {
        let call = found.address.add(at);
        (*call == 0xE8).then(|| {
            let relative = call.add(1).cast::<i32>().read_unaligned() as isize;
            call.add(5).wrapping_offset(relative)
        })
    };
    target
        .filter(|&target| shape.matches(target))
        .ok_or_else(|| format!("the call at +{at} isn't {}", shape.name))
}

/// Finds everything (after engine init: reflection checks the object array's offset).
pub fn find() -> Result<VehicleArchive, String> {
    let reader = READER.locate()?;
    let writer = WRITER.locate()?;
    let save = SAVE_TO_ARCHIVE.locate()?;
    let interface = reflection::find_class("BrickEditorInterfaceComponent")
        .ok_or("class BrickEditorInterfaceComponent not found")?;
    let objects = reflection::field_offset(interface, "BrickEditorObjects", "ArrayProperty", 0x10)?;
    // SAFETY: every offset read lies within its matched pattern (see the patterns' comments).
    unsafe {
        let construct = call(&reader, 0, "FArchive()")?;
        if call(&writer, 0, "FArchive()")? != construct {
            return Err("the reader and the writer start differently".into());
        }
        let set_persistent = displacement32(&reader, 66);
        let offset = field(&reader, 16, 12)?;
        let bytes = field(&reader, 16, 31)?;
        if set_persistent != displacement32(&writer, 59)
            || offset != field(&writer, 16, 12)?
            || bytes != field(&writer, 16, 33)?
        {
            return Err("the reader and the writer have different layouts".into());
        }
        if displacement32(&save, 27) != objects {
            return Err(format!(
                "{} saves another array than BrickEditorObjects",
                SAVE_TO_ARCHIVE.name
            ));
        }
        Ok(VehicleArchive {
            construct: core::mem::transmute::<*const u8, ConstructFn>(construct),
            destruct: core::mem::transmute::<*const u8, DestructFn>(call(
                &reader,
                117,
                "~FArchive()",
            )?),
            set_persistent,
            offset,
            bytes,
            reader: Kind {
                vtable: rip_target(&reader, 5),
                set_direction: core::mem::transmute::<*const u8, SetFlagFn>(leaf_call(
                    &reader,
                    49,
                    &SET_IS_LOADING,
                )?),
                last_field: field(&reader, 16, 45)?,
            },
            writer: Kind {
                vtable: rip_target(&writer, 5),
                set_direction: core::mem::transmute::<*const u8, SetFlagFn>(leaf_call(
                    &writer,
                    42,
                    &SET_IS_SAVING,
                )?),
                last_field: field(&writer, 16, 29)?,
            },
            load_from_archive: core::mem::transmute::<*const u8, LoadFromArchiveFn>(call(
                &reader,
                108,
                "LoadFromArchive",
            )?),
            save_objects: core::mem::transmute::<*const u8, SaveObjectsFn>(call(
                &save,
                49,
                "SaveObjectsToArchive",
            )?),
            destroy_objects: core::mem::transmute::<*const u8, DestroyObjectsFn>(destroy_objects()?),
        })
    }
}

/// `objects` as the `const TArray&` the game reads (it doesn't keep or change it).
fn object_list(objects: &[*mut UObjectBase]) -> Result<TArray<*mut UObjectBase>, String> {
    let count = i32::try_from(objects.len()).map_err(|_| "too many objects")?;
    Ok(TArray {
        data: objects.as_ptr().cast_mut(),
        num: count,
        max: count,
    })
}

impl VehicleArchive {
    /// The archive layout found, for the log.
    pub fn layout(&self) -> String {
        format!(
            "Offset at {:#x}, Bytes at {:#x}, LimitSize at {:#x}, ArchiveName at {:#x}, \
             SetIsPersistent at vtable {:#x}",
            self.offset,
            self.bytes,
            self.reader.last_field,
            self.writer.last_field,
            self.set_persistent
        )
    }

    /// Builds a memory archive of `kind` in `memory` over `bytes`, as the game does.
    ///
    /// # Safety
    /// `bytes` must outlive the archive, which must be destroyed with [`Self::close`].
    unsafe fn open(
        &self,
        memory: &mut ArchiveMemory,
        kind: &Kind,
        bytes: *mut TArray<u8>,
        last: u64,
    ) -> *mut u8 {
        let archive = memory.0.as_mut_ptr();
        // SAFETY: the game's own sequence (see the module's docs), with offsets from its code and
        // checked to lie within `memory`; the vtable's slot is one the game calls there too.
        unsafe {
            (self.construct)(archive);
            archive.cast::<*const u8>().write(kind.vtable);
            archive.add(self.offset).cast::<i64>().write(0);
            archive
                .add(self.bytes)
                .cast::<*mut TArray<u8>>()
                .write(bytes);
            archive.add(kind.last_field).cast::<u64>().write(last);
            (kind.set_direction)(archive, true);
            let vtable = kind.vtable;
            let set_persistent = vtable.add(self.set_persistent).cast::<SetFlagFn>().read();
            set_persistent(archive, false);
        }
        archive
    }

    /// # Safety
    /// `archive` must come from [`Self::open`], once.
    unsafe fn close(&self, archive: *mut u8) {
        // SAFETY: per the contract.
        unsafe { (self.destruct)(archive) }
    }

    /// `objects` of `component` (a `UBrickEditorInterfaceComponent`, e.g. the editor's) saved as
    /// a vehicle file of their own, in their order.
    ///
    /// # Safety
    /// `component` must be live and `objects` live objects of it. Game thread only.
    pub unsafe fn save(
        &self,
        component: *mut UObjectBase,
        objects: &[*mut UObjectBase],
    ) -> Result<Vec<u8>, String> {
        let list = object_list(objects)?;
        let mut memory = ArchiveMemory([0; ARCHIVE_SIZE]);
        let mut header = [0u8; 16];
        let mut result = SaveResult {
            code: 0,
            _checked: 0,
            _rest: [0; 11],
        };
        // SAFETY: per the contract. The writer grows `bytes` with the game's allocator, so it's
        // freed with it; `list` and `header` outlive the call.
        unsafe {
            let mut bytes = TArray::<u8>::new();
            // `NAME_None` for the archive's name.
            let archive = self.open(&mut memory, &self.writer, &mut bytes, 0);
            let safe = SafeArchive { archive };
            (self.save_objects)(
                component,
                &mut result,
                &list,
                &safe,
                header.as_mut_ptr(),
                false,
            );
            self.close(archive);
            let saved = match usize::try_from(bytes.num) {
                Ok(len) if len > 0 && !bytes.data.is_null() => {
                    core::slice::from_raw_parts(bytes.data, len).to_vec()
                }
                _ => Vec::new(),
            };
            bytes.free();
            match result.code {
                0 if !saved.is_empty() => Ok(saved),
                0 => Err("the game saved nothing".into()),
                code => Err(format!("the game couldn't save the objects (code {code})")),
            }
        }
    }

    /// Loads a vehicle file into `component`'s vehicle: added to it (`append`) or replacing it.
    /// `from_undo` loads it as undo does: without moving the camera or clearing the undo steps.
    /// The new objects pass `OnBrickEditorObjectsCreated` (context 1 if appended, else 0), see
    /// `game::vehicle_changes`.
    ///
    /// # Safety
    /// `component` must be live. Game thread only.
    pub unsafe fn load(
        &self,
        component: *mut UObjectBase,
        vehicle: &[u8],
        append: bool,
        from_undo: bool,
    ) -> Result<(), String> {
        let len = i32::try_from(vehicle.len()).map_err(|_| "vehicle too big")?;
        if len == 0 {
            return Err("an empty vehicle".into());
        }
        let mut memory = ArchiveMemory([0; ARCHIVE_SIZE]);
        // SAFETY: per the contract. The bytes are in the game's memory, as its own loads' are
        // (the load passes them on to listeners), and freed after.
        let result = unsafe {
            let mut source = TArray::<u8>::with_size(len);
            core::ptr::copy_nonoverlapping(vehicle.as_ptr(), source.data, vehicle.len());
            let archive = self.open(&mut memory, &self.reader, &mut source, i64::MAX as u64);
            let safe = SafeArchive { archive };
            let result = (self.load_from_archive)(component, &safe, &mut source, append, from_undo);
            self.close(archive);
            source.free();
            result
        };
        match result {
            0 => Ok(()),
            1 | 2 => Err("the game found the vehicle's header broken".into()),
            3 => Err("the game rejected the vehicle".into()),
            4 => Err("the vehicle is from a newer game".into()),
            other => Err(format!(
                "the game couldn't load the vehicle (result {other})"
            )),
        }
    }

    /// Deletes `objects` of `component`, as the Delete key does after its checks: one editor
    /// operation, joining an open one.
    ///
    /// # Safety
    /// `component` must be live and `objects` live objects of it. Game thread only.
    pub unsafe fn destroy(&self, component: *mut UObjectBase, objects: &[*mut UObjectBase]) {
        let Ok(list) = object_list(objects) else {
            return;
        };
        // SAFETY: per the contract; the game only reads the list.
        unsafe { (self.destroy_objects)(component, &list, false) };
    }
}
