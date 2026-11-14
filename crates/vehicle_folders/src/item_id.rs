//! Checks on the game's item ids (`LocalItemId`: a vehicle's folder name in the vehicles folder).

/// Whether `id` can only name one vehicle's own folder inside the vehicles folder: not empty, not
/// `.` or `..`, no path separator, drive colon or control character (the browser's folder tiles
/// start with one, so a tile is never taken for a vehicle).
///
/// The game deletes a vehicle by deleting `<vehicles folder>/<id>` with everything in it, so an id
/// failing this must never reach that: an empty one would name the vehicles folder itself.
pub fn is_vehicle_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && !id.trim().is_empty()
        && !id
            .chars()
            .any(|c| matches!(c, '/' | '\\' | ':') || c.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_folder_names_are_vehicle_ids() {
        for id in [
            "491",
            "0",
            "My plane",
            "Planes.v2",
            "élan",
            "a..b",
            "3123456789",
        ] {
            assert!(is_vehicle_id(id), "{id:?}");
        }
    }

    #[test]
    fn anything_naming_more_than_one_folder_is_not() {
        for id in [
            "",
            " ",
            ".",
            "..",
            "/",
            "\\",
            "491/..",
            "..\\..",
            "C:",
            "C:\\Users",
            "a/b",
            "\u{1}ET:F/Planes",
            "\u{1}ET:U",
            "\u{1}ET:P",
            "tab\there",
            "nul\0",
        ] {
            assert!(!is_vehicle_id(id), "{id:?}");
        }
    }
}
