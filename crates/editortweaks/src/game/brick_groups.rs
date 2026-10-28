//! The two groups each editor object is in (`EBrickGroupType`: 0 editor group, 1 weld group), as
//! group numbers (0: in none): coop reads and writes them (`docs/re/coop/change-hooks.md`,
//! section 3); only in builds with the `coop` feature.
//!
//! Neither is reflected: the weld group is a `uint16` in the object, the editor group a `uint16`
//! in its `EditorParams`. Every game function that groups objects writes these words inline, then
//! adds [`GROUPS_CHANGED`] to the open operation, whose end (`PostModifyGroups`, game 0xCF4260)
//! updates the editor from every object's groups. So coop writes them the same way, inside an
//! operation with that flag.

use brickrust::ue::coreuobject::UObjectBase;

use crate::framework::sig::FunctionSig;

/// `void ABrickEditor::RemoveObjectFromGroup(EBrickGroupType Type, UBrickEditorObject* Object)`
/// (BRMK 0xD7610, game 1.11.7 RVA 0xCFFE20, matched at +0x3B):
///
/// ```text
/// Group = Type == Weld ? Object->WeldGroup : Type == Editor && EditorParams ? EditorParams->Group : 0;
/// if (Group != 0) {
///     if (Type == Weld) Object->WeldGroup = 0;
///     else if (Type == Editor && EditorParams) EditorParams->Group = 0;
///     Operation->Flags |= 0x80;
/// }
/// ```
pub const REMOVE_FROM_GROUP: FunctionSig = FunctionSig {
    name: "ABrickEditor::RemoveObjectFromGroup",
    // The weld group read (its offset at +0x15), `EditorParams` (+0x1B) and its group (+0x24),
    // then the same three written (+0x3C, +0x42, +0x4C), then the operation's flag.
    pattern: "0f b6 d2 4c 89 44 24 ?? 85 d2 74 ?? 83 fa 01 75 ?? 41 0f b7 42 ?? eb ?? 49 8b 42 ?? 48 85 c0 74 ?? 0f b7 40 ?? eb ?? 41 8b c3 66 85 c0 74 ?? 85 d2 74 ?? 83 fa 01 75 ?? 66 45 89 5a ?? eb ?? 49 8b 42 ?? 48 85 c0 74 ?? 66 44 89 58 ?? 4c 8b 44 24 ?? 41 81 48 10 80 00 00 00",
};

/// The operation flag of a group change (`1 << 7`, see `brick_editor::PROPERTY_CHANGED`).
pub const GROUPS_CHANGED: u32 = 0x80;

/// Where an editor object keeps its groups.
pub struct BrickGroups {
    /// Offset of the weld group in the object.
    weld: usize,
    /// Offset of the object's `EditorParams` pointer.
    editor_params: usize,
    /// Offset of the editor group in `EditorParams`.
    editor: usize,
}

pub fn find() -> Result<BrickGroups, String> {
    let found = REMOVE_FROM_GROUP.locate()?;
    // SAFETY: displacement bytes inside the matched pattern.
    let byte = |offset| usize::from(unsafe { found.address.add(offset).read() });
    let read = [byte(0x15), byte(0x1B), byte(0x24)];
    let written = [byte(0x3C), byte(0x42), byte(0x4C)];
    if read != written {
        return Err(format!(
            "{} reads groups at {read:x?} but writes them at {written:x?}",
            REMOVE_FROM_GROUP.name
        ));
    }
    Ok(BrickGroups {
        weld: read[0],
        editor_params: read[1],
        editor: read[2],
    })
}

impl BrickGroups {
    /// Where `object` keeps its group of `kind` (0 editor, 1 weld), or null if it has no
    /// `EditorParams` for an editor group.
    ///
    /// # Safety
    /// `object` must be a live editor object (`UBrickEditorObject`).
    unsafe fn word(&self, object: *mut UObjectBase, kind: usize) -> *mut u16 {
        let object = object.cast::<u8>();
        // SAFETY: per the contract; the offsets come from the game's own code, and `EditorParams`
        // is null or points to the object's parameters.
        unsafe {
            if kind == 1 {
                return object.add(self.weld).cast();
            }
            let params = object.add(self.editor_params).cast::<*mut u8>().read();
            if params.is_null() {
                return core::ptr::null_mut();
            }
            params.add(self.editor).cast()
        }
    }

    /// `object`'s groups, by kind (editor, weld).
    ///
    /// # Safety
    /// `object` must be a live editor object. Game thread only.
    pub unsafe fn read(&self, object: *mut UObjectBase) -> [u16; 2] {
        // SAFETY: per the contract.
        [0, 1].map(|kind| unsafe {
            let word = self.word(object, kind);
            if word.is_null() {
                0
            } else {
                word.read_unaligned()
            }
        })
    }

    /// Puts `object` in group `group` of `kind` (0 editor, 1 weld), as the game's own grouping
    /// does; false if it can't be in an editor group (no `EditorParams`).
    ///
    /// # Safety
    /// `object` must be a live editor object, and an editor operation open, which the caller gives
    /// [`GROUPS_CHANGED`]. Game thread only.
    pub unsafe fn write(&self, object: *mut UObjectBase, kind: usize, group: u16) -> bool {
        // SAFETY: per the contract.
        unsafe {
            let word = self.word(object, kind);
            if !word.is_null() {
                word.write_unaligned(group);
            }
            !word.is_null()
        }
    }
}
