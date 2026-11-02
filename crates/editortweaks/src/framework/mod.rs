//! Infrastructure shared by all features: the [`feature::Feature`] trait, lifecycle dispatch and
//! logging. Features never talk to the loader or install lifecycle hooks themselves.

pub mod clock;
pub mod expiry;
pub mod feature;
pub mod hook;
pub mod lifecycle;
pub mod log;
pub mod patch;
pub mod settings;
pub mod sig;
pub mod timings;
