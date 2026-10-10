//! `UScalableBrick`: bricks with an editable size (the `BrickSize` vector property, in cm).
//!
//! Besides the property's own range, the brick constrains its size whenever `BrickSize` is modified
//! ([`POST_MODIFY`]; not on load, so vehicles keep
//! whatever size they were saved with) in `ConstraintBrickSize`, found through BRMK 1.11.6's export
//! `?ConstraintBrickSize@UScalableBrick@@MEAAXXZ` (no subclass overrides it). Game 1.11.7: RVA
//! 0xD2D190, `BrickSize` at +0x100 (BRMK: +0x108).
//!
//! ```text
//! void ConstraintBrickSize() {
//!     BrickSize = Max(BrickSize, GetMinBrickSize());   // virtual, per brick type
//!     BrickSize = Min(BrickSize, GetMaxBrickSize());   // virtual; UScalableBrick: 9000 per axis
//! }
//! ```

use core::ffi::c_void;
use std::sync::OnceLock;

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;
use brickrust::ue::fproperty::FProperty;
use brickrust::ue::uclass::UStruct;

use crate::framework::sig::FunctionSig;
use crate::game::brick_property::{self, ChangedEvent};
use crate::game::editor_object::Vector;
use crate::game::reflection;

pub const CONSTRAINT_BRICK_SIZE: FunctionSig = FunctionSig {
    name: "UScalableBrick::ConstraintBrickSize",
    // The three `maxss` against BrickSize's components (field offsets wildcarded).
    pattern: "f3 44 0f 5f 83 ?? ?? 00 00 f3 0f 5f b3 ?? ?? 00 00 f3 0f 5f bb ?? ?? 00 00",
};

pub type ConstraintBrickSizeFn = unsafe extern "C" fn(brick: *mut c_void);

/// `UScalableBrick::PostModifyBrickProperty` (game RVA 0xD4F560; BRMK export 0x1672A0), see
/// `game::brick_property`. Subclasses that override it call it first (e.g. `ULightBrick`'s, game
/// 0xD4F4E0, then updates its brightness), so it sees every scalable brick's changes:
///
/// ```text
/// SizeChanged = IsPropertyChanged(Event, "BrickSize");
/// SpacingChanged = IsPropertyChanged(Event, "ConnectorSpacing");
/// if (SizeChanged) ConstraintBrickSize();             // virtual +0x4F0
/// if (SizeChanged || SpacingChanged) { mirror them onto the mirrored brick; rebuild mesh, ... }
/// UBrick::PostModifyBrickProperty(Event);
/// ```
pub const POST_MODIFY: FunctionSig = FunctionSig {
    name: "UScalableBrick::PostModifyBrickProperty",
    // `sete` of the spacing check, then the size check calling ConstraintBrickSize.
    pattern: "41 0f 94 c7 49 8d 4d ?? 45 84 f6 74 ?? 49 8b 45 ?? ff 90 ?? ?? 00 00",
};

/// The `BrickSize` property of scalable bricks (an `FVector`: three floats).
pub fn brick_size_field() -> Result<*const FProperty, String> {
    let class = reflection::find_class("ScalableBrick").ok_or("class ScalableBrick not found")?;
    reflection::field(class, "BrickSize", "StructProperty", 12)
}

/// Scalable bricks' sizes, read and changed like the editor's panel does (after engine init): write
/// `BrickSize`, then tell the brick ([`brick_property::post_modify`]), which constrains it
/// ([`CONSTRAINT_BRICK_SIZE`]) and rebuilds its mesh, collision, materials and so on.
pub struct BrickSizes {
    /// `ScalableBrick`.
    class: *const UStruct,
    /// Offset of `UScalableBrick::BrickSize`.
    size: usize,
    /// The property's name, for change events.
    name: FName,
    /// Where scalable bricks keep their `IBrickPropertyInterface`, once found on one.
    interface: OnceLock<usize>,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for BrickSizes {}
unsafe impl Sync for BrickSizes {}

impl BrickSizes {
    pub fn find() -> Result<Self, String> {
        let class =
            reflection::find_class("ScalableBrick").ok_or("class ScalableBrick not found")?;
        let field = reflection::field(class, "BrickSize", "StructProperty", 12)?;
        // SAFETY: a live property record.
        let (size, name) = unsafe {
            (
                (*field).offset_internal as usize,
                (*field).field.name_private,
            )
        };
        Ok(Self {
            class: class.cast(),
            size,
            name,
            interface: OnceLock::new(),
        })
    }

    /// Whether `object` is a scalable brick.
    ///
    /// # Safety
    /// `object` must be a live object.
    pub unsafe fn is_scalable(&self, object: *const UObjectBase) -> bool {
        // SAFETY: per the contract.
        unsafe { reflection::with_parents(reflection::class_of(object.cast())) }
            .any(|class| class == self.class)
    }

    /// The size of `brick`, in centimeters along its own axes.
    ///
    /// # Safety
    /// `brick` must be a live scalable brick.
    pub unsafe fn size(&self, brick: *const UObjectBase) -> Vector {
        // SAFETY: per the contract; the offset comes from reflection.
        unsafe {
            brick
                .cast::<u8>()
                .add(self.size)
                .cast::<Vector>()
                .read_unaligned()
        }
    }

    /// Gives `brick` the size `size`, `interactive`ly (while dragging: the brick skips its slow
    /// updates until a final change) or not, and returns the size it kept: the brick constrains it.
    /// `None` if the brick can't be told (its property interface wasn't found).
    ///
    /// # Safety
    /// `brick` must be a live scalable brick in the editor. Game thread only.
    pub unsafe fn resize(
        &self,
        brick: *mut UObjectBase,
        size: Vector,
        interactive: bool,
    ) -> Option<Vector> {
        // SAFETY: per the contract; the interface offset is checked on a brick first (any scalable
        // brick has it at the same place: it comes from `UBrickEditorObject`).
        unsafe {
            let offset = match self.interface.get() {
                Some(&offset) => offset,
                None => {
                    let offset = brick_property::interface_offset(self.class, brick)?;
                    *self.interface.get_or_init(|| offset)
                }
            };
            brick
                .cast::<u8>()
                .add(self.size)
                .cast::<Vector>()
                .write_unaligned(size);
            let interface = brick.cast::<u8>().add(offset).cast::<c_void>();
            ChangedEvent::with_property(self.name, interactive, |event| {
                brick_property::post_modify(interface, event);
            });
            Some(self.size(brick))
        }
    }
}

/// Every face connector spacing at its densest ([`connector_spacing`] gives 10 cm on all faces).
pub const DENSEST_CONNECTOR_SPACING: u16 = 0x0FFF;

/// The connector spacing of one face, in cm (`None` for a face without connectors).
///
/// `spacing` is the brick's `ConnectorSpacing` (`FScalableBrickConnectorSpacing`, 16 bits): two bits
/// per face at bit `4 * axis + 2 * positive` (`UBrickStatics::GetScalableBrickConnectorSpacingAxis`),
/// holding an `EConnectorSpacing`: 0 None, 1 Default (30 cm), 2 Half (15 cm), 3 Third (10 cm)
/// (distances from the game's `EConnectorSpacing` to distance switch).
pub fn connector_spacing(spacing: u16, axis: usize, positive: bool) -> Option<f32> {
    let shift = 4 * axis + if positive { 2 } else { 0 };
    match (spacing >> shift) & 3 {
        1 => Some(30.0),
        2 => Some(15.0),
        3 => Some(10.0),
        _ => None,
    }
}

/// How many connector points a box-shaped brick of `size` (cm) gets with `spacing` (see
/// [`connector_spacing`]); other shapes get fewer. Each face gets `max(1, round(side / spacing))`
/// points along each of its sides (`UScalableBrickBase::AddGeneratedConnector`). A negative size
/// (a mirrored brick) counts by its magnitude: such bricks still connect.
pub fn connector_points(size: [f32; 3], spacing: u16) -> f64 {
    let along = |side: f32, spacing: f32| {
        (f64::from(side.abs()) / f64::from(spacing))
            .round()
            .max(1.0)
    };
    let mut points = 0.0;
    for axis in 0..3 {
        // A face across `axis` spans the two other axes.
        let (a, b) = (size[(axis + 1) % 3], size[(axis + 2) % 3]);
        for positive in [false, true] {
            if let Some(spacing) = connector_spacing(spacing, axis, positive) {
                points += along(a, spacing) * along(b, spacing);
            }
        }
    }
    points
}
