//! The keys bound to the game's input actions, as the game uses them: the local player's
//! `UBrickPlayerInput` (a `UPlayerInput`, `APlayerController::PlayerInput`). See
//! `docs/re/input-bindings.md`.
//!
//! Its action bindings are a `TArray<FInputActionKeyMapping>` (the action's name, the modifier keys
//! it needs, the key), not reflected: each input table row's default keys, or those the player
//! rebound it to (`game::input_table`). Its offset is read from
//! `UBrickPlayerInput::IsInputActionKeyPressed` ([`ACTION_KEY`]), which looks a key up in them;
//! the elements' layout comes from reflection (`FInputActionKeyMapping` is a reflected struct).

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;
use brickrust::ue::tarray::TArray;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::framework::sig::{self, CodeShape};
use crate::game::reflection::{self, BoolField};
use crate::game::world;

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
