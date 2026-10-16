//! Rows of our own in the game's property panels: our settings in a settings page, the map's
//! controls in the editor's panel. A panel lists the brick properties an object reflects
//! (`IBrickPropertyInterface::ReflectBrickProperties`): each one is added to an
//! `FBrickPropertyReflection` ([`Reflection`]) under a category (a titled section), with its display
//! name ([`ADD_PROPERTY`]). Brick properties are small C++ objects (`{vtable, FProperty*, FName}`,
//! shared through `TSharedRef`s) whose virtual functions read and write the value in the reflected
//! object through the `FProperty`. Ours ([`Row`]) copy the game's tables for a bool (a check box)
//! and an enum (a list, its items side by side or a dropdown) but replace every function touching a
//! value with ours, which read and write a [`Value`] of ours instead: no game field is involved.
//!
//! Found in game 1.11.7 (build 25831956); see `docs/re/settings-menu.md`.

use core::ffi::c_void;
use std::sync::{Once, OnceLock};

use brickrust::ue::fmalloc;
use brickrust::ue::fname::FName;
use brickrust::ue::tarray::TArray;

use crate::framework::settings::{Choice, Setting, Toggle};
use crate::framework::sig::{CodeShape, FunctionSig, StringRefSig, VtableSig};
use crate::game::kismet::{self, Texts};
use crate::game::slider_text::{FString, OwnedFString};
use crate::game::text::Text;
use crate::prelude::log;

/// What a row of ours shows and changes: one of the mod's settings, or a feature's own value.
#[derive(Clone, Copy)]
pub enum Value {
    /// A check box.
    Check(&'static dyn Check),
    /// A list to choose an item from.
    List(&'static dyn List),
}

/// A value shown as a check box. Its functions run on the game thread, whenever a panel lists the
/// row or changes it.
pub trait Check: Sync {
    fn checked(&self) -> bool;
    fn set_checked(&self, checked: bool);
}

/// A value chosen from a list. Its functions run on the game thread, whenever a panel lists the
/// row or changes it.
pub trait List: Sync {
    /// The items' names, in order: asked each time a panel lists the row, so they may change. Only
    /// the first [`MAX_ITEMS`] are listed.
    fn items(&self) -> Vec<String>;
    /// The index of the chosen item.
    fn chosen(&self) -> usize;
    /// Chooses the item at `index`; whether it could.
    fn choose(&self, index: usize) -> bool;
    /// Whether a panel shows the list as a dropdown (a combo box, like the siren brick's type)
    /// rather than its items side by side in one row, the game's default (fine for a few short
    /// items).
    fn dropdown(&self) -> bool {
        false
    }
}

/// How many items a list can have: the game sets an item's index as a byte.
pub const MAX_ITEMS: usize = 256;

impl Check for Toggle {
    fn checked(&self) -> bool {
        self.get()
    }

    fn set_checked(&self, checked: bool) {
        self.set(checked);
    }
}

impl List for Choice {
    fn items(&self) -> Vec<String> {
        self.options
            .iter()
            .map(|option| option.name.to_owned())
            .collect()
    }

    fn chosen(&self) -> usize {
        usize::from(self.get())
    }

    fn choose(&self, index: usize) -> bool {
        match u8::try_from(index) {
            Ok(option) if index < self.options.len() => {
                self.set(option);
                true
            }
            _ => false,
        }
    }
}

impl From<Setting> for Value {
    fn from(setting: Setting) -> Self {
        match setting {
            Setting::Toggle(toggle) => Value::Check(toggle),
            Setting::Choice(choice) => Value::List(choice),
        }
    }
}

/// `UBrickUserSettings::ReflectBrickProperties` (game 0xE8E380): the game's settings, by category.
/// A settings page asks for its categories (gameplay tags) through the reflection's filter.
pub const REFLECT_USER_SETTINGS: StringRefSig = StringRefSig {
    name: "UBrickUserSettings::ReflectBrickProperties",
    text: "Name_Menu_Gameplay",
};

/// [`REFLECT_USER_SETTINGS`]; `settings` is the settings object's `IBrickPropertyInterface`.
pub type ReflectFn = unsafe extern "C" fn(settings: *const c_void, reflection: *mut Reflection);

/// `FBrickPropertyReflection::AddCategory(const FText& Name)` (our name; game 0xCCD8B0): makes the
/// category (section) with that name the current one, adding it if new.
const ADD_CATEGORY: FunctionSig = FunctionSig {
    name: "FBrickPropertyReflection::AddCategory",
    // The categories (+0x60) searched for the name: their data and count, then -1 (none found yet).
    pattern: "48 8b 69 60 4c 8b f2 48 63 41 68 4c 8b f9 48 8b dd be ff ff ff ff",
};

type AddCategoryFn = unsafe extern "C" fn(reflection: *mut Reflection, name: *const Text);

/// `FBrickPropertyReflection::AddProperty(const TSharedRef<FBrickProperty>&, const FString& Name,
/// const FText& DisplayName)` (our name; game 0xCCCFB0): adds a row to the current category and
/// returns its `FBrickPropertyEditInfo`, whose description the caller may set
/// ([`EDIT_INFO_DESCRIPTION`]).
const ADD_PROPERTY: FunctionSig = FunctionSig {
    name: "FBrickPropertyReflection::AddProperty",
    // The property's instance made with the reflection's path (+0x78), then its edit info with the
    // reflection's objects (+8) and filter (+0x18).
    pattern: "49 8b d9 48 8b e9 4c 8d 49 78 48 8d 4c 24 20 e8 ?? ?? ?? ?? 4c 8d 4d 08 48 8b d3 4c 8d 45 18",
};

type AddPropertyFn = unsafe extern "C" fn(
    reflection: *mut Reflection,
    property: *const SharedRef,
    name: *const FString,
    display_name: *const Text,
) -> *mut u8;

/// Offset of an edit info's description (`FText`), the row's tooltip: the game writes it right
/// after `AddProperty` (e.g. for `bAutoCounterSteering`, game 0xE8FEF9).
const EDIT_INFO_DESCRIPTION: usize = 0x80;

/// Offsets of how an edit info has a list shown, which `UEnumPropertyWidget::UpdateValue` (game
/// 0xF03A30, the reads at 0xF03D83) passes to its combo box: `MaxListItems` (our name; `int32`)
/// shows the items side by side when there are at most that many, else a dropdown; -1, the
/// default, always side by side, 0 always a dropdown (as the siren brick's type sets it). A flag
/// before it makes the widget pass 0 instead (false by default; what else it does is unknown).
const EDIT_INFO_LIST_FLAG: usize = 0xB1;
const EDIT_INFO_MAX_LIST_ITEMS: usize = 0xB4;

/// The game's brick property classes ours copy the tables of.
const BOOL_PROPERTY: VtableSig = VtableSig {
    name: "FBoolBrickProperty vtable",
    type_name: "FBoolBrickProperty",
    length: 20,
};
const ENUM_PROPERTY: VtableSig = VtableSig {
    name: "FEnumBrickProperty vtable",
    type_name: "FEnumBrickProperty",
    length: 22,
};

/// The slots of a brick property's table we replace (brickrust's `FBrickPropertyVTable`, then the
/// typed functions).
mod slot {
    /// `(this, flags) -> this`: the deleting destructor.
    pub const DESTRUCTOR: usize = 4;
    /// `(this, const T* a, const T* b) -> bool`: whether two values are the same.
    pub const COMPARE_VALUES: usize = 6;
    /// `(this) -> bool`: whether it has its `FProperty`.
    pub const IS_VALID: usize = 7;
    /// `(this, container a, container b) -> bool`: whether two objects hold the same value.
    pub const SAME_VALUES: usize = 8;
    /// `(this, result*, archive slot, container) -> result*`: saving or loading the value.
    pub const SERIALIZE: usize = 9;
    /// `(this, container, T* value) -> bool` (an `int32` for enums): reads the value.
    pub const GET: usize = 18;
    /// `(this, container, T value) -> bool` (a byte for enums): writes it.
    pub const SET: usize = 19;
    /// Enums: `(this, container, TArray<FEnumPropertyItem>* items)`, the list to choose from.
    pub const ITEMS: usize = 20;
    /// Enums: `(this, container, uint8 value) -> bool`, whether a value may be set.
    pub const ALLOWED: usize = 21;
}

/// The bool getter (game 0xCE8D20) as it should be in [`slot::GET`]: from the container's last
/// object (or its root), at the `FProperty`'s offset (+0x4C).
const BOOL_GET: CodeShape = CodeShape {
    name: "FBoolBrickProperty get",
    shape: "83 7a 10 00 4c 8b 49 08 74 10 48 63 4a 10 48 8b 42 08 48 8d 51 ff 48 8d 14 d0 49 63 49 4c 48 8b 02 48 03 c1 74 09 0f b6 00 41 88 00 b0 01 c3",
};

/// The enum items function (game 0x7FC0F0) as it should be in [`slot::ITEMS`]: calls the
/// property's `TFunction` (+0x18), if any.
const ENUM_ITEMS: CodeShape = CodeShape {
    name: "FEnumBrickProperty items",
    shape: "83 79 20 00 74 10 48 8b 49 18 48 85 c9 74 07 48 8b 01 48 ff 60 50 c3",
};

/// `FBrickPropertyReflection`'s start (brickrust declares it all): whether the properties are only
/// listed to be saved or loaded, and the categories the panel asks for.
#[repr(C)]
pub struct Reflection {
    is_serializing: bool,
    /// The reflected objects and the edited one.
    _objects: [u8; 0x1F],
    /// The filter's categories (`FGameplayTagContainer::GameplayTags`; a tag is an `FName`).
    categories: TArray<FName>,
}

const _: () = assert!(core::mem::offset_of!(Reflection, categories) == 0x20);

impl Reflection {
    /// The categories (gameplay tags) this reflection lists rows to show for; none when it lists
    /// them to save or load them.
    pub fn shown_categories(&self) -> &[FName] {
        if self.is_serializing || self.categories.data.is_null() {
            return &[];
        }
        let count = usize::try_from(self.categories.num).unwrap_or(0);
        // SAFETY: a live array of `num` names, read on the game thread during the reflection.
        unsafe { core::slice::from_raw_parts(self.categories.data, count) }
    }

    /// Whether this reflection lists rows to show in the category `tag` (not to save or load them).
    pub fn shows(&self, tag: &FName) -> bool {
        self.shown_categories().iter().any(|category| {
            category.comparison_index == tag.comparison_index && category.number == tag.number
        })
    }
}

/// `FEnumPropertyItem` (game 0xA8 bytes, BRMK 0xB0): an item of an enum's list.
pub const ITEM_SIZE: usize = 0xA8;
/// Its value (`int32`) is first; its name (`FText`) starts its display info.
pub const ITEM_NAME: usize = 8;
/// Its display info's icon (`TSoftObjectPtr`) starts with a weak pointer's object index, -1 for none
/// (`FDisplayInfo`'s constructor, game 0xCC42F0).
const ITEM_ICON_INDEX: usize = 0x20;
/// Three more texts, empty in the game's lists (`TransmissionMode`'s, game 0xE6C4B0).
const ITEM_TEXTS: [usize; 3] = [0x48, 0x60, 0x78];
/// A flag the game's lists set (purpose unknown; the two flags before it stay false).
const ITEM_FLAG: usize = 0xA0;

/// A `TArray` with elements of a size known only at run time.
#[repr(C)]
pub struct RawArray {
    pub data: *mut u8,
    pub num: i32,
    pub max: i32,
}

/// What we need from the game to add rows: its reflection functions and the tables we copy.
pub struct Rows {
    add_category: AddCategoryFn,
    add_property: AddPropertyFn,
    bool_table: &'static [*const c_void],
    enum_table: &'static [*const c_void],
}

// SAFETY: code pointers and tables that live for the whole game; only used on the game thread.
unsafe impl Send for Rows {}
unsafe impl Sync for Rows {}

impl Rows {
    /// Finds the functions and makes our tables. Works before engine init.
    pub fn find() -> Result<Self, String> {
        let add_category = ADD_CATEGORY.resolve()?;
        let add_property = ADD_PROPERTY.resolve()?;
        let shared: [(usize, *const c_void); 5] = [
            (slot::DESTRUCTOR, destroy as *const c_void),
            (slot::COMPARE_VALUES, compare_bytes as *const c_void),
            (slot::IS_VALID, is_valid as *const c_void),
            (slot::SAME_VALUES, same_values as *const c_void),
            (slot::SERIALIZE, serialize as *const c_void),
        ];
        let bool_table = table(
            &BOOL_PROPERTY,
            (slot::GET, &BOOL_GET),
            &[
                (slot::GET, get_bool as *const c_void),
                (slot::SET, set_bool as *const c_void),
            ],
            &shared,
        )?;
        let enum_table = table(
            &ENUM_PROPERTY,
            (slot::ITEMS, &ENUM_ITEMS),
            &[
                (slot::GET, get_choice as *const c_void),
                (slot::SET, set_choice as *const c_void),
                (slot::ITEMS, choice_items as *const c_void),
                (slot::ALLOWED, choice_allowed as *const c_void),
            ],
            &shared,
        )?;
        // SAFETY: the functions found have these signatures (see their declarations).
        unsafe {
            Ok(Self {
                add_category: core::mem::transmute::<*const u8, AddCategoryFn>(add_category),
                add_property: core::mem::transmute::<*const u8, AddPropertyFn>(add_property),
                bool_table,
                enum_table,
            })
        }
    }

    /// A row for `setting`, which calls `changed` after a panel changed it. Game thread only,
    /// after engine init (it makes names and texts).
    pub fn setting_row(&self, setting: Setting, changed: fn()) -> Result<Row, String> {
        self.row(
            setting.key(),
            setting.name(),
            setting.description(),
            setting.into(),
            changed,
        )
    }

    /// A row called `name` (`description` is its tooltip) showing `value`, which calls `changed`
    /// after a panel changed it. `key` tells it from our other rows. Game thread only, after engine
    /// init (it makes names and texts).
    pub fn row(
        &self,
        key: &str,
        name: &str,
        description: &str,
        value: Value,
        changed: fn(),
    ) -> Result<Row, String> {
        let texts = texts()?;
        let table = match value {
            Value::Check(_) => self.bool_table,
            Value::List(_) => self.enum_table,
        };
        let key = format!("EditorTweaks_{key}");
        let shared = Box::into_raw(Box::new(Shared {
            controller: Controller {
                vtable: &KEPT,
                // Ours, never given back: the game never destroys the property.
                shared: 1,
                weak: 1,
            },
            property: Property {
                vtable: table.as_ptr(),
                field: core::ptr::null(),
                name: kismet::string_to_name(&key)?,
                functions: [0; 4],
                value,
                changed,
                texts,
                empty: Box::leak(Box::new(texts.text(""))),
            },
        }));
        // Kept for the whole game: the game holds references to it.
        Ok(Row {
            shared,
            key: OwnedFString::new(&key),
            name: texts.text(name),
            description: texts.text(description),
        })
    }

    /// Starts the category (section) `title` in `reflection`: rows added next go under it.
    ///
    /// # Safety
    /// `reflection` must be the live reflection the game is filling. Game thread only.
    pub unsafe fn add_category(&self, reflection: *mut Reflection, title: &Text) {
        // SAFETY: per the contract; the function only reads the title (copying it).
        unsafe { (self.add_category)(reflection, title) };
    }

    /// Adds `row` to the current category of `reflection`.
    ///
    /// # Safety
    /// As for [`Self::add_category`].
    pub unsafe fn add_row(&self, reflection: *mut Reflection, row: &Row) {
        // SAFETY: per the contract; the row's property lives for the whole game. The game copies the
        // reference (counting it), the name and the display name, and returns the row's edit info,
        // whose description is an `FText`.
        unsafe {
            let shared = SharedRef {
                object: &raw mut (*row.shared).property,
                controller: &raw mut (*row.shared).controller,
            };
            let info = (self.add_property)(reflection, &shared, row.key.as_ptr(), &row.name);
            if info.is_null() {
                return;
            }
            row.description
                .assign_to(info.add(EDIT_INFO_DESCRIPTION).cast());
            if matches!((*row.shared).property.value, Value::List(list) if list.dropdown()) {
                show_as_dropdown(info);
            }
        }
    }
}

/// Has the list whose edit info is `info` shown as a dropdown, as the siren brick's type does
/// (`MaxListItems` 0, BRMK 0x178E08). Only if the edit info holds what `AddProperty` gives
/// it, so that a game update moving the fields can't have us write over another one: the list then
/// shows its items side by side, and the log says so once.
///
/// # Safety
/// `info` must be the edit info `AddProperty` just returned. Game thread only.
unsafe fn show_as_dropdown(info: *mut u8) {
    static MOVED: Once = Once::new();
    // SAFETY: per the contract; the edit info (0xD8 bytes, 8-aligned) holds a flag and an `int32`
    // at these offsets.
    unsafe {
        let flag = info.add(EDIT_INFO_LIST_FLAG).read();
        let max_items = info.add(EDIT_INFO_MAX_LIST_ITEMS).cast::<i32>();
        if flag == 0 && max_items.read() == -1 {
            max_items.write(0);
        } else {
            MOVED.call_once(|| {
                log!("a list's edit info isn't as expected: lists show as rows of buttons")
            });
        }
    }
}

/// Copies the game's table for `sig` with `overrides` and `shared` (ours) in place of the game's
/// functions, after checking that the slot `check` holds what we expect (so the slots haven't
/// moved). Identical copies of the table may be found: they must agree on every slot we keep.
fn table(
    sig: &VtableSig,
    (checked, shape): (usize, &CodeShape),
    overrides: &[(usize, *const c_void)],
    shared: &[(usize, *const c_void)],
) -> Result<&'static [*const c_void], String> {
    let found = sig.resolve()?;
    let ours = |index: usize| {
        overrides
            .iter()
            .chain(shared)
            .find(|&&(slot, _)| slot == index)
            .map(|&(_, function)| function)
    };
    // SAFETY: each table found has `length` code pointers.
    let read = |table: *const *const u8| unsafe { core::slice::from_raw_parts(table, sig.length) };
    let first = read(found[0]);
    if !shape.matches(first[checked]) {
        return Err(format!("{}: slot {checked} isn't {}", sig.name, shape.name));
    }
    for &other in &found[1..] {
        let other = read(other);
        if (0..sig.length).any(|index| ours(index).is_none() && other[index] != first[index]) {
            return Err(format!("{}: copies of the table differ", sig.name));
        }
    }
    let table: Vec<*const c_void> = (0..sig.length)
        .map(|index| ours(index).unwrap_or(first[index].cast()))
        .collect();
    Ok(table.leak())
}

/// A row of ours: the property the panel shows, with its name (`FString`), display name and
/// description.
pub struct Row {
    /// Leaked: the game keeps references to the property.
    shared: *mut Shared,
    key: OwnedFString,
    name: Text,
    description: Text,
}

// SAFETY: the row's game objects (the property, texts) are only used on the game thread.
unsafe impl Send for Row {}
unsafe impl Sync for Row {}

/// A property of ours as `MakeShared` lays it out: the reference controller, then the property.
#[repr(C)]
struct Shared {
    controller: Controller,
    property: Property,
}

/// `FReferenceControllerBase`: the reference counts (not thread-safe ones for brick properties).
#[repr(C)]
struct Controller {
    vtable: &'static ControllerTable,
    shared: i32,
    weak: i32,
}

#[repr(C)]
struct ControllerTable {
    destroy_object: unsafe extern "C" fn(controller: *mut Controller),
    delete: unsafe extern "C" fn(controller: *mut Controller, flags: u32),
}

/// Our properties are never destroyed (we keep a reference); should the counts ever run out, they
/// stay anyway.
static KEPT: ControllerTable = ControllerTable {
    destroy_object: keep,
    delete: keep_controller,
};

unsafe extern "C" fn keep(_controller: *mut Controller) {}

unsafe extern "C" fn keep_controller(_controller: *mut Controller, _flags: u32) {}

/// `TSharedRef<FBrickProperty>`.
#[repr(C)]
struct SharedRef {
    object: *mut Property,
    controller: *mut Controller,
}

/// The text conversions our rows make their texts with, found once (after engine init: finding
/// them walks every object).
fn texts() -> Result<&'static Texts, String> {
    static TEXTS: OnceLock<Texts> = OnceLock::new();
    if let Some(texts) = TEXTS.get() {
        return Ok(texts);
    }
    let found = kismet::texts()?;
    Ok(TEXTS.get_or_init(|| found))
}

/// A brick property of ours: the game's layout (its table, the `FProperty` it would edit, its name,
/// and for an enum the functions giving its items and checking values), then what our functions
/// need.
#[repr(C)]
struct Property {
    vtable: *const *const c_void,
    /// None: our functions never touch a game field.
    field: *const c_void,
    name: FName,
    /// An enum's `TFunction`s, empty: we replaced the functions calling them.
    functions: [usize; 4],
    value: Value,
    changed: fn(),
    /// For a list's items' names, made each time it's listed.
    texts: &'static Texts,
    /// An empty text, for the list items' other texts.
    empty: &'static Text,
}

unsafe extern "C" fn destroy(this: *mut Property, _flags: u32) -> *mut Property {
    this
}

unsafe extern "C" fn compare_bytes(_this: *const Property, a: *const u8, b: *const u8) -> bool {
    // SAFETY: the game passes two values of the property's type (a bool, or an enum's byte).
    unsafe { *a == *b }
}

unsafe extern "C" fn is_valid(_this: *const Property) -> bool {
    true
}

unsafe extern "C" fn same_values(
    _this: *const Property,
    _a: *const c_void,
    _b: *const c_void,
) -> bool {
    // Our settings have one value, whatever the object.
    true
}

/// What [`slot::SERIALIZE`] returns: the game's own result without a field (status 2).
#[repr(C)]
struct Serialized {
    status: u32,
    done: bool,
}

unsafe extern "C" fn serialize(
    _this: *const Property,
    result: *mut Serialized,
    _slot: *mut c_void,
    _container: *const c_void,
) -> *mut Serialized {
    // SAFETY: the caller passes room for the result. Never reached: we don't add rows while the
    // game lists properties to save or load them (see `Reflection::shows`).
    unsafe {
        result.write(Serialized {
            status: 2,
            done: false,
        })
    };
    result
}

unsafe extern "C" fn get_bool(
    this: *const Property,
    _container: *const c_void,
    value: *mut u8,
) -> bool {
    // SAFETY: `this` is one of our properties, `value` room for a bool.
    unsafe {
        let Value::Check(check) = (*this).value else {
            return false;
        };
        value.write(u8::from(check.checked()));
    }
    true
}

unsafe extern "C" fn set_bool(this: *const Property, _container: *const c_void, value: u8) -> bool {
    // SAFETY: `this` is one of our properties.
    let property = unsafe { &*this };
    let Value::Check(check) = property.value else {
        return false;
    };
    check.set_checked(value != 0);
    (property.changed)();
    true
}

unsafe extern "C" fn get_choice(
    this: *const Property,
    _container: *const c_void,
    value: *mut i32,
) -> bool {
    // SAFETY: `this` is one of our properties, `value` room for an `int32`.
    unsafe {
        let Value::List(list) = (*this).value else {
            return false;
        };
        value.write(i32::try_from(list.chosen()).unwrap_or(0));
    }
    true
}

unsafe extern "C" fn set_choice(
    this: *const Property,
    container: *const c_void,
    value: u8,
) -> bool {
    // SAFETY: `this` is one of our properties.
    let property = unsafe { &*this };
    let Value::List(list) = property.value else {
        return false;
    };
    // SAFETY: as for this function.
    if !unsafe { choice_allowed(this, container, value) } || !list.choose(usize::from(value)) {
        return false;
    }
    (property.changed)();
    true
}

unsafe extern "C" fn choice_allowed(
    this: *const Property,
    _container: *const c_void,
    value: u8,
) -> bool {
    // SAFETY: `this` is one of our properties.
    let property = unsafe { &*this };
    let Value::List(list) = property.value else {
        return false;
    };
    usize::from(value) < list.items().len().min(MAX_ITEMS)
}

unsafe extern "C" fn choice_items(
    this: *const Property,
    _container: *const c_void,
    items: *mut RawArray,
) {
    // SAFETY: `this` is one of our properties, `items` the game's `TArray<FEnumPropertyItem>`.
    unsafe {
        let property = &*this;
        let Value::List(list) = property.value else {
            return;
        };
        for (value, name) in list.items().iter().take(MAX_ITEMS).enumerate() {
            let name = property.texts.text(name);
            append_item(&mut *items, value as i32, &name, property.empty);
            // The item holds its own reference now.
            name.release();
        }
    }
}

/// Appends an item `value` named `name` to `items`, like the game's lists do (`TransmissionMode`'s,
/// game 0xE6C4B0): no icon, the other texts `empty`.
///
/// # Safety
/// `items` must be a live `TArray<FEnumPropertyItem>`. Game thread only.
unsafe fn append_item(items: &mut RawArray, value: i32, name: &Text, empty: &Text) {
    let count = usize::try_from(items.num).unwrap_or(0);
    // SAFETY: per the contract; items are relocatable like every `TArray` element, so growing the
    // array with the game's allocator is what `TArray` does too, and texts are assigned into
    // zeroed memory, which holds no reference to give back.
    unsafe {
        if usize::try_from(items.max).unwrap_or(0) <= count {
            let room = (count + 1).max(4);
            let grown = fmalloc::realloc(items.data.cast(), room * ITEM_SIZE).cast::<u8>();
            if grown.is_null() {
                return;
            }
            items.data = grown;
            items.max = room as i32;
        }
        let item = items.data.add(count * ITEM_SIZE);
        item.write_bytes(0, ITEM_SIZE);
        item.cast::<i32>().write(value);
        name.assign_to(item.add(ITEM_NAME).cast());
        item.add(ITEM_ICON_INDEX).cast::<i32>().write(-1);
        for offset in ITEM_TEXTS {
            empty.assign_to(item.add(offset).cast());
        }
        item.add(ITEM_FLAG).write(1);
        items.num = count as i32 + 1;
    }
}
