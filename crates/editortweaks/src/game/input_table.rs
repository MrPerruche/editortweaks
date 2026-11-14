//! The game's input actions, which its input settings list and let players rebind, and adding
//! actions of our own to them. See `docs/re/input-bindings.md`.
//!
//! Every action is a row of an input table: a `UDataTable` of `FInputTableRow`s (reflected) listed
//! in `UBrickAssetManager::InputDataTables`. `UBrickAssetManager::LoadAssetLibraries`
//! ([`LOAD_ASSET_LIBRARIES`]) fills that list with every such table under `/Game/BrickRigs/Input`
//! (the game has one, `InputTable`), and `InputCategories` with the top categories (`UInputCategory`
//! assets without a `ParentCategory`) that rows use. The player's input (`UBrickPlayerInput`, see
//! `game::player_input`) binds each row's `DefaultMappings` under the row's name unless the player
//! rebound it (saved through `UInputSettings` in `Input.ini`), and the input settings list every row
//! of the chosen top category, a section per subcategory (in their `SortPriority` order).
//!
//! Our actions get a table of their own ([`InputTables::new_table`]): each row is made by the row
//! struct's constructor (`InitializeStruct`), filled through reflection and handed to the table
//! (`UDataTable::AddRowInternal`, a plain `RowMap.Add`). Both are virtual functions, whose vtable
//! slots are read from `UDataTable::AddRow` ([`ADD_ROW`]), which calls them the same way.

use core::ffi::c_void;

use brickrust::ue::coreuobject::{EObjectFlags, UObjectBase};
use brickrust::ue::fmalloc;
use brickrust::ue::fname::FName;
use brickrust::ue::tarray::TArray;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::framework::sig::{self, FunctionSig};
use crate::game::menu::SoftPointer;
use crate::game::player_input::Chord;
use crate::game::reflection::{self, BoolField};
use crate::game::text::Text;

/// `void UBrickAssetManager::LoadAssetLibraries(ELoadAssetLibrariesMode)` (BRMK 0x2B9C70, game RVA
/// 0xE2CF70): loads the game's asset lists, the input tables and categories among them, replacing
/// the lists. Called at start (`PostInitialAssetScan`, `UBrickGameInstance::Init`) and at shutdown.
pub const LOAD_ASSET_LIBRARIES: FunctionSig = FunctionSig {
    name: "UBrickAssetManager::LoadAssetLibraries",
    // The path the input categories load from: "Input/Categories" (0x11 characters with its end).
    pattern: "4c 8d 05 ?? ?? ?? ?? 41 b9 11 00 00 00 41 be 3f 00 00 00 41 8b d1 66 44 89 74 24 20 e8",
};

pub type LoadAssetLibrariesFn = unsafe extern "C" fn(manager: *mut UObjectBase, mode: u8);

/// `virtual void UDataTable::AddRow(FName, const FTableRowBase&)` (game RVA 0x251C520, the last of
/// `UDataTable`'s own virtual functions): allocates the row, then
///
/// ```text
/// RowStruct->InitializeStruct(Row, 1);            // virtual, slot read at INITIALIZE_STRUCT_SLOT
/// RowStruct->CopyScriptStruct(Row, &RowData, 1);  // call read at COPY_SCRIPT_STRUCT_CALL
/// this->AddRowInternal(RowName, Row);             // virtual, slot read at ADD_ROW_INTERNAL_SLOT
/// ```
const ADD_ROW: FunctionSig = FunctionSig {
    name: "UDataTable::AddRow",
    pattern: "49 8b 0e 41 b8 01 00 00 00 48 8b d0 48 8b f8 4c 8b 91 ?? ?? ?? ?? 49 8b ce 41 ff d2 41 b9 01 00 00 00 4c 8b c5 48 8b d7 49 8b ce e8 ?? ?? ?? ?? 48 8b 0e 4c 8b c7 48 8b d3 4c 8b 89 ?? ?? ?? ?? 48 8b ce 41 ff d1",
};

/// Where [`ADD_ROW`]'s match holds the vtable offset of `UStruct::InitializeStruct` (0x2B0 in game
/// build 25831956).
const INITIALIZE_STRUCT_SLOT: usize = 18;
/// Where it holds the vtable offset of `UDataTable::AddRowInternal` (0x278).
const ADD_ROW_INTERNAL_SLOT: usize = 60;

/// `virtual void UStruct::InitializeStruct(void* Dest, int32 ArrayDim) const`: constructs `count`
/// structs in a row at `target`.
type InitializeStructFn =
    unsafe extern "C" fn(ustruct: *const UStruct, target: *mut u8, count: i32);

/// `virtual void UDataTable::AddRowInternal(FName RowName, uint8* RowDataPtr)`: the table takes the
/// row (frees it with `FMemory::Free` when emptied). The name is passed by value, as 8 bytes.
type AddRowInternalFn = unsafe extern "C" fn(table: *mut UObjectBase, name: u64, row: *mut u8);

/// Flags of the objects we make: never saved, and never garbage collected.
const OUR_FLAGS: EObjectFlags = EObjectFlags::RF_Transient.union(EObjectFlags::RF_MarkAsRootSet);

/// A `TArray` with elements of a size known only at run time.
#[repr(C)]
struct RawArray {
    data: *mut u8,
    num: i32,
    max: i32,
}

/// An action to add (see [`InputTables::add_row`]).
pub struct Action<'a> {
    /// The row's name: the action's name, which bindings are saved under.
    pub name: FName,
    /// What the input settings call it.
    pub label: &'a Text,
    /// Its description, shown with it.
    pub description: &'a Text,
    /// The keys bound to it by default, with the modifier keys each needs.
    pub keys: &'a [Chord],
    /// Its place among the category's actions (`SortPriority`).
    pub order: i32,
    /// For an action of the mod's editor menu (`features::editor_menu`): the path of its icon
    /// there (a texture's, like the game's actions'). Menu actions are clickable and close the menu
    /// when clicked.
    pub menu_icon: Option<FName>,
}

/// The input tables' reflected layout and the virtual functions that fill them (after engine
/// start; reflection works from the first [`LOAD_ASSET_LIBRARIES`] on).
pub struct InputTables {
    /// Offset of `UBrickAssetManager::InputDataTables` (`TArray<UDataTable*>`).
    tables: usize,
    /// Offset of `UBrickAssetManager::InputCategories` (`TArray<UInputCategory*>`).
    categories: usize,
    table_class: *const UClass,
    /// Offset of `UDataTable::RowStruct`.
    row_struct: usize,
    row: RowLayout,
    category: CategoryLayout,
    /// Offset of `InitializeStruct` in a struct's vtable.
    initialize_struct: usize,
    /// Offset of `AddRowInternal` in a table's vtable.
    add_row_internal: usize,
}

// SAFETY: reflection records and offsets living as long as the game; only used on the game thread.
unsafe impl Send for InputTables {}
unsafe impl Sync for InputTables {}

/// Offsets in an `FInputTableRow`.
struct RowLayout {
    ustruct: *const UStruct,
    /// `PrimaryDisplayInfo.Name` (`FText`).
    label: usize,
    /// `PrimaryDisplayInfo.IconTexture` (`TSoftObjectPtr<UTexture2D>`).
    icon: usize,
    /// `PrimaryActionDescription` (`FText`).
    description: usize,
    /// `Type` (`EInputRowType`, a byte).
    kind: usize,
    /// `EInputRowType::PressReleaseAction`: the action starts when its key is pressed and ends when
    /// it's released, like the game's held keys (Shift's `MovePerpendicular`). `HoldAction` is
    /// something else: a tap does one thing, holding another (two settings rows).
    press_release: u8,
    /// `SortPriority` (`int32`).
    order: usize,
    /// `DefaultMappings` (`TArray<FDefaultInputMapping>`).
    mappings: usize,
    /// `InputCategory` (`UInputCategory*`).
    category: usize,
    can_be_rebound: BoolField,
    hide_from_input_help: BoolField,
    clickable: BoolField,
    close_input_help: BoolField,
    display_action_index: BoolField,
    mapping: MappingLayout,
}

/// An `FDefaultInputMapping`: modifier keys (`bShift`, ...), `Key`, `Scale` (left at 1).
struct MappingLayout {
    ustruct: *const UStruct,
    size: usize,
    /// `Key.KeyName` (`FName`).
    key_name: usize,
    shift: BoolField,
    ctrl: BoolField,
    alt: BoolField,
    cmd: BoolField,
}

/// Offsets in a `UInputCategory`.
struct CategoryLayout {
    class: *const UClass,
    /// `DisplayInfo.Name` (`FText`).
    label: usize,
    /// `DisplayInfo.IconTexture` (`TSoftObjectPtr<UTexture2D>`).
    icon: usize,
    /// `ParentCategory` (`UInputCategory*`).
    parent: usize,
    /// `SortPriority` (`int32`): where a subcategory's section goes among its parent's.
    order: usize,
    show_in_input_help: BoolField,
}

impl InputTables {
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let script_struct = |name| {
            reflection::find_object(name, "ScriptStruct")
                .map(|found| found.cast_const().cast::<UStruct>())
                .ok_or(format!("struct {name} not found"))
        };
        let manager = class("BrickAssetManager")?;
        let table_class = class("DataTable")?;
        let category_class = class("InputCategory")?;
        let row = script_struct("InputTableRow")?;
        let text =
            |ustruct, name| reflection::struct_field_offset(ustruct, name, "TextProperty", 0x18);
        let icon = |ustruct| {
            reflection::struct_field_offset(ustruct, "IconTexture", "SoftObjectProperty", 0x28)
        };
        let bool_field = |name| reflection::bool_field(row, name);

        let (primary, display_info) = nested(row, "PrimaryDisplayInfo")?;
        let mappings = reflection::struct_field(row, "DefaultMappings", "ArrayProperty", 0x10)?;
        // SAFETY: a live ArrayProperty record and its element's.
        let mapping = unsafe { reflection::struct_type(reflection::array_inner(mappings)) };
        if mapping.is_null() {
            return Err("DefaultMappings: not an array of structs".into());
        }
        let (key, key_struct) = nested(mapping, "Key")?;
        let key_name = reflection::struct_field_offset(key_struct, "KeyName", "NameProperty", 8)?;
        let modifier = |name| reflection::bool_field(mapping, name);
        let press_release =
            reflection::enum_value("EInputRowType", "EInputRowType::PressReleaseAction")?;

        let (category_info, category_display_info) = nested(category_class.cast(), "DisplayInfo")?;
        let (initialize_struct, add_row_internal) = slots()?;
        Ok(Self {
            tables: reflection::field_offset(manager, "InputDataTables", "ArrayProperty", 0x10)?,
            categories: reflection::field_offset(
                manager,
                "InputCategories",
                "ArrayProperty",
                0x10,
            )?,
            table_class,
            row_struct: reflection::field_offset(table_class, "RowStruct", "ObjectProperty", 8)?,
            row: RowLayout {
                ustruct: row,
                label: primary + text(display_info, "Name")?,
                icon: primary + icon(display_info)?,
                description: text(row, "PrimaryActionDescription")?,
                kind: reflection::struct_field_offset(row, "Type", "EnumProperty", 1)?,
                press_release: u8::try_from(press_release)
                    .map_err(|_| "EInputRowType::PressReleaseAction: unexpected value")?,
                order: reflection::struct_field_offset(row, "SortPriority", "IntProperty", 4)?,
                // SAFETY: a live property record.
                mappings: unsafe { (*mappings).offset_internal as usize },
                category: reflection::struct_field_offset(
                    row,
                    "InputCategory",
                    "ObjectProperty",
                    8,
                )?,
                can_be_rebound: bool_field("bCanBeRebound")?,
                hide_from_input_help: bool_field("bHideFromInputHelp")?,
                clickable: bool_field("bIsClickable")?,
                close_input_help: bool_field("bCloseInputHelp")?,
                display_action_index: bool_field("bDisplayActionIndex")?,
                mapping: MappingLayout {
                    ustruct: mapping,
                    // SAFETY: a live struct.
                    size: unsafe { reflection::struct_size(mapping) },
                    key_name: key + key_name,
                    shift: modifier("bShift")?,
                    ctrl: modifier("bCtrl")?,
                    alt: modifier("bAlt")?,
                    cmd: modifier("bCmd")?,
                },
            },
            category: CategoryLayout {
                class: category_class,
                label: category_info + text(category_display_info, "Name")?,
                icon: category_info + icon(category_display_info)?,
                parent: reflection::field_offset(
                    category_class,
                    "ParentCategory",
                    "ObjectProperty",
                    8,
                )?,
                order: reflection::field_offset(category_class, "SortPriority", "IntProperty", 4)?,
                show_in_input_help: reflection::bool_field(
                    category_class.cast(),
                    "bShowInInputHelp",
                )?,
            },
            initialize_struct,
            add_row_internal,
        })
    }

    /// The asset manager's input tables.
    ///
    /// # Safety
    /// `manager` must be the live `UBrickAssetManager`, and the array not used elsewhere meanwhile.
    /// Game thread only.
    pub unsafe fn tables<'a>(&self, manager: *mut UObjectBase) -> &'a mut TArray<*mut UObjectBase> {
        // SAFETY: per the contract; the offset comes from reflection.
        unsafe { &mut *manager.cast::<u8>().add(self.tables).cast() }
    }

    /// The asset manager's top input categories.
    ///
    /// # Safety
    /// As for [`Self::tables`].
    pub unsafe fn categories<'a>(
        &self,
        manager: *mut UObjectBase,
    ) -> &'a mut TArray<*mut UObjectBase> {
        // SAFETY: per the contract; the offset comes from reflection.
        unsafe { &mut *manager.cast::<u8>().add(self.categories).cast() }
    }

    /// A new, empty input table called `name` inside `outer`, kept for the whole game.
    ///
    /// # Safety
    /// `outer` must be a live object without anything called `name` inside. Game thread only.
    pub unsafe fn new_table(
        &self,
        outer: *mut UObjectBase,
        name: FName,
    ) -> Result<*mut UObjectBase, String> {
        // SAFETY: per the contract; `DataTable` can be instantiated, and `RowStruct` is set right
        // after construction, before anything reads the table.
        unsafe {
            let table = reflection::new_object(self.table_class, outer, name, OUR_FLAGS);
            if table.is_null() {
                return Err("no input table made".into());
            }
            table
                .cast::<u8>()
                .add(self.row_struct)
                .cast::<*const UStruct>()
                .write(self.row.ustruct);
            Ok(table)
        }
    }

    /// A new input category called `name` inside `outer`, shown as `label` (with the texture at the
    /// path `icon`, if any, where the game shows categories with icons: its top bar's menus), inside
    /// the category `parent` (null: a top category) at place `order` among its subcategories, kept
    /// for the whole game.
    ///
    /// # Safety
    /// `outer` must be a live object without anything called `name` inside, `parent` null or a live
    /// input category. Game thread only.
    pub unsafe fn new_category(
        &self,
        outer: *mut UObjectBase,
        name: FName,
        label: &Text,
        icon: Option<FName>,
        parent: *mut UObjectBase,
        order: i32,
    ) -> Result<*mut UObjectBase, String> {
        // SAFETY: per the contract; the fields' offsets come from reflection, and the label is a
        // live text from the constructor.
        unsafe {
            let category = reflection::new_object(self.category.class, outer, name, OUR_FLAGS);
            if category.is_null() {
                return Err("no input category made".into());
            }
            let base = category.cast::<u8>();
            label.assign_to(base.add(self.category.label).cast());
            if let Some(icon) = icon {
                base.add(self.category.icon)
                    .cast::<SoftPointer>()
                    .write_unaligned(SoftPointer::new(icon));
            }
            base.add(self.category.parent)
                .cast::<*mut UObjectBase>()
                .write(parent);
            base.add(self.category.order).cast::<i32>().write(order);
            self.category.show_in_input_help.set(base, false);
            Ok(category)
        }
    }

    /// Adds `action` to our `table`, in `category`: rebindable, out of the in-game input help unless
    /// it's a menu action.
    ///
    /// # Safety
    /// `table` must come from [`Self::new_table`] and not have the action yet, `category` must be a
    /// live input category. Game thread only.
    pub unsafe fn add_row(
        &self,
        table: *mut UObjectBase,
        action: &Action,
        category: *mut UObjectBase,
    ) -> Result<(), String> {
        let row = &self.row;
        // SAFETY: per the contract; the row is a fresh, constructed `FInputTableRow`, written at
        // reflected offsets, and its mappings array gets constructed elements from the game's
        // allocator, which the table frees with the row.
        unsafe {
            let add: AddRowInternalFn = core::mem::transmute(
                virtual_function(table.cast(), self.add_row_internal)
                    .ok_or("AddRowInternal isn't game code")?,
            );
            let base = self.new_structs(row.ustruct, 1)?;
            action.label.assign_to(base.add(row.label).cast());
            action
                .description
                .assign_to(base.add(row.description).cast());
            base.add(row.kind).write(row.press_release);
            base.add(row.order)
                .cast::<i32>()
                .write_unaligned(action.order);
            base.add(row.category)
                .cast::<*mut UObjectBase>()
                .write_unaligned(category);
            row.can_be_rebound.set(base, true);
            let in_menu = action.menu_icon.is_some();
            row.hide_from_input_help.set(base, !in_menu);
            row.clickable.set(base, in_menu);
            row.close_input_help.set(base, in_menu);
            row.display_action_index.set(base, false);
            if let Some(icon) = action.menu_icon {
                base.add(row.icon)
                    .cast::<SoftPointer>()
                    .write_unaligned(SoftPointer::new(icon));
            }
            if !action.keys.is_empty() {
                let mapping = &row.mapping;
                let elements = self.new_structs(mapping.ustruct, action.keys.len())?;
                for (index, chord) in action.keys.iter().enumerate() {
                    let element = elements.add(index * mapping.size);
                    element
                        .add(mapping.key_name)
                        .cast::<FName>()
                        .write_unaligned(chord.key);
                    mapping.shift.set(element, chord.shift);
                    mapping.ctrl.set(element, chord.ctrl);
                    mapping.alt.set(element, chord.alt);
                    mapping.cmd.set(element, chord.cmd);
                }
                let count = action.keys.len() as i32;
                base.add(row.mappings).cast::<RawArray>().write(RawArray {
                    data: elements,
                    num: count,
                    max: count,
                });
            }
            let name =
                u64::from(action.name.comparison_index) | u64::from(action.name.number) << 32;
            add(table, name, base);
        }
        Ok(())
    }

    /// `count` constructed structs of `ustruct` in a row, in memory from the game's allocator.
    ///
    /// # Safety
    /// `ustruct` must be a live struct. Game thread only.
    unsafe fn new_structs(&self, ustruct: *const UStruct, count: usize) -> Result<*mut u8, String> {
        // SAFETY: per the contract; `InitializeStruct` constructs `count` structs of its size in
        // the memory, which holds that many.
        unsafe {
            let initialize: InitializeStructFn = core::mem::transmute(
                virtual_function(ustruct.cast(), self.initialize_struct)
                    .ok_or("InitializeStruct isn't game code")?,
            );
            let size = reflection::struct_size(ustruct) * count;
            let memory = fmalloc::malloc(size).cast::<u8>();
            if memory.is_null() {
                return Err("out of memory".into());
            }
            initialize(ustruct, memory, count as i32);
            Ok(memory)
        }
    }
}

/// Adds `object` to `list` unless it's there already: right after `after` if that's listed, else
/// last. Lists the game emptied (its tables at shutdown) stay empty. Whether it was added.
///
/// # Safety
/// `list` must be a live array of the game's (see [`InputTables::tables`]). Game thread only.
pub unsafe fn add_once(
    list: &mut TArray<*mut UObjectBase>,
    object: *mut UObjectBase,
    after: Option<*mut UObjectBase>,
) -> bool {
    // SAFETY: per the contract; `push` grows the array with the game's allocator, like `TArray`,
    // and the slice covers its elements.
    unsafe {
        if list.num <= 0 || list.iter().any(|listed| listed == object) {
            return false;
        }
        list.push(object);
        let objects = core::slice::from_raw_parts_mut(list.data, list.num as usize);
        if let Some(at) = after.and_then(|after| objects.iter().position(|&other| other == after)) {
            objects[at + 1..].rotate_right(1);
        }
    }
    true
}

/// The object called `name` in `list` (e.g. an input category), if any.
///
/// # Safety
/// `list` must hold live objects or nulls.
pub unsafe fn find_named(list: &TArray<*mut UObjectBase>, name: &str) -> Option<*mut UObjectBase> {
    // SAFETY: per the contract.
    unsafe {
        list.iter()
            .find(|&object| !object.is_null() && reflection::name_is(&(*object).name_private, name))
    }
}

/// The offset and struct of the `StructProperty` `name` of `ustruct`, of any size.
fn nested(ustruct: *const UStruct, name: &'static str) -> Result<(usize, *const UStruct), String> {
    // SAFETY: `ustruct` comes from reflection; property records live as long as their struct.
    unsafe {
        let property = reflection::find_property(ustruct, name);
        if property.is_null() {
            return Err(format!("field '{name}' not found"));
        }
        let nested = reflection::struct_type(property);
        if nested.is_null() {
            return Err(format!("field '{name}' isn't a struct"));
        }
        Ok(((*property).offset_internal as usize, nested))
    }
}

/// The vtable offset of `UStruct::InitializeStruct` (see [`ADD_ROW`]), for other structs to be
/// constructed as the game constructs them.
#[cfg_attr(not(feature = "vehicle-menu"), allow(dead_code))]
pub fn initialize_struct_slot() -> Result<usize, String> {
    slots().map(|(initialize, _)| initialize)
}

/// Where [`ADD_ROW`]'s match calls `UScriptStruct::CopyScriptStruct` (`call rel32`; game RVA
/// 0x11EBFC0 in build 25831956: `STRUCT_CopyNative` -> the C++ copy, `STRUCT_IsPlainOldData` ->
/// `memcpy`, else property by property).
const COPY_SCRIPT_STRUCT_CALL: usize = 43;

/// `void UScriptStruct::CopyScriptStruct(void* Dest, void const* Src, int32 ArrayDim) const`:
/// copies `count` structs in a row from `source` to `target` (constructed), as the game copies
/// them.
pub type CopyScriptStructFn =
    unsafe extern "C" fn(ustruct: *const UStruct, target: *mut u8, source: *const u8, count: i32);

/// `UScriptStruct::CopyScriptStruct`, called by [`ADD_ROW`].
#[cfg_attr(not(feature = "vehicle-menu"), allow(dead_code))]
pub fn copy_script_struct() -> Result<CopyScriptStructFn, String> {
    let found = ADD_ROW.locate()?;
    // SAFETY: the call and its 4-byte offset lie within the matched pattern.
    let function = unsafe { found.call_target(COPY_SCRIPT_STRUCT_CALL) }
        .ok_or_else(|| format!("{}: no call of CopyScriptStruct", ADD_ROW.name))?;
    // SAFETY: the function `AddRow` calls as (struct, row, row data, 1), a game function start.
    Ok(unsafe { core::mem::transmute::<*const u8, CopyScriptStructFn>(function) })
}

/// The vtable offsets of `InitializeStruct` and `AddRowInternal`, read from [`ADD_ROW`].
fn slots() -> Result<(usize, usize), String> {
    let found = ADD_ROW.locate()?;
    // SAFETY: both offsets lie within the matched pattern.
    let read = |at| unsafe { found.address.add(at).cast::<u32>().read_unaligned() } as usize;
    let slots = (read(INITIALIZE_STRUCT_SLOT), read(ADD_ROW_INTERNAL_SLOT));
    let plausible = |slot: usize| slot.is_multiple_of(8) && slot < 0x1000;
    if !plausible(slots.0) || !plausible(slots.1) {
        return Err(format!(
            "{}: unexpected vtable offsets {slots:x?}",
            ADD_ROW.name
        ));
    }
    Ok(slots)
}

/// The function at byte offset `slot` of `object`'s vtable, if it's game code.
///
/// # Safety
/// `object` must be a live object with a vtable at least `slot` bytes long.
unsafe fn virtual_function(object: *const c_void, slot: usize) -> Option<*const u8> {
    // SAFETY: per the contract.
    let function = unsafe {
        let vtable = object.cast::<*const *const u8>().read();
        vtable.add(slot / size_of::<usize>()).read()
    };
    sig::is_code(function).then_some(function)
}
