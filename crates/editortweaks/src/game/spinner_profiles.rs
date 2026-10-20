//! Spinner bricks' cross-sections. A spinner is a 2D profile (one per `ESpinnerBrickShape`) swept
//! along an elliptic arc; the game keeps the profiles in a table it builds on first use, each shape
//! at a few levels of detail: level 0 for the visible mesh's first LOD, a coarser one for the
//! collision (`docs/re/selection.md`). Found in BRMK 1.11.6 (not exported: 0x493CE0, called by the
//! spinner mesh and body setup builders 0x479220 and 0x478B90), then in game 1.11.7 (RVA 0xF5AD90).
//!
//! ```text
//! const FSpinnerProfiles* FindProfiles(uint8 Shape, int32* QuarterTurns);
//! // Mirrored/turned shapes (2..11) map to a base shape and set *QuarterTurns (else untouched).
//! // Shipping: 8 (null + 8) for a shape not in the table.
//! struct FSpinnerProfiles { TArray<FLevel> Levels; uint8 CollisionLevel; };   // +0x00, +0x10
//! struct FLevel { TArray<FProfilePoint> Points; /* 8 more bytes */ };          // 0x18 bytes
//! struct FProfilePoint { float X, Y; /* 12 more bytes */ };                     // 0x14 bytes
//! ```

use brickrust::ue::coreuobject::UObjectBase;
use brickrust::ue::tarray::TArray;
use brickrust::ue::uclass::UStruct;

use crate::framework::sig::FunctionSig;
use crate::game::reflection;

/// The profile table lookup.
pub const FIND_PROFILES: FunctionSig = FunctionSig {
    name: "spinner profile lookup",
    // The switch mapping the shapes 2..11 to base shapes (`Shape - 2 > 9`), into its jump table.
    pattern: "83 c0 fe 83 f8 09 77 ?? 48 8d 0d",
};

type FindProfilesFn = unsafe extern "C" fn(shape: u8, quarter_turns: *mut i32) -> *const u8;

const LEVEL_SIZE: usize = 0x18;
const POINT_SIZE: usize = 0x14;
const COLLISION_LEVEL: usize = 0x10;
/// Sanity limits for what's read from the table.
const MAX_LEVELS: i32 = 16;
const MAX_POINTS: i32 = 4096;

/// A spinner's cross-sections, as `[x, y]` before the turn.
pub struct Profiles {
    /// Quarter turns counterclockwise the profiles are placed with.
    pub quarter_turns: i32,
    /// The visible mesh's (its first LOD).
    pub visible: Vec<[f32; 2]>,
    /// The collision's.
    pub collision: Vec<[f32; 2]>,
}

/// Reads spinners' cross-sections (after engine init).
pub struct SpinnerProfiles {
    find_profiles: FindProfilesFn,
    /// `SpinnerBrick`.
    class: *const UStruct,
    /// `USpinnerBrick::SpinnerShape`, a one-byte enum.
    shape: usize,
}

// SAFETY: game code and reflection records living as long as the game; only used on the game
// thread.
unsafe impl Send for SpinnerProfiles {}
unsafe impl Sync for SpinnerProfiles {}

impl SpinnerProfiles {
    pub fn find() -> Result<Self, String> {
        let function = FIND_PROFILES.resolve()?;
        let class = reflection::find_class("SpinnerBrick").ok_or("class SpinnerBrick not found")?;
        let shape = reflection::field_offset(class, "SpinnerShape", "EnumProperty", 1)
            .or_else(|_| reflection::field_offset(class, "SpinnerShape", "ByteProperty", 1))?;
        Ok(Self {
            // SAFETY: the function the pattern identifies has this signature (see the module).
            find_profiles: unsafe { core::mem::transmute::<*const u8, FindProfilesFn>(function) },
            class: class.cast(),
            shape,
        })
    }

    /// `brick`'s cross-sections, if it's a spinner and the table reads as expected.
    ///
    /// # Safety
    /// `brick` must be a live brick. Game thread only (the game builds the table there).
    pub unsafe fn of(&self, brick: *mut UObjectBase) -> Option<Profiles> {
        // SAFETY: per the contract; the shape's offset comes from reflection, and the table is
        // read within the counts it reports, after checking them.
        unsafe {
            if !reflection::is_a(brick, self.class) {
                return None;
            }
            let shape = brick.cast::<u8>().add(self.shape).read();
            let mut quarter_turns = 0;
            let profiles = (self.find_profiles)(shape, &mut quarter_turns);
            if (profiles as usize) < 0x10000 {
                return None;
            }
            let levels = profiles.cast::<TArray<u8>>().read();
            if !(1..=MAX_LEVELS).contains(&levels.num) || levels.data.is_null() {
                return None;
            }
            let collision = usize::from(profiles.add(COLLISION_LEVEL).read());
            if collision >= levels.num as usize {
                return None;
            }
            let level = |index: usize| -> Option<Vec<[f32; 2]>> {
                let points = levels
                    .data
                    .add(index * LEVEL_SIZE)
                    .cast::<TArray<u8>>()
                    .read();
                if !(3..=MAX_POINTS).contains(&points.num) || points.data.is_null() {
                    return None;
                }
                Some(
                    (0..points.num as usize)
                        .map(|point| {
                            points
                                .data
                                .add(point * POINT_SIZE)
                                .cast::<[f32; 2]>()
                                .read()
                        })
                        .collect(),
                )
            };
            Some(Profiles {
                quarter_turns,
                visible: level(0)?,
                collision: level(collision)?,
            })
        }
    }
}
