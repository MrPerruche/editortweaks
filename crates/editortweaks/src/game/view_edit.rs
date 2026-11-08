//! A text box of ours over the editor's 3D view: a UMG `UEditableText` on the canvas the editor's
//! widget keeps over the view (`game::view_canvas`), placed like our plain texts
//! (`game::view_text`). Used by fast editing to type an exact length
//! (`features/brick_resize/line_label.rs`). All through reflection.
//!
//! The text box is Unreal's own (`SEditableText`), so it does all a text box does: a cursor moved
//! with the arrows, Home and End, the mouse; a selection (Shift, a drag, a double click, Ctrl + A);
//! Ctrl + C, X, V and Z; any character the keyboard types. It gets the keyboard focus when opened
//! (`UWidget::SetKeyboardFocus`) with its text all selected (`SelectAllTextWhenFocused`), so typing
//! replaces it and Left or Right moves into it. Enter commits it: Unreal then takes the focus away
//! (`ClearKeyboardFocusOnCommit`, true by default), which [`ViewEdits::has_focus`] tells
//! (`UWidget::HasKeyboardFocus`); so does a click elsewhere.
//!
//! Its look is its `WidgetStyle` (`FEditableTextStyle`): `Font` and `ColorAndOpacity`, written
//! like a text block's before it's added. The selection's and the cursor's look stay the
//! default style's.

use brickrust::ue::coreuobject::{EObjectFlags, UObjectBase};
use brickrust::ue::fname::FName;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::game::kismet::{self, Texts};
use crate::game::reflection::{
    self, BoolField, BoolFunction, Function, Parameters, SingleCall, put, take,
};
use crate::game::text::Text;
use crate::game::view_canvas::ViewCanvas;
use crate::game::view_text::{self, Style};
use crate::game::weak_object::WeakObject;

/// What text boxes over the view need from the game (after engine init).
pub struct ViewEdits {
    /// `EditableText`.
    editable_text: *const UClass,
    /// `UEditableText::SetText(FText InText)` and `FText GetText()`, and where their texts go.
    set_text: Function,
    set_text_value: usize,
    get_text: Function,
    get_text_result: usize,
    texts: Texts,
    /// `UWidget::SetKeyboardFocus()` and `bool UWidget::HasKeyboardFocus()`.
    set_focus: Function,
    has_focus: BoolFunction,
    /// `SelectAllTextWhenFocused`.
    select_all: BoolField,
    /// Offsets in a text box of its style's `ColorAndOpacity.SpecifiedColor` (`FLinearColor`),
    /// `Font.Size` (`int32`), `Font.OutlineSettings.OutlineSize` (`int32`) and
    /// `Font.OutlineSettings.OutlineColor` (`FLinearColor`).
    color: usize,
    size: usize,
    outline: usize,
    outline_color: usize,
    /// `UCanvasPanelSlot::SetAlignment(FVector2D InAlignment)`, `SetAutoSize(bool InbAutoSize)`.
    set_alignment: SingleCall,
    set_auto_size: SingleCall,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for ViewEdits {}
unsafe impl Sync for ViewEdits {}

/// A text box of ours on the view's canvas.
pub struct ViewEdit {
    widget: WeakObject,
    slot: WeakObject,
    alignment: Option<[f32; 2]>,
    visible: bool,
}

impl ViewEdit {
    /// Its widget, if it's still there.
    pub fn widget(&self) -> Option<*mut UObjectBase> {
        self.widget.get()
    }
}

/// Finds what [`ViewEdits`] needs.
pub fn find() -> Result<ViewEdits, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let editable_text = class("EditableText")?;
    let widget = class("Widget")?;
    let slot = class("CanvasPanelSlot")?;
    let set_text = Function::find(editable_text, "SetText")?;
    let get_text = Function::find(editable_text, "GetText")?;
    let text_size = size_of::<Text>() as i32;
    // SAFETY: a live class found through reflection.
    let edit = unsafe { &(*editable_text).ustruct } as *const UStruct;
    let (style_at, style) = view_text::struct_field(edit, "WidgetStyle")?;
    let (color_at, slate_color) = view_text::struct_field(style, "ColorAndOpacity")?;
    let (font_at, font) = view_text::struct_field(style, "Font")?;
    let (outline_at, outline) = view_text::struct_field(font, "OutlineSettings")?;
    let field = |owner, name, kind, size| {
        reflection::struct_field_offset(owner, name, kind, size)
            .map_err(|reason| format!("EditableText: {reason}"))
    };
    let font_at = style_at + font_at;
    Ok(ViewEdits {
        editable_text,
        set_text_value: set_text.parameter("InText", "TextProperty", text_size)?,
        set_text,
        get_text_result: get_text.parameter("ReturnValue", "TextProperty", text_size)?,
        get_text,
        texts: kismet::texts()?,
        set_focus: Function::find(widget, "SetKeyboardFocus")?,
        has_focus: BoolFunction::find(widget, "HasKeyboardFocus")?,
        select_all: reflection::bool_field(edit, "SelectAllTextWhenFocused")
            .map_err(|reason| format!("EditableText: {reason}"))?,
        color: style_at + color_at + field(slate_color, "SpecifiedColor", "StructProperty", 16)?,
        size: font_at + field(font, "Size", "IntProperty", 4)?,
        outline: font_at + outline_at + field(outline, "OutlineSize", "IntProperty", 4)?,
        outline_color: font_at + outline_at + field(outline, "OutlineColor", "StructProperty", 16)?,
        set_alignment: SingleCall::find(slot, "SetAlignment", "InAlignment", "StructProperty", 8)?,
        set_auto_size: SingleCall::find(slot, "SetAutoSize", "InbAutoSize", "BoolProperty", 1)?,
    })
}

impl ViewEdits {
    /// A new, empty text box on `panel` (from [`ViewCanvas::panel`]) looking as `style` says,
    /// hidden until shown.
    ///
    /// # Safety
    /// `panel` must be a live canvas panel of an editor widget. Game thread only.
    pub unsafe fn add(
        &self,
        canvas: &ViewCanvas,
        panel: *mut UObjectBase,
        style: &Style,
    ) -> Option<ViewEdit> {
        // SAFETY: per the contract. The new text box is live and not yet drawn; its fields are
        // plain data written at their reflected offsets, inside it.
        unsafe {
            // Made in the user widget's tree (the panel's outer), as our texts.
            let tree = (*panel).outer_private.cast_mut();
            if tree.is_null() {
                return None;
            }
            let widget = reflection::new_object(
                self.editable_text,
                tree,
                FName::default(),
                EObjectFlags::RF_Transient,
            );
            if widget.is_null() {
                return None;
            }
            let base = widget.cast::<u8>();
            base.add(self.color)
                .cast::<[f32; 4]>()
                .write_unaligned(style.color);
            base.add(self.size)
                .cast::<i32>()
                .write_unaligned(style.size);
            base.add(self.outline)
                .cast::<i32>()
                .write_unaligned(style.outline);
            base.add(self.outline_color)
                .cast::<[f32; 4]>()
                .write_unaligned(style.outline_color);
            self.select_all.set(base, true);
            let slot = canvas.add_widget(panel, widget, view_text::Z_ORDER)?;
            self.set_auto_size.call(slot, true);
            Some(ViewEdit {
                widget: WeakObject::new(widget),
                slot: WeakObject::new(slot),
                alignment: None,
                visible: false,
            })
        }
    }

    /// Whether `edit` is still there.
    pub fn is_alive(&self, edit: &ViewEdit) -> bool {
        edit.widget.get().is_some() && edit.slot.get().is_some()
    }

    /// Makes `edit` hold `string`.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn write(&self, edit: &ViewEdit, string: &str) {
        let Some(widget) = edit.widget.get() else {
            return;
        };
        // SAFETY: per the contract; the widget is a live text box (`WeakObject::get`). `SetText`
        // copies the text, ProcessEvent doesn't destroy it, so it's released after.
        unsafe {
            let mut parameters = Parameters::new();
            put(
                &mut parameters,
                self.set_text_value,
                self.texts.text(string),
            );
            self.set_text.call(widget, &mut parameters);
            take::<Text>(&parameters, self.set_text_value).release();
        }
    }

    /// What `edit` holds now (empty if it's gone).
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn read(&self, edit: &ViewEdit) -> String {
        let Some(widget) = edit.widget.get() else {
            return String::new();
        };
        // SAFETY: per the contract; the widget is a live text box. The result is a new text
        // holding one reference, ours: `Texts::string` gives it up.
        unsafe {
            let mut parameters = Parameters::new();
            self.get_text.call(widget, &mut parameters);
            self.texts
                .string(take::<Text>(&parameters, self.get_text_result))
        }
    }

    /// Gives `edit` the keyboard focus (which selects all its text). It may take a frame to take
    /// it (see [`Self::has_focus`]).
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn focus(&self, edit: &ViewEdit) {
        if let Some(widget) = edit.widget.get() {
            // SAFETY: per the contract; a live widget, no parameters.
            unsafe { self.set_focus.call(widget, &mut Parameters::new()) };
        }
    }

    /// Whether `edit` has the keyboard focus.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn has_focus(&self, edit: &ViewEdit) -> bool {
        // SAFETY: per the contract; a live widget.
        edit.widget
            .get()
            .is_some_and(|widget| unsafe { self.has_focus.call(widget) })
    }

    /// Shows `edit` with the point `alignment` of it (0 to 1 across and down) at `position` (the
    /// panel's units), taking the mouse (a click places the cursor).
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn show(
        &self,
        canvas: &ViewCanvas,
        edit: &mut ViewEdit,
        position: [f32; 2],
        alignment: [f32; 2],
    ) {
        let (Some(widget), Some(slot)) = (edit.widget.get(), edit.slot.get()) else {
            return;
        };
        // SAFETY: per the contract; both are live (`WeakObject::get`), the slot a canvas slot.
        unsafe {
            if edit.alignment != Some(alignment) {
                edit.alignment = Some(alignment);
                self.set_alignment.call(slot, alignment);
            }
            canvas.show_clickable_widget(widget, slot, position);
        }
        edit.visible = true;
    }

    /// Hides `edit`.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn hide(&self, canvas: &ViewCanvas, edit: &mut ViewEdit) {
        if !edit.visible {
            return;
        }
        edit.visible = false;
        if let Some(widget) = edit.widget.get() {
            // SAFETY: per the contract; the widget is live (`WeakObject::get`).
            unsafe { canvas.hide_widget(widget) };
        }
    }
}
