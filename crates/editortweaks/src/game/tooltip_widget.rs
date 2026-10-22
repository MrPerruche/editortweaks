//! The game's own tooltip (`WBP_Tooltip`, a `UTooltipWidget`): a title over a text, in the game's
//! colors, with an arrow at the corner pointing at what it's about. Made, filled and placed as the
//! game's window manager does for the widgets it shows tooltips of, all through reflection.
//!
//! `UWindowManagerWidget::ShowTooltip` (BRMK 0x4520A0) makes the tooltip class's widget (from the
//! game's widget pool), adds it to its canvas (`AddChildToCanvas`, Z order 1001, `AutoSize`) and
//! places it beside the hovered widget: on the side of the screen's middle away from it, so the
//! tooltip grows toward the middle, `TooltipOffset` (reflected) away from its edge, kept
//! `TooltipSafetyMargin` (reflected) from the screen's side; the slot's alignment is that side (0 or
//! 1 per axis), which `UpdateTooltipAlignment(FVector2D Alignment)` (a Blueprint event) turns the
//! arrow to. A new content (`NativeUpdateTooltipContent`, 0x441CF0) fades it in (`FadeIn`, a
//! Blueprint event), then goes to `UpdateTooltipContent(const FTooltipContent& InContent)` (a
//! Blueprint event setting the texts and colors). [`Tooltips`] does the same around a point (the
//! mouse), fading in only when it appears.

use brickrust::ue::coreuobject::{EObjectFlags, GObjects, UObjectBase};
use brickrust::ue::uclass::UClass;

use crate::game::reflection::{self, Function, Parameters, SingleCall, put, take};
use crate::game::text::Text;
use crate::game::view_canvas::ViewCanvas;
use crate::game::weak_object::WeakObject;
use crate::prelude::*;

/// The window manager's draw order for its tooltip.
const Z_ORDER: i32 = 1001;

/// The window manager's `TooltipOffset` and `TooltipSafetyMargin`, if it can't be found (in
/// UI units).
const FALLBACK_OFFSET: f32 = 8.0;
const FALLBACK_MARGIN: f32 = 8.0;

/// What showing the game's tooltip takes (after engine init, once the game's UI is up).
pub struct Tooltips {
    /// `WBP_Tooltip_C`.
    class: *const UClass,
    /// `static UUserWidget* UWidgetBlueprintLibrary::Create(UObject* WorldContextObject,
    /// TSubclassOf<UUserWidget> WidgetType, APlayerController* OwningPlayer)`.
    library: *mut UObjectBase,
    create: Function,
    create_context: usize,
    create_type: usize,
    create_player: usize,
    create_result: usize,
    /// `UpdateTooltipContent`, its `InContent`, and the offsets of `FTooltipContent`'s
    /// `TitleText`, `ContentText` and `ColorStyle` (`EBrickUIColorStyle`) in it.
    update_content: Function,
    content: usize,
    title: usize,
    text: usize,
    color_style: usize,
    /// `EBrickUIColorStyle::Default`.
    default_style: u8,
    update_alignment: SingleCall,
    fade_in: Function,
    /// `UCanvasPanelSlot::SetAlignment(FVector2D InAlignment)`, `SetAutoSize(bool InbAutoSize)`.
    set_alignment: SingleCall,
    set_auto_size: SingleCall,
    /// The window manager's `TooltipOffset` and `TooltipSafetyMargin` (UI units).
    offset: f32,
    margin: f32,
}

// SAFETY: reflection records, the library's default object and a Blueprint class kept by the game's
// UI, living as long as the game; only used on the game thread.
unsafe impl Send for Tooltips {}
unsafe impl Sync for Tooltips {}

/// A tooltip of ours on the view's canvas.
pub struct Tooltip {
    widget: WeakObject,
    slot: WeakObject,
    /// The alignment it was last placed with, which the arrow points from.
    alignment: Option<[f32; 2]>,
    shown: bool,
}

impl Tooltip {
    /// Its widget, if it's still there.
    pub fn widget(&self) -> Option<*mut UObjectBase> {
        self.widget.get()
    }
}

/// Finds what [`Tooltips`] needs; looks through every object (call once).
pub fn find() -> Result<Tooltips, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let native = class("TooltipWidget")?;
    let tooltip = reflection::find_object("WBP_Tooltip_C", "WidgetBlueprintGeneratedClass")
        .ok_or("WBP_Tooltip not loaded")?
        .cast_const()
        .cast::<UClass>();
    // SAFETY: a live class found through reflection.
    if !unsafe { reflection::with_parents(tooltip.cast()) }.any(|parent| parent == native.cast()) {
        return Err("WBP_Tooltip isn't a TooltipWidget".into());
    }
    let library_class = class("WidgetBlueprintLibrary")?;
    let create = Function::find(library_class, "Create")?;
    // The Blueprint's own events, which `ProcessEvent` runs (the native class's are empty).
    let update_content = Function::find(tooltip, "UpdateTooltipContent")?;
    let content_function = reflection::find_function(tooltip, "UpdateTooltipContent")?;
    // SAFETY: a live function record, its parameters linked to it.
    let content_struct = unsafe {
        let content = reflection::find_property(content_function.cast(), "InContent");
        if content.is_null() || !reflection::is_kind(content, "StructProperty") {
            return Err("UpdateTooltipContent: no InContent".into());
        }
        reflection::struct_type(content)
    };
    let text_size = size_of::<Text>() as i32;
    let field = |name, kind, size| {
        reflection::struct_field_offset(content_struct, name, kind, size)
            .map_err(|reason| format!("TooltipContent: {reason}"))
    };
    let default_style =
        reflection::enum_value("EBrickUIColorStyle", "EBrickUIColorStyle::Default")?;
    let slot = class("CanvasPanelSlot")?;
    let (offset, margin) = window_manager_spacing(class("WindowManagerWidget")?);
    Ok(Tooltips {
        class: tooltip,
        library: reflection::find_object(
            "Default__WidgetBlueprintLibrary",
            "WidgetBlueprintLibrary",
        )
        .ok_or("WidgetBlueprintLibrary's default object not found")?,
        create_context: create.parameter("WorldContextObject", "ObjectProperty", 8)?,
        create_type: create.parameter("WidgetType", "ClassProperty", 8)?,
        create_player: create.parameter("OwningPlayer", "ObjectProperty", 8)?,
        create_result: create.parameter("ReturnValue", "ObjectProperty", 8)?,
        create,
        content: update_content.parameter(
            "InContent",
            "StructProperty",
            // SAFETY: a live struct found through reflection.
            unsafe { reflection::struct_size(content_struct) } as i32,
        )?,
        title: field("TitleText", "TextProperty", text_size)?,
        text: field("ContentText", "TextProperty", text_size)?,
        color_style: field("ColorStyle", "EnumProperty", 1)?,
        default_style: u8::try_from(default_style)
            .map_err(|_| format!("EBrickUIColorStyle::Default is {default_style}"))?,
        update_content,
        update_alignment: SingleCall::find(
            tooltip,
            "UpdateTooltipAlignment",
            "Alignment",
            "StructProperty",
            8,
        )?,
        fade_in: Function::find(tooltip, "FadeIn")?,
        set_alignment: SingleCall::find(slot, "SetAlignment", "InAlignment", "StructProperty", 8)?,
        set_auto_size: SingleCall::find(slot, "SetAutoSize", "InbAutoSize", "BoolProperty", 1)?,
        offset,
        margin,
    })
}

/// The live window manager's `TooltipOffset` and `TooltipSafetyMargin`, or [`FALLBACK_OFFSET`] and
/// [`FALLBACK_MARGIN`] if there's none or they can't be read (logged).
fn window_manager_spacing(class: *const UClass) -> (f32, f32) {
    let fields =
        reflection::field_offset(class, "TooltipOffset", "FloatProperty", 4).and_then(|offset| {
            let margin =
                reflection::field_offset(class, "TooltipSafetyMargin", "FloatProperty", 4)?;
            Ok((offset, margin))
        });
    let templates = EObjectFlags::RF_ClassDefaultObject.union(EObjectFlags::RF_ArchetypeObject);
    // SAFETY: on the game thread, GObjects slots hold live objects or null; the fields are read at
    // their reflected offsets in a live window manager.
    let found = fields.and_then(|(offset, margin)| unsafe {
        let manager = GObjects()
            .iter()
            .find(|&object| {
                !object.is_null()
                    && !(*object).object_flags.intersects(templates)
                    && reflection::is_a(object, class.cast())
            })
            .ok_or("no window manager")?;
        let read = |at: usize| manager.cast::<u8>().add(at).cast::<f32>().read_unaligned();
        Ok((read(offset), read(margin)))
    });
    found.unwrap_or_else(|reason: String| {
        log!("the tooltip is placed as the game's might be: {reason}");
        (FALLBACK_OFFSET, FALLBACK_MARGIN)
    })
}

impl Tooltips {
    /// A new tooltip on `panel` (from [`ViewCanvas::panel`]), for the player `controller`, hidden
    /// until placed.
    ///
    /// # Safety
    /// `panel` must be a live canvas panel of an editor widget, `controller` a live player
    /// controller. Game thread only.
    pub unsafe fn add(
        &self,
        canvas: &ViewCanvas,
        panel: *mut UObjectBase,
        controller: *mut UObjectBase,
    ) -> Option<Tooltip> {
        // SAFETY: per the contract; every call gets its parameters at their reflected offsets.
        unsafe {
            let mut parameters = Parameters::new();
            put(&mut parameters, self.create_context, controller);
            put(&mut parameters, self.create_type, self.class);
            put(&mut parameters, self.create_player, controller);
            self.create.call(self.library, &mut parameters);
            let widget: *mut UObjectBase = take(&parameters, self.create_result);
            if widget.is_null() {
                return None;
            }
            let slot = canvas.add_widget(panel, widget, Z_ORDER)?;
            self.set_auto_size.call(slot, true);
            Some(Tooltip {
                widget: WeakObject::new(widget),
                slot: WeakObject::new(slot),
                alignment: None,
                shown: false,
            })
        }
    }

    /// Whether `tooltip` is still there.
    pub fn is_alive(&self, tooltip: &Tooltip) -> bool {
        tooltip.widget.get().is_some() && tooltip.slot.get().is_some()
    }

    /// Shows `title` over `text` in `tooltip`, giving up the texts' references (the widget keeps
    /// its own).
    ///
    /// # Safety
    /// `title` and `text` must hold live references. Game thread only.
    pub unsafe fn set_content(&self, tooltip: &Tooltip, title: Text, text: Text) {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the content goes in at its reflected offset, its other fields
        // zero (no action shown: `NAME_None`). The texts go in as they are: the event copies them,
        // and ProcessEvent doesn't destroy parameters, so our references come back out to release.
        unsafe {
            let content = self.content;
            put(&mut parameters, content + self.title, title);
            put(&mut parameters, content + self.text, text);
            put(
                &mut parameters,
                content + self.color_style,
                self.default_style,
            );
            if let Some(widget) = tooltip.widget.get() {
                self.update_content.call(widget, &mut parameters);
            }
            take::<Text>(&parameters, content + self.title).release();
            take::<Text>(&parameters, content + self.text).release();
        }
    }

    /// Shows `tooltip` beside `point` as the game places its tooltips beside what they're about,
    /// within `area` (both in the panel's units, `area` its bottom right corner): fading in if it
    /// was hidden.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn place(
        &self,
        canvas: &ViewCanvas,
        tooltip: &mut Tooltip,
        point: [f32; 2],
        area: [f32; 2],
    ) {
        let (Some(widget), Some(slot)) = (tooltip.widget.get(), tooltip.slot.get()) else {
            return;
        };
        let far = [0, 1].map(|axis| point[axis] > area[axis] * 0.5);
        let alignment = far.map(|far| if far { 1.0 } else { 0.0 });
        // As `ShowTooltip`: kept from the screen's sides, not its top and bottom.
        let x = if far[0] {
            (point[0] - self.offset).max(self.margin)
        } else {
            (point[0] + self.offset).min(area[0] - self.margin)
        };
        let y = if far[1] {
            point[1] - self.offset
        } else {
            point[1] + self.offset
        };
        // SAFETY: per the contract; both are live (`WeakObject::get`), the widget a tooltip.
        unsafe {
            if tooltip.alignment != Some(alignment) {
                tooltip.alignment = Some(alignment);
                self.set_alignment.call(slot, alignment);
                self.update_alignment.call(widget, alignment);
            }
            canvas.show_widget(widget, slot, [x, y]);
            if !tooltip.shown {
                tooltip.shown = true;
                self.fade_in.call(widget, &mut Parameters::new());
            }
        }
    }

    /// Hides `tooltip`.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn hide(&self, canvas: &ViewCanvas, tooltip: &mut Tooltip) {
        tooltip.shown = false;
        if let Some(widget) = tooltip.widget.get() {
            // SAFETY: per the contract; the widget is live (`WeakObject::get`).
            unsafe { canvas.hide_widget(widget) };
        }
    }
}
