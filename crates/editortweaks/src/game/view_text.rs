//! Plain text of ours over the editor's 3D view: UMG `UTextBlock`s on the canvas the editor's widget
//! keeps over the view (`game::view_canvas`). Placed in the screen's space (the canvas's units), so
//! a text stays exactly where it's put while the camera or the mouse moves, and draws over the 3D
//! view. Used by fast editing for the length of its line (`features/brick_resize/line_label.rs`).
//! All through reflection.
//!
//! A text block's look is its fields, which its Slate widget copies when it's made, and adding it
//! to the canvas makes it: so its color, letter size and outline are written to its fields before
//! (`ColorAndOpacity.SpecifiedColor`; `Font.Size`, in points, of the default font, Roboto;
//! `Font.OutlineSettings.OutlineSize` and `.OutlineColor`), as plain data in a new object copied
//! from the class's defaults. Its text goes through `SetText`, its place through its canvas slot
//! (`SetPosition`, `SetAlignment`; `AutoSize` to fit the text).

use brickrust::ue::coreuobject::{EObjectFlags, UObjectBase};
use brickrust::ue::fname::FName;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::game::kismet::{self, Texts};
use crate::game::reflection::{self, Function, Parameters, SingleCall, put, take};
use crate::game::text::Text;
use crate::game::view_canvas::ViewCanvas;
use crate::game::weak_object::WeakObject;

/// Draw order of our texts among the canvas's other children: over the mode widgets, as our
/// borders, under the tooltip.
pub(crate) const Z_ORDER: i32 = 1000;

/// How a text looks.
pub struct Style {
    /// Its letters' color and its outline's (linear, with alpha).
    pub color: [f32; 4],
    pub outline_color: [f32; 4],
    /// Its letters' size (points) and its outline's width (pixels; 0 for none).
    pub size: i32,
    pub outline: i32,
}

/// What texts over the view need from the game (after engine init).
pub struct ViewTexts {
    /// `TextBlock`.
    text_block: *const UClass,
    /// `UTextBlock::SetText(FText InText)`, and where `InText` goes.
    set_text: Function,
    set_text_value: usize,
    texts: Texts,
    /// Offsets in a text block of its `ColorAndOpacity.SpecifiedColor` (`FLinearColor`),
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
unsafe impl Send for ViewTexts {}
unsafe impl Sync for ViewTexts {}

/// A text of ours on the view's canvas, and what it was last given.
pub struct ViewText {
    widget: WeakObject,
    slot: WeakObject,
    shown: Option<String>,
    alignment: Option<[f32; 2]>,
    visible: bool,
}

impl ViewText {
    /// Whether it shows.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Its widget, if it's still there.
    pub fn widget(&self) -> Option<*mut UObjectBase> {
        self.widget.get()
    }
}

/// The offset of the struct field `name` in `owner`, and the struct it holds.
pub(crate) fn struct_field(
    owner: *const UStruct,
    name: &'static str,
) -> Result<(usize, *const UStruct), String> {
    // SAFETY: `owner` is a live, linked struct found through reflection; property records live as
    // long as it.
    unsafe {
        let property = reflection::find_property(owner, name);
        if property.is_null() || !reflection::is_kind(property, "StructProperty") {
            return Err(format!("no struct field '{name}'"));
        }
        Ok((
            (*property).offset_internal as usize,
            reflection::struct_type(property),
        ))
    }
}

/// Finds what [`ViewTexts`] needs.
pub fn find() -> Result<ViewTexts, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let text_block = class("TextBlock")?;
    let slot = class("CanvasPanelSlot")?;
    let set_text = Function::find(text_block, "SetText")?;
    // SAFETY: a live class found through reflection.
    let block = unsafe { &(*text_block).ustruct } as *const UStruct;
    let (color_at, slate_color) = struct_field(block, "ColorAndOpacity")?;
    let (font_at, font) = struct_field(block, "Font")?;
    let (outline_at, outline) = struct_field(font, "OutlineSettings")?;
    let field = |owner, name, kind, size| {
        reflection::struct_field_offset(owner, name, kind, size)
            .map_err(|reason| format!("TextBlock: {reason}"))
    };
    Ok(ViewTexts {
        text_block,
        set_text_value: set_text.parameter("InText", "TextProperty", size_of::<Text>() as i32)?,
        set_text,
        texts: kismet::texts()?,
        color: color_at + field(slate_color, "SpecifiedColor", "StructProperty", 16)?,
        size: font_at + field(font, "Size", "IntProperty", 4)?,
        outline: font_at + outline_at + field(outline, "OutlineSize", "IntProperty", 4)?,
        outline_color: font_at + outline_at + field(outline, "OutlineColor", "StructProperty", 16)?,
        set_alignment: SingleCall::find(slot, "SetAlignment", "InAlignment", "StructProperty", 8)?,
        set_auto_size: SingleCall::find(slot, "SetAutoSize", "InbAutoSize", "BoolProperty", 1)?,
    })
}

impl ViewTexts {
    /// A new, empty text on `panel` (from [`ViewCanvas::panel`]) looking as `style` says, hidden
    /// until shown.
    ///
    /// # Safety
    /// `panel` must be a live canvas panel of an editor widget. Game thread only.
    pub unsafe fn add(
        &self,
        canvas: &ViewCanvas,
        panel: *mut UObjectBase,
        style: &Style,
    ) -> Option<ViewText> {
        // SAFETY: per the contract. The new text block is live and not yet drawn; its fields are
        // plain numbers written at their reflected offsets, inside it.
        unsafe {
            // Made where `UWidgetTree::ConstructWidget` makes widgets: in the user widget's tree
            // (the panel's outer), as the canvas's borders.
            let tree = (*panel).outer_private.cast_mut();
            if tree.is_null() {
                return None;
            }
            let widget = reflection::new_object(
                self.text_block,
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
            let slot = canvas.add_widget(panel, widget, Z_ORDER)?;
            self.set_auto_size.call(slot, true);
            Some(ViewText {
                widget: WeakObject::new(widget),
                slot: WeakObject::new(slot),
                shown: None,
                alignment: None,
                visible: false,
            })
        }
    }

    /// Whether `text` is still there.
    pub fn is_alive(&self, text: &ViewText) -> bool {
        text.widget.get().is_some() && text.slot.get().is_some()
    }

    /// Makes `text` read `string`, if it doesn't already.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn write(&self, text: &mut ViewText, string: &str) {
        if text.shown.as_deref() == Some(string) {
            return;
        }
        let Some(widget) = text.widget.get() else {
            return;
        };
        // SAFETY: per the contract; the widget is a live text block (`WeakObject::get`). The text
        // goes in as is: `SetText` copies it, ProcessEvent doesn't destroy it, so it's released
        // after.
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
        text.shown = Some(string.to_owned());
    }

    /// Shows `text` with the point `alignment` of it (0 to 1 across and down: 0.5, 1 its bottom
    /// middle) at `position` (the panel's units).
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn show(
        &self,
        canvas: &ViewCanvas,
        text: &mut ViewText,
        position: [f32; 2],
        alignment: [f32; 2],
    ) {
        let (Some(widget), Some(slot)) = (text.widget.get(), text.slot.get()) else {
            return;
        };
        // SAFETY: per the contract; both are live (`WeakObject::get`), the slot a canvas slot.
        unsafe {
            if text.alignment != Some(alignment) {
                text.alignment = Some(alignment);
                self.set_alignment.call(slot, alignment);
            }
            canvas.show_widget(widget, slot, position);
        }
        text.visible = true;
    }

    /// Hides `text`.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn hide(&self, canvas: &ViewCanvas, text: &mut ViewText) {
        if !text.visible {
            return;
        }
        text.visible = false;
        if let Some(widget) = text.widget.get() {
            // SAFETY: per the contract; the widget is live (`WeakObject::get`).
            unsafe { canvas.hide_widget(widget) };
        }
    }
}
