//! Rows of our own in the game's property panels: our settings in a settings page, the map's
//! controls in the editor's panel. A panel lists the brick properties an object reflects
//! (`IBrickPropertyInterface::ReflectBrickProperties`): each one is added to an
//! `FBrickPropertyReflection` ([`Reflection`]) under a category (a titled section), with its display
//! name ([`ADD_PROPERTY`]). Brick properties are small C++ objects (`{vtable, FProperty*, FName}`,
//! shared through `TSharedRef`s) whose virtual functions read and write the value in the reflected
//! object through the `FProperty`. Ours ([`Row`]) copy the game's tables for a bool (a check box),
//! an enum (a list, its items side by side or a dropdown), a number (one to three sliders that
//! also take typed values) and a color (the color picker, see [`color`]) but replace every function
//! touching a value with ours, which read and write a [`Value`] of ours instead: no game field is
//! involved.
//!
//! Found in game 1.11.7 (build 25831956); see `docs/re/settings-menu.md` and, for numbers and
//! colors, `docs/re/property-panel.md`.

mod color;

pub use color::Color;

use core::cell::Cell;
use core::ffi::c_void;
use std::sync::{Once, OnceLock};

use brickrust::ue::fmalloc;
use brickrust::ue::fname::FName;
use brickrust::ue::tarray::TArray;

use crate::framework::settings::{Choice, Setting, Toggle};
use crate::framework::sig::{CodeShape, FunctionSig, StringRefSig, VtableSig};
use crate::game::kismet::{self, Texts};
use crate::game::numeric_property::NumericValue;
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
    /// Numbers, each a slider taking typed values too.
    Number(&'static dyn Number),
    /// A color, shown as the game's color picker, like a brick's.
    Color(&'static dyn Color),
}

/// One to three numbers shown as sliders side by side, like a brick's position. Its functions run
/// on the game thread, whenever a panel lists the row or changes it: while a slider is dragged,
/// every frame.
pub trait Number: Sync {
    /// The values shown, one per slider (1 to 3; more are left out).
    fn values(&self) -> Vec<f32>;
    /// Sets the values (as many as [`Self::values`] gives), within [`Self::limit`].
    fn set_values(&self, values: &[f32]);
    /// The smallest and largest values, the sliders' ends.
    fn range(&self) -> (f32, f32);
    /// The sliders' ends of the value `index`, if they differ by value (like an axis' bounds).
    fn component_range(&self, _index: usize) -> (f32, f32) {
        self.range()
    }
    /// The smallest and largest values that can be set at all, typed ones too: the sliders'
    /// ends unless the row lets typed values go past them (like bricks').
    fn limit(&self) -> (f32, f32) {
        self.range()
    }
    /// How the values are shown.
    fn unit(&self) -> Unit;
    /// How the value `index` is shown, if it differs by value (like a time next to a percentage).
    /// Only shown so while the panel's `UpdateValue` is hooked (`features::number_rows`); otherwise
    /// every slider shows [`Self::unit`].
    fn component_unit(&self, _index: usize) -> Unit {
        self.unit()
    }
    /// Whether the values are alike, like a size's axes: then the row has the game's lock and our
    /// value order button. Unlike values (a time and a percentage) have neither.
    fn alike(&self) -> bool {
        true
    }
}

/// How a number row shows its values: the game's `ENumericValueType`s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Unit {
    /// A fraction shown as a percentage (0.5 is 50 %).
    Percent = 2,
    /// Degrees.
    Angle = 3,
    /// Hours, shown and typed as `h:m` (the game's `TimeOfDay`).
    TimeOfDay = 9,
    /// A distance in cm, shown in the user's units (`DistanceAutoOrBrickUnits`, like bricks'
    /// positions).
    Distance = 18,
}

/// How many sliders a number row can have (`FNumericBrickPropertyValue` holds three).
const MAX_COMPONENTS: usize = 3;

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

/// `FNumericBrickPropertyValue`: up to three components, `count` of them used.
#[repr(C)]
#[derive(Clone, Copy, PartialEq)]
struct Numbers {
    data: [f32; MAX_COMPONENTS],
    count: u8,
}

/// `FNumericBrickPropertyRange`: the smallest and largest value of each component.
#[repr(C)]
struct NumbersRange {
    min: Numbers,
    max: Numbers,
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
/// ([`EDIT_INFO_DESCRIPTION`]). `Name` is the edited field's name (`"EditorMirrorAxis"`, a
/// struct's `"SunAngle"`). Hooked by `features::settings_list` to rework the game's rows.
pub const ADD_PROPERTY: FunctionSig = FunctionSig {
    name: "FBrickPropertyReflection::AddProperty",
    // The property's instance made with the reflection's path (+0x78), then its edit info with the
    // reflection's objects (+8) and filter (+0x18).
    pattern: "49 8b d9 48 8b e9 4c 8d 49 78 48 8d 4c 24 20 e8 ?? ?? ?? ?? 4c 8d 4d 08 48 8b d3 4c 8d 45 18",
};

/// [`ADD_PROPERTY`]; `property` is a `TSharedRef<FBrickProperty>`, the result the row's edit info
/// (null if none was made).
pub type AddPropertyFn = unsafe extern "C" fn(
    reflection: *mut Reflection,
    property: *const c_void,
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
/// The typed variants (float, vector, rotator, ...) keep the base's name and differ only in the
/// slots we replace (compare, get, set), so any copy does.
const NUMERIC_PROPERTY: VtableSig = VtableSig {
    name: "FNumericBrickProperty vtable",
    type_name: "FNumericBrickProperty",
    length: 21,
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
    /// Numbers: `(this, FNumericBrickPropertyRange* out, container) -> out`, the sliders' ends.
    pub const RANGE: usize = 20;
}

/// The float variant's getter (game 0xCE8BE0) as it is in [`slot::GET`]: one float from the
/// container's last object (or its root) at the `FProperty`'s offset, the other components zeroed,
/// one component used. The other variants' tables (vector, rotator, ...) are left out.
const FLOAT_GET: CodeShape = CodeShape {
    name: "FNumericBrickProperty float get",
    shape: "83 7a 10 00 4c 8b 49 08 74 10 48 63 4a 10 48 8b 42 08 48 8d 51 ff 48 8d 14 d0 49 63 49 4c 48 8b 12 48 03 d1 74 17 f3 0f 10 02 33 c0 49 89 40 04 b0 01 f3 41 0f 11 00 41 c6 40 0c 01 c3",
};

/// Where a numeric property keeps its value type (`ENumericValueType`), an attribute like its
/// others: the stored value, a flag saying it's set, then a delegate (unbound when zero).
/// `UNumericPropertyWidget::UpdateValue` (game 0xF03680) reads it (the delegate's result if bound,
/// else the stored value if set, else 2) and hands it to the row's Blueprint
/// (`UpdateNumericProperty`). Offsets from the start of [`Property::fields`].
const NUMBER_UNIT: usize = 0x18 - FIELDS;
const NUMBER_UNIT_SET: usize = 0x19 - FIELDS;

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
    /// `None` if not found (logged).
    number_table: Option<&'static [*const c_void]>,
    /// `None` if not found or not hooked (logged).
    color_table: Option<&'static [*const c_void]>,
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
            None,
            (slot::GET, &BOOL_GET),
            &[
                (slot::GET, get_bool as *const c_void),
                (slot::SET, set_bool as *const c_void),
            ],
            &shared,
        )?;
        let enum_table = table(
            &ENUM_PROPERTY,
            None,
            (slot::ITEMS, &ENUM_ITEMS),
            &[
                (slot::GET, get_choice as *const c_void),
                (slot::SET, set_choice as *const c_void),
                (slot::ITEMS, choice_items as *const c_void),
                (slot::ALLOWED, choice_allowed as *const c_void),
            ],
            &shared,
        )?;
        // Without it, only number rows are missing.
        let number_table = table(
            &NUMERIC_PROPERTY,
            Some((slot::GET, &FLOAT_GET)),
            // Not the range function: `property_limits` hooks it, which changes its first bytes.
            (slot::GET, &FLOAT_GET),
            &[
                (slot::COMPARE_VALUES, compare_numbers as *const c_void),
                (slot::GET, get_numbers as *const c_void),
                (slot::SET, set_numbers as *const c_void),
                (slot::RANGE, numbers_range as *const c_void),
            ],
            &shared,
        )
        .map_err(|reason| log!("no number rows: {reason}"))
        .ok();
        // Without it, only color rows are missing.
        let color_table = color::table(&shared)
            .map_err(|reason| log!("no color rows: {reason}"))
            .ok();
        // SAFETY: the functions found have these signatures (see their declarations).
        unsafe {
            Ok(Self {
                add_category: core::mem::transmute::<*const u8, AddCategoryFn>(add_category),
                add_property: core::mem::transmute::<*const u8, AddPropertyFn>(add_property),
                bool_table,
                enum_table,
                number_table,
                color_table,
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
        self.row_named(
            &format!("EditorTweaks_{key}"),
            name,
            description,
            value,
            changed,
        )
    }

    /// [`Self::row`] whose property is called exactly `property` (the name of the field it would
    /// edit): a row standing in for one of the game's, under its name, so that the game reacts to
    /// it changing as to its own row (the event a panel sends after a change names the property).
    pub fn row_named(
        &self,
        property: &str,
        name: &str,
        description: &str,
        value: Value,
        changed: fn(),
    ) -> Result<Row, String> {
        let texts = texts()?;
        let table = match value {
            Value::Check(_) => self.bool_table,
            Value::List(_) => self.enum_table,
            Value::Number(_) => self.number_table.ok_or("number rows aren't available")?,
            Value::Color(_) => self.color_table.ok_or("color rows aren't available")?,
        };
        let mut fields = [0; FIELDS_SIZE];
        match value {
            Value::Number(number) => {
                fields[NUMBER_UNIT] = number.unit() as u8;
                fields[NUMBER_UNIT_SET] = 1;
            }
            Value::Color(color) => {
                fields[color::ALPHA] = u8::from(color.has_alpha());
                fields[color::ALPHA_SET] = 1;
            }
            Value::Check(_) | Value::List(_) => {}
        }
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
                name: kismet::string_to_name(property)?,
                fields: Fields(fields),
                value,
                changed,
                texts,
                empty: Box::leak(Box::new(texts.text(""))),
            },
        }));
        // Kept for the whole game: the game holds references to it.
        Ok(Row {
            shared,
            key: OwnedFString::new(property),
            name: texts.text(name),
            description: texts.text(description),
            when: None,
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

    /// Adds `row` to the current category of `reflection`, and returns its edit info (null if the
    /// game made none).
    ///
    /// # Safety
    /// As for [`Self::add_category`].
    pub unsafe fn add_row(&self, reflection: *mut Reflection, row: &Row) -> *mut u8 {
        // SAFETY: per the contract; the row's property lives for the whole game. The game copies the
        // reference (counting it), the name and the display name, and returns the row's edit info.
        unsafe {
            let shared = SharedRef {
                object: &raw mut (*row.shared).property,
                controller: &raw mut (*row.shared).controller,
            };
            let info = (self.add_property)(
                reflection,
                (&raw const shared).cast(),
                row.key.as_ptr(),
                &row.name,
            );
            if info.is_null() {
                return info;
            }
            describe(info, row);
            if matches!((*row.shared).property.value, Value::List(list) if list.dropdown()) {
                show_as_dropdown(info);
            }
            info
        }
    }
}

/// Gives the row whose edit info is `info` the description of `row` (its tooltip), e.g. again
/// after the game wrote its own there.
///
/// # Safety
/// `info` must be a live edit info, as `AddProperty` returns it. Game thread only.
pub unsafe fn describe(info: *mut u8, row: &Row) {
    // SAFETY: per the contract; the description is an `FText` at this offset.
    unsafe {
        row.description
            .assign_to(info.add(EDIT_INFO_DESCRIPTION).cast())
    };
}

/// Has the list whose edit info is `info` shown as a dropdown, as the siren brick's type does
/// (`MaxListItems` 0, BRMK 0x178E08). Only if the edit info holds what `AddProperty` gives
/// it, so that a game update moving the fields can't have us write over another one: the list then
/// shows its items side by side, and the log says so once. The game's own lists may set how many
/// items go in a row of buttons (+0xB8, e.g. the view mode's 3), which a dropdown ignores.
///
/// # Safety
/// `info` must be a live list's edit info, as `AddProperty` returns it. Game thread only.
pub unsafe fn show_as_dropdown(info: *mut u8) {
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
/// moved). With a `variant` (a slot and its shape), only the tables holding that shape there count
/// (the typed variants of a class share its name). Identical copies of the table may be found:
/// they must agree on every slot we keep.
fn table(
    sig: &VtableSig,
    variant: Option<(usize, &CodeShape)>,
    (checked, shape): (usize, &CodeShape),
    overrides: &[(usize, *const c_void)],
    shared: &[(usize, *const c_void)],
) -> Result<&'static [*const c_void], String> {
    let ours = |index: usize| {
        overrides
            .iter()
            .chain(shared)
            .find(|&&(slot, _)| slot == index)
            .map(|&(_, function)| function)
    };
    // SAFETY: each table found has `length` code pointers.
    let read = |table: *const *const u8| unsafe { core::slice::from_raw_parts(table, sig.length) };
    let found: Vec<_> = sig
        .resolve()?
        .into_iter()
        .filter(|&table| variant.is_none_or(|(slot, shape)| shape.matches(read(table)[slot])))
        .collect();
    let Some(&first) = found.first() else {
        return Err(format!("{}: no table of the variant wanted", sig.name));
    };
    let first = read(first);
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
    /// Whether the row is listed now (always without).
    when: Option<fn() -> bool>,
}

impl Row {
    /// The row, listed only while `when` says so (asked each time a panel lists its rows, on the
    /// game thread).
    pub fn when(self, when: fn() -> bool) -> Self {
        Self {
            when: Some(when),
            ..self
        }
    }

    /// Whether a panel listing its rows now lists this one.
    pub fn is_listed(&self) -> bool {
        self.when.is_none_or(|when| when())
    }
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

/// Where [`Property::fields`] starts, and its size: up to the end of a numeric property's
/// attributes (value type +0x18, range +0x30, axis lock +0x68, another +0x80, "don't clamp"
/// +0x98, each with its delegate; the last ends at +0xB0).
const FIELDS: usize = 0x18;
const FIELDS_SIZE: usize = 0xB0 - FIELDS;

/// [`Property::fields`], aligned like the pointers in it.
#[repr(C, align(8))]
struct Fields([u8; FIELDS_SIZE]);

/// A brick property of ours: the game's layout (its table, the `FProperty` it would edit, its name,
/// and its type's own fields), then what our functions need.
#[repr(C)]
struct Property {
    vtable: *const *const c_void,
    /// None: our functions never touch a game field.
    field: *const c_void,
    name: FName,
    /// The type's own fields, zeros (empty, unset) but for a number's value type: an enum's
    /// `TFunction`s (we replaced the functions calling them), a number's attributes.
    fields: Fields,
    value: Value,
    changed: fn(),
    /// For a list's items' names, made each time it's listed.
    texts: &'static Texts,
    /// An empty text, for the list items' other texts.
    empty: &'static Text,
}

const _: () = assert!(core::mem::offset_of!(Property, fields) == FIELDS);

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

unsafe extern "C" fn compare_numbers(
    _this: *const Property,
    a: *const Numbers,
    b: *const Numbers,
) -> bool {
    // SAFETY: the game passes two values of the property's type.
    unsafe { *a == *b }
}

thread_local! {
    /// While a numeric row updates on this thread ([`noting_number_shown`]): our number it shows,
    /// once its value was read. `None` outside of an update.
    static NUMBER_SHOWN: Cell<Option<Option<&'static dyn Number>>> = const { Cell::new(None) };
}

/// Runs `update`, a numeric row's `UpdateValue` (`UNumericPropertyWidget`), and returns which of our
/// number rows it showed, if one: the row reads its value through our getter first thing.
pub fn noting_number_shown(update: impl FnOnce()) -> Option<&'static dyn Number> {
    let previous = NUMBER_SHOWN.replace(Some(None));
    update();
    NUMBER_SHOWN.replace(previous).flatten()
}

/// During [`noting_number_shown`]'s update, once the row read its value: our number it shows, if
/// it's one of ours. E.g. while the row's Blueprint adds its buttons.
pub fn number_shown() -> Option<&'static dyn Number> {
    NUMBER_SHOWN.get().flatten()
}

unsafe extern "C" fn get_numbers(
    this: *const Property,
    _container: *const c_void,
    out: *mut Numbers,
) -> bool {
    // SAFETY: `this` is one of our properties, `out` room for a value.
    unsafe {
        let Value::Number(number) = (*this).value else {
            return false;
        };
        if NUMBER_SHOWN.get().is_some() {
            NUMBER_SHOWN.set(Some(Some(number)));
        }
        let values = number.values();
        let mut numbers = Numbers {
            data: [0.0; MAX_COMPONENTS],
            count: values.len().min(MAX_COMPONENTS) as u8,
        };
        for (slot, value) in numbers.data.iter_mut().zip(&values) {
            *slot = *value;
        }
        out.write(numbers);
    }
    true
}

/// What a number row of ours stores instead of a value a panel writes to it (`None`: the value as
/// written), called with the row's property and container like a game property's `ClampValue`.
/// Features rewrite the values the game's numeric properties store from `property_limits`' hook of
/// `ClampValue`, which our rows don't call.
pub type NumbersRewrite = unsafe fn(
    property: *const c_void,
    container: *const c_void,
    written: &NumericValue,
) -> Option<NumericValue>;

static NUMBERS_REWRITE: OnceLock<NumbersRewrite> = OnceLock::new();

/// Makes number rows of ours store what `rewrite` gives for the values written to them. Once.
pub fn rewrite_written_numbers(rewrite: NumbersRewrite) {
    let _ = NUMBERS_REWRITE.set(rewrite);
}

unsafe extern "C" fn set_numbers(
    this: *const Property,
    container: *const c_void,
    value: *const Numbers,
) -> bool {
    // SAFETY: `this` is one of our properties, `value` a value of its type.
    let (property, value) = unsafe { (&*this, *value) };
    let Value::Number(number) = property.value else {
        return false;
    };
    let written = NumericValue {
        data: value.data,
        num_used: value.count,
    };
    // SAFETY: the property and container of this setter call, as the rewrite takes them.
    let value = match NUMBERS_REWRITE
        .get()
        .and_then(|rewrite| unsafe { rewrite(this.cast(), container, &written) })
    {
        Some(rewritten) => Numbers {
            data: rewritten.data,
            count: rewritten.num_used,
        },
        None => value,
    };
    let (min, max) = number.limit();
    let count = usize::from(value.count).min(MAX_COMPONENTS);
    let values: Vec<f32> = value.data[..count]
        .iter()
        .map(|value| {
            if value.is_nan() {
                min
            } else {
                value.clamp(min, max)
            }
        })
        .collect();
    number.set_values(&values);
    (property.changed)();
    true
}

unsafe extern "C" fn numbers_range(
    this: *const Property,
    out: *mut NumbersRange,
    _container: *const c_void,
) -> *mut NumbersRange {
    // SAFETY: `this` is one of our properties, `out` room for a range.
    unsafe {
        let ends = |index| match (*this).value {
            Value::Number(number) => number.component_range(index),
            _ => (0.0, 1.0),
        };
        let ranges: [(f32, f32); MAX_COMPONENTS] = core::array::from_fn(ends);
        let side = |pick: fn(&(f32, f32)) -> f32| Numbers {
            data: ranges.each_ref().map(pick),
            count: MAX_COMPONENTS as u8,
        };
        out.write(NumbersRange {
            min: side(|&(min, _)| min),
            max: side(|&(_, max)| max),
        });
    }
    out
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
