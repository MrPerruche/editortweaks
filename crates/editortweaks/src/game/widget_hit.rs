//! Widgets and the mouse, for dragging in menus: whether the cursor is over a widget (its last laid
//! out geometry, `UWidget::GetCachedGeometry()`, and `USlateBlueprintLibrary::IsUnderLocation(
//! Geometry, AbsoluteCoordinate)`: Slate's absolute space is desktop pixels, what `GetCursorPos`
//! gives, see `platform::cursor`), the cursor a widget shows (`UWidget::SetCursor(EMouseCursor)` /
//! `ResetCursor()`), and its render opacity (`SetRenderOpacity`). All UE 4.27 `BlueprintCallable`s,
//! so reflected.

use brickrust::ue::coreuobject::UObjectBase;

use crate::game::reflection::{self, BoolFunction, Function, Parameters, SingleCall};

/// What hit testing and the drag look take (after engine init).
pub struct WidgetHits {
    /// `FGeometry UWidget::GetCachedGeometry() const`, and `FGeometry`'s size.
    geometry: Function,
    geometry_result: usize,
    geometry_size: usize,
    /// `USlateBlueprintLibrary`'s default object, and `IsUnderLocation` on it.
    library: *mut UObjectBase,
    is_under: Function,
    under_geometry: usize,
    under_point: usize,
    under_result: usize,
    /// `bool UWidget::IsVisible() const`.
    is_visible: BoolFunction,
    /// `UWidget::SetCursor(EMouseCursor::Type)`, `ResetCursor()`, and the closed grab hand.
    set_cursor: SingleCall,
    reset_cursor: Function,
    grab: u8,
    /// `UWidget::SetRenderOpacity(float)`.
    set_opacity: SingleCall,
}

// SAFETY: reflection records and the library's default object live as long as the game; only used
// on the game thread.
unsafe impl Send for WidgetHits {}
unsafe impl Sync for WidgetHits {}

impl WidgetHits {
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let widget = class("Widget")?;
        let geometry = Function::find(widget, "GetCachedGeometry")?;
        let is_under = Function::find(class("SlateBlueprintLibrary")?, "IsUnderLocation")?;
        let geometry_struct = reflection::find_object("Geometry", "ScriptStruct")
            .ok_or("struct Geometry not found")?;
        // SAFETY: a live struct found through reflection.
        let geometry_size = unsafe { reflection::struct_size(geometry_struct.cast()) };
        let size = i32::try_from(geometry_size).map_err(|_| "Geometry: odd size")?;
        // `EMouseCursor` is a namespaced enum: its entries are spelled with the namespace.
        let grab = reflection::enum_value("EMouseCursor", "EMouseCursor::GrabHandClosed")
            .or_else(|_| reflection::enum_value("EMouseCursor", "GrabHandClosed"))?;
        Ok(Self {
            geometry_result: geometry.parameter("ReturnValue", "StructProperty", size)?,
            geometry_size,
            geometry,
            library: reflection::find_object(
                "Default__SlateBlueprintLibrary",
                "SlateBlueprintLibrary",
            )
            .ok_or("SlateBlueprintLibrary's default object not found")?,
            under_geometry: is_under.parameter("Geometry", "StructProperty", size)?,
            under_point: is_under.parameter("AbsoluteCoordinate", "StructProperty", 8)?,
            under_result: is_under.parameter("ReturnValue", "BoolProperty", 1)?,
            is_under,
            is_visible: BoolFunction::find(widget, "IsVisible")?,
            set_cursor: SingleCall::find_only(
                widget,
                "SetCursor",
                &["ByteProperty", "EnumProperty"],
                1,
            )?,
            reset_cursor: Function::find(widget, "ResetCursor")?,
            grab: u8::try_from(grab).map_err(|_| format!("GrabHandClosed is {grab}"))?,
            set_opacity: SingleCall::find_only(widget, "SetRenderOpacity", &["FloatProperty"], 4)?,
        })
    }

    /// Whether `point` (desktop pixels) is over `widget` as it was last laid out. A widget never
    /// laid out has an empty geometry, so nothing is over it.
    ///
    /// # Safety
    /// `widget` must be a live widget. Game thread only.
    pub unsafe fn contains(&self, widget: *mut UObjectBase, point: (i32, i32)) -> bool {
        let mut got = Parameters::new();
        let mut asked = Parameters::new();
        // SAFETY: per the contract; the geometry (plain values) is copied between the two calls'
        // parameters at their reflected offsets, both within the buffers (checked sizes).
        unsafe {
            self.geometry.call(widget, &mut got);
            core::ptr::copy_nonoverlapping(
                got.as_mut_ptr().add(self.geometry_result),
                asked.as_mut_ptr().add(self.under_geometry),
                self.geometry_size,
            );
            reflection::put(
                &mut asked,
                self.under_point,
                [point.0 as f32, point.1 as f32],
            );
            self.is_under.call(self.library, &mut asked);
            asked.as_mut_ptr().add(self.under_result).read() != 0
        }
    }

    /// Whether `widget` shows (not collapsed or hidden).
    ///
    /// # Safety
    /// `widget` must be a live widget. Game thread only.
    pub unsafe fn is_visible(&self, widget: *mut UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { self.is_visible.call(widget) }
    }

    /// Shows the closed grab hand over `widget` (and over what holds the mouse inside it), or its
    /// own cursor again.
    ///
    /// # Safety
    /// `widget` must be a live widget. Game thread only.
    pub unsafe fn show_grab(&self, widget: *mut UObjectBase, grab: bool) {
        // SAFETY: per the contract; the parameter at its reflected offset.
        unsafe {
            if grab {
                self.set_cursor.call(widget, self.grab);
            } else {
                self.reset_cursor.call(widget, &mut Parameters::new());
            }
        }
    }

    /// # Safety
    /// `widget` must be a live widget. Game thread only.
    pub unsafe fn set_opacity(&self, widget: *mut UObjectBase, opacity: f32) {
        // SAFETY: per the contract.
        unsafe { self.set_opacity.call(widget, opacity) }
    }
}
