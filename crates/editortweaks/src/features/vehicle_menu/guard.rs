//! A last check before the game deletes a vehicle: `UBrickGameInstance::DeleteUGCItem` deletes the
//! vehicle's folder with everything in it, so it only goes ahead for an id that names one vehicle's
//! folder ([`vehicle_folders::is_vehicle_id`]): never a folder tile, never an empty id (which would
//! name the whole vehicles folder). The game never selects a tile anyway (`tiles`), and rebuilding a
//! page clears its selection (`UPagedListWidget::OnEntriesLoaded`): this is the net under both.
//!
//! A vehicle the game did delete is forgotten from the folders file, so a new vehicle that gets its
//! id later starts unfiled.

use std::sync::OnceLock;

use vehicle_folders::is_vehicle_id;

use super::{ITEMS, save, with_state};
use crate::framework::timings;
use crate::game::ugc_browser::DeleteItemFn;
use crate::prelude::*;

pub static ORIGINAL_DELETE: OnceLock<DeleteItemFn> = OnceLock::new();

/// `UBrickGameInstance::DeleteUGCItem(const FUGCFileInfo&)`: the original only for a vehicle's id.
pub unsafe extern "C" fn delete_item(game_instance: *mut u8, info: *const u8) -> u8 {
    let _span = timings::span("vehicle_menu::delete_item");
    let original = *ORIGINAL_DELETE.get().expect("hook recorded before use");
    // Before engine init (no rows known) the game is on its own, as without the mod; tiles only
    // exist after it.
    let id = ITEMS
        .get()
        .filter(|_| !info.is_null())
        // SAFETY: the game passes a live file info: a row (`FUGCQueryResult` starts with it).
        .map(|items| unsafe { items.rows.local_id(info) });
    if let Some(id) = &id
        && !is_vehicle_id(id)
    {
        log!("vehicle folders: refused to delete {id:?}, not a vehicle's folder name");
        return 0;
    }
    // SAFETY: the original with its own arguments.
    let deleted = unsafe {
        timings::game("UBrickGameInstance::DeleteUGCItem", || {
            original(game_instance, info)
        })
    };
    if deleted != 0
        && let Some(id) = id
        && with_state(|state| state.ready && state.store.local.forget_vehicle(&id))
    {
        save();
    }
    deleted
}
