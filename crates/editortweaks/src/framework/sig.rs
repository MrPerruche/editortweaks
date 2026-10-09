//! Locating game functions by byte pattern, robustly.
//!
//! A [`FunctionSig`] pattern may match anywhere *inside* the function: the function's start is
//! then looked up in the exe's unwind table (`.pdata`), which lists every non-leaf function.
//! So code added before the pattern in a game update doesn't break it, unlike "match + offset".
//! Patterns must match exactly once; `et.py sigcheck` checks them against a new game build.

use std::sync::OnceLock;

use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

/// A byte pattern identifying one function in the game exe, e.g. `"48 8b ?? 0f 2f"`.
/// Keep the `name: "...", pattern: "..."` shape: `et.py sigcheck` parses these declarations.
pub struct FunctionSig {
    pub name: &'static str,
    pub pattern: &'static str,
}

/// Where a [`FunctionSig`] matched.
#[derive(Clone, Copy, Debug)]
pub struct SigMatch {
    /// The first byte of the match, e.g. to read an offset from the matched code.
    pub address: *const u8,
    /// The start of the function containing it.
    pub function: *const u8,
}

impl FunctionSig {
    /// Address of the function containing the pattern's single match.
    pub fn resolve(&self) -> Result<*const u8, String> {
        self.locate().map(|found| found.function)
    }

    /// The pattern's single match and the function containing it.
    pub fn locate(&self) -> Result<SigMatch, String> {
        let pattern = parse_pattern(self.pattern).map_err(|e| format!("{}: {e}", self.name))?;
        let image = GameImage::get();
        let mut matches = image
            .code_sections()
            .flat_map(|code| find_all(code, &pattern));
        let first = matches
            .next()
            .ok_or_else(|| format!("{}: pattern not found", self.name))?;
        if matches.next().is_some() {
            return Err(format!("{}: pattern matches more than once", self.name));
        }
        let rva = (first as usize - image.base as usize) as u32;
        let start = image.function_containing(rva).ok_or_else(|| {
            format!(
                "{}: match at RVA 0x{rva:X} isn't inside a known function",
                self.name
            )
        })?;
        Ok(SigMatch {
            address: first,
            // SAFETY: `start` is an RVA from the image's own exception directory.
            function: unsafe { image.base.add(start as usize) },
        })
    }
}

/// Whether `address` is where a function of the game exe starts (per its unwind table), e.g. to
/// check a code pointer read from game data before hooking it.
pub fn is_function_start(address: *const u8) -> bool {
    let image = GameImage::get();
    let Some(rva) = (address as usize)
        .checked_sub(image.base as usize)
        .and_then(|rva| u32::try_from(rva).ok())
    else {
        return false;
    };
    image
        .functions
        .binary_search_by_key(&rva, |function| function[0])
        .is_ok()
}

/// The game's main module as mapped in memory.
struct GameImage {
    base: *const u8,
    /// (rva, size) of executable sections.
    code: Vec<(u32, u32)>,
    /// (begin, end, unwind info) RVAs from the exception directory, sorted by begin.
    functions: &'static [[u32; 3]],
}

// SAFETY: the image is mapped for the whole process lifetime and only read.
unsafe impl Send for GameImage {}
unsafe impl Sync for GameImage {}

impl GameImage {
    fn get() -> &'static GameImage {
        static IMAGE: OnceLock<GameImage> = OnceLock::new();
        // SAFETY: the main module's PE headers are mapped and valid while the process runs.
        IMAGE.get_or_init(|| unsafe {
            Self::parse(GetModuleHandleW(core::ptr::null()) as *const u8)
        })
    }

    unsafe fn parse(base: *const u8) -> GameImage {
        // SAFETY (whole fn): reads PE header fields at offsets defined by the PE format.
        unsafe {
            let read_u16 = |off: usize| (base.add(off) as *const u16).read_unaligned();
            let read_u32 = |off: usize| (base.add(off) as *const u32).read_unaligned();
            let nt = read_u32(0x3C) as usize;
            let section_count = read_u16(nt + 6) as usize;
            let optional = nt + 24;
            let optional_size = read_u16(nt + 20) as usize;
            let exception_dir = optional + 112 + 3 * 8; // PE32+ data directory #3
            let (pdata_rva, pdata_size) = (read_u32(exception_dir), read_u32(exception_dir + 4));

            const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;
            let code = (0..section_count)
                .map(|i| optional + optional_size + i * 40)
                .filter(|&s| read_u32(s + 36) & IMAGE_SCN_MEM_EXECUTE != 0)
                .map(|s| (read_u32(s + 12), read_u32(s + 8)))
                .collect();
            let functions = std::slice::from_raw_parts(
                base.add(pdata_rva as usize) as *const [u32; 3],
                pdata_size as usize / 12,
            );
            GameImage {
                base,
                code,
                functions,
            }
        }
    }

    fn code_sections(&self) -> impl Iterator<Item = &'static [u8]> + '_ {
        self.code.iter().map(|&(rva, size)| {
            // SAFETY: executable sections are mapped and readable for `size` bytes.
            unsafe { std::slice::from_raw_parts(self.base.add(rva as usize), size as usize) }
        })
    }

    /// Start RVA of the function containing `rva`, following chained entries (code the compiler
    /// split off into separate chunks) back to the primary function.
    fn function_containing(&self, rva: u32) -> Option<u32> {
        let index = self
            .functions
            .partition_point(|f| f[0] <= rva)
            .checked_sub(1)?;
        let [begin, end, mut unwind] = self.functions[index];
        if rva >= end {
            return None;
        }
        let mut start = begin;
        for _ in 0..8 {
            // SAFETY: unwind info RVAs come from the exception directory of this image.
            unsafe {
                let info = self.base.add(unwind as usize);
                const UNW_FLAG_CHAININFO: u8 = 0x4;
                if (*info >> 3) & UNW_FLAG_CHAININFO == 0 {
                    return Some(start);
                }
                let code_count = *info.add(2) as usize;
                let parent = info.add(4 + 2 * (code_count + (code_count & 1))) as *const [u32; 3];
                [start, _, unwind] = parent.read_unaligned();
            }
        }
        Some(start)
    }
}

fn parse_pattern(pattern: &str) -> Result<Vec<Option<u8>>, String> {
    pattern
        .split_whitespace()
        .map(|token| match token {
            "?" | "??" => Ok(None),
            hex => u8::from_str_radix(hex, 16)
                .map(Some)
                .map_err(|_| format!("bad pattern byte '{hex}'")),
        })
        .collect()
}

/// Every position in `haystack` where `pattern` matches.
fn find_all<'a>(
    haystack: &'a [u8],
    pattern: &'a [Option<u8>],
) -> impl Iterator<Item = *const u8> + 'a {
    // Anchor on the first fixed byte so most positions are rejected by a single comparison.
    let (anchor, anchor_byte) = pattern
        .iter()
        .enumerate()
        .find_map(|(i, b)| b.map(|b| (i, b)))
        .unwrap_or((0, 0));
    let last_start = haystack.len().saturating_sub(pattern.len());
    (0..=last_start)
        .filter(move |&pos| haystack[pos + anchor] == anchor_byte)
        .filter(move |&pos| {
            pattern
                .iter()
                .zip(&haystack[pos..])
                .all(|(want, have)| want.is_none_or(|w| w == *have))
        })
        .map(move |pos| haystack[pos..].as_ptr())
}
