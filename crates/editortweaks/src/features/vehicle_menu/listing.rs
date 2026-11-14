//! The Local and Subscribed lists with folders: [`query_complete`] shows one page of our listing
//! (the folder shown, its tiles, pins first) instead of the game's. See `docs/vehicle-menu.md`, "How
//! the Local list gets folders", and `subscribed`.
//!
//! `OnLoadListPage(page)` -> `QueryUGC(settings)` -> `OnUGCQueryComplete(results)`: [`load_page`]
//! notes the browser; [`query`] asks, for Local, for page 0 of [`ALL`] items (answered at once),
//! for Subscribed, Steam for the items it wasn't asked about yet this session (answered later; else
//! answered at once from the rows kept, `subscribed`), and notes the page wanted; [`query_complete`] builds that page from our rows and the game's and hands it on.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;
use vehicle_folders::{Category, Item, ListOptions, Location, Row, ViewMode, list, page_range};

use super::subscribed::{self, MAX_IDS};
use super::tiles::{self, Refresh, Tile};
use super::{BROWSERS, DECOR, ITEMS, bar, buttons, category_of, drag, view_mode, with_state};
use crate::framework::timings;
use crate::game::ugc_browser::{self, LoadPageFn, QueryCompleteFn, QueryFn, ResultArray};
use crate::game::weak_object::WeakObject;
use crate::prelude::*;

/// The page size asking for every item: more than anyone has, and small enough that the game's
/// page count (`count - 1 + size`) can't overflow.
const ALL: i32 = 1 << 20;

/// Whether a folder's page starts with a ".." tile going up a level. Off: the navigation bar's Up
/// and path buttons do it (opening, and as drop targets while dragging).
const UP_TILE: bool = false;

/// The sort by title (`EUGCSortMethod::TitleAsc`, the settings' byte 2).
const SORT_BY_TITLE: u8 = 0;

pub static ORIGINAL_LOAD_PAGE: OnceLock<LoadPageFn> = OnceLock::new();
pub static ORIGINAL_QUERY: OnceLock<QueryFn> = OnceLock::new();
pub static ORIGINAL_QUERY_COMPLETE: OnceLock<QueryCompleteFn> = OnceLock::new();

/// A query of ours, waiting for its results.
#[derive(Clone)]
struct Pending {
    browser: *mut u8,
    page: usize,
    page_size: usize,
    sort: u8,
    kind: Kind,
    /// Whether its results came (later ones are a stale query's).
    answered: bool,
}

#[derive(Clone)]
enum Kind {
    /// Every local item, sorted by the game.
    Local,
    /// The Workshop items asked for (none: answered at once from the rows kept).
    Subscribed { asked: Vec<u64> },
}

// Game thread only: the browser asks and gets its pages there.
thread_local! {
    /// The browser loading a page, while it does.
    static LOADING: Cell<Option<*mut u8>> = const { Cell::new(None) };
    /// The browser's last query, if ours; until its next one.
    static PENDING: RefCell<Option<Pending>> = const { RefCell::new(None) };
}

/// `UUGCBrowserWidget::OnLoadListPage`: notes which browser asks, for [`query`].
pub unsafe extern "C" fn load_page(browser: *mut u8, page: i32) {
    let _span = timings::span("vehicle_menu::load_page");
    let original = *ORIGINAL_LOAD_PAGE.get().expect("hook recorded before use");
    if let Some(browsers) = BROWSERS.get() {
        // SAFETY: the game passes its live browser.
        let browser_object = browser.cast::<UObjectBase>();
        let is_browser = unsafe { browsers.is_browser(browser_object) };
        if is_browser {
            // SAFETY: as above.
            let (weak, category) = unsafe {
                (
                    WeakObject::new(browser_object),
                    category_of(browsers.query_type(browser_object)),
                )
            };
            with_state(|state| {
                state.browser = Some(weak);
                if let Some(category) = category {
                    state.show(category);
                }
            });
        }
    }
    let outer = LOADING.replace(Some(browser));
    // SAFETY: the original with its own arguments.
    unsafe {
        timings::game("UUGCBrowserWidget::OnLoadListPage", || {
            original(browser, page)
        })
    };
    LOADING.set(outer);
    // The bar and dragging follow what the browser shows now (a Local page is shown already).
    let browser_object = browser.cast::<UObjectBase>();
    // SAFETY: the game's live object, on the game thread.
    if BROWSERS
        .get()
        .is_some_and(|browsers| unsafe { browsers.is_browser(browser_object) })
    {
        // SAFETY: a live browser, on the game thread.
        unsafe {
            bar::update(browser_object);
            buttons::follow_category(browser_object);
        }
        drag::watch();
    }
}

/// `UBrickGameInstance::QueryUGC`'s arguments, to call it again.
#[derive(Clone, Copy)]
struct QueryArgs {
    game_instance: *mut u8,
    settings: *const u8,
    task: *mut u8,
}

impl QueryArgs {
    /// The original with these arguments.
    ///
    /// # Safety
    /// The arguments must be the game's own, in their call.
    unsafe fn original(self) -> u8 {
        let original = *ORIGINAL_QUERY.get().expect("hook recorded before use");
        // SAFETY: per the contract.
        unsafe {
            timings::game("UBrickGameInstance::QueryUGC", || {
                original(self.game_instance, self.settings, self.task)
            })
        }
    }
}

/// `UBrickGameInstance::QueryUGC`: a browser's Local query asks for every item, its Subscribed
/// query Steam for the items not asked about yet this session (when folders or pins change
/// anything), the page it wanted noted for [`query_complete`].
pub unsafe extern "C" fn query(game_instance: *mut u8, settings: *const u8, task: *mut u8) -> u8 {
    let _span = timings::span("vehicle_menu::query");
    let args = QueryArgs {
        game_instance,
        settings,
        task,
    };
    // Only the browser's own query, once per page it loads.
    let Some(browser) = LOADING.take() else {
        // SAFETY: the game's own arguments.
        return unsafe { args.original() };
    };
    // A newer query: an earlier one's results are stale.
    PENDING.set(None);
    // SAFETY: the game passes its live settings.
    let asked = unsafe { ugc_browser::read_settings(settings) };
    let category = category_of(asked.query_type).filter(|&category| {
        with_state(|state| state.ready && state.changes_list(category, view_mode()))
            && ITEMS.get().is_some()
    });
    let Some(category) = category else {
        // SAFETY: the game's own arguments.
        return unsafe { args.original() };
    };
    let pending = Pending {
        browser,
        page: usize::try_from(asked.page).unwrap_or(0),
        page_size: usize::try_from(asked.page_size).unwrap_or(1).max(1),
        sort: asked.sort,
        kind: Kind::Local,
        answered: false,
    };
    let settings_mut = settings.cast_mut();
    match category {
        Category::Local => {
            PENDING.set(Some(pending));
            // SAFETY: `OnLoadListPage` builds the settings on its own stack and only reads them
            // after; they're put back before returning.
            let answer = unsafe {
                ugc_browser::write_page(settings_mut, 0, ALL);
                let answer = args.original();
                ugc_browser::write_page(settings_mut, asked.page, asked.page_size);
                answer
            };
            // A Local query answers at once; if not, the game's list shows as it comes.
            PENDING.set(None);
            answer
        }
        // SAFETY: the game's own arguments, `OnLoadListPage`'s settings; on the game thread.
        Category::Subscribed => unsafe {
            match ask_steam(args, pending) {
                Some(answer) => answer,
                None => args.original(),
            }
        },
    }
}

/// Asks Steam about the subscribed items it wasn't asked about yet this session (up to
/// [`MAX_IDS`]); or, with every item's row kept, answers at once. `None`: the game's own query
/// instead (while searching, or when the items can't be asked for).
///
/// # Safety
/// `args` must be the game's own, `OnLoadListPage`'s settings. Game thread only.
unsafe fn ask_steam(args: QueryArgs, mut pending: Pending) -> Option<u8> {
    let browsers = BROWSERS.get()?;
    // SAFETY: the browser is live while it loads its page.
    if unsafe { browsers.is_filtering(pending.browser.cast()) } {
        return None;
    }
    let known = match subscribed::subscribed(pending.sort) {
        Ok(known) => known,
        Err(reason) => {
            static TOLD: std::sync::Once = std::sync::Once::new();
            TOLD.call_once(|| log!("vehicle folders: Subscriptions without folders ({reason})"));
            return None;
        }
    };
    let wanted: Vec<u64> = known.unknown.into_iter().take(MAX_IDS).collect();
    pending.kind = Kind::Subscribed {
        asked: wanted.clone(),
    };
    let browser = pending.browser;
    if wanted.is_empty() {
        // Nothing to ask Steam: the page now, as a Local one comes.
        PENDING.set(Some(pending));
        let none = ResultArray {
            data: core::ptr::null_mut(),
            num: 0,
            max: 0,
        };
        // SAFETY: our own hook, with an empty result array, for the browser loading its page.
        unsafe { query_complete(browser, &none, 0, 0) };
        return Some(1);
    }
    // SAFETY: per the contract.
    let ids = unsafe { subscribed::item_ids(&wanted)? };
    let settings = args.settings.cast_mut();
    // SAFETY: per the contract; `ids` outlive the call, and the settings are put back before
    // `OnLoadListPage` sees them again (so it never destroys our array).
    let asked = unsafe { ugc_browser::ask_for_items(settings, &ids)? };
    log!(
        "vehicle folders: asking Steam about {} subscribed items",
        wanted.len()
    );
    PENDING.set(Some(pending));
    // SAFETY: as above.
    let answer = unsafe {
        let answer = args.original();
        ugc_browser::put_back_items(settings, asked);
        answer
    };
    if answer == 0 {
        PENDING.set(None);
    }
    Some(answer)
}

/// `UUGCBrowserWidget::OnUGCQueryComplete`: for a query of ours, the page of our listing instead
/// of the game's results. A drag going on finds its row on the page shown.
pub unsafe extern "C" fn query_complete(
    browser: *mut u8,
    results: *const ResultArray,
    entries: u32,
    pages: u32,
) {
    let _span = timings::span("vehicle_menu::query_complete");
    // SAFETY: the game's own arguments.
    unsafe { show_results(browser, results, entries, pages) };
    drag::page_shown();
}

/// [`query_complete`]'s page.
///
/// # Safety
/// The game's own arguments to `OnUGCQueryComplete`. Game thread only.
unsafe fn show_results(browser: *mut u8, results: *const ResultArray, entries: u32, pages: u32) {
    let original = *ORIGINAL_QUERY_COMPLETE
        .get()
        .expect("hook recorded before use");
    // SAFETY: the original with its own arguments.
    let vanilla = || unsafe {
        timings::game("UUGCBrowserWidget::OnUGCQueryComplete", || {
            original(browser, results, entries, pages)
        })
    };
    let pending = PENDING.with_borrow_mut(|pending| {
        let pending = pending
            .as_mut()
            .filter(|pending| pending.browser == browser)?;
        let first = !pending.answered;
        pending.answered = true;
        Some((pending.clone(), first))
    });
    let Some((pending, first)) = pending else {
        return vanilla();
    };
    // SAFETY: the game's live results, read while it waits for this call.
    let rows = unsafe { result_rows(results) };
    let built = match &pending.kind {
        // SAFETY: as above.
        Kind::Local => unsafe { build_local(browser, &rows, &pending) },
        Kind::Subscribed { asked } => {
            // SAFETY: as above.
            let answered = unsafe { subscribed::keep(&rows) };
            // An answer with items not asked for is an earlier query's: kept only.
            let stale = !first || answered.iter().any(|id| !asked.contains(id));
            if stale {
                return;
            }
            // SAFETY: on the game thread.
            unsafe { build_subscribed(&pending, asked, &answered) }
        }
    };
    let Some(page) = built else {
        return vanilla();
    };
    let header = ResultArray {
        data: page.rows.as_ptr().cast_mut().cast(),
        num: page.count as i32,
        max: page.count as i32,
    };
    // SAFETY: the array holds `count` rows: bitwise copies of the game's (alive until the game
    // destroys its results, after this call) and of ours (kept for the session). The browser only
    // copies them (`OnUGCQueryComplete` reads its argument), and never destroys our array.
    unsafe {
        timings::game("UUGCBrowserWidget::OnUGCQueryComplete", || {
            original(browser, &header, page.total as u32, page.pages as u32)
        })
    };
}

/// The rows of the game's `results`.
///
/// # Safety
/// `results` must be the game's live results. Game thread only.
unsafe fn result_rows(results: *const ResultArray) -> Vec<*const u8> {
    let Some(items) = ITEMS.get() else {
        return Vec::new();
    };
    // SAFETY: per the contract.
    let (data, count) = unsafe { ((*results).data, (*results).num) };
    let count = usize::try_from(count).unwrap_or(0);
    (0..count)
        .map(|index| data.wrapping_add(index * items.rows.size).cast_const())
        .collect()
}

/// The Local page `pending` asks for, out of every local item (`rows`, in the game's sort order).
///
/// # Safety
/// `rows` must be the game's live results for `browser`. Game thread only.
unsafe fn build_local(browser: *mut u8, rows: &[*const u8], pending: &Pending) -> Option<Page> {
    // SAFETY: per the contract.
    let source = unsafe { Source::local(rows) };
    // SAFETY: the browser is live while it gets its results.
    let filtering = unsafe { BROWSERS.get()?.is_filtering(browser.cast()) };
    // SAFETY: per the contract.
    unsafe { build_page(&source, pending, filtering, pending.sort == SORT_BY_TITLE) }
}

/// The Subscribed page `pending` asks for, out of the rows kept, after Steam answered with
/// `answered` of the items `asked` (none: nothing was asked). Items it answered without are left
/// out for the session; if there are more items to ask about, the page is asked for again.
///
/// # Safety
/// Game thread only.
unsafe fn build_subscribed(
    pending: &Pending,
    asked: &[u64],
    answered: &HashSet<u64>,
) -> Option<Page> {
    if !asked.is_empty() {
        log!(
            "vehicle folders: Steam told about {} of {} subscribed items asked",
            answered.len(),
            asked.len()
        );
        // An empty answer is a query that failed (no connection, too many queries): asked again
        // with the next page shown, not before.
        if !answered.is_empty() {
            subscribed::gone(asked.iter().copied().filter(|id| !answered.contains(id)));
        }
    }
    let known = subscribed::subscribed(pending.sort).ok()?;
    let source = Source::kept(&known.listed);
    // SAFETY: the kept rows live for the session; on the game thread.
    let page = unsafe {
        build_page(
            &source,
            pending,
            false,
            subscribed::sorts_by_title(pending.sort),
        )?
    };
    if !answered.is_empty() && !known.unknown.is_empty() {
        // More than one query's worth: the next ones.
        tiles::show_next_frame(Refresh::SamePage);
    }
    Some(page)
}

/// What a page is built from: every item to list, in order, and its row.
struct Source {
    items: Vec<Item>,
    rows: Vec<*const u8>,
}

impl Source {
    /// Every local item, from the game's rows.
    ///
    /// # Safety
    /// `rows` must be the game's live results. Game thread only.
    unsafe fn local(rows: &[*const u8]) -> Self {
        let rows_api = &ITEMS.get().expect("checked by the caller").rows;
        let items = rows
            .iter()
            // SAFETY: per the contract.
            .map(|&row| unsafe { Item::new(super::row_key(row), rows_api.title(row)) })
            .collect();
        Self {
            items,
            rows: rows.to_vec(),
        }
    }

    /// The subscribed items listed, from their kept rows.
    fn kept(listed: &[(u64, *const u8)]) -> Self {
        let rows_api = &ITEMS.get().expect("checked by the caller").rows;
        Self {
            items: listed
                .iter()
                // SAFETY: a kept row, alive for the session; on the game thread.
                .map(|&(id, row)| Item::new(id.to_string(), unsafe { rows_api.title(row) }))
                .collect(),
            rows: listed.iter().map(|&(_, row)| row).collect(),
        }
    }
}

/// One page of our listing, as rows laid out like the game's array.
struct Page {
    /// The rows' bytes (`u64`s keep them aligned like the game's).
    rows: Vec<u64>,
    count: usize,
    /// Rows in the whole listing, and pages.
    total: usize,
    pages: usize,
}

/// What a slot of the listing shows.
enum Shown {
    /// The item at this index of the source.
    Vehicle(usize),
    /// A tile, the vehicles in it, and those it previews (indexes into the source).
    Tile(Tile, usize, Vec<usize>),
}

/// The listing of `items` at the location shown; and whether it's flat (no folders).
fn view(items: &[Item], filtering: bool, sort_by_title: bool) -> (Vec<Shown>, bool) {
    let mode = view_mode();
    let (listed, location) = with_state(|state| {
        // A folder deleted or renamed since is left for one that exists.
        state.fix_up_history();
        let location = state.here().clone();
        let options = ListOptions {
            location: location.clone(),
            mode,
            search_active: filtering,
            sort_by_title,
        };
        (list(state.tree(), items, &options), location)
    });
    let flat = mode == ViewMode::Off || filtering;
    let mut shown: Vec<Shown> = Vec::with_capacity(listed.len() + 1);
    if UP_TILE && !flat && location != Location::Root {
        shown.push(Shown::Tile(Tile::Up, 0, Vec::new()));
    }
    shown.extend(listed.into_iter().map(|row| match row {
        Row::Vehicle { index, .. } => Shown::Vehicle(index),
        Row::Folder {
            path,
            vehicle_count,
            preview,
            ..
        } => Shown::Tile(Tile::Folder(path), vehicle_count, preview),
        Row::Uncategorized {
            vehicle_count,
            preview,
        } => Shown::Tile(Tile::Uncategorized, vehicle_count, preview),
    }));
    (shown, flat)
}

/// The page `pending` asks for, out of `source`.
///
/// # Safety
/// `source`'s rows must be live (the game's results while it waits, or ours). Game thread only.
unsafe fn build_page(
    source: &Source,
    pending: &Pending,
    filtering: bool,
    sort_by_title: bool,
) -> Option<Page> {
    let rows_api = &ITEMS.get()?.rows;
    let (shown, flat) = view(&source.items, filtering, sort_by_title);
    if !flat {
        // SAFETY: per the contract.
        let times = (0..source.items.len())
            .filter_map(|index| unsafe {
                let (created, updated) = rows_api.times(source.rows[index])?;
                Some((source.items[index].id.clone(), created, updated))
            })
            .collect();
        with_state(|state| state.vehicle_times = times);
    }
    let (range, pages) = page_range(shown.len(), pending.page, pending.page_size);
    let words_per_row = rows_api.size.div_ceil(size_of::<u64>());
    // Rows must follow each other at the game's stride.
    if words_per_row * size_of::<u64>() != rows_api.size {
        return None;
    }
    let mut rows = vec![0u64; words_per_row * range.len()];
    let mut previews = Vec::new();
    let mut counts = Vec::new();
    let mut slot = 0;
    for shown in &shown[range] {
        let source_row = match shown {
            Shown::Vehicle(index) => source.rows[*index],
            Shown::Tile(tile, vehicle_count, preview) => {
                counts.push((tile.clone(), *vehicle_count));
                // The thumbnails now, while the game's rows are alive.
                if let Some(decor) = DECOR.get()
                    && !preview.is_empty()
                {
                    // SAFETY: per the contract; each index is one of the source's.
                    let thumbnails = preview
                        .iter()
                        .map(|&index| unsafe { decor.thumbnail_of(source.rows[index]) })
                        .collect();
                    previews.push((tile.clone(), thumbnails));
                }
                // SAFETY: on the game thread; the game copied our rows' last use before.
                unsafe { tiles::row(tile, &tiles::label(tile, *vehicle_count))? }
            }
        };
        // SAFETY: both are rows of `size` bytes; the target slot is within `rows`.
        unsafe {
            core::ptr::copy_nonoverlapping(
                source_row,
                rows.as_mut_ptr().add(slot * words_per_row).cast::<u8>(),
                rows_api.size,
            );
        }
        slot += 1;
    }
    tiles::set_previews(previews);
    tiles::set_counts(counts);
    Some(Page {
        rows,
        count: slot,
        total: shown.len(),
        pages,
    })
}
