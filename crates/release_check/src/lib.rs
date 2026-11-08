//! Decides whether to remind the player that a newer Editor Tweaks release is out.
//!
//! The mod asks GitHub's API (`GET /repos/<owner>/<repo>/releases/latest`) and hands the JSON
//! response body to this crate as a string. Release tags are named `vX.Y.Z`. There is no network
//! code and no JSON library here: just version parsing, a small scan for the tag name, and the
//! reminder rule.

use core::fmt;

/// The longest tag name, in bytes, that [`tag_name`] accepts.
const MAX_TAG_LEN: usize = 64;

/// A release's version, `major.minor.patch`.
///
/// The field order matters: the derived ordering compares `major` first, then `minor`, then
/// `patch`, which is exactly how versions are ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    /// The first number; changes for big releases.
    pub major: u32,
    /// The second number.
    pub minor: u32,
    /// The third number; changes for small fixes.
    pub patch: u32,
}

impl Version {
    /// Parses `X.Y.Z` (as Cargo's `CARGO_PKG_VERSION`) or a tag `vX.Y.Z` (also `V`).
    ///
    /// There must be exactly three parts of ASCII digits only (no sign, no spaces, no empty part,
    /// each fits in a `u32`). Anything else, such as a pre-release or build suffix (`-beta`, `+1`)
    /// or two or four parts, gives `None`.
    pub fn parse(text: &str) -> Option<Self> {
        let digits = text.strip_prefix(['v', 'V']).unwrap_or(text);
        let mut parts = digits.split('.');
        let major = parse_part(parts.next()?)?;
        let minor = parse_part(parts.next()?)?;
        let patch = parse_part(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
        })
    }
}

/// One number of a version: ASCII digits only, at least one, and it fits in a `u32`.
fn parse_part(part: &str) -> Option<u32> {
    if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

impl fmt::Display for Version {
    /// Writes the version as a tag, `vX.Y.Z`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The `tag_name` of a GitHub release JSON object (`releases/latest`'s body): the string value of
/// the first `"tag_name"` key.
///
/// This is a minimal scan, not a JSON parser: it finds `"tag_name"`, skips whitespace, expects
/// `:`, skips whitespace, expects `"`, and returns the text up to the next `"`. It gives `None` if
/// any step fails, if the value contains a backslash (tags never need escapes), or if the value is
/// longer than 64 bytes.
pub fn tag_name(json: &str) -> Option<&str> {
    const KEY: &str = "\"tag_name\"";
    let after_key = &json[json.find(KEY)? + KEY.len()..];
    let after_colon = after_key.trim_start().strip_prefix(':')?;
    let value_and_rest = after_colon.trim_start().strip_prefix('"')?;
    let value = &value_and_rest[..value_and_rest.find('"')?];
    if value.contains('\\') || value.len() > MAX_TAG_LEN {
        return None;
    }
    Some(value)
}

/// The newer release to remind about, if any: `latest` if it is newer than `current` and newer
/// than `skipped` (the release the player asked not to be reminded of again, if any).
pub fn to_remind(current: Version, latest: Version, skipped: Option<Version>) -> Option<Version> {
    let newer_than_current = latest > current;
    let newer_than_skipped = skipped.is_none_or(|skipped| latest > skipped);
    (newer_than_current && newer_than_skipped).then_some(latest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(major: u32, minor: u32, patch: u32) -> Version {
        Version {
            major,
            minor,
            patch,
        }
    }

    #[test]
    fn parses_plain_and_tagged_versions() {
        assert_eq!(Version::parse("0.1.0"), Some(v(0, 1, 0)));
        assert_eq!(Version::parse("v1.2.3"), Some(v(1, 2, 3)));
        assert_eq!(Version::parse("V10.20.30"), Some(v(10, 20, 30)));
        assert_eq!(
            Version::parse("4294967295.4294967295.4294967295"),
            Some(v(u32::MAX, u32::MAX, u32::MAX))
        );
    }

    #[test]
    fn rejects_malformed_versions() {
        for bad in [
            "",
            "v",
            "1.2",
            "1.2.3.4",
            "1.2.3-beta",
            "v1.2.3+1",
            "1..3",
            " 1.2.3",
            "1.2.3 ",
            "+1.2.3",
            "-1.2.3",
            "vv1.2.3",
            "1.2.x",
            "4294967296.0.0",
            "1.2.",
            ".1.2",
            "1.2.3.",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad:?} should not parse");
        }
    }

    #[test]
    fn orders_by_major_then_minor_then_patch() {
        assert!(v(1, 10, 0) > v(1, 9, 0));
        assert!(v(0, 2, 0) > v(0, 1, 9));
        assert!(v(2, 0, 0) > v(1, 99, 99));
        assert!(v(1, 2, 4) > v(1, 2, 3));
        assert_eq!(v(1, 2, 3), v(1, 2, 3));
    }

    #[test]
    fn parsed_versions_order_numerically_not_as_text() {
        let nine = Version::parse("1.9.0").unwrap();
        let ten = Version::parse("1.10.0").unwrap();
        assert!(ten > nine);
    }

    #[test]
    fn displays_as_a_tag() {
        assert_eq!(v(1, 2, 3).to_string(), "v1.2.3");
        assert_eq!(v(0, 1, 0).to_string(), "v0.1.0");
        let parsed = Version::parse("V10.20.30").unwrap();
        assert_eq!(Version::parse(&parsed.to_string()), Some(parsed));
    }

    const GITHUB_BODY: &str = r#"{
  "url": "https://api.github.com/repos/someone/editortweaks/releases/123456",
  "html_url": "https://github.com/someone/editortweaks/releases/tag/v1.2.3",
  "id": 123,
  "tag_name": "v1.2.3",
  "name": "Editor Tweaks 1.2.3",
  "draft": false,
  "body": "Fixes things.\r\n\"tag_name\": \"v9.9.9\""
}"#;

    #[test]
    fn finds_tag_in_a_pretty_printed_github_body() {
        assert_eq!(tag_name(GITHUB_BODY), Some("v1.2.3"));
    }

    #[test]
    fn finds_tag_in_compact_json() {
        assert_eq!(tag_name(r#"{"tag_name":"v1.0.0"}"#), Some("v1.0.0"));
    }

    #[test]
    fn finds_tag_with_whitespace_around_the_colon() {
        assert_eq!(tag_name("{\"tag_name\"  \n:\t \"v2.0.0\"}"), Some("v2.0.0"));
    }

    #[test]
    fn takes_the_first_tag_name_key() {
        let json = r#"{"tag_name": "v1.0.0", "tag_name": "v2.0.0"}"#;
        assert_eq!(tag_name(json), Some("v1.0.0"));
    }

    #[test]
    fn tag_name_fails_without_a_string_value() {
        assert_eq!(tag_name(r#"{"name": "x"}"#), None);
        assert_eq!(tag_name(""), None);
        assert_eq!(tag_name(r#"{"tag_name": null}"#), None);
        assert_eq!(tag_name(r#"{"tag_name": 5}"#), None);
        assert_eq!(tag_name(r#"{"tag_name""#), None);
        assert_eq!(tag_name(r#"{"tag_name" "v1.0.0"}"#), None);
    }

    #[test]
    fn tag_name_fails_on_an_unterminated_string() {
        assert_eq!(tag_name(r#"{"tag_name": "v1.0.0"#), None);
    }

    #[test]
    fn tag_name_fails_on_an_escaped_value() {
        assert_eq!(tag_name(r#"{"tag_name": "v1\"x"}"#), None);
    }

    #[test]
    fn tag_name_length_limit_is_64_bytes() {
        let ok = "v".repeat(64);
        let long = "v".repeat(65);
        assert_eq!(
            tag_name(&format!("{{\"tag_name\": \"{ok}\"}}")),
            Some(ok.as_str())
        );
        assert_eq!(tag_name(&format!("{{\"tag_name\": \"{long}\"}}")), None);
    }

    #[test]
    fn tag_name_handles_multibyte_text() {
        assert_eq!(tag_name("{\"é\": 1, \"tag_name\": \"vé\"}"), Some("vé"));
    }

    #[test]
    fn reminds_about_a_newer_release() {
        assert_eq!(to_remind(v(1, 0, 0), v(1, 1, 0), None), Some(v(1, 1, 0)));
    }

    #[test]
    fn does_not_remind_about_the_same_or_an_older_release() {
        assert_eq!(to_remind(v(1, 1, 0), v(1, 1, 0), None), None);
        assert_eq!(to_remind(v(1, 1, 0), v(1, 0, 9), None), None);
    }

    #[test]
    fn respects_the_skipped_release() {
        let current = v(1, 0, 0);
        let latest = v(1, 2, 0);
        // Skipped exactly this release.
        assert_eq!(to_remind(current, latest, Some(latest)), None);
        // Skipped an older release: the newer one is worth a reminder.
        assert_eq!(to_remind(current, latest, Some(v(1, 1, 0))), Some(latest));
        // Skipped a release newer than the latest (e.g. it was withdrawn).
        assert_eq!(to_remind(current, latest, Some(v(1, 3, 0))), None);
    }

    #[test]
    fn skipped_does_not_matter_when_not_newer_than_current() {
        assert_eq!(to_remind(v(2, 0, 0), v(1, 0, 0), Some(v(0, 5, 0))), None);
    }
}
