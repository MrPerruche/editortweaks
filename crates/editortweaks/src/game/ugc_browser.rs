//! The vehicle browser's list (`UUGCBrowserWidget`, Blueprint `WBP_UGCBrowser`): how a page of
//! items is asked for and shown. See `docs/vehicle-menu.md`, "What the game does".
//!
//! A page goes `UUGCBrowserWidget::OnLoadListPage(page)` -> `UBrickGameInstance::QueryUGC(settings,
//! task)` -> (Local: the game walks every vehicle folder, sorts the whole list and cuts out the
//! page; else: Steam) -> the settings' delegate, bound to `UUGCBrowserWidget::OnUGCQueryComplete(
//! results, numEntries, numPages)`, which copies the results into the list's rows.
//!
//! Found in BRMK 1.11.6 (exports), then in the shipping exe of game 1.11.8 (build 26488256):
//! `OnLoadListPage` 0xEE96B0, `QueryUGC` 0xDF28D0, `OnUGCQueryComplete` 0xEEB4C0.
//!
//! The left panel, while no vehicle is selected, lists what the browser reflects
//! (`UUGCBrowserWidget::ReflectBrickProperties`, [`REFLECT_PANEL`]): category, sort, search text.

use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::UClass;

use crate::framework::hook;
use crate::framework::sig::{CodeShape, FunctionSig, StringRefSig};
use crate::game::property_reflection::ReflectFn;
use crate::game::reflection::{self, Function, Getter, NativeThunk, Parameters, SingleCall};
use crate::game::slider_text::FString;
use crate::prelude::log;

/// `UUGCBrowserWidget::OnLoadListPage(int32 page)`, by its start: keeps the page, gets the game
/// instance and the popup params, then checks its paged list (offset wildcarded).
pub const LOAD_PAGE: FunctionSig = FunctionSig {
    name: "UUGCBrowserWidget::OnLoadListPage",
    pattern: "45 33 ed 8b f2 44 89 6d ?? 4c 8b f9 e8 ?? ?? ?? ?? 49 8b cf 48 89 45 ?? 4c 8b e0 e8 ?? ?? ?? ?? 4c 8b f0 4d 39 af ?? ?? 00 00",
};

/// `UBrickGameInstance::QueryUGC(const FUGCQuerySettings&, FFluUGCAsyncTaskHandle&)`, by its Local
/// branch's paging: `PageSize * Page` items are dropped from the front. The match also tells where
/// the settings keep the page size ([`PAGE_SIZE_AT`]) and the page ([`PAGE_AT`]).
pub const QUERY: FunctionSig = FunctionSig {
    name: "UBrickGameInstance::QueryUGC",
    pattern: "44 8b eb 8b 4e ?? 8b d3 44 8b f9 44 0f af 7e ?? 41 2b d7",
};
/// Where [`QUERY`]'s match holds the settings' offset of the page size (`mov ecx, [rsi + size]`).
const PAGE_SIZE_AT: usize = 5;
/// Where [`QUERY`]'s match holds the settings' offset of the page (`imul r15d, [rsi + page]`).
const PAGE_AT: usize = 15;

/// `UUGCBrowserWidget::OnUGCQueryComplete(const TArray<FUGCQueryResult>&, uint32 numEntries,
/// uint32 numPages)`, by its start: whether the paged list exists, then the arguments kept.
pub const QUERY_COMPLETE: FunctionSig = FunctionSig {
    name: "UUGCBrowserWidget::OnUGCQueryComplete",
    pattern: "41 54 41 55 41 56 41 57 48 83 ec 28 48 83 b9 ?? ?? 00 00 00 45 8b e1 45 8b e8 4c 8b f2 4c 8b f9 0f 84",
};

/// `UPagedListWidget::OnEntryClicked(UPagedListEntryWidget* entry, bool bDoubleClicked)` (BRMK
/// 0x442D60, game 0xF2FE40), by its start: keeps both arguments, then a single click on the
/// selected entry clears the selection (offset of the selection wildcarded). Every click on a row
/// of any paged list (vehicle browser, server browser, bans) comes here, before the list selects
/// the row and tells its owner.
pub const ENTRY_CLICKED: FunctionSig = FunctionSig {
    name: "UPagedListWidget::OnEntryClicked",
    pattern: "41 0f b6 e8 48 8b da 48 8b f1 45 84 c0 75 ?? 48 8d 91 ?? ?? 00 00 48 8b cb e8",
};

/// `UBrickGameInstance::DeleteUGCItem(const FUGCFileInfo&) -> bool` (BRMK 0x230160, game
/// 0xDD8090), by its start: refuses an info whose string at +0x10 is empty, then builds the item's
/// folder (`<UGC folder>/.../<LocalItemId>`, game 0xCE4C10) and deletes it **with everything in
/// it** (`IFileManager::DeleteDirectory(path, bRequireExists, bTree)`, vtable +0x60, both true),
/// then reloads its thumbnail. The browser's Delete button reaches it from its confirmation popup
/// (`OnDeleteItemPopupClosed`) with the selected row's entry.
pub const DELETE_ITEM: FunctionSig = FunctionSig {
    name: "UBrickGameInstance::DeleteUGCItem",
    pattern: "83 7a 18 01 48 8b da 48 8b f9 0f 8e ?? ?? 00 00 48 8d 4c 24 20 e8",
};

pub type DeleteItemFn = unsafe extern "C" fn(game_instance: *mut u8, info: *const u8) -> u8;

/// `UUGCBrowserWidget::ReflectBrickProperties` (BRMK 0x3E6630, game 1.11.8 0xEF11E0): the rows of
/// the left panel while no vehicle is selected, in one untitled category: `UGCQueryType`
/// (category), `UGCSortMethod`, `UGCSearchText`, ... Found by its sort row's name, a string table
/// key only it loads. `this` is the browser's `IBrickPropertyInterface`, as for every
/// `ReflectBrickProperties`.
pub const REFLECT_PANEL: StringRefSig = StringRefSig {
    name: "UUGCBrowserWidget::ReflectBrickProperties",
    text: "Name_Property_SortMethod",
};

/// `EUGCQueryType`, the browser's categories (byte 0 of the query settings).
pub mod query_type {
    pub const LOCAL: u8 = 0;
    pub const SUBSCRIBED: u8 = 3;
    /// Given items' details (the server's vehicle whitelist uses it).
    pub const SELECTION: u8 = 6;
}

pub type LoadPageFn = unsafe extern "C" fn(browser: *mut u8, page: i32);
/// Returns the game's `bool` in its lowest byte.
pub type QueryFn =
    unsafe extern "C" fn(game_instance: *mut u8, settings: *const u8, task: *mut u8) -> u8;
pub type QueryCompleteFn =
    unsafe extern "C" fn(browser: *mut u8, results: *const ResultArray, entries: u32, pages: u32);
/// `double_click` is the game's `bool` (lowest byte).
pub type EntryClickedFn =
    unsafe extern "C" fn(list: *mut UObjectBase, entry: *mut UObjectBase, double_click: u8);

/// A `TArray<FUGCQueryResult>`'s header. The elements have the reflected `FUGCFileInfo`'s layout
/// (0xE8 bytes: BRMK and game copy and destroy result arrays in steps of that size).
#[repr(C)]
pub struct ResultArray {
    pub data: *mut u8,
    pub num: i32,
    pub max: i32,
}

/// What a query asks for: the parts of an `FUGCQuerySettings` we read (not reflected: bytes 0..3
/// from `OnLoadListPage`'s code, the page fields from `QueryUGC`'s).
#[derive(Clone, Copy)]
pub struct QuerySettings {
    pub query_type: u8,
    pub sort: u8,
    pub page: i32,
    pub page_size: i32,
}

/// Where the page fields are in an `FUGCQuerySettings`.
#[derive(Clone, Copy)]
struct PageFields {
    page_at: usize,
    page_size_at: usize,
}

static PAGE_FIELDS: OnceLock<PageFields> = OnceLock::new();

/// Reads `settings`. Only after [`hook_query`] (which finds the page fields).
///
/// # Safety
/// `settings` must be a live `FUGCQuerySettings`.
pub unsafe fn read_settings(settings: *const u8) -> QuerySettings {
    let fields = PAGE_FIELDS.get().expect("page fields found before use");
    // SAFETY: the caller's contract; the offsets come from the game's own code reading them.
    unsafe {
        QuerySettings {
            query_type: *settings,
            sort: *settings.add(2),
            page: settings.add(fields.page_at).cast::<i32>().read_unaligned(),
            page_size: settings
                .add(fields.page_size_at)
                .cast::<i32>()
                .read_unaligned(),
        }
    }
}

/// Sets the page and page size `settings` ask for. Only after [`hook_query`].
///
/// # Safety
/// `settings` must be a live, writable `FUGCQuerySettings` (the caller's own, as `OnLoadListPage`
/// builds it on its stack).
pub unsafe fn write_page(settings: *mut u8, page: i32, page_size: i32) {
    let fields = PAGE_FIELDS.get().expect("page fields found before use");
    // SAFETY: the caller's contract; the offsets come from the game's own code reading them.
    unsafe {
        settings
            .add(fields.page_at)
            .cast::<i32>()
            .write_unaligned(page);
        settings
            .add(fields.page_size_at)
            .cast::<i32>()
            .write_unaligned(page_size);
    }
}

/// Where an `FUGCQuerySettings` keeps the items a `Selection` query asks for: a
/// `TArray<TSharedRef<FFluUGCItemId>>` right after the three type bytes (BRMK 0x3DDCDA fills it,
/// FluUGCSteam's `QueryUGC` copies it into its task). Not reflected nor in code we match, so only
/// used while the settings are laid out as in BRMK: the page fields at 0x40 and 0x44.
const ITEMS_AT: usize = 8;
const BRMK_PAGE_FIELDS: (usize, usize) = (0x40, 0x44);

/// A `TArray`'s header.
#[repr(C)]
#[derive(Clone, Copy)]
struct ArrayHeader {
    data: *mut u8,
    num: i32,
    max: i32,
}

/// What [`ask_for_items`] changed in the settings, to put back.
pub struct AskedItems {
    query_type: u8,
    items: ArrayHeader,
}

/// Whether queries can be made to ask for given items ([`ask_for_items`]). Only after
/// [`hook_query`].
pub fn can_ask_for_items() -> bool {
    PAGE_FIELDS
        .get()
        .is_some_and(|fields| (fields.page_at, fields.page_size_at) == BRMK_PAGE_FIELDS)
}

/// Makes `settings` a `Selection` query of `ids` (16-byte shared references to the game's item
/// ids, which the query copies), if it asks for no items now. Put back with [`put_back_items`]
/// before the settings' owner sees them again, which keeps the game from destroying `ids`.
///
/// # Safety
/// `settings` must be a live, writable `FUGCQuerySettings` (`OnLoadListPage`'s own, as for
/// [`write_page`]), [`can_ask_for_items`] true, and `ids` alive until put back. Game thread only.
pub unsafe fn ask_for_items<T>(settings: *mut u8, ids: &[T]) -> Option<AskedItems> {
    const { assert!(size_of::<T>() == 16) };
    // SAFETY: per the contract; the array is where the game's own code keeps it.
    unsafe {
        let field = settings.add(ITEMS_AT).cast::<ArrayHeader>();
        let before = field.read_unaligned();
        if before.num != 0 || before.max < 0 {
            return None;
        }
        let count = i32::try_from(ids.len()).ok()?;
        field.write_unaligned(ArrayHeader {
            data: ids.as_ptr().cast_mut().cast(),
            num: count,
            max: count,
        });
        let asked = AskedItems {
            query_type: *settings,
            items: before,
        };
        *settings = query_type::SELECTION;
        Some(asked)
    }
}

/// Puts back what [`ask_for_items`] changed.
///
/// # Safety
/// `settings` must be the settings `asked` came from, still alive. Game thread only.
pub unsafe fn put_back_items(settings: *mut u8, asked: AskedItems) {
    // SAFETY: per the contract.
    unsafe {
        settings
            .add(ITEMS_AT)
            .cast::<ArrayHeader>()
            .write_unaligned(asked.items);
        *settings = asked.query_type;
    }
}

/// Hooks `OnLoadListPage` with `detour`, the original stored in `original`.
pub fn hook_load_page(detour: LoadPageFn, original: &OnceLock<LoadPageFn>) -> Result<(), String> {
    let target = LOAD_PAGE.resolve()?;
    // SAFETY: `target` starts the function the signature names, and `detour` has its signature.
    unsafe { hook::install_typed(LOAD_PAGE.name, target, detour, original) }
}

/// Hooks `QueryUGC` with `detour`, the original stored in `original`; finds the page fields.
pub fn hook_query(detour: QueryFn, original: &OnceLock<QueryFn>) -> Result<(), String> {
    let found = QUERY.locate()?;
    // SAFETY: both offsets are inside the matched pattern (one-byte displacements).
    let fields = unsafe {
        PageFields {
            page_at: usize::from(*found.address.add(PAGE_AT)),
            page_size_at: usize::from(*found.address.add(PAGE_SIZE_AT)),
        }
    };
    let _ = PAGE_FIELDS.set(fields);
    // SAFETY: `found.function` starts `QueryUGC` (the match is inside it), and `detour` has its
    // signature.
    unsafe { hook::install_typed(QUERY.name, found.function, detour, original) }
}

/// Hooks `OnUGCQueryComplete` with `detour`, the original stored in `original`.
pub fn hook_query_complete(
    detour: QueryCompleteFn,
    original: &OnceLock<QueryCompleteFn>,
) -> Result<(), String> {
    let target = QUERY_COMPLETE.resolve()?;
    // SAFETY: `target` starts the function the signature names, and `detour` has its signature.
    unsafe { hook::install_typed(QUERY_COMPLETE.name, target, detour, original) }
}

/// `OnUGCQueryComplete`'s copy of the results into the browser: the copy constructor's call, then
/// both pointers step by the row size (the immediates), then the count goes down.
const COPY_LOOP: CodeShape = CodeShape {
    name: "OnUGCQueryComplete's copy loop",
    shape: "e8 ?? ?? ?? ?? 48 81 c3 ?? ?? ?? ?? 48 81 c7 ?? ?? ?? ?? 83 ed 01",
};
/// Where [`COPY_LOOP`] holds the target's step (`add rbx, size`).
const COPY_LOOP_STEP: usize = 8;
/// How far into `OnUGCQueryComplete` the loop is looked for (game 1.11.8: at +0x97).
const COPY_LOOP_WITHIN: usize = 0x140;

/// The distance between rows in the game's result arrays, read from `OnUGCQueryComplete`'s code
/// (the struct isn't reflected under that name; the item widget's reflected copy must match it).
pub fn result_stride() -> Result<usize, String> {
    let function = QUERY_COMPLETE.resolve()?;
    let at = (0..COPY_LOOP_WITHIN)
        .map(|offset| function.wrapping_add(offset))
        .find(|&address| COPY_LOOP.matches(address))
        .ok_or_else(|| format!("{} not found", COPY_LOOP.name))?;
    // SAFETY: the shape matched covers the immediate.
    let step = unsafe { at.add(COPY_LOOP_STEP).cast::<u32>().read_unaligned() };
    Ok(step as usize)
}

/// Hooks `UPagedListWidget::OnEntryClicked` with `detour`, the original stored in `original`.
/// Hooks the exec thunk of `bool UPagedListWidget::ClearSelectedEntryWidget()` (reflected): what
/// the details header's X calls (`WBP_PagedList`, bound to the header's `OnClickedClear`), as do
/// Blueprints clearing a list's selection. After engine init (reflection).
pub fn hook_clear_selection(
    detour: NativeThunk,
    original: &OnceLock<NativeThunk>,
) -> Result<(), String> {
    const NAME: &str = "UPagedListWidget::ClearSelectedEntryWidget";
    let class =
        reflection::find_class("PagedListWidget").ok_or("class PagedListWidget not found")?;
    let function = reflection::find_function(class, "ClearSelectedEntryWidget")?;
    let thunk = reflection::native_thunk(function).map_err(|reason| format!("{NAME}: {reason}"))?;
    // SAFETY: `thunk` is the function's exec thunk, which has the thunks' signature.
    unsafe { hook::install_typed(NAME, thunk, detour, original) }
}

pub fn hook_entry_clicked(
    detour: EntryClickedFn,
    original: &OnceLock<EntryClickedFn>,
) -> Result<(), String> {
    let found = ENTRY_CLICKED.locate()?;
    // SAFETY: `found.function` starts the function the match is in (the pattern starts 0x14 bytes
    // into it), and `detour` has its signature.
    unsafe { hook::install_typed(ENTRY_CLICKED.name, found.function, detour, original) }
}

/// Hooks the browser's [`REFLECT_PANEL`] with `detour`, the original stored in `original`.
pub fn hook_reflect_panel(detour: ReflectFn, original: &OnceLock<ReflectFn>) -> Result<(), String> {
    let target = REFLECT_PANEL.resolve()?;
    // SAFETY: `target` starts the function the string is loaded in, and every
    // `ReflectBrickProperties` has `detour`'s signature.
    unsafe { hook::install_typed(REFLECT_PANEL.name, target, detour, original) }
}

/// Hooks `UBrickGameInstance::DeleteUGCItem` with `detour`, the original stored in `original`.
pub fn hook_delete_item(
    detour: DeleteItemFn,
    original: &OnceLock<DeleteItemFn>,
) -> Result<(), String> {
    let found = DELETE_ITEM.locate()?;
    // SAFETY: `found.function` starts the function the match is in (the pattern starts 0xA bytes
    // into it), and `detour` has its signature.
    unsafe { hook::install_typed(DELETE_ITEM.name, found.function, detour, original) }
}

/// `UUGCBrowserWidget::UGCTags`, the filter the left panel's Department, Era and Type boxes set.
///
/// In 1.11 it is `FUGCTags`, three bytes, one per box, all 0 for "any" (BRMK
/// `UUGCTagsPropertyWidget::ClearTags`, 0x3ABCB0, passes a zeroed word and byte). Reading it as a
/// `TArray` read past its end, whatever field followed, and the list lost its folders when that
/// wasn't 0.
#[derive(Clone, Copy)]
enum Tags {
    /// A `TArray` of tags: filtering when it holds any.
    Array { offset: usize },
    /// A small struct of one byte per filter: filtering when any isn't 0.
    Bytes { offset: usize, size: usize },
}

/// The largest tag struct read byte by byte: a few filters, too small to hold an array.
const MOST_TAG_BYTES: usize = 8;

/// The browser widget's and its paged list's reflected parts (after engine init).
pub struct Browsers {
    class: *const UClass,
    /// `UUGCBrowserWidget::PagedList` (`UPagedListWidget*`).
    paged_list: usize,
    /// `UUGCBrowserWidget::UGCSearchText` (`FString`).
    search_text: usize,
    /// `UUGCBrowserWidget::UGCTags`: the tag filter.
    tags: Tags,
    /// `UPagedListWidget::SetCurrentPage(int32)`: moves the page selector, loads nothing.
    set_current_page: SingleCall,
    /// `UPagedListWidget::RefreshEntries()`: asks the owner for the selector's page again.
    refresh_entries: Function,
    /// `UUGCBrowserWidget::UGCQueryType` (`EUGCQueryType`, one byte): the category shown.
    query_type: usize,
    /// `UPagedListWidget::EntryWidgets`: the page's row widgets (`TArray`).
    entry_widgets: usize,
    /// `UPopupWidget::GetPopupContainer()`: null once the popup is closed.
    popup_container: Getter<*mut UObjectBase>,
    /// `UPagedListWidget::GetSelectedEntryWidget()`: the row the game has selected.
    selected_entry: Getter<*mut UObjectBase>,
    /// `UPopupWidget::GetButtonPanel()`: the left panel the browser makes its buttons in.
    button_panel: Getter<*mut UObjectBase>,
    /// `UUGCBrowserWidget::OnListEntrySelected(entry, bDoubleClicked)`: reads the list's selection
    /// (not its argument), then `UpdateButtons()`; a double click spawns or opens it.
    entry_selected: Function,
    /// `UPagedListWidget::InitializePropertiesPanel(UObject* Owner)` (BRMK 0x43A010): sets the
    /// panel's owner, then `UpdateSelectedEntryDetails()`, which fills the side panel from the
    /// selected row (its info) or else the owner (the browser: category, sort, search). `None` if
    /// not found (logged): the panel then changes only with the game's own selection.
    initialize_panel: Option<SingleCall>,
}

// SAFETY: reflection records and offsets living as long as the game; only used on the game thread.
unsafe impl Send for Browsers {}
unsafe impl Sync for Browsers {}

impl Browsers {
    pub fn find() -> Result<Self, String> {
        let class =
            reflection::find_class("UGCBrowserWidget").ok_or("class UGCBrowserWidget not found")?;
        let list_class =
            reflection::find_class("PagedListWidget").ok_or("class PagedListWidget not found")?;
        let popup_class =
            reflection::find_class("PopupWidget").ok_or("class PopupWidget not found")?;
        let string = size_of::<FString>() as i32;
        // SAFETY: a live class record; property records live as long as it.
        let query_type = unsafe {
            let property = reflection::find_property(class.cast(), "UGCQueryType");
            if property.is_null() || (*property).element_size != 1 {
                return Err("field 'UGCQueryType' not found as one byte".into());
            }
            (*property).offset_internal as usize
        };
        let entry_selected = Function::find(class, "OnListEntrySelected")?;
        // Both parameters are read as zero (no entry, no double click): checked to be those.
        entry_selected.parameter_kinds(&["ObjectProperty", "BoolProperty"])?;
        // SAFETY: a live class record; property records live as long as it.
        let tags = unsafe {
            let property = reflection::find_property(class.cast(), "UGCTags");
            if property.is_null() {
                return Err("field 'UGCTags' not found".into());
            }
            let offset = (*property).offset_internal as usize;
            let size = usize::try_from((*property).element_size).unwrap_or(0);
            if reflection::is_kind(property, "ArrayProperty") {
                Tags::Array { offset }
            } else if reflection::is_kind(property, "StructProperty")
                && (1..=MOST_TAG_BYTES).contains(&size)
            {
                Tags::Bytes { offset, size }
            } else {
                return Err(format!(
                    "field 'UGCTags' is neither an array nor a struct of at most \
                     {MOST_TAG_BYTES} bytes ({size} bytes)"
                ));
            }
        };
        Ok(Self {
            class,
            paged_list: reflection::field_offset(class, "PagedList", "ObjectProperty", 8)?,
            search_text: reflection::field_offset(class, "UGCSearchText", "StrProperty", string)?,
            tags,
            set_current_page: SingleCall::find_only(
                list_class,
                "SetCurrentPage",
                &["IntProperty"],
                4,
            )?,
            refresh_entries: Function::find(list_class, "RefreshEntries")?,
            query_type,
            entry_widgets: reflection::field_offset(
                list_class,
                "EntryWidgets",
                "ArrayProperty",
                0x10,
            )?,
            popup_container: Getter::find(popup_class, "GetPopupContainer", "ObjectProperty")?,
            selected_entry: Getter::find(list_class, "GetSelectedEntryWidget", "ObjectProperty")?,
            button_panel: Getter::find(popup_class, "GetButtonPanel", "ObjectProperty")?,
            entry_selected,
            initialize_panel: SingleCall::find_only(
                list_class,
                "InitializePropertiesPanel",
                &["ObjectProperty"],
                8,
            )
            .map_err(|reason| log!("vehicle browser: the side panel isn't refreshed ({reason})"))
            .ok(),
        })
    }

    /// Fills the side panel again: the selected row's info, or else what the browser lists
    /// (`ReflectBrickProperties`).
    ///
    /// # Safety
    /// `browser` must be a live vehicle browser. Game thread only.
    pub unsafe fn refresh_panel(&self, browser: *mut UObjectBase) {
        // SAFETY: per the contract; the function takes the panel's owner, the browser it already
        // has.
        unsafe {
            if let (Some(initialize), Some(list)) =
                (&self.initialize_panel, self.paged_list(browser))
            {
                initialize.call(list, browser);
            }
        }
    }

    /// The category the browser shows (`query_type`).
    ///
    /// # Safety
    /// `browser` must be a live vehicle browser. Game thread only.
    pub unsafe fn query_type(&self, browser: *const UObjectBase) -> u8 {
        // SAFETY: per the contract; the offset is reflected.
        unsafe { browser.cast::<u8>().add(self.query_type).read() }
    }

    /// The row widget the game has selected, if any.
    ///
    /// # Safety
    /// `browser` must be a live vehicle browser. Game thread only.
    pub unsafe fn selected_entry(&self, browser: *const UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; the getter returns a live widget or null.
        unsafe {
            let list = self.paged_list(browser)?;
            let entry = self.selected_entry.call(list);
            (!entry.is_null()).then_some(entry)
        }
    }

    /// The panel the browser makes its buttons in, if it has one now.
    ///
    /// # Safety
    /// `browser` must be a live vehicle browser. Game thread only.
    pub unsafe fn button_panel(&self, browser: *mut UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; the getter returns a live panel or null.
        let panel = unsafe { self.button_panel.call(browser) };
        (!panel.is_null()).then_some(panel)
    }

    /// Makes the browser's buttons again, for what is selected now.
    ///
    /// # Safety
    /// `browser` must be a live vehicle browser. Game thread only.
    pub unsafe fn update_buttons(&self, browser: *mut UObjectBase) {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; zero parameters are no entry and a single click (checked in
        // `find`), and the function reads the list's own selection.
        unsafe { self.entry_selected.call(browser, &mut parameters) };
    }

    /// Asks for the page shown again (after a change to what it lists).
    ///
    /// # Safety
    /// `browser` must be a live vehicle browser. Game thread only.
    pub unsafe fn refresh(&self, browser: *const UObjectBase) {
        // SAFETY: per the contract; the function belongs to the paged list's class.
        unsafe {
            if let Some(list) = self.paged_list(browser) {
                let mut parameters = Parameters::new();
                self.refresh_entries.call(list, &mut parameters);
            }
        }
    }

    /// Whether `object` is a vehicle browser.
    ///
    /// # Safety
    /// `object` must be a live object.
    pub unsafe fn is_browser(&self, object: *const UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { reflection::is_a(object, self.class.cast()) }
    }

    /// The browser's paged list, if it has one.
    ///
    /// # Safety
    /// `browser` must be a live vehicle browser. Game thread only.
    pub unsafe fn paged_list(&self, browser: *const UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; the offset is reflected.
        let list = unsafe {
            browser
                .cast::<u8>()
                .add(self.paged_list)
                .cast::<*mut UObjectBase>()
                .read()
        };
        (!list.is_null()).then_some(list)
    }

    /// The row widgets of the page `list` shows.
    ///
    /// # Safety
    /// `list` must be the browser's live paged list, and the result only used until it changes
    /// its rows. Game thread only.
    pub unsafe fn entry_widgets(&self, list: *mut UObjectBase) -> Vec<*mut UObjectBase> {
        // SAFETY: per the contract; the field is a reflected array of widget pointers.
        unsafe {
            let array = &*list
                .cast::<u8>()
                .add(self.entry_widgets)
                .cast::<brickrust::ue::tarray::TArray<*mut UObjectBase>>();
            match usize::try_from(array.num) {
                Ok(count) if count > 0 && !array.data.is_null() => {
                    core::slice::from_raw_parts(array.data, count)
                        .iter()
                        .copied()
                        .filter(|widget| !widget.is_null())
                        .collect()
                }
                _ => Vec::new(),
            }
        }
    }

    /// Whether the browser is open (a popup in a container).
    ///
    /// # Safety
    /// `browser` must be a live vehicle browser. Game thread only.
    pub unsafe fn is_open(&self, browser: *mut UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { !self.popup_container.call(browser).is_null() }
    }

    /// Whether the browser filters by a search text or tags (it then lists matches from every
    /// folder).
    ///
    /// # Safety
    /// `browser` must be a live vehicle browser. Game thread only.
    pub unsafe fn is_filtering(&self, browser: *const UObjectBase) -> bool {
        // SAFETY: per the contract; both offsets and the tags' size are reflected, and the string
        // is the browser's own (read, not freed).
        unsafe {
            let base = browser.cast::<u8>();
            let text = (*base.add(self.search_text).cast::<FString>()).to_string();
            let tags = match self.tags {
                // A `TArray` header: its count is at +8.
                Tags::Array { offset } => base.add(offset + 8).cast::<i32>().read() > 0,
                // One byte per filter, 0 for any.
                Tags::Bytes { offset, size } => core::slice::from_raw_parts(base.add(offset), size)
                    .iter()
                    .any(|&byte| byte != 0),
            };
            !text.trim().is_empty() || tags
        }
    }

    /// Shows the first page again, asked from the game as a page change is (`OnLoadListPage`).
    ///
    /// # Safety
    /// `browser` must be a live vehicle browser. Game thread only.
    pub unsafe fn show_first_page(&self, browser: *const UObjectBase) {
        // SAFETY: per the contract; both functions belong to the paged list's class and take what
        // they're given.
        unsafe {
            let Some(list) = self.paged_list(browser) else {
                return;
            };
            self.set_current_page.call(list, 0i32);
            let mut parameters = Parameters::new();
            self.refresh_entries.call(list, &mut parameters);
        }
    }
}
