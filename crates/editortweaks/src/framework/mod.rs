//! Infrastructure shared by all features: the [`feature::Feature`] trait, lifecycle dispatch and
//! logging. Features never talk to the loader or install lifecycle hooks themselves.

pub mod feature;
pub mod lifecycle;
pub mod log;
