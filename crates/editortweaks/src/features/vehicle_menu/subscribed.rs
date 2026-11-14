//! The Subscribed list with folders. Steam pages this list itself, so the game never holds it
//! whole; but which items there are is Steam's list on this computer (`steam_ugc`), and which
//! folder each is in is ours. What's left is each item's row (title, dates, preview, its id for
//! the game): Steam is asked once a session for every subscribed item, as a `Selection` query (the
//! given items' details, the query the server's vehicle whitelist uses;
//! `ugc_browser::ask_for_items`), up to [`MAX_IDS`] at a time, and each row it answers with is
//! copied and kept for the session ([`Rows::copy_row`](crate::game::ugc_item::Rows::copy_row)).
//! Every page after that, and every change of folder, is built from the kept rows without asking
//! Steam again: a session asks fewer times than the game's own list, which asks for every page it
//! shows. Only items subscribed to since are asked about. Search keeps the game's own query: its
//! list is flat anyway. See `docs/re/ugc-steam-queries.md`.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use vehicle_folders::natural_cmp;

use super::ITEMS;
use crate::game::steam_ugc;
use crate::game::ugc_item_id::{ItemId, ItemIds};
use crate::prelude::*;

/// Most items one query asks for: Steam's limit for a details request ("currently a 1,000 limit",
/// partner.steamgames.com ISteamUGC, read 2026-11-11). One answer holds them all (91 seen in game).
pub const MAX_IDS: usize = 1000;

/// `EUGCSortMethod` values the Subscribed category offers besides title (FluUGCSteam maps them to
/// Steam's `SubscriptionDateDesc` and `LastUpdatedDesc`; the rest falls back to title).
const SORT_SUBSCRIBED_NEWEST: u8 = 2;
const SORT_LAST_UPDATED: u8 = 3;

static IDS: OnceLock<ItemIds> = OnceLock::new();

/// A row Steam answered with, copied (ours, never destroyed), and what the listing sorts by.
struct Kept {
    row: *mut u8,
    title: String,
    updated: i64,
}

// Game thread only: the browser's hooks run there.
thread_local! {
    /// Every item Steam answered with this session.
    static KEPT: RefCell<HashMap<u64, Kept>> = RefCell::new(HashMap::new());
    /// Asked for, and Steam answered without them (deleted, private): not listed this session.
    static GONE: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
    /// The game made no id from text, or a row couldn't be kept: the Subscribed list stays the
    /// game's for the session.
    static BROKEN: Cell<bool> = const { Cell::new(false) };
}

/// Finds what the list needs (after engine init, once the rows are known).
pub fn start() -> Result<(), String> {
    let items = ITEMS.get().ok_or("the rows weren't found")?;
    if let Some(reason) = items.rows.no_copy() {
        return Err(format!("rows can't be kept ({reason})"));
    }
    let ids = ItemIds::find(items.rows.ustruct())?;
    if !crate::game::ugc_browser::can_ask_for_items() {
        return Err("the query settings aren't laid out as expected".into());
    }
    let _ = IDS.set(ids);
    Ok(())
}

/// Whether the Subscribed list gets folders.
pub fn available() -> bool {
    IDS.get().is_some() && !BROKEN.get()
}

/// The Subscribed list stops getting folders for the session (logged).
fn give_up(reason: &str) {
    if !BROKEN.replace(true) {
        log!("vehicle folders: no folders in Subscriptions ({reason})");
    }
}

/// The Workshop item id of a Steam result `row`, if it has one.
///
/// # Safety
/// `row` must be a live row (`FUGCQueryResult`). Game thread only.
pub unsafe fn workshop_id(row: *const u8) -> Option<u64> {
    // SAFETY: per the contract.
    unsafe { IDS.get()?.of_row(row) }
}

/// The subscribed items, as far as Steam answered about them.
pub struct Subscribed {
    /// The items to list and their kept rows (alive for the session), in the order the sort asks
    /// for.
    pub listed: Vec<(u64, *const u8)>,
    /// Items Steam wasn't asked about yet, newest subscription first.
    pub unknown: Vec<u64>,
}

/// The subscribed items, `listed` in the order of `sort` (the browser's `EUGCSortMethod`).
pub fn subscribed(sort: u8) -> Result<Subscribed, String> {
    // Steam's order is the subscriptions', newest first (seen in game, 2026-11-11).
    let ids = steam_ugc::subscribed_items()?;
    KEPT.with_borrow(|kept| {
        let mut listed = Vec::new();
        let mut unknown = Vec::new();
        GONE.with_borrow(|gone| {
            for id in ids {
                match kept.get(&id) {
                    Some(item) => listed.push((id, item)),
                    None if gone.contains(&id) => {}
                    None => unknown.push(id),
                }
            }
        });
        match sort {
            SORT_SUBSCRIBED_NEWEST => {}
            SORT_LAST_UPDATED => listed.sort_by_key(|(_, item)| std::cmp::Reverse(item.updated)),
            _ => listed.sort_by(|a, b| natural_cmp(&a.1.title, &b.1.title)),
        }
        Ok(Subscribed {
            listed: listed
                .into_iter()
                .map(|(id, item)| (id, item.row.cast_const()))
                .collect(),
            unknown,
        })
    })
}

/// Whether `sort` lists by title (for the "Mixed" view, which sorts folders in among them).
pub fn sorts_by_title(sort: u8) -> bool {
    !matches!(sort, SORT_SUBSCRIBED_NEWEST | SORT_LAST_UPDATED)
}

/// The game's ids of `ids`, to ask Steam for. If the game makes none, the Subscribed list stays the
/// game's (logged once).
///
/// # Safety
/// Game thread only, after engine init.
pub unsafe fn item_ids(ids: &[u64]) -> Option<Vec<ItemId>> {
    let maker = IDS.get()?;
    // SAFETY: per the contract.
    let made: Result<Vec<ItemId>, String> =
        ids.iter().map(|&id| unsafe { maker.make(id) }).collect();
    made.map_err(|reason| give_up(&reason)).ok()
}

/// Keeps the items of Steam's answer `rows` not kept yet (a row without a title is one Steam
/// answered for without its details: left out). The items it answered with.
///
/// # Safety
/// `rows` must be the game's live results. Game thread only.
pub unsafe fn keep(rows: &[*const u8]) -> HashSet<u64> {
    let Some(items) = ITEMS.get() else {
        return HashSet::new();
    };
    let mut answered = HashSet::with_capacity(rows.len());
    KEPT.with_borrow_mut(|kept| {
        for &row in rows {
            // SAFETY: per the contract.
            let Some(id) = (unsafe { workshop_id(row) }) else {
                continue;
            };
            // SAFETY: as above.
            let title = unsafe { items.rows.title(row) };
            if title.is_empty() {
                continue;
            }
            answered.insert(id);
            if kept.contains_key(&id) {
                continue;
            }
            // SAFETY: as above.
            let (copied, updated) = unsafe {
                (
                    items.rows.copy_row(row),
                    items.rows.times(row).map_or(0, |(_, updated)| updated),
                )
            };
            match copied {
                Ok(row) => {
                    kept.insert(
                        id,
                        Kept {
                            row,
                            title,
                            updated,
                        },
                    );
                }
                Err(reason) => give_up(&format!("a row wasn't kept: {reason}")),
            }
        }
    });
    GONE.with_borrow_mut(|gone| gone.retain(|id| !answered.contains(id)));
    answered
}

/// Leaves `ids` out of the list this session: asked for, Steam answered without them.
pub fn gone(ids: impl IntoIterator<Item = u64>) {
    GONE.with_borrow_mut(|gone| gone.extend(ids));
}
