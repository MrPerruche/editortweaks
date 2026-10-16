//! Engine actors through reflection (`AActor`): where one is, teleporting it, and whether it's
//! hidden in game (`bHidden`, what `SetActorHiddenInGame` sets; replicated, so this mod hides
//! actors with `game::hidden_actors` instead).

use brickrust::ue::coreuobject::UObjectBase;

use crate::game::editor_object::Vector;
use crate::game::reflection::{self, BoolField, Function, Getter, Parameters};

pub struct Actors {
    /// `FVector AActor::K2_GetActorLocation() const`.
    location: Getter<Vector>,
    /// `FRotator AActor::K2_GetActorRotation() const`.
    rotation: Getter<Vector>,
    /// `FVector AActor::GetActorScale3D() const`.
    scale: Getter<Vector>,
    /// `bool K2_SetActorLocationAndRotation(FVector NewLocation, FRotator NewRotation, bool
    /// bSweep, FHitResult& SweepHitResult, bool bTeleport)`.
    set_place: Function,
    new_location: usize,
    new_rotation: usize,
    teleport: usize,
    hidden: BoolField,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for Actors {}
unsafe impl Sync for Actors {}

/// Finds the actor functions and fields. After engine init.
pub fn find() -> Result<Actors, String> {
    const SET_PLACE: &str = "K2_SetActorLocationAndRotation";
    let actor = reflection::find_class("Actor").ok_or("class Actor not found")?;
    let set_place = Function::find(actor, SET_PLACE)?;
    Ok(Actors {
        location: Getter::find(actor, "K2_GetActorLocation", "StructProperty")?,
        rotation: Getter::find(actor, "K2_GetActorRotation", "StructProperty")?,
        scale: Getter::find(actor, "GetActorScale3D", "StructProperty")?,
        new_location: set_place.parameter("NewLocation", "StructProperty", 12)?,
        new_rotation: set_place.parameter("NewRotation", "StructProperty", 12)?,
        teleport: set_place.parameter("bTeleport", "BoolProperty", 1)?,
        set_place,
        hidden: reflection::bool_field(actor.cast(), "bHidden")?,
    })
}

impl Actors {
    /// Where the actor is in the world.
    ///
    /// # Safety
    /// `actor` must be a live actor. Game thread only.
    pub unsafe fn location(&self, actor: *mut UObjectBase) -> Vector {
        // SAFETY: per the contract.
        unsafe { self.location.call(actor) }
    }

    /// The actor's rotation (pitch, yaw, roll in degrees).
    ///
    /// # Safety
    /// As for [`Self::location`].
    pub unsafe fn rotation(&self, actor: *mut UObjectBase) -> Vector {
        // SAFETY: per the contract.
        unsafe { self.rotation.call(actor) }
    }

    /// The actor's scale.
    ///
    /// # Safety
    /// As for [`Self::location`].
    pub unsafe fn scale(&self, actor: *mut UObjectBase) -> Vector {
        // SAFETY: per the contract.
        unsafe { self.scale.call(actor) }
    }

    /// Teleports the actor (and what's attached to it) to `location` with `rotation` (pitch, yaw,
    /// roll), without sweeping: nothing is pushed or stopped on the way.
    ///
    /// # Safety
    /// As for [`Self::location`].
    pub unsafe fn teleport(&self, actor: *mut UObjectBase, location: Vector, rotation: Vector) {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the offsets come from the function's own parameters, and
        // the others (sweep, the hit result) stay zero.
        unsafe {
            reflection::put(&mut parameters, self.new_location, location);
            reflection::put(&mut parameters, self.new_rotation, rotation);
            reflection::put(&mut parameters, self.teleport, true);
            self.set_place.call(actor, &mut parameters);
        }
    }

    /// Whether the actor is hidden in game (`bHidden`).
    ///
    /// # Safety
    /// As for [`Self::location`].
    pub unsafe fn is_hidden(&self, actor: *const UObjectBase) -> bool {
        // SAFETY: per the contract; the field's offset comes from reflection.
        unsafe { self.hidden.get(actor.cast()) }
    }
}
