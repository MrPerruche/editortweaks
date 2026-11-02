//! Test builds that stop loading after a moment: `et.py build|install|package --expires <when>`
//! builds with `ET_EXPIRES` set (seconds since 1970, UTC), and past it [`allows_start`] says no, so
//! the mod starts nothing (no features, no hooks). Builds without it never expire.
//!
//! The PC's clock decides: it keeps an old build from being used by mistake, not on purpose.

use super::clock;
use super::log::log;

/// When this build expires (seconds since 1970, as text), for a test build.
const EXPIRES: Option<&str> = option_env!("ET_EXPIRES");

/// Whether this build may start now. Logs when a test build expires, or that it has.
pub fn allows_start() -> bool {
    let Some(text) = EXPIRES else {
        return true;
    };
    let Ok(expires) = text.trim().parse::<u64>() else {
        log!("this test build's expiry ({text}) is unreadable: not starting");
        return false;
    };
    let when = clock::utc_text(expires);
    if clock::now() >= expires {
        log!("this test build expired on {when}: not starting (ask for a newer build)");
        false
    } else {
        log!("test build: stops loading after {when}");
        true
    }
}
