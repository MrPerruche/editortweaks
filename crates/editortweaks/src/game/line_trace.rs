//! Tracing a line through the world (`UKismetSystemLibrary::LineTraceSingle`, engine, called
//! through reflection like a Blueprint does): what it hits first, and whether that's a brick (and
//! which); or against one component only ([`ComponentTrace`]); or as a body with a collision
//! profile meets the world ([`ProfileTrace`], e.g. a projectile).

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::tshared::FWeakObjectPtr;

use crate::game::editor_object::Vector;
use crate::game::editor_trace;
use crate::game::reflection::{self, Function, Parameters};
use crate::game::weak_object;

/// `static bool LineTraceSingle(const UObject* WorldContextObject, const FVector Start, const
/// FVector End, ETraceTypeQuery TraceChannel, bool bTraceComplex, const TArray<AActor*>&
/// ActorsToIgnore, EDrawDebugTrace::Type DrawDebugType, FHitResult& OutHit, bool bIgnoreSelf,
/// ...)`, the trace channel `TraceTypeQuery1` (visibility, which the editor's clicks trace too).
pub struct LineTrace {
    /// `UKismetSystemLibrary`'s default object, which static functions are called on.
    library: *mut UObjectBase,
    function: Function,
    world_context: usize,
    start: usize,
    end: usize,
    complex: usize,
    result: usize,
    /// Offsets of the hit's `Distance` (from the start) and `Component` (a weak pointer) in the
    /// parameters.
    distance: usize,
    component: usize,
}

/// What a trace hit first.
pub struct Hit {
    /// How far from the start, in cm.
    pub distance: f32,
    /// The component hit, if it still exists.
    pub component: Option<*mut UObjectBase>,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for LineTrace {}
unsafe impl Sync for LineTrace {}

/// Finds `LineTraceSingle`. After engine init.
pub fn find() -> Result<LineTrace, String> {
    const LIBRARY: &str = "KismetSystemLibrary";
    let class = reflection::find_class(LIBRARY).ok_or("class KismetSystemLibrary not found")?;
    let library = reflection::find_object("Default__KismetSystemLibrary", LIBRARY)
        .ok_or("KismetSystemLibrary's default object not found")?;
    let function = Function::find(class, "LineTraceSingle")?;
    let hit = reflection::find_function(class, "LineTraceSingle")?;
    // SAFETY: a live function record, its parameters linked to it.
    let (out_hit, hit_struct) = unsafe {
        let out_hit = reflection::find_property(hit.cast(), "OutHit");
        if out_hit.is_null() || !reflection::is_kind(out_hit, "StructProperty") {
            return Err("LineTraceSingle: no OutHit".into());
        }
        (
            (*out_hit).offset_internal as usize,
            reflection::struct_type(out_hit),
        )
    };
    let hit_field = |name, kind, size| {
        reflection::struct_field_offset(hit_struct, name, kind, size)
            .map_err(|reason| format!("HitResult: {reason}"))
    };
    Ok(LineTrace {
        library,
        world_context: function.parameter("WorldContextObject", "ObjectProperty", 8)?,
        start: function.parameter("Start", "StructProperty", 12)?,
        end: function.parameter("End", "StructProperty", 12)?,
        complex: function.parameter("bTraceComplex", "BoolProperty", 1)?,
        result: function.parameter("ReturnValue", "BoolProperty", 1)?,
        distance: out_hit + hit_field("Distance", "FloatProperty", 4)?,
        component: out_hit + hit_field("Component", "WeakObjectProperty", 8)?,
        function,
    })
}

impl LineTrace {
    /// What the line from `start` to `end` first hits that blocks visibility (complex collision,
    /// like the editor's clicks), in the world of `world` (an object in it). Nothing is left out:
    /// the bricks the editor shows are its own components. While the editor's queries are kept to
    /// its own components (`game::editor_trace`, the map shown around it), so is this one.
    ///
    /// # Safety
    /// `world` must be a live object in the world to trace (e.g. an actor). Game thread only.
    pub unsafe fn first_hit(
        &self,
        world: *mut UObjectBase,
        start: Vector,
        end: Vector,
    ) -> Option<Hit> {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; each value goes to its parameter's reflected offset, the others
        // stay zero (an empty array, no debug drawing, the first trace channel, `bIgnoreSelf`
        // false).
        unsafe {
            reflection::put(&mut parameters, self.world_context, world);
            reflection::put(&mut parameters, self.start, start);
            reflection::put(&mut parameters, self.end, end);
            reflection::put(&mut parameters, self.complex, true);
            editor_trace::as_editor(|| self.function.call(self.library, &mut parameters));
            if reflection::take::<u8>(&parameters, self.result) == 0 {
                return None;
            }
            Some(Hit {
                distance: reflection::take(&parameters, self.distance),
                component: weak_object::resolve(reflection::take::<FWeakObjectPtr>(
                    &parameters,
                    self.component,
                )),
            })
        }
    }
}

impl Hit {
    /// Whether it hit a brick, or another object the editor shows (its component implements
    /// `IBrickEditorComponentInterface`, through which the editor's clicks find the object), rather
    /// than the world around.
    pub fn is_brick(&self) -> bool {
        // SAFETY: a live component (just resolved).
        self.component
            .is_some_and(|component| unsafe { is_editor_component(component) })
    }

    /// The editor object (e.g. a brick) whose component it hit, as the editor's clicks find it:
    /// `IBrickEditorComponentInterface::GetBrickEditorObject()` (BRMK 0xA76F0, inlined in the game)
    /// returns the interface's weak pointer, right after its vtable ([`EDITOR_OBJECT`]). Check that
    /// it's one of the editor's before using it.
    pub fn editor_object(&self) -> Option<*mut UObjectBase> {
        let component = self.component?;
        // SAFETY: a live component (just resolved), so its class is live; the interface lies within
        // it, with the weak pointer after its vtable pointer (a plain value, resolved safely).
        unsafe {
            let class = reflection::class_of(component.cast_const().cast());
            let offset = reflection::interface_offset(class, "BrickEditorComponentInterface")?;
            let pointer = component
                .cast::<u8>()
                .add(offset + EDITOR_OBJECT)
                .cast::<FWeakObjectPtr>()
                .read_unaligned();
            weak_object::resolve(pointer)
        }
    }
}

/// Whether `component` is one the editor shows (a brick's, or another editor object's): it
/// implements `IBrickEditorComponentInterface`, through which the editor's clicks find the object.
///
/// # Safety
/// `component` must be a live object.
pub unsafe fn is_editor_component(component: *mut UObjectBase) -> bool {
    // SAFETY: per the contract, so its class is live.
    unsafe {
        let class = reflection::class_of(component.cast_const().cast());
        reflection::interface_offset(class, "BrickEditorComponentInterface").is_some()
    }
}

/// Offset of the editor object's weak pointer in `IBrickEditorComponentInterface` (see
/// [`Hit::editor_object`]).
const EDITOR_OBJECT: usize = 8;

/// `static bool LineTraceSingleByProfile(const UObject* WorldContextObject, const FVector Start,
/// const FVector End, FName ProfileName, bool bTraceComplex, const TArray<AActor*>&
/// ActorsToIgnore, EDrawDebugTrace::Type DrawDebugType, FHitResult& OutHit, bool bIgnoreSelf,
/// ...)`: a line through the world as a body with that collision profile meets it.
pub struct ProfileTrace {
    library: *mut UObjectBase,
    function: Function,
    world_context: usize,
    start: usize,
    end: usize,
    profile: usize,
    ignore_self: usize,
    result: usize,
    /// Offsets of the hit's `Distance`, `ImpactNormal`, `Component` and `PhysMaterial` (weak
    /// pointers) in the parameters.
    distance: usize,
    normal: usize,
    component: usize,
    material: usize,
}

/// What a [`ProfileTrace`] hit first.
pub struct WorldHit {
    /// How far from the start, in cm.
    pub distance: f32,
    /// The surface's normal where it hit.
    pub normal: Vector,
    /// The component hit, if it still exists.
    pub component: Option<*mut UObjectBase>,
    /// The physical material hit, if any.
    pub material: Option<*mut UObjectBase>,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for ProfileTrace {}
unsafe impl Sync for ProfileTrace {}

/// Finds `LineTraceSingleByProfile`. After engine init.
pub fn profile_trace() -> Result<ProfileTrace, String> {
    const LIBRARY: &str = "KismetSystemLibrary";
    const NAME: &str = "LineTraceSingleByProfile";
    let class = reflection::find_class(LIBRARY).ok_or("class KismetSystemLibrary not found")?;
    let library = reflection::find_object("Default__KismetSystemLibrary", LIBRARY)
        .ok_or("KismetSystemLibrary's default object not found")?;
    let function = Function::find(class, NAME)?;
    let found = reflection::find_function(class, NAME)?;
    // SAFETY: a live function record, its parameters linked to it.
    let (out_hit, hit_struct) = unsafe {
        let out_hit = reflection::find_property(found.cast(), "OutHit");
        if out_hit.is_null() || !reflection::is_kind(out_hit, "StructProperty") {
            return Err(format!("{NAME}: no OutHit"));
        }
        (
            (*out_hit).offset_internal as usize,
            reflection::struct_type(out_hit),
        )
    };
    let hit_field = |name, kind, size| {
        reflection::struct_field_offset(hit_struct, name, kind, size)
            .map_err(|reason| format!("HitResult: {reason}"))
    };
    Ok(ProfileTrace {
        library,
        world_context: function.parameter("WorldContextObject", "ObjectProperty", 8)?,
        start: function.parameter("Start", "StructProperty", 12)?,
        end: function.parameter("End", "StructProperty", 12)?,
        profile: function.parameter("ProfileName", "NameProperty", 8)?,
        ignore_self: function.parameter("bIgnoreSelf", "BoolProperty", 1)?,
        result: function.parameter("ReturnValue", "BoolProperty", 1)?,
        distance: out_hit + hit_field("Distance", "FloatProperty", 4)?,
        normal: out_hit + hit_field("ImpactNormal", "StructProperty", 12)?,
        component: out_hit + hit_field("Component", "WeakObjectProperty", 8)?,
        material: out_hit + hit_field("PhysMaterial", "WeakObjectProperty", 8)?,
        function,
    })
}

impl ProfileTrace {
    /// What the line from `start` to `end` first hits that blocks the collision profile `profile`
    /// (an `FName`), against simple collision as moving bodies sweep, leaving out the actor
    /// `context` (`bIgnoreSelf`). Not kept to the editor's components (see `game::editor_trace`).
    ///
    /// # Safety
    /// `context` must be a live actor in the world to trace. Game thread only.
    pub unsafe fn first_hit(
        &self,
        context: *mut UObjectBase,
        start: Vector,
        end: Vector,
        profile: [u8; 8],
    ) -> Option<WorldHit> {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; each value goes to its parameter's reflected offset, the others
        // stay zero (no actors to ignore, no debug drawing, simple collision).
        unsafe {
            reflection::put(&mut parameters, self.world_context, context);
            reflection::put(&mut parameters, self.start, start);
            reflection::put(&mut parameters, self.end, end);
            reflection::put(&mut parameters, self.profile, profile);
            reflection::put(&mut parameters, self.ignore_self, true);
            self.function.call(self.library, &mut parameters);
            if reflection::take::<u8>(&parameters, self.result) == 0 {
                return None;
            }
            let weak = |offset| {
                weak_object::resolve(reflection::take::<FWeakObjectPtr>(&parameters, offset))
            };
            Some(WorldHit {
                distance: reflection::take(&parameters, self.distance),
                normal: reflection::take(&parameters, self.normal),
                component: weak(self.component),
                material: weak(self.material),
            })
        }
    }
}

/// `bool UPrimitiveComponent::K2_LineTraceComponent(FVector TraceStart, FVector TraceEnd, bool
/// bTraceComplex, bool bShowTrace, bool bPersistentShowTrace, FVector& HitLocation, FVector&
/// HitNormal, FName& BoneName, FHitResult& OutHit)` (engine): a line against one component alone.
pub struct ComponentTrace {
    function: Function,
    start: usize,
    end: usize,
    complex: usize,
    location: usize,
    normal: usize,
    result: usize,
}

/// Where a [`ComponentTrace`] met its component.
#[derive(Clone, Copy)]
pub struct SurfaceHit {
    pub location: Vector,
    /// The surface's normal there.
    pub normal: Vector,
}

// SAFETY: a reflection record living as long as the game; only used on the game thread.
unsafe impl Send for ComponentTrace {}
unsafe impl Sync for ComponentTrace {}

/// Finds `K2_LineTraceComponent`. After engine init.
pub fn component_trace() -> Result<ComponentTrace, String> {
    let class =
        reflection::find_class("PrimitiveComponent").ok_or("class PrimitiveComponent not found")?;
    let function = Function::find(class, "K2_LineTraceComponent")?;
    Ok(ComponentTrace {
        start: function.parameter("TraceStart", "StructProperty", 12)?,
        end: function.parameter("TraceEnd", "StructProperty", 12)?,
        complex: function.parameter("bTraceComplex", "BoolProperty", 1)?,
        location: function.parameter("HitLocation", "StructProperty", 12)?,
        normal: function.parameter("HitNormal", "StructProperty", 12)?,
        result: function.parameter("ReturnValue", "BoolProperty", 1)?,
        function,
    })
}

impl ComponentTrace {
    /// Where the line from `start` to `end` first meets `component` (its complex collision, like the
    /// editor's clicks), if it does.
    ///
    /// # Safety
    /// `component` must be a live primitive component. Game thread only.
    pub unsafe fn first_hit(
        &self,
        component: *mut UObjectBase,
        start: Vector,
        end: Vector,
    ) -> Option<Vector> {
        // SAFETY: per the contract.
        unsafe { self.first_hit_with(component, start, end, true) }
    }

    /// [`Self::first_hit`] against the component's complex collision, or its simple collision
    /// (what moving bodies sweep against).
    ///
    /// # Safety
    /// As for [`Self::first_hit`].
    pub unsafe fn first_hit_with(
        &self,
        component: *mut UObjectBase,
        start: Vector,
        end: Vector,
        complex: bool,
    ) -> Option<Vector> {
        // SAFETY: per the contract.
        unsafe { self.surface_hit(component, start, end, complex) }.map(|hit| hit.location)
    }

    /// [`Self::first_hit_with`], with the surface's normal there.
    ///
    /// # Safety
    /// As for [`Self::first_hit`].
    pub unsafe fn surface_hit(
        &self,
        component: *mut UObjectBase,
        start: Vector,
        end: Vector,
        complex: bool,
    ) -> Option<SurfaceHit> {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; each value goes to its parameter's reflected offset, the others
        // stay zero (no debug drawing).
        unsafe {
            reflection::put(&mut parameters, self.start, start);
            reflection::put(&mut parameters, self.end, end);
            reflection::put(&mut parameters, self.complex, complex);
            self.function.call(component, &mut parameters);
            (reflection::take::<u8>(&parameters, self.result) != 0).then(|| SurfaceHit {
                location: reflection::take(&parameters, self.location),
                normal: reflection::take(&parameters, self.normal),
            })
        }
    }
}
