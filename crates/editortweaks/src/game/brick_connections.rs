//! Which bricks hold together, and how strongly: the connections a vehicle gets when it spawns, as
//! the editor finds them, and the strength the game gives each. Found through BRMK 1.11.6's exports
//! and stat names, then in game 1.11.7 (build 25831956); see `docs/re/brick-strength.md`.
//!
//! A spawned vehicle's bricks hold together by `UBrickConnection`s, one per pair of bricks touching
//! through connectors (studs and the like). The editor has none, but its "select attached" (L)
//! finds the same pairs with the game's connection finder ([`FINDER`]): [`Connections::find_all`]
//! runs it ([`ATTACHED_OBJECTS_CALL`]) and records every pair the finder reports. A pair's strength
//! is the game's own computation ([`STRENGTH`]), run on a stand-in connection.

use core::cell::RefCell;
use core::ffi::c_void;
use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::tarray::TArray;

use crate::framework::hook;
use crate::framework::sig::{self, FunctionSig, SigMatch};
use crate::framework::timings;

/// `bool FBrickConnectionGenerator::ForEachConnection(const FParams&, TFunctionRef<void(const
/// FFoundConnection&)>)` (the name from BRMK 0x42D410's stat, the parameter types ours; game RVA
/// 0xF1C550): calls back once per pair of connectors that connect two
/// bricks of the generator (built from a list of bricks just before). Its callers: a spawning
/// vehicle (creating a `UBrickConnection` per call, `ABrickVehicle::CreateBrickConnection`, game
/// 0xF19A40), and the editor's `GetAttachedObjects`, `GetMoveSnappingOffset` and
/// `UpdateMoveValidityStates`. Bricks of the same weld group aren't reported (unless the generator
/// asks for them, the flag at generator +0x1C6, which the editor's doesn't): they're one body.
pub const FINDER: FunctionSig = FunctionSig {
    name: "FBrickConnectionGenerator::ForEachConnection",
    // The weld group check before the callback: the brick's weld group (its offset at +3) is
    // nonzero and equals the other brick's (offset at +12), then the connector pair's params.
    pattern: "0f b7 40 ?? 66 85 c0 74 ?? 66 3b 46 ?? 74 ?? 48 8b 55 ?? 33 c0",
};

/// Where [`FINDER`]'s match holds the offset of a brick's weld group (`uint16`, 0 for none).
const FINDER_WELD_GROUP: usize = 3;
const FINDER_OTHER_WELD_GROUP: usize = 12;

/// `UBrickEditorDefaultMode::SelectAttachedObjects(bool bRecursive)` (BRMK 0xE3EC0, game RVA
/// 0xD046F0): L and Ctrl + L. Selects what the editor's `GetAttachedObjects` returns:
///
/// ```text
/// TArray<UBrickEditorObject*> Attached;
/// Editor->GetAttachedObjects(Attached, bRecursive);   // virtual: game +0x658, BRMK +0x7B0
/// Editor->SelectObjects(Attached, ...);
/// ```
///
/// `AVehicleEditor::GetAttachedObjects(TArray<UBrickEditorObject*>& Out, bool bRecursive) const`
/// (BRMK 0xA52A0, game 0xCDD480) builds a connection generator from every brick not hidden, runs
/// [`FINDER`] once over it (merging connected bricks), then collects what's attached to the
/// selection: also with nothing selected, it finds every connection.
pub const ATTACHED_OBJECTS_CALL: FunctionSig = FunctionSig {
    name: "UBrickEditorDefaultMode::SelectAttachedObjects",
    // The editor (offset at +3), `bRecursive`, the empty array, then the virtual call (its offset
    // at +28) and the selection.
    pattern: "48 8b 49 ?? 44 0f b6 c2 48 89 44 24 ?? 48 8d 54 24 ?? 48 89 44 24 ?? 48 8b 01 ff 90 ?? ?? ?? ?? 48 8b 4b ?? 48 8d 54 24 ?? 45 33 c9",
};

/// Where [`ATTACHED_OBJECTS_CALL`]'s match holds `GetAttachedObjects`' vtable offset.
const ATTACHED_OBJECTS_SLOT: usize = 28;

/// Our name for the function computing a connection's strength (BRMK 0x45A890, game RVA 0xF3F450),
/// called once the connection is created or grows (`CreateBrickConnection`):
///
/// ```text
/// Strength = min(A->GetBrickPhysMaterial()->Strength, B->...->Strength)   // 1 without a material
///     * KindFactor(Kind)                  // a constant per connector kind (1 to 27), else one more
///     * max(Points, 9) * Scale;           // Scale: a global
/// ```
///
/// `GetBrickPhysMaterial(false)` (virtual) is the brick's `BrickMaterial->PhysicalMaterial`: plain
/// data, so it works on editor bricks too. A brick takes no damage until the damage it gets is at
/// least the sum of its weld group's connection strengths; then those connections break and the
/// rest of the damage goes on to the bricks it was connected to (`UBrick::ReceiveDamage`, BRMK
/// 0xC87E0).
pub const STRENGTH: FunctionSig = FunctionSig {
    name: "UBrickConnection strength",
    // Both bricks' physical materials: the bricks' offsets at +8 and +21, `GetBrickPhysMaterial`'s
    // vtable offset at +14.
    pattern: "48 8b f9 33 d2 48 8b 49 ?? 48 8b 01 ff 90 ?? ?? ?? ?? 48 8b 4f ?? 48 8b d8 48 8b 11 4c 8b 82 ?? ?? ?? ?? 33 d2 41 ff d0",
};

/// [`STRENGTH`]'s connector kind (offset at +3) and the kinds' factors (a jump table on kinds 1 to 27).
pub const STRENGTH_KIND: FunctionSig = FunctionSig {
    name: "UBrickConnection strength kind",
    pattern: "0f b6 47 ?? f3 0f 5d d0 ff c8 83 f8 1a",
};

/// [`STRENGTH`]'s end: the points (offset at +3), at least 9, times the rest, into the strength
/// (offset at +46).
pub const STRENGTH_POINTS: FunctionSig = FunctionSig {
    name: "UBrickConnection strength points",
    pattern: "0f b7 4f ?? b8 09 00 00 00 48 8b 5c 24 ?? 3b c8 f3 0f 59 d1 0f 43 c1 66 0f 6e c0 0f 5b c0 f3 0f 59 c2 f3 0f 59 05 ?? ?? ?? ?? f3 0f 11 47 ??",
};

type FinderFn = unsafe extern "C" fn(
    generator: *mut c_void,
    params: *const c_void,
    callback: *const FunctionRef,
) -> bool;

type AttachedObjectsFn = unsafe extern "C" fn(
    editor: *const UObjectBase,
    out: *mut TArray<*mut UObjectBase>,
    recursive: bool,
);

type StrengthFn = unsafe extern "C" fn(connection: *mut u8);

/// `UBrickPhysicalMaterial* UBrick::GetBrickPhysMaterial(bool)` (virtual), called with false as
/// [`STRENGTH`] calls it.
type PhysMaterialFn =
    unsafe extern "C" fn(brick: *const UObjectBase, flag: bool) -> *mut UObjectBase;

/// Where [`STRENGTH`]'s match holds `GetBrickPhysMaterial`'s vtable offset: for the first brick,
/// then again for the other.
const STRENGTH_MATERIAL_SLOT: usize = 14;
const STRENGTH_OTHER_MATERIAL_SLOT: usize = 31;

/// `TFunctionRef<void(const FFoundConnection&)>` in the shipping game: the function calling the
/// callable, then the callable.
#[repr(C)]
struct FunctionRef {
    call: unsafe extern "C" fn(callable: *mut c_void, found: *const FoundConnection),
    callable: *mut c_void,
}

/// What [`FINDER`] reports per connector pair (our name), as the vehicle's callback reads it.
#[repr(C)]
#[derive(Clone, Copy)]
struct FoundConnection {
    /// `FBrickConnectionParams`, copied into the connection by `CreateBrickConnection`.
    params: ConnectionParams,
    brick: *mut UObjectBase,
    other: *mut UObjectBase,
}

/// `FBrickConnectionParams` (8 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
struct ConnectionParams {
    /// The connectors' kind.
    kind: u8,
    _pad: u8,
    /// How many points connect, e.g. studs.
    points: u16,
    /// The two bricks' connector indices.
    _connectors: [u16; 2],
}

/// A connection the finder reported: one pair of connectors joining two bricks. A pair of bricks
/// can have several (see [`Connections::find_all`]).
#[derive(Clone, Copy, Debug)]
pub struct Connection {
    pub brick: *mut UObjectBase,
    pub other: *mut UObjectBase,
    /// The connectors' kind: a connection keeps its first kind, and grows only by more of it.
    pub kind: u8,
    pub points: u16,
}

/// The connection finding and strength code (see [`find`]).
pub struct Connections {
    /// Offset of `GetAttachedObjects` in an editor's vtable.
    attached_slot: usize,
    strength: StrengthFn,
    /// Offsets in a `UBrickConnection` (see [`STRENGTH`]).
    layout: ConnectionLayout,
    /// Offset of a brick's weld group.
    weld_group: usize,
    /// Offset of `GetBrickPhysMaterial` in a brick's vtable.
    material_slot: usize,
}

#[derive(Clone, Copy)]
struct ConnectionLayout {
    brick: usize,
    other: usize,
    kind: usize,
    points: usize,
    strength: usize,
}

/// Size of the stand-in connection [`Connections::strength`] passes (`UBrickConnection` is 0x60
/// bytes in game 1.11.7): every offset read from the code must fit in it.
const STAND_IN_SIZE: usize = 0x100;

#[repr(C, align(16))]
struct StandIn([u8; STAND_IN_SIZE]);

// SAFETY: game code and offsets living as long as the game; only used on the game thread.
unsafe impl Send for Connections {}
unsafe impl Sync for Connections {}

static ORIGINAL_FINDER: OnceLock<FinderFn> = OnceLock::new();

thread_local! {
    /// While [`Connections::find_all`] runs on this thread: the connections reported so far.
    static FOUND: RefCell<Option<Vec<Connection>>> = const { RefCell::new(None) };
}

/// Finds it all and hooks [`FINDER`] (once; before or after engine init).
pub fn find() -> Result<&'static Connections, String> {
    static FOUND: OnceLock<Result<Connections, String>> = OnceLock::new();
    FOUND.get_or_init(find_now).as_ref().map_err(Clone::clone)
}

fn find_now() -> Result<Connections, String> {
    let finder = FINDER.locate()?;
    let attached = ATTACHED_OBJECTS_CALL.locate()?;
    let [start, kind, points] =
        [&STRENGTH, &STRENGTH_KIND, &STRENGTH_POINTS].map(FunctionSig::locate);
    let (start, kind, points) = (start?, kind?, points?);
    for (part, name) in [(&kind, STRENGTH_KIND.name), (&points, STRENGTH_POINTS.name)] {
        if part.function != start.function {
            return Err(format!("{name} isn't where expected"));
        }
    }
    // SAFETY: each offset lies inside its matched pattern (see the patterns' comments).
    let (connections, other_weld_group, other_material_slot) = unsafe {
        let byte = |at: &SigMatch, offset| usize::from(at.address.add(offset).read());
        let slot = at_u32(&attached, ATTACHED_OBJECTS_SLOT);
        let layout = ConnectionLayout {
            brick: byte(&start, 8),
            other: byte(&start, 21),
            kind: byte(&kind, 3),
            points: byte(&points, 3),
            strength: byte(&points, 46),
        };
        let connections = Connections {
            attached_slot: slot,
            strength: core::mem::transmute::<*const u8, StrengthFn>(start.function),
            layout,
            weld_group: byte(&finder, FINDER_WELD_GROUP),
            material_slot: at_u32(&start, STRENGTH_MATERIAL_SLOT),
        };
        (
            connections,
            byte(&finder, FINDER_OTHER_WELD_GROUP),
            at_u32(&start, STRENGTH_OTHER_MATERIAL_SLOT),
        )
    };
    if other_material_slot != connections.material_slot {
        return Err(format!(
            "{}: the bricks' materials come from different calls",
            STRENGTH.name
        ));
    }
    let layout = connections.layout;
    let fits = [layout.brick, layout.other, layout.strength]
        .iter()
        .all(|&offset| offset + 8 <= STAND_IN_SIZE)
        && layout.kind < STAND_IN_SIZE
        && layout.points + 2 <= STAND_IN_SIZE;
    if !fits {
        return Err(format!(
            "{}: unexpected UBrickConnection layout",
            STRENGTH.name
        ));
    }
    if other_weld_group != connections.weld_group {
        return Err(format!("{}: unexpected weld group check", FINDER.name));
    }
    for (slot, name) in [
        (connections.attached_slot, ATTACHED_OBJECTS_CALL.name),
        (connections.material_slot, STRENGTH.name),
    ] {
        if !slot.is_multiple_of(8) || slot >= 0x1000 {
            return Err(format!("{name}: unexpected vtable offset {slot:#x}"));
        }
    }
    // SAFETY: the target is the finder's start (the function containing the pattern), and the
    // detour has its signature.
    unsafe {
        hook::install_typed(
            FINDER.name,
            finder.function,
            finder_detour as FinderFn,
            &ORIGINAL_FINDER,
        )?;
    }
    Ok(connections)
}

/// # Safety
/// `offset + 4` must lie inside the matched pattern.
unsafe fn at_u32(at: &SigMatch, offset: usize) -> usize {
    // SAFETY: per the contract.
    unsafe { at.address.add(offset).cast::<u32>().read_unaligned() as usize }
}

impl Connections {
    /// Every connection between `editor`'s bricks that aren't hidden, as a vehicle spawned from
    /// them would have them: one per connector pair, so a pair of bricks may come several times.
    /// Bricks of the same weld group aren't connected (they're one body).
    ///
    /// # Safety
    /// `editor` must be a live vehicle editor (`AVehicleEditor`). Game thread only.
    pub unsafe fn find_all(&self, editor: *const UObjectBase) -> Vec<Connection> {
        // SAFETY: per the contract; the slot comes from the game's own call on the editor.
        let attached = unsafe {
            let vtable = (*editor).vtable.cast::<u8>();
            vtable.add(self.attached_slot).cast::<*const u8>().read()
        };
        if !sig::is_code(attached) {
            return Vec::new();
        }
        FOUND.with_borrow_mut(|found| *found = Some(Vec::new()));
        // SAFETY: a vehicle editor's `GetAttachedObjects` (the slot `SelectAttachedObjects` calls),
        // with an empty array of ours, which it fills with the game's allocator; we free it.
        unsafe {
            let attached: AttachedObjectsFn = core::mem::transmute(attached);
            let mut out = TArray::<*mut UObjectBase>::new();
            attached(editor, &mut out, true);
            if !out.data.is_null() {
                out.free();
            }
        }
        FOUND.with_borrow_mut(Option::take).unwrap_or_default()
    }

    /// The strength the game gives a connection of `kind` with `points` points between `brick` and
    /// `other` (see [`STRENGTH`]): how much damage breaks it.
    ///
    /// # Safety
    /// `brick` and `other` must be live bricks. Game thread only.
    pub unsafe fn strength(
        &self,
        brick: *mut UObjectBase,
        other: *mut UObjectBase,
        kind: u8,
        points: u16,
    ) -> f32 {
        let layout = self.layout;
        let mut connection = StandIn([0; STAND_IN_SIZE]);
        let base = connection.0.as_mut_ptr();
        // SAFETY: every offset fits in the stand-in (checked in `find`), which holds all the
        // function reads (the two bricks, the kind and points) and writes (the strength).
        unsafe {
            base.add(layout.brick)
                .cast::<*mut UObjectBase>()
                .write_unaligned(brick);
            base.add(layout.other)
                .cast::<*mut UObjectBase>()
                .write_unaligned(other);
            base.add(layout.kind).write(kind);
            base.add(layout.points)
                .cast::<u16>()
                .write_unaligned(points);
            (self.strength)(base);
            base.add(layout.strength).cast::<f32>().read_unaligned()
        }
    }

    /// `brick`'s physical material (a `UBrickPhysicalMaterial`: its material's), or null; the one
    /// its connections' strength comes from.
    ///
    /// # Safety
    /// `brick` must be a live brick. Game thread only.
    pub unsafe fn physical_material(&self, brick: *const UObjectBase) -> *mut UObjectBase {
        // SAFETY: per the contract; the slot comes from the game's own call on a brick, which it
        // calls the same way.
        unsafe {
            let vtable = (*brick).vtable.cast::<u8>();
            let function = vtable.add(self.material_slot).cast::<*const u8>().read();
            if !sig::is_code(function) {
                return core::ptr::null_mut();
            }
            core::mem::transmute::<*const u8, PhysMaterialFn>(function)(brick, false)
        }
    }

    /// `brick`'s weld group, 0 for none: welded bricks are one body, breaking off together.
    ///
    /// # Safety
    /// `brick` must be a live brick.
    pub unsafe fn weld_group(&self, brick: *const UObjectBase) -> u16 {
        // SAFETY: per the contract; the offset comes from the game's own check.
        unsafe {
            brick
                .cast::<u8>()
                .add(self.weld_group)
                .cast::<u16>()
                .read_unaligned()
        }
    }
}

/// Runs the finder; while [`Connections::find_all`] runs on this thread, through a callback of ours
/// that records each connection after passing it on.
unsafe extern "C" fn finder_detour(
    generator: *mut c_void,
    params: *const c_void,
    callback: *const FunctionRef,
) -> bool {
    let _span = timings::span("game::brick_connections::finder_detour");
    let original = *ORIGINAL_FINDER.get().expect("hook recorded before use");
    let recording = FOUND.with_borrow(Option::is_some);
    // SAFETY: the game's call, passed on, with its callback or ours wrapping it; ours lives on
    // this stack frame for the whole call.
    unsafe {
        if !recording {
            return timings::game(FINDER.name, || original(generator, params, callback));
        }
        let wrapper = FunctionRef {
            call: record,
            callable: callback.cast_mut().cast(),
        };
        timings::game(FINDER.name, || original(generator, params, &wrapper))
    }
}

/// Our callback (see [`finder_detour`]): `callable` is the game's callback.
unsafe extern "C" fn record(callable: *mut c_void, found: *const FoundConnection) {
    // SAFETY: `callable` is the game's `TFunctionRef`, passed on unchanged; `found` is the
    // finder's, valid during the call.
    let found = unsafe {
        let game = &*callable.cast::<FunctionRef>();
        (game.call)(game.callable, found);
        *found
    };
    FOUND.with_borrow_mut(|recorded| {
        if let Some(recorded) = recorded {
            recorded.push(Connection {
                brick: found.brick,
                other: found.other,
                kind: found.params.kind,
                points: found.params.points,
            });
        }
    });
}
