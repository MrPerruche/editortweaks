//! Distinct brick names: the editor calls every scalable brick "Scalable brick" and every light
//! brick "Light brick", whatever their shape. This gives the bricks listed in [`names`] their own
//! names, and logs the bricks that still share one.
//!
//! The name comes from each kind of brick's static info (see `docs/re/brick-names.md`), so
//! changing it there changes every place the editor shows it. Only the editor's display changes.

mod names;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::game::brick_static_info::{self, StaticInfos};
use crate::game::kismet::{self, Texts};
use crate::prelude::*;

pub struct BrickNames;

struct Code {
    infos: StaticInfos,
    texts: Texts,
}

// SAFETY: reflection records living as long as the game; only used on the game thread.
unsafe impl Send for Code {}
unsafe impl Sync for Code {}

static CODE: OnceLock<Option<Code>> = OnceLock::new();
/// The last list of shared names logged, so each change is logged once.
static REPORTED: Mutex<String> = Mutex::new(String::new());

impl Feature for BrickNames {
    fn id(&self) -> &'static str {
        "brick_names"
    }

    fn on_engine_init(&self) {
        rename(false);
    }

    // Blueprint classes load with the maps that use them: more of them may be there each time.
    fn on_map_load(&self) {
        rename(true);
    }
}

fn code() -> Option<&'static Code> {
    CODE.get_or_init(|| {
        let found = brick_static_info::find().and_then(|infos| {
            Ok(Code {
                infos,
                texts: kismet::texts()?,
            })
        });
        found.map_err(|reason| log!("inactive: {reason}")).ok()
    })
    .as_ref()
}

/// A kind of brick as the editor shows it.
struct Shown {
    class: String,
    /// The name, with the size if the game appends one.
    name: String,
}

/// Gives every loaded kind of brick listed in [`names`] its name, and logs the ones still sharing
/// a name if `report`.
fn rename(report: bool) {
    let Some(code) = code() else { return };
    let mut shown = Vec::new();
    let mut renamed = 0;
    for kind in code.infos.blueprints() {
        // SAFETY: `blueprints` only returns live static infos; on the game thread.
        let (name, size) = unsafe {
            let mut name = code.infos.name(&code.texts, &kind);
            if let Some(wanted) = names::of(&kind.class)
                && name != wanted
            {
                code.infos.set_name(&code.texts, &kind, wanted);
                name = wanted.to_owned();
                renamed += 1;
            }
            (name, code.infos.size_in_name(&kind))
        };
        let name = match size {
            Some([x, y, z]) => format!("{name} [size {x} x {y} x {z}]"),
            None => name,
        };
        shown.push(Shown {
            class: kind.class,
            name,
        });
    }
    if renamed > 0 {
        log!("renamed {renamed} bricks");
    }
    if report {
        log_report(&shown);
    }
}

/// Logs the names several kinds of brick share and the entries of [`names`] without a brick, if
/// they differ from the last time.
fn log_report(shown: &[Shown]) {
    let mut by_name: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for brick in shown {
        by_name.entry(&brick.name).or_default().push(&brick.class);
    }
    let mut report = String::new();
    for (name, classes) in by_name.iter().filter(|(_, classes)| classes.len() > 1) {
        let _ = writeln!(report, "  '{name}': {}", classes.join(", "));
    }
    let missing: Vec<&str> = names::NAMES
        .iter()
        .map(|&(class, _)| class)
        .filter(|&class| !shown.iter().any(|brick| brick.class == class))
        .collect();
    if !missing.is_empty() {
        let _ = writeln!(report, "  no brick for entries: {}", missing.join(", "));
    }
    let mut reported = REPORTED.lock().unwrap_or_else(PoisonError::into_inner);
    if *reported != report {
        if !report.is_empty() {
            log!(
                "bricks sharing a name, to name in features/brick_names/names.rs:\n{}",
                report.trim_end()
            );
        }
        *reported = report;
    }
}
