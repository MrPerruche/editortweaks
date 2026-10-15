//! The keys bound to the game's input actions, as the game uses them: the local player's
//! `UBrickPlayerInput` (a `UPlayerInput`, `APlayerController::PlayerInput`). See
//! `docs/re/input-bindings.md`.
//!
//! Its action bindings are a `TArray<FInputActionKeyMapping>` (the action's name, the modifier keys
//! it needs, the key), not reflected: each input table row's default keys, or those the player
//! rebound it to (`game::input_table`). Its offset is read from
//! `UBrickPlayerInput::IsInputActionKeyPressed` ([`ACTION_KEY`]), which looks a key up in them;
//! the elements' layout comes from reflection (`FInputActionKeyMapping` is a reflected struct).
//!
//! Keys reach it through `UPlayerInput::InputKey` ([`INPUT_KEY`]), and [`ViewTargets`] tells what a
//! player looks through (the editor, while editing).

use core::ffi::c_void;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;
use brickrust::ue::tarray::TArray;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::framework::sig::{self, CodeShape, FunctionSig};
use crate::game::reflection::{self, BoolField};
use crate::game::world;

/// `virtual bool UPlayerInput::InputKey(FKey Key, EInputEvent Event, float AmountDepressed, bool
/// bGamepad)` (engine; BRMK engine module 0x1758A30, game RVA 0x29C9DB0, slot +0x278 of
/// `UBrickPlayerInput`'s vtable, which keeps it): every key and mouse button the player's
/// controller gets (`APlayerController::InputKey`, from the game's viewport, not what menus take),
/// before the bindings see it. UE 4.27:
///
/// ```text
/// FKeyState& KeyState = KeyStateMap.FindOrAdd(Key);
/// switch (Event) {
/// case IE_Pressed: case IE_Repeat:  KeyState.RawValueAccumulator.X = AmountDepressed; ...
/// case IE_Released:                 KeyState.RawValueAccumulator.X = 0; ...
/// case IE_DoubleClick:              ...
/// }
/// ```
///
/// The bindings then read the key states each frame (`ProcessInputStack`), so a key renamed here
/// acts as the other key everywhere.
pub const INPUT_KEY: FunctionSig = FunctionSig {
    name: "UPlayerInput::InputKey",
    // The switch on the event (pressed, released, repeat, double click), then the double click's
    // value written to the key state (`RawValueAccumulator.X`, +0xC0).
    pattern: "83 ea 01 0f 84 ?? ?? ?? ?? 83 ea 01 0f 84 ?? ?? ?? ?? 83 fa 01 0f 85 ?? ?? ?? ?? f3 0f 11 ?? c0 00 00 00",
};

/// The key comes as a pointer to the caller's copy (an `FKey` by value), which the function
/// destroys when it's done (Microsoft's x64 convention).
pub type InputKeyFn = unsafe extern "C" fn(
    input: *mut UObjectBase,
    key: *mut Key,
    event: i32,
    amount: f32,
    gamepad: bool,
) -> bool;

/// `EInputEvent`: what happened to the key.
pub const PRESSED: i32 = 0;
pub const RELEASED: i32 = 1;
pub const DOUBLE_CLICK: i32 = 3;

/// An `FKey` (UE 4.27, 0x18 bytes): its name, and its details (`TSharedPtr<FKeyDetails>`: the
/// details, then their reference controller), which the engine looks up from the name while null.
#[repr(C)]
pub struct Key {
    pub name: FName,
    details: *mut c_void,
    controller: *mut ReferenceController,
}

/// `SharedPointerInternals::FReferenceControllerBase`: its vtable, then the counts. `FKey`'s
/// pointer isn't thread safe (UE 4.27's default on Windows): plain counts.
#[repr(C)]
struct ReferenceController {
    vtable: *const c_void,
    shared: i32,
    weak: i32,
}

impl Key {
    /// Makes this key the one called `name`: its details are let go (the engine finds the new
    /// key's from its name when it needs them).
    ///
    /// # Safety
    /// `self` must be a live `FKey` of the game's. Game thread only (the counts aren't atomic).
    pub unsafe fn rename(&mut self, name: FName) {
        self.name = name;
        // SAFETY: per the contract; the controller is null or the details' live controller. The
        // engine's list of keys keeps its own reference to every key's details, so this one is
        // never the last: if it seems to be, it's kept (the details stay alive, unused).
        unsafe {
            if let Some(controller) = self.controller.as_mut()
                && controller.shared > 1
            {
                controller.shared -= 1;
            }
        }
        self.details = core::ptr::null_mut();
        self.controller = core::ptr::null_mut();
    }
}

/// What players look through: `APlayerController::PlayerCameraManager`'s `ViewTarget.Target` (all
/// reflected). The editor makes itself its player's (`ABrickEditor::BecomeViewTarget`).
pub struct ViewTargets {
    /// `PlayerController`.
    controller: *const UStruct,
    /// Offset of `APlayerController::PlayerCameraManager`.
    camera_manager: usize,
    /// Offset of `ViewTarget.Target` in a camera manager.
    target: usize,
    /// `BrickEditor`.
    editor: *const UStruct,
}

// SAFETY: reflection records and offsets living as long as the game; only used on the game thread.
unsafe impl Send for ViewTargets {}
unsafe impl Sync for ViewTargets {}

pub fn view_targets() -> Result<ViewTargets, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let controller = class("PlayerController")?;
    let manager = class("PlayerCameraManager")?;
    // SAFETY: a live class; property records live as long as their class.
    let target = unsafe {
        let view_target = reflection::find_property(manager.cast(), "ViewTarget");
        if view_target.is_null() {
            return Err("PlayerCameraManager: field 'ViewTarget' not found".into());
        }
        let view = reflection::struct_type(view_target);
        if view.is_null() {
            return Err("PlayerCameraManager: 'ViewTarget' isn't a struct".into());
        }
        (*view_target).offset_internal as usize
            + reflection::struct_field_offset(view, "Target", "ObjectProperty", 8)?
    };
    Ok(ViewTargets {
        controller: controller.cast(),
        camera_manager: reflection::field_offset(
            controller,
            "PlayerCameraManager",
            "ObjectProperty",
            8,
        )?,
        target,
        editor: class("BrickEditor")?.cast(),
    })
}

impl ViewTargets {
    /// Whether the player whose input is `input` (a `UPlayerInput`, inside its player controller)
    /// looks through an editor: is editing.
    ///
    /// # Safety
    /// `input` must be a live player input. Game thread only.
    pub unsafe fn in_editor(&self, input: *const UObjectBase) -> bool {
        // SAFETY: per the contract; an object's outer is live or null, and each pointer read is a
        // reflected field of an object checked to have it (or null).
        unsafe {
            let controller = (*input).outer_private;
            if controller.is_null() || !is_a(controller, self.controller) {
                return false;
            }
            let read = |object: *const UObjectBase, offset: usize| {
                object
                    .cast::<u8>()
                    .add(offset)
                    .cast::<*const UObjectBase>()
                    .read()
            };
            let manager = read(controller, self.camera_manager);
            if manager.is_null() {
                return false;
            }
            let target = read(manager, self.target);
            !target.is_null() && is_a(target, self.editor)
        }
    }
}

/// Whether `object` is of `class` or a subclass.
///
/// # Safety
/// `object` must be a live object.
unsafe fn is_a(object: *const UObjectBase, class: *const UStruct) -> bool {
    // SAFETY: per the contract.
    unsafe { reflection::with_parents(reflection::class_of(object.cast())) }
        .any(|parent| parent == class)
}

/// `bool UBrickPlayerInput::IsInputActionKeyPressed(FName ActionName, const FKey& Key) const` (BRMK
/// 0x331B20, game RVA 0xE75440): whether `Key` is bound to the action, ignoring modifier keys.
///
/// ```text
/// for (const FInputActionKeyMapping& Mapping : ActionMappings)
///     if (Mapping.ActionName == ActionName && Mapping.Key.KeyName == Key.KeyName) return true;
/// return false;
/// ```
///
/// A leaf function, called by its reflected thunk (found through reflection).
pub const ACTION_KEY: CodeShape = CodeShape {
    name: "UBrickPlayerInput::IsInputActionKeyPressed",
    // The bindings' data pointer (offset at +3) and count (+10), elements 0x28 bytes apart.
    shape: "48 8b 81 ?? ?? ?? ?? 48 63 89 ?? ?? ?? ?? 4c 8d 0c 89 4e 8d 14 c8 49 3b c2",
};

/// Distance between bindings that [`ACTION_KEY`] steps by.
const BINDING_SIZE: usize = 0x28;

/// A key bound to an action, with the modifier keys it needs too (any others may be held).
#[derive(Clone, Copy, Debug)]
pub struct Chord {
    /// The key's name (`FKey::KeyName`, e.g. `LeftAlt`).
    pub key: FName,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// The Windows key.
    pub cmd: bool,
}

/// Where the local player's bindings are (after engine init).
pub struct PlayerInputs {
    /// Offset of `UWorld::OwningGameInstance`.
    game_instance: usize,
    /// Offset of `UGameInstance::LocalPlayers`.
    local_players: usize,
    /// Offset of `UPlayer::PlayerController`.
    controller: usize,
    /// Offset of `APlayerController::PlayerInput`.
    input: usize,
    /// `BrickPlayerInput`.
    brick_input: *const UStruct,
    /// Offset of the action bindings in a `UBrickPlayerInput`.
    bindings: usize,
    binding: BindingLayout,
}

// SAFETY: reflection records and offsets living as long as the game; only used on the game thread.
unsafe impl Send for PlayerInputs {}
unsafe impl Sync for PlayerInputs {}

/// Offsets in an `FInputActionKeyMapping`.
struct BindingLayout {
    /// `ActionName`.
    action: usize,
    /// `Key.KeyName`.
    key_name: usize,
    shift: BoolField,
    ctrl: BoolField,
    alt: BoolField,
    cmd: BoolField,
}

pub fn find() -> Result<PlayerInputs, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let object = |class, name| reflection::field_offset(class, name, "ObjectProperty", 8);
    let brick_input = class("BrickPlayerInput")?;
    let bindings = bindings_offset(brick_input)?;

    let binding = reflection::find_object("InputActionKeyMapping", "ScriptStruct")
        .ok_or("struct InputActionKeyMapping not found")?
        .cast_const()
        .cast::<UStruct>();
    // SAFETY: a live struct.
    if unsafe { reflection::struct_size(binding) } != BINDING_SIZE {
        return Err("InputActionKeyMapping: unexpected size".into());
    }
    let key = reflection::struct_field(binding, "Key", "StructProperty", 0x18)?;
    // SAFETY: a live StructProperty record.
    let (key_offset, key_struct) = unsafe {
        (
            (*key).offset_internal as usize,
            reflection::struct_type(key),
        )
    };
    let key_name =
        key_offset + reflection::struct_field_offset(key_struct, "KeyName", "NameProperty", 8)?;
    // ACTION_KEY reads the key's name there.
    if key_name != 0x10 {
        return Err("InputActionKeyMapping: unexpected key offset".into());
    }
    let flag = |name| reflection::bool_field(binding, name);
    Ok(PlayerInputs {
        game_instance: object(class("World")?, "OwningGameInstance")?,
        local_players: reflection::field_offset(
            class("GameInstance")?,
            "LocalPlayers",
            "ArrayProperty",
            0x10,
        )?,
        controller: object(class("Player")?, "PlayerController")?,
        input: object(class("PlayerController")?, "PlayerInput")?,
        brick_input: brick_input.cast(),
        bindings,
        binding: BindingLayout {
            action: reflection::struct_field_offset(binding, "ActionName", "NameProperty", 8)?,
            key_name,
            shift: flag("bShift")?,
            ctrl: flag("bCtrl")?,
            alt: flag("bAlt")?,
            cmd: flag("bCmd")?,
        },
    })
}

/// The offset of the action bindings in a `UBrickPlayerInput`, read from [`ACTION_KEY`]: the
/// function its reflected thunk calls.
fn bindings_offset(brick_input: *const UClass) -> Result<usize, String> {
    let function = reflection::find_function(brick_input, "IsInputActionKeyPressed")?;
    let thunk = reflection::native_thunk(function)
        .map_err(|reason| format!("IsInputActionKeyPressed: {reason}"))?;
    let found = sig::direct_calls(thunk)
        .into_iter()
        .find(|&target| ACTION_KEY.matches(target))
        .ok_or(format!("{} not found", ACTION_KEY.name))?;
    // SAFETY: both offsets lie within the matched shape.
    let (data, count) = unsafe {
        (
            found.add(3).cast::<u32>().read_unaligned() as usize,
            found.add(10).cast::<u32>().read_unaligned() as usize,
        )
    };
    if count != data + 8 {
        return Err(format!("{}: unexpected offsets", ACTION_KEY.name));
    }
    Ok(data)
}

impl PlayerInputs {
    /// The local player's input (a `UBrickPlayerInput`), if there's one now (none before the first
    /// map, for instance).
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn local(&self) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract; each pointer is null or a live object of the class whose
        // reflected field is read (the world's game instance, its first local player, ...).
        unsafe {
            let field = |object: *mut UObjectBase, offset: usize| {
                (!object.is_null())
                    .then(|| {
                        object
                            .cast::<u8>()
                            .add(offset)
                            .cast::<*mut UObjectBase>()
                            .read()
                    })
                    .filter(|found| !found.is_null())
            };
            let game_instance = field(world::current(), self.game_instance)?;
            let players = &*game_instance
                .cast::<u8>()
                .add(self.local_players)
                .cast::<TArray<*mut UObjectBase>>();
            if players.num <= 0 || players.data.is_null() {
                return None;
            }
            let controller = field(players.data.read(), self.controller)?;
            let input = field(controller, self.input)?;
            reflection::with_parents(reflection::class_of(input.cast_const().cast()))
                .any(|class| class == self.brick_input)
                .then_some(input)
        }
    }

    /// Calls `each` with every key bound to `action` in `input`.
    ///
    /// # Safety
    /// `input` must be a live `UBrickPlayerInput` (see [`Self::local`]). Game thread only.
    pub unsafe fn chords(
        &self,
        input: *mut UObjectBase,
        action: FName,
        mut each: impl FnMut(Chord),
    ) {
        // SAFETY: per the contract; the bindings are a TArray of `FInputActionKeyMapping`s at the
        // offset `UBrickPlayerInput` reads them from, laid out as reflected.
        unsafe {
            let bindings = &*input.cast::<u8>().add(self.bindings).cast::<TArray<u8>>();
            let Ok(count) = usize::try_from(bindings.num) else {
                return;
            };
            if bindings.data.is_null() {
                return;
            }
            let layout = &self.binding;
            for index in 0..count {
                let binding = bindings.data.add(index * BINDING_SIZE).cast_const();
                let name = binding.add(layout.action).cast::<FName>().read_unaligned();
                if name.comparison_index != action.comparison_index || name.number != action.number
                {
                    continue;
                }
                each(Chord {
                    key: binding
                        .add(layout.key_name)
                        .cast::<FName>()
                        .read_unaligned(),
                    shift: layout.shift.get(binding),
                    ctrl: layout.ctrl.get(binding),
                    alt: layout.alt.get(binding),
                    cmd: layout.cmd.get(binding),
                });
            }
        }
    }
}
