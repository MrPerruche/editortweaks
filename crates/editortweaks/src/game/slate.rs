//! Slate's input types (UE 4.27 engine layouts). Only the leading fields we read are declared:
//! these structs are only ever used through pointers to the engine's own objects. The offsets are
//! the ones the game's own code reads (`game::slider_drag`).

use core::ffi::c_void;
use core::mem::offset_of;

/// `FModifierKeysState` bits for the left and right Shift keys (Control is 0b1100, Alt 0b11_0000).
pub const SHIFT: u16 = 0b00_0011;
/// `FModifierKeysState` bits for the left and right Control keys.
pub const CONTROL: u16 = 0b00_1100;

/// `FPointerEvent` (a mouse event): `FInputEvent`'s fields, then the positions in screen space.
#[repr(C)]
pub struct PointerEvent {
    _vtable: *const c_void,
    /// `FInputEvent::ModifierKeys`: e.g. the [`SHIFT`] bits.
    pub modifier_keys: u16,
    _input_event: [u8; 0x0E],
    pub screen_space_position: [f32; 2],
    pub last_screen_space_position: [f32; 2],
    /// The mouse movement; raw mouse counts while a widget captures the mouse in high precision
    /// mode (the cursor is hidden then).
    pub cursor_delta: [f32; 2],
}

const _: () = {
    assert!(offset_of!(PointerEvent, modifier_keys) == 0x8);
    assert!(offset_of!(PointerEvent, screen_space_position) == 0x18);
    assert!(offset_of!(PointerEvent, cursor_delta) == 0x28);
};

/// `FGeometry`: a widget's size and where it is on screen.
#[repr(C)]
pub struct Geometry {
    /// In the widget's own (layout) units.
    pub size: [f32; 2],
    pub scale: f32,
    pub absolute_position: [f32; 2],
    pub position: [f32; 2],
    /// `AccumulatedRenderTransform`: local to screen space, a 2x2 matrix (row major) and then
    /// the translation.
    pub render_matrix: [f32; 4],
    pub render_translation: [f32; 2],
}

const _: () = assert!(offset_of!(Geometry, render_matrix) == 0x1C);

impl Geometry {
    /// The widget's width on screen, in pixels: its size transformed to screen space, as the
    /// slider computes it.
    pub fn absolute_width(&self) -> f32 {
        self.size[0] * self.render_matrix[0] + self.size[1] * self.render_matrix[2]
    }

    /// The screen X of the widget's left edge: its local origin transformed to screen space, as
    /// the slider computes it (`FVector2D::ZeroVector` through the render transform).
    pub fn absolute_left(&self) -> f32 {
        self.render_translation[0]
    }
}
