//! The HUD's zoom notification (`UGameOverlayWidget`, the Blueprint `WBP_GameOverlay`): the zoom
//! icon and ratio (0 to 100) shown while the view zooms. Found through BRMK 1.11.6's exports and
//! Blueprints, then in game 1.11.7 (build 25831956); see `docs/re/editor-camera.md`.
//!
//! The overlay follows the view target's zoom (its `IViewTargetInterface`'s zoom delegate, which
//! `UpdateZoom` broadcasts each frame the ratio changes) with `OnCameraZoomRatioChanged`. That gets
//! the notification called "zoom" and calls the Blueprint event `UpdateCameraZoomNotification`,
//! which puts `NumberToText(Ratio, Percent)` and the zoom icon in it; the notification's native
//! `InitializeNotification` sets its `TextBlock`'s text and its icon.
//!
//! Any other notification works the same way, so [`ZoomNotification::show_text`] shows an
//! arbitrary text under an arbitrary context name (a slot of its own), without an icon update: see
//! its doc for what the icon then shows.

use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;
use brickrust::ue::uclass::UStruct;

use crate::framework::sig::FunctionSig;
use crate::game::kismet::{self, Names, Texts};
use crate::game::reflection::{self, Function, Parameters};
use crate::game::text::Text;
use crate::game::ui_library::{self, NumberText};
use crate::game::world;

/// `void UGameOverlayWidget::OnCameraZoomRatioChanged(float NewZoomRatio)` (BRMK 0x338F00, game RVA
/// 0xE7BE70): shows the zoom notification for the view target's new zoom ratio (0 to 1).
///
/// ```text
/// if (UHUDNotificationWidget* Notification = CreateHUDNotification(FName("zoom"), true))
///     UpdateCameraZoomNotification(Notification, NewZoomRatio);
/// ```
pub const ZOOM_CHANGED: FunctionSig = FunctionSig {
    name: "UGameOverlayWidget::OnCameraZoomRatioChanged",
    // The whole function: the name's characters (their address at +0x1C), CreateHUDNotification
    // (called at +0x31), then UpdateCameraZoomNotification (called at +0x44).
    pattern: "40 53 48 83 ec 30 48 8b d9 0f 29 74 24 20 48 8d 4c 24 ?? 41 b8 01 00 00 00 48 8d 15 ?? ?? ?? ?? 0f 28 f1 e8 ?? ?? ?? ?? 48 8b d0 41 b0 01 48 8b cb e8 ?? ?? ?? ?? 48 85 c0 74 ?? 0f 28 d6 48 8b d0 48 8b cb e8",
};

/// [`ZOOM_CHANGED`]'s signature.
pub type ZoomChangedFn = unsafe extern "C" fn(overlay: *mut UObjectBase, ratio: f32);

/// `UHUDNotificationWidget* UGameOverlayWidget::CreateHUDNotification(const FName& Context, bool
/// bAutoEnd)` (BRMK 0x313D50): the notification called `Context` if it shows, else a new one (from
/// the widget pool), unless the overlay hides those (`ShouldShowHUDNotifications`); then
/// (re)started, ending by itself after a while with `bAutoEnd` (`StartNotification`).
type CreateFn = unsafe extern "C" fn(
    overlay: *mut UObjectBase,
    context: *const FName,
    auto_end: bool,
) -> *mut UObjectBase;

/// `void UGameOverlayWidget::UpdateCameraZoomNotification(UHUDNotificationWidget*, float)` (BRMK
/// 0x53F840): calls the Blueprint event of that name.
type UpdateFn =
    unsafe extern "C" fn(overlay: *mut UObjectBase, notification: *mut UObjectBase, ratio: f32);

/// The notification's name, which [`ZOOM_CHANGED`] loads.
const NAME: &str = "zoom";

/// What [`ZOOM_CHANGED`]'s code tells: itself and what it calls.
#[derive(Clone, Copy)]
pub struct ZoomCode {
    /// `OnCameraZoomRatioChanged`'s start.
    pub function: *const u8,
    create: CreateFn,
    update: UpdateFn,
}

// SAFETY: points into the game's code; only used on the game thread (and while loading).
unsafe impl Send for ZoomCode {}
unsafe impl Sync for ZoomCode {}

/// Finds [`ZoomCode`], checking that the function is the one for the notification called "zoom".
/// Searched once and kept: `ortho_view` hooks the function, after which its start no longer
/// matches the pattern.
pub fn code() -> Result<ZoomCode, String> {
    static CODE: OnceLock<Result<ZoomCode, String>> = OnceLock::new();
    CODE.get_or_init(search).clone()
}

fn search() -> Result<ZoomCode, String> {
    let found = ZOOM_CHANGED.locate()?;
    // SAFETY: 32-bit displacements inside the matched pattern (see its comment), each relative to
    // the end of its instruction; the name's characters are constant data of the game's.
    unsafe {
        let target = |displacement: usize, end: usize| {
            let offset = found
                .address
                .add(displacement)
                .cast::<i32>()
                .read_unaligned();
            found.address.add(end).offset(offset as isize)
        };
        let name = core::slice::from_raw_parts(target(0x1C, 0x20), NAME.len() + 1);
        if !name.starts_with(NAME.as_bytes()) || name[NAME.len()] != 0 {
            return Err(format!(
                "{}: not the notification called '{NAME}'",
                ZOOM_CHANGED.name
            ));
        }
        Ok(ZoomCode {
            function: found.function,
            create: core::mem::transmute::<*const u8, CreateFn>(target(0x32, 0x36)),
            update: core::mem::transmute::<*const u8, UpdateFn>(target(0x45, 0x49)),
        })
    }
}

/// The notification and what writing its text takes (after engine init).
pub struct ZoomNotification {
    code: ZoomCode,
    /// [`NAME`] as an `FName`.
    name: FName,
    names: Names,
    /// Where the overlay is, as `UGameOverlayWidget::Get` finds it: `UWorld::OwningGameInstance`;
    /// in a `BrickGameInstance`, its `WindowManagerWidget`; that one's `ActiveWidget`, when it's
    /// the HUD (a `HUDContainerWidget`); and that one's `GameOverlayWidget`.
    game_instance: usize,
    brick_game_instance: *const UStruct,
    window_manager: usize,
    active_widget: usize,
    hud: *const UStruct,
    overlay: usize,
    /// Offset of `UHUDNotificationWidget::TextBlock`.
    text_block: usize,
    /// `UTextBlock::SetText(FText InText)`, and where `InText` goes.
    set_text: Function,
    set_text_value: usize,
    /// `UUIFunctionLibrary::NumberToText` (reflected; see `game::ui_library`), and the value
    /// types used with it here: `ENumericValueType::Percent` and `ENumericValueType::DistanceAuto`
    /// (metric or imperial, as the player chose).
    numbers: NumberText,
    percent: u8,
    distance: u8,
    texts: Texts,
}

// SAFETY: reflection records and game code living as long as the game; only used on the game
// thread.
unsafe impl Send for ZoomNotification {}
unsafe impl Sync for ZoomNotification {}

/// Finds the notification's parts. After engine init.
pub fn find(code: ZoomCode) -> Result<ZoomNotification, String> {
    let class =
        |name: &'static str| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let object = |class, name| reflection::field_offset(class, name, "ObjectProperty", 8);
    let text = size_of::<Text>() as i32;
    let text_block = class("TextBlock")?;
    let set_text = Function::find(text_block, "SetText")?;
    let brick_game_instance = class("BrickGameInstance")?;
    let window_manager = class("WindowManagerWidget")?;
    let hud = class("HUDContainerWidget")?;
    let names = kismet::names()?;
    Ok(ZoomNotification {
        code,
        name: names.name(NAME)?,
        names,
        game_instance: object(class("World")?, "OwningGameInstance")?,
        brick_game_instance: brick_game_instance.cast(),
        window_manager: object(brick_game_instance, "WindowManagerWidget")?,
        active_widget: object(window_manager, "ActiveWidget")?,
        hud: hud.cast(),
        overlay: object(hud, "GameOverlayWidget")?,
        text_block: object(class("HUDNotificationWidget")?, "TextBlock")?,
        set_text_value: set_text.parameter("InText", "TextProperty", text)?,
        set_text,
        numbers: ui_library::number_text()?,
        percent: ui_library::value_type("Percent")?,
        distance: ui_library::value_type("DistanceAuto")?,
        texts: kismet::texts()?,
    })
}

impl ZoomNotification {
    /// The HUD's overlay, if the HUD shows (see [`ZoomNotification`]'s fields).
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn overlay(&self) -> Option<*mut UObjectBase> {
        // SAFETY: on the game thread; each object comes from a reflected object property of the
        // one before (its class checked where the property belongs to a subclass).
        unsafe {
            let world = world::current();
            if world.is_null() {
                return None;
            }
            let instance = object_at(world, self.game_instance)?;
            if !reflection::is_a(instance, self.brick_game_instance) {
                return None;
            }
            let manager = object_at(instance, self.window_manager)?;
            let active = object_at(manager, self.active_widget)?;
            if !reflection::is_a(active, self.hud) {
                return None;
            }
            object_at(active, self.overlay)
        }
    }

    /// Shows the notification as the game does for the zoom ratio `ratio` (0 to 1), restarting it
    /// if it shows, then labels it `label`.
    ///
    /// # Safety
    /// `overlay` must be the live overlay. Game thread only.
    pub unsafe fn show(&self, overlay: *mut UObjectBase, ratio: f32, label: &str) {
        // SAFETY: per the contract; the functions come from the game's own call, the notification
        // is a live `UHUDNotificationWidget` and its text block a live `UTextBlock`. The text goes
        // in as is: `SetText` copies it, ProcessEvent doesn't destroy it, so it's released after.
        unsafe {
            let notification = (self.code.create)(overlay, &self.name, true);
            if notification.is_null() {
                return;
            }
            (self.code.update)(overlay, notification, ratio);
            self.label(notification, label);
        }
    }

    /// Shows a notification reading `text` in the slot called `context` (each context is its own
    /// notification; showing one that shows updates it and restarts its timer), ending by itself
    /// after a while. If the overlay won't make one for `context`, the zoom's slot shows it. Fails
    /// if the overlay hides notifications. The HUD overlay is [`ZoomNotification::overlay`].
    ///
    /// The notification's icon is not set: it keeps what the widget pool's notification showed
    /// before (the icon of the zoom, speed, camera mode or HUD visibility notification it was
    /// last, or the Blueprint's default the first time). `InitializeNotification` takes the icon as
    /// a soft texture reference and a null one would draw an empty brush; see
    /// `docs/re/editor-camera.md`.
    ///
    /// # Safety
    /// `overlay` must be the live overlay. Game thread only.
    pub unsafe fn show_text(
        &self,
        overlay: *mut UObjectBase,
        context: &str,
        text: &str,
    ) -> Result<(), String> {
        let context = self.names.name(context)?;
        // SAFETY: per the contract; the notification is a live `UHUDNotificationWidget` and the
        // names outlive the calls (the game copies the name it keeps).
        unsafe {
            let mut notification = (self.code.create)(overlay, &context, true);
            if notification.is_null() {
                notification = (self.code.create)(overlay, &self.name, true);
            }
            if notification.is_null() {
                return Err("the HUD hides notifications".into());
            }
            self.label(notification, text);
        }
        Ok(())
    }

    /// Sets the notification's text block to `text`.
    ///
    /// # Safety
    /// `notification` must be a live `UHUDNotificationWidget`. Game thread only.
    unsafe fn label(&self, notification: *mut UObjectBase, text: &str) {
        // SAFETY: per the contract; the text block is a live `UTextBlock`. The text goes in as is:
        // `SetText` copies it, ProcessEvent doesn't destroy it, so it's released after.
        unsafe {
            let Some(text_block) = object_at(notification, self.text_block) else {
                return;
            };
            let mut parameters = Parameters::new();
            reflection::put(&mut parameters, self.set_text_value, self.texts.text(text));
            self.set_text.call(text_block, &mut parameters);
            reflection::take::<Text>(&parameters, self.set_text_value).release();
        }
    }

    /// A zoom ratio (0 to 1) as the notification shows it: 0 to 100, no unit.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn ratio_text(&self, ratio: f32) -> String {
        // SAFETY: per the contract.
        unsafe { self.numbers.text(ratio, self.percent, 0, false) }
    }

    /// A distance of `cm` in the player's units (metric or imperial), with the unit and at most
    /// `decimals` decimals.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn distance_text(&self, cm: f32, decimals: i32) -> String {
        // SAFETY: per the contract.
        unsafe { self.numbers.text(cm, self.distance, decimals, true) }
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
