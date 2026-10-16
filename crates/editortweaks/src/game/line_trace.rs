//! Tracing a line through the world (`UKismetSystemLibrary::LineTraceSingle`, engine, called
//! through reflection like a Blueprint does): what it hits first, and whether that's a brick.

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
        self.component.is_some_and(|component| {
            // SAFETY: a live component (just resolved), so its class is live.
            unsafe {
                let class = reflection::class_of(component.cast_const().cast());
                reflection::interface_offset(class, "BrickEditorComponentInterface").is_some()
            }
        })
    }
}
