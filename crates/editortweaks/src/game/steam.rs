//! Steam's peer-to-peer messages (`ISteamNetworkingMessages`), through the game's own
//! `steam_api64.dll` (SDK 1.63, `Engine\Binaries\ThirdParty\Steamworks\Steamv163\Win64`).
//! Coop's transport, beside the game's connection, so the server never sees it.
//!
//! Found statically (`docs/re/coop/steam-api.md`): every function is a flat export looked up by
//! name, so nothing here depends on the game exe. No callbacks: the engine already pumps Steam's
//! standard dispatch, and polling (`AcceptSessionWithUser` for expected peers, then
//! `ReceiveMessagesOnChannel`) is all coop needs. The `steamworks` crate can't be used: it insists
//! on manual dispatch, which the DLL refuses, and shuts Steam down when dropped.

use core::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;

use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

use crate::framework::timings;

const DLL: &str = "steam_api64.dll";

/// `SteamNetworkingIdentity` (136 bytes): a type, a size, and a union holding the SteamID64 first.
#[repr(C)]
struct Identity {
    kind: i32,
    size: i32,
    data: [u8; 128],
}

const _: () = assert!(size_of::<Identity>() == 136);

impl Identity {
    /// `k_ESteamNetworkingIdentityType_SteamID`.
    const STEAM_ID: i32 = 16;

    fn steam_id(id: u64) -> Self {
        let mut data = [0; 128];
        data[..8].copy_from_slice(&id.to_le_bytes());
        Self {
            kind: Self::STEAM_ID,
            size: 8,
            data,
        }
    }
}

/// The start of `SteamNetworkingMessage_t` (216 bytes): the data, its size, then the sender's
/// identity at 16 (so its SteamID64 at 24). Nothing past it is read.
#[repr(C)]
struct MessageHead {
    data: *const u8,
    size: i32,
    _connection: u32,
    peer_kind: i32,
    _peer_size: i32,
    peer_steam_id: u64,
}

const _: () = assert!(core::mem::offset_of!(MessageHead, peer_steam_id) == 24);

/// `k_nSteamNetworkingSend_*`.
const SEND_RELIABLE: i32 = 8;
const SEND_UNRELIABLE: i32 = 0;
/// Reconnects a session that broke instead of failing every send.
const SEND_AUTO_RESTART: i32 = 32;
/// `k_EResultOK`.
const RESULT_OK: i32 = 1;

/// Config numbers from Valve's `steamnetworkingtypes.h`. `k_ESteamNetworkingConfig_P2P_Transport_
/// ICE_Enable`: which ICE candidate types (our addresses) a P2P connection may share with its peer.
const CONFIG_P2P_ICE_ENABLE: i32 = 104;
/// `k_ESteamNetworkingConfig_Global`.
const SCOPE_GLOBAL: i32 = 1;
/// `k_ESteamNetworkingConfig_Int32`.
const TYPE_INT32: i32 = 1;
/// `k_nSteamNetworkingConfig_P2P_Transport_ICE_Enable_Disable`: no ICE, no addresses shared.
const ICE_DISABLED: i32 = 0;
/// `k_ESteamNetworkingGetConfigValue_OK` and `_OKInherited`: the value was read.
const GET_CONFIG_OK: i32 = 1;
const GET_CONFIG_OK_INHERITED: i32 = 2;

/// `k_nSteamNetworkConnectionInfoFlags_Relayed`: "relayed somehow (SDR or TURN)".
const CONNECTION_FLAG_RELAYED: i32 = 16;

/// `SteamNetConnectionInfo_t` (696 bytes). Only the relay, state and flags are read; the remote
/// address stays as unread bytes, so no IP address can end up in a log.
#[repr(C)]
struct ConnectionInfo {
    _identity_remote: Identity,
    _user_data: i64,
    _listen_socket: u32,
    _address_remote: [u8; 18],
    _pad: u16,
    _pop_remote: u32,
    pop_relay: u32,
    state: i32,
    _end_reason: i32,
    _end_debug: [u8; 128],
    _description: [u8; 128],
    flags: i32,
    _reserved: [u32; 63],
}

const _: () = assert!(size_of::<ConnectionInfo>() == 696);
const _: () = assert!(core::mem::offset_of!(ConnectionInfo, pop_relay) == 172);
const _: () = assert!(core::mem::offset_of!(ConnectionInfo, state) == 176);
const _: () = assert!(core::mem::offset_of!(ConnectionInfo, flags) == 440);

/// `SteamNetConnectionRealTimeStatus_t` (120 bytes, Valve's `steamnetworkingtypes.h`): the live
/// numbers of a connection. Only the ping, the rates and the pending bytes are read.
#[repr(C)]
struct RealTimeStatus {
    _state: i32,
    ping: i32,
    _connection_quality_local: f32,
    _connection_quality_remote: f32,
    _out_packets_per_sec: f32,
    out_bytes_per_sec: f32,
    _in_packets_per_sec: f32,
    _in_bytes_per_sec: f32,
    send_rate_bytes_per_second: i32,
    pending_unreliable: i32,
    pending_reliable: i32,
    sent_unacked_reliable: i32,
    _queue_time_usec: i64,
    _reserved: [u32; 16],
}

const _: () = assert!(size_of::<RealTimeStatus>() == 120);
const _: () = assert!(core::mem::offset_of!(RealTimeStatus, pending_reliable) == 40);
const _: () = assert!(core::mem::offset_of!(RealTimeStatus, _queue_time_usec) == 48);

/// What Steam says about the connection it keeps with one peer.
#[derive(Clone, Debug)]
pub struct PeerConnection {
    /// `ESteamNetworkingConnectionState`: 1 connecting, 2 finding a route, 3 connected, 4 closed
    /// by the peer, 5 problem detected locally.
    pub state: i32,
    /// Goes through Valve's relay (SDR), not directly to the peer's address.
    pub relayed: bool,
    /// `k_nSteamNetworkConnectionInfoFlags_*` as Steam gave them (4 = loopback to ourselves, 8 =
    /// fast LAN, 16 = relayed).
    pub flags: i32,
    /// The relay's location code (like `fra`), `None` if not applicable.
    pub relay_pop: Option<String>,
}

/// How much is waiting to go to one peer, and how fast it goes (see `Steam::peer_queue`).
#[derive(Clone, Copy, Debug)]
pub struct PeerQueue {
    /// Bytes Steam holds for the peer: not yet sent, or sent but not yet acknowledged.
    pub pending: usize,
    /// The send rate Steam allows, in bytes per second.
    pub send_rate: i32,
    /// The bytes per second actually sent lately.
    pub out_bytes_per_sec: f32,
    /// Round-trip time in milliseconds.
    pub ping: i32,
}

impl PeerQueue {
    fn from_status(status: &RealTimeStatus) -> Self {
        // Steam reports byte counts that can't be negative; a negative one counts as none.
        let pending = [
            status.pending_unreliable,
            status.pending_reliable,
            status.sent_unacked_reliable,
        ]
        .into_iter()
        .map(|bytes| usize::try_from(bytes).unwrap_or(0))
        .sum();
        Self {
            pending,
            send_rate: status.send_rate_bytes_per_second,
            out_bytes_per_sec: status.out_bytes_per_sec,
            ping: status.ping,
        }
    }
}

impl PeerConnection {
    fn from_info(info: &ConnectionInfo) -> Self {
        // A `SteamNetworkingPOPID` packs up to four ASCII letters, the first in the high byte.
        let letters: String = info
            .pop_relay
            .to_be_bytes()
            .into_iter()
            .filter(|&byte| byte != 0)
            .map(char::from)
            .collect();
        Self {
            state: info.state,
            relayed: info.flags & CONNECTION_FLAG_RELAYED != 0,
            flags: info.flags,
            relay_pop: (!letters.is_empty()).then_some(letters),
        }
    }
}

/// `FriendGameInfo_t` (24 bytes, Valve's `isteamfriends.h`): what game a friend is in. Every field
/// is naturally aligned, so the layout is the same under the SDK's `pack(4)` and `pack(8)`. Only
/// the game id is read.
#[repr(C)]
struct FriendGameInfo {
    game_id: u64,
    _game_ip: u32,
    _game_port: u16,
    _query_port: u16,
    _lobby: u64,
}

const _: () = assert!(size_of::<FriendGameInfo>() == 24);

/// `k_EFriendFlagImmediate`: the "regular" friends of the friends list.
const FRIEND_FLAG_IMMEDIATE: i32 = 4;

/// A `CGameID` holds the app id in its low 24 bits (the rest is type and mod).
const GAME_ID_APP_MASK: u64 = 0x00FF_FFFF;

/// A Steam friend.
#[derive(Clone, Debug)]
pub struct Friend {
    pub id: u64,
    pub name: String,
}

type Interface = *mut c_void;

/// The functions, resolved once.
struct Api {
    messages: Interface,
    user: Interface,
    friends: Interface,
    utils: Interface,
    steam_utils: Interface,
    get_steam_id: unsafe extern "C" fn(user: Interface) -> u64,
    get_persona_name: unsafe extern "C" fn(friends: Interface) -> *const c_char,
    get_app_id: unsafe extern "C" fn(steam_utils: Interface) -> u32,
    friend_count: unsafe extern "C" fn(friends: Interface, flags: i32) -> i32,
    friend_by_index: unsafe extern "C" fn(friends: Interface, index: i32, flags: i32) -> u64,
    friend_game_played:
        unsafe extern "C" fn(friends: Interface, friend: u64, info: *mut FriendGameInfo) -> bool,
    friend_persona_name: unsafe extern "C" fn(friends: Interface, friend: u64) -> *const c_char,
    send: unsafe extern "C" fn(
        messages: Interface,
        to: *const Identity,
        data: *const u8,
        len: u32,
        flags: i32,
        channel: i32,
    ) -> i32,
    receive: unsafe extern "C" fn(
        messages: Interface,
        channel: i32,
        out: *mut *mut MessageHead,
        max: i32,
    ) -> i32,
    accept: unsafe extern "C" fn(messages: Interface, who: *const Identity) -> bool,
    release: unsafe extern "C" fn(message: *mut MessageHead),
    set_global_int32: unsafe extern "C" fn(utils: Interface, value: i32, new_value: i32) -> bool,
    get_config_value: unsafe extern "C" fn(
        utils: Interface,
        value: i32,
        scope: i32,
        scope_object: isize,
        data_type: *mut i32,
        result: *mut c_void,
        result_size: *mut usize,
    ) -> i32,
    connection_info: unsafe extern "C" fn(
        messages: Interface,
        peer: *const Identity,
        info: *mut ConnectionInfo,
        quick_status: *mut RealTimeStatus,
    ) -> i32,
}

// SAFETY: the interface pointers live as long as Steam does (the whole game), and are only used
// on the game thread.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

static API: OnceLock<Result<Api, String>> = OnceLock::new();

/// Looks up `name` in `module` as a function of type `F`.
///
/// # Safety
///
/// `F` must be the export's real signature, as a function pointer type.
unsafe fn export<F>(module: *mut c_void, name: &CStr) -> Result<F, String> {
    // SAFETY: `module` is a loaded module's handle and `name` is NUL-terminated.
    let address = unsafe { GetProcAddress(module, name.as_ptr().cast()) }
        .ok_or_else(|| format!("{DLL} has no {}", name.to_string_lossy()))?;
    // SAFETY: the caller guarantees `F` is the export's signature; both are function pointers.
    Ok(unsafe { core::mem::transmute_copy::<_, F>(&address) })
}

/// Calls an interface accessor (`SteamAPI_SteamUser_v023` etc.).
///
/// # Safety
///
/// Steam must be initialised (the accessors fail before `SteamAPI_Init`).
unsafe fn interface(module: *mut c_void, name: &CStr) -> Result<Interface, String> {
    // SAFETY: the accessors take nothing and return the interface pointer.
    let accessor: unsafe extern "C" fn() -> Interface = unsafe { export(module, name)? };
    // SAFETY: the caller guarantees Steam is up.
    let pointer = unsafe { accessor() };
    if pointer.is_null() {
        return Err(format!("{} returned null", name.to_string_lossy()));
    }
    Ok(pointer)
}

fn resolve() -> Result<Api, String> {
    // The networking interfaces' first use starts Steam's networking.
    let _span = timings::span("steam::resolve");
    let wide: Vec<u16> = DLL.encode_utf16().chain([0]).collect();
    // SAFETY: a NUL-terminated name.
    let module = unsafe { GetModuleHandleW(wide.as_ptr()) };
    if module.is_null() {
        return Err(format!("{DLL} isn't loaded (game started without Steam?)"));
    }
    // SAFETY: each type below is the function's signature from Valve's `steam_api_flat.h` (SDK
    // 1.63, `steam-api.md` section 5). Steam is initialised: callers only get here from a map
    // load or later, after the online subsystem started.
    unsafe {
        Ok(Api {
            messages: interface(module, c"SteamAPI_SteamNetworkingMessages_SteamAPI_v002")?,
            user: interface(module, c"SteamAPI_SteamUser_v023")?,
            friends: interface(module, c"SteamAPI_SteamFriends_v018")?,
            utils: interface(module, c"SteamAPI_SteamNetworkingUtils_SteamAPI_v004")?,
            steam_utils: interface(module, c"SteamAPI_SteamUtils_v010")?,
            get_steam_id: export(module, c"SteamAPI_ISteamUser_GetSteamID")?,
            get_persona_name: export(module, c"SteamAPI_ISteamFriends_GetPersonaName")?,
            get_app_id: export(module, c"SteamAPI_ISteamUtils_GetAppID")?,
            friend_count: export(module, c"SteamAPI_ISteamFriends_GetFriendCount")?,
            friend_by_index: export(module, c"SteamAPI_ISteamFriends_GetFriendByIndex")?,
            friend_game_played: export(module, c"SteamAPI_ISteamFriends_GetFriendGamePlayed")?,
            friend_persona_name: export(module, c"SteamAPI_ISteamFriends_GetFriendPersonaName")?,
            send: export(
                module,
                c"SteamAPI_ISteamNetworkingMessages_SendMessageToUser",
            )?,
            receive: export(
                module,
                c"SteamAPI_ISteamNetworkingMessages_ReceiveMessagesOnChannel",
            )?,
            accept: export(
                module,
                c"SteamAPI_ISteamNetworkingMessages_AcceptSessionWithUser",
            )?,
            release: export(module, c"SteamAPI_SteamNetworkingMessage_t_Release")?,
            set_global_int32: export(
                module,
                c"SteamAPI_ISteamNetworkingUtils_SetGlobalConfigValueInt32",
            )?,
            get_config_value: export(module, c"SteamAPI_ISteamNetworkingUtils_GetConfigValue")?,
            connection_info: export(
                module,
                c"SteamAPI_ISteamNetworkingMessages_GetSessionConnectionInfo",
            )?,
        })
    }
}

/// Steam's messages, once the game's Steam is up. Only call on the game thread, from a map load
/// on.
#[derive(Clone, Copy)]
pub struct Steam(&'static Api);

impl Steam {
    pub fn get() -> Result<Self, String> {
        match API.get_or_init(resolve) {
            Ok(api) => Ok(Self(api)),
            Err(reason) => Err(reason.clone()),
        }
    }

    pub fn own_id(self) -> u64 {
        // SAFETY: the resolved function with the interface it belongs to.
        unsafe { (self.0.get_steam_id)(self.0.user) }
    }

    pub fn own_name(self) -> String {
        // SAFETY: as above; the name is a NUL-terminated string Steam keeps.
        unsafe {
            let name = (self.0.get_persona_name)(self.0.friends);
            if name.is_null() {
                return String::new();
            }
            CStr::from_ptr(name).to_string_lossy().into_owned()
        }
    }

    /// The app id Steam runs this game as.
    pub fn app_id(self) -> u32 {
        // SAFETY: the resolved function with the `ISteamUtils` interface it belongs to.
        unsafe { (self.0.get_app_id)(self.0.steam_utils) }
    }

    /// The friends (regular friends list entries) who are playing the game with `app_id` right now.
    pub fn friends_playing(self, app_id: u32) -> Vec<Friend> {
        // SAFETY: the resolved function with the friends interface it belongs to.
        let count = unsafe { (self.0.friend_count)(self.0.friends, FRIEND_FLAG_IMMEDIATE) };
        let mut playing = Vec::new();
        for index in 0..count.max(0) {
            // SAFETY: as above; `index` is below the friend count.
            let id =
                unsafe { (self.0.friend_by_index)(self.0.friends, index, FRIEND_FLAG_IMMEDIATE) };
            let mut info = FriendGameInfo {
                game_id: 0,
                _game_ip: 0,
                _game_port: 0,
                _query_port: 0,
                _lobby: 0,
            };
            // SAFETY: `info` is the 24 bytes Steam fills in when it returns true.
            let in_game = unsafe { (self.0.friend_game_played)(self.0.friends, id, &mut info) };
            if !in_game || info.game_id & GAME_ID_APP_MASK != u64::from(app_id) {
                continue;
            }
            // SAFETY: as above; the name is a NUL-terminated string Steam keeps (or null).
            let name = unsafe {
                let name = (self.0.friend_persona_name)(self.0.friends, id);
                if name.is_null() {
                    String::new()
                } else {
                    CStr::from_ptr(name).to_string_lossy().into_owned()
                }
            };
            playing.push(Friend { id, name });
        }
        playing
    }

    /// Makes every peer-to-peer connection created from now on go only through Valve's relay
    /// (SDR), so a peer never learns our IP address. Returns whether Steam accepted the setting
    /// (it is read back); coop must not talk to anyone when it didn't.
    ///
    /// Direct connections work by each side telling the other its addresses (ICE candidates);
    /// `P2P_Transport_ICE_Enable = 0` makes Steam skip ICE altogether, which leaves the relay as
    /// the only transport. Steam reads the value once, when a connection is created, and then
    /// locks it for that connection, so this must run before the first `send` or `accept` of a
    /// session. It is a global setting: connections the game makes later (joining a game over
    /// SteamSockets) become relay-only too, until the game exits.
    /// See `docs/re/coop/steam-api.md`, "Relay only".
    pub fn force_relay(self) -> bool {
        let _span = timings::span("steam::force_relay");
        // SAFETY: the resolved function with the utils interface it belongs to; the value is
        // copied during the call.
        let accepted =
            unsafe { (self.0.set_global_int32)(self.0.utils, CONFIG_P2P_ICE_ENABLE, ICE_DISABLED) };
        if !accepted {
            return false;
        }
        let mut data_type = 0_i32;
        let mut value = -1_i32;
        let mut value_size = size_of::<i32>();
        // SAFETY: `value` has room for the `value_size` bytes we say it has, and the other
        // pointers are valid locals for the duration of the call.
        let result = unsafe {
            (self.0.get_config_value)(
                self.0.utils,
                CONFIG_P2P_ICE_ENABLE,
                SCOPE_GLOBAL,
                0,
                &mut data_type,
                (&mut value as *mut i32).cast(),
                &mut value_size,
            )
        };
        let read = result == GET_CONFIG_OK || result == GET_CONFIG_OK_INHERITED;
        read && data_type == TYPE_INT32 && value == ICE_DISABLED
    }

    /// How Steam currently reaches `peer` (`None` if it has no session with them), to check that
    /// the traffic is relayed. Never exposes the peer's address.
    pub fn peer_connection(self, peer: u64) -> Option<PeerConnection> {
        let peer = Identity::steam_id(peer);
        // SAFETY: every field of `ConnectionInfo` is an integer or an array of them, so all-zero
        // bytes are a valid value.
        let mut info: ConnectionInfo = unsafe { core::mem::zeroed() };
        // SAFETY: a valid identity, and `info` is the 696 bytes Steam fills in; the optional
        // real-time status is not wanted (null).
        let state = unsafe {
            (self.0.connection_info)(self.0.messages, &peer, &mut info, core::ptr::null_mut())
        };
        // `k_ESteamNetworkingConnectionState_None`: no such session.
        (state != 0).then(|| PeerConnection::from_info(&info))
    }

    /// How much is queued toward `peer` and how fast it goes (`None` if it has no session with
    /// them). Never exposes the peer's address.
    pub fn peer_queue(self, peer: u64) -> Option<PeerQueue> {
        let peer = Identity::steam_id(peer);
        // SAFETY: every field of `ConnectionInfo` is an integer or an array of them, so all-zero
        // bytes are a valid value.
        let mut info: ConnectionInfo = unsafe { core::mem::zeroed() };
        // SAFETY: every field of `RealTimeStatus` is a number or an array of them, so all-zero
        // bytes are a valid value.
        let mut status: RealTimeStatus = unsafe { core::mem::zeroed() };
        // SAFETY: a valid identity, and `info` and `status` are the 696 and 120 bytes Steam fills
        // in. `info` is passed as `peer_connection` passes it, zeroed.
        let state =
            unsafe { (self.0.connection_info)(self.0.messages, &peer, &mut info, &mut status) };
        // `k_ESteamNetworkingConnectionState_None`: no such session.
        (state != 0).then(|| PeerQueue::from_status(&status))
    }

    /// Sends `bytes` (at most 512 KiB) to `to` on `channel`. Steam copies the data. `false` if
    /// Steam refused it.
    pub fn send(self, to: u64, channel: i32, bytes: &[u8], reliable: bool) -> bool {
        let Ok(len) = u32::try_from(bytes.len()) else {
            return false;
        };
        let flags = SEND_AUTO_RESTART
            | if reliable {
                SEND_RELIABLE
            } else {
                SEND_UNRELIABLE
            };
        let to = Identity::steam_id(to);
        // SAFETY: valid identity and data for the call; Steam copies the bytes before returning.
        let result =
            unsafe { (self.0.send)(self.0.messages, &to, bytes.as_ptr(), len, flags, channel) };
        result == RESULT_OK
    }

    /// Accepts a pending session from `peer`, if there is one (`false` otherwise, harmless).
    /// Sessions from peers never accepted are never opened.
    pub fn accept(self, peer: u64) -> bool {
        let who = Identity::steam_id(peer);
        // SAFETY: a valid identity for the call.
        unsafe { (self.0.accept)(self.0.messages, &who) }
    }

    /// What arrived on `channel`, as (sender's SteamID64, bytes).
    pub fn receive(self, channel: i32) -> Vec<(u64, Vec<u8>)> {
        const BATCH: usize = 64;
        let mut received = Vec::new();
        loop {
            let mut messages = [core::ptr::null_mut::<MessageHead>(); BATCH];
            // SAFETY: `messages` has room for `BATCH` pointers.
            let count = unsafe {
                (self.0.receive)(
                    self.0.messages,
                    channel,
                    messages.as_mut_ptr(),
                    BATCH as i32,
                )
            };
            let count = usize::try_from(count).unwrap_or(0).min(BATCH);
            for &message in &messages[..count] {
                // SAFETY: Steam handed us `count` valid messages, each ours until released once.
                unsafe {
                    let head = &*message;
                    if head.peer_kind == Identity::STEAM_ID && !head.data.is_null() {
                        let len = usize::try_from(head.size).unwrap_or(0);
                        let bytes = core::slice::from_raw_parts(head.data, len).to_vec();
                        received.push((head.peer_steam_id, bytes));
                    }
                    (self.0.release)(message);
                }
            }
            if count < BATCH {
                return received;
            }
        }
    }
}
