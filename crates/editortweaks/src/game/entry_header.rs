//! The vehicle browser's header over its details (the paged list's `SelectedEntryHeaderWidget`, a
//! `WBP_PagedListHeader` like the editor's bottom row: `list_header`), filled for an entry of ours
//! (a folder): its title in the name box, as the game shows a selected vehicle's, texts of ours in
//! the stat boxes under it (size, weight, brick count, price), the rest collapsed, and its clear
//! button (the X) enabled.
//!
//! The game fills the header itself whenever it fills the details (`UPagedListWidget::
//! UpdateSelectedEntryDetails`, BRMK 0x463970: `UPagedListHeaderWidget::SetPagedListEntry`, not
//! reflected), with "Nothing selected" (`Info_NothingSelected`) and every stat box collapsed when no
//! row is selected; ours goes on top after that. The header's events are reflected Blueprint
//! events: `UpdateEntry(FText InTitleText, bool bInHasEntry, bool bInHasUnsavedChanges)` (an entry:
//! the `Highlight` color, bold), `UpdateClearButton(bool bInIsEnabled)` (the X; the list enables it
//! only for its own selection, `WBP_PagedList.UpdateIsEntrySelected`), and each box's `UpdateItem`
//! (`list_header`). The X calls the list's `ClearSelectedEntryWidget()` (`WBP_PagedList`, through
//! the header's `OnClickedClear`). Found 2026-11-11 in BRMK 1.11.6; see `docs/re/item-header.md`.
//!
//! The Blueprint classes (`WBP_PagedListHeader_C`, `WBP_PagedListHeaderField_C`) are only loaded
//! with the menus, after engine init: what they hold is found on the live header the first time.

use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UClass;

use crate::game::kismet::{self, Texts};
use crate::game::list_header::{self, UpdateItem};
use crate::game::reflection::{self, Function, Parameters, put, take};
use crate::game::text::Text;
use crate::game::ui_style;

/// The stat boxes, left to right as `PropsBox` holds them.
const BOXES: [&str; 4] = [
    "DimensionsWidget",
    "MassWidget",
    "ObjectsWidget",
    "PriceWidget",
];

/// What filling the header takes (after engine init).
pub struct EntryHeaders {
    /// `UPagedListWidget::SelectedEntryHeaderWidget`.
    header: usize,
    /// The styles the game's stat boxes show with: `Default` color and text, `Border` brush.
    box_styles: [u8; 3],
    texts: Texts,
    /// The header Blueprint's parts, found on the first header shown (or why not).
    blueprint: OnceLock<Result<Blueprint, String>>,
}

/// The header Blueprint's events and its boxes' class.
struct Blueprint {
    /// `UpdateEntry` and its parameters.
    update_entry: Function,
    title: usize,
    has_entry: usize,
    unsaved: usize,
    /// `UpdateClearButton` and its parameter.
    update_clear: Function,
    clear_enabled: usize,
    /// The boxes' class (`WBP_PagedListHeaderField_C`) and its `UpdateItem`.
    field_class: *const UClass,
    update_item: UpdateItem,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for EntryHeaders {}
unsafe impl Sync for EntryHeaders {}
unsafe impl Send for Blueprint {}
unsafe impl Sync for Blueprint {}

impl EntryHeaders {
    pub fn find() -> Result<Self, String> {
        let byte = |name: &'static str, entry: &str| -> Result<u8, String> {
            let value = reflection::enum_value(name, entry)?;
            u8::try_from(value).map_err(|_| format!("{entry} is {value}"))
        };
        let list =
            reflection::find_class("PagedListWidget").ok_or("class PagedListWidget not found")?;
        Ok(Self {
            header: reflection::field_offset(
                list,
                "SelectedEntryHeaderWidget",
                "ObjectProperty",
                8,
            )?,
            box_styles: [
                ui_style::color_style_values()?.default,
                byte("EBrickUITextStyle", "EBrickUITextStyle::Default")?,
                byte("EBrickUIBrushStyle", "EBrickUIBrushStyle::Border")?,
            ],
            texts: kismet::texts()?,
            blueprint: OnceLock::new(),
        })
    }

    /// The header of `list`, if it has one.
    ///
    /// # Safety
    /// `list` must be the vehicle browser's live paged list. Game thread only.
    unsafe fn header(&self, list: *mut UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; the field is a reflected object pointer.
        let header = unsafe {
            list.cast::<u8>()
                .add(self.header)
                .cast::<*mut UObjectBase>()
                .read()
        };
        (!header.is_null()).then_some(header)
    }

    /// Shows `title` as the header's entry, `stats` in its first stat boxes (the others
    /// collapsed), and its X enabled. Call after the game filled the header (it overwrites it each
    /// time).
    ///
    /// # Safety
    /// `list` must be the vehicle browser's live paged list. Game thread only.
    pub unsafe fn show(
        &self,
        list: *mut UObjectBase,
        title: &str,
        stats: &[String],
    ) -> Result<(), String> {
        // SAFETY: per the contract; the header comes from a reflected object property, its boxes
        // from its Blueprint's variables (checked to be boxes), and every call gets its parameters
        // at their reflected offsets. ProcessEvent doesn't destroy parameters, and the Blueprint
        // copies the title: released after.
        unsafe {
            let header = self.header(list).ok_or("the list has no header")?;
            let class = reflection::class_of(header.cast());
            let blueprint = self.blueprint(header)?;
            let mut parameters = Parameters::new();
            put(&mut parameters, blueprint.title, self.texts.text(title));
            put(&mut parameters, blueprint.has_entry, true);
            put(&mut parameters, blueprint.unsaved, false);
            blueprint.update_entry.call(header, &mut parameters);
            take::<Text>(&parameters, blueprint.title).release();
            for (index, name) in BOXES.iter().enumerate() {
                let field = list_header::variable(header, class, name)?;
                if !reflection::is_a(field, blueprint.field_class.cast()) {
                    return Err(format!("{name} isn't a header box"));
                }
                let text = stats.get(index).map_or("", String::as_str);
                blueprint.update_item.call(
                    field,
                    index < stats.len(),
                    self.texts.text(text),
                    self.box_styles,
                );
            }
            self.enable_clear(header, blueprint, true);
        }
        Ok(())
    }

    /// Disables the header's X, as the game leaves it with nothing of its own selected (after our
    /// folder was unselected).
    ///
    /// # Safety
    /// As for [`Self::show`].
    pub unsafe fn disable_clear(&self, list: *mut UObjectBase) -> Result<(), String> {
        // SAFETY: per the contract.
        unsafe {
            let header = self.header(list).ok_or("the list has no header")?;
            let blueprint = self.blueprint(header)?;
            self.enable_clear(header, blueprint, false);
        }
        Ok(())
    }

    /// Calls the header's `UpdateClearButton(enabled)`.
    ///
    /// # Safety
    /// `header` must be the live header `blueprint` was found on. Game thread only.
    unsafe fn enable_clear(&self, header: *mut UObjectBase, blueprint: &Blueprint, enabled: bool) {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the parameter at its reflected offset.
        unsafe {
            put(&mut parameters, blueprint.clear_enabled, enabled);
            blueprint.update_clear.call(header, &mut parameters);
        }
    }

    /// The header Blueprint's parts, found on `header` the first time.
    ///
    /// # Safety
    /// `header` must be a live header. Game thread only.
    unsafe fn blueprint(&self, header: *mut UObjectBase) -> Result<&Blueprint, String> {
        self.blueprint
            // SAFETY: per the contract.
            .get_or_init(|| unsafe { Blueprint::find(header) })
            .as_ref()
            .map_err(Clone::clone)
    }
}

impl Blueprint {
    /// Finds the events on `header`'s class and the boxes' on its first box's.
    ///
    /// # Safety
    /// `header` must be a live header. Game thread only.
    unsafe fn find(header: *mut UObjectBase) -> Result<Self, String> {
        // SAFETY: per the contract; the box comes from the Blueprint's variable.
        unsafe {
            // A class starts with its struct.
            let class = reflection::class_of(header.cast());
            let update_entry = Function::find(class.cast(), "UpdateEntry")?;
            let update_clear = Function::find(class.cast(), "UpdateClearButton")?;
            let field = list_header::variable(header, class, BOXES[0])?;
            let field_class = reflection::class_of(field.cast()).cast::<UClass>();
            Ok(Self {
                title: update_entry.parameter(
                    "InTitleText",
                    "TextProperty",
                    size_of::<Text>() as i32,
                )?,
                has_entry: update_entry.parameter("bInHasEntry", "BoolProperty", 1)?,
                unsaved: update_entry.parameter("bInHasUnsavedChanges", "BoolProperty", 1)?,
                update_entry,
                clear_enabled: update_clear.parameter("bInIsEnabled", "BoolProperty", 1)?,
                update_clear,
                update_item: UpdateItem::find(field_class)?,
                field_class,
            })
        }
    }
}
