//! Every feature of the mod. To add one: create `features/<name>.rs` with a type implementing
//! [`Feature`], declare the module below and append it to [`ALL`].

use crate::framework::feature::Feature;

mod hello;

/// All features, in load order.
pub static ALL: &[&dyn Feature] = &[&hello::Hello];
