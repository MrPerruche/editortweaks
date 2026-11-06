//! The collision queries the game makes to place a vehicle spawned at the player's location
//! (`docs/re/map-in-editor.md`, "Current location"), made through the engine's Blueprint library
//! (`UKismetSystemLibrary::BoxTraceSingle` and `SphereTraceSingleByProfile`) and turned into the
//! game's by `game::editor_trace` (the channel and flags the library can't ask for).

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::fname::FName;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::game::editor_object::Vector;
use crate::game::editor_trace::{self, GameQuery};
use crate::game::kismet;
use crate::game::reflection::{self, BoolField, Function, Parameters, put, take};
use crate::prelude::log;

/// `ECC_GameTraceChannel13`, called `VehicleBlocking` (BRMK's `DefaultEngine.ini`): what stops a
/// vehicle being placed. `GetVehicleSpawnOffset` passes it as a constant.
const VEHICLE_BLOCKING: u32 = 26;
/// `ABaseCharacter::FakeBlockCollisionChannels` (`Brick`, `Train`) and `Water`: what the game's
/// ground sweep blocks on top of the `Pawn` profile's responses.
const GROUND_BLOCKS: &[u8] = &[15, 22, 23];
/// The radius of the game's ground sweep, in cm (`RestartPlayerAt`'s lambda, a constant).
const GROUND_RADIUS: f32 = 50.0;

/// What a sweep hit first.
#[derive(Clone, Copy, Debug)]
pub struct SweepHit {
    /// Where the shape's centre was when it hit.
    pub location: Vector,
    /// Where it touched, and the surface's normal there.
    pub impact_point: Vector,
    pub impact_normal: Vector,
    /// It started inside what it hit.
    pub start_penetrating: bool,
}

/// A `TArray<AActor*>` as the game lays it out, lending a slice.
#[derive(Clone, Copy)]
#[repr(C)]
struct ActorArray {
    data: *const *mut UObjectBase,
    count: i32,
    capacity: i32,
}

/// A trace's parameters, each at its reflected offset.
struct Trace {
    function: Function,
    world_context: usize,
    start: usize,
    end: usize,
    ignored: usize,
    ignore_self: usize,
    result: usize,
    location: usize,
    impact_point: usize,
    impact_normal: usize,
    /// Offset of `OutHit`, and its `bStartPenetrating` in it.
    out_hit: usize,
    start_penetrating: BoolField,
}

impl Trace {
    fn find(class: *const UClass, name: &'static str) -> Result<Self, String> {
        let function = Function::find(class, name)?;
        let found = reflection::find_function(class, name)?;
        // SAFETY: a live function record, its parameters linked to it.
        let (out_hit, hit): (usize, *const UStruct) = unsafe {
            let out_hit = reflection::find_property(found.cast(), "OutHit");
            if out_hit.is_null() || !reflection::is_kind(out_hit, "StructProperty") {
                return Err(format!("{name}: no OutHit"));
            }
            (
                (*out_hit).offset_internal as usize,
                reflection::struct_type(out_hit),
            )
        };
        let vector = |field| {
            reflection::struct_field_offset(hit, field, "StructProperty", 12)
                .map(|offset| out_hit + offset)
                .map_err(|reason| format!("HitResult: {reason}"))
        };
        Ok(Self {
            world_context: function.parameter("WorldContextObject", "ObjectProperty", 8)?,
            start: function.parameter("Start", "StructProperty", 12)?,
            end: function.parameter("End", "StructProperty", 12)?,
            ignored: function.parameter("ActorsToIgnore", "ArrayProperty", 16)?,
            ignore_self: function.parameter("bIgnoreSelf", "BoolProperty", 1)?,
            result: function.parameter("ReturnValue", "BoolProperty", 1)?,
            location: vector("Location")?,
            impact_point: vector("ImpactPoint")?,
            impact_normal: vector("ImpactNormal")?,
            out_hit,
            start_penetrating: reflection::bool_field(hit, "bStartPenetrating")?,
            function,
        })
    }

    /// Runs the trace from `start` to `end` with the rest of `parameters` set, leaving out `context`
    /// (an actor in the world to trace) and `ignored` actors, as `query` says.
    ///
    /// # Safety
    /// `context` must be a live actor and `ignored` live actors. Game thread only.
    unsafe fn run(
        &self,
        library: *mut UObjectBase,
        mut parameters: Parameters,
        context: *mut UObjectBase,
        ignored: &[*mut UObjectBase],
        start: Vector,
        end: Vector,
        query: GameQuery,
    ) -> (Option<SweepHit>, bool) {
        // SAFETY: per the contract; each value goes to its reflected offset, the others stay zero
        // (no debug drawing, the first trace channel, simple collision). The array only lends
        // `ignored` for the call: its fields are a `TArray`'s, and the thunk takes it by reference,
        // so nothing frees it.
        unsafe {
            put(&mut parameters, self.world_context, context);
            put(&mut parameters, self.start, start);
            put(&mut parameters, self.end, end);
            let count = ignored.len() as i32;
            put(
                &mut parameters,
                self.ignored,
                ActorArray {
                    data: ignored.as_ptr(),
                    count,
                    capacity: count,
                },
            );
            put(&mut parameters, self.ignore_self, true);
            let ((), seen) =
                editor_trace::as_game_query(query, || self.function.call(library, &mut parameters));
            if take::<u8>(&parameters, self.result) == 0 {
                return (None, seen);
            }
            let hit = parameters.as_mut_ptr().add(self.out_hit).cast_const();
            let found = SweepHit {
                location: take(&parameters, self.location),
                impact_point: take(&parameters, self.impact_point),
                impact_normal: take(&parameters, self.impact_normal),
                start_penetrating: self.start_penetrating.get(hit),
            };
            (Some(found), seen)
        }
    }
}

/// The game's queries for placing a vehicle.
pub struct SpawnTrace {
    /// `UKismetSystemLibrary`'s default object, which static functions are called on.
    library: *mut UObjectBase,
    /// `static bool BoxTraceSingle(WorldContextObject, Start, End, HalfSize, Orientation,
    /// TraceChannel, bTraceComplex, ActorsToIgnore, DrawDebugType, OutHit, bIgnoreSelf, ...)`.
    boxes: Trace,
    half_size: usize,
    orientation: usize,
    /// `static bool SphereTraceSingleByProfile(WorldContextObject, Start, End, Radius, ProfileName,
    /// bTraceComplex, ActorsToIgnore, DrawDebugType, OutHit, bIgnoreSelf, ...)`.
    spheres: Trace,
    radius: usize,
    profile: usize,
    /// `UCollisionProfile::Pawn_ProfileName`, what `GetCharacterCollisionResponseParams` asks for.
    pawn: FName,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for SpawnTrace {}
unsafe impl Sync for SpawnTrace {}

/// Finds the queries. After engine init.
pub fn find() -> Result<SpawnTrace, String> {
    const LIBRARY: &str = "KismetSystemLibrary";
    let class = reflection::find_class(LIBRARY).ok_or("class KismetSystemLibrary not found")?;
    let boxes = Trace::find(class, "BoxTraceSingle")?;
    let spheres = Trace::find(class, "SphereTraceSingleByProfile")?;
    Ok(SpawnTrace {
        library: reflection::find_object("Default__KismetSystemLibrary", LIBRARY)
            .ok_or("KismetSystemLibrary's default object not found")?,
        half_size: boxes.function.parameter("HalfSize", "StructProperty", 12)?,
        orientation: boxes
            .function
            .parameter("Orientation", "StructProperty", 12)?,
        radius: spheres.function.parameter("Radius", "FloatProperty", 4)?,
        profile: spheres
            .function
            .parameter("ProfileName", "NameProperty", 8)?,
        pawn: kismet::string_to_name("Pawn")?,
        boxes,
        spheres,
    })
}

impl SpawnTrace {
    /// The ground under `from`, as `ABrickPlayerController::SweepPawnOrMarker` finds it for a spawn:
    /// a sphere from 10 m above to 10 m below, as a character meets the world.
    ///
    /// # Safety
    /// `context` must be a live actor in the world to trace, and `ignored` live actors. Game thread
    /// only.
    pub unsafe fn ground(
        &self,
        context: *mut UObjectBase,
        ignored: &[*mut UObjectBase],
        from: Vector,
    ) -> Option<SweepHit> {
        const REACH: f32 = 1000.0;
        let [x, y, z] = from;
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; each value goes to its reflected offset.
        let (hit, seen) = unsafe {
            put(&mut parameters, self.radius, GROUND_RADIUS);
            put(&mut parameters, self.profile, self.pawn);
            self.spheres.run(
                self.library,
                parameters,
                context,
                ignored,
                [x, y, z + REACH],
                [x, y, z - REACH],
                GameQuery {
                    channel: None,
                    blocks: GROUND_BLOCKS,
                    find_initial_overlaps: false,
                    ignore_touches: true,
                },
            )
        };
        warn_unseen(seen);
        hit
    }

    /// What a box of half size `half`, turned by `rotation`, hits going from `start` to `end`,
    /// through what stops a vehicle being placed. `find_initial_overlaps` as the query should.
    ///
    /// # Safety
    /// As for [`Self::ground`].
    pub unsafe fn sweep_box(
        &self,
        context: *mut UObjectBase,
        ignored: &[*mut UObjectBase],
        start: Vector,
        end: Vector,
        half: Vector,
        rotation: Vector,
        find_initial_overlaps: bool,
    ) -> Option<SweepHit> {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; each value goes to its reflected offset.
        let (hit, seen) = unsafe {
            put(&mut parameters, self.half_size, half);
            put(&mut parameters, self.orientation, rotation);
            self.boxes.run(
                self.library,
                parameters,
                context,
                ignored,
                start,
                end,
                GameQuery {
                    channel: Some(VEHICLE_BLOCKING),
                    blocks: &[],
                    find_initial_overlaps,
                    ignore_touches: false,
                },
            )
        };
        warn_unseen(seen);
        hit
    }
}

/// A sweep the hook on the engine's didn't see didn't use the game's channel or flags.
fn warn_unseen(seen: bool) {
    static UNSEEN: std::sync::Once = std::sync::Once::new();
    if !seen {
        UNSEEN.call_once(|| {
            log!("a spawn query didn't go through the engine's sweep: Current location may be off")
        });
    }
}
