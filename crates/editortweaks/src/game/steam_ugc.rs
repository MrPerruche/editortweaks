//! The player's subscribed Workshop items, from Steam's own list on this computer
//! (`ISteamUGC::GetSubscribedItems`, no network), through the game's `steam_api64.dll` (SDK 1.63,
//! `ISteamUGC` v021). Every function is a flat export looked up by name (as `steam`, coop's), so
//! nothing here depends on the game exe. See `docs/re/ugc-steam-queries.md`.

use core::ffi::{CStr, c_void};
use std::sync::OnceLock;

use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

const DLL: &str = "steam_api64.dll";

type Interface = *mut c_void;
/// `uint32 GetNumSubscribedItems(bool bIncludeLocallyDisabled)` (SDK 1.58 and later).
type CountFn = unsafe extern "C" fn(ugc: Interface, include_disabled: bool) -> u32;
/// `uint32 GetSubscribedItems(PublishedFileId_t* ids, uint32 max, bool bIncludeLocallyDisabled)`.
type ListFn =
    unsafe extern "C" fn(ugc: Interface, ids: *mut u64, max: u32, include_disabled: bool) -> u32;

struct Api {
    ugc: Interface,
    count: CountFn,
    list: ListFn,
}

// SAFETY: Steam's interface pointer and exports live as long as the game; only called on the game
// thread.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

static API: OnceLock<Api> = OnceLock::new();

/// Looks up `name` in `module` as a function of type `F`.
///
/// # Safety
/// `F` must be the export's real signature, as a function pointer type.
unsafe fn export<F>(module: *mut c_void, name: &CStr) -> Result<F, String> {
    // SAFETY: `module` is a loaded module's handle and `name` is NUL-terminated.
    let address = unsafe { GetProcAddress(module, name.as_ptr().cast()) }
        .ok_or_else(|| format!("{DLL} has no {}", name.to_string_lossy()))?;
    // SAFETY: the caller guarantees `F` is the export's signature; both are function pointers.
    Ok(unsafe { core::mem::transmute_copy::<_, F>(&address) })
}

fn resolve() -> Result<Api, String> {
    let wide: Vec<u16> = DLL.encode_utf16().chain([0]).collect();
    // SAFETY: a NUL-terminated name.
    let module = unsafe { GetModuleHandleW(wide.as_ptr()) };
    if module.is_null() {
        return Err(format!("{DLL} isn't loaded"));
    }
    // SAFETY: each type is the export's signature (`steam_api_flat.h`, SDK 1.63); the accessor
    // takes nothing and returns the interface, null while Steam isn't up.
    unsafe {
        let accessor: unsafe extern "C" fn() -> Interface =
            export(module, c"SteamAPI_SteamUGC_v021")?;
        let ugc = accessor();
        if ugc.is_null() {
            return Err("SteamAPI_SteamUGC_v021 returned null".into());
        }
        Ok(Api {
            ugc,
            count: export(module, c"SteamAPI_ISteamUGC_GetNumSubscribedItems")?,
            list: export(module, c"SteamAPI_ISteamUGC_GetSubscribedItems")?,
        })
    }
}

/// Every Workshop item the player is subscribed to (locally disabled ones left out, as Steam does
/// by default), in Steam's order: the order they were subscribed in, newest first (seen in game).
pub fn subscribed_items() -> Result<Vec<u64>, String> {
    let api = match API.get() {
        Some(api) => api,
        // Kept only once it worked: Steam may not be up yet the first time.
        None => {
            let api = resolve()?;
            API.get_or_init(|| api)
        }
    };
    // SAFETY: Steam's live interface with its own functions; the buffer holds `count` ids.
    unsafe {
        let count = (api.count)(api.ugc, false);
        let mut ids = vec![0u64; count as usize];
        let written = (api.list)(api.ugc, ids.as_mut_ptr(), count, false);
        ids.truncate(written.min(count) as usize);
        Ok(ids)
    }
}
