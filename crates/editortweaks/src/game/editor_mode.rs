//! The vehicle editor's modes (`UBrickEditorMode` subclasses, one current at a time,
//! `ABrickEditor::CurrentEditorMode`): the default one (editing: selecting, the panel), moving
//! bricks (`UBrickEditorMoveMode`: G, dragging a brick, or a gizmo axis), placing new ones, and so
//! on. Which mode is current is reflected; how a move mode moves isn't (see
//! [`SHOULD_CAPTURE_MOUSE`]). Found through BRMK 1.11.6's exports, then in game 1.11.7 (build
//! 25831956); see `docs/re/editor-input.md`.

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::framework::sig::{self, CodeShape};
use crate::game::reflection;

/// `virtual bool UBrickEditorMoveMode::ShouldCaptureMouse() const` (BRMK 0xEAD40, game RVA
/// 0xD08E20): whether the mode moves bricks by a gizmo axis, its first own field, an
/// `FBrickEditorGizmoAxis` (not reflected): the axis (4 bytes, `EAxis`: 1 X, 2 Y, 3 Z) and how
/// bricks move by it (1 byte, see [`FREE_MOVE`]), 0 for none. Without one, the mouse is free and
/// the gizmo shows: G during a G move switches between the two (`HandleStartMoveOrSetGizmoAxis`,
/// BRMK 0x23F010: {Z, 1} and {Z, 0}).
///
/// ```text
/// return GizmoAxis.Axis != 0 && GizmoAxis.Type != 0;
/// ```
///
/// (`IsInPrecisionMoveMode`, BRMK 0xBC650, is the opposite check.) A leaf function: found among the
/// move mode's virtual functions, for the two offsets.
pub const SHOULD_CAPTURE_MOUSE: CodeShape = CodeShape {
    name: "UBrickEditorMoveMode::ShouldCaptureMouse",
    // The axis's offset at +2, the type's at +8.
    shape: "83 79 ?? 00 74 09 80 79 ?? 00 74 03 b0 01 c3 32 c0 c3",
};

/// The gizmo axis type of moves following the mouse in the plane across the axis, where Shift
/// (`MovePerpendicular`) moves bricks along the axis instead: G (`Action_MoveSelection` starts a
/// move by {Z, 1}) and dragging a brick. `UBrickEditorMoveMode::TickMode` (BRMK 0xF19A0) reads the
/// perpendicular flag only for these; the gizmo's handles use the other types (3: rotating around
/// the axis).
const FREE_MOVE: u8 = 1;

/// How an editor moves bricks (see [`EditorModes::moving`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Moving {
    /// Following the mouse in a plane (G, or dragging a brick): Shift moves them across it.
    Freely,
    /// By a gizmo handle (along or around an axis).
    ByGizmo,
    /// Not moving them for now: the mouse is free and the gizmo shows, to grab one of its handles
    /// (the game's precision move mode; a click elsewhere ends the move).
    Precisely,
    /// In a move mode, but how wasn't found (see [`SHOULD_CAPTURE_MOUSE`]).
    Somehow,
}

/// How many entries of the move mode's vtable to look through for [`SHOULD_CAPTURE_MOUSE`]
/// (`UObject` alone has about 80 virtual functions).
const MAX_VIRTUALS: usize = 400;

/// How many outers to go through to find an object's editor (an input component's is its own).
const MAX_OUTERS: usize = 8;

/// The editor's modes (after engine init).
pub struct EditorModes {
    /// `BrickEditor`.
    editor: *const UStruct,
    /// Offset of `ABrickEditor::CurrentEditorMode`.
    current: usize,
    /// `BrickEditorDefaultMode`: editing, not moving bricks or placing new ones.
    default_mode: *const UStruct,
    /// `BrickEditorMoveMode`.
    move_mode: *const UStruct,
    /// Where move modes keep their gizmo axis, if found.
    gizmo_axis: Option<GizmoAxis>,
}

/// Offsets of a move mode's `FBrickEditorGizmoAxis` fields (see [`SHOULD_CAPTURE_MOUSE`]).
#[derive(Clone, Copy, PartialEq, Eq)]
struct GizmoAxis {
    axis: usize,
    kind: usize,
}

// SAFETY: reflection records and offsets living as long as the game; only used on the game thread.
unsafe impl Send for EditorModes {}
unsafe impl Sync for EditorModes {}

pub fn find() -> Result<EditorModes, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let editor = class("BrickEditor")?;
    let move_mode = class("BrickEditorMoveMode")?;
    Ok(EditorModes {
        editor: editor.cast(),
        current: reflection::field_offset(editor, "CurrentEditorMode", "ObjectProperty", 8)?,
        default_mode: class("BrickEditorDefaultMode")?.cast(),
        move_mode: move_mode.cast(),
        // SAFETY: a live class.
        gizmo_axis: unsafe { gizmo_axis(move_mode) },
    })
}

/// Where move modes keep their gizmo axis: read from their `ShouldCaptureMouse`, found among the
/// virtual functions of the class's default object. `None` if it isn't found, or found with
/// different offsets.
///
/// # Safety
/// `class` must be the live class `BrickEditorMoveMode`.
unsafe fn gizmo_axis(class: *const UClass) -> Option<GizmoAxis> {
    // SAFETY: per the contract. The vtable is read for [`MAX_VIRTUALS`] entries at most, up to the
    // first that isn't game code. The game has no RTTI, so vtables follow one another in its
    // constant data: past the move mode's own, the scan reads other classes' (still mapped), which
    // only matters if one has the shape with other offsets (then nothing is found). The bytes read
    // from a function lie in its checked shape.
    unsafe {
        let defaults = reflection::default_object(class)?;
        let object_size = usize::try_from((*class).ustruct.properties_size).ok()?;
        let vtable = (*defaults).vtable.cast::<*const u8>().cast_const();
        let mut found = None;
        for index in 0..MAX_VIRTUALS {
            let function = vtable.add(index).read();
            if !sig::is_code(function) {
                break;
            }
            if !SHOULD_CAPTURE_MOUSE.matches(function) {
                continue;
            }
            let offsets = GizmoAxis {
                axis: usize::from(function.add(2).read()),
                kind: usize::from(function.add(8).read()),
            };
            if found.is_some_and(|known| known != offsets) {
                return None;
            }
            found = Some(offsets);
        }
        found.filter(|offsets| offsets.axis + 4 <= object_size && offsets.kind < object_size)
    }
}

impl EditorModes {
    /// Whether the gizmo axis of move modes was found (see [`Self::moving`]).
    pub fn knows_gizmo_axis(&self) -> bool {
        self.gizmo_axis.is_some()
    }

    /// The editor `object` is part of (itself or an outer), e.g. an editor input component's.
    ///
    /// # Safety
    /// `object` must be a live object.
    pub unsafe fn editor_of(&self, object: *const UObjectBase) -> Option<*mut UObjectBase> {
        let mut current = object;
        for _ in 0..MAX_OUTERS {
            if current.is_null() {
                return None;
            }
            // SAFETY: per the contract; an object's outers are live objects (or null).
            unsafe {
                if self.is_a(current, self.editor) {
                    return Some(current.cast_mut());
                }
                current = (*current).outer_private;
            }
        }
        None
    }

    /// Whether `editor` is in its default mode: editing, not moving bricks or placing new ones.
    ///
    /// # Safety
    /// `editor` must be a live `ABrickEditor`.
    pub unsafe fn is_editing(&self, editor: *const UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe {
            let mode = self.current(editor);
            !mode.is_null() && self.is_a(mode, self.default_mode)
        }
    }

    /// How `editor` moves bricks, if it's in a move mode.
    ///
    /// # Safety
    /// `editor` must be a live `ABrickEditor`.
    pub unsafe fn moving(&self, editor: *const UObjectBase) -> Option<Moving> {
        // SAFETY: per the contract; the offsets were checked to lie within move modes.
        unsafe {
            let mode = self.current(editor);
            if mode.is_null() || !self.is_a(mode, self.move_mode) {
                return None;
            }
            let Some(offsets) = self.gizmo_axis else {
                return Some(Moving::Somehow);
            };
            let mode = mode.cast::<u8>();
            let axis = mode.add(offsets.axis).cast::<u32>().read_unaligned();
            let kind = mode.add(offsets.kind).read();
            Some(if axis == 0 || kind == 0 {
                Moving::Precisely
            } else if kind == FREE_MOVE {
                Moving::Freely
            } else {
                Moving::ByGizmo
            })
        }
    }

    /// `editor`'s current mode if it's a move mode.
    ///
    /// # Safety
    /// `editor` must be a live `ABrickEditor`.
    pub unsafe fn move_mode(&self, editor: *const UObjectBase) -> Option<*mut UObjectBase> {
        // SAFETY: per the contract.
        unsafe {
            let mode = self.current(editor);
            (!mode.is_null() && self.is_a(mode, self.move_mode)).then_some(mode)
        }
    }

    /// The editor's current mode, or null.
    ///
    /// # Safety
    /// `editor` must be a live `ABrickEditor`.
    unsafe fn current(&self, editor: *const UObjectBase) -> *mut UObjectBase {
        // SAFETY: per the contract; the offset comes from reflection.
        unsafe {
            editor
                .cast::<u8>()
                .add(self.current)
                .cast::<*mut UObjectBase>()
                .read()
        }
    }

    /// Whether `object` is of `class` or a subclass.
    ///
    /// # Safety
    /// `object` must be a live object.
    unsafe fn is_a(&self, object: *const UObjectBase, class: *const UStruct) -> bool {
        // SAFETY: per the contract.
        unsafe { reflection::with_parents(reflection::class_of(object.cast())) }
            .any(|parent| parent == class)
    }
}
