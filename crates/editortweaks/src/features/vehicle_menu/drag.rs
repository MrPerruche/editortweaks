//! Dragging vehicles and folders onto folders in the Local list. See `docs/vehicle-menu.md`, "Drag
//! and drop".
//!
//! UMG's own drag and drop would need a native virtual of the row widget hooked, so instead: while
//! the browser is open, a timer looks at the primary mouse button and the cursor about once a frame
//! ([`vehicle_folders::Gesture`] decides what's a drag). A press on a row's button makes the button
//! capture the mouse until it's released, and a release elsewhere is no click, so the game neither
//! selects nor spawns what was dragged away. Rows and the bar's buttons are found under the cursor
//! by their last layout (`game::widget_hit`). While dragging, the row dragged is see-through and
//! shows the grab hand, a see-through copy of its thumbnail follows the cursor (`game::drag_ghost`),
//! and the folder tile (or bar button) under the cursor is highlighted. Released
//! over one: moved there, as the folders file says ([`vehicle_folders::drop_onto`]). A vehicle
//! released over another vehicle (not while searching, the list being flat then): both go into a
//! new folder where that one is, as phones group apps ([`vehicle_folders::group_into_new_folder`]),
//! which is selected to be named in the side panel. Anywhere else: nothing happens.
//!
//! Held over the page selector's previous or next button, the page turns (after a moment, then
//! again while it stays: [`vehicle_folders::HoverTimer`]; `game::page_selector`), so things can be
//! dropped on other pages. The rows are the same widgets showing other items then: the row faded
//! is whichever shows what's dragged ([`page_shown`]), and the release, which the pressed row's
//! button may take as a click on whatever it shows now, is no click ([`swallows_click`]).

use std::cell::{Cell, RefCell};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use brickrust::ue::coreuobject::UObjectBase;
use vehicle_folders::{
    Dragged, Event, Gesture, HoverTimer, Location, ViewMode, drop_onto, group_into_new_folder,
};

use super::tiles::{self, Refresh, Tile};
use super::{BROWSERS, DECOR, ITEMS, bar, buttons, category_of, save, view_mode, with_state};
use crate::framework::timings;
use crate::game::drag_ghost::{Ghost, Ghosts};
use crate::game::page_selector::{PageSelectors, Turn};
use crate::game::weak_object::WeakObject;
use crate::game::widget_hit::WidgetHits;
use crate::platform::timer::ThreadTimer;
use crate::platform::{cursor, window};
use crate::prelude::*;

/// How often the mouse is looked at: about once a frame (the timer ticks with the game thread's
/// messages).
const TICK_MS: u32 = 15;

/// How see-through the dragged row is.
const DRAGGED_OPACITY: f32 = 0.5;

/// How long after a drop a click on a row is still its release (Slate may tell it a frame later).
const RELEASE_CLICK: Duration = Duration::from_millis(300);

static HITS: OnceLock<WidgetHits> = OnceLock::new();
/// The copy following the cursor; dragging works without it.
static GHOSTS: OnceLock<Ghosts> = OnceLock::new();
/// Turning pages while dragging; dragging works without it.
static PAGES: OnceLock<PageSelectors> = OnceLock::new();

/// What a press landed on: what it drags, the row widget pressed (holding the mouse until the
/// release, with the grab hand), and the one showing what's dragged now (faded; another row, or
/// none, once the page changed).
#[derive(Clone)]
struct Pressed {
    dragged: Dragged,
    widget: WeakObject,
    shown: Option<WeakObject>,
}

/// A page button under the cursor while dragging, and its own color style to give back.
#[derive(Clone)]
struct PageSpot {
    button: WeakObject,
    style: Option<u8>,
}

/// Something under the cursor a drop can land on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Spot {
    /// A row of the list: a folder tile (or "..", Uncategorized), or a vehicle.
    Tile(*mut UObjectBase),
    /// A button of the navigation bar.
    Button(*mut UObjectBase),
}

/// What a drop does.
#[derive(Clone)]
enum Drop {
    /// Files what's dragged there.
    Into(Location),
    /// Puts the dragged vehicle and this one (by id) into a new folder.
    Group(String),
}

struct Watch {
    /// Kept for its ticks: dropping it stops them.
    _timer: ThreadTimer,
    gesture: Gesture<Pressed>,
    /// The drop target highlighted now, and what a drop there does.
    hovered: Option<(Spot, Drop)>,
    /// The copy of the dragged row's thumbnail following the cursor.
    ghost: Option<Ghost>,
    /// The page button highlighted now, and when to turn the page.
    page: Option<PageSpot>,
    turner: HoverTimer<Turn>,
}

thread_local! {
    /// While the browser is watched. Game thread only.
    static WATCH: RefCell<Option<Watch>> = const { RefCell::new(None) };
    /// When the last drag was dropped.
    static DROPPED_AT: Cell<Option<Instant>> = const { Cell::new(None) };
    /// The page turns' clock.
    static EPOCH: Instant = Instant::now();
}

/// Finds what dragging needs. After engine init.
pub fn start() -> Result<(), String> {
    let _ = HITS.set(WidgetHits::find()?);
    match Ghosts::find() {
        Ok(ghosts) => {
            let _ = GHOSTS.set(ghosts);
        }
        Err(reason) => log!("vehicle folders: nothing follows the cursor when dragging ({reason})"),
    }
    match PageSelectors::find() {
        Ok(pages) => {
            let _ = PAGES.set(pages);
        }
        Err(reason) => log!("vehicle folders: no page turns while dragging ({reason})"),
    }
    Ok(())
}

/// Whether a click on a row now is a drag's release, not a click: while dragging, and just after
/// a drop.
pub fn swallows_click() -> bool {
    let dragging = WATCH.with_borrow(|watch| {
        watch
            .as_ref()
            .is_some_and(|watch| watch.gesture.dragging().is_some())
    });
    dragging
        || DROPPED_AT
            .get()
            .is_some_and(|at| at.elapsed() < RELEASE_CLICK)
}

/// Watches the mouse while the browser is open (called as it loads a page); nothing if it already
/// does.
pub fn watch() {
    if HITS.get().is_none() || WATCH.with_borrow(Option::is_some) {
        return;
    }
    let Some(timer) = ThreadTimer::start(TICK_MS, tick) else {
        log!("vehicle folders: no timer, so no drag and drop");
        return;
    };
    // A button already down (the click that opened the list) is never a drag.
    let gesture = if cursor::primary_button_down() {
        Gesture::held()
    } else {
        Gesture::new()
    };
    WATCH.with_borrow_mut(|watch| {
        *watch = Some(Watch {
            _timer: timer,
            gesture,
            hovered: None,
            ghost: None,
            page: None,
            turner: HoverTimer::new(),
        })
    });
}

/// The browser and its list, while dragging can happen: open, showing the Local list with folders.
fn context() -> Option<(*mut UObjectBase, *mut UObjectBase)> {
    let browsers = BROWSERS.get()?;
    let browser = with_state(|state| {
        state
            .ready
            .then_some(state.browser)
            .flatten()
            .and_then(|browser| browser.get_live())
    })?;
    // SAFETY: the live browser, on the game thread (the timer runs there).
    unsafe {
        let usable = browsers.is_open(browser)
            && category_of(browsers.query_type(browser)).is_some()
            && view_mode() != ViewMode::Off;
        if !usable {
            return None;
        }
        Some((browser, browsers.paged_list(browser)?))
    }
}

/// The timer's tick.
fn tick() {
    let _span = timings::span("vehicle_menu::drag");
    let Some((browser, list)) = context() else {
        stop();
        return;
    };
    // While searching the list is flat, its vehicles from any folder: no grouping.
    // SAFETY: the live browser, on the game thread.
    let can_group = BROWSERS
        .get()
        .is_some_and(|browsers| !unsafe { browsers.is_filtering(browser) });
    // Another window over the game: its clicks aren't ours.
    let down = cursor::primary_button_down() && window::is_foreground();
    let Some(point) = cursor::position() else {
        return;
    };
    let gesture = WATCH.with_borrow_mut(|watch| {
        watch
            .as_mut()
            .map(|watch| core::mem::take(&mut watch.gesture))
    });
    let Some(mut gesture) = gesture else {
        return;
    };
    // SAFETY: the live list, on the game thread; the state isn't borrowed while the game is called.
    let event = gesture.update(down, point, || unsafe { pressed_on(list, point) });
    WATCH.with_borrow_mut(|watch| {
        if let Some(watch) = watch {
            watch.gesture = gesture;
        }
    });
    match event {
        Event::Nothing => {}
        Event::Started(pressed) => {
            if let (Some(widget), Some(hits)) = (pressed.widget.get_live(), HITS.get()) {
                // SAFETY: a live row widget, on the game thread.
                unsafe {
                    hits.set_opacity(widget, DRAGGED_OPACITY);
                    hits.show_grab(widget, true);
                    // A vehicle's row: its own image shows its thumbnail.
                    let row = match pressed.dragged {
                        Dragged::Vehicle(_) => ITEMS.get().and_then(|items| items.row_of(widget)),
                        Dragged::Folder(_) => None,
                    };
                    let thumbnail = |source, copy| {
                        if let Some(decor) = DECOR.get() {
                            // The live row widget, its row and image, and our copy (unsafe as
                            // the block around).
                            decor.copy_thumbnail(widget, row, source, copy);
                        }
                    };
                    let ghost = GHOSTS
                        .get()
                        .and_then(|ghosts| ghosts.make(widget, point, &thumbnail));
                    if ghost.is_none() && GHOSTS.get().is_some() {
                        log!("vehicle folders: the dragged row isn't copied to follow the cursor");
                    }
                    WATCH.with_borrow_mut(|watch| {
                        if let Some(watch) = watch {
                            watch.ghost = ghost;
                        }
                    });
                }
            }
            // SAFETY: as above.
            unsafe {
                let target = target_at(list, point, &pressed, can_group);
                let over_target = target.is_some();
                hover(target);
                hover_page(list, point, !over_target);
            }
        }
        Event::Moved(pressed) => {
            // SAFETY: as above; the ghost is moved without the state borrowed.
            unsafe {
                let ghost = WATCH.with_borrow_mut(|watch| watch.as_mut()?.ghost.take());
                if let (Some(ghost), Some(ghosts)) = (&ghost, GHOSTS.get()) {
                    ghosts.move_to(ghost, point);
                }
                WATCH.with_borrow_mut(|watch| {
                    if let Some(watch) = watch {
                        watch.ghost = ghost;
                    }
                });
                let target = target_at(list, point, &pressed, can_group);
                let over_target = target.is_some();
                hover(target);
                hover_page(list, point, !over_target);
            }
        }
        Event::Dropped(pressed) => {
            // SAFETY: as above.
            let target = unsafe { target_at(list, point, &pressed, can_group) };
            let (hovered, ghost) = WATCH
                .with_borrow_mut(|watch| {
                    let watch = watch.as_mut()?;
                    Some((watch.hovered.take(), watch.ghost.take()))
                })
                .unwrap_or_default();
            end_drag(&pressed, hovered.map(|(spot, _)| spot), ghost);
            // SAFETY: on the game thread.
            unsafe { hover_page(list, point, false) };
            DROPPED_AT.set(Some(Instant::now()));
            match target {
                Some((_, Drop::Into(location))) => move_there(&pressed.dragged, &location),
                Some((_, Drop::Group(target))) => group_with(&pressed.dragged, &target),
                None => {}
            }
        }
    }
}

/// Stops watching (the browser closed or shows something else): a drag is dropped nowhere.
fn stop() {
    // The timer stops as the watch is dropped, after this.
    let Some(mut watch) = WATCH.with_borrow_mut(Option::take) else {
        return;
    };
    if let Some(pressed) = watch.gesture.cancel() {
        end_drag(
            &pressed,
            watch.hovered.take().map(|(spot, _)| spot),
            watch.ghost.take(),
        );
    }
    if let Some(page) = watch.page.take() {
        unhighlight_page(&page);
    }
}

/// Finds the row showing what's dragged on the page shown now (called once a page is shown), and
/// fades it instead of the one faded before.
pub fn page_shown() {
    let pressed = WATCH.with_borrow(|watch| watch.as_ref()?.gesture.dragging().cloned());
    let (Some(pressed), Some(hits), Some(browsers)) = (pressed, HITS.get(), BROWSERS.get()) else {
        return;
    };
    let Some((_, list)) = context() else {
        return;
    };
    // SAFETY: the live list's rows, on the game thread.
    let shown = unsafe {
        browsers
            .entry_widgets(list)
            .into_iter()
            .find(|&row| hits.is_visible(row) && dragged_of(row).as_ref() == Some(&pressed.dragged))
    };
    // SAFETY: live row widgets (the one before checked), on the game thread.
    unsafe {
        if let Some(before) = pressed.shown.as_ref().and_then(WeakObject::get_live) {
            hits.set_opacity(before, 1.0);
        }
        if let Some(row) = shown {
            hits.set_opacity(row, DRAGGED_OPACITY);
        }
    }
    // SAFETY: live.
    let shown = shown.map(|row| unsafe { WeakObject::new(row) });
    WATCH.with_borrow_mut(|watch| {
        if let Some(dragging) = watch
            .as_mut()
            .and_then(|watch| watch.gesture.dragging_mut())
        {
            dragging.shown = shown;
        }
    });
}

/// What a press at `point` landed on, if it's draggable: a vehicle or folder row of the list.
///
/// # Safety
/// `list` must be the live list. Game thread only.
unsafe fn pressed_on(list: *mut UObjectBase, point: (i32, i32)) -> Option<Pressed> {
    // SAFETY: per the contract.
    let widget = unsafe { row_at(list, point)? };
    // SAFETY: a live row widget.
    let dragged = unsafe { dragged_of(widget)? };
    // SAFETY: live.
    let widget = unsafe { WeakObject::new(widget) };
    Some(Pressed {
        dragged,
        shown: Some(widget),
        widget,
    })
}

/// What dragging the row widget `row` drags, if it can be: a vehicle or a folder tile.
///
/// # Safety
/// `row` must be a live row widget. Game thread only.
unsafe fn dragged_of(row: *mut UObjectBase) -> Option<Dragged> {
    // SAFETY: per the contract; its row is its own.
    let id = unsafe { super::row_key(ITEMS.get()?.row_of(row)?) };
    match Tile::of_id(&id) {
        Some(Tile::Folder(path)) => Some(Dragged::Folder(path)),
        Some(_) => None,
        None if id.is_empty() => None,
        None => Some(Dragged::Vehicle(id)),
    }
}

/// The row widget shown at `point`, if any: inside the entries' area (rows scrolled out of it are
/// still laid out there), visible.
///
/// # Safety
/// `list` must be the live list. Game thread only.
unsafe fn row_at(list: *mut UObjectBase, point: (i32, i32)) -> Option<*mut UObjectBase> {
    let (hits, browsers) = (HITS.get()?, BROWSERS.get()?);
    // SAFETY: per the contract; the list's rows are live widgets.
    unsafe {
        let area = bar::entries_area(list)?;
        if !hits.contains(area, point) {
            return None;
        }
        browsers
            .entry_widgets(list)
            .into_iter()
            .find(|&row| hits.is_visible(row) && hits.contains(row, point))
    }
}

/// The drop target at `point` for `pressed`, and what a drop there does: a folder tile (not the
/// one dragged), "..", Uncategorized, or one of the bar's path parts files it there; another
/// vehicle, for a vehicle and if `can_group`, makes a folder of both.
///
/// # Safety
/// `list` must be the live list. Game thread only.
unsafe fn target_at(
    list: *mut UObjectBase,
    point: (i32, i32),
    pressed: &Pressed,
    can_group: bool,
) -> Option<(Spot, Drop)> {
    let (hits, items) = (HITS.get()?, ITEMS.get()?);
    // SAFETY: per the contract; rows and buttons are live widgets.
    unsafe {
        if let Some(row) = row_at(list, point) {
            if pressed.shown.as_ref().is_some_and(|shown| shown.is(row)) {
                return None;
            }
            let id = super::row_key(items.row_of(row)?);
            let here = with_state(|state| state.here().clone());
            let location = match Tile::of_id(&id) {
                Some(Tile::Folder(path)) => Location::Folder(path),
                Some(Tile::Uncategorized) => Location::Uncategorized,
                Some(Tile::Up) => here.parent()?,
                None => {
                    let groups = can_group
                        && !id.is_empty()
                        && matches!(pressed.dragged, Dragged::Vehicle(_));
                    return groups.then_some((Spot::Tile(row), Drop::Group(id)));
                }
            };
            return Some((Spot::Tile(row), Drop::Into(location)));
        }
        bar::drop_targets()
            .into_iter()
            .find(|&(button, _)| hits.contains(button, point))
            .map(|(button, location)| (Spot::Button(button), Drop::Into(location)))
    }
}

/// Highlights the page button at `point` that can turn the page, if `looks` (the cursor isn't over
/// a drop target), and turns `list`'s page with it when it's time; the one before as usual again.
///
/// # Safety
/// `list` must be the live list. Game thread only.
unsafe fn hover_page(list: *mut UObjectBase, point: (i32, i32), looks: bool) {
    let (Some(pages), Some(hits)) = (PAGES.get(), HITS.get()) else {
        return;
    };
    // SAFETY: per the contract; the buttons are live widgets of the list's selector.
    let over = unsafe {
        if looks {
            pages.buttons(list)
        } else {
            Vec::new()
        }
        .into_iter()
        .find(|&(button, _)| {
            hits.is_visible(button) && pages.is_enabled(button) && hits.contains(button, point)
        })
    };
    let now = u64::try_from(EPOCH.with(Instant::elapsed).as_millis()).unwrap_or(u64::MAX);
    // The spot left (`Some`) if the button under the cursor changed, and the page turn due.
    let (left, turn) = WATCH
        .with_borrow_mut(|watch| {
            let watch = watch.as_mut()?;
            let turn = watch.turner.update(over.map(|(_, turn)| turn), now);
            let same = match (&watch.page, over) {
                (Some(page), Some((button, _))) => page.button.is(button),
                (None, None) => true,
                _ => false,
            };
            Some(((!same).then(|| watch.page.take()), turn))
        })
        .unwrap_or_default();
    if let Some(left) = left {
        if let Some(page) = left {
            unhighlight_page(&page);
        }
        if let Some((button, _)) = over {
            // SAFETY: a live button of the selector, on the game thread.
            let page = unsafe {
                let style = pages.style(button);
                pages.show(button, None);
                PageSpot {
                    button: WeakObject::new(button),
                    style,
                }
            };
            WATCH.with_borrow_mut(|watch| {
                if let Some(watch) = watch {
                    watch.page = Some(page);
                }
            });
        }
    }
    if let Some(turn) = turn {
        // SAFETY: per the contract.
        unsafe { pages.turn(list, turn) };
    }
}

/// Gives the page button of `page` its own look back, if it's still there.
fn unhighlight_page(page: &PageSpot) {
    if let (Some(pages), Some(button), Some(style)) =
        (PAGES.get(), page.button.get_live(), page.style)
    {
        // SAFETY: a live widget, on the game thread.
        unsafe { pages.show(button, Some(style)) };
    }
}

/// Highlights `target` (the previous one as usual again).
///
/// # Safety
/// Game thread only.
unsafe fn hover(target: Option<(Spot, Drop)>) {
    let previous = WATCH.with_borrow_mut(|watch| {
        let watch = watch.as_mut()?;
        let same =
            watch.hovered.as_ref().map(|(spot, _)| *spot) == target.as_ref().map(|(spot, _)| *spot);
        (!same).then(|| core::mem::replace(&mut watch.hovered, target.clone()))
    });
    let Some(previous) = previous else {
        return;
    };
    // SAFETY: per the contract.
    unsafe {
        if let Some((spot, _)) = previous {
            show_spot(spot, false);
        }
        if let Some((spot, _)) = target {
            show_spot(spot, true);
        }
    }
}

/// Shows `spot` highlighted as a drop target, or as usual (a selected tile or vehicle stays
/// selected).
///
/// # Safety
/// Game thread only.
unsafe fn show_spot(spot: Spot, highlighted: bool) {
    let Some(items) = ITEMS.get() else {
        return;
    };
    // SAFETY: on the game thread. A spot holds a plain pointer, so it's only used if the widget is
    // still among the list's rows or the bar's buttons (live objects) now.
    unsafe {
        match spot {
            Spot::Tile(row) => {
                let (Some((browser, list)), Some(browsers)) = (context(), BROWSERS.get()) else {
                    return;
                };
                if browsers.entry_widgets(list).contains(&row) {
                    let selected =
                        tiles::is_selected(row) || browsers.selected_entry(browser) == Some(row);
                    items.show_selected(row, highlighted || selected);
                }
            }
            Spot::Button(button) => {
                if bar::drop_targets()
                    .iter()
                    .any(|&(known, _)| known == button)
                {
                    bar::show_target(button, highlighted);
                }
            }
        }
    }
}

/// Puts the dragged row and the highlighted target (`hovered`, already forgotten) back as usual,
/// and takes the copy following the cursor away.
fn end_drag(pressed: &Pressed, hovered: Option<Spot>, ghost: Option<Ghost>) {
    // SAFETY: on the game thread; the row checked live.
    unsafe {
        if let (Some(ghost), Some(ghosts)) = (ghost, GHOSTS.get()) {
            ghosts.remove(ghost);
        }
        if let Some(hits) = HITS.get() {
            if let Some(widget) = pressed.widget.get_live() {
                hits.set_opacity(widget, 1.0);
                hits.show_grab(widget, false);
            }
            if let Some(shown) = pressed.shown.as_ref().and_then(WeakObject::get_live) {
                hits.set_opacity(shown, 1.0);
            }
        }
        if let Some(spot) = hovered {
            show_spot(spot, false);
        }
    }
}

/// Moves `dragged` onto `target`, saves, and shows the list again.
fn move_there(dragged: &Dragged, target: &Location) {
    let outcome = with_state(|state| drop_onto(state.tree_mut(), dragged, target));
    match outcome {
        Ok(true) => {
            tiles::unselect();
            save();
            tiles::show_next_frame(Refresh::SamePage);
        }
        Ok(false) => {}
        Err(reason) => log!("vehicle folders: not moved: {reason}"),
    }
}

/// Puts the dragged vehicle and the vehicle `target` into a new folder (where `target` is),
/// selects it to be named, saves, and shows the list again.
fn group_with(dragged: &Dragged, target: &str) {
    let Dragged::Vehicle(id) = dragged else {
        return;
    };
    let made = with_state(|state| {
        let made = group_into_new_folder(state.tree_mut(), id, target);
        if let Ok(folder) = &made {
            buttons::stamp_created(state.tree_mut(), folder);
        }
        made
    });
    match made {
        Ok(folder) => {
            tiles::select_later(folder);
            save();
            tiles::show_next_frame(Refresh::SamePage);
        }
        Err(reason) => log!("vehicle folders: no folder made: {reason}"),
    }
}
