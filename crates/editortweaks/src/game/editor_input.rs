//! The vehicle editor's input handlers (`UEditorInputComponent`). The input system calls them by
//! name: `UBaseInputComponent::SetupInputComponent` binds each input row (`InputTable`) to the
//! component's function `Action_<Row>` or `Axis_<Row>` (`FindFunctionChecked`, then a delegate
//! calling it through `ProcessEvent`), so hooking their exec thunks intercepts every call. Found
//! through BRMK 1.11.6's exports, then in game 1.11.7 (build 25831956); see
//! `docs/re/editor-input.md`.

use core::ffi::c_void;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fproperty::FProperty;
use brickrust::ue::uclass::UClass;

use crate::framework::sig::FunctionSig;
use crate::game::reflection;

/// `FBrActionResult Axis_MouseMoveRight(const FBrActionParams&)` and `Axis_MouseMoveUp`: the mouse's
/// movement (`Params.AxisValue`: the engine's MouseX or MouseY, see [`AXIS_PER_PIXEL`]), which the
/// engine reports while a mouse button is held over the 3D view. The editor's versions
/// (`HandleStealMouseMove(Params, Axis)` in the game, inlined in BRMK): in a move mode (G or the
/// gizmo), multiply it by the user's `EditorMouseMoveSensitivity` and add it to the mode's input
/// (`UBrickEditorMoveMode::AddMouseMoveInput`), which moves the selection by it on its next tick
/// (`TickMode`); otherwise, or while the camera is moved (right or middle button), pass it to
/// `Axis_ViewYaw`/`Axis_ViewPitch`, which turn the camera. Nothing else adds mouse input to move
/// mode.
pub struct MouseMoveAxes {
    /// `EditorInputComponent`.
    class: *const UClass,
    pub right: Axis,
    pub up: Axis,
    /// Offset of `AxisValue` (a float) in `FBrActionParams`.
    axis_value: usize,
}

/// One of the mouse axis handlers.
pub struct Axis {
    /// The exec thunk.
    pub thunk: *const u8,
    /// Its `const FBrActionParams&` parameter.
    params: *const FProperty,
}

/// How far the mouse axes go per pixel the mouse moves: the engine's default sensitivity of MouseX
/// and MouseY (`BaseInput.ini`), which the game keeps (a one-pixel move reads 0.07). Up is
/// positive.
pub const AXIS_PER_PIXEL: f32 = 0.07;

/// `UEditorInputComponent::HandleStealMouseMove(Params, Axis)` (game RVA 0xDE3CC0; inlined in BRMK's
/// `Axis_MouseMoveRight`/`Up`, 0x2282C0), which both mouse axis handlers call (see
/// [`MouseMoveAxes`]): the movement goes to the move mode unless the camera is moved, a flag of the
/// input component:
///
/// ```text
/// Mode = this->Mode (a weak pointer), if a UBrickEditorMoveMode;
/// if (!(this->CameraFlags & 4) && Mode) { ...AddMouseMoveInput...; return Handled; }
/// return Axis_ViewYaw(...);   // or Pitch: turns the camera
/// ```
///
/// The camera flags (a byte; game +0x200, BRMK +0x210): bit 0 while `MoveCamera` is held (right
/// button), bit 1 while `PivotCamera` is (middle button, Space), bit 2 when either is
/// (`UpdateCameraMode`).
pub const STEAL_MOUSE_MOVE: FunctionSig = FunctionSig {
    name: "UEditorInputComponent::HandleStealMouseMove",
    // The camera flags tested (their offset at +2), the mode checked, `Params.Mode == Real`, then
    // the call getting the user settings.
    pattern: "f6 86 ?? ?? 00 00 04 75 ?? 48 85 ff 74 ?? 80 7d 00 01 75 ?? e8",
};

/// Whether the editor's camera is being moved or pivoted (after load).
pub struct CameraFlags {
    /// Offset of the camera flags in the editor's input component.
    offset: usize,
}

/// Set in [`CameraFlags`] while the camera is pivoted.
const CAMERA_PIVOTING: u8 = 2;
/// Set in [`CameraFlags`] while the camera is moved or pivoted.
const CAMERA_MOVING: u8 = 4;

pub fn camera_flags() -> Result<CameraFlags, String> {
    let found = STEAL_MOUSE_MOVE.locate()?;
    // SAFETY: displacement bytes inside the matched pattern.
    let offset = unsafe { found.address.add(2).cast::<u32>().read_unaligned() };
    Ok(CameraFlags {
        offset: offset as usize,
    })
}

impl CameraFlags {
    /// Whether `input`'s camera is moved or pivoted (the mouse turns it rather than moving bricks).
    ///
    /// # Safety
    /// `input` must be a live editor input component.
    pub unsafe fn moving(&self, input: *const UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { self.flags(input) & CAMERA_MOVING != 0 }
    }

    /// Whether `input`'s camera is pivoted (`PivotCamera` held): turning it turns it around the
    /// selection (`ABrickEditor::UpdateViewRotation`), rather than in place.
    ///
    /// # Safety
    /// As for [`Self::moving`].
    pub unsafe fn pivoting(&self, input: *const UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { self.flags(input) & CAMERA_PIVOTING != 0 }
    }

    /// # Safety
    /// As for [`Self::moving`].
    unsafe fn flags(&self, input: *const UObjectBase) -> u8 {
        // SAFETY: per the contract; the offset comes from the game's own check.
        unsafe { input.cast::<u8>().add(self.offset).read() }
    }
}

// SAFETY: reflection records and game code live as long as the game; only used on the game thread.
unsafe impl Send for MouseMoveAxes {}
unsafe impl Sync for MouseMoveAxes {}

pub fn mouse_move_axes() -> Result<MouseMoveAxes, String> {
    let class = reflection::find_class("EditorInputComponent")
        .ok_or("class EditorInputComponent not found")?;
    let right = axis(class, "Axis_MouseMoveRight")?;
    let up = axis(class, "Axis_MouseMoveUp")?;
    returns_action_result(class, "Axis_MouseMoveRight")?;
    returns_action_result(class, "Axis_MouseMoveUp")?;
    // SAFETY: `axis` checked that the parameter is an FBrActionParams, a reflected struct.
    let axis_value = unsafe {
        reflection::struct_field_offset(
            reflection::struct_type(right.params),
            "AxisValue",
            "FloatProperty",
            4,
        )?
    };
    Ok(MouseMoveAxes {
        class,
        right,
        up,
        axis_value,
    })
}

/// The input handler (axis or action) `name`, whose first parameter is its `FBrActionParams`
/// (`Mode`, `bIsSecondaryAction`, `AxisValue`).
fn axis(class: *const UClass, name: &'static str) -> Result<Axis, String> {
    let function = reflection::find_function(class, name)?;
    let thunk = reflection::native_thunk(function).map_err(|e| format!("{name}: {e}"))?;
    // SAFETY: a live function record; its parameters are linked to it.
    let params = unsafe {
        reflection::properties(function.cast())
            .next()
            .filter(|&params| {
                let ustruct = reflection::struct_type(params);
                !ustruct.is_null()
                    && reflection::name_is(reflection::struct_name(ustruct), "BrActionParams")
            })
            .ok_or(format!("{name}: first parameter isn't an FBrActionParams"))?
    };
    Ok(Axis { thunk, params })
}

/// Checks that the input handler `name` returns an `FBrActionResult` starting with its outcome (a
/// 1-byte enum), which [`set_handled`] writes.
fn returns_action_result(class: *const UClass, name: &'static str) -> Result<(), String> {
    let function = reflection::find_function(class, name)?;
    // SAFETY: a live function record; its return value's struct is a live reflected struct.
    unsafe {
        let result = reflection::find_property(function.cast(), "ReturnValue");
        let returned = if result.is_null() {
            core::ptr::null()
        } else {
            reflection::struct_type(result)
        };
        if returned.is_null()
            || !reflection::name_is(reflection::struct_name(returned), "BrActionResult")
        {
            return Err(format!("{name}: doesn't return an FBrActionResult"));
        }
        let first = reflection::properties(returned).find(|&field| (*field).offset_internal == 0);
        if !first.is_some_and(|field| (*field).element_size == 1) {
            return Err(format!("{name}: FBrActionResult changed its layout"));
        }
    }
    Ok(())
}

/// Marks a call of an input handler's thunk as handled without running the game's handler: sets
/// the outcome in its return value, which the caller made (the thunk only assigns to it).
///
/// # Safety
/// `result` must be the return value pointer the thunk was called with, of a handler checked by
/// [`returns_action_result`].
unsafe fn set_handled(result: *mut c_void) {
    // SAFETY: per the contract; the return value is a constructed FBrActionResult whose first
    // field is its 1-byte outcome.
    unsafe {
        if !result.is_null() {
            result.cast::<u8>().write(HANDLED);
        }
    }
}

/// `FBrActionResult Action_Select(const FBrActionParams&)` (`UBaseEditorInputComponent`, BRMK
/// 0x223B50): the left mouse button in the editor, pressed and released. It calls
/// `HandleSelect(Params, false)` (BRMK 0x23EF50):
///
/// ```text
/// Mode = Editor->CurrentEditorMode;
/// if (!Mode || !Editor->PlayerController) return { Result 2 };
/// if (Params.Mode == 1) {                         // EBrActionMode::Real (not a test)
///     GetMousePosition(X, Y);
///     if (Params.bReleased == 0) Mode->PressObjectAtScreenPosition({X, Y}, false);   // vtable
///     else Mode->ReleaseObjectAtScreenPosition({X, Y}, false);
/// }
/// return { Result 0 };                            // handled
/// ```
///
/// `bReleased` (our name) is the byte at +8 of `FBrActionParams`, after its reflected fields.
pub const SELECT: &str = "Action_Select";

/// `FBrActionResult Action_AddToSelection(const FBrActionParams&)` (`UBaseEditorInputComponent`,
/// BRMK 0x2200B0): Shift + the left mouse button, pressed and released: `HandleSelect(Params, true)`,
/// as [`SELECT`] with `bAdd` (`docs/re/selection.md`).
pub const ADD_TO_SELECTION: &str = "Action_AddToSelection";

/// `FBrActionResult Action_MoveCamera(const FBrActionParams&)` (`UEditorInputComponent`, BRMK
/// 0x222710): the right mouse button in the editor, pressed and released. While it's held, bit 0
/// of the component's camera flags is set (see [`CameraFlags`]), then `UpdateCameraMode`: the mouse
/// turns the camera, WASD moves it, the cursor hides.
pub const MOVE_CAMERA: &str = "Action_MoveCamera";

/// `FBrActionResult Action_PivotCamera(const FBrActionParams&)` (`UEditorInputComponent`, BRMK
/// 0x223110): the middle mouse button (and Space) in the editor, pressed and released. Like
/// [`MOVE_CAMERA`] with bit 1 of the camera flags: while it's held the mouse turns the camera around
/// the selection, and the cursor hides.
pub const PIVOT_CAMERA: &str = "Action_PivotCamera";

/// `FBrActionResult Action_DeleteSelection(const FBrActionParams&)`: deletes the selected bricks
/// (Delete; input row `DeleteSelectedBricks`). The name is in the game's function table (a string at
/// game 0x39904F0).
pub const DELETE_SELECTION: &str = "Action_DeleteSelection";

/// `FBrActionResult Action_DuplicateSelection(const FBrActionParams&)`: duplicates the selected
/// bricks onto the mouse (Ctrl + C; input row `DuplicateSelectedBricks`).
pub const DUPLICATE_SELECTION: &str = "Action_DuplicateSelection";

/// `FBrActionResult Action_HideSelection(const FBrActionParams&)` (`UBaseEditorInputComponent`, BRMK
/// 0x222070): hides the selected bricks until `Action_UnhideAll` (H; input row `HideSelection`).
pub const HIDE_SELECTION: &str = "Action_HideSelection";

/// `FBrActionResult Action_UnhideAll(const FBrActionParams&)` (`UBaseEditorInputComponent`, BRMK
/// 0x225340): shows every hidden brick again (Alt + H; input row `UnhideAll`).
pub const UNHIDE_ALL: &str = "Action_UnhideAll";

/// `FBrActionResult Action_EditorCancel(const FBrActionParams&)` (`UBaseEditorInputComponent`, BRMK
/// 0x221880): `ABrickEditor::OnPressedCancel` (BRMK 0xC3C60).
pub const EDITOR_CANCEL: &str = "Action_EditorCancel";

/// The handler of a button's action, e.g. [`SELECT`] or [`MOVE_CAMERA`]: called when the button is
/// pressed and when it's released (`FBrActionParams` byte +8, see [`SELECT`]).
pub struct ButtonAction {
    /// `EditorInputComponent` (the editor's input, which the handlers belong to).
    class: *const UClass,
    /// The exec thunk.
    pub thunk: *const u8,
    /// Its `const FBrActionParams&` parameter.
    params: *const FProperty,
}

// SAFETY: reflection records and game code live as long as the game; only used on the game thread.
unsafe impl Send for ButtonAction {}
unsafe impl Sync for ButtonAction {}

/// The button's state in a call to a [`ButtonAction`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Click {
    Pressed,
    Released,
}

/// `EBrActionMode::Real` in `FBrActionParams::Mode`: the input happened (the other value is a test
/// of whether it would be handled).
const REAL: u8 = 1;
/// Where `FBrActionParams` keeps whether the button was released (see [`SELECT`]), and its
/// size: after `Mode` (+0), `bIsSecondaryAction` (+1) and `AxisValue` (+4).
const RELEASED: usize = 8;
const ACTION_PARAMS_SIZE: u32 = 0xC;
/// `FBrActionResult`'s first byte when the action was handled.
const HANDLED: u8 = 0;

/// The button action handler `name` (e.g. [`SELECT`]) of the editor's input.
pub fn button_action(name: &'static str) -> Result<ButtonAction, String> {
    let class = reflection::find_class("EditorInputComponent")
        .ok_or("class EditorInputComponent not found")?;
    let action = axis(class, name)?;
    // SAFETY: live reflection records: the function's parameters and their structs.
    unsafe {
        let params = reflection::struct_type(action.params);
        let mode = reflection::struct_field_offset(params, "Mode", "EnumProperty", 1)
            .or_else(|_| reflection::struct_field_offset(params, "Mode", "ByteProperty", 1))?;
        let axis_value = reflection::struct_field_offset(params, "AxisValue", "FloatProperty", 4)?;
        if mode != 0 || axis_value != 4 || (*params).properties_size != ACTION_PARAMS_SIZE {
            return Err(format!("{name}: FBrActionParams changed its layout"));
        }
        returns_action_result(class, name)?;
        Ok(ButtonAction {
            class,
            thunk: action.thunk,
            params: action.params,
        })
    }
}

impl ButtonAction {
    /// Whether `object` is an editor input component.
    ///
    /// # Safety
    /// `object` must be a live object.
    pub unsafe fn is_editor_input(&self, object: *const c_void) -> bool {
        // SAFETY: per the contract.
        unsafe { reflection::with_parents(reflection::class_of(object)) }
            .any(|class| class == self.class.cast())
    }

    /// What a call of the thunk does with the button, if it's a real input called through
    /// `ProcessEvent` (as the input system's calls are).
    ///
    /// # Safety
    /// `stack` must be the frame the thunk was called with.
    pub unsafe fn click(&self, stack: *const c_void) -> Option<Click> {
        // SAFETY: per the contract; the parameter is an FBrActionParams of the checked size.
        unsafe {
            let params = reflection::native_call_parameter(stack, self.params)?;
            if params.read() != REAL {
                return None;
            }
            Some(if params.add(RELEASED).read() == 0 {
                Click::Pressed
            } else {
                Click::Released
            })
        }
    }

    /// Marks the call as handled without running the game's handler (see [`set_handled`]).
    ///
    /// # Safety
    /// `result` must be the return value pointer the thunk was called with.
    pub unsafe fn handled(&self, result: *mut c_void) {
        // SAFETY: per the contract; `button_action` checked the return value.
        unsafe { set_handled(result) }
    }
}

/// `FBrActionResult Action_ZoomInStep(const FBrActionParams&)` and `Action_ZoomOutStep`
/// (`UBaseInputComponent`, BRMK 0x225520 / 0x2255D0): a zoom step (the mouse wheel in the editor).
/// For a real input (`Params.Mode`), they step the view target's zoom ratio by +1 or -1 (BRMK
/// 0x372190, `Step` of its zoom params), and return handled if it has zoom params.
pub struct ZoomSteps {
    /// `EditorInputComponent`.
    class: *const UClass,
    pub zoom_in: Axis,
    pub zoom_out: Axis,
}

// SAFETY: reflection records and game code live as long as the game; only used on the game thread.
unsafe impl Send for ZoomSteps {}
unsafe impl Sync for ZoomSteps {}

pub fn zoom_steps() -> Result<ZoomSteps, String> {
    let class = reflection::find_class("EditorInputComponent")
        .ok_or("class EditorInputComponent not found")?;
    let steps = ZoomSteps {
        class,
        zoom_in: axis(class, "Action_ZoomInStep")?,
        zoom_out: axis(class, "Action_ZoomOutStep")?,
    };
    returns_action_result(class, "Action_ZoomInStep")?;
    returns_action_result(class, "Action_ZoomOutStep")?;
    // SAFETY: `axis` checked that the parameter is an FBrActionParams, a reflected struct.
    let mode = unsafe {
        let params = reflection::struct_type(steps.zoom_in.params);
        reflection::struct_field_offset(params, "Mode", "EnumProperty", 1)
            .or_else(|_| reflection::struct_field_offset(params, "Mode", "ByteProperty", 1))?
    };
    if mode != 0 {
        return Err("FBrActionParams changed its layout".into());
    }
    Ok(steps)
}

impl ZoomSteps {
    /// Whether `object` is an editor input component.
    ///
    /// # Safety
    /// `object` must be a live object.
    pub unsafe fn is_editor_input(&self, object: *const c_void) -> bool {
        // SAFETY: per the contract.
        unsafe { reflection::with_parents(reflection::class_of(object)) }
            .any(|class| class == self.class.cast())
    }

    /// Whether a call of `step`'s thunk is a real zoom step called through `ProcessEvent` (as the
    /// input system's calls are), not a test of whether it would be handled.
    ///
    /// # Safety
    /// `stack` must be the frame `step`'s thunk was called with.
    pub unsafe fn is_real(&self, step: &Axis, stack: *const c_void) -> bool {
        // SAFETY: per the contract; `Mode` is the first byte of the FBrActionParams.
        unsafe {
            match reflection::native_call_parameter(stack, step.params) {
                Some(params) => params.read() == REAL,
                None => false,
            }
        }
    }

    /// Marks a call of a zoom step thunk as handled without running the game's handler.
    ///
    /// # Safety
    /// `result` must be the return value pointer the thunk was called with.
    pub unsafe fn handled(&self, result: *mut c_void) {
        // SAFETY: per the contract; `zoom_steps` checked the return values.
        unsafe { set_handled(result) }
    }
}

impl MouseMoveAxes {
    /// Whether `object` is an editor input component.
    ///
    /// # Safety
    /// `object` must be a live object.
    pub unsafe fn is_editor_input(&self, object: *const c_void) -> bool {
        // SAFETY: per the contract.
        unsafe { reflection::with_parents(reflection::class_of(object)) }
            .any(|class| class == self.class.cast())
    }

    /// The mouse movement a call to `axis`'s thunk carries, as the handler will read it, if the call
    /// came through `ProcessEvent` (as the input system's calls do). Its parameter is a `const&`,
    /// so that's the caller's own `FBrActionParams`.
    ///
    /// # Safety
    /// `stack` must be the frame `axis`'s thunk was called with.
    pub unsafe fn axis_value(&self, axis: &Axis, stack: *const c_void) -> Option<*mut f32> {
        // SAFETY: per the contract; the parameter is an FBrActionParams.
        unsafe {
            let params = reflection::native_call_parameter(stack, axis.params)?;
            Some(params.add(self.axis_value).cast::<f32>())
        }
    }

    /// Marks a call of a mouse axis thunk as handled without running the game's handler: the
    /// movement neither moves bricks nor turns the camera (see [`set_handled`]).
    ///
    /// # Safety
    /// `result` must be the return value pointer the thunk was called with.
    pub unsafe fn handled(&self, result: *mut c_void) {
        // SAFETY: per the contract; `mouse_move_axes` checked the return values.
        unsafe { set_handled(result) }
    }
}
