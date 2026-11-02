//! The game's popups (`docs/re/popups.md`): a question with Confirm and Cancel
//! (`UMessagePopupParams`, as the game's "Quit to desktop?") and the ban dialog
//! (`UKickPlayerPopupParams`, the Players page's), opened and answered through reflection only.
//!
//! A popup is a params object (the data) shown by a `UPopupContainerWidget` of the window
//! manager. Every way of confirming goes through the container's C++ `ConfirmPopup`, which a
//! feature hooks ([`Popups::confirm_popup`]) and tells its popups apart by their params
//! ([`Popups::params_of`]). Everything else (Cancel, Escape, the editor closing) is not confirming.
//!
//! Button widgets are pooled: a closed popup's buttons go to other popups and menus as they are,
//! so a button hidden ([`Popups::wait`]) must be shown again once its popup closes
//! ([`Popups::unhide`]), or the next one to get it lacks it.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::{UClass, UFunction, UStruct};

use crate::game::menu::ButtonPanels;
use crate::game::reflection::{self, BoolField, Function, Parameters, SingleCall};
use crate::game::slider_text::{FString, OwnedFString};
use crate::game::text::Text;
use crate::game::weak_object::WeakObject;
use crate::game::world;

/// `UPopupContainerWidget::ConfirmPopup`, its C++ function (no parameters).
pub type ConfirmFn = unsafe extern "C" fn(container: *mut UObjectBase);

/// Size of an `FUniqueNetIdRepl` (vtable, shared pointer, replication bytes).
pub const NET_ID_SIZE: usize = 0x28;

/// What a confirmed ban dialog says.
pub struct Ban {
    pub name: String,
    pub reason: String,
    pub seconds: u64,
}

/// A popup button hidden by [`Popups::wait`], and its visibility before.
pub struct HiddenButton {
    button: WeakObject,
    shown: u8,
}

/// An `TArray`'s header.
#[repr(C)]
struct RawArray<T> {
    data: *const T,
    num: i32,
    max: i32,
}

pub struct Popups {
    /// `UWorld::OwningGameInstance`, `UBrickGameInstance`, its `WindowManagerWidget` and the
    /// manager's `PopupContainerWidgets`.
    game_instance: usize,
    brick_game_instance: *const UClass,
    window_manager: usize,
    containers: usize,
    /// `UBrickGameInstance::CreatePopupParams(Class) -> Params` and `OpenPopup(Handle&, Params,
    /// bToggleOpen) -> bool`.
    create_params: Function,
    create_class: usize,
    create_result: usize,
    open_popup: Function,
    open_params: usize,
    open_result: usize,
    /// `UMessagePopupParams`, its `TitleText` (the whole question) and `bCanCancel`.
    message_class: *const UClass,
    title_text: usize,
    can_cancel: BoolField,
    /// `UPopupContainerWidget::PopupParams`, `PopupWidget` and `CancelPopup`.
    container_params: usize,
    container_widget: usize,
    cancel_popup: Function,
    /// `UPopupContainerWidget::ConfirmPopup`'s C++ function, to hook.
    pub confirm_popup: *const u8,
    /// `UKickPlayerPopupParams` and its `PlayerName`, `KickReason`, `KickDuration` (`FTimespan`,
    /// 100 ns ticks); its default object's `PlayerId`, an empty id.
    kick_class: *const UClass,
    kick_name: usize,
    kick_reason: usize,
    kick_duration: usize,
    empty_id: *const u8,
    /// `ABasePlayerController::OpenKickPlayerPopup(OtherPlayerId, OtherPlayerName)`.
    open_kick: Function,
    open_kick_id: usize,
    open_kick_name: usize,
    /// `UKickPlayerPopupWidget::UpdateCanConfirmKick(bool)` (a Blueprint event) and its parameter.
    can_confirm_kick: Function,
    can_confirm_value: usize,
    /// The container's `ButtonPanel` (`UMainWidgetBase`'s, as a menu's), its buttons, and
    /// `UWidget::Visibility` and `SetVisibility` with `ESlateVisibility::Collapsed`, to hide one.
    container_buttons: usize,
    panels: ButtonPanels,
    visibility: usize,
    set_visibility: SingleCall,
    collapsed: u8,
}

// SAFETY: reflection records and a default object living as long as the game; only used on the
// game thread.
unsafe impl Send for Popups {}
unsafe impl Sync for Popups {}

fn class(name: &'static str) -> Result<*const UClass, String> {
    reflection::find_class(name).ok_or(format!("class {name} not found"))
}

/// The offset of `function`'s return value (`result`) or of its one other parameter of `kind` and
/// `size`, whatever it's called.
pub fn parameter_by_kind(
    function: &'static str,
    found: *const UFunction,
    kind: &'static str,
    size: i32,
    result: bool,
) -> Result<usize, String> {
    // SAFETY: a live function record; its properties are its parameters.
    unsafe {
        let mut matching = reflection::properties(found.cast()).filter(|&property| {
            reflection::is_kind(property, kind)
                && (*property).element_size == size
                && ((*property).property_flags & reflection::RETURN_PARAMETER != 0) == result
        });
        match (matching.next(), matching.next()) {
            (Some(property), None) => Ok((*property).offset_internal as usize),
            _ => Err(format!("{function}: no single {kind} parameter")),
        }
    }
}

impl Popups {
    /// Finds everything. After engine init (reflection).
    pub fn find() -> Result<Self, String> {
        let object = |class, name| reflection::field_offset(class, name, "ObjectProperty", 8);
        let brick_game_instance = class("BrickGameInstance")?;
        let manager = class("WindowManagerWidget")?;
        let container = class("PopupContainerWidget")?;
        let message_class = class("MessagePopupParams")?;
        let kick_class = class("KickPlayerPopupParams")?;
        let string = |name| {
            reflection::field_offset(kick_class, name, "StrProperty", size_of::<FString>() as i32)
        };

        let create = reflection::find_function(brick_game_instance, "CreatePopupParams")?;
        let open = Function::find(brick_game_instance, "OpenPopup")?;
        let open_kick = Function::find(class("BasePlayerController")?, "OpenKickPlayerPopup")?;
        let can_confirm_kick =
            Function::find(class("KickPlayerPopupWidget")?, "UpdateCanConfirmKick")?;
        let kick_id =
            reflection::field_offset(kick_class, "PlayerId", "StructProperty", NET_ID_SIZE as i32)?;
        // SAFETY: a live class; its default object lives as long as the game.
        let defaults = unsafe { reflection::default_object(kick_class) }
            .ok_or("KickPlayerPopupParams has no default object")?;
        let confirm = reflection::find_function(container, "ConfirmPopup")?;
        Ok(Self {
            game_instance: object(class("World")?, "OwningGameInstance")?,
            brick_game_instance,
            window_manager: object(brick_game_instance, "WindowManagerWidget")?,
            containers: reflection::field_offset(
                manager,
                "PopupContainerWidgets",
                "ArrayProperty",
                16,
            )?,
            create_class: parameter_by_kind(
                "CreatePopupParams",
                create,
                "ClassProperty",
                8,
                false,
            )?,
            create_result: parameter_by_kind(
                "CreatePopupParams",
                create,
                "ObjectProperty",
                8,
                true,
            )?,
            create_params: Function::find(brick_game_instance, "CreatePopupParams")?,
            open_params: open.parameter("PopupParams", "ObjectProperty", 8)?,
            open_result: open.parameter("ReturnValue", "BoolProperty", 1)?,
            open_popup: open,
            message_class,
            title_text: reflection::field_offset(message_class, "TitleText", "TextProperty", 0x18)?,
            can_cancel: reflection::bool_field(message_class.cast(), "bCanCancel")?,
            container_params: object(container, "PopupParams")?,
            container_widget: object(container, "PopupWidget")?,
            container_buttons: object(container, "ButtonPanel")?,
            panels: ButtonPanels::find()?,
            visibility: reflection::field_offset(
                class("Widget")?,
                "Visibility",
                "EnumProperty",
                1,
            )?,
            set_visibility: SingleCall::find(
                class("Widget")?,
                "SetVisibility",
                "InVisibility",
                "EnumProperty",
                1,
            )?,
            collapsed: {
                let entry = "ESlateVisibility::Collapsed";
                let value = reflection::enum_value("ESlateVisibility", entry)?;
                u8::try_from(value).map_err(|_| format!("{entry} is {value}"))?
            },
            cancel_popup: Function::find(container, "CancelPopup")?,
            confirm_popup: reflection::parameterless_native(confirm)?,
            kick_class,
            kick_name: string("PlayerName")?,
            kick_reason: string("KickReason")?,
            kick_duration: reflection::field_offset(
                kick_class,
                "KickDuration",
                "StructProperty",
                8,
            )?,
            // SAFETY: the field lies within the default object.
            empty_id: unsafe { defaults.cast::<u8>().add(kick_id) },
            open_kick_id: open_kick.parameter(
                "OtherPlayerId",
                "StructProperty",
                NET_ID_SIZE as i32,
            )?,
            open_kick_name: open_kick.parameter(
                "OtherPlayerName",
                "StrProperty",
                size_of::<FString>() as i32,
            )?,
            open_kick,
            can_confirm_value: can_confirm_kick.only_parameter(&["BoolProperty"], 1)?,
            can_confirm_kick,
        })
    }

    /// The game instance, if the current world has ours.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn instance(&self) -> Option<*mut UObjectBase> {
        // SAFETY: on the game thread; the object comes from a reflected object property, its
        // class checked.
        unsafe {
            let world = world::current();
            if world.is_null() {
                return None;
            }
            let instance = read::<*mut UObjectBase>(world, self.game_instance);
            (!instance.is_null() && reflection::is_a(instance, self.brick_game_instance.cast()))
                .then_some(instance)
        }
    }

    /// Opens a question with Confirm and Cancel, showing `question`. Returns its params object:
    /// [`Popups::params_of`] gives it back for the container confirming it.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn ask(&self, question: &Text) -> Result<*mut UObjectBase, String> {
        // SAFETY: per the contract.
        unsafe { self.message(question, true) }
    }

    /// Opens a popup showing `text` with only Confirm (`bCanCancel` off), to tell something that
    /// can't wait for the HUD; returns its params. Confirming closes it.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn inform(&self, text: &Text) -> Result<*mut UObjectBase, String> {
        // SAFETY: per the contract.
        unsafe { self.message(text, false) }
    }

    /// Opens a popup showing `text` with only Cancel (e.g. while waiting for something); returns
    /// its params and the Confirm button hidden, which [`Popups::unhide`] must get back once the
    /// popup closes. Escape or Cancel closes it; [`Popups::cancel`] too.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn wait(
        &self,
        text: &Text,
    ) -> Result<(*mut UObjectBase, Option<HiddenButton>), String> {
        // SAFETY: on the game thread; the container just made its buttons, Confirm first, and they
        // are live widgets with a reflected visibility.
        unsafe {
            let params = self.message(text, true)?;
            let container = self
                .container_of(params)
                .ok_or("its container wasn't found")?;
            let panel = read::<*mut UObjectBase>(container, self.container_buttons);
            let mut hidden = None;
            if !panel.is_null()
                && let [confirm, _cancel] = self.panels.buttons(panel)
            {
                hidden = Some(HiddenButton {
                    button: WeakObject::new(*confirm),
                    shown: read(*confirm, self.visibility),
                });
                self.set_visibility.call(*confirm, self.collapsed);
            }
            Ok((params, hidden))
        }
    }

    /// Shows the button [`Popups::wait`] hid again, unless something showed it since.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn unhide(&self, hidden: HiddenButton) {
        let Some(button) = hidden.button.get() else {
            return;
        };
        // SAFETY: on the game thread; a live widget with a reflected visibility.
        unsafe {
            if read::<u8>(button, self.visibility) == self.collapsed {
                self.set_visibility.call(button, hidden.shown);
            }
        }
    }

    /// The container showing the popup of `params`, if one does.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn container_of(&self, params: *mut UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: on the game thread.
        unsafe {
            self.shown()
                .into_iter()
                .find(|&(_, shown)| shown == params)
                .map(|(container, _)| container)
        }
    }

    /// Opens a message popup showing `question`, with Confirm and (if `can_cancel`) Cancel;
    /// returns its params.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn message(
        &self,
        question: &Text,
        can_cancel: bool,
    ) -> Result<*mut UObjectBase, String> {
        // SAFETY: on the game thread; each parameter goes at its reflected offset, and the params
        // object made is a live `UMessagePopupParams`.
        unsafe {
            let instance = self.instance().ok_or("no game instance")?;
            let mut parameters = Parameters::new();
            reflection::put(&mut parameters, self.create_class, self.message_class);
            self.create_params.call(instance, &mut parameters);
            let params: *mut UObjectBase = reflection::take(&parameters, self.create_result);
            if params.is_null() || !reflection::is_a(params, self.message_class.cast()) {
                return Err("no popup params made".into());
            }
            question.assign_to(params.cast::<u8>().add(self.title_text).cast());
            self.can_cancel.set(params.cast(), can_cancel);
            // A zero handle names no popup: a new one opens.
            let mut parameters = Parameters::new();
            reflection::put(&mut parameters, self.open_params, params);
            self.open_popup.call(instance, &mut parameters);
            if !reflection::take::<bool>(&parameters, self.open_result) {
                return Err("the game didn't open it".into());
            }
            Ok(params)
        }
    }

    /// The params of the popup `container` shows (null if none).
    ///
    /// # Safety
    /// `container` must be a live `UPopupContainerWidget`. Game thread only.
    pub unsafe fn params_of(&self, container: *mut UObjectBase) -> *mut UObjectBase {
        // SAFETY: per the contract; a reflected object property.
        unsafe { read(container, self.container_params) }
    }

    /// Closes the popup `container` shows as cancelled.
    ///
    /// # Safety
    /// As for [`Popups::params_of`].
    pub unsafe fn cancel(&self, container: *mut UObjectBase) {
        // SAFETY: per the contract; `CancelPopup` takes no parameters.
        unsafe { self.cancel_popup.call(container, &mut Parameters::new()) }
    }

    /// The popups shown (and pooled ones, maybe): their containers and params.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn shown(&self) -> Vec<(*mut UObjectBase, *mut UObjectBase)> {
        // SAFETY: on the game thread; the manager's array of containers, a reflected property.
        unsafe {
            let Some(instance) = self.instance() else {
                return Vec::new();
            };
            let manager = read::<*mut UObjectBase>(instance, self.window_manager);
            if manager.is_null() {
                return Vec::new();
            }
            let array = manager
                .cast::<u8>()
                .add(self.containers)
                .cast::<RawArray<*mut UObjectBase>>();
            let len = usize::try_from((*array).num).unwrap_or(0);
            if (*array).data.is_null() || len == 0 {
                return Vec::new();
            }
            core::slice::from_raw_parts((*array).data, len)
                .iter()
                .filter(|container| !container.is_null())
                .map(|&container| (container, self.params_of(container)))
                .collect()
        }
    }

    /// Opens the game's ban dialog for the player `name`, with `id` (the bytes of the match's
    /// `FUniqueNetIdRepl` for them, read only during the call) or an empty id. Returns the
    /// container showing it and its params.
    ///
    /// # Safety
    /// `controller` must be the local `ABasePlayerController`, and `id` a live `FUniqueNetIdRepl`.
    /// Game thread only.
    pub unsafe fn open_ban(
        &self,
        controller: *mut UObjectBase,
        id: Option<*const u8>,
        name: &str,
    ) -> Result<(*mut UObjectBase, *mut UObjectBase), String> {
        // SAFETY: per the contract. The id goes in as a bitwise copy of a live one and the name as
        // our string's header: the function only reads both (it copies them into its params), and
        // ProcessEvent destroys neither.
        unsafe {
            let before: Vec<_> = self.shown().into_iter().map(|(_, params)| params).collect();
            let string = OwnedFString::new(name);
            let mut parameters = Parameters::new();
            let base = parameters.as_mut_ptr();
            core::ptr::copy_nonoverlapping(
                id.unwrap_or(self.empty_id),
                base.add(self.open_kick_id),
                NET_ID_SIZE,
            );
            core::ptr::copy_nonoverlapping(
                string.as_ptr().cast::<u8>(),
                base.add(self.open_kick_name),
                size_of::<FString>(),
            );
            self.open_kick.call(controller, &mut parameters);
            self.shown()
                .into_iter()
                .find(|&(_, params)| {
                    !params.is_null()
                        && !before.contains(&params)
                        && reflection::is_a(params, self.kick_class.cast())
                })
                .ok_or_else(|| "the game didn't open it".into())
        }
    }

    /// Whether `container` still shows `params`.
    ///
    /// # Safety
    /// `container` must be a live `UPopupContainerWidget`. Game thread only.
    pub unsafe fn shows(&self, container: *mut UObjectBase, params: *mut UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { self.shown().contains(&(container, params)) }
    }

    /// Enables the Confirm of the ban dialog `container` shows (the game only does for players with
    /// an id). Whether its widget was there to enable.
    ///
    /// # Safety
    /// `container` must be a live `UPopupContainerWidget` showing a ban dialog. Game thread only.
    pub unsafe fn enable_ban_confirm(&self, container: *mut UObjectBase) -> bool {
        // SAFETY: per the contract; the widget is a reflected object property, the event's one
        // parameter a bool.
        unsafe {
            let widget = read::<*mut UObjectBase>(container, self.container_widget);
            if widget.is_null() {
                return false;
            }
            let mut parameters = Parameters::new();
            reflection::put(&mut parameters, self.can_confirm_value, true);
            self.can_confirm_kick
                .call_event(widget, &mut parameters)
                .is_ok()
        }
    }

    /// What the ban dialog of `params` says, if they're a ban dialog's.
    ///
    /// # Safety
    /// `params` must be a live object. Game thread only.
    pub unsafe fn ban(&self, params: *mut UObjectBase) -> Option<Ban> {
        // SAFETY: per the contract; the class is checked, the fields are reflected.
        unsafe {
            if !reflection::is_a(params, self.kick_class.cast::<UStruct>()) {
                return None;
            }
            let string =
                |offset: usize| (*params.cast::<u8>().add(offset).cast::<FString>()).to_string();
            let ticks = read::<i64>(params, self.kick_duration);
            Some(Ban {
                name: string(self.kick_name),
                reason: string(self.kick_reason),
                seconds: u64::try_from(ticks / 10_000_000).unwrap_or(0),
            })
        }
    }
}

/// Reads the `T` at `offset` in `object`.
///
/// # Safety
/// `object` must be live, with a `T` at `offset`.
unsafe fn read<T: Copy>(object: *mut UObjectBase, offset: usize) -> T {
    // SAFETY: per the contract.
    unsafe { object.cast::<u8>().add(offset).cast::<T>().read_unaligned() }
}
