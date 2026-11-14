//! The vehicle browser's rows: the items it lists (`FUGCQueryResult`, the reflected `FUGCFileInfo`'s
//! fields and a C++ vtable) and the widget showing each (`UUGCItemWidget`, Blueprint `WBP_UGCItem`).
//! See `docs/vehicle-menu.md`, "Folder tiles".
//!
//! A row widget is filled by `UUGCItemWidget::InitializeEntry_Implementation` (BRMK 0x3CC8C0, a
//! parameterless `BlueprintNativeEvent`, so found by name): `NameTextBlock` gets the entry's display
//! name, `Image` a thumbnail request, then `UpdateItemWidget()` (native, found by its code in
//! `ugc_autosave`) passes the selected state and autosave flags to the Blueprint.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::framework::sig;
use crate::game::reflection::{self, Function, Parameters, SingleCall};
use crate::game::slider_text::FString;
use crate::game::text::Text;
use crate::game::weak_object::WeakObject;
use crate::game::{assets, input_table, ugc_autosave};

/// The folder icon the game's input help shows for its categories (`WBP_InputHelpCategory`).
/// The big icon a tile shows when it shows no thumbnails.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TileIcon {
    Folder,
    /// The "up" tile (`..`).
    Up,
}

impl TileIcon {
    fn path(self) -> &'static str {
        match self {
            TileIcon::Folder => "/Game/BrickRigs/UI/Textures/Icons/T_Folder.T_Folder",
            TileIcon::Up => "/Game/BrickRigs/UI/Textures/Icons/T_UpKey.T_UpKey",
        }
    }
}

/// `virtual void UStruct::InitializeStruct(void* Dest, int32 ArrayDim) const`.
pub type InitializeStructFn =
    unsafe extern "C" fn(ustruct: *const UStruct, target: *mut u8, count: i32);

/// `ustruct`'s `InitializeStruct`, to construct one as the game does (its C++ constructor's
/// defaults).
pub fn struct_initializer(ustruct: *const UStruct) -> Result<InitializeStructFn, String> {
    let slot = input_table::initialize_struct_slot()?;
    // SAFETY: `ustruct` is a live struct record, whose vtable has the slot (every `UStruct`'s
    // has); the function found is checked to be game code.
    unsafe {
        let vtable = ustruct.cast::<*const *const u8>().read();
        let function = vtable.add(slot / size_of::<usize>()).read();
        if !sig::is_function_start(function) {
            return Err(format!("InitializeStruct at {function:p} isn't a function"));
        }
        Ok(core::mem::transmute::<*const u8, InitializeStructFn>(
            function,
        ))
    }
}

/// `void UUGCItemWidget::InitializeEntry_Implementation()` and `UpdateItemWidget()`.
pub type WidgetFn = unsafe extern "C" fn(widget: *mut UObjectBase);

/// The rows' struct (`FUGCQueryResult`, the item widget's `Entry`): its size and the fields we use.
pub struct Rows {
    ustruct: *const UStruct,
    /// Bytes per row, also the distance between rows in the game's arrays.
    pub size: usize,
    /// `LocalItemId` (`FString`): the vehicle's folder name on this computer.
    local_id: usize,
    /// `Title` (`FString`).
    title: usize,
    /// `TimeCreated` and `TimeUpdated` (`FDateTime`s), or why they aren't read.
    times: Result<(usize, usize), String>,
    initialize: InitializeStructFn,
    /// `CopyScriptStruct`, or why it isn't found (only the Subscribed list keeps copies).
    copy: Result<input_table::CopyScriptStructFn, String>,
}

// SAFETY: reflection records and game code living as long as the game; only used on the game
// thread.
unsafe impl Send for Rows {}
unsafe impl Sync for Rows {}

impl Rows {
    fn find(ustruct: *const UStruct) -> Result<Self, String> {
        let string = size_of::<FString>() as i32;
        let initialize = struct_initializer(ustruct)?;
        Ok(Self {
            ustruct,
            // SAFETY: a live, linked struct.
            size: unsafe { reflection::struct_size(ustruct) },
            local_id: reflection::struct_field_offset(
                ustruct,
                "LocalItemId",
                "StrProperty",
                string,
            )?,
            title: reflection::struct_field_offset(ustruct, "Title", "StrProperty", string)?,
            times: date_time_field(ustruct, "TimeCreated")
                .and_then(|created| Ok((created, date_time_field(ustruct, "TimeUpdated")?))),
            initialize,
            copy: input_table::copy_script_struct(),
        })
    }

    /// The rows' struct (`UGCFileInfo`).
    pub fn ustruct(&self) -> *const UStruct {
        self.ustruct
    }

    /// Why rows' times can't be read, if they can't.
    pub fn no_times(&self) -> Option<&str> {
        self.times.as_ref().err().map(String::as_str)
    }

    /// The row's `TimeCreated` and `TimeUpdated` as unix seconds (UTC), if they're read.
    ///
    /// # Safety
    /// As for [`Rows::local_id`].
    pub unsafe fn times(&self, row: *const u8) -> Option<(i64, i64)> {
        let &(created, updated) = self.times.as_ref().ok()?;
        // SAFETY: per the contract; both fields are 8-byte `FDateTime`s (checked at start).
        unsafe {
            Some((
                unix_seconds(row.add(created).cast::<i64>().read_unaligned()),
                unix_seconds(row.add(updated).cast::<i64>().read_unaligned()),
            ))
        }
    }

    /// The row's `LocalItemId`.
    ///
    /// # Safety
    /// `row` must be a live row. Game thread only.
    pub unsafe fn local_id(&self, row: *const u8) -> String {
        // SAFETY: per the contract; the string is the row's own (read, not freed).
        unsafe { (*row.add(self.local_id).cast::<FString>()).to_string() }
    }

    /// The row's `Title`.
    ///
    /// # Safety
    /// As for [`Rows::local_id`].
    pub unsafe fn title(&self, row: *const u8) -> String {
        // SAFETY: as above.
        unsafe { (*row.add(self.title).cast::<FString>()).to_string() }
    }

    /// A new row of ours, constructed as the game constructs one, in the game's memory. Never
    /// destroyed: the caller keeps and reuses it (the game only copies rows it's given).
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn new_row(&self) -> Result<*mut u8, String> {
        // SAFETY: `InitializeStruct` constructs one struct of its size in memory holding that many.
        unsafe {
            let memory = brickrust::ue::fmalloc::malloc(self.size).cast::<u8>();
            if memory.is_null() {
                return Err("out of memory".into());
            }
            (self.initialize)(self.ustruct, memory, 1);
            Ok(memory)
        }
    }

    /// Why rows can't be copied, if they can't.
    pub fn no_copy(&self) -> Option<&str> {
        self.copy.as_ref().err().map(String::as_str)
    }

    /// A copy of `row` of ours, made as the game copies rows (its strings and item id its own).
    /// Never destroyed, as for [`Rows::new_row`].
    ///
    /// # Safety
    /// `row` must be a live row. Game thread only.
    pub unsafe fn copy_row(&self, row: *const u8) -> Result<*mut u8, String> {
        let copy = *self.copy.as_ref().map_err(Clone::clone)?;
        // SAFETY: per the contract; `made` is a constructed row of the same struct.
        unsafe {
            let made = self.new_row()?;
            copy(self.ustruct, made, row, 1);
            Ok(made)
        }
    }

    /// Sets the `LocalItemId` of a row of ours.
    ///
    /// # Safety
    /// `row` must come from [`Rows::new_row`] and not be in use by the game. Game thread only.
    pub unsafe fn set_local_id(&self, row: *mut u8, text: &str) {
        // SAFETY: per the contract.
        unsafe { replace_string(row.add(self.local_id), text) }
    }

    /// Sets the `Title` of a row of ours.
    ///
    /// # Safety
    /// As for [`Rows::set_local_id`].
    pub unsafe fn set_title(&self, row: *mut u8, text: &str) {
        // SAFETY: per the contract.
        unsafe { replace_string(row.add(self.title), text) }
    }
}

/// The offset of `name` in `ustruct`, checked to be an `FDateTime` (a `StructProperty` of the
/// struct `DateTime`, 8 bytes: ticks).
fn date_time_field(ustruct: *const UStruct, name: &'static str) -> Result<usize, String> {
    let property = reflection::struct_field(ustruct, name, "StructProperty", 8)?;
    // SAFETY: a live StructProperty record; its struct lives as long as it.
    let is_date_time = unsafe {
        let date_time = reflection::struct_type(property);
        !date_time.is_null()
            && reflection::name_is(&(*date_time).ufield.uobject.name_private, "DateTime")
    };
    if !is_date_time {
        return Err(format!("field '{name}' isn't an FDateTime"));
    }
    // SAFETY: as above.
    Ok(unsafe { (*property).offset_internal as usize })
}

/// An `FDateTime`'s ticks (100 ns since 0001-01-01) as unix
/// seconds.
fn unix_seconds(ticks: i64) -> i64 {
    /// `FDateTime(1970, 1, 1).GetTicks()`.
    const UNIX_EPOCH_TICKS: i64 = 621_355_968_000_000_000;
    const TICKS_PER_SECOND: i64 = 10_000_000;
    (ticks - UNIX_EPOCH_TICKS).div_euclid(TICKS_PER_SECOND)
}

/// Puts a new string of the game's allocator in the `FString` at `field`, freeing the old one.
///
/// # Safety
/// `field` must be a live `FString` whose characters come from the game's allocator. Game thread
/// only.
unsafe fn replace_string(field: *mut u8, text: &str) {
    // SAFETY: per the contract; the old string is given up once replaced.
    unsafe {
        let old = field.cast::<FString>().replace(FString::allocated(text));
        old.free();
    }
}

/// The row widgets' reflected parts and the functions that fill them (after engine init).
pub struct ItemWidgets {
    class: *const UClass,
    /// `UUGCItemWidget::Entry`: the widget's copy of its row.
    entry: usize,
    /// `UUGCItemWidget::Image` (`UBrickImage*`) and `NameTextBlock` (`UBrickTextBlock*`).
    image: usize,
    name: usize,
    /// `UTextBlock::SetText(FText)`, and its parameter's offset.
    set_text: Function,
    text: usize,
    /// `UImage::SetBrushFromTexture(UTexture2D* Texture, bool bMatchSize)`.
    set_brush: Function,
    brush_texture: usize,
    /// `UPagedListEntryWidget::Button` and its class's `UBrickButtonWidget::SetIsSelected(bool)`:
    /// what `WBP_UGCItem` calls to show the row (un)selected (the square behind it).
    button: usize,
    button_class: *const UClass,
    set_selected: SingleCall,
    texture_class: *const UClass,
    /// The tiles' icons, once loaded (a shown brush keeps its texture alive; reloaded if it went).
    icons: std::sync::Mutex<Vec<(TileIcon, WeakObject)>>,
    /// Where `Image`'s brush texture is in a row widget's image (`UImage::Brush`, then
    /// `FSlateBrush::ResourceObject`), and `UImage::SetBrushResourceObject(UObject*)`.
    brush_resource: usize,
    set_brush_resource: SingleCall,
    /// `Image`'s own brush texture (`WBP_UGCItem`'s "No Image"), read before a tile first put an
    /// icon there (`None` inside: it had none). A vehicle without a thumbnail shows that brush, so
    /// a row widget that showed a tile gets it back (`show_own_image`).
    default_image: std::sync::OnceLock<Option<WeakObject>>,
    /// `UUGCItemWidget::InitializeEntry_Implementation`, to hook.
    pub initialize_entry: *const u8,
    update_item: WidgetFn,
    pub rows: Rows,
}

// SAFETY: reflection records and game code living as long as the game; only used on the game
// thread.
unsafe impl Send for ItemWidgets {}
unsafe impl Sync for ItemWidgets {}

impl ItemWidgets {
    pub fn find() -> Result<Self, String> {
        let class =
            reflection::find_class("UGCItemWidget").ok_or("class UGCItemWidget not found")?;
        let text_block = reflection::find_class("TextBlock").ok_or("class TextBlock not found")?;
        let image = reflection::find_class("Image").ok_or("class Image not found")?;
        let entry_class = reflection::find_class("PagedListEntryWidget")
            .ok_or("class PagedListEntryWidget not found")?;
        let texture_class =
            reflection::find_class("Texture2D").ok_or("class Texture2D not found")?;
        // SAFETY: a live class record; property records live as long as it.
        let (entry, row_struct) = unsafe {
            let property = reflection::find_property(class.cast(), "Entry");
            let row = if property.is_null() {
                core::ptr::null()
            } else {
                reflection::struct_type(property)
            };
            if row.is_null() {
                return Err("UGCItemWidget::Entry isn't a struct field".into());
            }
            ((*property).offset_internal as usize, row)
        };
        let set_text = Function::find(text_block, "SetText")?;
        let set_brush = Function::find(image, "SetBrushFromTexture")?;
        let button_class = reflection::find_class("BrickButtonWidget")
            .ok_or("class BrickButtonWidget not found")?;
        let initialize = reflection::find_function(class, "InitializeEntry")?;
        // SAFETY: a live class record; property records live as long as it.
        let brush_resource = unsafe {
            let property = reflection::find_property(image.cast(), "Brush");
            let brush = if property.is_null() {
                core::ptr::null()
            } else {
                reflection::struct_type(property)
            };
            if brush.is_null() {
                return Err("Image::Brush isn't a struct field".into());
            }
            (*property).offset_internal as usize
                + reflection::struct_field_offset(brush, "ResourceObject", "ObjectProperty", 8)?
        };
        Ok(Self {
            class,
            entry,
            image: reflection::field_offset(class, "Image", "ObjectProperty", 8)?,
            name: reflection::field_offset(class, "NameTextBlock", "ObjectProperty", 8)?,
            text: set_text.only_parameter(&["TextProperty"], size_of::<Text>() as i32)?,
            set_text,
            brush_texture: set_brush.parameter("Texture", "ObjectProperty", 8)?,
            set_brush,
            button: reflection::field_offset(entry_class, "Button", "ObjectProperty", 8)?,
            button_class,
            set_selected: SingleCall::find_only(
                button_class,
                "SetIsSelected",
                &["BoolProperty"],
                1,
            )?,
            texture_class,
            icons: std::sync::Mutex::new(Vec::new()),
            brush_resource,
            set_brush_resource: SingleCall::find(
                image,
                "SetBrushResourceObject",
                "ResourceObject",
                "ObjectProperty",
                8,
            )?,
            default_image: std::sync::OnceLock::new(),
            initialize_entry: reflection::parameterless_virtual(class, initialize)?,
            // SAFETY: the function the autosave's signature is in starts `UpdateItemWidget()`,
            // which takes only the widget.
            update_item: unsafe {
                core::mem::transmute::<*const u8, WidgetFn>(ugc_autosave::CALLER.locate()?.function)
            },
            rows: Rows::find(row_struct)?,
        })
    }

    /// The row `widget` shows, if it's a vehicle browser's row widget.
    ///
    /// # Safety
    /// `widget` must be a live object or null. Game thread only.
    pub unsafe fn row_of(&self, widget: *const UObjectBase) -> Option<*const u8> {
        // SAFETY: per the contract; the entry's offset is reflected.
        unsafe {
            (!widget.is_null() && reflection::is_a(widget, self.class.cast()))
                .then(|| widget.cast::<u8>().add(self.entry))
        }
    }

    /// Fills `widget` as a tile: `name`, `icon`, then the game's own update (selected state, no
    /// autosave icon). Instead of `InitializeEntry`, so no thumbnail is asked for.
    ///
    /// # Safety
    /// `widget` must be a live row widget. Game thread only.
    pub unsafe fn show_folder(&self, widget: *mut UObjectBase, name: &Text, icon: TileIcon) {
        // SAFETY: per the contract; both widgets are the row's own (reflected), checked non-null,
        // and the functions are their classes'.
        unsafe {
            let base = widget.cast::<u8>();
            let name_block = base.add(self.name).cast::<*mut UObjectBase>().read();
            if !name_block.is_null() {
                // A reference of its own for the call, given back after: `ProcessEvent` leaves
                // the parameters to the caller.
                let mut parameters = Parameters::new();
                reflection::put(&mut parameters, self.text, Text::copy_of(name));
                self.set_text.call(name_block, &mut parameters);
                reflection::take::<Text>(&parameters, self.text).release();
            }
            let image = base.add(self.image).cast::<*mut UObjectBase>().read();
            if !image.is_null() {
                // The first image ever given an icon still has the Blueprint's own brush.
                self.default_image.get_or_init(|| {
                    let own = self.brush_texture_of(image);
                    (!own.is_null()).then(|| WeakObject::new(own))
                });
            }
            // The folder icon if the other one is gone (an update renamed it).
            let icon = self.icon(icon).or_else(|| self.icon(TileIcon::Folder));
            if let (false, Some(icon)) = (image.is_null(), icon) {
                let mut parameters = Parameters::new();
                reflection::put(&mut parameters, self.brush_texture, icon);
                self.set_brush.call(image, &mut parameters);
            }
            (self.update_item)(widget);
        }
    }

    /// Gives `widget`'s `Image` its own brush back if a tile left an icon there, before the game
    /// shows a vehicle on it: the game only asks for a thumbnail, and a vehicle without one shows
    /// the image's brush (the Blueprint's "No Image"), which would be a pooled tile's folder icon.
    ///
    /// # Safety
    /// `widget` must be a live row widget. Game thread only.
    pub unsafe fn show_own_image(&self, widget: *mut UObjectBase) {
        // `None`: no tile was ever shown, so no image has an icon of ours.
        let Some(own) = self.default_image.get() else {
            return;
        };
        // SAFETY: per the contract; the image is the row's own (reflected), checked non-null, and
        // the function is its class's (`UImage`).
        unsafe {
            let image = widget
                .cast::<u8>()
                .add(self.image)
                .cast::<*mut UObjectBase>()
                .read();
            if image.is_null() || !self.is_icon(self.brush_texture_of(image)) {
                return;
            }
            let own = match own {
                Some(texture) => match texture.get_live() {
                    Some(live) => live,
                    // Unloaded (it shouldn't be: the Blueprint holds it): the icon stays.
                    None => return,
                },
                None => core::ptr::null_mut(),
            };
            self.set_brush_resource.call(image, own);
        }
    }

    /// The texture (or other resource) `image`'s brush shows, or null.
    ///
    /// # Safety
    /// `image` must be a live `UImage`. Game thread only.
    unsafe fn brush_texture_of(&self, image: *mut UObjectBase) -> *mut UObjectBase {
        // SAFETY: per the contract; the field's offset is reflected (`UImage::Brush`, then
        // `FSlateBrush::ResourceObject`, an object pointer).
        unsafe {
            image
                .cast::<u8>()
                .add(self.brush_resource)
                .cast::<*mut UObjectBase>()
                .read()
        }
    }

    /// Whether `texture` is one of the tiles' icons.
    fn is_icon(&self, texture: *mut UObjectBase) -> bool {
        !texture.is_null()
            && self
                .icons
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .any(|(_, weak)| weak.get_live() == Some(texture))
    }

    /// Shows `widget` (un)selected, the square behind it as on the row the list selects.
    ///
    /// Not through the row's `UpdateButtonSelected(bool)`: `UUGCItemWidget`'s ignores its argument
    /// and shows the list's own selected state (BRMK 0x3F67F0 jumps to `UpdateItemWidget()`, which
    /// passes the entry's private `bIsSelected`, set only by the list's `SetIsSelected`), so a row
    /// the list didn't select stays unselected. `WBP_UGCItem` shows it with its button's
    /// `SetIsSelected`, called here directly. The game's next `UpdateItemWidget` on the row puts its
    /// own state back (a page rebuilt: our tiles select theirs again after).
    ///
    /// # Safety
    /// `widget` must be a live row widget. Game thread only.
    pub unsafe fn show_selected(&self, widget: *mut UObjectBase, selected: bool) {
        // SAFETY: per the contract; the button is the row's reflected object property, checked
        // to be a `UBrickButtonWidget`, whose function this is.
        unsafe {
            let button = widget
                .cast::<u8>()
                .add(self.button)
                .cast::<*mut UObjectBase>()
                .read();
            if !button.is_null() && reflection::is_a(button, self.button_class.cast()) {
                self.set_selected.call(button, selected);
            }
        }
    }

    /// `icon`'s texture, loaded the first time (and again if it was unloaded).
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn icon(&self, icon: TileIcon) -> Option<*mut UObjectBase> {
        let mut icons = self
            .icons
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(live) = icons
            .iter()
            .find(|(known, _)| *known == icon)
            .and_then(|(_, weak)| weak.get_live())
        {
            return Some(live);
        }
        // SAFETY: per the contract; a native class.
        let loaded = unsafe { assets::load(self.texture_class, icon.path()) };
        if loaded.is_null() {
            return None;
        }
        // SAFETY: just loaded, live.
        let weak = unsafe { WeakObject::new(loaded) };
        icons.retain(|(known, _)| *known != icon);
        icons.push((icon, weak));
        Some(loaded)
    }
}
