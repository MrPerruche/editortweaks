//! A see-through copy of a dragged vehicle browser row's thumbnail that follows the cursor (see
//! `features::vehicle_menu::drag`).
//!
//! It goes where the game puts its tooltips, over every menu: the window manager's
//! `MainCanvasPanel` (a reflected `UWindowManagerWidget` property; `ShowTooltip`, BRMK 0x4520A0,
//! adds the tooltip there with `AddChildToCanvas`, Z order 1001, `AutoSize`). The copy is a new
//! `UOverlay` holding a copy of each image the row's thumbnail overlay shows (`WBP_UGCItem`'s
//! `Image`, its autosave icon, and ours from `game::ugc_decor`: the accent area, the 2x2 grid and
//! the corner icon), with the same slot padding and alignment, brush (`UImage::SetBrush`), color
//! style, tint, render opacity and visibility. A vehicle thumbnail isn't in the brush: a
//! `UBrickImage` keeps the one it loaded apart (not reflected) and draws its brush only without
//! one, so the brush is whatever the image showed before (the "No Image" placeholder, or a pooled
//! row's folder icon). The caller asks the copies for their thumbnails again ([`Ghosts::make`]'s
//! `thumbnail`). It doesn't take the mouse (`HitTestInvisible`, for its children too).
//!
//! The cursor is in desktop pixels, Slate's absolute space: `USlateBlueprintLibrary::
//! AbsoluteToLocal` with the canvas's last geometry (`UWidget::GetCachedGeometry`) places the copy,
//! its slot's alignment the point of the thumbnail where it was grabbed. All reflected
//! (`BlueprintCallable`s and properties of UE 4.27 and the game). Found 2026-11-11 in BRMK 1.11.6.

use brickrust::ue::coreuobject::{EObjectFlags, UObjectBase};
use brickrust::ue::fname::FName;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::game::menu::WindowManager;
use crate::game::reflection::{self, Function, Getter, Parameters, SingleCall, put, take};
use crate::game::ui_style::SetColorStyle;
use crate::game::weak_object::WeakObject;

/// Over the game's tooltip (1001).
const Z_ORDER: i32 = 1002;

/// How see-through the copy is (0: invisible, 1: solid).
const OPACITY: f32 = 0.7;

/// Byte-sized enum fields and parameters (`TEnumAsByte` is a byte property, an `enum class` an
/// enum one).
const BYTE_KINDS: &[&str] = &["ByteProperty", "EnumProperty"];

/// A reflected `FGeometry` read from a widget, then given to a library function.
struct Geometry {
    /// `FGeometry UWidget::GetCachedGeometry() const`, and `FGeometry`'s size.
    get: Function,
    result: usize,
    size: usize,
    /// `USlateBlueprintLibrary`'s default object.
    library: *mut UObjectBase,
    /// `FVector2D AbsoluteToLocal(const FGeometry& Geometry, FVector2D AbsoluteCoordinate)`.
    to_local: Function,
    to_local_geometry: usize,
    to_local_point: usize,
    to_local_result: usize,
    /// `FVector2D GetLocalSize(const FGeometry& Geometry)`.
    local_size: Function,
    local_size_geometry: usize,
    local_size_result: usize,
}

/// Slot fields copied from the row's widgets to ours.
struct SlotFields {
    /// `UOverlaySlot`: its class, `Padding`, `HorizontalAlignment`, `VerticalAlignment`, and their
    /// setters.
    overlay_slot: *const UStruct,
    padding: usize,
    horizontal: usize,
    vertical: usize,
    set_padding: SingleCall,
    set_horizontal: SingleCall,
    set_vertical: SingleCall,
    /// `UUniformGridSlot`: its class, `Row`, `Column`.
    grid_slot: *const UStruct,
    row: usize,
    column: usize,
    /// `UUniformGridPanel::SlotPadding` and `SetSlotPadding(FMargin)`.
    grid_padding: usize,
    set_grid_padding: SingleCall,
}

/// What making and moving the copy takes (after engine init).
pub struct Ghosts {
    windows: WindowManager,
    /// `UWindowManagerWidget::MainCanvasPanel`.
    main_canvas: usize,
    /// `UGCItemWidget::Image`: the row's thumbnail, in the overlay that's copied.
    row_image: usize,
    image: *const UClass,
    brick_image: *const UClass,
    overlay: *const UClass,
    grid: *const UClass,
    /// `UPanelWidget::GetChildrenCount()` and `GetChildAt(int32 Index)`.
    children_count: Getter<i32>,
    child_at: Function,
    child_index: usize,
    child_result: usize,
    /// `UPanelWidget* UWidget::GetParent()`, `UWidget::Slot`.
    get_parent: Getter<*mut UObjectBase>,
    slot: usize,
    /// `UWidget::Visibility`, `SetVisibility`, and the values used.
    visibility: usize,
    set_visibility: SingleCall,
    collapsed: u8,
    hit_test_invisible: u8,
    /// `UWidget::RenderOpacity`, `SetRenderOpacity`.
    opacity: usize,
    set_opacity: SingleCall,
    /// `UImage::Brush` (an `FSlateBrush`) and `SetBrush(const FSlateBrush& InBrush)`.
    brush: usize,
    brush_size: usize,
    set_brush: Function,
    set_brush_value: usize,
    /// `UImage::ColorAndOpacity` and `SetColorAndOpacity(FLinearColor)`.
    tint: usize,
    set_tint: SingleCall,
    /// `UBrickImage::ColorStyle` and `SetColorStyle`.
    color_style: usize,
    set_color_style: SetColorStyle,
    slots: SlotFields,
    /// `UOverlay::AddChildToOverlay(UWidget* Content)`.
    add_to_overlay: Function,
    overlay_content: usize,
    overlay_result: usize,
    /// `UUniformGridPanel::AddChildToUniformGrid(UWidget* Content, int32 InRow, int32 InColumn)`.
    add_cell: Function,
    cell_content: usize,
    cell_row: usize,
    cell_column: usize,
    /// `UCanvasPanel::AddChildToCanvas(UWidget* Content)`.
    add_to_canvas: Function,
    canvas_content: usize,
    canvas_result: usize,
    /// `UCanvasPanelSlot::SetPosition`, `SetAlignment`, `SetAutoSize`, `SetZOrder`.
    set_position: SingleCall,
    set_alignment: SingleCall,
    set_auto_size: SingleCall,
    set_z_order: SingleCall,
    /// `UWidget::RemoveFromParent()`.
    remove_from_parent: Function,
    geometry: Geometry,
}

// SAFETY: reflection records and the library's default object live as long as the game; only used
// on the game thread.
unsafe impl Send for Ghosts {}
unsafe impl Sync for Ghosts {}

/// A copy on the window manager's canvas.
pub struct Ghost {
    root: WeakObject,
    slot: WeakObject,
}

impl Ghosts {
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let enum_byte = |name: &'static str, entry: &str| -> Result<u8, String> {
            let value = reflection::enum_value(name, entry)?;
            u8::try_from(value).map_err(|_| format!("{entry} is {value}"))
        };
        let byte_field = |class, name| {
            reflection::field_offset(class, name, "ByteProperty", 1)
                .or_else(|_| reflection::field_offset(class, name, "EnumProperty", 1))
        };
        let widget = class("Widget")?;
        let panel = class("PanelWidget")?;
        let image = class("Image")?;
        let brick_image = class("BrickImage")?;
        let overlay = class("Overlay")?;
        let grid = class("UniformGridPanel")?;
        let canvas = class("CanvasPanel")?;
        let canvas_slot = class("CanvasPanelSlot")?;
        let overlay_slot = class("OverlaySlot")?;
        let grid_slot = class("UniformGridSlot")?;
        let library = class("SlateBlueprintLibrary")?;

        // SAFETY: a live class record; property records live as long as it.
        let (brush, brush_struct) = unsafe {
            let property = reflection::find_property(image.cast(), "Brush");
            let brush_struct = if property.is_null() {
                core::ptr::null()
            } else {
                reflection::struct_type(property)
            };
            if brush_struct.is_null() {
                return Err("Image::Brush isn't a struct field".into());
            }
            ((*property).offset_internal as usize, brush_struct)
        };
        // SAFETY: a live struct record.
        let brush_size = unsafe { reflection::struct_size(brush_struct) };
        let set_brush = Function::find(image, "SetBrush")?;
        let geometry_struct = reflection::find_object("Geometry", "ScriptStruct")
            .ok_or("struct Geometry not found")?;
        // SAFETY: a live struct found through reflection.
        let geometry_size = unsafe { reflection::struct_size(geometry_struct.cast()) };
        let geometry_i32 = i32::try_from(geometry_size).map_err(|_| "Geometry: odd size")?;
        let get_geometry = Function::find(widget, "GetCachedGeometry")?;
        let to_local = Function::find(library, "AbsoluteToLocal")?;
        let local_size = Function::find(library, "GetLocalSize")?;
        let child_at = Function::find(panel, "GetChildAt")?;
        let add_to_overlay = Function::find(overlay, "AddChildToOverlay")?;
        let add_cell = Function::find(grid, "AddChildToUniformGrid")?;
        let add_to_canvas = Function::find(canvas, "AddChildToCanvas")?;
        Ok(Self {
            windows: WindowManager::find()?,
            main_canvas: reflection::field_offset(
                class("WindowManagerWidget")?,
                "MainCanvasPanel",
                "ObjectProperty",
                8,
            )?,
            row_image: reflection::field_offset(
                class("UGCItemWidget")?,
                "Image",
                "ObjectProperty",
                8,
            )?,
            image,
            brick_image,
            overlay,
            grid,
            children_count: Getter::find(panel, "GetChildrenCount", "IntProperty")?,
            child_index: child_at.parameter("Index", "IntProperty", 4)?,
            child_result: child_at.parameter("ReturnValue", "ObjectProperty", 8)?,
            child_at,
            get_parent: Getter::find(widget, "GetParent", "ObjectProperty")?,
            slot: reflection::field_offset(widget, "Slot", "ObjectProperty", 8)?,
            visibility: byte_field(widget, "Visibility")?,
            set_visibility: SingleCall::find_only(widget, "SetVisibility", BYTE_KINDS, 1)?,
            collapsed: enum_byte("ESlateVisibility", "ESlateVisibility::Collapsed")?,
            hit_test_invisible: enum_byte(
                "ESlateVisibility",
                "ESlateVisibility::HitTestInvisible",
            )?,
            opacity: reflection::field_offset(widget, "RenderOpacity", "FloatProperty", 4)?,
            set_opacity: SingleCall::find_only(widget, "SetRenderOpacity", &["FloatProperty"], 4)?,
            brush,
            brush_size,
            set_brush_value: set_brush.parameter(
                "InBrush",
                "StructProperty",
                i32::try_from(brush_size).map_err(|_| "SlateBrush: odd size")?,
            )?,
            set_brush,
            tint: reflection::field_offset(image, "ColorAndOpacity", "StructProperty", 16)?,
            set_tint: SingleCall::find_only(image, "SetColorAndOpacity", &["StructProperty"], 16)?,
            color_style: byte_field(brick_image, "ColorStyle")?,
            set_color_style: SetColorStyle::find(brick_image)?,
            slots: SlotFields {
                overlay_slot: overlay_slot.cast(),
                padding: reflection::field_offset(overlay_slot, "Padding", "StructProperty", 16)?,
                horizontal: byte_field(overlay_slot, "HorizontalAlignment")?,
                vertical: byte_field(overlay_slot, "VerticalAlignment")?,
                set_padding: SingleCall::find_only(
                    overlay_slot,
                    "SetPadding",
                    &["StructProperty"],
                    16,
                )?,
                set_horizontal: SingleCall::find_only(
                    overlay_slot,
                    "SetHorizontalAlignment",
                    BYTE_KINDS,
                    1,
                )?,
                set_vertical: SingleCall::find_only(
                    overlay_slot,
                    "SetVerticalAlignment",
                    BYTE_KINDS,
                    1,
                )?,
                grid_slot: grid_slot.cast(),
                row: reflection::field_offset(grid_slot, "Row", "IntProperty", 4)?,
                column: reflection::field_offset(grid_slot, "Column", "IntProperty", 4)?,
                grid_padding: reflection::field_offset(grid, "SlotPadding", "StructProperty", 16)?,
                set_grid_padding: SingleCall::find_only(
                    grid,
                    "SetSlotPadding",
                    &["StructProperty"],
                    16,
                )?,
            },
            overlay_content: add_to_overlay.parameter("Content", "ObjectProperty", 8)?,
            overlay_result: add_to_overlay.parameter("ReturnValue", "ObjectProperty", 8)?,
            add_to_overlay,
            cell_content: add_cell.parameter("Content", "ObjectProperty", 8)?,
            cell_row: add_cell.parameter("InRow", "IntProperty", 4)?,
            cell_column: add_cell.parameter("InColumn", "IntProperty", 4)?,
            add_cell,
            canvas_content: add_to_canvas.parameter("Content", "ObjectProperty", 8)?,
            canvas_result: add_to_canvas.parameter("ReturnValue", "ObjectProperty", 8)?,
            add_to_canvas,
            set_position: SingleCall::find(
                canvas_slot,
                "SetPosition",
                "InPosition",
                "StructProperty",
                8,
            )?,
            set_alignment: SingleCall::find(
                canvas_slot,
                "SetAlignment",
                "InAlignment",
                "StructProperty",
                8,
            )?,
            set_auto_size: SingleCall::find(
                canvas_slot,
                "SetAutoSize",
                "InbAutoSize",
                "BoolProperty",
                1,
            )?,
            set_z_order: SingleCall::find(canvas_slot, "SetZOrder", "InZOrder", "IntProperty", 4)?,
            remove_from_parent: Function::find(widget, "RemoveFromParent")?,
            geometry: Geometry {
                result: get_geometry.parameter("ReturnValue", "StructProperty", geometry_i32)?,
                size: geometry_size,
                get: get_geometry,
                library: reflection::find_object(
                    "Default__SlateBlueprintLibrary",
                    "SlateBlueprintLibrary",
                )
                .ok_or("SlateBlueprintLibrary's default object not found")?,
                to_local_geometry: to_local.parameter(
                    "Geometry",
                    "StructProperty",
                    geometry_i32,
                )?,
                to_local_point: to_local.parameter("AbsoluteCoordinate", "StructProperty", 8)?,
                to_local_result: to_local.parameter("ReturnValue", "StructProperty", 8)?,
                to_local,
                local_size_geometry: local_size.parameter(
                    "Geometry",
                    "StructProperty",
                    geometry_i32,
                )?,
                local_size_result: local_size.parameter("ReturnValue", "StructProperty", 8)?,
                local_size,
            },
        })
    }

    /// A copy of `row`'s thumbnail, grabbed at `point` (desktop pixels) and shown there; `None` if
    /// the row or the window manager isn't laid out as expected. `thumbnail(source, copy)` is
    /// called for each `UBrickImage` copied, to ask the copy for the thumbnail the source shows.
    ///
    /// # Safety
    /// `row` must be a live vehicle browser row widget (`UUGCItemWidget`). Game thread only.
    pub unsafe fn make(
        &self,
        row: *mut UObjectBase,
        point: (i32, i32),
        thumbnail: &dyn Fn(*mut UObjectBase, *mut UObjectBase),
    ) -> Option<Ghost> {
        // SAFETY: per the contract; every object read comes from a reflected object property or a
        // reflected call (checked non-null, panels checked by class), each call's parameters at
        // their reflected offsets.
        unsafe {
            let canvas = self.canvas()?;
            let row_image = field::<*mut UObjectBase>(row, self.row_image);
            if row_image.is_null() {
                return None;
            }
            let source = self.get_parent.call(row_image);
            if source.is_null() || !reflection::is_a(source, self.overlay.cast()) {
                return None;
            }
            // Made where `UWidgetTree::ConstructWidget` makes widgets: in the window manager's tree
            // (the canvas's outer).
            let tree = (*canvas).outer_private.cast_mut();
            if tree.is_null() {
                return None;
            }
            let root = self.new_widget(self.overlay, tree)?;
            self.copy_children(source, root, tree, thumbnail);
            self.set_visibility.call(root, self.hit_test_invisible);
            self.set_opacity.call(root, OPACITY);
            let mut parameters = Parameters::new();
            put(&mut parameters, self.canvas_content, root);
            self.add_to_canvas.call(canvas, &mut parameters);
            let slot: *mut UObjectBase = take(&parameters, self.canvas_result);
            if slot.is_null() {
                self.remove_from_parent.call(root, &mut Parameters::new());
                return None;
            }
            self.set_z_order.call(slot, Z_ORDER);
            self.set_auto_size.call(slot, true);
            self.set_alignment
                .call(slot, self.grab_point(source, point));
            let ghost = Ghost {
                root: WeakObject::new(root),
                slot: WeakObject::new(slot),
            };
            self.move_to(&ghost, point);
            Some(ghost)
        }
    }

    /// Moves `ghost` to `point` (desktop pixels).
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn move_to(&self, ghost: &Ghost, point: (i32, i32)) {
        // SAFETY: on the game thread; the slot checked live, the canvas from the window manager.
        unsafe {
            let (Some(slot), Some(canvas)) = (ghost.slot.get_live(), self.canvas()) else {
                return;
            };
            let at = self.to_local(canvas, point);
            self.set_position.call(slot, at);
        }
    }

    /// Takes `ghost` off the canvas for good.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn remove(&self, ghost: Ghost) {
        if let Some(root) = ghost.root.get_live() {
            // SAFETY: a live widget of ours; no parameters.
            unsafe { self.remove_from_parent.call(root, &mut Parameters::new()) };
        }
    }

    /// The window manager's canvas, if there's one.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn canvas(&self) -> Option<*mut UObjectBase> {
        // SAFETY: on the game thread; a live window manager, the field a reflected object pointer.
        unsafe {
            let manager = self.windows.manager()?;
            let canvas = field::<*mut UObjectBase>(manager, self.main_canvas);
            (!canvas.is_null()).then_some(canvas)
        }
    }

    /// Copies the images and grids `source` (a panel) holds into `target` (a new panel of the same
    /// class), with their slots' placement; `thumbnail` as for [`Self::make`].
    ///
    /// # Safety
    /// Both must be live panels, `source` an overlay or uniform grid, `target` made in `tree`. Game
    /// thread only.
    unsafe fn copy_children(
        &self,
        source: *mut UObjectBase,
        target: *mut UObjectBase,
        tree: *mut UObjectBase,
        thumbnail: &dyn Fn(*mut UObjectBase, *mut UObjectBase),
    ) {
        // SAFETY: per the contract; children come from the panel's own calls (checked non-null and
        // by class), each call's parameters at their reflected offsets.
        unsafe {
            let count = self.children_count.call(source);
            for index in 0..count {
                let mut parameters = Parameters::new();
                put(&mut parameters, self.child_index, index);
                self.child_at.call(source, &mut parameters);
                let child: *mut UObjectBase = take(&parameters, self.child_result);
                if child.is_null() || field::<u8>(child, self.visibility) == self.collapsed {
                    continue;
                }
                let copy = if reflection::is_a(child, self.image.cast()) {
                    self.copy_image(child, tree, thumbnail)
                } else if reflection::is_a(child, self.grid.cast()) {
                    self.copy_grid(child, tree, thumbnail)
                } else {
                    None
                };
                let Some(copy) = copy else {
                    continue;
                };
                self.set_visibility
                    .call(copy, field::<u8>(child, self.visibility));
                self.set_opacity
                    .call(copy, field::<f32>(child, self.opacity));
                self.place_like(child, copy, target);
            }
        }
    }

    /// Adds `copy` to `target` placed like `child` is in its panel.
    ///
    /// # Safety
    /// As for [`Self::copy_children`]; `child` a live child of the source panel.
    unsafe fn place_like(
        &self,
        child: *mut UObjectBase,
        copy: *mut UObjectBase,
        target: *mut UObjectBase,
    ) {
        let slots = &self.slots;
        // SAFETY: per the contract; the child's slot checked by class before its fields are read.
        unsafe {
            let slot = field::<*mut UObjectBase>(child, self.slot);
            if slot.is_null() {
                return;
            }
            if reflection::is_a(target, self.overlay.cast()) {
                let mut parameters = Parameters::new();
                put(&mut parameters, self.overlay_content, copy);
                self.add_to_overlay.call(target, &mut parameters);
                let added: *mut UObjectBase = take(&parameters, self.overlay_result);
                if added.is_null() || !reflection::is_a(slot, slots.overlay_slot) {
                    return;
                }
                slots
                    .set_padding
                    .call(added, field::<[f32; 4]>(slot, slots.padding));
                slots
                    .set_horizontal
                    .call(added, field::<u8>(slot, slots.horizontal));
                slots
                    .set_vertical
                    .call(added, field::<u8>(slot, slots.vertical));
            } else if reflection::is_a(slot, slots.grid_slot) {
                let mut parameters = Parameters::new();
                put(&mut parameters, self.cell_content, copy);
                put(
                    &mut parameters,
                    self.cell_row,
                    field::<i32>(slot, slots.row),
                );
                put(
                    &mut parameters,
                    self.cell_column,
                    field::<i32>(slot, slots.column),
                );
                self.add_cell.call(target, &mut parameters);
            }
        }
    }

    /// A copy of the image `source`: its brush, tint and (for the game's images) color style, and
    /// for those `thumbnail` called.
    ///
    /// # Safety
    /// `source` must be a live `UImage`, `tree` a live widget tree. Game thread only.
    unsafe fn copy_image(
        &self,
        source: *mut UObjectBase,
        tree: *mut UObjectBase,
        thumbnail: &dyn Fn(*mut UObjectBase, *mut UObjectBase),
    ) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; fields at their reflected offsets. The brush is copied bit for
        // bit into the parameters: `SetBrush` copies it from there as C++ does (taking its own
        // references), and ProcessEvent doesn't destroy parameters, so the bits never release
        // anything; the source keeps its brush.
        unsafe {
            let brick = reflection::is_a(source, self.brick_image.cast());
            let class = if brick { self.brick_image } else { self.image };
            let copy = self.new_widget(class, tree)?;
            if brick {
                self.set_color_style
                    .call(copy.cast(), field::<u8>(source, self.color_style));
            }
            self.set_tint
                .call(copy, field::<[f32; 4]>(source, self.tint));
            let mut parameters = Parameters::new();
            core::ptr::copy_nonoverlapping(
                source.cast::<u8>().add(self.brush),
                parameters.as_mut_ptr().add(self.set_brush_value),
                self.brush_size,
            );
            self.set_brush.call(copy, &mut parameters);
            if brick {
                thumbnail(source, copy);
            }
            Some(copy)
        }
    }

    /// A copy of the uniform grid `source` and its images (`thumbnail` as for [`Self::make`]).
    ///
    /// # Safety
    /// `source` must be a live `UUniformGridPanel`, `tree` a live widget tree. Game thread only.
    unsafe fn copy_grid(
        &self,
        source: *mut UObjectBase,
        tree: *mut UObjectBase,
        thumbnail: &dyn Fn(*mut UObjectBase, *mut UObjectBase),
    ) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; the padding at its reflected offset.
        unsafe {
            let copy = self.new_widget(self.grid, tree)?;
            self.slots
                .set_grid_padding
                .call(copy, field::<[f32; 4]>(source, self.slots.grid_padding));
            self.copy_children(source, copy, tree, thumbnail);
            Some(copy)
        }
    }

    /// Where `point` (desktop pixels) is in `source` (the row's thumbnail overlay), as a fraction
    /// of its size, kept within it: the copy's alignment, so it's held where it was grabbed.
    ///
    /// # Safety
    /// `source` must be a live widget. Game thread only.
    unsafe fn grab_point(&self, source: *mut UObjectBase, point: (i32, i32)) -> [f32; 2] {
        let geometry = &self.geometry;
        // SAFETY: per the contract; the geometry is plain data, copied from one call's result to
        // the other's parameter at their reflected offsets.
        unsafe {
            let local = self.to_local(source, point);
            let mut got = Parameters::new();
            geometry.get.call(source, &mut got);
            let mut parameters = Parameters::new();
            core::ptr::copy_nonoverlapping(
                got.as_mut_ptr().add(geometry.result),
                parameters.as_mut_ptr().add(geometry.local_size_geometry),
                geometry.size,
            );
            geometry.local_size.call(geometry.library, &mut parameters);
            let size: [f32; 2] = take(&parameters, geometry.local_size_result);
            [0, 1].map(|axis| {
                if size[axis] > 1.0 {
                    (local[axis] / size[axis]).clamp(0.0, 1.0)
                } else {
                    0.5
                }
            })
        }
    }

    /// `point` (desktop pixels) in `widget`'s units, as it was last laid out.
    ///
    /// # Safety
    /// `widget` must be a live widget. Game thread only.
    unsafe fn to_local(&self, widget: *mut UObjectBase, point: (i32, i32)) -> [f32; 2] {
        let geometry = &self.geometry;
        // SAFETY: per the contract; as in `grab_point`.
        unsafe {
            let mut got = Parameters::new();
            geometry.get.call(widget, &mut got);
            let mut parameters = Parameters::new();
            core::ptr::copy_nonoverlapping(
                got.as_mut_ptr().add(geometry.result),
                parameters.as_mut_ptr().add(geometry.to_local_geometry),
                geometry.size,
            );
            put(
                &mut parameters,
                geometry.to_local_point,
                [point.0 as f32, point.1 as f32],
            );
            geometry.to_local.call(geometry.library, &mut parameters);
            take(&parameters, geometry.to_local_result)
        }
    }

    /// A new widget of `class` in `tree`, or `None`.
    ///
    /// # Safety
    /// `class` must be a widget class, `tree` a live widget tree. Game thread only.
    unsafe fn new_widget(
        &self,
        class: *const UClass,
        tree: *mut UObjectBase,
    ) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; an unused name (none).
        let made = unsafe {
            reflection::new_object(class, tree, FName::default(), EObjectFlags::RF_Transient)
        };
        (!made.is_null()).then_some(made)
    }
}

/// Reads the `T` at `offset` in `object`.
///
/// # Safety
/// `object` must be live, with a `T` at `offset`.
unsafe fn field<T: Copy>(object: *mut UObjectBase, offset: usize) -> T {
    // SAFETY: per the contract.
    unsafe { object.cast::<u8>().add(offset).cast::<T>().read_unaligned() }
}
