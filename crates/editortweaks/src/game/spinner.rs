//! `USpinnerBrick`: a cross-section swept along an elliptic arc, with its values as properties
//! (`SpinnerShape`, `SpinnerSize`, `SpinnerRadius`, `SpinnerAngle`), all found by reflection. See
//! `docs/re/spinners.md` (BRMK 1.11.6) for what the brick does with them, and `model_io::spinner`
//! for its shape.
//!
//! Changed like the editor's panel does: write the property, then tell the brick
//! ([`brick_property::post_modify`], the brick's own `PostModifyBrickProperty`, BRMK 0x167890).
//! While dragging (interactive) the brick only redraws its mesh; a final change also rebuilds its
//! connectors and collision, and takes a thickness wider than the radii allow down to them
//! (`SpinnerRadius` changed) and an angle under 1 degree up to it (`SpinnerAngle` changed).

use core::ffi::c_void;
use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;
use brickrust::ue::uclass::UStruct;

use crate::game::brick_property::{self, ChangedEvent};
use crate::game::reflection;

/// A spinner's values, as the brick stores them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Values {
    /// `ESpinnerBrickShape`: 0 Rectangle ... 12 Diamond.
    pub shape: u8,
    /// Thickness across the ring, height along Z (cm).
    pub size: [f32; 2],
    /// The arc's semi-axes along X and Y (cm).
    pub radius: [f32; 2],
    /// Degrees, signed.
    pub angle: f32,
}

/// A value a change can name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Property {
    Size,
    Radius,
    Angle,
}

/// A reflected property: where it is in the brick, and its name for change events.
#[derive(Clone, Copy)]
struct Field {
    offset: usize,
    name: FName,
}

/// Spinners' values, read and changed (after engine init).
pub struct Spinners {
    /// `SpinnerBrick`.
    class: *const UStruct,
    /// `SpinnerShape`, a one-byte enum.
    shape: usize,
    /// `SpinnerSize` and `SpinnerRadius` (`FVector2D`s), `SpinnerAngle` (a float).
    size: Field,
    radius: Field,
    angle: Field,
    /// Where spinners keep their `IBrickPropertyInterface`, once found on one.
    interface: OnceLock<usize>,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for Spinners {}
unsafe impl Sync for Spinners {}

impl Spinners {
    pub fn find() -> Result<Self, String> {
        let class = reflection::find_class("SpinnerBrick").ok_or("class SpinnerBrick not found")?;
        let shape = reflection::field_offset(class, "SpinnerShape", "EnumProperty", 1)
            .or_else(|_| reflection::field_offset(class, "SpinnerShape", "ByteProperty", 1))?;
        let field = |name: &'static str, kind: &'static str, size: i32| -> Result<Field, String> {
            let property = reflection::field(class, name, kind, size)?;
            // SAFETY: `field` only returns live property records.
            let (offset, name) = unsafe {
                (
                    (*property).offset_internal as usize,
                    (*property).field.name_private,
                )
            };
            Ok(Field { offset, name })
        };
        Ok(Self {
            class: class.cast(),
            shape,
            size: field("SpinnerSize", "StructProperty", 8)?,
            radius: field("SpinnerRadius", "StructProperty", 8)?,
            angle: field("SpinnerAngle", "FloatProperty", 4)?,
            interface: OnceLock::new(),
        })
    }

    /// Whether `object` is a spinner.
    ///
    /// # Safety
    /// `object` must be a live object.
    pub unsafe fn is_spinner(&self, object: *const UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { reflection::is_a(object, self.class) }
    }

    /// `brick`'s values.
    ///
    /// # Safety
    /// `brick` must be a live spinner.
    pub unsafe fn read(&self, brick: *const UObjectBase) -> Values {
        // SAFETY: per the contract; the offsets come from reflection.
        unsafe {
            let at = |offset: usize| brick.cast::<u8>().add(offset);
            Values {
                shape: at(self.shape).read(),
                size: at(self.size.offset).cast::<[f32; 2]>().read_unaligned(),
                radius: at(self.radius.offset).cast::<[f32; 2]>().read_unaligned(),
                angle: at(self.angle.offset).cast::<f32>().read_unaligned(),
            }
        }
    }

    /// Gives `brick` the size, radius and angle of `values` (not its shape) and tells it about
    /// each of `changed`, `interactive`ly (while dragging) or not; returns the values it kept.
    /// `None` if the brick can't be told (its property interface wasn't found).
    ///
    /// # Safety
    /// `brick` must be a live spinner in the editor. Game thread only.
    pub unsafe fn write(
        &self,
        brick: *mut UObjectBase,
        values: &Values,
        changed: &[Property],
        interactive: bool,
    ) -> Option<Values> {
        // SAFETY: per the contract; the interface offset is checked on a brick first (every
        // spinner has it at the same place).
        unsafe {
            let offset = match self.interface.get() {
                Some(&offset) => offset,
                None => {
                    let offset = brick_property::interface_offset(self.class, brick)?;
                    *self.interface.get_or_init(|| offset)
                }
            };
            let at = |offset: usize| brick.cast::<u8>().add(offset);
            at(self.size.offset)
                .cast::<[f32; 2]>()
                .write_unaligned(values.size);
            at(self.radius.offset)
                .cast::<[f32; 2]>()
                .write_unaligned(values.radius);
            at(self.angle.offset)
                .cast::<f32>()
                .write_unaligned(values.angle);
            let interface = brick.cast::<u8>().add(offset).cast::<c_void>();
            // The radius first: on a final change the brick fits the thickness to it, which the
            // size written then keeps.
            for property in [Property::Radius, Property::Size, Property::Angle] {
                if !changed.contains(&property) {
                    continue;
                }
                let name = match property {
                    Property::Size => self.size.name,
                    Property::Radius => self.radius.name,
                    Property::Angle => self.angle.name,
                };
                ChangedEvent::with_property(name, interactive, |event| {
                    brick_property::post_modify(interface, event);
                });
            }
            Some(self.read(brick))
        }
    }
}
