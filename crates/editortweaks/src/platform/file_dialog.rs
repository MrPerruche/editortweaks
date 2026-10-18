//! The Windows "Open file" dialog. It runs on a thread of its own: the dialog has a message loop
//! that blocks its caller until it closes, which on the game thread would freeze the game. The
//! chosen file comes back through a channel the caller polls.

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use windows_sys::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST,
    OPENFILENAMEW,
};

/// Room for the chosen path, in UTF-16 units (long paths are allowed).
const PATH_BUFFER_LEN: usize = 32768;

/// Whether one of our dialogs is open. Windows allows several, but one at a time is plenty.
static DIALOG_OPEN: AtomicBool = AtomicBool::new(false);

/// A file type the dialog offers, e.g. `Filter { name: "Images (PNG, JPEG)", patterns: "*.png;*.jpg;*.jpeg" }`.
pub struct Filter {
    /// Text shown in the file type drop-down.
    pub name: &'static str,
    /// Patterns separated by semicolons.
    pub patterns: &'static str,
}

/// Opens the dialog on a thread of its own. The receiver gets the chosen file, or `None` if the
/// user cancelled or the dialog failed (if the dialog thread panics, the channel just closes).
///
/// `filters` are offered in order, followed by "All files"; `folder` is where the dialog starts.
/// Returns `None`, without opening anything, if one of our dialogs is already open.
pub fn open_file(
    title: &str,
    filters: &[Filter],
    folder: Option<&Path>,
) -> Option<Receiver<Option<PathBuf>>> {
    DIALOG_OPEN
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .ok()?;
    // Moved into the thread; if the thread never starts it is dropped with the closure.
    let open_guard = OpenGuard;

    // The thread needs owned data, not borrows.
    let title = to_wide(title);
    let filter = filter_string(filters);
    let folder: Option<Vec<u16>> =
        folder.map(|folder| folder.as_os_str().encode_wide().chain(Some(0)).collect());
    let (sender, receiver) = mpsc::channel();

    thread::Builder::new()
        .name("editortweaks-file-dialog".into())
        .spawn(move || {
            let _open_guard = open_guard;
            let chosen = show_dialog(&title, &filter, folder.as_deref());
            // The caller may have stopped listening.
            let _ = sender.send(chosen);
        })
        .ok()?;
    Some(receiver)
}

/// Marks our dialog as closed when dropped, whichever way the dialog thread ends.
struct OpenGuard;

impl Drop for OpenGuard {
    fn drop(&mut self) {
        DIALOG_OPEN.store(false, Ordering::Release);
    }
}

/// Shows the dialog and blocks until it closes. `title`, `filter` and `folder` are NUL-terminated.
fn show_dialog(title: &[u16], filter: &[u16], folder: Option<&[u16]>) -> Option<PathBuf> {
    let mut file = vec![0u16; PATH_BUFFER_LEN];
    // The owner keeps the dialog in front of the game. Found here because HWND is not `Send`.
    let owner = super::window::find_game_window().unwrap_or(core::ptr::null_mut());

    // SAFETY: OPENFILENAMEW is plain data for which all-zero is valid: every pointer null, every
    // number 0. The fields we need are set below.
    let mut ofn: OPENFILENAMEW = unsafe { core::mem::zeroed() };
    ofn.lStructSize = size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = owner;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.nFilterIndex = 1;
    ofn.lpstrFile = file.as_mut_ptr();
    ofn.nMaxFile = file.len() as u32;
    ofn.lpstrInitialDir = folder.map_or(core::ptr::null(), <[u16]>::as_ptr);
    ofn.lpstrTitle = title.as_ptr();
    // NOCHANGEDIR: the dialog must not change the game's working directory.
    ofn.Flags = OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;

    // SAFETY: `ofn` is initialised as the API requires, and the buffers it points to (`filter`,
    // `title`, `folder`, `file`) are NUL-terminated and outlive the call; `file` holds
    // `nMaxFile` u16s. Returns 0 on cancel or error.
    let accepted = unsafe { GetOpenFileNameW(&mut ofn) } != 0;
    accepted.then(|| path_from_wide(&file))
}

/// `text` as NUL-terminated UTF-16.
fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// The filter string Windows expects: `name NUL patterns NUL` for each filter, then an extra NUL.
/// "All files" is always added last.
fn filter_string(filters: &[Filter]) -> Vec<u16> {
    let all_files = Filter {
        name: "All files (*.*)",
        patterns: "*.*",
    };
    let mut wide = Vec::new();
    for filter in filters.iter().chain(Some(&all_files)) {
        wide.extend(filter.name.encode_utf16().chain(Some(0)));
        wide.extend(filter.patterns.encode_utf16().chain(Some(0)));
    }
    wide.push(0);
    wide
}

/// The path in a NUL-terminated UTF-16 buffer (everything up to the first NUL).
fn path_from_wide(buffer: &[u16]) -> PathBuf {
    let len = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    PathBuf::from(OsString::from_wide(&buffer[..len]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn filter_string_pairs_names_with_patterns_and_adds_all_files() {
        let filters = [Filter {
            name: "Images",
            patterns: "*.png;*.jpg",
        }];
        let expected = wide("Images\0*.png;*.jpg\0All files (*.*)\0*.*\0\0");
        assert_eq!(filter_string(&filters), expected);
    }

    #[test]
    fn filter_string_without_filters_is_just_all_files() {
        assert_eq!(filter_string(&[]), wide("All files (*.*)\0*.*\0\0"));
    }

    #[test]
    fn path_from_wide_stops_at_the_first_nul() {
        let mut buffer = wide("C:\\models\\car.glb");
        buffer.extend([0, 0, u16::from(b'x')]);
        assert_eq!(
            path_from_wide(&buffer),
            PathBuf::from("C:\\models\\car.glb")
        );
    }

    #[test]
    fn path_from_wide_accepts_a_buffer_without_nul() {
        assert_eq!(path_from_wide(&wide("a.png")), PathBuf::from("a.png"));
    }

    #[test]
    fn path_from_wide_of_an_empty_buffer_is_empty() {
        assert_eq!(path_from_wide(&[0, 0]), PathBuf::new());
    }
}
