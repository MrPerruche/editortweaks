//! The editor's side panel (`UBrickEditorWidget::PropertiesPanel`, a `UPropertiesPanelWidget`): the
//! selection's properties, or with nothing selected the editor's settings, rows of ours among them
//! (`features::settings_list`). [`EditorPanel::refresh`] has it list them again after a value one of
//! our rows shows changed outside the panel: the panel's reflected `UpdateProperties()` runs every
//! listed object's reflection again (our rows' hook included) and updates the rows from it (BRMK
//! 1.11.6: 0x3FAC30, which is `UpdateProperties(nullptr)`; see `docs/re/map-in-editor.md`). All
//! through reflection.
//!
//! Nothing the editor can reach keeps its widget in a reflected field, so the widget is found among
//! all objects by its editor (`GetBrickEditor`): keep the panel found.

use brickrust::ue::coreuobject::{EObjectFlags, GObjects, UObjectBase};
use brickrust::ue::uclass::{UFunction, UStruct};

use crate::game::reflection::{self, Getter, Parameters};

pub struct EditorPanel {
    /// `BrickEditorWidget`.
    widget_class: *const UStruct,
    /// `ABrickEditor* UBrickEditorWidget::GetBrickEditor()`.
    editor_of: Getter<*mut UObjectBase>,
    /// Offset of `UBrickEditorWidget::PropertiesPanel`.
    panel: usize,
    /// `UPropertiesPanelWidget::UpdateProperties()`.
    update: *const UFunction,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for EditorPanel {}
unsafe impl Sync for EditorPanel {}

/// Finds the widget's and the panel's reflection. After engine init.
pub fn find() -> Result<EditorPanel, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let widget = class("BrickEditorWidget")?;
    let update = reflection::find_function(class("PropertiesPanelWidget")?, "UpdateProperties")?;
    // SAFETY: a live function record.
    if unsafe { reflection::properties(update.cast()).next().is_some() } {
        return Err("UpdateProperties: unexpected parameters".into());
    }
    Ok(EditorPanel {
        widget_class: widget.cast(),
        editor_of: Getter::find(widget, "GetBrickEditor", "ObjectProperty")?,
        panel: reflection::field_offset(widget, "PropertiesPanel", "ObjectProperty", 8)?,
        update,
    })
}

impl EditorPanel {
    /// The side panel of `editor`'s widget, if it has one. Looks through every object: keep the
    /// result (weakly).
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    pub unsafe fn of(&self, editor: *mut UObjectBase) -> Option<*mut UObjectBase> {
        let templates = EObjectFlags::RF_ClassDefaultObject.union(EObjectFlags::RF_ArchetypeObject);
        // SAFETY: per the contract; on the game thread, GObjects slots hold live objects or null,
        // and the panel is read from a live editor widget at its reflected offset.
        unsafe {
            GObjects().iter().find_map(|object| {
                if object.is_null()
                    || (*object).object_flags.intersects(templates)
                    || !reflection::is_a(object, self.widget_class)
                    || self.editor_of.call(object) != editor
                {
                    return None;
                }
                let panel = object
                    .cast::<u8>()
                    .add(self.panel)
                    .cast::<*mut UObjectBase>()
                    .read();
                (!panel.is_null()).then_some(panel)
            })
        }
    }

    /// Has `panel` (from [`Self::of`]) list its objects' properties again and update its rows.
    ///
    /// # Safety
    /// `panel` must be a live properties panel. Game thread only, and not while one of our rows'
    /// functions holds a lock they take (they run during the update).
    pub unsafe fn refresh(&self, panel: *mut UObjectBase) {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the function has no parameters (checked in `find`).
        unsafe { reflection::call(panel, self.update, parameters.as_mut_ptr()) };
    }
}
