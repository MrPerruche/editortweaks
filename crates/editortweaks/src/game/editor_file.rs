//! Which vehicle file the editor has open: `ABrickEditor`'s `FUGCFileInfo` (`GetFileInfo()`, a C++
//! accessor, not reflected). See `docs/re/references.md`.
//!
//! The struct is reflected (its fields' offsets are read at run time), but the editor's field
//! holding it isn't. In BRMK 1.11.6 it sits between two reflected ones:
//!
//! ```text
//! +0x348  APlayerController* PlayerController   (reflected)
//! +0x378  FUGCFileInfo FileInfo                  (GetFileInfo: lea rax, [rcx + 0x378])
//! +0x488  UBrickEditorMode* CurrentEditorMode    (reflected)
//! ```
//!
//! The shipping game's offsets differ (editor-only engine fields), but not the class's own layout:
//! the file info is read at `PlayerController + 0x30`, and only while `CurrentEditorMode` is still
//! `0x140` after `PlayerController`, so a changed layout disables this instead of reading garbage.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UStruct;

use crate::game::reflection;
use crate::game::slider_text::FString;

/// The file info's place after `PlayerController`, and `CurrentEditorMode`'s (BRMK 1.11.6).
const FILE_INFO_AFTER_CONTROLLER: usize = 0x30;
const MODE_AFTER_CONTROLLER: usize = 0x140;

/// What the editor knows about the open file, as far as references need it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileInfo {
    /// `FileType`, `UGCType`: which kind of file (enums, as numbers).
    pub file_type: u8,
    pub ugc_type: u8,
    /// `LocalItemId`: the item's id on this computer (empty for a vehicle not saved yet).
    pub local_id: String,
    pub title: String,
}

/// Reads the editor's file info (after engine init).
pub struct EditorFile {
    /// Offset of the file info in the editor.
    offset: usize,
    file_type: usize,
    ugc_type: usize,
    local_id: usize,
    title: usize,
}

// SAFETY: offsets living as long as the game; only used on the game thread.
unsafe impl Send for EditorFile {}
unsafe impl Sync for EditorFile {}

impl EditorFile {
    pub fn find() -> Result<Self, String> {
        let editor = reflection::find_class("BrickEditor").ok_or("class BrickEditor not found")?;
        let controller = reflection::field_offset(editor, "PlayerController", "ObjectProperty", 8)?;
        let mode = reflection::field_offset(editor, "CurrentEditorMode", "ObjectProperty", 8)?;
        if mode.checked_sub(controller) != Some(MODE_AFTER_CONTROLLER) {
            return Err(format!(
                "ABrickEditor's layout changed: CurrentEditorMode at {mode:#x}, PlayerController at {controller:#x}"
            ));
        }
        let info: *const UStruct = reflection::find_object("UGCFileInfo", "ScriptStruct")
            .ok_or("struct UGCFileInfo not found")?
            .cast_const()
            .cast();
        let offset = controller + FILE_INFO_AFTER_CONTROLLER;
        // SAFETY: a live struct record.
        let size = unsafe { reflection::struct_size(info) };
        if offset + size > mode {
            return Err(format!(
                "FUGCFileInfo ({size:#x} bytes) doesn't fit before CurrentEditorMode"
            ));
        }
        let field = |name, kind, size| reflection::struct_field_offset(info, name, kind, size);
        Ok(Self {
            offset,
            file_type: field("FileType", "EnumProperty", 1)?,
            ugc_type: field("UGCType", "EnumProperty", 1)?,
            local_id: field("LocalItemId", "StrProperty", size_of::<FString>() as i32)?,
            title: field("Title", "StrProperty", size_of::<FString>() as i32)?,
        })
    }

    /// The file info of `editor`, copied.
    ///
    /// # Safety
    /// `editor` must be the live `ABrickEditor`. Game thread only.
    pub unsafe fn read(&self, editor: *const UObjectBase) -> FileInfo {
        // SAFETY: per the contract; the struct's place was checked in `find`, its fields' offsets
        // come from reflection, and its strings are the editor's own (read, not freed).
        unsafe {
            let base = editor.cast::<u8>().add(self.offset);
            let string = |offset: usize| (*base.add(offset).cast::<FString>()).to_string();
            FileInfo {
                file_type: base.add(self.file_type).read(),
                ugc_type: base.add(self.ugc_type).read(),
                local_id: string(self.local_id),
                title: string(self.title),
            }
        }
    }
}
