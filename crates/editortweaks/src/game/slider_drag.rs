//! Dragging a `UBrickSliderWidget` with the mouse: its `NativeOnMouseMove` (BRMK 1.11.6 export
//! 0x440A50; game 0xF2D610 in build 25831956, 0xF2D570 in build 25633865).
//!
//! ```text
//! FReply NativeOnMouseMove(const FGeometry& Geometry, const FPointerEvent& Event)
//! {
//!     if (!bDragging) return Super::NativeOnMouseMove(Geometry, Event);
//!     bool bAbsolute = !Event.IsShiftDown()
//!         && UUIFunctionLibrary::ShouldUseFastSliderMode(ValueType) != Event.IsControlDown();
//!     Range = UUIFunctionLibrary::GetDefaultValueRange(ValueType, {MinValue, MaxValue});
//!     if (bAbsolute)  // the value under the cursor
//!         DragValue = Range.Min + (Event.ScreenX - Left) / Width * (Range.Max - Range.Min);
//!     else            // moved by the mouse movement: 20 pixels per step, 500 with Shift
//!         DragValue += (Event.IsShiftDown() ? 0.002 : 0.05) * Step * Event.CursorDelta.X;
//!     DragValue = Clamp(DragValue, Range.Min, Range.Max);
//!     if (LastFrame != GFrameCounter) {  // one value change per frame
//!         LastFrame = GFrameCounter;
//!         SetValueInternal(DragValue, true, true, Interactive);
//!     }
//!     if (!bAbsolute) keep the (hidden) cursor over the slider: FReply::SetMousePos
//!     return FReply::Handled();
//! }
//! ```
//!
//! - `bDragging` is set and `DragValue` set to `Value` by `OnDragged`, once the mouse moved far
//!   enough after a press; `OnReleased` clears it and commits the value (see `game::slider`).
//! - `ShouldUseFastSliderMode` is false only for the distance types (15-19): those drag relatively
//!   by default, and Ctrl makes them absolute. `Step` is `ValueStep`, or `GetDefaultValueStep
//!   (ValueType)` when that's 0. Neither function is reflected.
//! - The slider captures the mouse in high precision mode (`ShouldCaptureHighPrecisionMouseMovement`
//!   is true): the cursor is hidden and `CursorDelta` holds raw mouse movement. Mouse move events
//!   only come while the mouse moves.
//! - `bDragging` (bit 0 of a flags byte), `DragValue` and `LastFrame` aren't reflected. Their
//!   offsets (+0x298, +0x29C, +0x2A0 in the game; +0x430, +0x434, +0x438 in BRMK) are read from
//!   the code matched by the patterns below.

use core::ffi::c_void;

use crate::framework::patch;
use crate::framework::sig::FunctionSig;
use crate::game::slate::{Geometry, PointerEvent};

/// The start of `NativeOnMouseMove`: testing `bDragging` (the flags byte's offset at +2).
pub const DRAGGING_CHECK: FunctionSig = FunctionSig {
    name: "UBrickSliderWidget::NativeOnMouseMove",
    pattern: "f6 81 ?? ?? 00 00 01 49 8b f1 4d 8b f0 48 8b fa 48 8b d9 0f 84",
};

/// `NativeOnMouseMove` clamping the new drag value (xmm4) to the range (xmm1, xmm2), storing it in
/// `DragValue` before and after (offsets at +15 and +32). The register restore in between
/// (`mov r15, [rsp+...]`) is wildcarded.
pub const DRAG_CLAMP: FunctionSig = FunctionSig {
    name: "UBrickSliderWidget::NativeOnMouseMove (clamp)",
    pattern: "0f 2f e1 ?? ?? ?? ?? ?? ?? ?? ?? f3 0f 11 a3 ?? ?? 00 00 72 07 0f 28 cc f3 0f 5d ca f3 0f 11 8b ?? ?? 00 00",
};

/// Where in [`DRAG_CLAMP`] the clamp is, and the code replacing it: `jb +7; movaps xmm1, xmm4;
/// minss xmm1, xmm2` (xmm1 = the clamped value) becomes `nop; movaps xmm1, xmm4` (xmm1 = the value
/// as it is). The same size, and the jump's target (the store right after) is unchanged.
const CLAMP_AT: usize = 19;
const CLAMP: [u8; 9] = [0x72, 0x07, 0x0f, 0x28, 0xcc, 0xf3, 0x0f, 0x5d, 0xca];
const NO_CLAMP: [u8; 9] = [0x66, 0x0f, 0x1f, 0x44, 0x00, 0x00, 0x0f, 0x28, 0xcc];

/// `FReply NativeOnMouseMove(const FGeometry&, const FPointerEvent&)`: the reply is returned
/// through a hidden pointer, passed after `this` and returned.
pub type MouseMoveFn = unsafe extern "C" fn(
    this: *mut c_void,
    reply: *mut c_void,
    geometry: *const Geometry,
    event: *const PointerEvent,
) -> *mut c_void;

pub struct MouseDrag {
    /// `NativeOnMouseMove`.
    pub mouse_move: *const u8,
    /// Offset of the flags byte whose bit 0 is `bDragging`.
    dragging: usize,
    /// Offset of `DragValue`.
    drag_value: usize,
    /// The clamp: [`CLAMP`].
    clamp: *mut u8,
}

// SAFETY: points into the game's code; only used on the game thread (and while loading).
unsafe impl Send for MouseDrag {}
unsafe impl Sync for MouseDrag {}

pub fn mouse_drag() -> Result<MouseDrag, String> {
    let check = DRAGGING_CHECK.locate()?;
    let clamp = DRAG_CLAMP.locate()?;
    if clamp.function != check.function {
        return Err("the drag patterns matched in different functions".into());
    }
    // SAFETY: each offset is a 32-bit displacement inside its pattern's match.
    let read = |at: *const u8| unsafe { at.cast::<u32>().read_unaligned() } as usize;
    // SAFETY: the offsets are inside the matched patterns (see their docs).
    let (dragging, before, after) = unsafe {
        (
            read(check.address.add(2)),
            read(clamp.address.add(15)),
            read(clamp.address.add(32)),
        )
    };
    if before != after {
        return Err(format!(
            "the drag value is stored at 0x{before:X} and 0x{after:X}"
        ));
    }
    Ok(MouseDrag {
        mouse_move: check.function,
        dragging,
        drag_value: after,
        // SAFETY: inside the match, see `CLAMP_AT`.
        clamp: unsafe { clamp.address.add(CLAMP_AT) }.cast_mut(),
    })
}

impl MouseDrag {
    /// Whether `slider` is being dragged.
    ///
    /// # Safety
    /// `slider` must be a live `UBrickSliderWidget`.
    pub unsafe fn is_dragging(&self, slider: *const c_void) -> bool {
        // SAFETY: per the contract; the offset comes from the slider's own code.
        unsafe { slider.byte_add(self.dragging).cast::<u8>().read() & 1 != 0 }
    }

    /// The value being dragged to, before snapping (`DragValue`).
    ///
    /// # Safety
    /// As for [`Self::is_dragging`].
    pub unsafe fn drag_value(&self, slider: *const c_void) -> f32 {
        // SAFETY: as for `is_dragging`.
        unsafe { slider.byte_add(self.drag_value).cast::<f32>().read() }
    }

    /// # Safety
    /// As for [`Self::is_dragging`]; game thread only.
    pub unsafe fn set_drag_value(&self, slider: *mut c_void, value: f32) {
        // SAFETY: as for `is_dragging`.
        unsafe { slider.byte_add(self.drag_value).cast::<f32>().write(value) }
    }

    /// Removes the clamp to the slider's range from dragging: `SetValueInternal` then gets the
    /// dragged value as it is, and clamps it itself.
    ///
    /// # Safety
    /// Only while nothing runs the clamp: on the game thread, outside of `NativeOnMouseMove`.
    pub unsafe fn lift_clamp(&self) -> Result<(), String> {
        // SAFETY: `clamp` is the matched clamp, whole instructions with no jump into them (see
        // `CLAMP`); per the contract, nothing runs them meanwhile.
        unsafe { patch::write_code(self.clamp, &CLAMP, &NO_CLAMP) }
    }

    /// Puts the clamp back after [`Self::lift_clamp`].
    ///
    /// # Safety
    /// As for [`Self::lift_clamp`].
    pub unsafe fn restore_clamp(&self) -> Result<(), String> {
        // SAFETY: `clamp` holds the code `lift_clamp` wrote, whole instructions with no jump into
        // them (see `CLAMP`); per the contract, nothing runs them meanwhile.
        unsafe { patch::write_code(self.clamp, &NO_CLAMP, &CLAMP) }
    }
}
