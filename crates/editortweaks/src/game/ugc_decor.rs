//! What we add over a vehicle browser row's thumbnail: an area in the UI style's accent color (a
//! folder tile's), a 2x2 grid of vehicle thumbnails (its preview) and a small icon in the corner
//! where the game shows the autosave's (a pin, a folder in the accent color).
//!
//! `WBP_UGCItem` (BRMK 1.11.6's content): `Button` > `BrickVerticalBox_0` > [`Overlay_0` >
//! [`Image` (the thumbnail, 256x256, padding 8, centered), `AutoSaveIcon` (a `UBrickImage`, 48x48,
//! aligned right and top)], `NameTextBlock`]. Its Blueprint only sets `AutoSaveIcon`'s visibility
//! (shown for the dedicated autosave). Ours are made the first time a row widget needs them and
//! added to that overlay (`AddChildToOverlay` appends, and later children are drawn over earlier
//! ones): the accent area (a `UBrickImage` without a texture, which draws a plain box, in the
//! `Highlight` color style and partly see-through), a `UUniformGridPanel` of four `UBrickImage`s,
//! both placed like `Image` (which goes see-through while the grid shows), then the corner icon, a
//! `UBrickImage` placed and sized like the autosave icon. None of them takes the mouse. Row widgets are pooled and reused, so ours stay with their
//! widget and are shown or hidden each time it shows a row. All through reflection;
//! `AutoSaveIcon` is a Blueprint variable, found on the widget's own class.

use std::cell::RefCell;
use std::collections::HashMap;

use brickrust::ue::coreuobject::{EObjectFlags, UObjectBase};
use brickrust::ue::fname::FName;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::game::assets;
use crate::game::reflection::{self, Function, Getter, Parameters, SingleCall};
use crate::game::ugc_thumbnail::{Style, Thumbnail, Thumbnails};
use crate::game::ui_style::{self, SetColorStyle};
use crate::game::weak_object::WeakObject;

/// The Blueprint variable holding the autosave's icon.
const AUTOSAVE_ICON: &str = "AutoSaveIcon";

const ICONS: &str = "/Game/BrickRigs/UI/Textures/Icons/";

/// The grid's cells: 2x2.
pub const CELLS: usize = 4;

/// Pixels around each cell, so 4 between two cells.
const CELL_PADDING: f32 = 2.0;

/// How much the accent area covers the row's background (0: not at all, 1: a solid box).
const ACCENT_OPACITY: f32 = 0.3;

/// What the corner icon shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Badge {
    /// A pinned vehicle or folder (the pin our Pin button shows).
    Pin,
    /// A folder tile showing its vehicles (the folder icon of the game's input help), in the accent
    /// color.
    Folder,
}

impl Badge {
    fn texture(self) -> &'static str {
        match self {
            Badge::Pin => "T_PinRoot.T_PinRoot",
            Badge::Folder => "T_Folder.T_Folder",
        }
    }
}

/// What a row shows of ours.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Look {
    pub badge: Option<Badge>,
    /// The grid's cells in reading order, a missing or `None` one left blank; `None`: no grid, the
    /// row's own image shows.
    pub grid: Option<Vec<Option<Thumbnail>>>,
    /// The accent area under the grid (a folder tile's).
    pub accent: bool,
}

/// Byte-sized enum parameters (`TEnumAsByte` is a byte property, an `enum class` an enum one).
const BYTE_KINDS: &[&str] = &["ByteProperty", "EnumProperty"];

/// The reflected parts our widgets are made and shown with (after engine init).
pub struct Decorations {
    brick_image: *const UClass,
    grid_class: *const UClass,
    texture_class: *const UClass,
    overlay_class: *const UStruct,
    /// `UPanelWidget* UWidget::GetParent()`.
    get_parent: Getter<*mut UObjectBase>,
    /// `UWidget::Slot`, and `UOverlaySlot::Padding` (an `FMargin`).
    slot: usize,
    slot_padding: usize,
    /// `UOverlaySlot* UOverlay::AddChildToOverlay(UWidget* Content)`.
    add_child: Function,
    add_content: usize,
    add_result: usize,
    /// `UOverlaySlot::SetHorizontalAlignment` / `SetVerticalAlignment` / `SetPadding`, and the
    /// values used.
    set_horizontal: SingleCall,
    set_vertical: SingleCall,
    set_padding: SingleCall,
    right: u8,
    top: u8,
    center_horizontal: u8,
    center_vertical: u8,
    /// `UUniformGridSlot* UUniformGridPanel::AddChildToUniformGrid(UWidget*, int32 InRow, int32
    /// InColumn)` and `SetSlotPadding(FMargin)`.
    add_cell: Function,
    cell_content: usize,
    cell_row: usize,
    cell_column: usize,
    set_cell_padding: SingleCall,
    /// `UImage::SetBrushFromTexture(UTexture2D* Texture, bool bMatchSize)`.
    set_brush: Function,
    brush_texture: usize,
    /// `UImage::SetBrushSize(FVector2D DesiredSize)`.
    set_size: SingleCall,
    /// `UImage::Brush`'s offset, and its `ImageSize`'s inside it.
    brush: usize,
    image_size: usize,
    /// `UWidget::SetVisibility(ESlateVisibility)`, and the values used.
    set_visibility: SingleCall,
    collapsed: u8,
    hidden: u8,
    hit_test_invisible: u8,
    /// `UWidget::SetRenderOpacity(float)`: the row's image is see-through while the grid shows.
    set_opacity: SingleCall,
    /// The row widget's `Image`, `ThumbnailRoundedEdgeRadius` and `ThumbnailShadowOffset`.
    row_image: usize,
    row_radius: usize,
    row_shadow: usize,
    /// Thumbnail requests, or why there are none; without them the grid isn't made.
    thumbnails: Result<Thumbnails, String>,
    /// Images in the accent color, or why not; without it there's no accent area and the folder
    /// icon keeps the corner icon's own color.
    accent: Result<Accent, String>,
}

/// Restyling our images (`UBrickImage::SetColorStyle`, reflected; `ColorStyle` holds the current
/// one).
struct Accent {
    set_style: SetColorStyle,
    style: usize,
    highlight: u8,
}

impl Accent {
    fn find(brick_image: *const UClass) -> Result<Self, String> {
        let style = reflection::field_offset(brick_image, "ColorStyle", "EnumProperty", 1)
            .or_else(|_| reflection::field_offset(brick_image, "ColorStyle", "ByteProperty", 1))?;
        Ok(Self {
            set_style: SetColorStyle::find(brick_image)?,
            style,
            highlight: ui_style::color_style_values()?.highlight,
        })
    }

    /// `image`'s color style.
    ///
    /// # Safety
    /// `image` must be a live `UBrickImage`.
    unsafe fn style_of(&self, image: *const UObjectBase) -> u8 {
        // SAFETY: per the contract; the field's offset is reflected.
        unsafe { image.cast::<u8>().add(self.style).read() }
    }
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for Decorations {}
unsafe impl Sync for Decorations {}

/// A grid and its cells.
type Grid = (WeakObject, [WeakObject; CELLS]);

/// One row widget's widgets of ours, as `show` uses them.
#[derive(Clone, Copy)]
struct Parts {
    badge: WeakObject,
    badge_style: u8,
    backdrop: Option<WeakObject>,
    grid: Option<Grid>,
}

/// One row widget's widgets of ours.
struct Made {
    /// The row widget (its address may be reused once it's gone).
    owner: WeakObject,
    badge: WeakObject,
    /// The corner icon's own color style, given back when it shows a pin.
    badge_style: u8,
    /// The accent area; `None` without accent colors.
    backdrop: Option<WeakObject>,
    /// The grid and its cells; `None` without thumbnail requests.
    grid: Option<Grid>,
    /// What's shown.
    look: Look,
}

impl Made {
    fn parts(&self) -> Parts {
        Parts {
            badge: self.badge,
            badge_style: self.badge_style,
            backdrop: self.backdrop,
            grid: self.grid,
        }
    }
}

thread_local! {
    /// Our widgets by row widget address. Game thread only.
    static MADE: RefCell<HashMap<usize, Made>> = RefCell::new(HashMap::new());
    /// The autosave icon's offset on the last row widget class seen.
    static ICON_FIELD: RefCell<Option<(*const UStruct, Option<usize>)>> = const { RefCell::new(None) };
    /// The icons' textures once loaded (a shown brush keeps its texture alive; reloaded if it went).
    static TEXTURES: RefCell<HashMap<&'static str, WeakObject>> = RefCell::new(HashMap::new());
}

/// How many rows' widgets are kept before those of destroyed rows are dropped.
const TIDY_ABOVE: usize = 256;

impl Decorations {
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let widget = class("Widget")?;
        let image = class("Image")?;
        let overlay = class("Overlay")?;
        let overlay_slot = class("OverlaySlot")?;
        let grid = class("UniformGridPanel")?;
        let row = class("UGCItemWidget")?;
        let brick_image = class("BrickImage")?;
        let enum_byte = |name: &'static str, entry: &str| -> Result<u8, String> {
            let value = reflection::enum_value(name, entry)?;
            u8::try_from(value).map_err(|_| format!("{entry} is {value}"))
        };
        let visibility =
            |entry: &str| enum_byte("ESlateVisibility", &format!("ESlateVisibility::{entry}"));
        let add_child = Function::find(overlay, "AddChildToOverlay")?;
        let add_cell = Function::find(grid, "AddChildToUniformGrid")?;
        let set_brush = Function::find(image, "SetBrushFromTexture")?;
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
        Ok(Self {
            brick_image,
            grid_class: grid,
            texture_class: class("Texture2D")?,
            overlay_class: overlay.cast(),
            get_parent: Getter::find(widget, "GetParent", "ObjectProperty")?,
            slot: reflection::field_offset(widget, "Slot", "ObjectProperty", 8)?,
            slot_padding: reflection::field_offset(overlay_slot, "Padding", "StructProperty", 16)?,
            add_content: add_child.parameter("Content", "ObjectProperty", 8)?,
            add_result: add_child.parameter("ReturnValue", "ObjectProperty", 8)?,
            add_child,
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
            set_padding: SingleCall::find_only(
                overlay_slot,
                "SetPadding",
                &["StructProperty"],
                16,
            )?,
            right: enum_byte("EHorizontalAlignment", "HAlign_Right")?,
            top: enum_byte("EVerticalAlignment", "VAlign_Top")?,
            center_horizontal: enum_byte("EHorizontalAlignment", "HAlign_Center")?,
            center_vertical: enum_byte("EVerticalAlignment", "VAlign_Center")?,
            cell_content: add_cell.parameter("Content", "ObjectProperty", 8)?,
            cell_row: add_cell.parameter("InRow", "IntProperty", 4)?,
            cell_column: add_cell.parameter("InColumn", "IntProperty", 4)?,
            add_cell,
            set_cell_padding: SingleCall::find_only(
                grid,
                "SetSlotPadding",
                &["StructProperty"],
                16,
            )?,
            brush_texture: set_brush.parameter("Texture", "ObjectProperty", 8)?,
            set_brush,
            set_size: SingleCall::find_only(image, "SetBrushSize", &["StructProperty"], 8)?,
            brush,
            image_size: reflection::struct_field_offset(
                brush_struct,
                "ImageSize",
                "StructProperty",
                8,
            )?,
            set_visibility: SingleCall::find_only(widget, "SetVisibility", BYTE_KINDS, 1)?,
            collapsed: visibility("Collapsed")?,
            hidden: visibility("Hidden")?,
            hit_test_invisible: visibility("HitTestInvisible")?,
            set_opacity: SingleCall::find_only(widget, "SetRenderOpacity", &["FloatProperty"], 4)?,
            row_image: reflection::field_offset(row, "Image", "ObjectProperty", 8)?,
            row_radius: reflection::field_offset(
                row,
                "ThumbnailRoundedEdgeRadius",
                "FloatProperty",
                4,
            )?,
            row_shadow: reflection::field_offset(
                row,
                "ThumbnailShadowOffset",
                "StructProperty",
                8,
            )?,
            thumbnails: Thumbnails::find(),
            accent: Accent::find(brick_image).map_err(|reason| format!("accent color: {reason}")),
        })
    }

    /// Why folder tiles can't show their vehicles, if they can't.
    pub fn no_thumbnails(&self) -> Option<&str> {
        self.thumbnails.as_ref().err().map(String::as_str)
    }

    /// Why folder tiles can't show the accent color, if they can't.
    pub fn no_accent(&self) -> Option<&str> {
        self.accent.as_ref().err().map(String::as_str)
    }

    /// The thumbnail of a vehicle row, if thumbnails can be shown and it has one.
    ///
    /// # Safety
    /// `row` must be a live vehicle row (`FUGCQueryResult`). Game thread only.
    pub unsafe fn thumbnail_of(&self, row: *const u8) -> Option<Thumbnail> {
        let thumbnails = self.thumbnails.as_ref().ok()?;
        // SAFETY: per the contract.
        unsafe { thumbnails.of_row(row) }
    }

    /// Shows `look` on `widget`. A widget that never needed anything gets nothing made.
    ///
    /// # Safety
    /// `widget` must be a live vehicle browser row widget. Game thread only.
    pub unsafe fn show(&self, widget: *mut UObjectBase, look: Look) {
        let key = widget as usize;
        let known = MADE.with_borrow(|made| {
            made.get(&key)
                .filter(|made| made.owner.get_live() == Some(widget))
                .map(|made| (made.parts(), made.look.clone()))
        });
        let (parts, shown) = match known {
            Some(known) => known,
            None if look == Look::default() => return,
            // SAFETY: per the contract.
            None => match unsafe { self.make(widget) } {
                Some(parts) => (parts, Look::default()),
                None => return,
            },
        };
        let Parts {
            badge,
            badge_style,
            backdrop,
            grid,
        } = parts;
        // The grid's thumbnails are asked for again even when they're the same, as the game asks
        // for its own image's each time a row is shown: a row widget taken off the screen may have
        // had its cells' loads cancelled (`UBrickImage::ReleaseSlateResources`), leaving them
        // blank (a plain white box).
        if shown == look && look.grid.is_none() {
            return;
        }
        // SAFETY: per the contract; our widgets checked live, each call's parameters at their
        // reflected offsets. The state isn't borrowed while the game is called.
        unsafe {
            if shown.badge != look.badge
                && let Some(badge) = badge.get_live()
            {
                self.show_badge(badge, look.badge, badge_style);
            }
            if shown.accent != look.accent
                && let Some(backdrop) = backdrop.and_then(|backdrop| backdrop.get_live())
            {
                let visibility = if look.accent {
                    self.hit_test_invisible
                } else {
                    self.collapsed
                };
                self.set_visibility.call(backdrop, visibility);
            }
            if (shown.grid.is_some() || look.grid.is_some())
                && let Some((grid, cells)) = grid
            {
                self.show_grid(widget, grid, cells, look.grid.as_deref());
            }
        }
        MADE.with_borrow_mut(|made| {
            if let Some(made) = made.get_mut(&key) {
                made.look = look;
            }
        });
    }

    /// Shows `badge` in the corner, the folder in the accent color and the pin in the image's own
    /// (`own_style`).
    ///
    /// # Safety
    /// `image` must be our live corner image. Game thread only.
    unsafe fn show_badge(&self, image: *mut UObjectBase, badge: Option<Badge>, own_style: u8) {
        // SAFETY: per the contract; each call's parameter at its reflected offset.
        unsafe {
            match badge.and_then(|badge| Some((badge, self.texture(badge)?))) {
                Some((badge, texture)) => {
                    let mut parameters = Parameters::new();
                    reflection::put(&mut parameters, self.brush_texture, texture);
                    self.set_brush.call(image, &mut parameters);
                    if let Ok(accent) = &self.accent {
                        let style = match badge {
                            Badge::Folder => accent.highlight,
                            Badge::Pin => own_style,
                        };
                        if accent.style_of(image) != style {
                            accent.set_style.call(image.cast(), style);
                        }
                    }
                    self.set_visibility.call(image, self.hit_test_invisible);
                }
                None => self.set_visibility.call(image, self.collapsed),
            }
        }
    }

    /// Shows `cells` in the grid over the row's own image (made see-through), or hides the grid
    /// and shows the image again.
    ///
    /// # Safety
    /// `widget` must be the live row widget the grid was made for. Game thread only.
    unsafe fn show_grid(
        &self,
        widget: *mut UObjectBase,
        grid: WeakObject,
        cells: [WeakObject; CELLS],
        shown: Option<&[Option<Thumbnail>]>,
    ) {
        let (Ok(thumbnails), Some(grid)) = (&self.thumbnails, grid.get_live()) else {
            return;
        };
        // SAFETY: per the contract; the row's image is its own (reflected, checked non-null), our
        // widgets checked live.
        unsafe {
            let base = widget.cast::<u8>();
            let row_image = base.add(self.row_image).cast::<*mut UObjectBase>().read();
            let Some(shown) = shown else {
                self.set_visibility.call(grid, self.collapsed);
                if !row_image.is_null() {
                    self.set_opacity.call(row_image, 1.0f32);
                }
                return;
            };
            let style = self.request_style(widget, cells[0].get_live());
            for (index, cell) in cells.iter().enumerate() {
                let Some(cell) = cell.get_live() else {
                    continue;
                };
                match shown.get(index).and_then(Option::as_ref) {
                    Some(thumbnail) => {
                        thumbnails.show(cell, thumbnail, style);
                        self.set_visibility.call(cell, self.hit_test_invisible);
                    }
                    // Hidden, not collapsed: a blank cell keeps its place.
                    None => self.set_visibility.call(cell, self.hidden),
                }
            }
            self.set_visibility.call(grid, self.hit_test_invisible);
            if !row_image.is_null() {
                self.set_opacity.call(row_image, 0.0f32);
            }
        }
    }

    /// Makes `widget`'s accent area and grid (if they can be shown) and corner image in the overlay
    /// holding its autosave icon; `None` if the widget isn't laid out as expected.
    ///
    /// # Safety
    /// As for [`Self::show`].
    unsafe fn make(&self, widget: *mut UObjectBase) -> Option<Parts> {
        // SAFETY: per the contract; every object read is live (checked non-null, the parent checked
        // to be an overlay), each call's parameters at their reflected offsets.
        unsafe {
            let field = autosave_icon_field(widget)?;
            let icon = widget
                .cast::<u8>()
                .add(field)
                .cast::<*mut UObjectBase>()
                .read();
            if icon.is_null() {
                return None;
            }
            let overlay = self.get_parent.call(icon);
            if overlay.is_null() || !reflection::is_a(overlay, self.overlay_class) {
                return None;
            }
            // Made where `UWidgetTree::ConstructWidget` makes widgets: in the row widget's tree
            // (the icon's outer).
            let tree = (*icon).outer_private.cast_mut();
            if tree.is_null() {
                return None;
            }
            // In drawing order: the accent area, the grid over it, the corner icon over both.
            let backdrop = self.make_backdrop(widget, tree, overlay);
            let grid = if self.thumbnails.is_ok() {
                self.make_grid(widget, tree, overlay)
            } else {
                None
            };
            let badge = self.new_widget(self.brick_image, tree)?;
            self.set_visibility.call(badge, self.collapsed);
            self.set_size.call(badge, self.brush_size(icon));
            if let Some(slot) = self.add_to_overlay(overlay, badge) {
                self.set_horizontal.call(slot, self.right);
                self.set_vertical.call(slot, self.top);
            }
            let badge_style = match &self.accent {
                Ok(accent) => accent.style_of(badge),
                Err(_) => 0,
            };
            let badge = WeakObject::new(badge);
            let made = Made {
                owner: WeakObject::new(widget),
                badge,
                badge_style,
                backdrop,
                grid,
                look: Look::default(),
            };
            let parts = made.parts();
            MADE.with_borrow_mut(|known| {
                if known.len() > TIDY_ABOVE {
                    known.retain(|_, made| made.owner.get_live().is_some());
                }
                known.insert(widget as usize, made);
            });
            Some(parts)
        }
    }

    /// The accent area, collapsed: a plain box placed and sized like the row's image, in the
    /// accent color, partly see-through. `None` without accent colors.
    ///
    /// # Safety
    /// As for [`Self::make_grid`].
    unsafe fn make_backdrop(
        &self,
        widget: *mut UObjectBase,
        tree: *mut UObjectBase,
        overlay: *mut UObjectBase,
    ) -> Option<WeakObject> {
        let accent = self.accent.as_ref().ok()?;
        // SAFETY: per the contract; objects checked non-null, parameters at reflected offsets.
        unsafe {
            let row_image = self.row_image_of(widget)?;
            let backdrop = self.new_widget(self.brick_image, tree)?;
            self.set_visibility.call(backdrop, self.collapsed);
            self.set_size.call(backdrop, self.brush_size(row_image));
            accent.set_style.call(backdrop.cast(), accent.highlight);
            self.set_opacity.call(backdrop, ACCENT_OPACITY);
            if let Some(slot) = self.add_to_overlay(overlay, backdrop) {
                self.place_like(slot, row_image);
            }
            Some(WeakObject::new(backdrop))
        }
    }

    /// The row widget's own image, if it has one.
    ///
    /// # Safety
    /// `widget` must be a live row widget.
    unsafe fn row_image_of(&self, widget: *mut UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; the field's offset is reflected.
        let image = unsafe {
            widget
                .cast::<u8>()
                .add(self.row_image)
                .cast::<*mut UObjectBase>()
                .read()
        };
        (!image.is_null()).then_some(image)
    }

    /// Places a widget of ours (by its overlay `slot`) over the row's image: centered, with the
    /// image's padding.
    ///
    /// # Safety
    /// `slot` must be a live overlay slot, `row_image` the live row image in the same overlay. Game
    /// thread only.
    unsafe fn place_like(&self, slot: *mut UObjectBase, row_image: *mut UObjectBase) {
        // SAFETY: per the contract; the image's slot checked to be an overlay slot like ours, its
        // padding at its reflected offset.
        unsafe {
            self.set_horizontal.call(slot, self.center_horizontal);
            self.set_vertical.call(slot, self.center_vertical);
            let image_slot = row_image
                .cast::<u8>()
                .add(self.slot)
                .cast::<*mut UObjectBase>()
                .read();
            if !image_slot.is_null()
                && reflection::is_a(image_slot, reflection::class_of(slot.cast()))
            {
                let padding = image_slot
                    .cast::<u8>()
                    .add(self.slot_padding)
                    .cast::<[f32; 4]>()
                    .read_unaligned();
                self.set_padding.call(slot, padding);
            }
        }
    }

    /// The grid of [`CELLS`] images, collapsed, placed over the row's image like it: the same
    /// padding in the overlay, centered, each cell a quarter of the image's size.
    ///
    /// # Safety
    /// `widget` must be a live row widget, `tree` its widget tree and `overlay` the overlay holding
    /// its images. Game thread only.
    unsafe fn make_grid(
        &self,
        widget: *mut UObjectBase,
        tree: *mut UObjectBase,
        overlay: *mut UObjectBase,
    ) -> Option<Grid> {
        // SAFETY: per the contract; objects checked non-null, parameters at reflected offsets.
        unsafe {
            let row_image = self.row_image_of(widget)?;
            let [width, height] = self.brush_size(row_image);
            let cell_size = [
                ((width - 4.0 * CELL_PADDING) / 2.0).max(1.0),
                ((height - 4.0 * CELL_PADDING) / 2.0).max(1.0),
            ];
            let grid = self.new_widget(self.grid_class, tree)?;
            self.set_visibility.call(grid, self.collapsed);
            self.set_cell_padding.call(grid, [CELL_PADDING; 4]);
            let mut cells = Vec::with_capacity(CELLS);
            for index in 0..CELLS {
                let cell = self.new_widget(self.brick_image, tree)?;
                self.set_visibility.call(cell, self.hidden);
                self.set_size.call(cell, cell_size);
                // The row's own image's color style (`Custom`): a selected row's button tints
                // the images styled otherwise in its selected colors, which would hide a selected
                // folder's thumbnails.
                if let Ok(accent) = &self.accent {
                    accent
                        .set_style
                        .call(cell.cast(), accent.style_of(row_image));
                }
                let mut parameters = Parameters::new();
                reflection::put(&mut parameters, self.cell_content, cell);
                reflection::put(&mut parameters, self.cell_row, (index / 2) as i32);
                reflection::put(&mut parameters, self.cell_column, (index % 2) as i32);
                self.add_cell.call(grid, &mut parameters);
                cells.push(WeakObject::new(cell));
            }
            let cells: [WeakObject; CELLS] = cells.try_into().ok()?;
            if let Some(slot) = self.add_to_overlay(overlay, grid) {
                self.place_like(slot, row_image);
            }
            Some((WeakObject::new(grid), cells))
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

    /// Adds `child` last to `overlay`, returning its slot.
    ///
    /// # Safety
    /// Both must be live, `overlay` a `UOverlay`. Game thread only.
    unsafe fn add_to_overlay(
        &self,
        overlay: *mut UObjectBase,
        child: *mut UObjectBase,
    ) -> Option<*mut UObjectBase> {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; parameters at their reflected offsets.
        unsafe {
            reflection::put(&mut parameters, self.add_content, child);
            self.add_child.call(overlay, &mut parameters);
            let slot = reflection::take::<*mut UObjectBase>(&parameters, self.add_result);
            (!slot.is_null()).then_some(slot)
        }
    }

    /// An image's brush size.
    ///
    /// # Safety
    /// `image` must be a live `UImage`.
    /// How a thumbnail is asked for on `image` of the row widget `widget`, as the game asks for the
    /// row's own (`InitializeEntry`): the image's brush size (none without an image) and the row's
    /// rounded edges and shadow.
    ///
    /// # Safety
    /// `widget` must be a live row widget, `image` a live image or `None`. Game thread only.
    unsafe fn request_style(
        &self,
        widget: *mut UObjectBase,
        image: Option<*mut UObjectBase>,
    ) -> Style {
        // SAFETY: per the contract; the row's fields at their reflected offsets.
        unsafe {
            let base = widget.cast::<u8>();
            Style {
                size: image.map_or([0, 0], |image| {
                    self.brush_size(image).map(|side| side as i32)
                }),
                rounded_edge_radius: base.add(self.row_radius).cast::<f32>().read_unaligned(),
                shadow_offset: base
                    .add(self.row_shadow)
                    .cast::<[f32; 2]>()
                    .read_unaligned(),
            }
        }
    }

    /// Asks `copy` (a `UBrickImage`) to show the thumbnail `source` shows, if it's one: the row's
    /// own image when the row shows a vehicle (`row`, its `FUGCQueryResult`), or a cell of our grid.
    /// A loaded thumbnail isn't in the image's reflected brush (`UBrickImage` keeps it apart, see
    /// `docs/vehicle-menu.md`), so a copy asks for it again (through the game's thumbnail
    /// subsystem; a throbber shows until it's there). Whether it was one.
    ///
    /// # Safety
    /// `widget` must be a live row widget, `row` its row if it shows a vehicle, `source` one of its
    /// images, `copy` a live `UBrickImage`. Game thread only.
    pub unsafe fn copy_thumbnail(
        &self,
        widget: *mut UObjectBase,
        row: Option<*const u8>,
        source: *mut UObjectBase,
        copy: *mut UObjectBase,
    ) -> bool {
        let Ok(thumbnails) = &self.thumbnails else {
            return false;
        };
        // SAFETY: per the contract; the row's image is its own (reflected).
        let row_image = unsafe {
            widget
                .cast::<u8>()
                .add(self.row_image)
                .cast::<*mut UObjectBase>()
                .read()
        };
        let thumbnail = if source == row_image {
            // SAFETY: per the contract.
            row.and_then(|row| unsafe { thumbnails.of_row(row) })
        } else {
            let key = widget as usize;
            MADE.with_borrow(|made| {
                let made = made
                    .get(&key)
                    .filter(|made| made.owner.get_live() == Some(widget))?;
                let (_, cells) = made.grid.as_ref()?;
                let index = cells
                    .iter()
                    .position(|cell| cell.get_live() == Some(source))?;
                made.look.grid.as_ref()?.get(index)?.clone()
            })
        };
        let Some(thumbnail) = thumbnail else {
            return false;
        };
        // SAFETY: per the contract.
        unsafe {
            let style = self.request_style(widget, Some(source));
            thumbnails.show(copy, &thumbnail, style);
        }
        true
    }

    unsafe fn brush_size(&self, image: *mut UObjectBase) -> [f32; 2] {
        // SAFETY: per the contract; the brush and its size at reflected offsets.
        unsafe {
            image
                .cast::<u8>()
                .add(self.brush + self.image_size)
                .cast::<[f32; 2]>()
                .read_unaligned()
        }
    }

    /// `badge`'s texture, loaded the first time (and again if it was unloaded).
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn texture(&self, badge: Badge) -> Option<*mut UObjectBase> {
        let name = badge.texture();
        let known = TEXTURES.with_borrow(|textures| textures.get(name).copied());
        if let Some(live) = known.and_then(|weak| weak.get_live()) {
            return Some(live);
        }
        // SAFETY: per the contract; a native class.
        let loaded = unsafe { assets::load(self.texture_class, &format!("{ICONS}{name}")) };
        if loaded.is_null() {
            return None;
        }
        // SAFETY: just loaded, live.
        let weak = unsafe { WeakObject::new(loaded) };
        TEXTURES.with_borrow_mut(|textures| textures.insert(name, weak));
        Some(loaded)
    }
}

/// The offset of the autosave icon on `widget`'s class (a Blueprint variable), if it has one.
///
/// # Safety
/// `widget` must be a live object. Game thread only.
unsafe fn autosave_icon_field(widget: *const UObjectBase) -> Option<usize> {
    // SAFETY: per the contract.
    let class = unsafe { reflection::class_of(widget.cast()) };
    if let Some((known, field)) = ICON_FIELD.with_borrow(|known| *known)
        && known == class
    {
        return field;
    }
    // SAFETY: a live class record; its property records live as long as it.
    let field = unsafe {
        let property = reflection::find_property(class, AUTOSAVE_ICON);
        (!property.is_null() && reflection::is_kind(property, "ObjectProperty"))
            .then(|| (*property).offset_internal as usize)
    };
    ICON_FIELD.with_borrow_mut(|known| *known = Some((class, field)));
    field
}
