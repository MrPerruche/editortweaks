//! Where the player is in the world while in the editor (`docs/re/map-in-editor.md`): the vehicle
//! they drive, else their character, which stays where the editor was entered. The game's spawn at
//! the player's location starts from there too (`IBrickPawnInterface::GetPawnRestartTransform`: a
//! vehicle's bounds' middle, a character's feet). Through reflection only:
//! `ABrickPlayerController::GetPlayerVehicle` and `GetPlayerCharacter` (exported as `exec...` by BRMK
//! 1.11.6), `AActor::K2_GetActorRotation` and `GetActorBounds` (UE 4.27).

use brickrust::ue::coreuobject::UObjectBase;

use crate::game::editor_object::Vector;
use crate::game::reflection::{self, Function, Getter, Parameters, put, take};

/// What's needed to tell where the player is.
pub struct PlayerPlaces {
    /// `ABrickVehicle* ABrickPlayerController::GetPlayerVehicle() const`.
    vehicle: Getter<*mut UObjectBase>,
    /// `ABrickCharacter* ABrickPlayerController::GetPlayerCharacter() const`.
    character: Getter<*mut UObjectBase>,
    /// `FRotator AActor::K2_GetActorRotation() const`.
    rotation: Getter<Vector>,
    /// `void AActor::GetActorBounds(bool bOnlyCollidingComponents, FVector& Origin, FVector&
    /// BoxExtent, bool bIncludeFromChildActors) const`.
    bounds: Function,
    bounds_colliding: usize,
    bounds_origin: usize,
    bounds_extent: usize,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for PlayerPlaces {}
unsafe impl Sync for PlayerPlaces {}

/// Where the player is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerPlace {
    /// Where a spawn there starts from.
    pub location: Vector,
    /// How what stands there is turned.
    pub rotation: Vector,
    /// The player's vehicle and character, either may be null.
    pub vehicle: *mut UObjectBase,
    pub character: *mut UObjectBase,
}

/// Finds what's needed. After engine init.
pub fn find() -> Result<PlayerPlaces, String> {
    let class = |name| reflection::find_class(name).ok_or(format!("class {name} not found"));
    let actor = class("Actor")?;
    let controller = class("BrickPlayerController")?;
    let bounds = Function::find(actor, "GetActorBounds")?;
    Ok(PlayerPlaces {
        vehicle: Getter::find(controller, "GetPlayerVehicle", "ObjectProperty")?,
        character: Getter::find(controller, "GetPlayerCharacter", "ObjectProperty")?,
        rotation: Getter::find(actor, "K2_GetActorRotation", "StructProperty")?,
        bounds_colliding: bounds.parameter("bOnlyCollidingComponents", "BoolProperty", 1)?,
        bounds_origin: bounds.parameter("Origin", "StructProperty", 12)?,
        bounds_extent: bounds.parameter("BoxExtent", "StructProperty", 12)?,
        bounds,
    })
}

impl PlayerPlaces {
    /// Where `controller`'s player is, if they have a vehicle or a character.
    ///
    /// # Safety
    /// `controller` must be a live `ABrickPlayerController`. Game thread only.
    pub unsafe fn of(&self, controller: *mut UObjectBase) -> Option<PlayerPlace> {
        // SAFETY: per the contract; both getters return a live actor or null, and the offsets come
        // from the function's own parameters.
        unsafe {
            let vehicle = self.vehicle.call(controller);
            let character = self.character.call(controller);
            let actor = if vehicle.is_null() {
                character
            } else {
                vehicle
            };
            if actor.is_null() {
                return None;
            }
            let mut parameters = Parameters::new();
            put(&mut parameters, self.bounds_colliding, true);
            self.bounds.call(actor, &mut parameters);
            let [x, y, z]: Vector = take(&parameters, self.bounds_origin);
            let extent: Vector = take(&parameters, self.bounds_extent);
            Some(PlayerPlace {
                location: if vehicle.is_null() {
                    [x, y, z - extent[2]]
                } else {
                    [x, y, z]
                },
                rotation: self.rotation.call(actor),
                vehicle,
                character,
            })
        }
    }
}
