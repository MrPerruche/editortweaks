//! A primitive component's collision, through reflection (UE 4.27). It is read from the
//! component's reflected `BodyInstance` (`FBodyInstance::CollisionEnabled`, `ObjectType` and
//! `CollisionResponses.ResponseToChannels`, one `ECollisionResponse` per channel), and changed
//! through `UPrimitiveComponent`'s Blueprint setters, which also update the physics engine. Also an
//! actor's primitive components (`AActor::K2_GetComponentsByClass`), and a channel no game system
//! uses (not one of the project's, `UCollisionProfile::DefaultChannelResponses`).

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::tarray::TArray;
use brickrust::ue::uclass::{UClass, UStruct};

use crate::game::reflection::{self, Function, Parameters, SingleCall};

/// Channels (`ECollisionChannel`), each with a response in a component's collision.
pub const CHANNELS: usize = 32;
/// `ECC_Visibility`: what the editor's clicks trace.
pub const VISIBILITY: u8 = 3;
/// The game's own channels, `ECC_GameTraceChannel1` to `18`.
const GAME_CHANNELS: core::ops::RangeInclusive<u8> = 14..=31;

/// `ECollisionResponse`.
pub const IGNORE: u8 = 0;
pub const BLOCK: u8 = 2;

/// `ECollisionEnabled::Type`, two bits: queries (traces, sweeps, overlaps) and physics.
pub const NO_COLLISION: u8 = 0;
const QUERIES: u8 = 1;

/// What a component collides with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Collision {
    /// `ECollisionEnabled`: none, queries, physics, or both.
    pub enabled: u8,
    /// The channel the component is to others' queries and bodies (`ECollisionChannel`).
    pub object_type: u8,
    /// Its response to each channel (`ECollisionResponse`).
    pub responses: [u8; CHANNELS],
}

impl Collision {
    /// Whether queries (traces, sweeps, overlaps) can find the component.
    pub fn has_queries(&self) -> bool {
        self.enabled & QUERIES != 0
    }
}

/// One call of `UPrimitiveComponent`'s collision setters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Enabled(u8),
    ObjectType(u8),
    AllResponses(u8),
    Response { channel: u8, response: u8 },
}

/// The setter calls that turn `from` into `to`. The responses are set one by one, or all at once
/// and then the exceptions, whichever takes fewer calls. Collision is turned off first and on last,
/// so the component never collides with more than `from` or `to` do.
pub fn changes(from: &Collision, to: &Collision) -> Vec<Change> {
    let enabled = (from.enabled != to.enabled).then_some(Change::Enabled(to.enabled));
    let adds = to.enabled & !from.enabled != 0;
    let mut changes = Vec::new();
    if !adds {
        changes.extend(enabled);
    }
    if from.object_type != to.object_type {
        changes.push(Change::ObjectType(to.object_type));
    }
    changes.extend(response_changes(&from.responses, &to.responses));
    if adds {
        changes.extend(enabled);
    }
    changes
}

fn response_changes(from: &[u8; CHANNELS], to: &[u8; CHANNELS]) -> Vec<Change> {
    let set = |channel: usize| Change::Response {
        channel: channel as u8,
        response: to[channel],
    };
    let one_by_one: Vec<Change> = (0..CHANNELS)
        .filter(|&channel| from[channel] != to[channel])
        .map(set)
        .collect();
    // The response most channels have, to set first for all of them.
    let common = [IGNORE, 1, BLOCK]
        .into_iter()
        .max_by_key(|&response| to.iter().filter(|&&other| other == response).count())
        .unwrap_or(IGNORE);
    let exceptions = (0..CHANNELS).filter(|&channel| to[channel] != common);
    if 1 + exceptions.clone().count() < one_by_one.len() {
        core::iter::once(Change::AllResponses(common))
            .chain(exceptions.map(set))
            .collect()
    } else {
        one_by_one
    }
}

/// The reflected fields and functions; see the module's documentation.
pub struct ComponentCollision {
    /// Offsets in a component of its body's `CollisionEnabled`, `ObjectType` and responses.
    enabled: usize,
    object_type: usize,
    responses: usize,
    set_enabled: SingleCall,
    set_object_type: SingleCall,
    set_all_responses: SingleCall,
    /// `SetCollisionResponseToChannel(ECollisionChannel Channel, ECollisionResponse NewResponse)`.
    set_response: Function,
    response_channel: usize,
    response_value: usize,
    /// `TArray<UActorComponent*> AActor::K2_GetComponentsByClass(TSubclassOf<UActorComponent>)`.
    components: Function,
    component_class: usize,
    components_result: usize,
    primitive: *const UClass,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for ComponentCollision {}
unsafe impl Sync for ComponentCollision {}

/// Finds the fields and functions. After engine init.
pub fn find() -> Result<ComponentCollision, String> {
    let primitive =
        reflection::find_class("PrimitiveComponent").ok_or("class PrimitiveComponent not found")?;
    let actor = reflection::find_class("Actor").ok_or("class Actor not found")?;
    // SAFETY: a live class record.
    let (body, body_type) = struct_field(unsafe { &(*primitive).ustruct }, "BodyInstance")?;
    let (responses, responses_type) = struct_field(body_type, "CollisionResponses")?;
    let byte = |name| {
        reflection::struct_field_offset(body_type, name, "ByteProperty", 1)
            .map_err(|reason| format!("BodyInstance: {reason}"))
    };
    let container = reflection::struct_field_offset(
        responses_type,
        "ResponseToChannels",
        "StructProperty",
        CHANNELS as i32,
    )
    .map_err(|reason| format!("CollisionResponses: {reason}"))?;
    let set_response = Function::find(primitive, "SetCollisionResponseToChannel")?;
    let components = Function::find(actor, "K2_GetComponentsByClass")?;
    Ok(ComponentCollision {
        enabled: body + byte("CollisionEnabled")?,
        object_type: body + byte("ObjectType")?,
        responses: body + responses + container,
        set_enabled: enum_setter(primitive, "SetCollisionEnabled", "NewType")?,
        set_object_type: enum_setter(primitive, "SetCollisionObjectType", "Channel")?,
        set_all_responses: enum_setter(
            primitive,
            "SetCollisionResponseToAllChannels",
            "NewResponse",
        )?,
        response_channel: enum_parameter(&set_response, "Channel")?,
        response_value: enum_parameter(&set_response, "NewResponse")?,
        set_response,
        component_class: components.parameter("ComponentClass", "ClassProperty", 8)?,
        components_result: components.parameter("ReturnValue", "ArrayProperty", 16)?,
        components,
        primitive,
    })
}

/// The offset of the struct field `name` of `ustruct`, and the struct it holds.
fn struct_field(
    ustruct: *const UStruct,
    name: &'static str,
) -> Result<(usize, *const UStruct), String> {
    // SAFETY: `ustruct` comes from reflection; property records live as long as their struct.
    unsafe {
        let property = reflection::find_property(ustruct, name);
        let held = if property.is_null() {
            core::ptr::null()
        } else {
            reflection::struct_type(property)
        };
        if held.is_null() {
            return Err(format!("struct field '{name}' not found"));
        }
        Ok(((*property).offset_internal as usize, held))
    }
}

/// A setter taking one enum (a byte, or an enum property of a byte).
fn enum_setter(
    class: *const UClass,
    name: &'static str,
    parameter: &'static str,
) -> Result<SingleCall, String> {
    SingleCall::find(class, name, parameter, "ByteProperty", 1)
        .or_else(|_| SingleCall::find(class, name, parameter, "EnumProperty", 1))
}

fn enum_parameter(function: &Function, parameter: &'static str) -> Result<usize, String> {
    function
        .parameter(parameter, "ByteProperty", 1)
        .or_else(|_| function.parameter(parameter, "EnumProperty", 1))
}

impl ComponentCollision {
    /// The component's collision now.
    ///
    /// # Safety
    /// `component` must be a live primitive component. Game thread only.
    pub unsafe fn read(&self, component: *const UObjectBase) -> Collision {
        // SAFETY: per the contract; the offsets come from reflection.
        unsafe {
            let base = component.cast::<u8>();
            Collision {
                enabled: base.add(self.enabled).read(),
                object_type: base.add(self.object_type).read(),
                responses: base.add(self.responses).cast::<[u8; CHANNELS]>().read(),
            }
        }
    }

    /// Gives the component the collision `to`; how many setters it called.
    ///
    /// # Safety
    /// As for [`Self::read`].
    pub unsafe fn set(&self, component: *mut UObjectBase, to: &Collision) -> usize {
        // SAFETY: per the contract.
        let changes = changes(&unsafe { self.read(component) }, to);
        for &change in &changes {
            // SAFETY: per the contract; each value is the setter parameter's enum.
            unsafe {
                match change {
                    Change::Enabled(enabled) => self.set_enabled.call(component, enabled),
                    Change::ObjectType(channel) => self.set_object_type.call(component, channel),
                    Change::AllResponses(response) => {
                        self.set_all_responses.call(component, response)
                    }
                    Change::Response { channel, response } => {
                        let mut parameters = Parameters::new();
                        reflection::put(&mut parameters, self.response_channel, channel);
                        reflection::put(&mut parameters, self.response_value, response);
                        self.set_response.call(component, &mut parameters);
                    }
                }
            }
        }
        changes.len()
    }

    /// The actor's primitive components.
    ///
    /// # Safety
    /// `actor` must be a live actor. Game thread only.
    pub unsafe fn primitive_components(&self, actor: *mut UObjectBase) -> Vec<*mut UObjectBase> {
        let mut parameters = Parameters::new();
        // SAFETY: per the contract; the class goes to its parameter, and the returned array was
        // allocated by the game for us, so it's ours to free (with the game's allocator).
        unsafe {
            reflection::put(&mut parameters, self.component_class, self.primitive);
            self.components.call(actor, &mut parameters);
            let mut list: TArray<*mut UObjectBase> =
                reflection::take(&parameters, self.components_result);
            let found = list
                .iter()
                .filter(|component| !component.is_null())
                .collect();
            list.free();
            found
        }
    }
}

/// The last of the game's channels (`ECC_GameTraceChannel1` to `18`) the project doesn't define:
/// none of `UCollisionProfile::DefaultChannelResponses`. No profile can name it, so no game system
/// queries it, and objects of its type are found by no query of another channel.
pub fn unused_channel() -> Result<u8, String> {
    const CLASS: &str = "CollisionProfile";
    let class = reflection::find_class(CLASS).ok_or(format!("class {CLASS} not found"))?;
    // SAFETY: a live class; its default object is the profile the engine uses
    // (`UCollisionProfile::Get`), with its config loaded.
    let profile = unsafe { reflection::default_object(class) }
        .ok_or(format!("{CLASS} has no default object"))?;
    let property = reflection::field(class, "DefaultChannelResponses", "ArrayProperty", 16)?;
    // SAFETY: a live property record; the array's elements are `FCustomChannelSetup`s, each
    // `struct_size` bytes with its channel at `channel`.
    unsafe {
        let setup = reflection::struct_type(reflection::array_inner(property));
        if setup.is_null() {
            return Err("DefaultChannelResponses: not an array of structs".into());
        }
        let channel = reflection::struct_field_offset(setup, "Channel", "ByteProperty", 1)?;
        let size = reflection::struct_size(setup);
        let list = profile
            .cast::<u8>()
            .add((*property).offset_internal as usize)
            .cast::<TArray<u8>>()
            .read();
        let defined: Vec<u8> = (0..usize::try_from(list.num).unwrap_or(0))
            .map(|index| list.data.add(index * size + channel).read())
            .collect();
        GAME_CHANNELS
            .rev()
            .find(|candidate| !defined.contains(candidate))
            .ok_or_else(|| "every game channel is defined".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUERY_ONLY: u8 = 1;

    const BRICK: Collision = Collision {
        enabled: 3,
        object_type: 15,
        responses: {
            let mut responses = [BLOCK; CHANNELS];
            responses[4] = IGNORE;
            responses[21] = IGNORE;
            responses
        },
    };

    fn apply(from: &Collision, changes: &[Change]) -> Collision {
        let mut to = *from;
        for change in changes {
            match *change {
                Change::Enabled(enabled) => to.enabled = enabled,
                Change::ObjectType(channel) => to.object_type = channel,
                Change::AllResponses(response) => to.responses = [response; CHANNELS],
                Change::Response { channel, response } => to.responses[channel as usize] = response,
            }
        }
        to
    }

    fn isolated() -> Collision {
        let mut responses = [IGNORE; CHANNELS];
        responses[31] = BLOCK;
        Collision {
            enabled: QUERY_ONLY,
            object_type: 31,
            responses,
        }
    }

    #[test]
    fn changes_lead_to_the_target_both_ways() {
        let there = changes(&BRICK, &isolated());
        assert_eq!(apply(&BRICK, &there), isolated());
        let back = changes(&isolated(), &BRICK);
        assert_eq!(apply(&isolated(), &back), BRICK);
    }

    #[test]
    fn many_responses_are_set_at_once() {
        // Ignore everything but one: all at once, then the one.
        let there = changes(&BRICK, &isolated());
        let responses = there
            .iter()
            .filter(|change| matches!(change, Change::AllResponses(_) | Change::Response { .. }))
            .count();
        assert_eq!(responses, 2, "{there:?}");
    }

    #[test]
    fn collision_goes_off_first_and_on_last() {
        assert_eq!(changes(&BRICK, &isolated())[0], Change::Enabled(QUERY_ONLY));
        assert_eq!(
            changes(&isolated(), &BRICK).last(),
            Some(&Change::Enabled(3))
        );
    }

    #[test]
    fn nothing_to_change() {
        assert!(changes(&BRICK, &BRICK).is_empty());
    }
}
