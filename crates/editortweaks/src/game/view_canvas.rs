//! Widgets of our own over the editor's 3D view, on the canvas panel the editor's widget keeps over
//! the view (`WBP_BrickEditor`'s `ViewportCanvasPanel`, a Blueprint variable, between the top and
//! bottom dashboards), drawn with the game's UI, after the 3D view and its post-processing: plain
//! borders (UMG `UBorder`s, whose default brush draws a solid color), or any widget
//! ([`ViewCanvas::add_widget`], e.g. the game's tooltip, `game::tooltip_widget`). All through
//! reflection; found in BRMK 1.11.6's content and game 1.11.7 (build 25831956), see
//! `docs/re/selection.md`.
//!
//! The editor's widget (a `UBrickEditorWidget`) keeps its editor in a weak pointer that isn't
//! reflected, so the live one is found among all objects by its owning player. Borders are made
//! like `UWidgetTree::ConstructWidget` makes them (inside the widget's tree); widgets are added with
//! `UCanvasPanel::AddChildToCanvas`, and don't take the mouse (`HitTestInvisible`).

use brickrust::ue::coreuobject::{EObjectFlags, GObjects, UObjectBase};
use brickrust::ue::fname::FName;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::game::reflection::{
    self, BoolFunction, Function, Getter, Parameters, SingleCall, put, take,
};
use crate::game::weak_object::WeakObject;

/// The editor widget's variable holding the canvas over the 3D view.
const VIEW_PANEL: &str = "ViewportCanvasPanel";

/// Draw order of our borders among the canvas's other children (mode widgets): over them.
const Z_ORDER: i32 = 1000;

/// `UWidget::GetCachedGeometry()`, then `USlateBlueprintLibrary::LocalToViewport(UObject*
/// WorldContextObject, const FGeometry& Geometry, FVector2D LocalCoordinate, FVector2D&
/// PixelPosition, FVector2D& ViewportPosition)`: where a point of a widget is in the viewport.
struct Geometry {
    get: Function,
    result: usize,
    /// `FGeometry`'s size.
    size: usize,
    to_viewport: Function,
    library: *mut UObjectBase,
    context: usize,
    geometry: usize,
    local: usize,
    pixel: usize,
}

/// Makes and places borders over the editor's 3D view (after engine init).
pub struct ViewCanvas {
    /// `BrickEditorWidget`.
    editor_widget: *const UStruct,
    /// `APlayerController* UUserWidget::GetOwningPlayer()`.
    owning_player: Getter<*mut UObjectBase>,
    /// `bool UWidget::IsVisible()`: on screen (false once the widget is removed).
    is_visible: BoolFunction,
    /// `Border`.
    border: *const UClass,
    /// `UCanvasPanelSlot* UCanvasPanel::AddChildToCanvas(UWidget* Content)`.
    add_child: Function,
    add_content: usize,
    add_result: usize,
    /// `UBorder::SetBrushColor(FLinearColor InBrushColor)`.
    set_color: SingleCall,
    /// `UWidget::RemoveFromParent()`.
    remove_from_parent: Function,
    /// `UWidget::SetVisibility(ESlateVisibility InVisibility)`, and the values used.
    set_visibility: SingleCall,
    shown: u8,
    hidden: u8,
    /// `Visible`: shown and taking the mouse (our text boxes).
    clickable: u8,
    /// `UCanvasPanelSlot::SetPosition(FVector2D InPosition)`, `SetSize(FVector2D InSize)`,
    /// `SetZOrder(int32 InZOrder)`.
    set_position: SingleCall,
    set_size: SingleCall,
    set_z_order: SingleCall,
    geometry: Geometry,
}

// SAFETY: reflection records and the library's default object live as long as the game; only used
// on the game thread.
unsafe impl Send for ViewCanvas {}
unsafe impl Sync for ViewCanvas {}

/// A border on the view's canvas: the widget and its slot.
pub struct Border {
    widget: WeakObject,
    slot: WeakObject,
}

/// How the viewport's pixels map to a panel's units: `units = (pixels - origin) / scale`.
#[derive(Clone, Copy, Debug)]
pub struct PanelSpace {
    origin: [f32; 2],
    scale: [f32; 2],
}

impl PanelSpace {
    /// `pixel` (viewport pixels) in the panel's units.
    pub fn units(&self, pixel: [f32; 2]) -> [f32; 2] {
        [0, 1].map(|axis| (pixel[axis] - self.origin[axis]) / self.scale[axis])
    }
}

pub fn find() -> Result<ViewCanvas, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let user_widget = class("UserWidget")?;
    let widget = class("Widget")?;
    let canvas = class("CanvasPanel")?;
    let slot = class("CanvasPanelSlot")?;
    let visibility = |entry: &str| -> Result<u8, String> {
        let value = reflection::enum_value("ESlateVisibility", entry)?;
        u8::try_from(value).map_err(|_| format!("{entry} is {value}"))
    };
    let add_child = Function::find(canvas, "AddChildToCanvas")?;
    let get = Function::find(widget, "GetCachedGeometry")?;
    let to_viewport = Function::find(class("SlateBlueprintLibrary")?, "LocalToViewport")?;
    let geometry_struct =
        reflection::find_object("Geometry", "ScriptStruct").ok_or("struct Geometry not found")?;
    // SAFETY: a live struct found through reflection.
    let size = unsafe { reflection::struct_size(geometry_struct.cast()) };
    let size_i32 = i32::try_from(size).map_err(|_| "Geometry: odd size")?;
    Ok(ViewCanvas {
        editor_widget: class("BrickEditorWidget")?.cast(),
        owning_player: Getter::find(user_widget, "GetOwningPlayer", "ObjectProperty")?,
        is_visible: BoolFunction::find(widget, "IsVisible")?,
        border: class("Border")?,
        add_content: add_child.parameter("Content", "ObjectProperty", 8)?,
        add_result: add_child.parameter("ReturnValue", "ObjectProperty", 8)?,
        add_child,
        set_color: SingleCall::find(
            class("Border")?,
            "SetBrushColor",
            "InBrushColor",
            "StructProperty",
            16,
        )?,
        remove_from_parent: Function::find(widget, "RemoveFromParent")?,
        set_visibility: SingleCall::find(
            widget,
            "SetVisibility",
            "InVisibility",
            "EnumProperty",
            1,
        )?,
        shown: visibility("ESlateVisibility::HitTestInvisible")?,
        hidden: visibility("ESlateVisibility::Collapsed")?,
        clickable: visibility("ESlateVisibility::Visible")?,
        set_position: SingleCall::find(slot, "SetPosition", "InPosition", "StructProperty", 8)?,
        set_size: SingleCall::find(slot, "SetSize", "InSize", "StructProperty", 8)?,
        set_z_order: SingleCall::find(slot, "SetZOrder", "InZOrder", "IntProperty", 4)?,
        geometry: Geometry {
            result: get.parameter("ReturnValue", "StructProperty", size_i32)?,
            size,
            context: to_viewport.parameter("WorldContextObject", "ObjectProperty", 8)?,
            geometry: to_viewport.parameter("Geometry", "StructProperty", size_i32)?,
            local: to_viewport.parameter("LocalCoordinate", "StructProperty", 8)?,
            pixel: to_viewport.parameter("PixelPosition", "StructProperty", 8)?,
            library: reflection::find_object(
                "Default__SlateBlueprintLibrary",
                "SlateBlueprintLibrary",
            )
            .ok_or("SlateBlueprintLibrary's default object not found")?,
            get,
            to_viewport,
        },
    })
}

impl ViewCanvas {
    /// The canvas over the 3D view of the editor `controller` (a player controller) edits with, if
    /// its widget shows. Looks through every object: keep the result.
    ///
    /// # Safety
    /// `controller` must be a live player controller. Game thread only.
    pub unsafe fn panel(&self, controller: *mut UObjectBase) -> Option<*mut UObjectBase> {
        let templates = EObjectFlags::RF_ClassDefaultObject.union(EObjectFlags::RF_ArchetypeObject);
        // SAFETY: on the game thread, GObjects slots hold live objects or null; candidates are
        // live editor widgets.
        unsafe {
            GObjects().iter().find_map(|object| {
                if object.is_null()
                    || (*object).object_flags.intersects(templates)
                    || !reflection::with_parents(reflection::class_of(object.cast()))
                        .any(|class| class == self.editor_widget)
                    || self.owning_player.call(object) != controller
                    || !self.is_visible.call(object)
                {
                    return None;
                }
                let class = reflection::class_of(object.cast());
                let property = reflection::find_property(class, VIEW_PANEL);
                if property.is_null() || !reflection::is_kind(property, "ObjectProperty") {
                    return None;
                }
                let panel = object
                    .cast::<u8>()
                    .add((*property).offset_internal as usize)
                    .cast::<*mut UObjectBase>()
                    .read();
                (!panel.is_null()).then_some(panel)
            })
        }
    }

    /// A new border on `panel` (the canvas from [`Self::panel`]), in `color` (linear, with alpha),
    /// hidden until placed.
    ///
    /// # Safety
    /// `panel` must be a live canvas panel of an editor widget. Game thread only.
    pub unsafe fn add_border(&self, panel: *mut UObjectBase, color: [f32; 4]) -> Option<Border> {
        // SAFETY: per the contract; every call gets its parameters at their reflected offsets.
        unsafe {
            // Made where `UWidgetTree::ConstructWidget` makes widgets: in the user widget's tree
            // (the panel's outer).
            let tree = (*panel).outer_private.cast_mut();
            if tree.is_null() {
                return None;
            }
            let widget = reflection::new_object(
                self.border,
                tree,
                FName::default(),
                EObjectFlags::RF_Transient,
            );
            if widget.is_null() {
                return None;
            }
            self.set_color.call(widget, color);
            let slot = self.add_widget(panel, widget, Z_ORDER)?;
            Some(Border {
                widget: WeakObject::new(widget),
                slot: WeakObject::new(slot),
            })
        }
    }

    /// Adds `widget` to `panel` (the canvas from [`Self::panel`]) at the draw order `z_order`
    /// among its other children, hidden until shown ([`Self::show_widget`]): its slot, `None` if it
    /// couldn't be added.
    ///
    /// # Safety
    /// `panel` must be a live canvas panel of an editor widget, `widget` a live widget not on
    /// another panel. Game thread only.
    pub unsafe fn add_widget(
        &self,
        panel: *mut UObjectBase,
        widget: *mut UObjectBase,
        z_order: i32,
    ) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; every call gets its parameters at their reflected offsets.
        unsafe {
            self.set_visibility.call(widget, self.hidden);
            let mut parameters = Parameters::new();
            put(&mut parameters, self.add_content, widget);
            self.add_child.call(panel, &mut parameters);
            let slot: *mut UObjectBase = take(&parameters, self.add_result);
            if slot.is_null() {
                return None;
            }
            self.set_z_order.call(slot, z_order);
            Some(slot)
        }
    }

    /// Shows `widget` (added with [`Self::add_widget`], `slot` its slot) at `position` (the panel's
    /// units).
    ///
    /// # Safety
    /// `widget` and `slot` must be live. Game thread only.
    pub unsafe fn show_widget(
        &self,
        widget: *mut UObjectBase,
        slot: *mut UObjectBase,
        position: [f32; 2],
    ) {
        // SAFETY: per the contract.
        unsafe {
            self.set_position.call(slot, position);
            self.set_visibility.call(widget, self.shown);
        }
    }

    /// Shows `widget` like [`Self::show_widget`], taking the mouse (a text box being clicked into).
    ///
    /// # Safety
    /// `widget` and `slot` must be live. Game thread only.
    pub unsafe fn show_clickable_widget(
        &self,
        widget: *mut UObjectBase,
        slot: *mut UObjectBase,
        position: [f32; 2],
    ) {
        // SAFETY: per the contract.
        unsafe {
            self.set_position.call(slot, position);
            self.set_visibility.call(widget, self.clickable);
        }
    }

    /// Hides `widget` (added with [`Self::add_widget`]).
    ///
    /// # Safety
    /// `widget` must be live. Game thread only.
    pub unsafe fn hide_widget(&self, widget: *mut UObjectBase) {
        // SAFETY: per the contract.
        unsafe { self.set_visibility.call(widget, self.hidden) };
    }

    /// Takes `widget` off its panel for good (the editor's widget is kept for the next editor, so
    /// a widget of ours left on it would stay).
    ///
    /// # Safety
    /// `widget` must be live. Game thread only.
    pub unsafe fn remove_widget(&self, widget: *mut UObjectBase) {
        // SAFETY: per the contract; no parameters.
        unsafe { self.remove_from_parent.call(widget, &mut Parameters::new()) };
    }

    /// Takes `border` off its panel for good, if it's still there.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn remove(&self, border: &Border) {
        if let Some(widget) = border.widget.get() {
            // SAFETY: per the contract; the widget is live (`WeakObject::get`).
            unsafe { self.remove_widget(widget) };
        }
    }

    /// Whether `panel` (from [`Self::panel`]) is still on screen.
    ///
    /// # Safety
    /// `panel` must be a live widget. Game thread only.
    pub unsafe fn shows(&self, panel: *mut UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { self.is_visible.call(panel) }
    }

    /// Whether `border` is still there.
    pub fn is_alive(&self, border: &Border) -> bool {
        border.widget.get().is_some() && border.slot.get().is_some()
    }

    /// Gives `border` the color `color` (linear, with alpha).
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn set_color(&self, border: &Border, color: [f32; 4]) {
        if let Some(widget) = border.widget.get() {
            // SAFETY: per the contract; the widget is live (`WeakObject::get`).
            unsafe { self.set_color.call(widget, color) };
        }
    }

    /// Shows `border` at `position` with `size` (the panel's units).
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn place(&self, border: &Border, position: [f32; 2], size: [f32; 2]) {
        let (Some(widget), Some(slot)) = (border.widget.get(), border.slot.get()) else {
            return;
        };
        // SAFETY: per the contract; both are live (`WeakObject::get`).
        unsafe {
            self.set_size.call(slot, size);
            self.show_widget(widget, slot, position);
        }
    }

    /// Hides `border`.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn hide(&self, border: &Border) {
        if let Some(widget) = border.widget.get() {
            // SAFETY: per the contract; the widget is live (`WeakObject::get`).
            unsafe { self.hide_widget(widget) };
        }
    }

    /// How the viewport's pixels map to `panel`'s units, as it was last drawn; `None` before it
    /// was.
    ///
    /// # Safety
    /// `panel` must be a live widget, `controller` a live player controller. Game thread only.
    pub unsafe fn space(
        &self,
        panel: *mut UObjectBase,
        controller: *mut UObjectBase,
    ) -> Option<PanelSpace> {
        let geometry = &self.geometry;
        // SAFETY: per the contract; every call gets its parameters at their reflected offsets, the
        // geometry copied as plain data from one function's result to the other's parameter (it
        // has no pointers).
        unsafe {
            let mut got = Parameters::new();
            geometry.get.call(panel, &mut got);
            let source = got.as_mut_ptr().cast_const().add(geometry.result);
            let pixel = |local: [f32; 2]| -> [f32; 2] {
                let mut parameters = Parameters::new();
                put(&mut parameters, geometry.context, controller);
                core::ptr::copy_nonoverlapping(
                    source,
                    parameters.as_mut_ptr().add(geometry.geometry),
                    geometry.size,
                );
                put(&mut parameters, geometry.local, local);
                geometry.to_viewport.call(geometry.library, &mut parameters);
                take(&parameters, geometry.pixel)
            };
            // 100 units, for precision.
            let origin = pixel([0.0, 0.0]);
            let far = pixel([100.0, 100.0]);
            let scale = [0, 1].map(|axis| (far[axis] - origin[axis]) / 100.0);
            (scale.iter().all(|&scale| scale > 1e-3)).then_some(PanelSpace { origin, scale })
        }
    }
}
