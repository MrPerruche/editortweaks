//! A row of menu buttons of ours above a paged list's entries: the vehicle browser's navigation bar
//! (Back, Forward, Up, the path). All through reflection; read 2026-11-10 in BRMK 1.11.6's content
//! (`docs/vehicle-menu.md`, "The navigation bar").
//!
//! `WBP_PagedList`'s left column is `BrickVerticalBox_1` > [`Overlay_0` > [`ScrollBox` (a native
//! `BindWidget` of `UPagedListWidget`, reflected) > `EntriesPanel`, `Throbber`], `PageSelector`].
//! `UPanelWidget::InsertChildAt` is editor-only, so the bar goes into `Overlay_0` (the scroll box's
//! parent): appended (drawn over the rest), stretched across, aligned to the top, and the scroll
//! box's own overlay slot gets a top padding of the bar's height, measured after a layout pass
//! (`ForceLayoutPrepass`, `GetDesiredSize`). The bar is a `UMenuButtonPanelWidget` of the class the
//! browser's own left panel is (`WBP_MenuButtonPanel_C`, made with `UWidgetBlueprintLibrary::
//! Create`), with every button in one row (`NumButtonsPerRow` 0) and not stretched (`bFillWidth`
//! false, a Blueprint variable its `PreConstruct` reads), so its buttons and clicks work like the
//! left panel's (`game::menu::ButtonPanels`, `features::menu_buttons`). Its buttons are remade by
//! `ClearButtons(true)` (keeps the widgets for reuse: BRMK 0x3ABA40 only zeroes the used count when
//! given true), `CreateButton`s, then `RemoveUnusedButtons()`.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::game::reflection::{self, Function, Getter, Parameters, SingleCall};
use crate::game::weak_object::WeakObject;

/// Pixels between the bar and the entries.
const GAP: f32 = 8.0;

/// What making, placing and measuring the bar takes (after engine init).
pub struct NavBars {
    /// `UPagedListWidget::ScrollBox`.
    scroll_box: usize,
    /// `UPanelWidget* UWidget::GetParent()`, `UWidget::Slot`, `UOverlaySlot::Padding`.
    get_parent: Getter<*mut UObjectBase>,
    slot: usize,
    slot_padding: usize,
    overlay_class: *const UStruct,
    overlay_slot_class: *const UStruct,
    /// `UOverlaySlot* UOverlay::AddChildToOverlay(UWidget* Content)`.
    add_child: Function,
    add_content: usize,
    add_result: usize,
    /// `UOverlaySlot::SetHorizontalAlignment` / `SetVerticalAlignment` / `SetPadding`.
    set_horizontal: SingleCall,
    set_vertical: SingleCall,
    set_padding: SingleCall,
    fill: u8,
    top: u8,
    /// `APlayerController* UUserWidget::GetOwningPlayer()` and `static UUserWidget*
    /// UWidgetBlueprintLibrary::Create(WorldContextObject, WidgetType, OwningPlayer)`.
    owning_player: Getter<*mut UObjectBase>,
    library: *mut UObjectBase,
    create: Function,
    create_context: usize,
    create_type: usize,
    create_player: usize,
    create_result: usize,
    /// `UMenuButtonPanelWidget::NumButtonsPerRow`, `ClearButtons(bool)`, `RemoveUnusedButtons()`.
    per_row: usize,
    clear: SingleCall,
    remove_unused: Function,
    /// `UWidget::ForceLayoutPrepass()` and `FVector2D UWidget::GetDesiredSize() const`.
    prepass: Function,
    desired_size: Getter<[f32; 2]>,
    /// `UWidget::SetVisibility(ESlateVisibility)`, and the values used.
    set_visibility: SingleCall,
    collapsed: u8,
    shown: u8,
}

// SAFETY: reflection records and the library's default object live as long as the game; only used
// on the game thread.
unsafe impl Send for NavBars {}
unsafe impl Sync for NavBars {}

/// A bar made over a paged list.
#[derive(Clone, Copy)]
pub struct Bar {
    /// The paged list it's in, the bar (a button panel), and the scroll box's overlay slot.
    list: WeakObject,
    pub panel: WeakObject,
    scroll_slot: WeakObject,
    /// The scroll box's padding before ours.
    padding: [f32; 4],
}

impl Bar {
    /// Whether this bar is `list`'s, and still there.
    pub fn is_on(&self, list: *mut UObjectBase) -> bool {
        self.list.get_live() == Some(list)
            && self.panel.get_live().is_some()
            && self.scroll_slot.get_live().is_some()
    }
}

impl NavBars {
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let widget = class("Widget")?;
        let overlay = class("Overlay")?;
        let overlay_slot = class("OverlaySlot")?;
        let panel = class("MenuButtonPanelWidget")?;
        let byte_kinds: &[&str] = &["ByteProperty", "EnumProperty"];
        let enum_byte = |name: &'static str, entry: &str| -> Result<u8, String> {
            let value = reflection::enum_value(name, entry)?;
            u8::try_from(value).map_err(|_| format!("{entry} is {value}"))
        };
        let visibility =
            |entry: &str| enum_byte("ESlateVisibility", &format!("ESlateVisibility::{entry}"));
        let add_child = Function::find(overlay, "AddChildToOverlay")?;
        let create = Function::find(class("WidgetBlueprintLibrary")?, "Create")?;
        Ok(Self {
            scroll_box: reflection::field_offset(
                class("PagedListWidget")?,
                "ScrollBox",
                "ObjectProperty",
                8,
            )?,
            get_parent: Getter::find(widget, "GetParent", "ObjectProperty")?,
            slot: reflection::field_offset(widget, "Slot", "ObjectProperty", 8)?,
            slot_padding: reflection::field_offset(overlay_slot, "Padding", "StructProperty", 16)?,
            overlay_class: overlay.cast(),
            overlay_slot_class: overlay_slot.cast(),
            add_content: add_child.parameter("Content", "ObjectProperty", 8)?,
            add_result: add_child.parameter("ReturnValue", "ObjectProperty", 8)?,
            add_child,
            set_horizontal: SingleCall::find_only(
                overlay_slot,
                "SetHorizontalAlignment",
                byte_kinds,
                1,
            )?,
            set_vertical: SingleCall::find_only(
                overlay_slot,
                "SetVerticalAlignment",
                byte_kinds,
                1,
            )?,
            set_padding: SingleCall::find_only(
                overlay_slot,
                "SetPadding",
                &["StructProperty"],
                16,
            )?,
            fill: enum_byte("EHorizontalAlignment", "HAlign_Fill")?,
            top: enum_byte("EVerticalAlignment", "VAlign_Top")?,
            owning_player: Getter::find(class("UserWidget")?, "GetOwningPlayer", "ObjectProperty")?,
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
            per_row: reflection::field_offset(panel, "NumButtonsPerRow", "IntProperty", 4)?,
            clear: SingleCall::find_only(panel, "ClearButtons", &["BoolProperty"], 1)?,
            remove_unused: Function::find(panel, "RemoveUnusedButtons")?,
            prepass: Function::find(widget, "ForceLayoutPrepass")?,
            desired_size: Getter::find(widget, "GetDesiredSize", "StructProperty")?,
            set_visibility: SingleCall::find_only(widget, "SetVisibility", byte_kinds, 1)?,
            collapsed: visibility("Collapsed")?,
            shown: visibility("SelfHitTestInvisible")?,
        })
    }

    /// Makes a bar of `panel_class` (a button panel class) over `list` (a paged list of `owner`, a
    /// user widget), collapsed and empty. `None` if the list isn't laid out as expected.
    ///
    /// # Safety
    /// `owner` and `list` must be live, `panel_class` a live `UMenuButtonPanelWidget` class. Game
    /// thread only.
    pub unsafe fn make(
        &self,
        owner: *mut UObjectBase,
        list: *mut UObjectBase,
        panel_class: *const UClass,
    ) -> Option<Bar> {
        // SAFETY: per the contract; every object read is checked non-null (and the overlay and
        // its slot by class), parameters go at their reflected offsets.
        unsafe {
            let scroll_box = read(list, self.scroll_box)?;
            let overlay = self.get_parent.call(scroll_box);
            if overlay.is_null() || !reflection::is_a(overlay, self.overlay_class) {
                return None;
            }
            let scroll_slot = read(scroll_box, self.slot)?;
            if !reflection::is_a(scroll_slot, self.overlay_slot_class) {
                return None;
            }
            let padding = scroll_slot
                .cast::<u8>()
                .add(self.slot_padding)
                .cast::<[f32; 4]>()
                .read_unaligned();
            let player = self.owning_player.call(owner);
            let mut parameters = Parameters::new();
            reflection::put(&mut parameters, self.create_context, owner);
            reflection::put(&mut parameters, self.create_type, panel_class);
            reflection::put(&mut parameters, self.create_player, player);
            self.create.call(self.library, &mut parameters);
            let panel = reflection::take::<*mut UObjectBase>(&parameters, self.create_result);
            if panel.is_null() {
                return None;
            }
            // Before its first layout, when `PreConstruct` reads it.
            if let Ok(fill_width) =
                reflection::bool_field(reflection::class_of(panel.cast()), "bFillWidth")
            {
                fill_width.set(panel.cast(), false);
            }
            panel.cast::<u8>().add(self.per_row).cast::<i32>().write(0);
            self.set_visibility.call(panel, self.collapsed);
            let mut parameters = Parameters::new();
            reflection::put(&mut parameters, self.add_content, panel);
            self.add_child.call(overlay, &mut parameters);
            let slot = reflection::take::<*mut UObjectBase>(&parameters, self.add_result);
            if !slot.is_null() {
                self.set_horizontal.call(slot, self.fill);
                self.set_vertical.call(slot, self.top);
            }
            Some(Bar {
                list: WeakObject::new(list),
                panel: WeakObject::new(panel),
                scroll_slot: WeakObject::new(scroll_slot),
                padding,
            })
        }
    }

    /// Starts remaking `bar`'s buttons: [`ButtonPanels::add_button`] them, then [`Self::finish`].
    ///
    /// [`ButtonPanels::add_button`]: crate::game::menu::ButtonPanels::add_button
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn begin(&self, bar: &Bar) -> Option<*mut UObjectBase> {
        let panel = bar.panel.get_live()?;
        // SAFETY: a live button panel; true keeps its widgets for the buttons made next.
        unsafe { self.clear.call(panel, true) };
        Some(panel)
    }

    /// Pools the buttons not made again, shows the bar and moves the entries below it. Whether the
    /// bar could be measured (if not, the entries only moved by the gap: measure again later).
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn finish(&self, bar: &Bar) -> bool {
        let (Some(panel), Some(scroll_slot)) = (bar.panel.get_live(), bar.scroll_slot.get_live())
        else {
            return false;
        };
        // SAFETY: live widgets of the classes these functions are on.
        unsafe {
            self.remove_unused.call(panel, &mut Parameters::new());
            self.set_visibility.call(panel, self.shown);
            self.prepass.call(panel, &mut Parameters::new());
            let [_, height] = self.desired_size.call(panel);
            let [left, top, right, bottom] = bar.padding;
            self.set_padding.call(
                scroll_slot,
                [left, top + height.max(0.0) + GAP, right, bottom],
            );
            height > 0.0
        }
    }

    /// Hides `bar`, the entries back where they were.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn hide(&self, bar: &Bar) {
        // SAFETY: live widgets of the classes these functions are on.
        unsafe {
            if let Some(panel) = bar.panel.get_live() {
                self.set_visibility.call(panel, self.collapsed);
            }
            if let Some(scroll_slot) = bar.scroll_slot.get_live() {
                self.set_padding.call(scroll_slot, bar.padding);
            }
        }
    }

    /// The scroll box of `list`, the area the entries show in.
    ///
    /// # Safety
    /// `list` must be a live paged list. Game thread only.
    pub unsafe fn scroll_box(&self, list: *mut UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract.
        unsafe { read(list, self.scroll_box) }
    }
}

/// The object pointer at `offset` in `object`, if not null.
///
/// # Safety
/// `object` must be live, with an object pointer field at `offset`.
unsafe fn read(object: *mut UObjectBase, offset: usize) -> Option<*mut UObjectBase> {
    // SAFETY: per the contract.
    let value = unsafe {
        object
            .cast::<u8>()
            .add(offset)
            .cast::<*mut UObjectBase>()
            .read()
    };
    (!value.is_null()).then_some(value)
}
