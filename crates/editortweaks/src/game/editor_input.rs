//! The vehicle editor's input handlers (`UEditorInputComponent`). The input system calls them by
//! name: `UBaseInputComponent::SetupInputComponent` binds each input row (`InputTable`) to the
//! component's function `Action_<Row>` or `Axis_<Row>` (`FindFunctionChecked`, then a delegate
//! calling it through `ProcessEvent`), so hooking their exec thunks intercepts every call. Found
//! through BRMK 1.11.6's exports, then in game 1.11.7 (build 25831956); see
//! `docs/re/editor-input.md`.

use core::ffi::c_void;

use brickrust::ue::fproperty::FProperty;
use brickrust::ue::uclass::UClass;

use crate::game::reflection;

/// `FBrActionResult Axis_MouseMoveRight(const FBrActionParams&)` and `Axis_MouseMoveUp`: the mouse's
/// movement while moving or rotating bricks in move mode (G or the gizmo). The editor's versions
/// (`HandleStealMouseMove(Params, Axis)` in the game, inlined in BRMK) multiply
/// `Params.AxisValue` (MouseX or MouseY) by the user's `EditorMouseMoveSensitivity` and add it to
/// the move mode's input (`UBrickEditorMoveMode::AddMouseMoveInput`), which moves the selection by
/// it on its next tick (`TickMode`). Nothing else adds mouse input to move mode.
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

// SAFETY: reflection records and game code live as long as the game; only used on the game thread.
unsafe impl Send for MouseMoveAxes {}
unsafe impl Sync for MouseMoveAxes {}

pub fn mouse_move_axes() -> Result<MouseMoveAxes, String> {
    let class = reflection::find_class("EditorInputComponent")
        .ok_or("class EditorInputComponent not found")?;
    let right = axis(class, "Axis_MouseMoveRight")?;
    let up = axis(class, "Axis_MouseMoveUp")?;
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

/// The axis handler `name`, whose first parameter is its `FBrActionParams` (`Mode`,
/// `bIsSecondaryAction`, `AxisValue`).
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
}
