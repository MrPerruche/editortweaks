//! What the panel lets players do that a peer's op could go beyond (`docs/re/coop/
//! archive-addresses.md`, "Checks the panel makes"). Every member checks a peer's edit alike, so a
//! refused one leaves only its sender's editor different, which its audits then mend.
//!
//! - **Object order:** the panel only moves seats among seats and cameras among cameras (each
//!   class' `FObjectOrderBrickProperty`); every other brick keeps its place.
//! - **A camera's seat** (`OwningSeat`): a seat or none (`UCameraBrick::IsSeatObjectAllowed`).

use brickrust::ue::uclass::UStruct;

use super::changes::Object;
use crate::game::reflection;

/// The classes the panel's checks name, found by reflection.
pub struct Checks {
    seat: *const UStruct,
    camera: *const UStruct,
}

// SAFETY: class records living as long as the game; only used on the game thread.
unsafe impl Send for Checks {}
unsafe impl Sync for Checks {}

/// What the panel reorders an object among.
#[derive(PartialEq, Eq)]
enum Kind {
    Seat,
    Camera,
    /// Not reordered: it keeps its place.
    Other(Object),
}

impl Checks {
    pub fn find() -> Result<Self, String> {
        let class = |name| {
            reflection::find_class(name)
                .map(|class| class.cast::<UStruct>())
                .ok_or(format!("class {name} not found"))
        };
        Ok(Self {
            seat: class("SeatBrick")?,
            camera: class("CameraBrick")?,
        })
    }

    /// Whether the panel could have put the objects in the order `from` into the order `to`.
    ///
    /// # Safety
    /// Every object must be live. Game thread only.
    pub unsafe fn reorders(&self, from: &[Object], to: &[Object]) -> bool {
        // SAFETY: per the contract.
        let kind = |object| unsafe { self.kind(object) };
        from.len() == to.len() && from.iter().zip(to).all(|(&a, &b)| kind(a) == kind(b))
    }

    /// Whether `bricks` may be a camera's seat: one seat, or none.
    ///
    /// # Safety
    /// Every brick must be live. Game thread only.
    pub unsafe fn camera_seat(&self, bricks: &[Object]) -> bool {
        match bricks {
            [] => true,
            // SAFETY: per the contract.
            [seat] => unsafe { reflection::is_a(*seat, self.seat) },
            _ => false,
        }
    }

    /// # Safety
    /// `object` must be live. Game thread only.
    unsafe fn kind(&self, object: Object) -> Kind {
        // SAFETY: per the contract.
        unsafe {
            if reflection::is_a(object, self.seat) {
                Kind::Seat
            } else if reflection::is_a(object, self.camera) {
                Kind::Camera
            } else {
                Kind::Other(object)
            }
        }
    }
}
