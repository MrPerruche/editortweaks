//! The match's players by SteamID (`docs/re/coop/findings.md`, "players' SteamIDs"): reflected
//! `UWorld::GameState` -> `AGameStateBase::PlayerArray` -> `APlayerState::UniqueId`, turned into
//! text by the game's own `UBrickStatics::UniqueNetIdToString` (the decimal SteamID, or
//! "INVALID"); their names from `APlayerState::PlayerNamePrivate` (reflected, read only).

use brickrust::ue::coreuobject::UObjectBase;

use crate::game::popup::{NET_ID_SIZE, parameter_by_kind};
use crate::game::reflection::{self, Function, Parameters};
use crate::game::slider_text::FString;
use crate::game::world;

const STATICS: &str = "BrickStatics";

pub struct MatchPlayers {
    /// `UWorld::GameState`, `AGameStateBase::PlayerArray`, `APlayerState::UniqueId`.
    game_state: usize,
    player_array: usize,
    unique_id: usize,
    /// `APlayerState::PlayerNamePrivate` (`None` if not found: names are then empty).
    player_name: Option<usize>,
    /// `UBrickStatics`' default object, `UniqueNetIdToString` and its parameter and result.
    statics: *mut UObjectBase,
    to_string: Function,
    input: usize,
    output: usize,
}

// SAFETY: reflection records and a default object living as long as the game; only used on the
// game thread.
unsafe impl Send for MatchPlayers {}
unsafe impl Sync for MatchPlayers {}

/// An `TArray`'s header.
#[repr(C)]
struct RawArray<T> {
    data: *const T,
    num: i32,
    max: i32,
}

impl MatchPlayers {
    /// Finds everything. After engine init (reflection).
    pub fn find() -> Result<Self, String> {
        let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
        let statics_class = class(STATICS)?;
        let to_string = Function::find(statics_class, "UniqueNetIdToString")?;
        Ok(Self {
            game_state: reflection::field_offset(
                class("World")?,
                "GameState",
                "ObjectProperty",
                8,
            )?,
            player_array: reflection::field_offset(
                class("GameStateBase")?,
                "PlayerArray",
                "ArrayProperty",
                16,
            )?,
            unique_id: reflection::field_offset(
                class("PlayerState")?,
                "UniqueId",
                "StructProperty",
                NET_ID_SIZE as i32,
            )?,
            player_name: reflection::field_offset(
                class("PlayerState")?,
                "PlayerNamePrivate",
                "StrProperty",
                size_of::<RawArray<u16>>() as i32,
            )
            .ok(),
            statics: reflection::find_object("Default__BrickStatics", STATICS)
                .ok_or("BrickStatics' default object not found")?,
            input: parameter_by_kind(
                "UniqueNetIdToString",
                reflection::find_function(statics_class, "UniqueNetIdToString")?,
                "StructProperty",
                NET_ID_SIZE as i32,
                false,
            )?,
            output: to_string.parameter(
                "ReturnValue",
                "StrProperty",
                size_of::<FString>() as i32,
            )?,
            to_string,
        })
    }

    /// The match's game state, if there's a match.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn game_state(&self) -> Option<*mut UObjectBase> {
        // SAFETY: on the game thread; a reflected object property of the world.
        unsafe {
            let world = world::current();
            if world.is_null() {
                return None;
            }
            let state = world
                .cast::<u8>()
                .add(self.game_state)
                .cast::<*mut UObjectBase>()
                .read();
            (!state.is_null()).then_some(state)
        }
    }

    /// The match's players with a SteamID: it and their name (empty if unknown).
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn players(&self) -> Vec<(u64, String)> {
        // SAFETY: per the contract.
        unsafe { self.states() }
            .into_iter()
            .filter_map(|player| {
                // SAFETY: a live player state of the match; game thread.
                let id = unsafe { self.steam_id(player) }?;
                // SAFETY: as above.
                Some((id, unsafe { self.name(player) }))
            })
            .collect()
    }

    /// Where the `FUniqueNetIdRepl` of the match's player with SteamID `steam_id` is, if they're
    /// in the match. Only valid this frame.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn net_id(&self, steam_id: u64) -> Option<*const u8> {
        // SAFETY: per the contract; the player states are the match's, live this frame.
        unsafe {
            self.states()
                .into_iter()
                .find(|&player| self.steam_id(player) == Some(steam_id))
                .map(|player| player.cast::<u8>().add(self.unique_id).cast_const())
        }
    }

    /// The match's player states (none without a match).
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn states(&self) -> Vec<*mut UObjectBase> {
        // SAFETY: on the game thread; the array is a reflected property of the game state.
        unsafe {
            let Some(state) = self.game_state() else {
                return Vec::new();
            };
            let array = state
                .cast::<u8>()
                .add(self.player_array)
                .cast::<RawArray<*mut UObjectBase>>();
            let len = usize::try_from((*array).num).unwrap_or(0);
            if (*array).data.is_null() || len == 0 {
                return Vec::new();
            }
            core::slice::from_raw_parts((*array).data, len)
                .iter()
                .copied()
                .filter(|player| !player.is_null())
                .collect()
        }
    }

    /// `player`'s SteamID, if it has one.
    ///
    /// # Safety
    /// `player` must be a live `APlayerState`. Game thread only.
    unsafe fn steam_id(&self, player: *mut UObjectBase) -> Option<u64> {
        // SAFETY: per the contract. The id goes in as a bitwise copy only read by the function,
        // and its result is a new string of the game's, ours to free.
        unsafe {
            let id = player.cast::<u8>().add(self.unique_id).cast_const();
            let mut parameters = Parameters::new();
            let base = parameters.as_mut_ptr();
            core::ptr::copy_nonoverlapping(id, base.add(self.input), NET_ID_SIZE);
            self.to_string.call(self.statics, &mut parameters);
            let text = reflection::take::<FString>(&parameters, self.output);
            let steam_id = text.to_string().parse().ok();
            text.free();
            steam_id
        }
    }

    /// `player`'s name, as the game shows it (empty if unknown).
    ///
    /// # Safety
    /// `player` must be a live `APlayerState`. Game thread only.
    unsafe fn name(&self, player: *mut UObjectBase) -> String {
        let Some(offset) = self.player_name else {
            return String::new();
        };
        // SAFETY: per the contract; a reflected `FString` of it (UTF-16 with its terminator,
        // counted), only read.
        unsafe {
            let text = player.cast::<u8>().add(offset).cast::<RawArray<u16>>();
            let len = usize::try_from((*text).num).unwrap_or(0);
            if (*text).data.is_null() || len < 2 {
                return String::new();
            }
            String::from_utf16_lossy(core::slice::from_raw_parts((*text).data, len - 1))
        }
    }
}
