//! What the editor's panel allows as the source bricks of a vehicle input channel
//! (`FVehicleInputChannel::SourceBricks`, the bricks whose output drives a seat/axis input), for
//! coop to refuse a peer's `Set` that the panel could not have produced. Only in builds with the
//! `coop` feature. How it was found: `docs/re/coop/archive-addresses.md`, "Checks the panel makes".
//!
//! `EBrickEditorObjectAllowed FVehicleInputChannel::IsSourceBrickAllowed(const
//! TArray<FBrickEditorObjectPtr>& Sources, const UBrickEditorObject* Candidate) const` (BRMK
//! 0x497040, game 1.11.7 RVA 0xF5BAB0) is the panel's filter. x64: `rcx` the channel (the
//! `FVehicleInputChannel` struct inside the brick), `rdx` the sources array (only read), `r8` the
//! candidate; `al` gets 0 (allowed), 1 (allowed, but another seat is already a source, which
//! `PostModifyBrickProperty` prunes) or 2 (refused). It reads the channel's `InputAxis` byte, and
//! the array only through its data pointer and count (elements are 12 bytes apart,
//! `FBrickEditorObjectPtr` = `FWeakObjectPtr` + 2-byte `FBrickEditorObjectID`, padded; the helper
//! at 0xF49490 resolves each element's weak pointer and ignores the id).
//!
//! The panel's source count limit is `GetMaxNumSourceBricks` (game 0xF5A1F0): -1 (unlimited) if
//! `InputAxis` is `EVehicleInputAxis::Custom`, else 1. It is a leaf function (no unwind entry, so
//! no [`FunctionSig`]) taking the panel's property container, not the channel, so [`InputSources`]
//! applies the same rule itself, with the `InputAxis` offset and the `Custom` value read from
//! reflection and cross-checked against the constants in `IsSourceBrickAllowed`'s code.
//!
//! Intended use, in coop's apply code once a peer's `Set` of a channel's sources is decoded:
//!
//! ```ignore
//! // `channel` = brick base + the offset of the struct property holding the channel, written
//! // with the proposed `InputAxis` already; `objects` = the bricks the peer's pointers resolve to
//! // (null where one doesn't resolve).
//! if let Some(reason) = unsafe { input_sources.refusal(channel, &objects) } {
//!     return Err(reason); // the game's panel could not have produced this
//! }
//! ```

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::tarray::TArray;
use brickrust::ue::tshared::FWeakObjectPtr;
use brickrust::ue::uclass::UStruct;

use crate::framework::sig::FunctionSig;
use crate::game::{reflection, weak_object};

/// `IsSourceBrickAllowed` (member form), by its prologue up to the "is the candidate a brick" test.
/// Unique in game 1.11.7; the function has an unwind entry, so a match gives its start.
pub const IS_SOURCE_BRICK_ALLOWED: FunctionSig = FunctionSig {
    name: "FVehicleInputChannel::IsSourceBrickAllowed",
    pattern: "48 89 5c 24 08 48 89 74 24 10 4c 89 44 24 18 57 48 83 ec 20 49 8b d8 48 8b f2 48 8b f9 4d 85 c0 0f 84 ?? ?? 00 00 e8 ?? ?? ?? ?? 4c 8b 43 10 4c 8d 48 30 48 63 40 38 41 3b 40 38 0f 8f",
};

/// The same function's `InputAxis == Custom` test, then the "another seat is a source" call:
/// `cmp byte ptr [rdi + InputAxis], Custom`. Holes: the `InputAxis` offset (+2) and the value (+3).
pub const INPUT_AXIS_TEST: FunctionSig = FunctionSig {
    name: "FVehicleInputChannel::IsSourceBrickAllowed (InputAxis test)",
    pattern: "80 7f ?? ?? 74 ?? 48 8d 54 24 40 48 8b ce e8",
};

/// Bytes of an `FBrickEditorObjectPtr`; checked against reflection in [`find`].
const ELEMENT_SIZE: usize = 12;

type IsSourceBrickAllowedFn = unsafe extern "C" fn(
    channel: *const u8,
    sources: *const TArray<[u8; ELEMENT_SIZE]>,
    candidate: *const UObjectBase,
) -> u8;

/// The game's rules for a channel's source bricks.
pub struct InputSources {
    is_allowed: IsSourceBrickAllowedFn,
    /// Where `InputAxis` (a byte) is in `FVehicleInputChannel`.
    axis_offset: usize,
    /// `EVehicleInputAxis::Custom`: the only axis taking several sources, and any brick with an output.
    custom_axis: u8,
}

// SAFETY: a function and plain numbers; the function is only called on the game thread.
unsafe impl Send for InputSources {}
unsafe impl Sync for InputSources {}

/// Finds the game's check, verifying it reads `InputAxis` where reflection says it is.
pub fn find() -> Result<InputSources, String> {
    let function = IS_SOURCE_BRICK_ALLOWED.resolve()?;
    let test = INPUT_AXIS_TEST.locate()?;
    if test.function != function {
        return Err(format!(
            "{} isn't inside {}",
            INPUT_AXIS_TEST.name, IS_SOURCE_BRICK_ALLOWED.name
        ));
    }
    // SAFETY: the pattern matched, so its first four bytes (`cmp byte ptr [rdi + disp8], imm8`)
    // are there.
    let (code_offset, code_custom) = unsafe { (*test.address.add(2), *test.address.add(3)) };

    let channel = reflection::find_object("VehicleInputChannel", "ScriptStruct")
        .ok_or("struct VehicleInputChannel not found")?
        .cast::<UStruct>()
        .cast_const();
    let axis_offset = reflection::struct_field_offset(channel, "InputAxis", "EnumProperty", 1)
        .or_else(|_| reflection::struct_field_offset(channel, "InputAxis", "ByteProperty", 1))?;
    let custom = reflection::enum_value("EVehicleInputAxis", "EVehicleInputAxis::Custom")?;
    let custom_axis = u8::try_from(custom).map_err(|_| "EVehicleInputAxis::Custom isn't a byte")?;
    if usize::from(code_offset) != axis_offset || code_custom != custom_axis {
        return Err(format!(
            "{} tests byte {code_offset:#x} for {code_custom:#x}, reflection says {axis_offset:#x} and {custom_axis:#x}",
            IS_SOURCE_BRICK_ALLOWED.name
        ));
    }

    let pointer = reflection::find_object("BrickEditorObjectPtr", "ScriptStruct")
        .ok_or("struct BrickEditorObjectPtr not found")?
        .cast::<UStruct>()
        .cast_const();
    // SAFETY: a live struct (found by its class' name).
    let size = unsafe { reflection::struct_size(pointer) };
    if size != ELEMENT_SIZE {
        return Err(format!(
            "BrickEditorObjectPtr is {size} bytes, not {ELEMENT_SIZE}"
        ));
    }

    Ok(InputSources {
        // SAFETY: the function the pattern is in, with the signature above (BRMK's export, x64
        // calling convention; the disassembly reads rcx, rdx and r8 and returns in al).
        is_allowed: unsafe { core::mem::transmute::<*const u8, IsSourceBrickAllowedFn>(function) },
        axis_offset,
        custom_axis,
    })
}

impl InputSources {
    /// How many sources the channel at `channel` takes, like the game's `GetMaxNumSourceBricks`:
    /// 1, or without a limit on the `Custom` axis.
    ///
    /// # Safety
    /// `channel` must point to a readable `FVehicleInputChannel`.
    pub unsafe fn max_sources(&self, channel: *const u8) -> usize {
        // SAFETY: per the contract, `InputAxis` lies inside the struct.
        let axis = unsafe { channel.add(self.axis_offset).read() };
        if axis == self.custom_axis {
            usize::MAX
        } else {
            1
        }
    }

    /// Why the game's panel could not have produced `sources` as the source bricks of the channel
    /// at `channel`, or `None` if it could (or its pruning would leave them): there are more than
    /// the axis takes, or the game's `IsSourceBrickAllowed` says no (a result of 2, or of 1 for a
    /// second seat) for one of them, judged against the proposed array itself. A null entry (a
    /// peer's pointer that doesn't resolve) is refused, as the game refuses a dead weak pointer.
    ///
    /// The channel's `InputAxis` is read from memory, so write the proposed one first, or judge
    /// the old one on purpose. Nothing is changed.
    ///
    /// # Safety
    /// `channel` must point to a live `FVehicleInputChannel` (brick base + the struct property's
    /// offset); every non-null entry of `sources` a live `UBrickEditorObject`. Game thread only.
    pub unsafe fn refusal(
        &self,
        channel: *const u8,
        sources: &[*mut UObjectBase],
    ) -> Option<String> {
        // SAFETY: per the contract.
        let max = unsafe { self.max_sources(channel) };
        if sources.len() > max {
            return Some(format!(
                "{} source bricks, the channel takes {max}",
                sources.len()
            ));
        }
        // The array the game reads: ours (it only reads it, never frees or resizes it), alive
        // until the end of this function.
        let mut elements = Vec::with_capacity(sources.len());
        for &source in sources {
            if source.is_null() {
                return Some("a source brick that doesn't exist".into());
            }
            // SAFETY: a live object per the contract.
            match unsafe { weak_object::new_game_pointer(source) } {
                Ok(weak) => elements.push(element(weak)),
                Err(reason) => {
                    return Some(format!("a source brick without a weak pointer: {reason}"));
                }
            }
        }
        let array = TArray {
            data: elements.as_mut_ptr(),
            num: i32::try_from(elements.len()).ok()?,
            max: i32::try_from(elements.capacity()).ok()?,
        };
        for &source in sources {
            // SAFETY: the game's function with its own arguments: the channel and a live candidate
            // per the contract, and our array, which it only reads.
            let verdict = unsafe { (self.is_allowed)(channel, &array, source) };
            match verdict {
                0 => {}
                1 => return Some("a second seat as a source".into()),
                _ => return Some("a brick that can't be a source here".into()),
            }
        }
        None
    }
}

/// An `FBrickEditorObjectPtr` for `weak`: the weak pointer, then the (unused by the check) id as 0.
fn element(weak: FWeakObjectPtr) -> [u8; ELEMENT_SIZE] {
    let mut bytes = [0u8; ELEMENT_SIZE];
    bytes[..4].copy_from_slice(&weak.object_index.to_le_bytes());
    bytes[4..8].copy_from_slice(&weak.object_serial_number.to_le_bytes());
    bytes
}
