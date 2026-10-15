//! The HUD's zoom notification (`UGameOverlayWidget`, the Blueprint `WBP_GameOverlay`): the zoom
//! icon and ratio (0 to 100) shown while the view zooms. Found through BRMK 1.11.6's exports and
//! Blueprints, then in game 1.11.7 (build 25831956); see `docs/re/editor-camera.md`.
//!
//! The overlay follows the view target's zoom (its `IViewTargetInterface`'s zoom delegate, which
//! `UpdateZoom` broadcasts each frame the ratio changes) with `OnCameraZoomRatioChanged`. That gets
//! the notification called "zoom" and calls the Blueprint event `UpdateCameraZoomNotification`,
//! which puts `NumberToText(Ratio, Percent)` and the zoom icon in it; the notification's native
//! `InitializeNotification` sets its `TextBlock`'s text and its icon.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;
use brickrust::ue::uclass::UStruct;

use crate::framework::sig::FunctionSig;
use crate::game::kismet::{self, Texts};
use crate::game::reflection::{self, Function, Parameters};
use crate::game::text::Text;
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
pub fn code() -> Result<ZoomCode, String> {
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
    numbers: NumberToText,
    texts: Texts,
}

// SAFETY: reflection records and game code living as long as the game; only used on the game
// thread.
unsafe impl Send for ZoomNotification {}
unsafe impl Sync for ZoomNotification {}

/// `UUIFunctionLibrary::NumberToText` (reflected; see `game::ui_library`), as the notification
/// uses it, and the value types it's used with here.
struct NumberToText {
    library: *mut UObjectBase,
    function: Function,
    value: usize,
    value_type: usize,
    decimals: usize,
    units: usize,
    result: usize,
    /// `ENumericValueType::Percent` and `ENumericValueType::DistanceAuto` (metric or imperial,
    /// as the player chose).
    percent: u8,
    distance: u8,
}

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
    Ok(ZoomNotification {
        code,
        name: kismet::string_to_name(NAME)?,
        game_instance: object(class("World")?, "OwningGameInstance")?,
        brick_game_instance: brick_game_instance.cast(),
        window_manager: object(brick_game_instance, "WindowManagerWidget")?,
        active_widget: object(window_manager, "ActiveWidget")?,
        hud: hud.cast(),
        overlay: object(hud, "GameOverlayWidget")?,
        text_block: object(class("HUDNotificationWidget")?, "TextBlock")?,
        set_text_value: set_text.parameter("InText", "TextProperty", text)?,
        set_text,
        numbers: number_to_text()?,
        texts: kismet::texts()?,
    })
}

fn number_to_text() -> Result<NumberToText, String> {
    let class =
        reflection::find_class("UIFunctionLibrary").ok_or("class UIFunctionLibrary not found")?;
    // SAFETY: a live class.
    let library = unsafe { reflection::default_object(class) }
        .ok_or("UIFunctionLibrary has no default object")?;
    let function = Function::find(class, "NumberToText")?;
    let value_type = |entry: &str| {
        let value =
            reflection::enum_value("ENumericValueType", &format!("ENumericValueType::{entry}"))?;
        u8::try_from(value).map_err(|_| format!("ENumericValueType::{entry} is {value}"))
    };
    Ok(NumberToText {
        library,
        value: function.parameter("Value", "FloatProperty", 4)?,
        value_type: function.parameter("ValueType", "EnumProperty", 1)?,
        decimals: function.parameter("MaxFractionalDigits", "IntProperty", 4)?,
        units: function.parameter("bIncludeUnits", "BoolProperty", 1)?,
        result: function.parameter("ReturnValue", "TextProperty", size_of::<Text>() as i32)?,
        function,
        percent: value_type("Percent")?,
        distance: value_type("DistanceAuto")?,
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
            let Some(text_block) = object_at(notification, self.text_block) else {
                return;
            };
            let mut parameters = Parameters::new();
            reflection::put(&mut parameters, self.set_text_value, self.texts.text(label));
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
        unsafe { self.number(ratio, self.numbers.percent, 0, false) }
    }

    /// A distance of `cm` in the player's units (metric or imperial), with the unit and at most
    /// `decimals` decimals.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn distance_text(&self, cm: f32, decimals: i32) -> String {
        // SAFETY: per the contract.
        unsafe { self.number(cm, self.numbers.distance, decimals, true) }
    }

    /// `NumberToText(value, value_type, decimals, units)`.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn number(&self, value: f32, value_type: u8, decimals: i32, units: bool) -> String {
        let numbers = &self.numbers;
        let mut parameters = Parameters::new();
        // SAFETY: on the game thread; each value goes to its parameter's reflected offset, the
        // others stay zero (false). The result is a new text, ours to release.
        unsafe {
            reflection::put(&mut parameters, numbers.value, value);
            reflection::put(&mut parameters, numbers.value_type, value_type);
            reflection::put(&mut parameters, numbers.decimals, decimals);
            reflection::put(&mut parameters, numbers.units, units);
            numbers.function.call(numbers.library, &mut parameters);
            self.texts
                .string(reflection::take::<Text>(&parameters, numbers.result))
        }
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
