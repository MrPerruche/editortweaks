//! Members seeing the subhost's references (`docs/coop-references.md`). The protocol and its limits
//! are `coop::references`, the cooked copies `model_io::shared`; this runs them in a session, cooking
//! and checking on worker threads so the game never waits.
//!
//! Subhost: every moment, the open vehicle's references (`references::received::shared_now`) are
//! measured, fitted under each level in use and cooked; each level's list goes to its members, and
//! what they ask for goes out a chunk at a time.
//!
//! Member: the quality setting says the level. What arrives is checked by [`model_io::shared::check`]
//! and written to the references' session folder; what's ready shows (`references::received::show`).
//! The player's changes of them go to the subhost ([`EDIT_PACE`] at most), which makes them as its own
//! (`references::received::apply_edits`): its next lists tell everyone.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use ::coop::PeerId;
use ::coop::Session;
use ::coop::references::{
    Edit, Entry, ImageLook, Kind, Level, Receiving, Sharing, Size, Skip, Status, Target, coalesce,
    split,
};
use model_io::image;
use model_io::plane::ImagePivot;
use model_io::readers::{self, Format};
use model_io::shared::{self, Aim, Checked, Cooked, Limits};
use model_io::sidecar::{self, ImageSettings, Reference, Source, SourceMode};
use model_io::space::ImportSettings;

use super::Wire;
use super::log::coop_log;
use crate::features::references::received;
use crate::framework::settings::{Alternative, Choice};
use ::coop::Transport;

/// What the host's references may cost a member's game (`docs/coop-references.md`). High is
/// reasonable on any PC.
pub static QUALITY: Choice = Choice::new(
    "CoopReferenceQuality",
    "Host's references",
    "Safeguard which reduces the quality of a model if it is too high, in order to save bandwidth and performance. Extreme is only recommended for huge, trusted projects.",
    &[
        Alternative {
            key: "Off",
            name: "Off",
        },
        Alternative {
            key: "High",
            name: "High",
        },
        Alternative {
            key: "VeryHigh",
            name: "Very high",
        },
        Alternative {
            key: "Extreme",
            name: "Extreme",
        },
    ],
    1,
);

/// How often a subhost looks at its references again.
const LOOK_EVERY: Duration = Duration::from_millis(500);
/// How often a member sends its changes of the subhost's references, at most (a slider dragged
/// changes them every frame; the subhost allows 50 messages a second).
const EDIT_PACE: Duration = Duration::from_millis(100);
/// Members' messages of changes a subhost keeps while it isn't in the session's vehicle, at most.
const MAX_WAITING_EDITS: usize = 1000;
/// How often a subhost logs how fast its data leaves, while any does.
const LOG_EVERY: Duration = Duration::from_secs(30);
/// A member writes its references along with the session's starting autosave once they've stopped
/// changing for this long.
const SETTLE: Duration = Duration::from_secs(5);

/// The level the setting asks for.
fn chosen_level() -> Option<Level> {
    match QUALITY.get() {
        1 => Some(Level::High),
        2 => Some(Level::VeryHigh),
        3 => Some(Level::Extreme),
        _ => None,
    }
}

/// A session's sharing of references: the subhost's side, or a member's.
pub enum Share {
    Host(Box<HostShare>),
    Member(Box<MemberShare>),
}

impl Share {
    pub fn host() -> Self {
        Self::Host(Box::new(HostShare::new()))
    }

    pub fn member() -> Self {
        Self::Member(Box::new(MemberShare::new()))
    }

    /// Member, let in: says which level it wants.
    pub fn joined(&mut self, session: &mut Session, now: u64) {
        if let Self::Member(member) = self {
            member.tell_level(session, now, true);
        }
    }

    /// Each tick of the session, whether or not the editor holds its vehicle (`editing`).
    pub fn tick(&mut self, session: &mut Session, transport: &mut Wire, now: u64, editing: bool) {
        match self {
            Self::Host(host) => host.tick(session, transport, now, editing),
            Self::Member(member) => member.tick(session, now),
        }
    }

    pub fn level_asked(&mut self, peer: PeerId, level: Option<Level>, now: u64) {
        if let Self::Host(host) = self {
            coop_log!("references: {peer} asks for {level:?}");
            host.sharing.level_asked(peer, level, now);
        }
    }

    pub fn wanted(&mut self, peer: PeerId, content: u64) {
        if let Self::Host(host) = self {
            host.sharing.wanted(peer, content);
        }
    }

    pub fn list(&mut self, entries: Vec<Entry>) {
        if let Self::Member(member) = self {
            coop_log!("references: the host lists {}", entries.len());
            member.receiving.list(entries);
        }
    }

    pub fn data(&mut self, content: u64, offset: u32, data: &[u8], now: u64) {
        if let Self::Member(member) = self {
            member.data(content, offset, data, now);
        }
    }

    /// Subhost: `peer` changed our references.
    pub fn edited(&mut self, peer: PeerId, edits: Vec<Edit>) {
        if let Self::Host(host) = self {
            // Made at the next tick in the session's vehicle; while away, the oldest go past a cap.
            if host.edits.len() >= MAX_WAITING_EDITS {
                host.edits.remove(0);
            }
            host.edits.push((peer, edits));
        }
    }

    /// Member: its references changed and have settled since it was last asked: whether to write
    /// them along with the session's starting autosave.
    pub fn settled(&mut self) -> bool {
        match self {
            Self::Member(member) => member.settled(),
            Self::Host(_) => false,
        }
    }
}

/// What the cooking thread is asked.
enum Job {
    Measure {
        key: String,
        kind: sidecar::Kind,
        path: PathBuf,
    },
    Cook {
        key: String,
        kind: sidecar::Kind,
        path: PathBuf,
        import: ImportSettings,
        aim: Aim,
    },
}

/// What it answers.
enum Done {
    Measured(String, Result<Size, String>),
    Cooked(String, Result<Cooked, String>),
}

/// A model in the editor's space, as it shows: a shared file's already is.
fn read_model(path: &std::path::Path, import: &ImportSettings) -> Result<model_io::Model, String> {
    let model = readers::read(path)?;
    if Format::of(path) == Some(Format::Shared) {
        Ok(model)
    } else {
        model_io::space::to_unreal(model, import)
    }
}

fn run_job(job: Job) -> Done {
    match job {
        Job::Measure { key, kind, path } => {
            let size = match kind {
                sidecar::Kind::Image => image::read(&path).map(|picture| Size {
                    triangles: 0,
                    pixels: u64::from(picture.width) * u64::from(picture.height),
                }),
                sidecar::Kind::Model => readers::read(&path).map(|model| Size {
                    triangles: model.triangle_count() as u64,
                    pixels: shared::model_pixels(&model),
                }),
            };
            Done::Measured(key, size)
        }
        Job::Cook {
            key,
            kind,
            path,
            import,
            aim,
        } => {
            let cooked = match kind {
                sidecar::Kind::Image => std::fs::read(&path)
                    .map_err(|error| format!("{}: {error}", path.display()))
                    .and_then(|bytes| shared::cook_image(&bytes, &aim)),
                sidecar::Kind::Model => {
                    read_model(&path, &import).and_then(|model| shared::cook_model(&model, &aim))
                }
            };
            Done::Cooked(key, cooked)
        }
    }
}

/// The subhost's side.
pub struct HostShare {
    sharing: Sharing,
    jobs: Option<Sender<Job>>,
    done: Receiver<Done>,
    /// Keys asked of the cooking thread, not answered yet.
    asked: Vec<String>,
    /// Each file's size, by [`file_key`] (`None`: can't be read, so it isn't shared).
    sizes: HashMap<String, Option<Size>>,
    /// Finished cooks by [`cook_key`] (`None`: failed).
    cooks: HashMap<String, Option<Arc<Cooked>>>,
    /// The cook each reference shows at each level, until a newer one is done.
    latest: HashMap<(String, Level), Arc<Cooked>>,
    /// What members may ask for.
    contents: HashMap<u64, Arc<Cooked>>,
    /// Each reference's opacity when last shown: hiding one (opacity 0, or not visible) stays local.
    opacities: HashMap<String, f32>,
    looked_at: Option<Instant>,
    logged_at: Instant,
    sent_at_log: HashMap<PeerId, u64>,
    /// Members' changes of our references, oldest first, not made yet (the references were busy).
    edits: Vec<(PeerId, Vec<Edit>)>,
}

impl HostShare {
    fn new() -> Self {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let (answers, done) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("editortweaks coop cooking".into())
            .spawn(move || {
                for job in inbox {
                    if answers.send(run_job(job)).is_err() {
                        return;
                    }
                }
            });
        if let Err(error) = &spawned {
            coop_log!("references won't be shared: no thread to cook them on: {error}");
        }
        Self {
            sharing: Sharing::default(),
            jobs: spawned.ok().map(|_| jobs),
            done,
            asked: Vec::new(),
            sizes: HashMap::new(),
            cooks: HashMap::new(),
            latest: HashMap::new(),
            contents: HashMap::new(),
            opacities: HashMap::new(),
            looked_at: None,
            logged_at: Instant::now(),
            sent_at_log: HashMap::new(),
            edits: Vec::new(),
        }
    }

    /// Makes the members' changes of our references waiting, unless the references are busy.
    fn apply_edits(&mut self) {
        while let Some((peer, edits)) = self.edits.first() {
            let Some(made) = received::apply_edits(edits) else {
                return;
            };
            coop_log!(
                "references: {peer} changed {made} of them ({} asked)",
                edits.len()
            );
            self.edits.remove(0);
        }
    }

    fn tick(&mut self, session: &mut Session, transport: &mut Wire, now: u64, editing: bool) {
        self.sharing.keep_members(&session.expected_peers());
        // Changes of another vehicle's references would land on the wrong ones: they wait.
        if editing {
            self.apply_edits();
        }
        while let Ok(done) = self.done.try_recv() {
            self.take(done);
        }
        // Another vehicle in the editor isn't the session's: what was shared stays.
        if editing && self.looked_at.is_none_or(|at| at.elapsed() >= LOOK_EVERY) {
            self.looked_at = Some(Instant::now());
            if let Some(shared) = received::shared_now() {
                self.plan(&shared);
            }
        }
        for (peer, list) in self.sharing.lists(now) {
            coop_log!("references: {} listed for {peer}", list.len());
            session.send_reference_list(peer, list);
        }
        let contents = &self.contents;
        let chunks = self.sharing.chunks(
            now,
            |peer| transport.backlog(peer),
            |content| contents.get(&content).map(|cooked| cooked.bytes.as_slice()),
        );
        for chunk in chunks {
            session.send_reference_data(chunk);
        }
        if self.logged_at.elapsed() >= LOG_EVERY {
            self.logged_at = Instant::now();
            for peer in session.expected_peers() {
                let sent = self.sharing.sent_to(peer);
                let before = self.sent_at_log.insert(peer, sent).unwrap_or(0);
                if sent != before {
                    coop_log!(
                        "references: {} KiB to {peer} in the last {}s",
                        // A member joining again starts over from nothing sent.
                        sent.saturating_sub(before) / 1024,
                        LOG_EVERY.as_secs()
                    );
                    transport.log_queue(peer);
                }
            }
        }
    }

    fn take(&mut self, done: Done) {
        match done {
            Done::Measured(key, size) => {
                self.asked.retain(|asked| *asked != key);
                if let Err(reason) = &size {
                    coop_log!("references: can't share {key}: {reason}");
                }
                self.sizes.insert(key, size.ok());
            }
            Done::Cooked(key, cooked) => {
                self.asked.retain(|asked| *asked != key);
                let cooked = match cooked {
                    Ok(cooked) => {
                        coop_log!(
                            "references: cooked {} KiB, {} triangles, {} pixels: {key}",
                            cooked.bytes.len() / 1024,
                            cooked.triangles,
                            cooked.pixels
                        );
                        Some(Arc::new(cooked))
                    }
                    Err(reason) => {
                        coop_log!("references: can't share {key}: {reason}");
                        None
                    }
                };
                self.cooks.insert(key, cooked);
            }
        }
    }

    fn ask(&mut self, key: String, job: impl FnOnce(String) -> Job) {
        if self.asked.contains(&key) {
            return;
        }
        let Some(jobs) = &self.jobs else { return };
        if jobs.send(job(key.clone())).is_ok() {
            self.asked.push(key);
        }
    }

    /// Asks for what isn't measured or cooked yet, and publishes each level's list as far as it's
    /// cooked. Nothing is cooked for a level nobody asked for.
    fn plan(&mut self, shared: &[received::Shared]) {
        let shared: Vec<&received::Shared> = shared
            .iter()
            .filter(|shared| usable_id(&shared.reference.id))
            .collect();
        for item in &shared {
            let key = file_key(item);
            if !self.sizes.contains_key(&key) {
                let (kind, path) = (item.reference.kind, item.file.clone());
                self.ask(key, |key| Job::Measure { key, kind, path });
            }
        }
        let levels = self.sharing.levels();
        for &level in &levels {
            let sizes: Vec<Size> = shared
                .iter()
                .map(|item| match self.sizes.get(&file_key(item)) {
                    Some(Some(size)) => *size,
                    _ => Size::default(),
                })
                .collect();
            let targets = split(level.caps(), &sizes);
            for (item, target) in shared.iter().zip(targets) {
                let file = file_key(item);
                if !matches!(self.sizes.get(&file), Some(Some(_))) {
                    continue;
                }
                let key = cook_key(&file, &target);
                match self.cooks.get(&key) {
                    Some(Some(cooked)) => {
                        self.latest
                            .insert((item.reference.id.clone(), level), Arc::clone(cooked));
                    }
                    Some(None) => {}
                    None if target.bytes > 0 => {
                        let (kind, path) = (item.reference.kind, item.file.clone());
                        let import = item.reference.import.unwrap_or_default();
                        let aim = Aim {
                            triangles: target.triangles,
                            longest_side: target.longest_side,
                            pixels: target.pixels,
                            bytes: target.bytes,
                        };
                        self.ask(key, |key| Job::Cook {
                            key,
                            kind,
                            path,
                            import,
                            aim,
                        });
                    }
                    None => {}
                }
            }
            let entries = shared
                .iter()
                .filter_map(|item| {
                    let cooked = self.latest.get(&(item.reference.id.clone(), level))?;
                    let reference = &item.reference;
                    let opacity = shown_opacity(&mut self.opacities, reference);
                    Some(entry_of(reference, cooked, opacity))
                })
                .collect();
            self.sharing.publish(level, entries);
        }
        self.prune(&shared);
    }

    /// Lets go of what no list can name any more.
    fn prune(&mut self, shared: &[&received::Shared]) {
        self.latest
            .retain(|(id, _), _| shared.iter().any(|item| item.reference.id == *id));
        self.contents = self
            .latest
            .values()
            .map(|cooked| (cooked.content(), Arc::clone(cooked)))
            .collect();
        let files: Vec<String> = shared.iter().map(|item| file_key(item)).collect();
        self.sizes.retain(|key, _| files.contains(key));
        // What each level asks for now, in use or not: going back to a level costs no cooking.
        let sizes: Vec<Size> = files
            .iter()
            .map(|file| self.sizes.get(file).copied().flatten().unwrap_or_default())
            .collect();
        let current: Vec<String> = Level::ALL
            .iter()
            .flat_map(|level| split(level.caps(), &sizes))
            .zip(files.iter().cycle())
            .map(|(target, file)| cook_key(file, &target))
            .collect();
        self.cooks.retain(|key, _| current.contains(key));
    }
}

/// Whether a member would take `id` (the sidecar can be edited by hand).
fn usable_id(id: &str) -> bool {
    (1..=32).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Names a reference's file as it is now, with what changes the model made of it.
fn file_key(item: &received::Shared) -> String {
    format!(
        "{} {:?} {:?}",
        item.file.display(),
        item.stamp,
        item.reference.import
    )
}

fn cook_key(file: &str, target: &Target) -> String {
    format!("{file} {target:?}")
}

/// The opacity members see `reference` at: hidden here (opacity 0, or not visible) it stays as it
/// was for them, since hiding is local.
fn shown_opacity(opacities: &mut HashMap<String, f32>, reference: &Reference) -> f32 {
    if reference.visible && reference.opacity > 0.0 {
        opacities.insert(reference.id.clone(), reference.opacity);
        return reference.opacity;
    }
    opacities.get(&reference.id).copied().unwrap_or(1.0)
}

/// A number a member takes: within `most` either way, a broken one `default` (a member refuses a
/// whole list over one past its bounds, and a sidecar can be edited by hand).
fn bounded(value: f64, most: f64, default: f64) -> f64 {
    if value.is_finite() {
        value.clamp(-most, most)
    } else {
        default
    }
}

fn entry_of(reference: &Reference, cooked: &Cooked, opacity: f32) -> Entry {
    let mut name = reference.name.clone();
    while name.len() > 128 {
        name.pop();
    }
    Entry {
        id: reference.id.clone(),
        name,
        kind: match reference.kind {
            sidecar::Kind::Image => Kind::Image,
            sidecar::Kind::Model => Kind::Model,
        },
        content: cooked.content(),
        bytes: cooked.bytes.len() as u32,
        triangles: cooked.triangles,
        pixels: cooked.pixels,
        simplified: cooked.simplified,
        position: reference.position.map(|value| bounded(value, 1e7, 0.0)),
        rotation: reference.rotation.map(|value| bounded(value, 1e5, 0.0)),
        scale: reference.scale.map(|value| bounded(value, 1e6, 1.0)),
        mirror: reference.mirror,
        opacity: if opacity.is_finite() { opacity } else { 1.0 },
        color: reference
            .color
            .map(|value| if value.is_finite() { value } else { 1.0 }),
        image: reference.image.as_ref().map(|image| ImageLook {
            width: bounded(image.width, 1e7, 100.0),
            flip_x: image.flip_x,
            flip_y: image.flip_y,
            double_sided: image.double_sided,
            pixelated: image.pixelated,
            pivot: match image.pivot {
                ImagePivot::Center => 0,
                ImagePivot::Corner => 1,
            },
        }),
    }
}

/// What a member's checking thread is asked.
struct Check {
    content: u64,
    bytes: Vec<u8>,
    limits: Limits,
    triangles: u32,
    pixels: u32,
    kind: Kind,
}

/// A member's side.
pub struct MemberShare {
    receiving: Receiving,
    told: bool,
    level: Option<Level>,
    jobs: Option<Sender<Check>>,
    done: Receiver<(u64, Result<PathBuf, String>)>,
    /// Files of what arrived, with their sizes.
    files: HashMap<u64, (PathBuf, u64)>,
    /// What was last shown, to tell a change.
    shown: Option<Vec<Reference>>,
    /// What the panel was last told.
    status: String,
    /// Since when `shown` holds, if it wasn't written along yet.
    changed_at: Option<Instant>,
    /// The player's changes of the subhost's references, not sent yet, and when some last went.
    edits: Vec<Edit>,
    edits_sent_at: Option<Instant>,
}

impl MemberShare {
    fn new() -> Self {
        let (jobs, inbox) = mpsc::channel::<Check>();
        let (answers, done) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("editortweaks coop references".into())
            .spawn(move || {
                // What an earlier session left. A vehicle saved then has its own copies.
                if let Some(folder) = received::folder() {
                    let _ = std::fs::remove_dir_all(&folder);
                }
                for check in inbox {
                    let content = check.content;
                    if answers.send((content, run_check(check))).is_err() {
                        return;
                    }
                }
            });
        if let Err(error) = &spawned {
            coop_log!("references won't be received: no thread to check them on: {error}");
        }
        Self {
            receiving: Receiving::new(None, 0),
            told: false,
            level: None,
            jobs: spawned.ok().map(|_| jobs),
            done,
            files: HashMap::new(),
            shown: None,
            status: String::new(),
            changed_at: None,
            edits: Vec::new(),
            edits_sent_at: None,
        }
    }

    /// Tells the host the level the setting asks for, if it changed (`joined`: at any rate).
    fn tell_level(&mut self, session: &mut Session, now: u64, joined: bool) {
        self.told |= joined;
        let level = if self.jobs.is_some() {
            chosen_level()
        } else {
            None
        };
        if self.told && (joined || level != self.level) {
            coop_log!("references: asking for {level:?}");
            session.reference_level(level);
            self.receiving.set_level(level, now);
            self.level = level;
        }
    }

    fn tick(&mut self, session: &mut Session, now: u64) {
        self.tell_level(session, now, false);
        while let Ok((content, result)) = self.done.try_recv() {
            match result {
                Ok(path) => {
                    let size = std::fs::metadata(&path).map_or(0, |metadata| metadata.len());
                    self.files.insert(content, (path, size));
                    self.receiving.ready(content);
                }
                Err(reason) => {
                    coop_log!("references: refused {content:016x}: {reason}");
                    self.receiving.failed(content);
                }
            }
        }
        if let Some(content) = self.receiving.next_want(now) {
            session.want_reference(content);
        }
        self.send_edits(session);
        self.show();
        self.prune();
    }

    /// Sends the player's changes of the subhost's references, the last of each reference only, at
    /// most every [`EDIT_PACE`].
    fn send_edits(&mut self, session: &mut Session) {
        self.edits.extend(received::take_edits());
        if self.edits.is_empty()
            || self
                .edits_sent_at
                .is_some_and(|at| at.elapsed() < EDIT_PACE)
        {
            return;
        }
        let edits = coalesce(std::mem::take(&mut self.edits));
        coop_log!("references: sending {} changes to the host", edits.len());
        session.edit_references(edits);
        self.edits_sent_at = Some(Instant::now());
    }

    fn data(&mut self, content: u64, offset: u32, data: &[u8], now: u64) {
        let Some(finished) = self.receiving.data(content, offset, data, now) else {
            return;
        };
        let (Some(level), Some(jobs)) = (self.level, &self.jobs) else {
            return;
        };
        let entries = self.receiving.entries();
        let Some((entry, _)) = entries.iter().find(|(entry, _)| entry.content == content) else {
            return;
        };
        let caps = level.caps();
        let check = Check {
            content,
            bytes: finished.bytes,
            limits: Limits {
                triangles: caps.triangles,
                longest_side: caps.longest_side,
                pixels: caps.pixels,
            },
            triangles: entry.triangles,
            pixels: entry.pixels,
            kind: entry.kind,
        };
        let _ = jobs.send(check);
    }

    /// Hands what's ready to the references feature. Off, or a host with none: the vehicle's own
    /// references show.
    fn show(&mut self) {
        let entries = self.receiving.entries();
        if entries.is_empty() {
            if self.shown.take().is_some() {
                received::stop();
                self.status.clear();
            }
            return;
        }
        let list: Vec<Reference> = self
            .receiving
            .shown()
            .into_iter()
            .filter_map(|entry| {
                let (path, _) = self.files.get(&entry.content)?;
                Some(reference_of(entry, path))
            })
            .collect();
        let simplified = self
            .receiving
            .shown()
            .iter()
            .filter(|e| e.simplified)
            .count();
        let skipped = entries
            .iter()
            .filter(|(_, status)| matches!(status, Status::Skipped(_)))
            .count();
        let mut status = format!("{} of {} received", list.len(), entries.len());
        if simplified > 0 {
            status.push_str(&format!(", {simplified} simplified"));
        }
        if skipped > 0 {
            status.push_str(&format!(", {skipped} over your quality's limits"));
        }
        if status != self.status {
            coop_log!("references: {status}");
            for (entry, state) in &entries {
                if let Status::Skipped(why) = state {
                    let why = match why {
                        Skip::Triangles => "triangles",
                        Skip::Pixels => "pixels",
                        Skip::Count => "how many",
                        Skip::Bytes => "size",
                    };
                    coop_log!("references: {} skipped ({why})", entry.name);
                }
            }
            self.status.clone_from(&status);
        }
        if self.shown.as_ref() != Some(&list) {
            self.changed_at = Some(Instant::now());
            self.shown = Some(list.clone());
        }
        received::show(list, status);
    }

    /// Past twice the level's burst, deletes files no list names now (they'd be asked for again).
    fn prune(&mut self) {
        let Some(level) = self.level else { return };
        let total: u64 = self.files.values().map(|(_, size)| size).sum();
        if total <= 2 * level.caps().burst_bytes {
            return;
        }
        let listed: Vec<u64> = self
            .receiving
            .entries()
            .iter()
            .map(|(entry, _)| entry.content)
            .collect();
        let unlisted: Vec<u64> = self
            .files
            .keys()
            .copied()
            .filter(|content| !listed.contains(content))
            .collect();
        for content in unlisted {
            if let Some((path, _)) = self.files.remove(&content) {
                let _ = std::fs::remove_file(path);
            }
            self.receiving.forget(content);
        }
    }

    fn settled(&mut self) -> bool {
        let due = self.changed_at.is_some_and(|at| at.elapsed() >= SETTLE);
        if due {
            self.changed_at = None;
        }
        due
    }
}

impl Drop for MemberShare {
    /// The session is over: the vehicle's own references show again.
    fn drop(&mut self) {
        received::stop();
    }
}

/// Checks what arrived and writes it for the references to show. On the checking thread.
fn run_check(check: Check) -> Result<PathBuf, String> {
    if shared::content_hash(&check.bytes) != check.content {
        return Err("it isn't what the list named".into());
    }
    let checked = shared::check(&check.bytes, &check.limits, check.triangles, check.pixels)?;
    let (bytes, extension) = match (checked, check.kind) {
        (Checked::Image { file, extension }, Kind::Image) => (file, extension),
        (Checked::Model, Kind::Model) => (check.bytes, shared::EXTENSION),
        _ => return Err("not the kind the list said".into()),
    };
    let folder = received::folder().ok_or("no folder to keep it in")?;
    std::fs::create_dir_all(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    let path = folder.join(format!("{:016x}.{extension}", check.content));
    let part = path.with_extension("part");
    std::fs::write(&part, &bytes)
        .and_then(|()| std::fs::rename(&part, &path))
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(path)
}

/// A received reference as a sidecar's: a copy still to make of its file, which a save does.
fn reference_of(entry: &Entry, path: &std::path::Path) -> Reference {
    let mut reference = Reference {
        id: entry.id.clone(),
        kind: match entry.kind {
            Kind::Image => sidecar::Kind::Image,
            Kind::Model => sidecar::Kind::Model,
        },
        name: entry.name.clone(),
        source: Source {
            mode: SourceMode::Copy,
            path: path.to_string_lossy().replace('\\', "/"),
            ..Source::default()
        },
        position: entry.position,
        rotation: entry.rotation,
        scale: entry.scale,
        mirror: entry.mirror,
        opacity: entry.opacity,
        color: entry.color,
        image: entry.image.as_ref().map(|image| ImageSettings {
            width: image.width,
            flip_x: image.flip_x,
            flip_y: image.flip_y,
            double_sided: image.double_sided,
            pixelated: image.pixelated,
            pivot: if image.pivot == 1 {
                ImagePivot::Corner
            } else {
                ImagePivot::Center
            },
        }),
        ..Reference::default()
    };
    reference.fill_kind_settings();
    reference
}
