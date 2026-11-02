//! The row of boxes at the bottom of the vehicle editor: the vehicle's name, size, weight, brick
//! count and price (`UBrickEditorWidget::ItemHeaderWidget`, a `WBP_PagedListHeader`, whose C++
//! class is `UPagedListHeaderWidget`). [`ListHeader::add_field`] puts one more box of the same kind
//! at the start of the row. Found 2026-11-01 in BRMK 1.11.6 (the widgets' assets, exports); see
//! `docs/re/item-header.md`. All through reflection.
//!
//! - The header's root is a grid (`UBrickGridPanel`, a `UGridPanel`) of two columns in the
//!   editor: the name (column 0, `ColumnFill` 0.4) and `PropsBox` (column 1, fill 1), a horizontal
//!   box holding the size, weight, brick count and price, each filling a quarter of it. Its
//!   `PreConstruct` puts `PropsBox` there (`SetColumn(1)`, `SetRow(0)`) because the header is the
//!   editor's (`bIsInEditor`); elsewhere (the vehicle browser) it's under the name, in row 1.
//! - Every box is a `WBP_PagedListHeaderField` (a border around a text block), filled by its
//!   Blueprint function `UpdateItem(bNewVisible, NewText, NewColorStyle, NewTextStyle,
//!   NewBrushStyle)`. The header's events update its own boxes and never touch other children.
//! - The new box goes into a column of its own before the others (`AddChildToGrid`, then
//!   `UGridSlot::SetColumn` on the two columns there, `SetColumnFill` for the widths): nothing is
//!   reordered, and the other boxes keep their widths relative to each other.
//! - The game keeps the row when the player leaves the editor and comes back: a row that already
//!   has the box (the grid's only other child of the boxes' class) gets it back instead.

use brickrust::ue::coreuobject::{EObjectFlags, GObjects, UObjectBase};
use brickrust::ue::tarray::TArray;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::game::kismet::{self, Texts};
use crate::game::reflection::{self, Function, Getter, Parameters, put, take};
use crate::game::text::Text;
use crate::game::ui_style;
use crate::game::weak_object::WeakObject;

/// The new box's width, against one of the row's own boxes (size, weight, ...).
const WIDTH_OF_A_BOX: f32 = 0.5;

/// What adding and filling a box takes (after engine init).
pub struct ListHeader {
    /// `BrickEditorWidget`.
    editor_widget: *const UStruct,
    /// `ABrickEditor* UBrickEditorWidget::GetBrickEditor()`.
    editor_of: Getter<*mut UObjectBase>,
    /// Offset of `UBrickEditorWidget::ItemHeaderWidget`.
    header: usize,
    /// `APlayerController* UUserWidget::GetOwningPlayer()`.
    owning_player: Getter<*mut UObjectBase>,
    /// `static UUserWidget* UWidgetBlueprintLibrary::Create(UObject* WorldContextObject,
    /// TSubclassOf<UUserWidget> WidgetType, APlayerController* OwningPlayer)`, on the library's
    /// default object.
    library: *mut UObjectBase,
    create: Function,
    create_context: usize,
    create_type: usize,
    create_player: usize,
    create_result: usize,
    /// Offsets of `UWidget::Slot`, `UPanelSlot::Parent` and `UPanelSlot::Content`.
    widget_slot: usize,
    slot_parent: usize,
    slot_content: usize,
    /// `GridPanel` and `GridSlot`.
    grid_class: *const UStruct,
    grid_slot_class: *const UStruct,
    /// `UGridSlot* UGridPanel::AddChildToGrid(UWidget* Content, int32 InRow, int32 InColumn)`.
    add_to_grid: Function,
    add_content: usize,
    add_row: usize,
    add_column: usize,
    add_result: usize,
    /// `void UGridPanel::SetColumnFill(int32 ColumnIndex, float Coefficient)`.
    set_fill: Function,
    fill_column: usize,
    fill_value: usize,
    /// Offset of `UGridPanel::ColumnFill` (`TArray<float>`).
    column_fill: usize,
    /// Offsets of `UGridSlot::Row` and `Column`, and `UGridSlot::SetColumn(int32 InColumn)`, which
    /// moves the widget (setting `Column` doesn't).
    row: usize,
    column: usize,
    set_column: reflection::SingleCall,
    /// `EBrickUIColorStyle::Default` and `Negative`, `EBrickUITextStyle::Bold` (as the name),
    /// `EBrickUIBrushStyle::Border` (what the row's own boxes show with).
    default_color: u8,
    negative_color: u8,
    text_style: u8,
    brush_style: u8,
    texts: Texts,
}

// SAFETY: reflection records and the library's default object, living as long as the game; only
// used on the game thread.
unsafe impl Send for ListHeader {}
unsafe impl Sync for ListHeader {}

/// The editor's row as [`ListHeader::add_field`] changed it, and the box it added.
pub struct Field {
    widget: WeakObject,
    /// The new box's grid slot, and those of the name's and the other boxes' columns.
    ours: WeakObject,
    name: WeakObject,
    props: WeakObject,
    grid: WeakObject,
    /// The widths of the three columns, as the row is kept.
    fills: [f32; 3],
    /// `UpdateItem` of the box's class.
    update: UpdateItem,
}

/// `WBP_PagedListHeaderField.UpdateItem`, and where its parameters go.
struct UpdateItem {
    function: Function,
    visible: usize,
    text: usize,
    color: usize,
    text_style: usize,
    brush_style: usize,
}

/// Finds what [`ListHeader`] needs. After engine init.
pub fn find() -> Result<ListHeader, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let object = |class, name| reflection::field_offset(class, name, "ObjectProperty", 8);
    let int = |class, name| reflection::field_offset(class, name, "IntProperty", 4);
    let byte = |name: &'static str, entry: &str| -> Result<u8, String> {
        let value = reflection::enum_value(name, entry)?;
        u8::try_from(value).map_err(|_| format!("{entry} is {value}"))
    };
    let editor_widget = class("BrickEditorWidget")?;
    let user_widget = class("UserWidget")?;
    let library_class = class("WidgetBlueprintLibrary")?;
    let create = Function::find(library_class, "Create")?;
    let grid = class("GridPanel")?;
    let grid_slot = class("GridSlot")?;
    let add_to_grid = Function::find(grid, "AddChildToGrid")?;
    let set_fill = Function::find(grid, "SetColumnFill")?;
    let colors = ui_style::color_style_values()?;
    let panel_slot = class("PanelSlot")?;
    Ok(ListHeader {
        editor_widget: editor_widget.cast(),
        editor_of: Getter::find(editor_widget, "GetBrickEditor", "ObjectProperty")?,
        header: object(editor_widget, "ItemHeaderWidget")?,
        owning_player: Getter::find(user_widget, "GetOwningPlayer", "ObjectProperty")?,
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
        widget_slot: object(class("Widget")?, "Slot")?,
        slot_parent: object(panel_slot, "Parent")?,
        slot_content: object(panel_slot, "Content")?,
        grid_class: grid.cast(),
        grid_slot_class: grid_slot.cast(),
        add_content: add_to_grid.parameter("Content", "ObjectProperty", 8)?,
        add_row: add_to_grid.parameter("InRow", "IntProperty", 4)?,
        add_column: add_to_grid.parameter("InColumn", "IntProperty", 4)?,
        add_result: add_to_grid.parameter("ReturnValue", "ObjectProperty", 8)?,
        add_to_grid,
        fill_column: set_fill.parameter("ColumnIndex", "IntProperty", 4)?,
        fill_value: set_fill.parameter("Coefficient", "FloatProperty", 4)?,
        set_fill,
        column_fill: reflection::field_offset(grid, "ColumnFill", "ArrayProperty", 0x10)?,
        row: int(grid_slot, "Row")?,
        column: int(grid_slot, "Column")?,
        set_column: reflection::SingleCall::find(
            grid_slot,
            "SetColumn",
            "InColumn",
            "IntProperty",
            4,
        )?,
        default_color: colors.default,
        negative_color: colors.negative,
        text_style: byte("EBrickUITextStyle", "EBrickUITextStyle::Bold")?,
        brush_style: byte("EBrickUIBrushStyle", "EBrickUIBrushStyle::Border")?,
        texts: kismet::texts()?,
    })
}

impl ListHeader {
    /// The row of `editor`'s widget, if it has one. Looks through every object: keep the result
    /// (weakly).
    ///
    /// # Safety
    /// `editor` must be the live editor. Game thread only.
    pub unsafe fn of(&self, editor: *mut UObjectBase) -> Option<*mut UObjectBase> {
        let templates = EObjectFlags::RF_ClassDefaultObject.union(EObjectFlags::RF_ArchetypeObject);
        // SAFETY: per the contract; on the game thread, GObjects slots hold live objects or null,
        // and the header is read from a live editor widget at its reflected offset.
        unsafe {
            GObjects().iter().find_map(|object| {
                if object.is_null()
                    || (*object).object_flags.intersects(templates)
                    || !reflection::is_a(object, self.editor_widget)
                    || self.editor_of.call(object) != editor
                {
                    return None;
                }
                object_at(object, self.header)
            })
        }
    }

    /// Adds a box before the row's others, as wide as half of one of them, empty until
    /// [`Self::show`]; or finds the one added before, should the row have been kept since.
    ///
    /// # Safety
    /// `header` must be a live editor row (from [`Self::of`]). Game thread only.
    pub unsafe fn add_field(&self, header: *mut UObjectBase) -> Result<Field, String> {
        // SAFETY: per the contract; each object comes from a reflected object property of the one
        // before, its class checked where it matters, and every call gets its parameters at their
        // reflected offsets.
        unsafe {
            let class = reflection::class_of(header.cast());
            let name_box = variable(header, class, "NameWidget")?;
            let props_box = variable(header, class, "PropsBox")?;
            // The name sits in a horizontal box (with the browser's clear button), in the grid.
            let name_holder = object_at(name_box, self.widget_slot)
                .and_then(|slot| object_at(slot, self.slot_parent))
                .ok_or("the name's box isn't in a panel")?;
            let name_slot = self
                .grid_slot(name_holder)
                .ok_or("the name isn't in a grid")?;
            let props_slot = self
                .grid_slot(props_box)
                .ok_or("PropsBox isn't in a grid")?;
            let grid = object_at(props_slot, self.slot_parent).ok_or("PropsBox has no parent")?;
            if !reflection::is_a(grid, self.grid_class)
                || object_at(name_slot, self.slot_parent) != Some(grid)
            {
                return Err("the name and PropsBox aren't in one grid".into());
            }
            let field_class = reflection::class_of(name_box.cast()).cast::<UClass>();
            let update = UpdateItem::find(field_class)?;
            // Ours from before: the grid's other child of the boxes' class.
            let before = panel_slots(grid)?.into_iter().find_map(|slot| {
                let content = object_at(slot, self.slot_content)?;
                (slot != name_slot
                    && slot != props_slot
                    && reflection::class_of(content.cast()).cast::<UClass>() == field_class)
                    .then_some((content, slot))
            });
            // Where the game puts them; one column further once ours is in.
            let (name_place, props_place) = (self.place(name_slot), self.place(props_slot));
            let shift = i32::from(before.is_some() && name_place.1 == 1);
            if name_place != (0, shift) || props_place != (0, shift + 1) {
                return Err(format!(
                    "unexpected layout: the name at {name_place:?}, PropsBox at {props_place:?} \
                     (row, column)"
                ));
            }
            let boxes = panel_slots(props_box)?.len();
            if boxes == 0 {
                return Err("PropsBox is empty".into());
            }
            // The widths: ours first once they were set for it (whatever moved since).
            let fills = self.fills(grid);
            let first = usize::from(before.is_some() && fills.len() >= 3);
            let fill = |column: usize| fills.get(column).copied().unwrap_or(1.0);
            let props_fill = fill(first + 1);
            let ours_fill = props_fill * WIDTH_OF_A_BOX / boxes as f32;
            let fills = [ours_fill, fill(first), props_fill];
            if let Some((widget, ours)) = before {
                let field = Field {
                    widget: WeakObject::new(widget),
                    ours: WeakObject::new(ours),
                    name: WeakObject::new(name_slot),
                    props: WeakObject::new(props_slot),
                    grid: WeakObject::new(grid),
                    fills,
                    update,
                };
                self.keep_layout(&field);
                return Ok(field);
            }

            // A box of the row's own kind.
            let player = self.owning_player.call(header);
            if player.is_null() {
                return Err("the row has no owning player".into());
            }
            let mut parameters = Parameters::new();
            put(&mut parameters, self.create_context, player);
            put(&mut parameters, self.create_type, field_class);
            put(&mut parameters, self.create_player, player);
            self.create.call(self.library, &mut parameters);
            let widget: *mut UObjectBase = take(&parameters, self.create_result);
            if widget.is_null() {
                return Err("the game made no box".into());
            }

            let mut parameters = Parameters::new();
            put(&mut parameters, self.add_content, widget);
            put(&mut parameters, self.add_row, 0i32);
            put(&mut parameters, self.add_column, 0i32);
            self.add_to_grid.call(grid, &mut parameters);
            let ours: *mut UObjectBase = take(&parameters, self.add_result);
            if ours.is_null() {
                return Err("the grid took no box".into());
            }
            let field = Field {
                widget: WeakObject::new(widget),
                ours: WeakObject::new(ours),
                name: WeakObject::new(name_slot),
                props: WeakObject::new(props_slot),
                grid: WeakObject::new(grid),
                fills,
                update,
            };
            self.keep_layout(&field);
            Ok(field)
        }
    }

    /// Whether `field`'s box is still there (its editor's widget not gone).
    pub fn is_alive(&self, field: &Field) -> bool {
        [
            field.widget,
            field.ours,
            field.name,
            field.props,
            field.grid,
        ]
        .iter()
        .all(|object| object.get_live().is_some())
    }

    /// Puts the row's columns back where [`Self::add_field`] put them, should the header have laid
    /// itself out again (its `PreConstruct` puts `PropsBox` in column 1). Only calls the game when
    /// something moved: cheap enough for each tick.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn keep_layout(&self, field: &Field) {
        let (Some(grid), Some(ours), Some(name), Some(props)) = (
            field.grid.get_live(),
            field.ours.get_live(),
            field.name.get_live(),
            field.props.get_live(),
        ) else {
            return;
        };
        // SAFETY: per the contract; live grid slots of a live grid, at their reflected offsets.
        unsafe {
            for (slot, column) in [(ours, 0), (name, 1), (props, 2)] {
                if self.place(slot).1 != column {
                    self.set_column.call(slot, column);
                }
            }
            if self.fills(grid).get(..3) != Some(&field.fills[..]) {
                for (column, fill) in field.fills.iter().enumerate() {
                    let mut parameters = Parameters::new();
                    put(&mut parameters, self.fill_column, column as i32);
                    put(&mut parameters, self.fill_value, *fill);
                    self.set_fill.call(grid, &mut parameters);
                }
            }
        }
    }

    /// Shows `text` in `field`'s box, red (the UI style's `Negative`) if `negative`, else in the
    /// row's own colours.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn show(&self, field: &Field, text: &str, negative: bool) {
        let Some(widget) = field.widget.get_live() else {
            return;
        };
        let update = &field.update;
        let color = if negative {
            self.negative_color
        } else {
            self.default_color
        };
        let mut parameters = Parameters::new();
        // SAFETY: on the game thread; the box is live and of the class `update` was found on, its
        // parameters go to their reflected offsets. The text goes in as is: the Blueprint copies
        // it into its text block, and ProcessEvent doesn't destroy parameters, so it's released
        // after.
        unsafe {
            put(&mut parameters, update.visible, true);
            put(&mut parameters, update.text, self.texts.text(text));
            put(&mut parameters, update.color, color);
            put(&mut parameters, update.text_style, self.text_style);
            put(&mut parameters, update.brush_style, self.brush_style);
            update.function.call(widget, &mut parameters);
            take::<Text>(&parameters, update.text).release();
        }
    }

    /// `slot`'s row and column.
    ///
    /// # Safety
    /// `slot` must be a live grid slot.
    unsafe fn place(&self, slot: *mut UObjectBase) -> (i32, i32) {
        // SAFETY: per the contract; both are `int32` fields at their reflected offsets.
        unsafe {
            let field = |offset| slot.cast::<u8>().add(offset).cast::<i32>().read();
            (field(self.row), field(self.column))
        }
    }

    /// `widget`'s grid slot, if it's in a grid.
    ///
    /// # Safety
    /// `widget` must be a live widget.
    unsafe fn grid_slot(&self, widget: *mut UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; `Slot` is an object pointer of widgets, checked to be a grid
        // slot.
        unsafe {
            object_at(widget, self.widget_slot)
                .filter(|&slot| reflection::is_a(slot, self.grid_slot_class))
        }
    }

    /// `grid`'s `ColumnFill`.
    ///
    /// # Safety
    /// `grid` must be a live grid panel.
    unsafe fn fills(&self, grid: *mut UObjectBase) -> Vec<f32> {
        // SAFETY: per the contract; the field is a `TArray<float>` at its reflected offset.
        unsafe {
            let fills = &*grid
                .cast::<u8>()
                .add(self.column_fill)
                .cast::<TArray<f32>>();
            if fills.data.is_null() || fills.num <= 0 {
                return Vec::new();
            }
            core::slice::from_raw_parts(fills.data, fills.num as usize).to_vec()
        }
    }
}

impl UpdateItem {
    fn find(class: *const UClass) -> Result<Self, String> {
        let function = Function::find(class, "UpdateItem")?;
        let style = |name| {
            function
                .parameter(name, "EnumProperty", 1)
                .or_else(|_| function.parameter(name, "ByteProperty", 1))
        };
        Ok(Self {
            visible: function.parameter("bNewVisible", "BoolProperty", 1)?,
            text: function.parameter("NewText", "TextProperty", size_of::<Text>() as i32)?,
            color: style("NewColorStyle")?,
            text_style: style("NewTextStyle")?,
            brush_style: style("NewBrushStyle")?,
            function,
        })
    }
}

/// The widget variable `name` of `object` (a widget Blueprint's), whose class is `class`.
///
/// # Safety
/// `object` must be a live object of `class`.
unsafe fn variable(
    object: *mut UObjectBase,
    class: *const UStruct,
    name: &'static str,
) -> Result<*mut UObjectBase, String> {
    // SAFETY: per the contract; the property is the class's, checked to be an object pointer.
    unsafe {
        let property = reflection::find_property(class, name);
        if property.is_null() || !reflection::is_kind(property, "ObjectProperty") {
            return Err(format!("variable {name} not found"));
        }
        object_at(object, (*property).offset_internal as usize).ok_or(format!("{name} is none"))
    }
}

/// `panel`'s children's slots: its `UPanelWidget::Slots` (`TArray<UPanelSlot*>`), found by name on
/// its class.
///
/// # Safety
/// `panel` must be a live panel widget.
unsafe fn panel_slots(panel: *mut UObjectBase) -> Result<Vec<*mut UObjectBase>, String> {
    // SAFETY: per the contract; the property is checked to be an array.
    unsafe {
        let property = reflection::find_property(reflection::class_of(panel.cast()), "Slots");
        if property.is_null() || !reflection::is_kind(property, "ArrayProperty") {
            return Err("a panel without Slots".into());
        }
        let slots = &*panel
            .cast::<u8>()
            .add((*property).offset_internal as usize)
            .cast::<TArray<*mut UObjectBase>>();
        if slots.data.is_null() || slots.num <= 0 {
            return Ok(Vec::new());
        }
        Ok(core::slice::from_raw_parts(slots.data, slots.num as usize)
            .iter()
            .copied()
            .filter(|slot| !slot.is_null())
            .collect())
    }
}

/// The object `base` points to at `offset` (an object property), if any.
///
/// # Safety
/// `base` must be a live object with an object property at `offset`.
unsafe fn object_at(base: *mut UObjectBase, offset: usize) -> Option<*mut UObjectBase> {
    // SAFETY: per the contract.
    let object = unsafe {
        base.cast::<u8>()
            .add(offset)
            .cast::<*mut UObjectBase>()
            .read()
    };
    (!object.is_null()).then_some(object)
}
