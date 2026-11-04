//! Which saved item counts as "the autosave", the way the game treats its own `Vehicles\AutoSave`.
//!
//! One game function answers that for a `FUGCFileInfo`, by its `LocalItemId` (BRMK 0x331AA0, game
//! 1.11.7 RVA 0xE74BC0), and everything special about an autosave asks it: saving one prompts Save
//! As instead of writing over it, the browser shows it first and with its icon. So [`detour`] says
//! yes for one more id, and that item gets all of these. See `docs/re/ugc_browser.md`.

use std::sync::OnceLock;

use brickrust::ue::uclass::UStruct;

use crate::framework::hook;
use crate::framework::sig::{CodeShape, FunctionSig};
use crate::framework::timings;
use crate::game::reflection;
use crate::game::slider_text::FString;

/// The browser item's `UUGCItemWidget::UpdateItemWidget()`, one of the check's callers, through
/// its call of the check (field offsets wildcarded).
pub const CALLER: FunctionSig = FunctionSig {
    name: "UUGCItemWidget::UpdateItemWidget",
    pattern: "80 b9 ?? ?? 00 00 06 48 8b f9 76 0e 80 b9 ?? ?? 00 00 02 74 05 40 32 ed eb 03 40 b5 01 0f b6 89 ?? ?? 00 00 80 f9 01 74 35 80 e9 02 80 f9 02 76 2d 33 f6 45 33 c9 0f b6 97 ?? ?? 00 00 44 0f b6 c5 48 8b cf 40 88 74 24 20 e8 ?? ?? ?? ?? 48 8b 6c 24 48 48 8b 74 24 50 48 83 c4 30 5f c3 48 8d 8f ?? ?? 00 00 48 89 5c 24 40 e8 ?? ?? ?? ??",
};
/// Where [`CALLER`]'s match calls the check.
const CHECK_CALL_AT: usize = 106;
/// The check's start: compares the id's length with that of the game's "AutoSave".
pub const CHECK: CodeShape = CodeShape {
    name: "FUGCFileInfo is the dedicated autosave",
    shape: "48 83 ec 28 48 8b 15 ?? ?? ?? ?? 8b 41 18 3b c2 74 0d 03 c2 83 f8 01 0f 94 c0",
};

/// `info`: a `FUGCFileInfo`. The answer as a byte: the game's `bool`, set in the lowest byte only.
type CheckFn = unsafe extern "C" fn(info: *const u8) -> u8;

/// The id that counts too, and where a file info keeps its id.
struct Marked {
    id: &'static str,
    local_id_at: usize,
}

static MARKED: OnceLock<Marked> = OnceLock::new();
static ORIGINAL: OnceLock<CheckFn> = OnceLock::new();

/// Makes the game treat the saved item with local id `id` as its autosave. After engine init, once.
pub fn treat_as_autosave(id: &'static str) -> Result<(), String> {
    let info: *const UStruct = reflection::find_object("UGCFileInfo", "ScriptStruct")
        .ok_or("struct UGCFileInfo not found")?
        .cast_const()
        .cast();
    let size = size_of::<FString>() as i32;
    let local_id_at = reflection::struct_field_offset(info, "LocalItemId", "StrProperty", size)?;
    let found = CALLER.locate()?;
    // SAFETY: the pattern holds the whole call instruction.
    let target = unsafe { found.call_target(CHECK_CALL_AT) }
        .ok_or("the autosave check isn't called where expected")?;
    if !CHECK.matches(target) {
        return Err("the autosave check looks different".into());
    }
    let _ = MARKED.set(Marked { id, local_id_at });
    // SAFETY: `target` is the start of the check (shape checked), and the detour has its signature.
    unsafe { hook::install_typed(CHECK.name, target, detour as CheckFn, &ORIGINAL) }
}

/// The game's answer, or yes for the marked id.
unsafe extern "C" fn detour(info: *const u8) -> u8 {
    let _span = timings::span("game::ugc_autosave::detour");
    let original = *ORIGINAL.get().expect("hook recorded before use");
    // SAFETY: the original function with its own argument.
    let answer = unsafe { timings::game(CHECK.name, || original(info)) };
    if answer != 0 {
        return answer;
    }
    // SAFETY: the game's live file info; the id is its own string at a reflected offset, read and
    // not freed.
    let marked = MARKED.get().is_some_and(|marked| unsafe {
        (*info.add(marked.local_id_at).cast::<FString>())
            .to_string()
            .eq_ignore_ascii_case(marked.id)
    });
    u8::from(marked)
}
