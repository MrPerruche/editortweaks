//! Sweeping a sphere, as a projectile moves and measures what it goes through (engine functions
//! called through reflection like a Blueprint does): against one component alone
//! ([`ComponentSweep`], `UPrimitiveComponent::K2_SphereTraceComponent`, the `FBodyInstance::Sweep`
//! the game's thickness measurement makes), or through the world as a body with a collision profile
//! meets it ([`ProfileSweep`], `UKismetSystemLibrary::SphereTraceSingleByProfile`). A radius of 0
//! makes either a line trace (the engine traces a line for a zero-sized shape).
//!
//! Both read their hit from the reflected `FHitResult` they fill: `Location` (the sphere's middle
//! when it touched), `ImpactNormal` (the surface
//! touched; what the game's ricochets turn by), `Component`, `PhysMaterial` and `bStartPenetrating`
//! (the sphere already overlapped it where it started).

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::tshared::FWeakObjectPtr;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::game::editor_object::Vector;
use crate::game::reflection::{self, BoolField, Function, Parameters};
use crate::game::weak_object;

/// What a sweep touched first.
#[derive(Clone, Copy)]
pub struct SweepHit {
    /// Where the sphere's middle was when it touched (the start if it already overlapped).
    pub location: Vector,
    /// The normal of the surface it touched.
    pub normal: Vector,
    /// The component touched, if it still exists.
    pub component: Option<*mut UObjectBase>,
    /// The physical material touched, if any.
    pub material: Option<*mut UObjectBase>,
    /// Whether the sphere already overlapped it where it started.
    pub start_penetrating: bool,
}

/// Offsets of the `FHitResult` fields read, in a function's parameters.
#[derive(Clone, Copy)]
struct HitFields {
    location: usize,
    normal: usize,
    component: usize,
    material: usize,
    start_penetrating: BoolField,
}

impl HitFields {
    /// The fields of the `FHitResult` parameter `OutHit` of `class`'s function `name`.
    fn find(class: *const UClass, name: &'static str) -> Result<Self, String> {
        let found = reflection::find_function(class, name)?;
        // SAFETY: a live function record, its parameters linked to it.
        let (out_hit, hit_struct): (usize, *const UStruct) = unsafe {
            let out_hit = reflection::find_property(found.cast(), "OutHit");
            if out_hit.is_null() || !reflection::is_kind(out_hit, "StructProperty") {
                return Err(format!("{name}: no OutHit"));
            }
            (
                (*out_hit).offset_internal as usize,
                reflection::struct_type(out_hit),
            )
        };
        if hit_struct.is_null() {
            return Err(format!("{name}: OutHit isn't a struct"));
        }
        let field = |name, kind, size| {
            reflection::struct_field_offset(hit_struct, name, kind, size)
                .map(|offset| out_hit + offset)
                .map_err(|reason| format!("HitResult: {reason}"))
        };
        Ok(Self {
            location: field("Location", "StructProperty", 12)?,
            normal: field("ImpactNormal", "StructProperty", 12)?,
            component: field("Component", "WeakObjectProperty", 8)?,
            material: field("PhysMaterial", "WeakObjectProperty", 8)?,
            start_penetrating: reflection::bool_field(hit_struct, "bStartPenetrating")
                .map_err(|reason| format!("HitResult: {reason}"))?
                .shifted(out_hit),
        })
    }

    /// The hit in `parameters`, once the function returned true.
    ///
    /// # Safety
    /// `parameters` must hold the hit the function filled. Game thread only.
    unsafe fn read(&self, parameters: &mut Parameters) -> SweepHit {
        // SAFETY: per the contract; each field is read at its reflected offset, the weak pointers
        // resolved safely.
        unsafe {
            let weak = |parameters: &Parameters, offset| {
                weak_object::resolve(reflection::take::<FWeakObjectPtr>(parameters, offset))
            };
            SweepHit {
                location: reflection::take(parameters, self.location),
                normal: reflection::take(parameters, self.normal),
                component: weak(parameters, self.component),
                material: weak(parameters, self.material),
                start_penetrating: self
                    .start_penetrating
                    .get(parameters.as_mut_ptr().cast_const()),
            }
        }
    }
}

/// `bool UPrimitiveComponent::K2_SphereTraceComponent(FVector TraceStart, FVector TraceEnd, float
/// SphereRadius, bool bTraceComplex, bool bShowTrace, bool bPersistentShowTrace, FVector&
/// HitLocation, FVector& HitNormal, FName& BoneName, FHitResult& OutHit)` (engine): a sphere
/// swept against one component alone (`SweepComponent`, its body's `FBodyInstance::Sweep`).
pub struct ComponentSweep {
    function: Function,
    start: usize,
    end: usize,
    radius: usize,
    complex: usize,
    result: usize,
    hit: HitFields,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for ComponentSweep {}
unsafe impl Sync for ComponentSweep {}

/// Finds `K2_SphereTraceComponent`. After engine init.
pub fn component_sweep() -> Result<ComponentSweep, String> {
    const NAME: &str = "K2_SphereTraceComponent";
    let class =
        reflection::find_class("PrimitiveComponent").ok_or("class PrimitiveComponent not found")?;
    let function = Function::find(class, NAME)?;
    Ok(ComponentSweep {
        start: function.parameter("TraceStart", "StructProperty", 12)?,
        end: function.parameter("TraceEnd", "StructProperty", 12)?,
        radius: function.parameter("SphereRadius", "FloatProperty", 4)?,
        complex: function.parameter("bTraceComplex", "BoolProperty", 1)?,
        result: function.parameter("ReturnValue", "BoolProperty", 1)?,
        hit: HitFields::find(class, NAME)?,
        function,
    })
}

impl ComponentSweep {
    /// Where a sphere of `radius` cm moving from `start` to `end` first touches `component`'s
    /// simple collision (what moving bodies sweep against), if it does.
    ///
    /// # Safety
    /// `component` must be a live primitive component. Game thread only.
    pub unsafe fn first_hit(
        &self,
        component: *mut UObjectBase,
        start: Vector,
        end: Vector,
        radius: f32,
    ) -> Option<SweepHit> {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; each value goes to its parameter's reflected offset, the others
        // stay zero (no debug drawing).
        unsafe {
            reflection::put(&mut parameters, self.start, start);
            reflection::put(&mut parameters, self.end, end);
            reflection::put(&mut parameters, self.radius, radius);
            reflection::put(&mut parameters, self.complex, false);
            self.function.call(component, &mut parameters);
            if reflection::take::<u8>(&parameters, self.result) == 0 {
                return None;
            }
            Some(self.hit.read(&mut parameters))
        }
    }
}

/// `static bool SphereTraceSingleByProfile(const UObject* WorldContextObject, const FVector Start,
/// const FVector End, float Radius, FName ProfileName, bool bTraceComplex, const TArray<AActor*>&
/// ActorsToIgnore, EDrawDebugTrace::Type DrawDebugType, FHitResult& OutHit, bool bIgnoreSelf, ...)`
/// (engine): what a sphere moving through the world first touches that blocks a collision profile.
pub struct ProfileSweep {
    /// `UKismetSystemLibrary`'s default object, which static functions are called on.
    library: *mut UObjectBase,
    function: Function,
    world_context: usize,
    start: usize,
    end: usize,
    radius: usize,
    profile: usize,
    ignore_self: usize,
    result: usize,
    hit: HitFields,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for ProfileSweep {}
unsafe impl Sync for ProfileSweep {}

/// Finds `SphereTraceSingleByProfile`. After engine init.
pub fn profile_sweep() -> Result<ProfileSweep, String> {
    const LIBRARY: &str = "KismetSystemLibrary";
    const NAME: &str = "SphereTraceSingleByProfile";
    let class = reflection::find_class(LIBRARY).ok_or("class KismetSystemLibrary not found")?;
    let library = reflection::find_object("Default__KismetSystemLibrary", LIBRARY)
        .ok_or("KismetSystemLibrary's default object not found")?;
    let function = Function::find(class, NAME)?;
    Ok(ProfileSweep {
        library,
        world_context: function.parameter("WorldContextObject", "ObjectProperty", 8)?,
        start: function.parameter("Start", "StructProperty", 12)?,
        end: function.parameter("End", "StructProperty", 12)?,
        radius: function.parameter("Radius", "FloatProperty", 4)?,
        profile: function.parameter("ProfileName", "NameProperty", 8)?,
        ignore_self: function.parameter("bIgnoreSelf", "BoolProperty", 1)?,
        result: function.parameter("ReturnValue", "BoolProperty", 1)?,
        hit: HitFields::find(class, NAME)?,
        function,
    })
}

impl ProfileSweep {
    /// What a sphere of `radius` cm moving from `start` to `end` first touches that blocks the
    /// collision profile `profile` (an `FName`), against simple collision as moving bodies sweep,
    /// leaving out the actor `context` (`bIgnoreSelf`). Not kept to the editor's components (see
    /// `game::editor_trace`).
    ///
    /// # Safety
    /// `context` must be a live actor in the world to sweep. Game thread only.
    pub unsafe fn first_hit(
        &self,
        context: *mut UObjectBase,
        start: Vector,
        end: Vector,
        radius: f32,
        profile: [u8; 8],
    ) -> Option<SweepHit> {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; each value goes to its parameter's reflected offset, the others
        // stay zero (no actors to ignore, no debug drawing, simple collision).
        unsafe {
            reflection::put(&mut parameters, self.world_context, context);
            reflection::put(&mut parameters, self.start, start);
            reflection::put(&mut parameters, self.end, end);
            reflection::put(&mut parameters, self.radius, radius);
            reflection::put(&mut parameters, self.profile, profile);
            reflection::put(&mut parameters, self.ignore_self, true);
            self.function.call(self.library, &mut parameters);
            if reflection::take::<u8>(&parameters, self.result) == 0 {
                return None;
            }
            Some(self.hit.read(&mut parameters))
        }
    }
}
