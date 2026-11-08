//! Sharing the host's reference images and models with the members (`docs/coop-references.md`).
//! No game code and no file formats: this is the list of references, who may have how much of
//! what, and the pacing of the data.
//!
//! The host fits its references under each member's quality level ([`split`]), tells the member
//! the list ([`Sharing::lists`]) and sends a reference's data when the member asks for it
//! ([`Sharing::wanted`], [`Sharing::chunks`]). The member never trusts the host: [`Receiving`]
//! enforces every cap itself, asks for one reference at a time, and takes only the chunks it
//! asked for, in order, up to the size the list declared.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::limits::{Bucket, Rate};
use crate::message::PeerId;
use crate::wire::{DecodeError, Reader, Result, Writer};

/// The most bytes of one [`crate::Message::ReferenceData`].
pub const CHUNK: usize = 16 * 1024;
/// A peer's backlog past this: no more chunks to it now.
pub const WINDOW: usize = 64 * 1024;
/// Bytes per second over all members (the burst is 64 KiB).
pub const PACE: f64 = 1024.0 * 1024.0;
/// A member's byte allowance (its level's burst) is refilled in this long.
pub const REFILL_SECONDS: u64 = 600;
/// References in a list, at most (the highest level's count).
pub const MAX_ENTRIES: usize = 256;
/// A download that got no data for this long (milliseconds) is dropped and asked for again.
pub const DATA_TIMEOUT: u64 = 120_000;

const ID_BYTES: usize = 32;
const NAME_BYTES: usize = 128;
/// What a reference's numbers may be at most (absolute), as the editor allows them.
const POSITION_LIMIT: f64 = 1e7;
const ROTATION_LIMIT: f64 = 1e5;
const SCALE_LIMIT: f64 = 1e6;
const WIDTH_LIMIT: f64 = 1e7;
/// Pivots an image has (a 4 x 4 grid of places on it).
const PIVOTS: u8 = 16;
/// What a triangle weighs in a reference's estimated size in bytes (a pixel weighs one).
const BYTES_PER_TRIANGLE: u64 = 12;

/// Level changes a member may ask for: one per 10 seconds, two in a row.
const LEVEL_ASKS: Rate = Rate {
    per_second: 0.1,
    burst: 2.0,
};
/// Lists sent to one member: two per second.
const LIST_RATE: Rate = Rate {
    per_second: 2.0,
    burst: 2.0,
};
const PACE_RATE: Rate = Rate {
    per_second: PACE,
    burst: 64.0 * 1024.0,
};

// --- Levels ---

/// How much of the host's references a member takes. Off is `None` where a level is optional.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Level {
    High,
    VeryHigh,
    Extreme,
}

/// The totals a level allows over all of a session's references.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    pub triangles: u64,
    /// The longest side of an image or a model's texture.
    pub longest_side: u32,
    pub pixels: u64,
    pub references: usize,
    /// Bytes that can be received at once, refilled in [`REFILL_SECONDS`].
    pub burst_bytes: u64,
}

impl Level {
    pub const ALL: [Level; 3] = [Level::High, Level::VeryHigh, Level::Extreme];

    pub fn caps(self) -> Caps {
        const MIB: u64 = 1 << 20;
        match self {
            Level::High => Caps {
                triangles: 250_000,
                longest_side: 2048,
                pixels: 16_000_000,
                references: 64,
                burst_bytes: 16 * MIB,
            },
            Level::VeryHigh => Caps {
                triangles: 1_000_000,
                longest_side: 4096,
                pixels: 48_000_000,
                references: 128,
                burst_bytes: 64 * MIB,
            },
            Level::Extreme => Caps {
                triangles: 4_000_000,
                longest_side: 8192,
                pixels: 128_000_000,
                references: 256,
                burst_bytes: 256 * MIB,
            },
        }
    }

    /// 0 for off, then 1 to 3.
    pub fn to_wire(level: Option<Level>) -> u8 {
        match level {
            None => 0,
            Some(Level::High) => 1,
            Some(Level::VeryHigh) => 2,
            Some(Level::Extreme) => 3,
        }
    }

    /// `None` for a number that is no level.
    pub fn from_wire(tag: u8) -> Option<Option<Level>> {
        match tag {
            0 => Some(None),
            1 => Some(Some(Level::High)),
            2 => Some(Some(Level::VeryHigh)),
            3 => Some(Some(Level::Extreme)),
            _ => None,
        }
    }
}

/// A member's bytes allowance: its level's burst, refilled at that per [`REFILL_SECONDS`]. The
/// host and the member keep the same numbers. A level change keeps what was spent, so changing
/// levels back and forth doesn't fill it again.
struct Allowance {
    level: Level,
    bucket: Bucket,
}

impl Allowance {
    fn rate(level: Level) -> Rate {
        let burst = level.caps().burst_bytes as f64;
        Rate {
            per_second: burst / REFILL_SECONDS as f64,
            burst,
        }
    }

    fn new(level: Level, now: u64) -> Self {
        Self {
            level,
            bucket: Bucket::full(Self::rate(level), now),
        }
    }

    fn change(&mut self, level: Level, now: u64) {
        let old = Self::rate(self.level);
        self.bucket.refill(old, now);
        let spent = old.burst - self.bucket.level;
        self.level = level;
        self.bucket.level = (Self::rate(level).burst - spent).max(0.0);
    }

    fn holds(&mut self, now: u64, bytes: u64) -> bool {
        self.bucket.refill(Self::rate(self.level), now);
        self.bucket.level >= bytes as f64
    }

    /// Takes `bytes`, which it holds ([`Allowance::holds`]).
    fn take(&mut self, bytes: u64) {
        self.bucket.level = (self.bucket.level - bytes as f64).max(0.0);
    }

    /// Gives back bytes taken and not used.
    fn refund(&mut self, bytes: u64) {
        let burst = Self::rate(self.level).burst;
        self.bucket.level = (self.bucket.level + bytes as f64).min(burst);
    }
}

// --- The list ---

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Image,
    Model,
}

/// How an image is shown.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageLook {
    pub width: f64,
    pub flip_x: bool,
    pub flip_y: bool,
    pub double_sided: bool,
    pub pixelated: bool,
    /// Which of the 16 places on the image sits at its position.
    pub pivot: u8,
}

/// One reference in the host's list, as it goes to a member. Whether it is visible isn't part of
/// it: hiding is each player's own.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Names it across lists: letters, digits, `_` and `-`, at most 32 bytes.
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// Hash of the cooked data, which also names the data.
    pub content: u64,
    /// The cooked data's size.
    pub bytes: u32,
    pub triangles: u32,
    pub pixels: u32,
    /// Fitted under the member's level, not as the host has it.
    pub simplified: bool,
    pub position: [f64; 3],
    pub rotation: [f64; 3],
    pub scale: [f64; 3],
    pub mirror: [bool; 3],
    pub opacity: f32,
    pub color: [f32; 3],
    /// Some exactly for an image.
    pub image: Option<ImageLook>,
}

/// Replaces control characters (a name is shown on a panel and written to a log) with spaces.
fn tidy(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// `name` cut to `max` bytes without splitting a character.
fn cut(name: &str, max: usize) -> &str {
    let mut end = max.min(name.len());
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

/// Writes the list (at most [`MAX_ENTRIES`], and names cut to what a reader takes).
pub fn write_entries(w: &mut Writer, entries: &[Entry]) {
    let entries = &entries[..entries.len().min(MAX_ENTRIES)];
    w.count(entries.len());
    for entry in entries {
        w.str(&entry.id);
        w.str(&tidy(cut(&entry.name, NAME_BYTES)));
        w.u8(match entry.kind {
            Kind::Image => 0,
            Kind::Model => 1,
        });
        w.u64(entry.content);
        w.u32(entry.bytes);
        w.u32(entry.triangles);
        w.u32(entry.pixels);
        w.bool(entry.simplified);
        for v in entry
            .position
            .iter()
            .chain(&entry.rotation)
            .chain(&entry.scale)
        {
            w.f64(*v);
        }
        entry.mirror.iter().for_each(|&m| w.bool(m));
        w.f32(entry.opacity);
        entry.color.iter().for_each(|&c| w.f32(c));
        w.bool(entry.image.is_some());
        if let Some(look) = &entry.image {
            w.f64(look.width);
            w.bool(look.flip_x);
            w.bool(look.flip_y);
            w.bool(look.double_sided);
            w.bool(look.pixelated);
            w.u8(look.pivot);
        }
    }
}

/// Reads a list written by [`write_entries`], refusing what no honest host sends.
pub fn read_entries(r: &mut Reader) -> Result<Vec<Entry>> {
    let n = r.count("references", MAX_ENTRIES)?;
    let mut entries: Vec<Entry> = Vec::new();
    for _ in 0..n {
        let entry = read_entry(r)?;
        if entries.iter().any(|other| other.id == entry.id) {
            return Err(DecodeError::BadValue("reference id used twice"));
        }
        entries.push(entry);
    }
    Ok(entries)
}

fn read_entry(r: &mut Reader) -> Result<Entry> {
    let id = r.str("reference id", ID_BYTES)?;
    let id_ok = !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !id_ok {
        return Err(DecodeError::BadValue("reference id"));
    }
    let name = tidy(&r.str("reference name", NAME_BYTES)?);
    let kind = match r.u8()? {
        0 => Kind::Image,
        1 => Kind::Model,
        tag => {
            return Err(DecodeError::BadTag {
                what: "reference kind",
                tag,
            });
        }
    };
    let content = r.u64()?;
    let bytes = r.u32()?;
    let triangles = r.u32()?;
    let pixels = r.u32()?;
    let simplified = r.bool()?;
    let position = read_vector(r, POSITION_LIMIT, "reference position")?;
    let rotation = read_vector(r, ROTATION_LIMIT, "reference rotation")?;
    let scale = read_vector(r, SCALE_LIMIT, "reference scale")?;
    let mirror = [r.bool()?, r.bool()?, r.bool()?];
    let opacity = read_finite(r, "reference opacity")?;
    let color = [
        read_finite(r, "reference colour")?,
        read_finite(r, "reference colour")?,
        read_finite(r, "reference colour")?,
    ];
    let image = if r.bool()? {
        let width = r.f64()?;
        if !(width.is_finite() && width.abs() <= WIDTH_LIMIT) {
            return Err(DecodeError::BadValue("reference width"));
        }
        let look = ImageLook {
            width,
            flip_x: r.bool()?,
            flip_y: r.bool()?,
            double_sided: r.bool()?,
            pixelated: r.bool()?,
            pivot: r.u8()?,
        };
        if look.pivot >= PIVOTS {
            return Err(DecodeError::BadValue("reference pivot"));
        }
        Some(look)
    } else {
        None
    };
    // Only an image has a look, and every image has one.
    if image.is_some() != (kind == Kind::Image) {
        return Err(DecodeError::BadValue("reference image"));
    }
    Ok(Entry {
        id,
        name,
        kind,
        content,
        bytes,
        triangles,
        pixels,
        simplified,
        position,
        rotation,
        scale,
        mirror,
        opacity,
        color,
        image,
    })
}

fn read_vector(r: &mut Reader, limit: f64, what: &'static str) -> Result<[f64; 3]> {
    let mut vector = [0.0; 3];
    for v in &mut vector {
        *v = r.f64()?;
        if !(v.is_finite() && v.abs() <= limit) {
            return Err(DecodeError::BadValue(what));
        }
    }
    Ok(vector)
}

fn read_finite(r: &mut Reader, what: &'static str) -> Result<f32> {
    let v = r.f32()?;
    if v.is_finite() {
        Ok(v)
    } else {
        Err(DecodeError::BadValue(what))
    }
}

// --- Members' edits ---

/// Where a reference is and how it looks: what a member may change of one of the host's
/// references (an entry's settings, but its name).
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub position: [f64; 3],
    pub rotation: [f64; 3],
    pub scale: [f64; 3],
    pub mirror: [bool; 3],
    pub opacity: f32,
    pub color: [f32; 3],
    /// Some exactly for an image.
    pub image: Option<ImageLook>,
}

/// A member's change of one of the host's references (`Message::ReferenceEdit`). The host makes it
/// as its own, and the next lists tell everyone.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// The reference `id` now has this look.
    Set { id: String, look: Look },
    /// The reference `id` is removed.
    Remove { id: String },
}

impl Edit {
    pub fn id(&self) -> &str {
        match self {
            Self::Set { id, .. } | Self::Remove { id } => id,
        }
    }
}

/// `edits` with only the last edit of each reference, in the order of those (several changes of a
/// slider while one message waits go as one).
pub fn coalesce(edits: Vec<Edit>) -> Vec<Edit> {
    let mut kept: Vec<Edit> = Vec::new();
    for edit in edits {
        kept.retain(|earlier| earlier.id() != edit.id());
        kept.push(edit);
    }
    kept
}

/// Writes `edits` (at most [`MAX_ENTRIES`]: one per reference).
pub fn write_edits(w: &mut Writer, edits: &[Edit]) {
    let edits = &edits[..edits.len().min(MAX_ENTRIES)];
    w.count(edits.len());
    for edit in edits {
        match edit {
            Edit::Set { id, look } => {
                w.u8(0);
                w.str(id);
                for v in look
                    .position
                    .iter()
                    .chain(&look.rotation)
                    .chain(&look.scale)
                {
                    w.f64(*v);
                }
                look.mirror.iter().for_each(|&m| w.bool(m));
                w.f32(look.opacity);
                look.color.iter().for_each(|&c| w.f32(c));
                w.bool(look.image.is_some());
                if let Some(image) = &look.image {
                    w.f64(image.width);
                    w.bool(image.flip_x);
                    w.bool(image.flip_y);
                    w.bool(image.double_sided);
                    w.bool(image.pixelated);
                    w.u8(image.pivot);
                }
            }
            Edit::Remove { id } => {
                w.u8(1);
                w.str(id);
            }
        }
    }
}

/// Reads edits written by [`write_edits`], refusing what no honest member sends: numbers past what
/// a list may hold, a reference edited twice. Whether each names a reference, and an image's look
/// an image, is for the host to see.
pub fn read_edits(r: &mut Reader) -> Result<Vec<Edit>> {
    let n = r.count("reference edits", MAX_ENTRIES)?;
    let mut edits: Vec<Edit> = Vec::new();
    for _ in 0..n {
        let tag = r.u8()?;
        let id = read_id(r)?;
        let edit = match tag {
            0 => {
                let position = read_vector(r, POSITION_LIMIT, "reference position")?;
                let rotation = read_vector(r, ROTATION_LIMIT, "reference rotation")?;
                let scale = read_vector(r, SCALE_LIMIT, "reference scale")?;
                let mirror = [r.bool()?, r.bool()?, r.bool()?];
                let opacity = read_finite(r, "reference opacity")?;
                let color = [
                    read_finite(r, "reference colour")?,
                    read_finite(r, "reference colour")?,
                    read_finite(r, "reference colour")?,
                ];
                let image = if r.bool()? {
                    Some(read_image(r)?)
                } else {
                    None
                };
                Edit::Set {
                    id,
                    look: Look {
                        position,
                        rotation,
                        scale,
                        mirror,
                        opacity,
                        color,
                        image,
                    },
                }
            }
            1 => Edit::Remove { id },
            tag => {
                return Err(DecodeError::BadTag {
                    what: "reference edit",
                    tag,
                });
            }
        };
        if edits.iter().any(|other| other.id() == edit.id()) {
            return Err(DecodeError::BadValue("reference edited twice"));
        }
        edits.push(edit);
    }
    Ok(edits)
}

/// A reference id: letters, digits, `_` and `-`, 1 to 32 bytes.
fn read_id(r: &mut Reader) -> Result<String> {
    let id = r.str("reference id", ID_BYTES)?;
    let id_ok = !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if id_ok {
        Ok(id)
    } else {
        Err(DecodeError::BadValue("reference id"))
    }
}

fn read_image(r: &mut Reader) -> Result<ImageLook> {
    let width = r.f64()?;
    if !(width.is_finite() && width.abs() <= WIDTH_LIMIT) {
        return Err(DecodeError::BadValue("reference width"));
    }
    let look = ImageLook {
        width,
        flip_x: r.bool()?,
        flip_y: r.bool()?,
        double_sided: r.bool()?,
        pixelated: r.bool()?,
        pivot: r.u8()?,
    };
    if look.pivot >= PIVOTS {
        return Err(DecodeError::BadValue("reference pivot"));
    }
    Ok(look)
}

// --- Fitting references under a level ---

/// How big a reference is, as the host has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Size {
    pub triangles: u64,
    pub pixels: u64,
}

/// What a reference is fitted to for a member's level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub triangles: u64,
    pub longest_side: u32,
    pub pixels: u64,
    pub bytes: u64,
}

/// Each reference's target under `caps`: a total over its cap is shared in proportion to each
/// one's own size, a total under it stays as it is. Bytes are always shared out of the burst, in
/// proportion to the size the targets estimate (12 bytes a triangle, a byte a pixel). The sums
/// never exceed the caps. A reference past `caps.references` gets nothing.
pub fn split(caps: Caps, sizes: &[Size]) -> Vec<Target> {
    let counted = sizes.len().min(caps.references);
    let kept = &sizes[..counted];
    let triangles = fit(caps.triangles, kept.iter().map(|s| s.triangles));
    let pixels = fit(caps.pixels, kept.iter().map(|s| s.pixels));
    let weights: Vec<u64> = triangles
        .iter()
        .zip(&pixels)
        .map(|(t, p)| t * BYTES_PER_TRIANGLE + p)
        .collect();
    let bytes = shares(caps.burst_bytes, &weights);
    (0..sizes.len())
        .map(|i| {
            if i >= counted {
                return Target {
                    triangles: 0,
                    longest_side: 0,
                    pixels: 0,
                    bytes: 0,
                };
            }
            Target {
                triangles: triangles[i],
                longest_side: caps.longest_side,
                pixels: pixels[i],
                bytes: bytes[i],
            }
        })
        .collect()
}

/// `sizes` as they are when they add up to at most `cap`, else shared out of it in proportion.
fn fit(cap: u64, sizes: impl Iterator<Item = u64>) -> Vec<u64> {
    let sizes: Vec<u64> = sizes.collect();
    let total: u128 = sizes.iter().map(|&s| u128::from(s)).sum();
    if total <= u128::from(cap) {
        sizes
    } else {
        shares(cap, &sizes)
    }
}

/// `amount` shared out in proportion to `weights` (rounded down: never more than `amount` in all).
fn shares(amount: u64, weights: &[u64]) -> Vec<u64> {
    let total: u128 = weights.iter().map(|&w| u128::from(w)).sum();
    weights
        .iter()
        .map(|&w| {
            // Nothing to share by when no weight is there.
            (u128::from(amount) * u128::from(w))
                .checked_div(total)
                .unwrap_or(0) as u64
        })
        .collect()
}

// --- Host side ---

/// A piece of a reference's data for a member.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub peer: PeerId,
    pub content: u64,
    pub offset: u32,
    pub data: Vec<u8>,
}

/// A reference a member asked for, and how much of it went already.
struct Want {
    content: u64,
    offset: usize,
}

struct Peer {
    level: Option<Level>,
    /// Level changes it may still ask for.
    asks: Bucket,
    /// The level it asked for past its allowance, applied when that has room.
    waiting: Option<Option<Level>>,
    lists: Bucket,
    /// The list it was sent last (`None`: it needs one).
    sent: Option<Vec<Entry>>,
    /// Whether it holds a list with something in it (to be told when that goes).
    told_something: bool,
    queue: VecDeque<Want>,
    /// From its first level on.
    allowance: Option<Allowance>,
    given: u64,
}

impl Peer {
    fn new(now: u64) -> Self {
        Self {
            level: None,
            asks: Bucket::full(LEVEL_ASKS, now),
            waiting: None,
            lists: Bucket::full(LIST_RATE, now),
            sent: None,
            told_something: false,
            queue: VecDeque::new(),
            allowance: None,
            given: 0,
        }
    }

    /// Takes the level: what it queued is gone and it needs the new list.
    fn apply(&mut self, level: Option<Level>, now: u64) {
        self.level = level;
        self.waiting = None;
        self.queue.clear();
        self.sent = None;
        if let Some(level) = level {
            match &mut self.allowance {
                Some(allowance) => allowance.change(level, now),
                None => self.allowance = Some(Allowance::new(level, now)),
            }
        }
    }
}

/// What the host keeps for sharing: the lists it has per level, and for each member its level,
/// the list it holds and what it asked for. Times are milliseconds on the session's clock.
pub struct Sharing {
    peers: BTreeMap<PeerId, Peer>,
    published: HashMap<Level, Vec<Entry>>,
    pace: Bucket,
    /// The member served last, so the next call goes on with the one after it.
    turn: Option<PeerId>,
}

impl Default for Sharing {
    fn default() -> Self {
        Self {
            peers: BTreeMap::new(),
            published: HashMap::new(),
            // Full whatever the clock starts at.
            pace: Bucket::full(PACE_RATE, 0),
            turn: None,
        }
    }
}

/// What one try at sending a member a piece came to.
enum Step {
    Sent(Chunk),
    /// The queue changed without a chunk: try again.
    Retry,
    /// Nothing for this member now.
    Out,
    /// The pace has no room for it: nothing for anyone.
    PaceFull,
}

/// A member being served in [`Sharing::chunks`].
struct Serving {
    peer: PeerId,
    /// Bytes it may still be given: what is left of [`WINDOW`] (`None`: not known).
    room: Option<usize>,
}

impl Sharing {
    /// `peer` asks for `level` (off is `None`). One change per 10 seconds, two in a row; past
    /// that the latest asked waits and applies when [`Sharing::lists`] finds room for it. Asking
    /// for the level it has (after asking for others that waited) sends the list again.
    pub fn level_asked(&mut self, peer: PeerId, level: Option<Level>, now: u64) {
        let state = self.peers.entry(peer).or_insert_with(|| Peer::new(now));
        if state.level == level {
            // The member's latest wish is what it has, but it dropped what it was receiving and
            // waits for a list, so it gets one again.
            state.waiting = None;
            state.queue.clear();
            state.sent = None;
        } else if state.asks.take(LEVEL_ASKS, now, 1.0) {
            state.apply(level, now);
        } else {
            state.waiting = Some(level);
        }
    }

    /// Forgets everyone but `peers`.
    pub fn keep_members(&mut self, peers: &[PeerId]) {
        self.peers.retain(|peer, _| peers.contains(peer));
        if self.turn.is_some_and(|turn| !peers.contains(&turn)) {
            self.turn = None;
        }
    }

    /// The levels some member has, sorted.
    pub fn levels(&self) -> Vec<Level> {
        let mut levels: Vec<Level> = self.peers.values().filter_map(|p| p.level).collect();
        levels.sort_by_key(|&level| Level::to_wire(Some(level)));
        levels.dedup();
        levels
    }

    /// The list for members at `level`.
    pub fn publish(&mut self, level: Level, entries: Vec<Entry>) {
        self.published.insert(level, entries);
    }

    /// The lists to send now: each member at a level whose list differs from the one it was sent
    /// last gets it, at most twice a second (the rest go on a later call). Members whose level
    /// change waited for room get it applied here. One that went off after being sent
    /// something gets an empty list. A new list takes the references it doesn't name out of the
    /// member's queue.
    pub fn lists(&mut self, now: u64) -> Vec<(PeerId, Vec<Entry>)> {
        let nothing = Vec::new();
        let mut lists = Vec::new();
        for (&id, peer) in &mut self.peers {
            if let Some(level) = peer.waiting
                && peer.asks.take(LEVEL_ASKS, now, 1.0)
            {
                peer.apply(level, now);
            }
            let list = match peer.level {
                Some(level) => match self.published.get(&level) {
                    Some(list) => list,
                    None => continue,
                },
                None if peer.told_something => &nothing,
                None => continue,
            };
            if peer.sent.as_ref() == Some(list) || !peer.lists.take(LIST_RATE, now, 1.0) {
                continue;
            }
            peer.queue
                .retain(|want| list.iter().any(|entry| entry.content == want.content));
            peer.sent = Some(list.clone());
            peer.told_something = !list.is_empty();
            lists.push((id, list.clone()));
        }
        lists
    }

    /// `peer` asks for `content` from its start, if the list it was sent names it. Asking for
    /// what is on its way starts that over (the member gave up on it).
    pub fn wanted(&mut self, peer: PeerId, content: u64) {
        let Some(state) = self.peers.get_mut(&peer) else {
            return;
        };
        let named = state
            .sent
            .as_ref()
            .is_some_and(|list| list.iter().any(|entry| entry.content == content));
        if !named || state.level.is_none() {
            return;
        }
        match state.queue.iter_mut().find(|want| want.content == content) {
            Some(want) => want.offset = 0,
            None => state.queue.push_back(Want { content, offset: 0 }),
        }
    }

    /// The chunks to send now, in turns of one chunk per member, as long as the pace has room.
    /// `backlog` says how many bytes wait to be delivered to a peer (`None`: it can't tell); a
    /// member with [`WINDOW`] or more waiting gets none now, nor one whose bytes allowance is
    /// spent. `data` is a reference's cooked bytes; what it doesn't know is dropped from the
    /// queues.
    pub fn chunks<'a>(
        &mut self,
        now: u64,
        mut backlog: impl FnMut(PeerId) -> Option<usize>,
        data: impl Fn(u64) -> Option<&'a [u8]>,
    ) -> Vec<Chunk> {
        self.pace.refill(PACE_RATE, now);
        let mut order: Vec<PeerId> = self
            .peers
            .iter()
            .filter(|(_, peer)| !peer.queue.is_empty())
            .map(|(&id, _)| id)
            .collect();
        if let Some(turn) = self.turn {
            let after = order.partition_point(|&id| id <= turn);
            order.rotate_left(after);
        }
        let mut serving: Vec<Serving> = order
            .into_iter()
            .map(|peer| Serving {
                peer,
                room: backlog(peer).map(|waiting| WINDOW.saturating_sub(waiting)),
            })
            .filter(|s| s.room != Some(0))
            .collect();

        let mut chunks = Vec::new();
        'turns: while !serving.is_empty() {
            let mut at = 0;
            while at < serving.len() {
                let peer = serving[at].peer;
                let Some(state) = self.peers.get_mut(&peer) else {
                    serving.remove(at);
                    continue;
                };
                match next_chunk(state, peer, &mut self.pace, now, &data) {
                    Step::Sent(chunk) => {
                        let room = &mut serving[at].room;
                        *room = room.map(|room| room.saturating_sub(chunk.data.len()));
                        chunks.push(chunk);
                        self.turn = Some(peer);
                        if serving[at].room == Some(0) {
                            serving.remove(at);
                        } else {
                            at += 1;
                        }
                    }
                    Step::Retry => {}
                    Step::Out => {
                        serving.remove(at);
                    }
                    Step::PaceFull => break 'turns,
                }
            }
        }
        chunks
    }

    /// Data bytes sent to `peer` so far.
    pub fn sent_to(&self, peer: PeerId) -> u64 {
        self.peers.get(&peer).map_or(0, |p| p.given)
    }
}

/// Takes the next piece of `peer`'s first wanted reference, if its allowance and the pace have room.
fn next_chunk<'a>(
    peer: &mut Peer,
    id: PeerId,
    pace: &mut Bucket,
    now: u64,
    data: &impl Fn(u64) -> Option<&'a [u8]>,
) -> Step {
    let Some(want) = peer.queue.front_mut() else {
        return Step::Out;
    };
    let Some(bytes) = data(want.content) else {
        peer.queue.pop_front();
        return Step::Retry;
    };
    let Some(offset) = u32::try_from(want.offset)
        .ok()
        .filter(|_| want.offset < bytes.len())
    else {
        // All of it went (or it got shorter than what went), or it's too big to address.
        peer.queue.pop_front();
        return Step::Retry;
    };
    let len = CHUNK.min(bytes.len() - want.offset);
    let Some(allowance) = &mut peer.allowance else {
        return Step::Out;
    };
    if !allowance.holds(now, len as u64) {
        return Step::Out;
    }
    if pace.level < len as f64 {
        return Step::PaceFull;
    }
    pace.level -= len as f64;
    allowance.take(len as u64);
    peer.given += len as u64;
    let chunk = Chunk {
        peer: id,
        content: want.content,
        offset,
        data: bytes[want.offset..want.offset + len].to_vec(),
    };
    want.offset += len;
    if want.offset >= bytes.len() {
        peer.queue.pop_front();
    }
    Step::Sent(chunk)
}

// --- Member side ---

/// Why a reference isn't taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    /// The triangles of the ones before it already fill the level's total.
    Triangles,
    Pixels,
    /// Past the level's number of references.
    Count,
    /// More bytes than the level can receive at once.
    Bytes,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// To be asked for.
    Waiting,
    Receiving {
        got: u32,
    },
    /// All of it arrived; the mod checks it.
    Checking,
    Ready,
    Skipped(Skip),
    Failed,
}

/// A reference's bytes, all there: for the mod to check and load.
#[derive(Debug, PartialEq, Eq)]
pub struct Finished {
    pub content: u64,
    pub bytes: Vec<u8>,
}

/// The reference being received.
struct Download {
    content: u64,
    /// What the list declared; no more is taken.
    total: u32,
    bytes: Vec<u8>,
    /// When data last arrived (or it was asked for).
    last: u64,
}

impl Download {
    /// Bytes of its allowance that were taken for it and not used.
    fn unused(&self) -> u64 {
        u64::from(self.total) - self.bytes.len() as u64
    }
}

/// What a member does with the host's references: judges the list under its level, asks for one
/// reference at a time, and takes the data only as asked for.
pub struct Receiving {
    level: Option<Level>,
    allowance: Option<Allowance>,
    /// The host's list, kept while off.
    list: Vec<Entry>,
    /// Why each reference of `list` isn't taken, if it isn't.
    skips: Vec<Option<Skip>>,
    /// Contents that were checked and are loaded, by hash.
    ready: HashSet<u64>,
    checking: HashSet<u64>,
    failed: HashSet<u64>,
    current: Option<Download>,
    /// After a level change, until the host's list for it comes: what the host held for us is gone.
    awaiting_list: bool,
    ignored: u64,
}

impl Receiving {
    pub fn new(level: Option<Level>, now: u64) -> Self {
        Self {
            level,
            allowance: level.map(|level| Allowance::new(level, now)),
            list: Vec::new(),
            skips: Vec::new(),
            ready: HashSet::new(),
            checking: HashSet::new(),
            failed: HashSet::new(),
            current: None,
            awaiting_list: false,
            ignored: 0,
        }
    }

    pub fn level(&self) -> Option<Level> {
        self.level
    }

    /// Takes another level (the mod tells the host). What was being received is dropped, since the
    /// host drops what it queued, and nothing is asked for before its list for the new level
    /// comes. Off shows nothing, but the list is kept for going back on.
    pub fn set_level(&mut self, level: Option<Level>, now: u64) {
        if level == self.level {
            return;
        }
        self.drop_download();
        self.level = level;
        if let Some(level) = level {
            match &mut self.allowance {
                Some(allowance) => allowance.change(level, now),
                None => self.allowance = Some(Allowance::new(level, now)),
            }
            self.awaiting_list = true;
        }
        self.judge();
    }

    /// The host's list, which replaces the last. Each reference is judged in order against the
    /// level's totals. What is ready stays ready (unless over the totals now), and a download of
    /// something no longer listed or allowed is dropped.
    pub fn list(&mut self, mut entries: Vec<Entry>) {
        entries.truncate(MAX_ENTRIES);
        self.list = entries;
        self.awaiting_list = false;
        let list = &self.list;
        self.failed
            .retain(|content| list.iter().any(|entry| entry.content == *content));
        self.judge();
        self.drop_disallowed_download();
    }

    /// Works out why references are skipped.
    fn judge(&mut self) {
        let Some(level) = self.level else {
            self.skips = vec![None; self.list.len()];
            return;
        };
        let caps = level.caps();
        let (mut triangles, mut pixels) = (0u64, 0u64);
        self.skips = self
            .list
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let skip = if i >= caps.references {
                    Some(Skip::Count)
                } else if u64::from(entry.bytes) > caps.burst_bytes {
                    Some(Skip::Bytes)
                } else if triangles + u64::from(entry.triangles) > caps.triangles {
                    Some(Skip::Triangles)
                } else if pixels + u64::from(entry.pixels) > caps.pixels {
                    Some(Skip::Pixels)
                } else {
                    None
                };
                if skip.is_none() {
                    triangles += u64::from(entry.triangles);
                    pixels += u64::from(entry.pixels);
                }
                skip
            })
            .collect();
    }

    fn status_at(&self, i: usize) -> Status {
        let entry = &self.list[i];
        if let Some(skip) = self.skips[i] {
            Status::Skipped(skip)
        } else if self.ready.contains(&entry.content) {
            Status::Ready
        } else if entry.bytes == 0 || self.failed.contains(&entry.content) {
            Status::Failed
        } else if self.checking.contains(&entry.content) {
            Status::Checking
        } else if let Some(download) = self.current.as_ref().filter(|d| d.content == entry.content)
        {
            Status::Receiving {
                got: download.bytes.len() as u32,
            }
        } else {
            Status::Waiting
        }
    }

    /// Drops the download, giving back the allowance it didn't use.
    fn drop_download(&mut self) {
        if let Some(download) = self.current.take()
            && let Some(allowance) = &mut self.allowance
        {
            allowance.refund(download.unused());
        }
    }

    /// Drops the download if no reference of the list allows it any more.
    fn drop_disallowed_download(&mut self) {
        let allowed = self.current.as_ref().is_none_or(|download| {
            self.list.iter().zip(&self.skips).any(|(entry, skip)| {
                skip.is_none() && entry.content == download.content && entry.bytes == download.total
            })
        });
        if !allowed {
            self.drop_download();
        }
    }

    /// The reference to ask the host for now, if any: one at a time, the first still waiting in
    /// list order, once the allowance holds its bytes (which are taken here). A download that got
    /// no data for [`DATA_TIMEOUT`] is dropped first, so it is asked for again.
    pub fn next_want(&mut self, now: u64) -> Option<u64> {
        if self
            .current
            .as_ref()
            .is_some_and(|download| now.saturating_sub(download.last) >= DATA_TIMEOUT)
        {
            self.drop_download();
        }
        if self.current.is_some() || self.awaiting_list || self.level.is_none() {
            return None;
        }
        let at = (0..self.list.len()).find(|&i| self.status_at(i) == Status::Waiting)?;
        let entry = &self.list[at];
        let allowance = self.allowance.as_mut()?;
        if !allowance.holds(now, u64::from(entry.bytes)) {
            return None;
        }
        allowance.take(u64::from(entry.bytes));
        self.current = Some(Download {
            content: entry.content,
            total: entry.bytes,
            bytes: Vec::new(),
            last: now,
        });
        Some(entry.content)
    }

    /// A chunk from the host. Only the one being received counts, at the place it goes and not
    /// past what the list declared; anything else is ignored (and counted). The last bytes give
    /// the whole reference, for the mod to check ([`Receiving::ready`] or [`Receiving::failed`]).
    pub fn data(&mut self, content: u64, offset: u32, data: &[u8], now: u64) -> Option<Finished> {
        let fits = self.current.as_ref().is_some_and(|download| {
            download.content == content
                && offset as usize == download.bytes.len()
                && !data.is_empty()
                && download.bytes.len() + data.len() <= download.total as usize
        });
        let Some(download) = self.current.as_mut().filter(|_| fits) else {
            self.ignored += 1;
            return None;
        };
        download.bytes.extend_from_slice(data);
        download.last = now;
        if download.bytes.len() < download.total as usize {
            return None;
        }
        let download = self.current.take()?;
        self.checking.insert(download.content);
        Some(Finished {
            content: download.content,
            bytes: download.bytes,
        })
    }

    /// The mod checked `content` and loaded it.
    pub fn ready(&mut self, content: u64) {
        if self.checking.remove(&content) {
            self.ready.insert(content);
        }
    }

    /// The mod found `content` broken: it isn't asked for again while the list names it.
    pub fn failed(&mut self, content: u64) {
        self.checking.remove(&content);
        if self.list.iter().any(|entry| entry.content == content) {
            self.failed.insert(content);
        }
    }

    /// `content` is already held from earlier in the session: ready without asking.
    pub fn known(&mut self, content: u64) {
        if self.current.as_ref().is_some_and(|d| d.content == content) {
            self.drop_download();
        }
        self.checking.remove(&content);
        self.failed.remove(&content);
        self.ready.insert(content);
    }

    /// Forgets that `content` was ready (its files are gone).
    pub fn forget(&mut self, content: u64) {
        if self.current.as_ref().is_some_and(|d| d.content == content) {
            self.drop_download();
        }
        self.ready.remove(&content);
        self.checking.remove(&content);
    }

    /// The references that are ready, in list order. None while off.
    pub fn shown(&self) -> Vec<&Entry> {
        self.entries()
            .into_iter()
            .filter(|(_, status)| *status == Status::Ready)
            .map(|(entry, _)| entry)
            .collect()
    }

    /// Every reference of the list with where it stands. None while off.
    pub fn entries(&self) -> Vec<(&Entry, Status)> {
        if self.level.is_none() {
            return Vec::new();
        }
        (0..self.list.len())
            .map(|i| (&self.list[i], self.status_at(i)))
            .collect()
    }

    /// Chunks that were ignored so far.
    pub fn ignored(&self) -> u64 {
        self.ignored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1 << 20;

    fn look(x: f64) -> Look {
        Look {
            position: [x, 0.0, 0.0],
            rotation: [0.0; 3],
            scale: [1.0; 3],
            mirror: [false, true, false],
            opacity: 0.5,
            color: [1.0, 0.5, 0.25],
            image: Some(ImageLook {
                width: 120.0,
                flip_x: true,
                flip_y: false,
                double_sided: true,
                pixelated: false,
                pivot: 1,
            }),
        }
    }

    fn edits_round_trip(edits: &[Edit]) -> Result<Vec<Edit>> {
        let mut w = Writer::new();
        write_edits(&mut w, edits);
        let bytes = w.finish();
        let mut r = Reader::new(&bytes);
        let read = read_edits(&mut r)?;
        r.finish()?;
        Ok(read)
    }

    #[test]
    fn edits_round_trip_and_refuse_nonsense() {
        let edits = vec![
            Edit::Set {
                id: "a".into(),
                look: look(5.0),
            },
            Edit::Remove { id: "b-2_c".into() },
        ];
        assert_eq!(edits_round_trip(&edits).unwrap(), edits);
        let set = |id: &str, look: Look| Edit::Set {
            id: id.into(),
            look,
        };
        assert!(edits_round_trip(&[set("a", look(f64::NAN))]).is_err());
        assert!(edits_round_trip(&[set("a", look(1e9))]).is_err());
        assert!(edits_round_trip(&[set("a/b", look(0.0))]).is_err());
        assert!(edits_round_trip(&[set("", look(0.0))]).is_err());
        let mut pivot = look(0.0);
        pivot.image.as_mut().unwrap().pivot = PIVOTS;
        assert!(edits_round_trip(&[set("a", pivot)]).is_err());
        let mut color = look(0.0);
        color.color[1] = f32::INFINITY;
        assert!(edits_round_trip(&[set("a", color)]).is_err());
        let twice = [set("a", look(1.0)), Edit::Remove { id: "a".into() }];
        assert!(edits_round_trip(&twice).is_err());
    }

    #[test]
    fn coalescing_keeps_each_references_last_edit() {
        let set = |id: &str, x| Edit::Set {
            id: id.into(),
            look: look(x),
        };
        let edits = vec![set("a", 1.0), set("b", 1.0), set("a", 2.0), set("a", 3.0)];
        assert_eq!(coalesce(edits), vec![set("b", 1.0), set("a", 3.0)]);
        let removed = vec![set("a", 1.0), Edit::Remove { id: "a".into() }];
        assert_eq!(coalesce(removed), vec![Edit::Remove { id: "a".into() }]);
    }

    fn entry(id: &str, content: u64, bytes: u32) -> Entry {
        Entry {
            id: id.into(),
            name: format!("name {id}"),
            kind: Kind::Model,
            content,
            bytes,
            triangles: 1000,
            pixels: 0,
            simplified: false,
            position: [1.0, -2.0, 3.5],
            rotation: [0.0, 90.0, -45.0],
            scale: [1.0; 3],
            mirror: [false, true, false],
            opacity: 0.75,
            color: [1.0, 0.5, 0.25],
            image: None,
        }
    }

    fn image(id: &str, content: u64, bytes: u32) -> Entry {
        Entry {
            kind: Kind::Image,
            triangles: 0,
            pixels: 1000,
            image: Some(ImageLook {
                width: 120.0,
                flip_x: true,
                flip_y: false,
                double_sided: true,
                pixelated: false,
                pivot: 4,
            }),
            ..entry(id, content, bytes)
        }
    }

    /// `n` bytes that depend on `seed`.
    fn payload(seed: u64, n: usize) -> Vec<u8> {
        let mut state = seed | 1;
        (0..n)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect()
    }

    // --- Levels and the list on the wire ---

    #[test]
    fn levels_on_the_wire() {
        assert_eq!(Level::from_wire(Level::to_wire(None)), Some(None));
        for level in Level::ALL {
            assert_eq!(
                Level::from_wire(Level::to_wire(Some(level))),
                Some(Some(level))
            );
        }
        assert_eq!(Level::from_wire(4), None);
        assert_eq!(Level::from_wire(255), None);
    }

    #[test]
    fn levels_grow() {
        let caps: Vec<Caps> = Level::ALL.iter().map(|l| l.caps()).collect();
        for pair in caps.windows(2) {
            assert!(pair[0].triangles < pair[1].triangles);
            assert!(pair[0].longest_side < pair[1].longest_side);
            assert!(pair[0].pixels < pair[1].pixels);
            assert!(pair[0].references < pair[1].references);
            assert!(pair[0].burst_bytes < pair[1].burst_bytes);
        }
        assert_eq!(caps[0].burst_bytes, 16 * MIB);
        assert_eq!(caps[2].references, MAX_ENTRIES);
    }

    fn encode(entries: &[Entry]) -> Vec<u8> {
        let mut w = Writer::new();
        write_entries(&mut w, entries);
        w.finish()
    }

    fn decode(bytes: &[u8]) -> Result<Vec<Entry>> {
        let mut r = Reader::new(bytes);
        let entries = read_entries(&mut r)?;
        r.finish()?;
        Ok(entries)
    }

    #[test]
    fn entries_round_trip() {
        let list = vec![
            entry("a", 1, 100),
            image("img_2-b", u64::MAX, u32::MAX),
            Entry {
                name: "Zażółć gęślą jaźń".into(),
                simplified: true,
                position: [-1e7, 1e7, 0.0],
                rotation: [1e5, -1e5, -0.0],
                scale: [-1e6, 1e6, 0.001],
                ..entry("c", 3, 5)
            },
        ];
        assert_eq!(decode(&encode(&list)), Ok(list));
        assert_eq!(decode(&encode(&[])), Ok(Vec::new()));
    }

    /// Refusals: `change` makes the entry wrong in one way.
    fn refused(change: impl FnOnce(&mut Entry)) -> bool {
        let mut bad = image("x", 1, 10);
        change(&mut bad);
        matches!(decode(&encode(&[bad])), Err(DecodeError::BadValue(_)))
    }

    #[test]
    fn each_kind_of_bad_entry_is_refused() {
        assert!(!refused(|_| {}));
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1.1e7, -1.1e7] {
            assert!(refused(|e| e.position[1] = bad), "position {bad}");
        }
        for bad in [f64::NAN, 1.1e5, -1.1e5] {
            assert!(refused(|e| e.rotation[2] = bad), "rotation {bad}");
        }
        for bad in [f64::NAN, 1.1e6, -1.1e6] {
            assert!(refused(|e| e.scale[0] = bad), "scale {bad}");
        }
        for bad in [f64::NAN, f64::INFINITY, 1.1e7] {
            assert!(
                refused(|e| e.image.as_mut().unwrap().width = bad),
                "width {bad}"
            );
        }
        assert!(refused(|e| e.opacity = f32::NAN));
        assert!(refused(|e| e.opacity = f32::INFINITY));
        assert!(refused(|e| e.color[2] = f32::NAN));
        assert!(refused(|e| e.image.as_mut().unwrap().pivot = 16));
        // An image has a look, a model has none.
        assert!(refused(|e| e.image = None));
        assert!(refused(|e| e.kind = Kind::Model));
        assert!(refused(|e| {
            e.kind = Kind::Model;
            e.image = None;
            e.id.clear();
        }));
        // Ids: 1 to 32 bytes of letters, digits, `_` and `-`.
        for bad in ["", "a b", "a/b", "..", "é", "a\n", &"x".repeat(33)] {
            if bad.len() > ID_BYTES {
                // Too long is a different refusal, but a refusal.
                assert!(decode(&encode(&[image(bad, 1, 1)])).is_err());
            } else {
                assert!(refused(|e| e.id = bad.into()), "id {bad:?}");
            }
        }
        assert!(!refused(|e| e.id = "x".repeat(32)));
        assert!(!refused(|e| e.id = "Az09_-".into()));
    }

    #[test]
    fn lists_are_bounded_and_have_no_repeated_ids() {
        let many: Vec<Entry> = (0..MAX_ENTRIES)
            .map(|n| entry(&format!("r{n}"), n as u64, 1))
            .collect();
        assert_eq!(decode(&encode(&many)).map(|l| l.len()), Ok(MAX_ENTRIES));

        // A count over the cap is refused when read, even with the data behind it.
        let mut over = Writer::new();
        over.count(MAX_ENTRIES + 1);
        let mut bytes = over.finish();
        bytes.extend(vec![0; 10_000]);
        assert!(matches!(decode(&bytes), Err(DecodeError::TooLong { .. })));

        // The writer never writes more than the reader takes.
        let more: Vec<Entry> = (0..MAX_ENTRIES + 5)
            .map(|n| entry(&format!("r{n}"), n as u64, 1))
            .collect();
        assert_eq!(decode(&encode(&more)).map(|l| l.len()), Ok(MAX_ENTRIES));

        let twice = vec![
            entry("same", 1, 1),
            image("other", 2, 1),
            entry("same", 3, 1),
        ];
        assert!(matches!(
            decode(&encode(&twice)),
            Err(DecodeError::BadValue(_))
        ));
        // Same content under two ids is fine (the same picture twice).
        let shared = vec![entry("a", 1, 1), entry("b", 1, 1)];
        assert_eq!(decode(&encode(&shared)), Ok(shared));
    }

    #[test]
    fn names_are_cut_and_cleaned() {
        let mut messy = entry("a", 1, 1);
        messy.name = "tab\there\nnew\u{0}line\u{85}".into();
        let read = decode(&encode(&[messy])).unwrap();
        assert_eq!(read[0].name, "tab here new line ");

        // The writer cuts at a character, so what it writes is read.
        let mut long = entry("a", 1, 1);
        long.name = "é".repeat(100);
        let read = decode(&encode(&[long])).unwrap();
        assert_eq!(read[0].name, "é".repeat(64));

        // A name over the cap on the wire is refused, like other strings.
        let mut w = Writer::new();
        w.count(1);
        w.str("a");
        w.str(&"n".repeat(NAME_BYTES + 1));
        assert!(matches!(
            decode(&w.finish()),
            Err(DecodeError::TooLong { .. })
        ));
    }

    #[test]
    fn every_cut_and_flip_of_a_list_is_handled() {
        let bytes = encode(&[entry("a", 1, 100), image("b", 2, 50)]);
        for len in 0..bytes.len() {
            assert!(decode(&bytes[..len]).is_err(), "cut at {len}");
        }
        for at in 0..bytes.len() {
            let mut flipped = bytes.clone();
            flipped[at] ^= 0xA5;
            // Whatever comes out, it can be written and read again as it is.
            if let Ok(list) = decode(&flipped) {
                assert_eq!(decode(&encode(&list)), Ok(list));
            }
        }
    }

    // --- Splitting ---

    fn sum(targets: &[Target], f: impl Fn(&Target) -> u64) -> u64 {
        targets.iter().map(f).sum()
    }

    #[test]
    fn under_the_caps_nothing_changes() {
        let caps = Level::High.caps();
        let sizes = [
            Size {
                triangles: 100_000,
                pixels: 1_000_000,
            },
            Size {
                triangles: 50_000,
                pixels: 0,
            },
        ];
        let targets = split(caps, &sizes);
        assert_eq!(targets[0].triangles, 100_000);
        assert_eq!(targets[1].triangles, 50_000);
        assert_eq!(targets[0].pixels, 1_000_000);
        assert_eq!(targets[1].pixels, 0);
        assert!(targets.iter().all(|t| t.longest_side == caps.longest_side));
    }

    #[test]
    fn over_a_cap_the_total_is_shared_in_proportion() {
        let caps = Level::High.caps();
        let sizes = [
            Size {
                triangles: 300_000,
                pixels: 8_000_000,
            },
            Size {
                triangles: 100_000,
                pixels: 24_000_000,
            },
        ];
        let targets = split(caps, &sizes);
        assert_eq!(targets[0].triangles, 187_500);
        assert_eq!(targets[1].triangles, 62_500);
        assert_eq!(targets[0].pixels, 4_000_000);
        assert_eq!(targets[1].pixels, 12_000_000);
        // Triangles and pixels are over independently: one over doesn't shrink the other.
        let only_triangles = split(
            caps,
            &[Size {
                triangles: 1_000_000,
                pixels: 5,
            }],
        );
        assert_eq!(only_triangles[0].triangles, 250_000);
        assert_eq!(only_triangles[0].pixels, 5);
    }

    #[test]
    fn bytes_are_shared_by_estimated_size() {
        let caps = Level::High.caps();
        let sizes = [
            Size {
                triangles: 10_000,
                pixels: 0,
            },
            Size {
                triangles: 0,
                pixels: 120_000,
            },
            Size::default(),
        ];
        let targets = split(caps, &sizes);
        // 120,000 bytes each by weight, and none for nothing.
        assert_eq!(targets[0].bytes, targets[1].bytes);
        assert_eq!(targets[2].bytes, 0);
        assert!(targets[0].bytes + targets[1].bytes <= caps.burst_bytes);
        assert!(targets[0].bytes + targets[1].bytes > caps.burst_bytes - 2);
        // A huge model doesn't starve the picture next to it: it is judged as it will be fitted.
        let huge = split(
            caps,
            &[
                Size {
                    triangles: 50_000_000,
                    pixels: 0,
                },
                Size {
                    triangles: 0,
                    pixels: 3_000_000,
                },
            ],
        );
        assert!(huge[1].bytes > caps.burst_bytes / 4);
    }

    #[test]
    fn splitting_never_exceeds_the_caps() {
        let mut state = 7u64;
        let mut next = move |below: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % below
        };
        for round in 0..2000 {
            let level = Level::ALL[round % 3];
            let caps = level.caps();
            let n = next(300) as usize;
            let scale = [10, 1_000, 1_000_000, u64::MAX / 4][next(4) as usize];
            let sizes: Vec<Size> = (0..n)
                .map(|_| Size {
                    triangles: next(scale),
                    pixels: next(scale),
                })
                .collect();
            let targets = split(caps, &sizes);
            assert_eq!(targets.len(), n);
            assert!(sum(&targets, |t| t.triangles) <= caps.triangles);
            assert!(sum(&targets, |t| t.pixels) <= caps.pixels);
            assert!(sum(&targets, |t| t.bytes) <= caps.burst_bytes);
            for (i, (size, target)) in sizes.iter().zip(&targets).enumerate() {
                assert!(target.triangles <= size.triangles, "{i}");
                assert!(target.pixels <= size.pixels, "{i}");
                if i >= caps.references {
                    assert_eq!(target.bytes + target.triangles + target.pixels, 0);
                    assert_eq!(target.longest_side, 0);
                }
            }
        }
    }

    #[test]
    fn splitting_handles_nothing_and_everything_past_the_count() {
        let caps = Level::High.caps();
        assert!(split(caps, &[]).is_empty());
        let sizes = vec![
            Size {
                triangles: 10,
                pixels: 10
            };
            caps.references + 3
        ];
        let targets = split(caps, &sizes);
        assert_eq!(targets.len(), sizes.len());
        assert!(targets[..caps.references].iter().all(|t| t.triangles == 10));
        for target in &targets[caps.references..] {
            assert_eq!(
                *target,
                Target {
                    triangles: 0,
                    longest_side: 0,
                    pixels: 0,
                    bytes: 0
                }
            );
        }
        // All of the references have no size: nothing to share, nothing divided by zero.
        let flat = split(caps, &[Size::default(); 4]);
        assert!(flat.iter().all(|t| t.triangles == 0 && t.bytes == 0));
    }

    // --- The host's pace ---

    const A: PeerId = 11;
    const B: PeerId = 12;

    /// A sharing where `peers` are at `level` and hold `list`, with every wait over.
    fn sharing_with(peers: &[PeerId], level: Level, list: Vec<Entry>) -> Sharing {
        let mut sharing = Sharing::default();
        sharing.publish(level, list);
        for &peer in peers {
            sharing.level_asked(peer, Some(level), 0);
        }
        assert_eq!(sharing.lists(0).len(), peers.len());
        sharing
    }

    fn no_backlog(_: PeerId) -> Option<usize> {
        None
    }

    #[test]
    fn a_member_gets_a_list_when_it_changes_and_asks_for_what_it_names() {
        let mut sharing = Sharing::default();
        sharing.publish(Level::High, vec![entry("a", 1, 10)]);
        // Nobody asked for a level: nothing to send.
        assert!(sharing.lists(0).is_empty());
        sharing.level_asked(A, Some(Level::High), 0);
        assert_eq!(sharing.levels(), vec![Level::High]);
        let lists = sharing.lists(0);
        assert_eq!(lists, vec![(A, vec![entry("a", 1, 10)])]);
        assert!(
            sharing.lists(5_000).is_empty(),
            "the same list isn't sent again"
        );

        // A change is sent, at most twice a second.
        sharing.publish(Level::High, vec![entry("a", 1, 10), entry("b", 2, 10)]);
        assert_eq!(sharing.lists(5_000).len(), 1);
        sharing.publish(Level::High, vec![entry("b", 2, 10)]);
        sharing.lists(5_000);
        sharing.publish(Level::High, vec![entry("c", 3, 10)]);
        assert!(
            sharing.lists(5_000).is_empty(),
            "a third within the second waits"
        );
        assert!(sharing.lists(5_100).is_empty());
        assert_eq!(sharing.lists(5_600).len(), 1, "and goes on a later call");

        // Only what its list names is queued.
        let data = payload(1, 10);
        sharing.wanted(A, 1);
        sharing.wanted(A, 99);
        sharing.wanted(B, 3);
        let out = sharing.chunks(6_000, no_backlog, |c| {
            (c == 1 || c == 3).then_some(&data[..])
        });
        assert!(out.is_empty(), "1 isn't listed any more, 99 never was");
        sharing.wanted(A, 3);
        let out = sharing.chunks(6_000, no_backlog, |c| (c == 3).then_some(&data[..]));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].peer, A);
        assert_eq!(out[0].data, data);
    }

    #[test]
    fn chunks_come_in_order_and_finish() {
        let data = payload(5, CHUNK * 2 + 100);
        let mut sharing = sharing_with(&[A], Level::High, vec![entry("a", 1, data.len() as u32)]);
        sharing.wanted(A, 1);
        let out = sharing.chunks(0, no_backlog, |_| Some(&data[..]));
        let offsets: Vec<u32> = out.iter().map(|c| c.offset).collect();
        assert_eq!(offsets, vec![0, CHUNK as u32, 2 * CHUNK as u32]);
        let joined: Vec<u8> = out.iter().flat_map(|c| c.data.iter().copied()).collect();
        assert_eq!(joined, data);
        assert!(out.iter().all(|c| c.data.len() <= CHUNK));
        assert_eq!(sharing.sent_to(A), data.len() as u64);
        // Nothing more: it left the queue.
        assert!(
            sharing
                .chunks(10_000, no_backlog, |_| Some(&data[..]))
                .is_empty()
        );
        // Asking again sends it again.
        sharing.wanted(A, 1);
        assert_eq!(
            sharing
                .chunks(10_000, no_backlog, |_| Some(&data[..]))
                .len(),
            3
        );
    }

    #[test]
    fn a_backlog_at_the_window_stops_chunks() {
        let data = payload(5, CHUNK * 10);
        let mut sharing = sharing_with(&[A], Level::High, vec![entry("a", 1, data.len() as u32)]);
        sharing.wanted(A, 1);
        assert!(
            sharing
                .chunks(0, |_| Some(WINDOW), |_| Some(&data[..]))
                .is_empty()
        );
        assert!(
            sharing
                .chunks(0, |_| Some(WINDOW + 5), |_| Some(&data[..]))
                .is_empty()
        );
        // Room for two chunks: bytes given in the call count on top of the backlog.
        let out = sharing.chunks(1_000, |_| Some(WINDOW - 2 * CHUNK), |_| Some(&data[..]));
        assert_eq!(out.len(), 2);
        // Without a backlog figure only the pace limits: 64 KiB at once, then time.
        let out = sharing.chunks(1_000, no_backlog, |_| Some(&data[..]));
        assert!(out.len() <= 4, "the pace's burst is 64 KiB");
    }

    #[test]
    fn the_pace_is_shared_between_members_in_turns() {
        let data = payload(5, 4 * MIB as usize);
        let list = vec![entry("a", 1, data.len() as u32)];
        let mut sharing = sharing_with(&[A, B], Level::Extreme, list);
        sharing.wanted(A, 1);
        sharing.wanted(B, 1);
        let mut by = HashMap::new();
        let mut order = Vec::new();
        for step in 0..40 {
            for chunk in sharing.chunks(step * 50, no_backlog, |_| Some(&data[..])) {
                *by.entry(chunk.peer).or_insert(0usize) += chunk.data.len();
                order.push(chunk.peer);
            }
        }
        // 2 seconds at 1 MiB/s plus the burst, split evenly.
        let (a, b) = (by[&A], by[&B]);
        assert!(a.abs_diff(b) <= CHUNK, "{a} against {b}");
        assert!((a + b) as f64 <= 2.0 * PACE + 64.0 * 1024.0 + 1.0);
        assert!((a + b) as f64 >= 1.8 * PACE);
        // Never one member twice in a row while the other waits.
        for pair in order.windows(2) {
            assert_ne!(pair[0], pair[1]);
        }
    }

    /// Calls `chunks` every 50 ms from `from` to `to` (milliseconds) and gives what it sends.
    fn run(sharing: &mut Sharing, from: u64, to: u64, data: &[u8]) -> Vec<Chunk> {
        (from..to)
            .step_by(50)
            .flat_map(|now| sharing.chunks(now, no_backlog, |_| Some(data)))
            .collect()
    }

    #[test]
    fn the_turn_goes_on_between_calls() {
        let data = payload(5, 2 * MIB as usize);
        let list = vec![entry("a", 1, data.len() as u32)];
        let mut sharing = sharing_with(&[A, B], Level::Extreme, list);
        sharing.wanted(A, 1);
        sharing.wanted(B, 1);
        // The first call spends the pace's burst; then there is room for one chunk per call.
        let mut served: Vec<PeerId> = Vec::new();
        for step in 0..12 {
            let out = sharing.chunks(step * 16, no_backlog, |_| Some(&data[..]));
            served.extend(out.iter().map(|c| c.peer));
        }
        assert!(served.len() >= 8, "{served:?}");
        assert_eq!(served[0], A);
        for pair in served.windows(2) {
            assert_ne!(pair[0], pair[1], "{served:?}");
        }
    }

    #[test]
    fn a_members_bytes_run_out_and_refill_with_time() {
        let level = Level::High;
        let burst = level.caps().burst_bytes;
        let data = payload(9, 2 * MIB as usize);
        let list: Vec<Entry> = (0..12)
            .map(|n| entry(&format!("r{n}"), n, data.len() as u32))
            .collect();
        let mut sharing = sharing_with(&[A], level, list);
        // Ask for everything every 20 seconds for 200: it stops at the burst, and the refill of
        // 200 seconds comes on top.
        let mut sent = 0u64;
        for round in 0..10 {
            for n in 0..12 {
                sharing.wanted(A, n);
            }
            let chunks = run(&mut sharing, round * 20_000, (round + 1) * 20_000, &data);
            sent += chunks.iter().map(|c| c.data.len() as u64).sum::<u64>();
        }
        assert!(sent >= burst - CHUNK as u64, "{sent}");
        assert!(sent <= burst + burst / 3 + CHUNK as u64, "{sent}");
        assert_eq!(sharing.sent_to(A), sent);

        // After a full refill time it is full again: what it still has queued goes, at the pace.
        let now = 200_000 + REFILL_SECONDS * 1000;
        let more: usize = run(&mut sharing, now, now + 10_000, &data)
            .iter()
            .map(|c| c.data.len())
            .sum();
        assert!(more as f64 >= 9.0 * PACE, "{more}");
    }

    #[test]
    fn a_member_with_no_bytes_left_does_not_hold_up_another() {
        let data = payload(9, 4 * MIB as usize);
        let list = vec![entry("a", 1, data.len() as u32)];
        let mut sharing = sharing_with(&[A, B], Level::High, list);
        // A spends its burst.
        for round in 0..6 {
            sharing.wanted(A, 1);
            run(&mut sharing, round * 10_000, (round + 1) * 10_000, &data);
        }
        let spent = sharing.sent_to(A);
        assert!((15 * MIB..=18 * MIB).contains(&spent), "{spent}");
        sharing.wanted(A, 1);
        sharing.wanted(B, 1);
        let out = run(&mut sharing, 60_000, 70_000, &data);
        let to_b: usize = out
            .iter()
            .filter(|c| c.peer == B)
            .map(|c| c.data.len())
            .sum();
        assert_eq!(to_b, data.len());
        assert!(sharing.sent_to(A) < spent + 4 * MIB);
    }

    #[test]
    fn content_the_host_lost_leaves_the_queue() {
        let data = payload(1, 100);
        let list = vec![entry("a", 1, 100), entry("b", 2, 100)];
        let mut sharing = sharing_with(&[A], Level::High, list);
        sharing.wanted(A, 1);
        sharing.wanted(A, 2);
        let out = sharing.chunks(0, no_backlog, |c| (c == 2).then_some(&data[..]));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content, 2);
        // And it never loops on content that is gone.
        sharing.wanted(A, 1);
        assert!(sharing.chunks(1_000, no_backlog, |_| None).is_empty());
        assert!(
            sharing
                .chunks(2_000, no_backlog, |_| Some(&[][..]))
                .is_empty()
        );
    }

    #[test]
    fn asking_for_what_is_on_its_way_starts_it_over() {
        let data = payload(1, CHUNK * 3);
        let mut sharing = sharing_with(&[A], Level::High, vec![entry("a", 1, data.len() as u32)]);
        sharing.wanted(A, 1);
        let out = sharing.chunks(0, |_| Some(WINDOW - CHUNK), |_| Some(&data[..]));
        assert_eq!(out.len(), 1);
        sharing.wanted(A, 1);
        let out = sharing.chunks(1_000, no_backlog, |_| Some(&data[..]));
        assert_eq!(out.len(), 3, "once, from its start");
        assert_eq!(out[0].offset, 0);
    }

    #[test]
    fn level_changes_are_limited_and_reset_the_member() {
        let data = payload(1, CHUNK);
        let mut sharing = Sharing::default();
        sharing.publish(Level::High, vec![entry("a", 1, CHUNK as u32)]);
        sharing.publish(
            Level::Extreme,
            vec![entry("a", 1, CHUNK as u32), entry("b", 2, 5)],
        );
        sharing.level_asked(A, Some(Level::High), 0);
        assert_eq!(sharing.lists(0).len(), 1);
        sharing.wanted(A, 1);
        // Another level: what it queued is gone and the new list goes out.
        sharing.level_asked(A, Some(Level::Extreme), 0);
        assert_eq!(sharing.levels(), vec![Level::Extreme]);
        assert!(
            sharing
                .chunks(0, no_backlog, |_| Some(&data[..]))
                .is_empty()
        );
        let lists = sharing.lists(0);
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].1.len(), 2);

        // Two changes in a row went through; the third waits, then the latest applies at 10 s.
        sharing.level_asked(A, Some(Level::VeryHigh), 0);
        assert_eq!(sharing.levels(), vec![Level::Extreme]);
        sharing.level_asked(A, Some(Level::High), 0);
        assert_eq!(sharing.levels(), vec![Level::Extreme]);
        sharing.lists(5_000);
        assert_eq!(sharing.levels(), vec![Level::Extreme], "not yet");
        let lists = sharing.lists(10_000);
        assert_eq!(sharing.levels(), vec![Level::High]);
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].1.len(), 1);
        // Asking for the level it has again clears a waiting one, and is answered with the list.
        sharing.level_asked(A, Some(Level::Extreme), 10_000);
        sharing.level_asked(A, Some(Level::High), 10_000);
        let lists = sharing.lists(60_000);
        assert_eq!(sharing.levels(), vec![Level::High]);
        assert_eq!(lists.len(), 1);
    }

    #[test]
    fn going_back_and_forth_does_not_fill_the_bytes_again() {
        let data = payload(1, 2 * MIB as usize);
        let list = vec![entry("a", 1, data.len() as u32)];
        let mut sharing = Sharing::default();
        sharing.publish(Level::High, list.clone());
        sharing.publish(Level::VeryHigh, list);
        sharing.level_asked(A, Some(Level::High), 0);
        sharing.lists(0);

        // Sends what it can for `seconds`, asking again whenever it's done.
        let mut now = 0u64;
        let mut send = |sharing: &mut Sharing, seconds: u64| {
            let mut sent = 0;
            for _ in 0..seconds * 10 {
                now += 100;
                sharing.wanted(A, 1);
                sent += sharing
                    .chunks(now, no_backlog, |_| Some(&data[..]))
                    .iter()
                    .map(|c| c.data.len() as u64)
                    .sum::<u64>();
            }
            (sent, now)
        };
        let (first, at) = send(&mut sharing, 30);
        assert!(first >= 16 * MIB - CHUNK as u64, "{first}");
        assert!(first <= 17 * MIB, "{first}: the burst and a little refill");

        // Up and down again: the bytes spent stay spent.
        sharing.level_asked(A, Some(Level::VeryHigh), at + 10_000);
        sharing.level_asked(A, Some(Level::High), at + 20_000);
        sharing.lists(at + 20_000);
        assert_eq!(sharing.levels(), vec![Level::High]);
        let mut now2 = at + 20_000;
        let mut second = 0;
        for _ in 0..100 {
            now2 += 100;
            sharing.wanted(A, 1);
            second += sharing
                .chunks(now2, no_backlog, |_| Some(&data[..]))
                .iter()
                .map(|c| c.data.len() as u64)
                .sum::<u64>();
        }
        assert!(second < 2 * MIB, "{second} bytes after changing levels");
    }

    #[test]
    fn a_member_gone_off_is_told_and_forgotten_members_are_dropped() {
        let mut sharing = Sharing::default();
        sharing.publish(Level::High, vec![entry("a", 1, 10)]);
        sharing.level_asked(A, Some(Level::High), 0);
        sharing.level_asked(B, None, 0);
        assert_eq!(sharing.lists(0).len(), 1, "B is off and was sent nothing");
        sharing.level_asked(A, None, 0);
        let lists = sharing.lists(0);
        assert_eq!(lists, vec![(A, Vec::new())]);
        assert!(sharing.lists(10_000).is_empty());
        assert!(sharing.levels().is_empty());

        sharing.level_asked(B, Some(Level::High), 10_000);
        sharing.lists(10_000);
        sharing.wanted(B, 1);
        sharing.keep_members(&[A]);
        assert!(sharing.levels().is_empty());
        let data = payload(1, 10);
        assert!(
            sharing
                .chunks(10_000, no_backlog, |_| Some(&data[..]))
                .is_empty()
        );
        assert_eq!(sharing.sent_to(B), 0);
    }

    // --- The member ---

    /// A receiving at `level` that holds `list`, with entry `n` of `bytes` each.
    fn receiving_with(level: Level, list: Vec<Entry>) -> Receiving {
        let mut receiving = Receiving::new(Some(level), 0);
        receiving.list(list);
        receiving
    }

    fn statuses(receiving: &Receiving) -> Vec<Status> {
        receiving.entries().into_iter().map(|(_, s)| s).collect()
    }

    #[test]
    fn totals_skip_in_list_order() {
        let mut list = vec![
            entry("a", 1, 100),
            entry("b", 2, 100),
            entry("c", 3, 100),
            entry("d", 4, 100),
            image("e", 5, 100),
            image("f", 6, 100),
        ];
        list[0].triangles = 200_000;
        list[1].triangles = 100_000; // over 250,000 with the first
        list[2].triangles = 50_000; // fits after them
        list[3].bytes = (17 * MIB) as u32; // more than a burst
        list[4].pixels = 10_000_000;
        list[5].pixels = 7_000_000; // over 16 MP with the first
        let receiving = receiving_with(Level::High, list);
        assert_eq!(
            statuses(&receiving),
            vec![
                Status::Waiting,
                Status::Skipped(Skip::Triangles),
                Status::Waiting,
                Status::Skipped(Skip::Bytes),
                Status::Waiting,
                Status::Skipped(Skip::Pixels),
            ]
        );

        // Too many references.
        let many: Vec<Entry> = (0..70).map(|n| entry(&format!("r{n}"), n, 10)).collect();
        let receiving = receiving_with(Level::High, many);
        let skipped = statuses(&receiving)
            .iter()
            .filter(|s| **s == Status::Skipped(Skip::Count))
            .count();
        assert_eq!(skipped, 6);

        // The same list under a bigger level: all taken.
        let mut receiving = receiving;
        receiving.set_level(Some(Level::Extreme), 0);
        assert!(statuses(&receiving).iter().all(|s| *s == Status::Waiting));

        // A reference with no bytes is broken from the start.
        let receiving = receiving_with(Level::High, vec![entry("a", 1, 0)]);
        assert_eq!(statuses(&receiving), vec![Status::Failed]);
        let mut receiving = receiving;
        assert_eq!(receiving.next_want(0), None);
    }

    /// Asks for the next content and gives all of it, in chunks of `chunk`.
    fn receive(receiving: &mut Receiving, content: u64, data: &[u8], now: u64) -> Option<Finished> {
        let mut finished = None;
        for (i, piece) in data.chunks(CHUNK).enumerate() {
            finished = receiving.data(content, (i * CHUNK) as u32, piece, now);
        }
        finished
    }

    #[test]
    fn one_reference_at_a_time_in_order() {
        let a = payload(1, 40_000);
        let b = payload(2, 100);
        let mut receiving = receiving_with(
            Level::High,
            vec![entry("a", 1, a.len() as u32), entry("b", 2, b.len() as u32)],
        );
        assert_eq!(receiving.next_want(0), Some(1));
        assert_eq!(receiving.next_want(0), None, "one at a time");
        assert_eq!(
            statuses(&receiving),
            vec![Status::Receiving { got: 0 }, Status::Waiting]
        );
        assert_eq!(receiving.data(1, 0, &a[..CHUNK], 5), None);
        assert_eq!(
            statuses(&receiving)[0],
            Status::Receiving { got: CHUNK as u32 }
        );
        let finished = receiving.data(1, CHUNK as u32, &a[CHUNK..2 * CHUNK], 6);
        assert!(finished.is_none());
        let finished = receiving
            .data(1, 2 * CHUNK as u32, &a[2 * CHUNK..], 7)
            .unwrap();
        assert_eq!(
            finished,
            Finished {
                content: 1,
                bytes: a.clone()
            }
        );
        assert_eq!(statuses(&receiving)[0], Status::Checking);
        // Checking isn't done: the next can be asked, but the first isn't shown.
        assert_eq!(receiving.next_want(8), Some(2));
        assert!(receiving.shown().is_empty());
        receiving.ready(1);
        assert_eq!(statuses(&receiving)[0], Status::Ready);
        assert_eq!(receiving.shown().len(), 1);
        assert_eq!(receiving.shown()[0].id, "a");
        let finished = receive(&mut receiving, 2, &b, 9).unwrap();
        assert_eq!(finished.bytes, b);
        receiving.ready(2);
        assert_eq!(receiving.next_want(10), None);
        assert_eq!(
            receiving
                .shown()
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert_eq!(receiving.ignored(), 0);
    }

    #[test]
    fn data_out_of_order_or_unasked_is_ignored() {
        let a = payload(1, 3 * CHUNK);
        let mut receiving = receiving_with(
            Level::High,
            vec![entry("a", 1, a.len() as u32), entry("b", 2, 50)],
        );
        // Nothing asked for yet.
        assert_eq!(receiving.data(1, 0, &a[..CHUNK], 0), None);
        assert_eq!(receiving.ignored(), 1);
        assert_eq!(receiving.next_want(0), Some(1));
        // Another content, a wrong offset, a repeat, empty, past the end.
        assert_eq!(receiving.data(2, 0, &[1; 50], 1), None);
        assert_eq!(
            receiving.data(1, CHUNK as u32, &a[CHUNK..2 * CHUNK], 1),
            None
        );
        assert_eq!(receiving.data(1, 0, &[], 1), None);
        assert_eq!(receiving.data(1, 0, &a[..CHUNK], 1), None);
        assert_eq!(receiving.data(1, 0, &a[..CHUNK], 1), None, "not twice");
        assert_eq!(receiving.data(1, 1, &a[1..CHUNK], 1), None);
        assert_eq!(receiving.ignored(), 1 + 6 - 1);
        let ignored = receiving.ignored();
        assert_eq!(
            statuses(&receiving)[0],
            Status::Receiving { got: CHUNK as u32 }
        );
        // The rest still comes in and is exactly the bytes.
        assert_eq!(
            receiving.data(1, CHUNK as u32, &a[CHUNK..2 * CHUNK], 2),
            None
        );
        let finished = receiving
            .data(1, 2 * CHUNK as u32, &a[2 * CHUNK..], 2)
            .unwrap();
        assert_eq!(finished.bytes, a);
        // After it finished, more of it is ignored.
        assert_eq!(receiving.data(1, 3 * CHUNK as u32, &[1], 3), None);
        assert_eq!(receiving.ignored(), ignored + 1);
    }

    #[test]
    fn data_past_the_declared_size_is_ignored() {
        let mut receiving = receiving_with(Level::High, vec![entry("a", 1, 100)]);
        assert_eq!(receiving.next_want(0), Some(1));
        assert_eq!(receiving.data(1, 0, &[7; 101], 0), None);
        assert_eq!(receiving.data(1, 0, &[7; 60], 0), None);
        assert_eq!(
            receiving.data(1, 60, &[7; 41], 0),
            None,
            "one byte too many"
        );
        assert_eq!(receiving.ignored(), 2);
        let finished = receiving.data(1, 60, &[7; 40], 0).unwrap();
        assert_eq!(finished.bytes.len(), 100);
    }

    #[test]
    fn a_silent_download_is_dropped_and_asked_again() {
        let mut receiving = receiving_with(Level::High, vec![entry("a", 1, 1000)]);
        assert_eq!(receiving.next_want(1_000), Some(1));
        assert_eq!(receiving.data(1, 0, &[1; 500], 50_000), None);
        assert_eq!(receiving.next_want(50_000 + DATA_TIMEOUT - 1), None);
        // No data for 120 s: dropped and asked for again from its start.
        assert_eq!(receiving.next_want(50_000 + DATA_TIMEOUT), Some(1));
        assert_eq!(statuses(&receiving), vec![Status::Receiving { got: 0 }]);
        assert_eq!(
            receiving.data(1, 500, &[1; 500], 200_000),
            None,
            "old stream"
        );
        assert_eq!(
            receiving
                .data(1, 0, &[1; 1000], 200_000)
                .unwrap()
                .bytes
                .len(),
            1000
        );
    }

    #[test]
    fn the_bytes_allowance_pauses_asking() {
        // Each takes more than half of the burst: only one fits at a time.
        let size = (10 * MIB) as u32;
        let mut receiving = receiving_with(
            Level::High,
            vec![
                entry("a", 1, size),
                entry("b", 2, size),
                entry("c", 3, size),
            ],
        );
        assert_eq!(receiving.next_want(0), Some(1));
        assert_eq!(
            receiving
                .data(1, 0, &vec![0; size as usize], 0)
                .unwrap()
                .content,
            1
        );
        receiving.ready(1);
        // 6 MiB are left; 10 come back in 10 minutes times (10 - 6) / 16.
        assert_eq!(receiving.next_want(1_000), None);
        // 4 MiB more take a quarter of the refill time: 150 seconds.
        assert_eq!(receiving.next_want(140_000), None);
        assert_eq!(receiving.next_want(151_000), Some(2));
        let wait = 151_000;
        // Nothing is asked for out of order to use what is left.
        assert_eq!(receiving.next_want(wait), None);
        assert_eq!(statuses(&receiving)[2], Status::Waiting);
    }

    #[test]
    fn dropping_a_download_gives_the_unused_bytes_back() {
        let size = (12 * MIB) as u32;
        let mut receiving =
            receiving_with(Level::High, vec![entry("a", 1, size), entry("b", 2, size)]);
        assert_eq!(receiving.next_want(0), Some(1));
        assert_eq!(receiving.next_want(0), None);
        // The host's list changed and no longer has it: the 12 MiB are back.
        receiving.list(vec![entry("b", 2, size)]);
        assert_eq!(receiving.next_want(0), Some(2));
    }

    #[test]
    fn a_new_list_keeps_what_is_ready_and_drops_what_is_gone() {
        let a = payload(1, 100);
        let mut receiving = receiving_with(
            Level::High,
            vec![entry("a", 1, 100), entry("b", 2, 100), entry("c", 3, 100)],
        );
        assert_eq!(receiving.next_want(0), Some(1));
        receive(&mut receiving, 1, &a, 0).unwrap();
        receiving.ready(1);
        assert_eq!(receiving.next_want(0), Some(2));
        receiving.data(2, 0, &[0; 50], 0);

        // 2 is gone from the list while it was coming; 1 moves to another id.
        receiving.list(vec![entry("a2", 1, 100), entry("c", 3, 100)]);
        assert_eq!(statuses(&receiving), vec![Status::Ready, Status::Waiting]);
        assert_eq!(receiving.shown()[0].id, "a2");
        assert_eq!(
            receiving.data(2, 50, &[0; 50], 0),
            None,
            "the download is gone"
        );
        assert_eq!(receiving.next_want(0), Some(3));
        // A list that names what is being received keeps it.
        receiving.list(vec![entry("c", 3, 100), entry("z", 9, 100)]);
        assert_eq!(statuses(&receiving)[0], Status::Receiving { got: 0 });
        // A list that says it is a different size drops it.
        receiving.list(vec![entry("c", 3, 200)]);
        assert_eq!(statuses(&receiving)[0], Status::Waiting);
    }

    #[test]
    fn ready_content_over_the_caps_is_not_shown() {
        let mut receiving = receiving_with(Level::High, vec![entry("a", 1, 100)]);
        receiving.known(1);
        assert_eq!(receiving.shown().len(), 1);
        let mut big = entry("a", 1, 100);
        big.triangles = 300_000;
        receiving.list(vec![big.clone()]);
        assert_eq!(statuses(&receiving), vec![Status::Skipped(Skip::Triangles)]);
        assert!(receiving.shown().is_empty());
        // Room again: ready, with no download.
        receiving.set_level(Some(Level::VeryHigh), 0);
        assert_eq!(receiving.shown().len(), 1);
    }

    #[test]
    fn off_shows_nothing_and_going_back_on_keeps_what_was_received() {
        let a = payload(1, 100);
        let mut receiving =
            receiving_with(Level::High, vec![entry("a", 1, 100), entry("b", 2, 100)]);
        assert_eq!(receiving.next_want(0), Some(1));
        receive(&mut receiving, 1, &a, 0).unwrap();
        receiving.ready(1);
        assert_eq!(receiving.next_want(0), Some(2));

        receiving.set_level(None, 10);
        assert_eq!(receiving.level(), None);
        assert!(receiving.shown().is_empty());
        assert!(receiving.entries().is_empty());
        assert_eq!(receiving.next_want(10), None);
        assert_eq!(
            receiving.data(2, 0, &[1; 100], 10),
            None,
            "its download ended"
        );
        // The host's list for off is empty.
        receiving.list(Vec::new());
        assert!(receiving.entries().is_empty());
        receiving.list(vec![entry("a", 1, 100), entry("b", 2, 100)]);

        receiving.set_level(Some(Level::High), 20);
        assert_eq!(receiving.level(), Some(Level::High));
        assert_eq!(receiving.shown().len(), 1, "1 needs no download");
        assert_eq!(receiving.shown()[0].id, "a");
        // It waits for the host's list for this level before asking for the rest.
        assert_eq!(receiving.next_want(20), None);
        receiving.list(vec![entry("a", 1, 100), entry("b", 2, 100)]);
        assert_eq!(receiving.next_want(20), Some(2));
    }

    #[test]
    fn a_list_kept_while_off_is_there_when_back_on() {
        let mut receiving = Receiving::new(None, 0);
        receiving.list(vec![entry("a", 1, 100)]);
        assert!(receiving.entries().is_empty());
        assert_eq!(receiving.next_want(0), None);
        receiving.set_level(Some(Level::High), 0);
        assert_eq!(statuses(&receiving), vec![Status::Waiting]);
    }

    #[test]
    fn a_failed_content_is_not_asked_for_again() {
        let mut receiving = receiving_with(Level::High, vec![entry("a", 1, 10), entry("b", 2, 10)]);
        assert_eq!(receiving.next_want(0), Some(1));
        receiving.data(1, 0, &[0; 10], 0).unwrap();
        receiving.failed(1);
        assert_eq!(statuses(&receiving)[0], Status::Failed);
        assert_eq!(receiving.next_want(0), Some(2));
        receiving.data(2, 0, &[0; 10], 0).unwrap();
        receiving.ready(2);
        assert_eq!(receiving.next_want(1_000_000), None);
        // The list still names it (under another id): still failed.
        receiving.list(vec![entry("a2", 1, 10), entry("b", 2, 10)]);
        assert_eq!(statuses(&receiving)[0], Status::Failed);
        assert_eq!(receiving.next_want(1_000_000), None);
        // Once the list doesn't, it is forgotten, and a list that has it again asks anew.
        receiving.list(vec![entry("b", 2, 10)]);
        receiving.list(vec![entry("a", 1, 10), entry("b", 2, 10)]);
        assert_eq!(receiving.next_want(1_000_000), Some(1));
    }

    #[test]
    fn forgetting_a_content_asks_for_it_again() {
        let mut receiving = receiving_with(Level::High, vec![entry("a", 1, 10)]);
        receiving.known(1);
        assert_eq!(receiving.next_want(0), None);
        receiving.forget(1);
        assert_eq!(statuses(&receiving), vec![Status::Waiting]);
        assert_eq!(receiving.next_want(0), Some(1));
        receiving.forget(1);
        assert_eq!(
            receiving.data(1, 0, &[0; 10], 0),
            None,
            "its download ended"
        );
        // A late answer from the worker for something it forgot does nothing.
        receiving.ready(1);
        assert_eq!(statuses(&receiving), vec![Status::Waiting]);
    }

    #[test]
    fn a_level_change_waits_for_the_hosts_list() {
        let mut receiving = receiving_with(Level::High, vec![entry("a", 1, 10)]);
        assert_eq!(receiving.next_want(0), Some(1));
        receiving.set_level(Some(Level::Extreme), 5);
        assert_eq!(receiving.data(1, 0, &[0; 10], 5), None, "dropped");
        assert_eq!(receiving.next_want(5), None);
        receiving.list(vec![entry("a", 1, 10)]);
        assert_eq!(receiving.next_want(5), Some(1));
    }

    #[test]
    fn hosts_and_members_agree_on_the_allowance() {
        // The same numbers on both sides: what a member could ask for in a row is what the host
        // would send.
        let size = (4 * MIB) as u32;
        let list: Vec<Entry> = (0..8).map(|n| entry(&format!("r{n}"), n, size)).collect();
        let mut receiving = receiving_with(Level::High, list.clone());
        let mut sharing = sharing_with(&[A], Level::High, list);
        let data = vec![3u8; size as usize];
        let mut now = 0u64;
        let mut asked = 0;
        let mut delivered = 0usize;
        for _ in 0..600 {
            now += 100;
            if let Some(content) = receiving.next_want(now) {
                asked += 1;
                sharing.wanted(A, content);
            }
            for chunk in sharing.chunks(now, no_backlog, |_| Some(&data[..])) {
                delivered += chunk.data.len();
                if let Some(done) = receiving.data(chunk.content, chunk.offset, &chunk.data, now) {
                    receiving.ready(done.content);
                }
            }
        }
        assert_eq!(
            asked, 4,
            "16 MiB are four of these, the fifth waits for the refill"
        );
        assert_eq!(delivered, 4 * size as usize);
        assert_eq!(receiving.ignored(), 0);
    }
}
