//! The game's menu (`UMenuWidget`, the main menu and the in-game one alike: `WBP_Menu`) and its
//! settings pages, to add a sub-menu of our own to the options the way the game makes its own.
//!
//! A menu shows a context, a path such as `MainMenu/Options/Gameplay`. `OpenMenu(name)` opens a
//! context (a name without the root, e.g. `Gameplay` from a button, is a sub-context of the current
//! one); then, in order: the menu makes a Back button if the context has a parent (Back reopens the
//! parent), its Blueprint's `OnMenuContextChanged` makes the context's buttons
//! (`UMenuButtonPanelWidget::CreateButton(text, icon, delegate)`) or shows its page
//! (`ShowMenuPage(class, title)`), and the panel pools the buttons left over from the previous
//! context (`RemoveUnusedButtons`), its widgets being reused. A button's click runs its delegate
//! (`UMenuButtonWidget::OnClicked`): the options' buttons each open their sub-context. A settings page
//! (`UMenuSettingsPageWidget`, e.g. `WBP_GameplaySettings`) lists the game's settings of its
//! categories (`SettingsCategories`, gameplay tags such as `Properties.Settings.Gameplay`) when
//! constructed (`AddProperties`). Found in BRMK 1.11.6 and game 1.11.7 (build 25831956); see
//! `docs/re/settings-menu.md`.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;
use brickrust::ue::tarray::TArray;
use brickrust::ue::uclass::UStruct;

use crate::framework::sig;
use crate::game::reflection::{self, Function, Parameters, SingleCall};
use crate::game::text::Text;
use crate::game::world;

/// `UMenuWidget::OpenMenu(FName InMenu)`, the C++ function: opens a context.
pub type OpenMenuFn = unsafe extern "C" fn(menu: *mut UObjectBase, context: FName);

/// `UMenuButtonPanelWidget::RemoveUnusedButtons()`: pools the buttons the context didn't use.
pub type RemoveUnusedButtonsFn = unsafe extern "C" fn(panel: *mut UObjectBase);

/// `UMenuButtonWidget::OnClicked()`: runs the button's delegate.
pub type ClickedFn = unsafe extern "C" fn(button: *mut UObjectBase);

/// `UMenuSettingsPageWidget::AddProperties_Implementation()`: lists the page's settings.
pub type AddPropertiesFn = unsafe extern "C" fn(page: *mut UObjectBase);

/// `TSoftObjectPtr` and `TSoftClassPtr` (0x28 bytes): a weak pointer to the object once found, and
/// its path. Made from a path alone, it finds the object (or loads it) when used.
#[repr(C)]
pub struct SoftPointer {
    /// `FWeakObjectPtr`: no object yet (`INDEX_NONE`).
    object_index: i32,
    serial_number: i32,
    tag_at_last_test: i32,
    _padding: u32,
    /// `FSoftObjectPath`: the package and object, then a sub-object path (an empty `FString`).
    path: FName,
    sub_path: [usize; 2],
}

const _: () = assert!(size_of::<SoftPointer>() == 0x28);
const _: () = assert!(core::mem::offset_of!(SoftPointer, path) == 0x10);

impl SoftPointer {
    pub fn new(path: FName) -> Self {
        Self {
            object_index: -1,
            serial_number: 0,
            tag_at_last_test: 0,
            _padding: 0,
            path,
            sub_path: [0; 2],
        }
    }
}

/// Button panels (`UMenuButtonPanelWidget`): a menu's buttons, and the small buttons of a property
/// row (lock, units, context menu; `UPropertyContainerWidget::GetMenuButtonPanelWidget`). Both make
/// their buttons the same way, pooling the widgets: a panel reuses its button widgets for the next
/// buttons it makes.
pub struct ButtonPanels {
    /// `UMenuButtonWidget::OnClicked` (game 0xEE7120), to hook.
    pub on_clicked: *const u8,
    /// `UMenuButtonPanelWidget::CreateButton(DisplayText, IconTexture, Delegate)`.
    create_button: Function,
    button_text: usize,
    button_icon: usize,
    button_made: usize,
    /// Offset of `UMenuButtonPanelWidget::Buttons`, the buttons in use (`TArray`).
    buttons: usize,
}

// SAFETY: code pointers and reflection records that live for the whole game; only used on the
// game thread.
unsafe impl Send for ButtonPanels {}
unsafe impl Sync for ButtonPanels {}

impl ButtonPanels {
    /// Finds everything. After engine init (reflection).
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let panel = class("MenuButtonPanelWidget")?;
        let button = class("MenuButtonWidget")?;
        let on_clicked = reflection::find_function(button, "OnClicked")
            .and_then(reflection::parameterless_native)
            .map_err(|reason| format!("OnClicked: {reason}"))?;
        let create_button = Function::find(panel, "CreateButton")?;
        Ok(Self {
            on_clicked,
            button_text: create_button.parameter("DisplayText", "TextProperty", 0x18)?,
            button_icon: create_button.parameter("IconTexture", "SoftObjectProperty", 0x28)?,
            button_made: create_button.parameter("ReturnValue", "ObjectProperty", 8)?,
            create_button,
            buttons: reflection::field_offset(panel, "Buttons", "ArrayProperty", 0x10)?,
        })
    }

    /// The buttons `panel` shows, in order.
    ///
    /// # Safety
    /// `panel` must be a live button panel, and the slice is only valid until it changes its
    /// buttons. Game thread only.
    pub unsafe fn buttons<'a>(&self, panel: *mut UObjectBase) -> &'a [*mut UObjectBase] {
        // SAFETY: per the contract; the field is an array of button pointers.
        unsafe {
            slice(
                &*panel
                    .cast::<u8>()
                    .add(self.buttons)
                    .cast::<TArray<*mut UObjectBase>>(),
            )
        }
    }

    /// The buttons `panel` shows, to reorder them.
    ///
    /// # Safety
    /// As for [`Self::buttons`]; only reorder the elements (the panel owns the array).
    unsafe fn buttons_mut<'a>(&self, panel: *mut UObjectBase) -> &'a mut [*mut UObjectBase] {
        // SAFETY: per the contract; the field is an array of button pointers.
        unsafe {
            let array = &*panel
                .cast::<u8>()
                .add(self.buttons)
                .cast::<TArray<*mut UObjectBase>>();
            match usize::try_from(array.num) {
                Ok(count) if count > 0 && !array.data.is_null() => {
                    core::slice::from_raw_parts_mut(array.data, count)
                }
                _ => &mut [],
            }
        }
    }

    /// Whether `button` is one of the buttons `panel` shows (it may have been pooled and reused by
    /// another panel since it was made).
    ///
    /// # Safety
    /// `panel` must be a live button panel. Game thread only.
    pub unsafe fn shows(&self, panel: *mut UObjectBase, button: *mut UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { self.buttons(panel).contains(&button) }
    }

    /// Adds a button showing `text` and the icon at `icon` (a texture's path) to `panel`, with no
    /// delegate: its clicks are told by `OnClicked` (see [`Self::on_clicked`]). The button, if made.
    ///
    /// # Safety
    /// `panel` must be a live button panel. Game thread only.
    pub unsafe fn add_button(
        &self,
        panel: *mut UObjectBase,
        text: &Text,
        icon: FName,
    ) -> Option<*mut UObjectBase> {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the parameters are written at their reflected offsets with
        // their types (an empty delegate is all zeros), and the text gets a reference of its own.
        unsafe {
            let base = parameters.as_mut_ptr();
            text.assign_to(base.add(self.button_text).cast());
            reflection::put(&mut parameters, self.button_icon, SoftPointer::new(icon));
            self.create_button.call(panel, &mut parameters);
            let made = reflection::take::<*mut UObjectBase>(&parameters, self.button_made);
            (!made.is_null()).then_some(made)
        }
    }
}

/// Where a button panel's buttons show: each in a cell of the panel's grid (its `ButtonPanel`, a
/// `UGridPanel`), row `index / NumButtonsPerRow` and column `index % NumButtonsPerRow` (all in row
/// 0 if that's 0), as `UMenuButtonPanelWidget::AddButton` places them (BRMK 0x3A8800) when
/// `CreateButton` makes them, in order. To check a panel shows its buttons in their order and put
/// back what doesn't (`docs/re/settings-menu.md`, "Buttons").
pub struct ButtonGrid {
    /// Offsets of `UMenuButtonPanelWidget::ButtonPanel` (the grid) and `NumButtonsPerRow`.
    grid: usize,
    per_row: usize,
    /// Offsets of `UWidget::Slot` and `UPanelSlot::Parent`.
    slot: usize,
    parent: usize,
    /// `UGridSlot`, its `Row` and `Column`, and their setters (which tell the grid).
    grid_slot: *const UStruct,
    row: usize,
    column: usize,
    set_row: SingleCall,
    set_column: SingleCall,
    /// `UWidget::Visibility`, `SetVisibility`, and `ESlateVisibility`'s `Collapsed` and `Visible`.
    visibility: usize,
    set_visibility: SingleCall,
    collapsed: u8,
    visible: u8,
}

// SAFETY: reflection records that live for the whole game; only used on the game thread.
unsafe impl Send for ButtonGrid {}
unsafe impl Sync for ButtonGrid {}

impl ButtonGrid {
    /// Finds everything. After engine init (reflection).
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let panel = class("MenuButtonPanelWidget")?;
        let widget = class("Widget")?;
        let grid_slot = class("GridSlot")?;
        let int = |class, name| reflection::field_offset(class, name, "IntProperty", 4);
        let visibility = |entry: &str| -> Result<u8, String> {
            let entry = format!("ESlateVisibility::{entry}");
            let value = reflection::enum_value("ESlateVisibility", &entry)?;
            u8::try_from(value).map_err(|_| format!("{entry} is {value}"))
        };
        Ok(Self {
            grid: reflection::field_offset(panel, "ButtonPanel", "ObjectProperty", 8)?,
            per_row: int(panel, "NumButtonsPerRow")?,
            slot: reflection::field_offset(widget, "Slot", "ObjectProperty", 8)?,
            parent: reflection::field_offset(class("PanelSlot")?, "Parent", "ObjectProperty", 8)?,
            grid_slot: grid_slot.cast(),
            row: int(grid_slot, "Row")?,
            column: int(grid_slot, "Column")?,
            set_row: SingleCall::find(grid_slot, "SetRow", "InRow", "IntProperty", 4)?,
            set_column: SingleCall::find(grid_slot, "SetColumn", "InColumn", "IntProperty", 4)?,
            visibility: reflection::field_offset(widget, "Visibility", "EnumProperty", 1)?,
            set_visibility: SingleCall::find(
                widget,
                "SetVisibility",
                "InVisibility",
                "EnumProperty",
                1,
            )?,
            collapsed: visibility("Collapsed")?,
            visible: visibility("Visible")?,
        })
    }

    /// Checks that `panel` shows its buttons in their order, `last` (those of them that are
    /// there) after the others, and puts back what doesn't: the order, then each button's cell and
    /// visibility. What was wrong, empty if nothing; a button shown twice isn't put back.
    ///
    /// # Safety
    /// `panel` must be a live button panel that isn't making buttons now. Game thread only.
    pub unsafe fn put_in_order(
        &self,
        panels: &ButtonPanels,
        panel: *mut UObjectBase,
        last: &[*mut UObjectBase],
    ) -> Vec<String> {
        let mut wrong = Vec::new();
        // SAFETY: per the contract; the buttons are live widgets (the panel holds them), each
        // field read at its reflected offset, and a widget's slot and its parent are live objects.
        unsafe {
            let buttons = panels.buttons_mut(panel);
            for (at, button) in buttons.iter().enumerate() {
                if let Some(first) = buttons[..at].iter().position(|other| other == button) {
                    wrong.push(format!("button {at} is button {first} too"));
                }
            }
            if !wrong.is_empty() {
                return wrong;
            }
            let wanted: Vec<_> = buttons
                .iter()
                .copied()
                .filter(|button| !last.contains(button))
                .chain(
                    last.iter()
                        .copied()
                        .filter(|button| buttons.contains(button)),
                )
                .collect();
            if wanted != buttons {
                let ours: Vec<String> = buttons
                    .iter()
                    .enumerate()
                    .filter(|(_, button)| last.contains(button))
                    .map(|(at, _)| at.to_string())
                    .collect();
                wrong.push(format!(
                    "the page's buttons were {} of {}",
                    ours.join(", "),
                    buttons.len()
                ));
                buttons.copy_from_slice(&wanted);
            }
            let grid = field::<*mut UObjectBase>(panel, self.grid);
            let per_row = field::<i32>(panel, self.per_row);
            for (at, &button) in buttons.iter().enumerate() {
                let at_i32 = i32::try_from(at).unwrap_or(i32::MAX);
                let (row, column) = if per_row > 0 {
                    (at_i32 / per_row, at_i32 % per_row)
                } else {
                    (0, at_i32)
                };
                let slot = field::<*mut UObjectBase>(button, self.slot);
                if slot.is_null() || !reflection::is_a(slot, self.grid_slot) {
                    wrong.push(format!("button {at} isn't in a grid"));
                    continue;
                }
                if field::<*mut UObjectBase>(slot, self.parent) != grid {
                    wrong.push(format!("button {at} is in another grid"));
                    continue;
                }
                let shown = (
                    field::<i32>(slot, self.row),
                    field::<i32>(slot, self.column),
                );
                if shown != (row, column) {
                    wrong.push(format!(
                        "button {at} was in row {}, column {}",
                        shown.0, shown.1
                    ));
                    self.set_row.call(slot, row);
                    self.set_column.call(slot, column);
                }
                if field::<u8>(button, self.visibility) == self.collapsed {
                    wrong.push(format!("button {at} was hidden"));
                    self.set_visibility.call(button, self.visible);
                }
            }
        }
        wrong
    }
}

/// The menu functions and fields we use.
pub struct Menus {
    /// `UMenuWidget::OpenMenu`'s C++ function (game 0xEEBF30), to hook: the reflected one's thunk
    /// (game 0xFE6590) reads the context and calls it last.
    pub open_menu: *const u8,
    /// `UMenuButtonPanelWidget::RemoveUnusedButtons` (game 0xEF3F00), to hook.
    pub remove_unused_buttons: *const u8,
    /// `UMenuSettingsPageWidget::AddProperties_Implementation`, to hook.
    pub add_properties: *const u8,
    /// The menus' button panels.
    pub panels: ButtonPanels,
    /// The reflected `UMenuWidget::OpenMenu(InMenu)`, to open a context as the game's buttons do.
    open: Function,
    open_context: usize,
    /// `UMenuWidget::CloseMenu()`: closes the menu, as its close button does.
    close: Function,
    /// `UMenuWidget::ShowMenuPage(WidgetClass, TitleText)`.
    show_page: Function,
    page_class: usize,
    page_title: usize,
    /// Offset of `UMainWidgetBase::ButtonPanel` in a menu.
    button_panel: usize,
    /// Offset of `UMenuWidget::CurrentMenuPage`.
    current_page: usize,
    /// Offset of `UMenuSettingsPageWidget::SettingsCategories` (`FGameplayTagContainer`).
    categories: usize,
}

// SAFETY: code pointers and reflection records that live for the whole game; only used on the
// game thread.
unsafe impl Send for Menus {}
unsafe impl Sync for Menus {}

impl Menus {
    /// Finds everything. After engine init (reflection).
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let menu = class("MenuWidget")?;
        let panel = class("MenuButtonPanelWidget")?;
        let page = class("MenuSettingsPageWidget")?;

        let open_menu_thunk =
            reflection::native_thunk(reflection::find_function(menu, "OpenMenu")?)
                .map_err(|reason| format!("OpenMenu: {reason}"))?;
        let open_menu = *sig::direct_calls(open_menu_thunk)
            .last()
            .filter(|&&target| sig::is_function_start(target))
            .ok_or("OpenMenu's thunk calls no function last")?;
        let parameterless = |class, name| {
            reflection::find_function(class, name)
                .and_then(reflection::parameterless_native)
                .map_err(|reason| format!("{name}: {reason}"))
        };
        let add_properties = reflection::find_function(page, "AddProperties")
            .and_then(|function| reflection::parameterless_virtual(page, function))
            .map_err(|reason| format!("AddProperties: {reason}"))?;

        let open = Function::find(menu, "OpenMenu")?;
        let show_page = Function::find(menu, "ShowMenuPage")?;
        Ok(Self {
            open_menu,
            remove_unused_buttons: parameterless(panel, "RemoveUnusedButtons")?,
            add_properties,
            panels: ButtonPanels::find()?,
            open_context: open.parameter("InMenu", "NameProperty", 8)?,
            open,
            close: Function::find(menu, "CloseMenu")?,
            page_class: show_page.parameter("WidgetClass", "SoftClassProperty", 0x28)?,
            page_title: show_page.parameter("TitleText", "TextProperty", 0x18)?,
            show_page,
            button_panel: reflection::field_offset(menu, "ButtonPanel", "ObjectProperty", 8)?,
            current_page: reflection::field_offset(menu, "CurrentMenuPage", "ObjectProperty", 8)?,
            categories: reflection::field_offset(
                page,
                "SettingsCategories",
                "StructProperty",
                0x20,
            )?,
        })
    }

    /// Opens the context `context` in `menu`, as the game's buttons do (a sub-context of the current
    /// one unless it starts with the root).
    ///
    /// # Safety
    /// `menu` must be a live menu. Game thread only.
    pub unsafe fn open(&self, menu: *mut UObjectBase, context: FName) {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the name is written at its reflected offset.
        unsafe {
            reflection::put(&mut parameters, self.open_context, context);
            self.open.call(menu, &mut parameters);
        }
    }

    /// Closes `menu`.
    ///
    /// # Safety
    /// `menu` must be a live menu. Game thread only.
    pub unsafe fn close(&self, menu: *mut UObjectBase) {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the function has no parameters.
        unsafe { self.close.call(menu, &mut parameters) };
    }

    /// The panel of `menu`'s buttons.
    ///
    /// # Safety
    /// `menu` must be a live menu. Game thread only.
    pub unsafe fn button_panel(&self, menu: *mut UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; the field is an object pointer of the menu's class.
        let panel = unsafe { field::<*mut UObjectBase>(menu, self.button_panel) };
        (!panel.is_null()).then_some(panel)
    }

    /// Shows the page of class `class` (a path) in `menu`, titled `title`.
    ///
    /// # Safety
    /// `menu` must be a live menu. Game thread only.
    pub unsafe fn show_page(&self, menu: *mut UObjectBase, class: FName, title: &Text) {
        let mut parameters = Parameters::new();
        // SAFETY: as for `add_button`.
        unsafe {
            reflection::put(&mut parameters, self.page_class, SoftPointer::new(class));
            title.assign_to(parameters.as_mut_ptr().add(self.page_title).cast());
            self.show_page.call(menu, &mut parameters);
        }
    }

    /// The page `menu` shows, or null.
    ///
    /// # Safety
    /// `menu` must be a live menu. Game thread only.
    pub unsafe fn current_page(&self, menu: *mut UObjectBase) -> *mut UObjectBase {
        // SAFETY: per the contract; the field is an object pointer of the menu's class.
        unsafe { field(menu, self.current_page) }
    }

    /// The categories a settings page lists (`SettingsCategories.GameplayTags`; a tag is an
    /// `FName`).
    ///
    /// # Safety
    /// `page` must be a live settings page (`UMenuSettingsPageWidget`). Game thread only.
    pub unsafe fn categories<'a>(&self, page: *mut UObjectBase) -> &'a mut [FName] {
        // SAFETY: per the contract; `FGameplayTagContainer` starts with its tags' array.
        unsafe {
            let tags = &*page
                .cast::<u8>()
                .add(self.categories)
                .cast::<TArray<FName>>();
            match usize::try_from(tags.num) {
                Ok(count) if count > 0 && !tags.data.is_null() => {
                    core::slice::from_raw_parts_mut(tags.data, count)
                }
                _ => &mut [],
            }
        }
    }
}

/// Opening the menu as Escape does: `UWindowManagerWidget::SetMenuOpen(true)` (reflected; BRMK
/// 0x450D70: if `CanOpenOrCloseMenu`, sets the flag and `UpdateActiveWidget`, which makes the menu
/// the active widget). The window manager is `UWorld::OwningGameInstance`'s `WindowManagerWidget`
/// (reflected on `UBrickGameInstance`, as `game::zoom_notification` finds it). The menu widget's
/// `NativeConstruct` (BRMK 0x3D3E20) then opens its root context (`InGameMenu`, or `MainMenu` in
/// the main menu) with `OpenMenu`. While it's open, the menu is the window manager's
/// `ActiveWidget` (reflected).
pub struct WindowManager {
    game_instance: usize,
    brick_game_instance: *const UStruct,
    window_manager: usize,
    active_widget: usize,
    set_menu_open: Function,
    open_value: usize,
}

// SAFETY: reflection records that live for the whole game; only used on the game thread.
unsafe impl Send for WindowManager {}
unsafe impl Sync for WindowManager {}

impl WindowManager {
    /// Finds everything. After engine init (reflection).
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let object = |class, name| reflection::field_offset(class, name, "ObjectProperty", 8);
        let brick_game_instance = class("BrickGameInstance")?;
        let window_manager = class("WindowManagerWidget")?;
        let set_menu_open = Function::find(window_manager, "SetMenuOpen")?;
        // Its one parameter, a bool, whatever it's called.
        let function = reflection::find_function(window_manager, "SetMenuOpen")?;
        // SAFETY: a live function record; its properties are its parameters.
        let open_value = unsafe {
            let mut parameters = reflection::properties(function.cast());
            match (parameters.next(), parameters.next()) {
                (Some(open), None) if reflection::is_kind(open, "BoolProperty") => {
                    (*open).offset_internal as usize
                }
                _ => return Err("SetMenuOpen: unexpected parameters".into()),
            }
        };
        Ok(Self {
            game_instance: object(class("World")?, "OwningGameInstance")?,
            brick_game_instance: brick_game_instance.cast(),
            window_manager: object(brick_game_instance, "WindowManagerWidget")?,
            active_widget: object(window_manager, "ActiveWidget")?,
            open_value,
            set_menu_open,
        })
    }

    /// Opens the menu; whether there was a window manager to ask (the game may still refuse).
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn open_menu(&self) -> bool {
        // SAFETY: on the game thread.
        let Some(manager) = (unsafe { self.manager() }) else {
            return false;
        };
        let mut parameters = Parameters::new();
        // SAFETY: a live window manager; the parameter is written at its reflected offset.
        unsafe {
            reflection::put(&mut parameters, self.open_value, true);
            self.set_menu_open.call(manager, &mut parameters);
        }
        true
    }

    /// Whether `widget` (a menu) is the one showing, open.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn is_active(&self, widget: *mut UObjectBase) -> bool {
        // SAFETY: on the game thread; the field is an object pointer of the manager's class.
        unsafe {
            self.manager().is_some_and(|manager| {
                field::<*mut UObjectBase>(manager, self.active_widget) == widget
            })
        }
    }

    /// The current world's window manager.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn manager(&self) -> Option<*mut UObjectBase> {
        // SAFETY: on the game thread; each object comes from a reflected object property of the
        // one before (the game instance's class checked).
        unsafe {
            let world = world::current();
            if world.is_null() {
                return None;
            }
            let instance = field::<*mut UObjectBase>(world, self.game_instance);
            if instance.is_null() || !reflection::is_a(instance, self.brick_game_instance) {
                return None;
            }
            let manager = field::<*mut UObjectBase>(instance, self.window_manager);
            (!manager.is_null()).then_some(manager)
        }
    }
}

/// Reads the `T` at `offset` in `object`.
///
/// # Safety
/// `object` must be live, with a `T` at `offset`.
unsafe fn field<T: Copy>(object: *mut UObjectBase, offset: usize) -> T {
    // SAFETY: per the contract.
    unsafe { object.cast::<u8>().add(offset).cast::<T>().read() }
}

/// The elements of a game array.
///
/// # Safety
/// `array` must be a live array of `T`s.
unsafe fn slice<'a, T>(array: &TArray<T>) -> &'a [T] {
    match usize::try_from(array.num) {
        // SAFETY: per the contract; a non-empty array's data holds `num` elements.
        Ok(count) if count > 0 && !array.data.is_null() => unsafe {
            core::slice::from_raw_parts(array.data, count)
        },
        _ => &[],
    }
}

/// What separates the parts of a menu context (`UMenuWidget::SplitMenuContext` splits on it).
const CONTEXT_SEPARATOR: char = '/';

/// The last part of a menu context (`MainMenu/Options`: `Options`).
///
/// # Safety
/// `context` must be a valid name.
pub unsafe fn context_leaf(context: &FName) -> String {
    // SAFETY: per the contract.
    let text = unsafe { reflection::name_text(context) };
    match text.rsplit_once(CONTEXT_SEPARATOR) {
        Some((_, leaf)) => leaf.to_owned(),
        None => text,
    }
}
