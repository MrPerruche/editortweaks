//! The editor's selection outline (`game::outline`) in colors of our own: bricks outlined like
//! selected ones, following their shape, in a color a feature picks. Shared by the features that
//! show bricks this way (the shot preview's hit bricks, coop's other players' selections).
//!
//! The outline material (`ABrickEditor::OutlineMID`) colors each brick by the case in its stencil
//! value. Cases 1-3 (`ValidColor`, `InvalidColor`, `NeutralColor`: the validity colors) only show
//! while moving bricks or picking a property, so in the editor's default mode they're free: a
//! feature gives its bricks those cases, and the material those parameters in its colors. Cases 1
//! and 2 are drawn thicker than case 3 (see `docs/re/selection.md`). As soon as the game uses those
//! cases itself (or outside of the default mode, which the caller checks) the game gets its looks
//! and colors back.
//!
//! One feature at a time holds the cases ([`Highlights`]' rank decides: a higher one takes them
//! over at its next frame, the other gives them back meanwhile).

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;

use crate::game::kismet;
use crate::game::outline::{self, Look, Outlines};
use crate::game::reflection::{self, Parameters, put, take};
use crate::game::textures::{Call, LinearColor};
use crate::game::weak_object::WeakObject;

/// How many free cases there are.
pub const CASES: usize = 3;
/// The free cases (`Look::case`) and their color parameters, by index.
const PARAMETERS: [(i32, &str); CASES] =
    [(1, "ValidColor"), (2, "InvalidColor"), (3, "NeutralColor")];

/// What outlining in our colors needs from the game (after engine init).
pub struct OutlineCases {
    outlines: Outlines,
    /// `ABrickEditor::OutlineMID`'s offset.
    mid: usize,
    /// `UMaterialInstanceDynamic::SetVectorParameterValue(FName, FLinearColor)` and
    /// `UMaterialInstanceDynamic::K2_GetVectorParameterValue(FName) -> FLinearColor`.
    set_color: Call<2>,
    get_color: Call<2>,
    /// [`PARAMETERS`]' names.
    names: [FName; CASES],
}

// SAFETY: reflection records and game code living as long as the game; only used on the game
// thread.
unsafe impl Send for OutlineCases {}
unsafe impl Sync for OutlineCases {}

pub fn find() -> Result<OutlineCases, String> {
    let editor = reflection::find_class("BrickEditor").ok_or("class BrickEditor not found")?;
    let mut names = [FName::default(); CASES];
    for (name, (_, parameter)) in names.iter_mut().zip(PARAMETERS) {
        *name = kismet::string_to_name(parameter)?;
    }
    let color = ("Value", "StructProperty", size_of::<LinearColor>() as i32);
    let returned = (
        "ReturnValue",
        "StructProperty",
        size_of::<LinearColor>() as i32,
    );
    let name = ("ParameterName", "NameProperty", size_of::<FName>() as i32);
    Ok(OutlineCases {
        outlines: outline::find()?,
        mid: reflection::field_offset(editor, "OutlineMID", "ObjectProperty", 8)?,
        set_color: Call::find(
            "MaterialInstanceDynamic",
            "SetVectorParameterValue",
            [name, color],
        )?,
        get_color: Call::find(
            "MaterialInstanceDynamic",
            "K2_GetVectorParameterValue",
            [name, returned],
        )?,
        names,
    })
}

impl OutlineCases {
    /// The outline itself, e.g. for [`Outlines::component_of`].
    pub fn outlines(&self) -> &Outlines {
        &self.outlines
    }

    /// The color of the case at `index` in `mid`.
    ///
    /// # Safety
    /// `mid` must be a live dynamic material instance. Game thread only.
    unsafe fn color(&self, mid: *mut UObjectBase, index: usize) -> LinearColor {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the parameters are an `FName` and an `FLinearColor`.
        unsafe {
            put(
                &mut parameters,
                self.get_color.parameters[0],
                self.names[index],
            );
            self.get_color.function.call(mid, &mut parameters);
            take(&parameters, self.get_color.parameters[1])
        }
    }

    /// Gives the case at `index` in `mid` the color `color`.
    ///
    /// # Safety
    /// As for [`Self::color`].
    unsafe fn set_color(&self, mid: *mut UObjectBase, index: usize, color: LinearColor) {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the parameters are an `FName` and an `FLinearColor`.
        unsafe {
            put(
                &mut parameters,
                self.set_color.parameters[0],
                self.names[index],
            );
            put(&mut parameters, self.set_color.parameters[1], color);
            self.set_color.function.call(mid, &mut parameters);
        }
    }
}

/// Who holds the free cases: its name and rank, and the rank of a higher one waiting for them.
struct Holder {
    owner: Option<(&'static str, u8)>,
    waiting: Option<u8>,
}

static HOLDER: Mutex<Holder> = Mutex::new(Holder {
    owner: None,
    waiting: None,
});

/// Takes or keeps the free cases for `owner` (of `rank`); `false` if another holds them (or a
/// higher one waits for them: `owner` gives them back then).
fn claim(owner: &'static str, rank: u8) -> bool {
    let mut holder = HOLDER.lock().unwrap_or_else(PoisonError::into_inner);
    match holder.owner {
        None => {
            holder.owner = Some((owner, rank));
            holder.waiting = None;
            true
        }
        Some((holding, _)) if holding == owner => holder.waiting.is_none_or(|waits| waits <= rank),
        Some((_, held)) => {
            if rank > held {
                holder.waiting = Some(holder.waiting.map_or(rank, |waits| waits.max(rank)));
            }
            false
        }
    }
}

/// Gives the free cases back, if `owner` holds them.
fn release(owner: &'static str) {
    let mut holder = HOLDER.lock().unwrap_or_else(PoisonError::into_inner);
    if holder.owner.is_some_and(|(holding, _)| holding == owner) {
        holder.owner = None;
    }
}

/// A component outlined in our colors: the look the game gave it, and ours.
struct Restyled {
    component: WeakObject,
    game: Look,
    shown: Look,
}

/// One feature's bricks outlined in its colors, and what to give the game back.
pub struct Highlights {
    owner: &'static str,
    rank: u8,
    /// By component address.
    restyled: HashMap<usize, Restyled>,
    /// The outline material, and per case the game's color and ours while ours shows.
    mid: Option<WeakObject>,
    colors: [Option<(LinearColor, LinearColor)>; CASES],
}

// SAFETY: weak pointers to game objects, only used on the game thread.
unsafe impl Send for Highlights {}

impl Highlights {
    /// `owner` (a feature's name) outlining in the free cases, taking them over from features of a
    /// lower `rank`.
    pub fn new(owner: &'static str, rank: u8) -> Self {
        Self {
            owner,
            rank,
            restyled: HashMap::new(),
            mid: None,
            colors: [None; CASES],
        }
    }

    /// Another editor shows the vehicle: what showed went with the old one.
    pub fn forget(&mut self) {
        self.restyled.clear();
        self.mid = None;
        self.colors = [None; CASES];
        release(self.owner);
    }

    /// Outlines `wanted`, each a component and the index (below [`CASES`]) of its color in
    /// `colors` (sRGB, 0 to 1: the outline is drawn after tone mapping); a component listed twice
    /// gets its first. Only for the editor's default mode. Returns `false`, showing nothing of
    /// ours, when the cases aren't free: the game uses them for one of these components (picking a
    /// property), or another feature holds them.
    ///
    /// # Safety
    /// `editor` must be the live editor, `wanted` live primitive components. Game thread only.
    pub unsafe fn show(
        &mut self,
        kit: &OutlineCases,
        editor: *mut UObjectBase,
        wanted: &[(*mut UObjectBase, usize)],
        colors: [Option<[f32; 3]>; CASES],
    ) -> bool {
        if wanted.is_empty() || !claim(self.owner, self.rank) {
            // SAFETY: per the contract.
            unsafe { self.hide(kit) };
            return false;
        }
        let mut cases: HashMap<usize, (*mut UObjectBase, usize)> = HashMap::new();
        for &(component, index) in wanted {
            cases
                .entry(component as usize)
                .or_insert((component, index.min(CASES - 1)));
        }
        // SAFETY: per the contract.
        unsafe { self.restore_looks(kit, |address| !cases.contains_key(&address)) };
        // The game's looks first: if it uses these cases itself, nothing changes.
        for (&address, &(component, _)) in &cases {
            // SAFETY: per the contract.
            let now = unsafe { kit.outlines.look(component) };
            let restyled = self.restyled.entry(address).or_insert(Restyled {
                // SAFETY: per the contract.
                component: unsafe { WeakObject::new(component) },
                game: now,
                shown: now,
            });
            if now != restyled.shown {
                // The game changed it meanwhile (hovered, say): that's its look now.
                restyled.game = now;
                restyled.shown = now;
            }
            if (1..=CASES as i32).contains(&restyled.game.case()) {
                // SAFETY: per the contract.
                unsafe { self.hide(kit) };
                return false;
            }
        }
        for (address, (component, index)) in cases {
            let restyled = self
                .restyled
                .get_mut(&address)
                .expect("each wanted component was restyled above");
            restyled.shown = restyled.game.in_case(PARAMETERS[index].0);
            // SAFETY: per the contract.
            unsafe { kit.outlines.set_look(component, restyled.shown) };
        }
        // SAFETY: per the contract.
        unsafe { self.recolor(kit, editor, colors) };
        true
    }

    /// Gives the game back every look and color, and the cases.
    ///
    /// # Safety
    /// Game thread only.
    pub unsafe fn hide(&mut self, kit: &OutlineCases) {
        // SAFETY: per the contract.
        unsafe {
            self.restore_looks(kit, |_| true);
            self.restore_colors(kit);
        }
        release(self.owner);
    }

    /// Gives the material our colors (and the game's back to the cases we don't use).
    ///
    /// # Safety
    /// As for [`Self::show`].
    unsafe fn recolor(
        &mut self,
        kit: &OutlineCases,
        editor: *mut UObjectBase,
        colors: [Option<[f32; 3]>; CASES],
    ) {
        // SAFETY: per the contract; the field's offset comes from reflection.
        let mid = unsafe {
            editor
                .cast::<u8>()
                .add(kit.mid)
                .cast::<*mut UObjectBase>()
                .read()
        };
        if mid.is_null() {
            return;
        }
        if self.mid.as_ref().and_then(WeakObject::get) != Some(mid) {
            // SAFETY: per the contract.
            unsafe { self.restore_colors(kit) };
            // SAFETY: the editor's live material instance.
            self.mid = Some(unsafe { WeakObject::new(mid) });
        }
        for (index, color) in colors.into_iter().enumerate() {
            // SAFETY: the editor's live material instance.
            let now = unsafe { kit.color(mid, index) };
            match color {
                Some([red, green, blue]) => {
                    let ours = [red, green, blue, 1.0];
                    let game = match self.colors[index] {
                        // The game set its own again (its UI style changed): keep that one.
                        Some((game, shown)) if shown == now => game,
                        _ => now,
                    };
                    if now != ours {
                        // SAFETY: as above.
                        unsafe { kit.set_color(mid, index, ours) };
                    }
                    self.colors[index] = Some((game, ours));
                }
                None => {
                    if let Some((game, shown)) = self.colors[index].take()
                        && shown == now
                    {
                        // SAFETY: as above.
                        unsafe { kit.set_color(mid, index, game) };
                    }
                }
            }
        }
    }

    /// Gives the restyled components `which` (by address) the game's look back, if ours still
    /// shows.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn restore_looks(&mut self, kit: &OutlineCases, which: impl Fn(usize) -> bool) {
        self.restyled.retain(|&address, restyled| {
            if !which(address) {
                return true;
            }
            if let Some(component) = restyled.component.get() {
                // SAFETY: per the contract; `get` checked it's alive.
                unsafe {
                    if kit.outlines.look(component) == restyled.shown {
                        kit.outlines.set_look(component, restyled.game);
                    }
                }
            }
            false
        });
    }

    /// Gives the material the game's colors back, where ours still show.
    ///
    /// # Safety
    /// Game thread only.
    unsafe fn restore_colors(&mut self, kit: &OutlineCases) {
        let mid = self.mid.as_ref().and_then(WeakObject::get);
        for index in 0..CASES {
            if let (Some((game, ours)), Some(mid)) = (self.colors[index].take(), mid) {
                // SAFETY: per the contract; `get` checked it's alive.
                unsafe {
                    if kit.color(mid, index) == ours {
                        kit.set_color(mid, index, game);
                    }
                }
            }
        }
    }
}
