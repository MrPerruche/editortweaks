//! Fuzzing the receive path. Peers in a session are untrusted, so whatever bytes arrive from the
//! network must never panic, hang, or make us allocate according to a length field the sender
//! chose. The tests here feed the decoder, the part assembler and live sessions with:
//!
//! - completely random bytes, and random bytes behind a valid header (so they get past the magic
//!   and the protocol check), with plausible and huge counts and lengths,
//! - every kind of message, mutated: cut short, bytes and bits flipped, bytes inserted and removed,
//!   length fields set to huge values, ranges repeated,
//! - messages split into parts that are then repeated, dropped, shuffled or given wrong counts.
//!
//! Sessions get these frames from strangers, from a member (to the subhost) and from the subhost
//! (to a member), mixed with honest traffic, and must keep working afterwards.
//!
//! Everything is deterministic from the seed. Environment variables:
//!
//! - `COOP_FUZZ_SEEDS`: how many seeds each test runs (a small default; try thousands with
//!   `--release`),
//! - `COOP_FUZZ_FIRST`: the first seed, to repeat a failure with `COOP_FUZZ_SEEDS=1`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};

use coop::framing::Assembler;
use coop::message::{HEADER_LEN, MAGIC, PROTOCOL, caps};
use coop::references::{Entry, ImageLook, Kind};
use coop::vehicle::Vehicle;
use coop::wire::Reader;
use coop::{
    Audit, Check, CoopId, Digest, Event, GroupKind, HostSettings, Member, Message, Op, Outgoing,
    PeerId, PeerInfo, RefuseReason, Session, Slot, Transform, Value,
};

// --- Watching allocations ---

thread_local! {
    /// The biggest single allocation this thread asked for since the last reset.
    static LARGEST: Cell<usize> = const { Cell::new(0) };
}

fn note_allocation(size: usize) {
    // A `const` thread local without a destructor can't fail, even during thread start-up.
    let _ = LARGEST.try_with(|largest| largest.set(largest.get().max(size)));
}

/// The system allocator, remembering the biggest request per thread.
struct Counting;

// SAFETY: every method forwards to `System` with the arguments it was given, so `System`'s
// guarantees carry over; the only extra work is a write to a thread local, which never allocates.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_allocation(layout.size());
        // SAFETY: same layout, same caller contract.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_allocation(layout.size());
        // SAFETY: same layout, same caller contract.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` and `layout` come from an earlier call that went to `System`.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_allocation(new_size);
        // SAFETY: `ptr` and `layout` come from an earlier call that went to `System`.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Runs `f` and returns its result with the biggest single allocation it asked for.
fn largest_allocation<R>(f: impl FnOnce() -> R) -> (R, usize) {
    LARGEST.with(|largest| largest.set(0));
    let result = f();
    (result, LARGEST.with(Cell::get))
}

/// Decoding may allocate up to this much, plus [`ALLOC_PER_BYTE`] for every byte of input. Elements
/// are bigger in memory than on the wire (an op is a few dozen bytes, at least 5 on the wire), so
/// the allowance per byte is generous; a buffer sized from a length field the input doesn't back
/// up (a 65,535 id list, a 16 MiB snapshot) is far over it for the short inputs used here.
const ALLOC_SLACK: usize = 32 << 10;
const ALLOC_PER_BYTE: usize = 128;

/// For whole sessions, which keep state across frames (parts being assembled): nothing a few
/// thousand bytes of input can justify.
const SESSION_ALLOC_LIMIT: usize = 4 << 20;

// --- Watchdog: a hang prints where it is stuck, instead of the test run just never ending ---

static PROGRESS: AtomicU64 = AtomicU64::new(0);
/// Per test: the seed it is on and the input it is processing.
type Cases = HashMap<String, (u64, Vec<u8>)>;

static CURRENT: Mutex<Option<Cases>> = Mutex::new(None);
static WATCHDOG: Once = Once::new();

/// How long nothing may happen in any test before the run is declared hung.
const HANG_AFTER: Duration = Duration::from_secs(60);

fn start_watchdog() {
    WATCHDOG.call_once(|| {
        std::thread::spawn(|| {
            let (mut seen, mut since) = (u64::MAX, Instant::now());
            loop {
                std::thread::sleep(Duration::from_millis(500));
                let now = PROGRESS.load(Ordering::Relaxed);
                if now != seen {
                    (seen, since) = (now, Instant::now());
                } else if since.elapsed() > HANG_AFTER {
                    eprintln!("HANG: no progress for {HANG_AFTER:?}. Last inputs per test:");
                    let current = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
                    for (test, (seed, input)) in current.iter().flatten() {
                        eprintln!("  {test}: seed {seed}, input {}", hex(input));
                    }
                    std::process::exit(101);
                }
            }
        });
    });
}

/// Tells the watchdog `test` is alive and about to process `input`.
fn note_case(test: &str, seed: u64, input: &[u8]) {
    PROGRESS.fetch_add(1, Ordering::Relaxed);
    let mut current = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    current
        .get_or_insert_with(HashMap::new)
        .insert(test.to_string(), (seed, input.to_vec()));
}

// --- Randomness (no dependency, repeatable; the same xorshift as sim.rs) ---

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    fn byte(&mut self) -> u8 {
        (self.next() >> 24) as u8
    }

    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.byte()).collect()
    }

    /// 0 to `n - 1` random bytes.
    fn bytes_below(&mut self, n: usize) -> Vec<u8> {
        let len = self.below(n);
        self.bytes(len)
    }

    /// 1 to `n` random bytes.
    fn bytes_below_at_least_one(&mut self, n: usize) -> Vec<u8> {
        let len = 1 + self.below(n);
        self.bytes(len)
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// A number that is likely to sit on an edge: zero, small, at a cap or just over it, huge.
    fn interesting(&mut self) -> u32 {
        const EDGES: [u32; 32] = [
            0,
            1,
            2,
            3,
            4,
            5,
            32,
            33,
            128,
            129,
            255,
            256,
            257,
            4096,
            4097,
            10_000,
            10_001,
            16_384,
            16_385,
            65_535,
            65_536,
            0x00FF_FFFF,
            0x0100_0000,
            (16 << 20) - 1,
            16 << 20,
            (16 << 20) + 1,
            24 << 20,
            500_000,
            0x7FFF_FFFF,
            0x8000_0000,
            0xFFFF_FFFE,
            0xFFFF_FFFF,
        ];
        *self.pick(&EDGES)
    }
}

fn seeds(default: u64) -> std::ops::Range<u64> {
    let read = |name: &str| std::env::var(name).ok().and_then(|v| v.parse::<u64>().ok());
    let first = read("COOP_FUZZ_FIRST").unwrap_or(0);
    first..first + read("COOP_FUZZ_SEEDS").unwrap_or(default)
}

// --- Reporting ---

fn hex(bytes: &[u8]) -> String {
    const SHOWN: usize = 1000;
    let mut text: String = bytes
        .iter()
        .take(SHOWN)
        .map(|b| format!("{b:02x}"))
        .collect();
    if bytes.len() > SHOWN {
        text.push_str(&format!("... ({} bytes in all)", bytes.len()));
    }
    if text.is_empty() {
        text.push_str("(empty)");
    }
    text
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(not a text)".into())
}

/// Removes pieces of `input` for as long as it still fails, to report a small example.
fn shrink(mut input: Vec<u8>, fails: impl Fn(&[u8]) -> bool) -> Vec<u8> {
    let mut chunk = input.len().div_ceil(2).max(1);
    loop {
        let mut start = 0;
        let mut progress = false;
        while start < input.len() {
            let end = (start + chunk).min(input.len());
            let mut candidate = input.clone();
            candidate.drain(start..end);
            if fails(&candidate) {
                input = candidate;
                progress = true;
            } else {
                start += chunk;
            }
        }
        if chunk > 1 {
            chunk = chunk.div_ceil(2);
        } else if !progress {
            return input;
        }
    }
}

// --- Messages to build frames from ---

fn id(slot: Slot, n: u32) -> CoopId {
    CoopId::new(slot, n).unwrap()
}

fn peer_info(hosting: bool) -> PeerInfo {
    PeerInfo {
        protocol: (1, PROTOCOL),
        mod_version: "0.1.0".into(),
        game_version: GAME_VERSION.into(),
        hosting,
    }
}

/// One of every op and value kind.
fn sample_ops() -> Vec<Op> {
    let transform = Transform {
        location: [1.0, -2.5, 3e5],
        rotation: [0.5, 90.0, -180.0],
    };
    let property = |name: &str, value| Op::Set {
        id: id(0, 2),
        property: name.into(),
        value,
    };
    vec![
        Op::Create {
            ids: vec![id(2, 1), id(2, 2), id(2, 3)],
            archive: (0..200u8).collect(),
        },
        Op::Delete { id: id(0, 1) },
        Op::Move {
            id: id(0, 3),
            transform,
        },
        property("Color", Value::Raw(vec![1, 2, 3, 4])),
        property("Label", Value::Text("hello".into())),
        property("Target", Value::Bricks(vec![id(0, 1), id(2, 1)])),
        Op::Group {
            id: id(0, 2),
            kind: GroupKind::Weld,
            group: 3,
        },
        Op::Order {
            ids: vec![id(0, 3), id(0, 2), id(0, 1)],
        },
    ]
}

/// Every kind of message (the test below checks that).
fn sample_messages() -> Vec<Message> {
    let vehicle = Vehicle {
        archive: (0..3000u32).map(|n| (n * 7 % 251) as u8).collect(),
        groups: vec![[0, 1], [2, 0], [0, 0]],
    };
    let ops = sample_ops();
    vec![
        Message::Hello(peer_info(true)),
        Message::HelloReply(peer_info(false)),
        Message::Join {
            code: CODE.into(),
            name: "Perru".into(),
            game_version: GAME_VERSION.into(),
        },
        Message::JoinAccepted { slot: 3 },
        Message::JoinRefused(RefuseReason::Full),
        Message::Members(vec![
            Member {
                slot: 1,
                peer: 76561198000000000,
                name: "host".into(),
            },
            Member {
                slot: 2,
                peer: 76561198000000001,
                name: "guest".into(),
            },
        ]),
        Message::Snapshot {
            commit: 9,
            ids: vec![id(0, 1), id(0, 2), id(0, 3)],
            vehicle: vehicle.encode(),
        },
        Message::Propose {
            local: 4,
            ops: ops.clone(),
        },
        Message::Commit {
            number: 10,
            author: 2,
            local: 4,
            ops,
            check: Check {
                count: 3,
                hash: 0xDEAD_BEEF,
            },
        },
        Message::Resync,
        Message::Selection {
            slot: 2,
            ids: vec![id(0, 1), id(2, 1)],
        },
        Message::Camera {
            slot: 2,
            view: Some(coop::presence::View {
                location: [-40.0, 0.0, 120.0],
                forward: [1.0, 0.0, 0.0],
            }),
        },
        Message::Leave,
        Message::Kick,
        Message::Close,
        Message::Ping,
        Message::Part {
            index: 1,
            count: 3,
            data: vec![1, 2, 3],
        },
        Message::Audit(Audit {
            commit: 12,
            count: 40,
            order: 0xABCD,
            bricks: (1..=5)
                .map(|n| {
                    (
                        id(0, n),
                        Digest {
                            place: n,
                            values: 2 * n,
                            groups: 3 * n,
                        },
                    )
                })
                .collect(),
        }),
        Message::OrderWanted,
        Message::Order {
            commit: 12,
            ids: vec![id(0, 3), id(0, 2), id(0, 1)],
        },
        Message::ReferenceLevel { level: 2 },
        Message::ReferenceList {
            entries: sample_references(),
        },
        Message::ReferenceWant { content: 0xFEED },
        Message::ReferenceData {
            content: 0xFEED,
            offset: 16384,
            data: (0..300u32).map(|n| (n * 13) as u8).collect(),
        },
    ]
}

/// A model and an image, with every field set to something.
fn sample_references() -> Vec<Entry> {
    let model = Entry {
        id: "model_1".into(),
        name: "A reference".into(),
        kind: Kind::Model,
        content: 0xFEED_FACE,
        bytes: 123_456,
        triangles: 4000,
        pixels: 1024,
        simplified: true,
        position: [10.5, -20.0, 300.0],
        rotation: [0.0, 45.0, 90.0],
        scale: [1.0, 2.0, 0.5],
        mirror: [true, false, true],
        opacity: 0.5,
        color: [1.0, 0.5, 0.25],
        image: None,
    };
    let image = Entry {
        id: "image-2".into(),
        name: "A photo".into(),
        kind: Kind::Image,
        content: 77,
        bytes: 9000,
        triangles: 0,
        pixels: 640 * 480,
        simplified: false,
        image: Some(ImageLook {
            width: 300.0,
            flip_x: false,
            flip_y: true,
            double_sided: true,
            pixelated: false,
            pivot: 5,
        }),
        ..model.clone()
    };
    vec![model, image]
}

/// The header bytes every message starts with.
fn header(protocol: u16, kind: u8, session: u32) -> Vec<u8> {
    let mut bytes = MAGIC.to_vec();
    bytes.extend(protocol.to_le_bytes());
    bytes.push(kind);
    bytes.extend(session.to_le_bytes());
    bytes
}

// --- Making bad frames ---

/// Random bytes: from pure noise to a valid header followed by numbers picked to be nasty.
fn random_frame(rng: &mut Rng, session: u32) -> Vec<u8> {
    match rng.below(7) {
        0 => rng.bytes_below(2000),
        1 => rng.bytes_below(HEADER_LEN + 3),
        2 => {
            let mut bytes = header(PROTOCOL, rng.byte() % 26, session);
            bytes.extend(rng.bytes_below(1500));
            bytes
        }
        3 => {
            let items = 1 + rng.below(30);
            structured_frame(rng, session, items)
        }
        4 => {
            // A kind that has counts first, and a count that promises far more than follows.
            let mut bytes = header(
                PROTOCOL,
                *rng.pick(&[5, 6, 7, 8, 10, 16, 18, 22, 24]),
                session,
            );
            for _ in 0..rng.below(3) {
                bytes.extend(rng.interesting().to_le_bytes());
            }
            bytes.extend(rng.interesting().to_le_bytes());
            bytes.extend(rng.bytes_below(40));
            bytes
        }
        5 => {
            // Another protocol version, or a hello that has to keep reading across versions.
            let protocol = if rng.chance(50) {
                PROTOCOL + 1 + rng.below(3) as u16
            } else {
                rng.next() as u16
            };
            let mut bytes = header(protocol, rng.byte() % 3, session);
            bytes.extend(structured_tail(rng, 8));
            bytes
        }
        _ => b"HTTP/1.1 200 OK\r\n\r\n".repeat(1 + rng.below(3)),
    }
}

/// A valid header, then `items` numbers and bytes, many of them at a cap or well over it.
fn structured_frame(rng: &mut Rng, session: u32, items: usize) -> Vec<u8> {
    let mut bytes = header(PROTOCOL, rng.byte() % 26, session);
    bytes.extend(structured_tail(rng, items));
    bytes
}

fn structured_tail(rng: &mut Rng, items: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    for _ in 0..items {
        match rng.below(4) {
            0 | 1 => bytes.extend(rng.interesting().to_le_bytes()),
            2 => bytes.push(rng.byte() % 8),
            _ => bytes.extend(rng.bytes_below_at_least_one(6)),
        }
    }
    bytes
}

/// A copy of `bytes` with one to three things done to it.
fn mutate(rng: &mut Rng, bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    for _ in 0..1 + rng.below(3) {
        // Counts and lengths sit near the start of most messages, so aim there half the time.
        let pos = |rng: &mut Rng, len: usize| {
            if len == 0 {
                0
            } else if rng.chance(50) {
                rng.below(len.min(48))
            } else {
                rng.below(len)
            }
        };
        let at = pos(rng, out.len());
        match rng.below(11) {
            0 => out.truncate(at),
            1 => {
                if let Some(byte) = out.get_mut(at) {
                    *byte ^= rng.byte() | 1;
                }
            }
            2 => {
                if let Some(byte) = out.get_mut(at) {
                    *byte ^= 1 << rng.below(8);
                }
            }
            3 => {
                let extra = rng.bytes_below_at_least_one(8);
                out.splice(at..at, extra);
            }
            4 => {
                let end = (at + 1 + rng.below(8)).min(out.len());
                out.drain(at..end);
            }
            5 | 6 => {
                // A length field set to an edge value (whichever 4 bytes are at `at`).
                if at + 4 <= out.len() {
                    out[at..at + 4].copy_from_slice(&rng.interesting().to_le_bytes());
                }
            }
            7 => {
                if at + 2 <= out.len() {
                    out[at..at + 2].copy_from_slice(&(rng.interesting() as u16).to_le_bytes());
                }
            }
            8 => {
                // A range repeated: a list that has more items than it says, or a repeated field.
                let end = (at + 1 + rng.below(32)).min(out.len());
                let copy = out[at..end].to_vec();
                out.splice(end..end, copy);
            }
            9 => out.extend(rng.bytes_below_at_least_one(64)),
            _ => {
                if let Some(byte) = out.get_mut(at) {
                    *byte = *rng.pick(&[0x00, 0x01, 0x7F, 0x80, 0xFF]);
                }
            }
        }
    }
    out
}

/// `whole` split into `k` parts, as `Message::Part` frames sent in order.
fn split_in_parts(whole: &[u8], k: usize) -> Vec<(u32, u32, Vec<u8>)> {
    let size = whole.len().div_ceil(k).max(1);
    let chunks: Vec<&[u8]> = whole.chunks(size).collect();
    let count = chunks.len() as u32;
    chunks
        .into_iter()
        .enumerate()
        .map(|(index, data)| (index as u32, count, data.to_vec()))
        .collect()
}

/// Part frames for `whole`, with some of: shuffled, repeated, dropped, wrong count or index, random
/// data.
fn broken_parts(rng: &mut Rng, whole: &[u8], session: u32) -> Vec<Vec<u8>> {
    let mut parts = split_in_parts(whole, 2 + rng.below(5));
    for _ in 0..rng.below(4) {
        if parts.is_empty() {
            break;
        }
        let at = rng.below(parts.len());
        match rng.below(8) {
            0 => {
                let other = rng.below(parts.len());
                parts.swap(at, other);
            }
            1 => parts.insert(rng.below(parts.len() + 1), parts[at].clone()),
            2 => {
                parts.remove(at);
            }
            3 => parts[at].1 = rng.interesting(),
            4 => parts[at].0 = rng.interesting(),
            5 => parts[at].2 = rng.bytes_below(64),
            6 => parts.extend(parts.clone()),
            _ => parts.truncate(at),
        }
    }
    let mut frames: Vec<Vec<u8>> = parts
        .into_iter()
        .map(|(index, count, data)| Message::Part { index, count, data }.encode(session))
        .collect();
    if rng.chance(30) {
        for frame in &mut frames {
            if rng.chance(30) {
                *frame = mutate(rng, frame);
            }
        }
    }
    frames
}

/// What a hostile peer might send in one go: usually one frame, sometimes a run of part frames.
/// `messages` are the valid messages to start from.
fn hostile_frames(rng: &mut Rng, messages: &[Message], session: u32) -> Vec<Vec<u8>> {
    let valid = |rng: &mut Rng| rng.pick(messages).encode(session);
    match rng.below(12) {
        0 | 1 => vec![random_frame(rng, session)],
        2..=6 => {
            let frame = valid(rng);
            vec![mutate(rng, &frame)]
        }
        7 => vec![valid(rng)],
        8 | 9 => {
            // A message cut into parts, sometimes mutated before the cutting.
            let mut whole = valid(rng);
            if rng.chance(50) {
                whole = mutate(rng, &whole);
            }
            broken_parts(rng, &whole, session)
        }
        10 => {
            // Parts that assemble to noise.
            let whole = random_frame(rng, session);
            broken_parts(rng, &whole, session)
        }
        _ => {
            // Nothing but part frames with arbitrary numbers.
            (0..1 + rng.below(4))
                .map(|_| {
                    Message::Part {
                        index: rng.interesting(),
                        count: rng.interesting(),
                        data: rng.bytes_below(100),
                    }
                    .encode(session)
                })
                .collect()
        }
    }
}

// --- The decoder ---

/// Everything wrong with decoding `bytes`, if anything: a panic, a slow decode, an allocation
/// that isn't backed by input, or a decoded message that doesn't survive being encoded again.
fn decode_problem(bytes: &[u8]) -> Option<String> {
    let started = Instant::now();
    let (result, largest) = largest_allocation(|| catch_unwind(|| Message::decode(bytes)));
    let decoded = match result {
        Ok(decoded) => decoded,
        Err(payload) => return Some(format!("panicked: {}", panic_text(&*payload))),
    };
    if started.elapsed() > Duration::from_secs(2) {
        return Some(format!("took {:?}", started.elapsed()));
    }
    let limit = ALLOC_SLACK + ALLOC_PER_BYTE * bytes.len();
    if largest > limit {
        return Some(format!(
            "allocated {largest} bytes at once for {} bytes of input",
            bytes.len()
        ));
    }
    if let Ok((session, message)) = decoded {
        let again = catch_unwind(|| Message::decode(&message.encode(session)));
        match again {
            Ok(Ok(same)) if same == (session, message) => {}
            other => {
                return Some(format!(
                    "decoded, but did not survive re-encoding: {other:?}"
                ));
            }
        }
    }
    None
}

fn check_decoding(test: &str, seed: u64, bytes: &[u8]) {
    note_case(test, seed, bytes);
    if let Some(problem) = decode_problem(bytes) {
        let small = shrink(bytes.to_vec(), |candidate| {
            decode_problem(candidate).is_some()
        });
        panic!(
            "seed {seed}: {problem}\n  input ({} bytes): {}\n  smallest failing input: {}",
            bytes.len(),
            hex(bytes),
            hex(&small),
        );
    }
}

/// The tools of this file must see what they are meant to see, or the fuzzing proves nothing.
#[test]
fn the_watching_tools_work() {
    let ((), largest) = largest_allocation(|| {
        let big = vec![1u8; 3 << 20];
        std::hint::black_box(&big);
    });
    assert!(largest >= 3 << 20, "saw only {largest}");

    // Shrinking keeps what makes an input fail and drops the rest.
    let small = shrink((0..200u8).collect(), |bytes| {
        bytes.contains(&17) && bytes.contains(&99)
    });
    assert_eq!(small, vec![17, 99]);

    assert!(decode_problem(&[]).is_none());
    assert!(decode_problem(&Message::Ping.encode(1)).is_none());
}

#[test]
fn samples_cover_every_message_kind() {
    let mut kinds: Vec<u8> = sample_messages()
        .iter()
        .map(|m| m.encode(1)[HEADER_LEN - 5])
        .collect();
    kinds.sort_unstable();
    assert_eq!(kinds, (0..=19).chain(21..=24).collect::<Vec<u8>>());
    for message in sample_messages() {
        assert_eq!(
            Message::decode(&message.encode(7)),
            Ok((7, message)),
            "a sample doesn't round-trip, so mutating it proves little"
        );
    }
}

#[test]
fn decoder_survives_random_and_mutated_bytes() {
    start_watchdog();
    let messages = sample_messages();
    let (mut total, mut decoded) = (0usize, 0usize);
    for seed in seeds(400) {
        let mut rng = Rng::new(seed);
        for _ in 0..200 {
            for frame in hostile_frames(&mut rng, &messages, 0x1234_5678) {
                check_decoding("decoder", seed, &frame);
                total += 1;
                decoded += Message::decode(&frame).is_ok() as usize;
            }
        }
    }
    eprintln!("decoder: {total} inputs, {decoded} of them still decoded");
    // Otherwise the mutations wouldn't reach the parts behind the header.
    assert!(decoded * 20 > total, "only {decoded} of {total} decode");
}

/// Every sample with every four bytes (at every offset) replaced by every edge number: the
/// length and count fields among them. A decoder that reserves memory for what a field says,
/// rather than for what is there, shows up here.
#[test]
fn decoder_never_trusts_a_length_field() {
    start_watchdog();
    let edges: Vec<u32> = {
        let mut rng = Rng::new(0);
        let mut all: Vec<u32> = (0..200).map(|_| rng.interesting()).collect();
        all.sort_unstable();
        all.dedup();
        all
    };
    let mut cases = 0;
    for message in sample_messages() {
        let bytes = message.encode(0x1234_5678);
        for at in 0..bytes.len().saturating_sub(3) {
            for &edge in &edges {
                let mut changed = bytes.clone();
                changed[at..at + 4].copy_from_slice(&edge.to_le_bytes());
                check_decoding("length fields", at as u64, &changed);
                cases += 1;
            }
        }
    }
    eprintln!("length fields: {cases} inputs");
}

#[test]
fn decoder_survives_every_cut_and_flip() {
    start_watchdog();
    for message in sample_messages() {
        let bytes = message.encode(3);
        for len in 0..bytes.len() {
            check_decoding("cuts", len as u64, &bytes[..len]);
        }
        for at in 0..bytes.len() {
            for flip in [0x01, 0x80, 0xFF] {
                let mut changed = bytes.clone();
                changed[at] ^= flip;
                check_decoding("flips", at as u64, &changed);
            }
        }
        // One byte more, or removed from the middle.
        for at in 0..bytes.len() {
            let mut removed = bytes.clone();
            removed.remove(at);
            check_decoding("removals", at as u64, &removed);
            let mut added = bytes.clone();
            added.insert(at, 0xFF);
            check_decoding("insertions", at as u64, &added);
        }
    }
}

/// Reference lists that survive the mutations are what the receiver's checks promise: ids that
/// are names, numbers within bounds, an image look for exactly the images, no repeated ids.
#[test]
fn reference_lists_that_decode_are_within_their_bounds() {
    start_watchdog();
    let whole = Message::ReferenceList {
        entries: sample_references(),
    }
    .encode(5);
    let (mut total, mut decoded) = (0usize, 0usize);
    for seed in seeds(400) {
        let mut rng = Rng::new(seed);
        for _ in 0..200 {
            let frame = mutate(&mut rng, &whole);
            check_decoding("reference lists", seed, &frame);
            total += 1;
            let Ok((_, Message::ReferenceList { entries })) = Message::decode(&frame) else {
                continue;
            };
            decoded += 1;
            assert!(entries.len() <= 256);
            let mut ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
            ids.sort_unstable();
            ids.dedup();
            assert_eq!(ids.len(), entries.len(), "seed {seed}: repeated ids");
            for e in &entries {
                let ok_id = (1..=32).contains(&e.id.len())
                    && e.id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
                assert!(ok_id, "seed {seed}: id {:?}", e.id);
                assert!(e.name.len() <= 128 && !e.name.chars().any(char::is_control));
                let within =
                    |v: &[f64; 3], max: f64| v.iter().all(|x| x.is_finite() && x.abs() <= max);
                assert!(within(&e.position, 1e7), "seed {seed}: {:?}", e.position);
                assert!(within(&e.rotation, 1e5), "seed {seed}: {:?}", e.rotation);
                assert!(within(&e.scale, 1e6), "seed {seed}: {:?}", e.scale);
                assert!(e.opacity.is_finite() && e.color.iter().all(|c| c.is_finite()));
                assert_eq!(e.image.is_some(), e.kind == Kind::Image, "seed {seed}");
                if let Some(look) = &e.image {
                    assert!(look.width.is_finite() && look.width.abs() <= 1e7);
                    assert!(look.pivot < 16);
                }
            }
        }
    }
    eprintln!("reference lists: {total} inputs, {decoded} of them still decoded");
    assert!(decoded > 0);
}

/// A snapshot that promises the biggest allowed vehicle but carries a few bytes must not make
/// the decoder reserve it; and the real thing (a vehicle that compresses a lot) must stay under
/// the cap.
#[test]
fn snapshot_sizes_are_checked_against_what_arrives() {
    start_watchdog();
    let mut forged = header(PROTOCOL, 6, 1);
    forged.extend(0u32.to_le_bytes()); // commit
    forged.extend(0u32.to_le_bytes()); // no ids
    forged.extend((caps::SNAPSHOT_BYTES as u32).to_le_bytes()); // unpacked length
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(&[7; 100], 6);
    forged.extend((packed.len() as u32).to_le_bytes());
    forged.extend(&packed);
    let (result, largest) = largest_allocation(|| Message::decode(&forged));
    assert!(result.is_err(), "100 bytes can't be the 16 MiB it promised");
    assert!(
        largest < 1 << 20,
        "decoding a {} byte snapshot that promises 16 MiB allocated {largest} bytes at once",
        forged.len()
    );

    // Over the cap is refused before anything is unpacked.
    let mut over = forged.clone();
    over[HEADER_LEN + 8..HEADER_LEN + 12]
        .copy_from_slice(&(caps::SNAPSHOT_BYTES as u32 + 1).to_le_bytes());
    let (result, largest) = largest_allocation(|| Message::decode(&over));
    assert!(result.is_err());
    assert!(largest < 1 << 20, "allocated {largest}");
}

#[test]
fn reader_survives_any_sequence_of_reads() {
    start_watchdog();
    for seed in seeds(400) {
        let mut rng = Rng::new(seed);
        for _ in 0..100 {
            let bytes = if rng.chance(50) {
                rng.bytes_below(300)
            } else {
                {
                    let items = 1 + rng.below(40);
                    structured_tail(&mut rng, items)
                }
            };
            note_case("reader", seed, &bytes);
            let (result, largest) = largest_allocation(|| {
                catch_unwind(AssertUnwindSafe(|| {
                    let mut reader = Reader::new(&bytes);
                    for _ in 0..20 {
                        let max = *rng.pick(&[0, 1, 5, 64, 4096, usize::MAX]);
                        let _ = match rng.below(9) {
                            0 => reader.u8().is_ok(),
                            1 => reader.u16().is_ok(),
                            2 => reader.u32().is_ok(),
                            3 => reader.u64().is_ok(),
                            4 => reader.f32().is_ok(),
                            5 => reader.bool().is_ok(),
                            6 => reader.count("count", max).is_ok(),
                            7 => reader.bytes("bytes", max).is_ok(),
                            _ => reader.str("str", max).is_ok(),
                        };
                    }
                    let _ = reader.rest();
                }))
            });
            if let Err(payload) = result {
                panic!(
                    "seed {seed}: reader panicked on {}: {}",
                    hex(&bytes),
                    panic_text(&*payload)
                );
            }
            assert!(
                largest <= ALLOC_SLACK + ALLOC_PER_BYTE * bytes.len(),
                "seed {seed}: reader allocated {largest} bytes on {}",
                hex(&bytes)
            );
        }
    }
}

/// The vehicle inside a snapshot comes from a peer too.
#[test]
fn vehicle_decoder_survives_junk() {
    start_watchdog();
    let valid = Vehicle {
        archive: (0..500u32).map(|n| n as u8).collect(),
        groups: vec![[1, 2], [3, 4], [0, 65_535]],
    }
    .encode();
    for seed in seeds(400) {
        let mut rng = Rng::new(seed);
        for _ in 0..100 {
            let bytes = match rng.below(3) {
                0 => rng.bytes_below(600),
                1 => {
                    let items = 1 + rng.below(20);
                    structured_tail(&mut rng, items)
                }
                _ => mutate(&mut rng, &valid),
            };
            note_case("vehicle", seed, &bytes);
            let (result, largest) = largest_allocation(|| catch_unwind(|| Vehicle::decode(&bytes)));
            if let Err(payload) = result {
                panic!(
                    "seed {seed}: Vehicle::decode panicked on {}: {}",
                    hex(&bytes),
                    panic_text(&*payload)
                );
            }
            assert!(
                largest <= ALLOC_SLACK + ALLOC_PER_BYTE * bytes.len(),
                "seed {seed}: Vehicle::decode allocated {largest} bytes on {}",
                hex(&bytes)
            );
        }
    }
}

// --- Putting parts together ---

#[test]
fn parts_in_order_give_the_message_back() {
    for message in sample_messages() {
        if matches!(message, Message::Part { .. }) {
            continue;
        }
        let whole = message.encode(5);
        for k in 2..6 {
            let mut assembler = Assembler::default();
            let parts: Vec<Vec<u8>> = split_in_parts(&whole, k)
                .into_iter()
                .map(|(index, count, data)| Message::Part { index, count, data }.encode(5))
                .collect();
            let (last, early) = parts.split_last().unwrap();
            for frame in early {
                assert_eq!(assembler.receive(1, frame), Ok(None));
            }
            assert_eq!(assembler.receive(1, last), Ok(Some((5, message.clone()))));
        }
    }
}

#[test]
fn assembler_survives_broken_parts_from_several_peers() {
    start_watchdog();
    let messages = sample_messages();
    let ping = Message::Ping.encode(9);
    for seed in seeds(400) {
        let mut rng = Rng::new(seed);
        let mut assembler = Assembler::default();
        for _ in 0..100 {
            let peer: PeerId = 1 + rng.below(3) as u64;
            for frame in hostile_frames(&mut rng, &messages, 9) {
                note_case("assembler", seed, &frame);
                let (result, largest) = largest_allocation(|| {
                    catch_unwind(AssertUnwindSafe(|| assembler.receive(peer, &frame)))
                });
                if let Err(payload) = result {
                    panic!(
                        "seed {seed}: assembler panicked for peer {peer} on {}: {}",
                        hex(&frame),
                        panic_text(&*payload)
                    );
                }
                // What it keeps for a peer is what that peer sent, so the bound is on the frame
                // plus what it holds (a few thousand bytes at most in this test).
                assert!(
                    largest <= 4 * ALLOC_SLACK + ALLOC_PER_BYTE * frame.len(),
                    "seed {seed}: assembler allocated {largest} bytes for a {} byte frame: {}",
                    frame.len(),
                    hex(&frame)
                );
            }
            if rng.chance(10) {
                assembler.forget(peer);
            }
        }
        // Whatever happened, every peer can still send a whole message.
        for peer in 1..=3 {
            assert_eq!(
                assembler.receive(peer, &ping),
                Ok(Some((9, Message::Ping))),
                "seed {seed}: peer {peer} is stuck"
            );
        }
    }
}

// --- Live sessions ---

const HOST: PeerId = 100;
const CODE: &str = "ABCD";
const GAME_VERSION: &str = "1.11.7";
const SESSION: u32 = 0x00C0_FFEE;
const HOST_BRICKS: u32 = 3;

/// Recently injected frames, shown when a session panics.
const REMEMBERED: usize = 6;

/// What a session told the editor, counted.
#[derive(Default)]
struct Log {
    /// Ops of other members' edits that the editor was told to apply.
    applied: Vec<Op>,
    drifts: usize,
    ended: usize,
    bad_messages: usize,
}

/// A subhost (index 0, peer `HOST`) and members (peers `HOST + 1`...) wired together by hand:
/// what a session sends goes to the session with that peer id, and to nobody if there is none.
/// The editors are not simulated beyond what a subhost needs to answer snapshot requests: the
/// list of its bricks.
struct Rig {
    sessions: Vec<(PeerId, Session)>,
    logs: Vec<Log>,
    /// The subhost's bricks, which it sends in snapshots.
    bricks: Vec<CoopId>,
    recent: Vec<(usize, PeerId, Vec<u8>)>,
    seed: u64,
    test: &'static str,
    /// The clock the sessions are given, in milliseconds (`Session::tick`).
    now: u64,
}

impl Rig {
    /// A subhost and `guests` members that joined and have the vehicle, unless `settle` is false:
    /// then they have only asked to join.
    fn new(test: &'static str, seed: u64, guests: usize, settle: bool) -> Self {
        let mut sessions = vec![(HOST, Session::new(peer_info(false)))];
        for n in 0..guests {
            sessions.push((HOST + 1 + n as u64, Session::new(peer_info(false))));
        }
        let bricks = sessions[0]
            .1
            .host(
                SESSION,
                "host".into(),
                HostSettings {
                    code: CODE.into(),
                    max_members: 8,
                    approve: false,
                },
                HOST_BRICKS as usize,
            )
            .unwrap();
        for (_, guest) in &mut sessions[1..] {
            guest.join(HOST, CODE.into(), "guest".into()).unwrap();
        }
        let mut rig = Self {
            logs: sessions.iter().map(|_| Log::default()).collect(),
            sessions,
            bricks,
            recent: Vec::new(),
            seed,
            test,
            now: 0,
        };
        if settle {
            rig.settle();
        }
        rig
    }

    fn index_of(&self, peer: PeerId) -> Option<usize> {
        self.sessions.iter().position(|(p, _)| *p == peer)
    }

    /// Moves frames and events around until nothing is left to do, letting time pass so that
    /// snapshots held back by a member's allowance get sent. Panics if it never ends.
    fn settle(&mut self) {
        let mut quiet_rounds = 0;
        for _ in 0..1000 {
            for (_, session) in &mut self.sessions {
                session.tick(self.now);
            }
            let mut busy = false;
            for i in 0..self.sessions.len() {
                let from = self.sessions[i].0;
                for Outgoing { to, bytes, .. } in self.sessions[i].1.take_outgoing() {
                    busy = true;
                    if let Some(j) = self.index_of(to) {
                        self.sessions[j].1.receive(from, &bytes);
                    }
                }
            }
            for i in 0..self.sessions.len() {
                busy |= self.handle_events(i);
            }
            if busy {
                quiet_rounds = 0;
            } else {
                quiet_rounds += 1;
                if quiet_rounds == 2 {
                    return;
                }
                // The snapshot allowance refills at one per five seconds.
                self.now += 6_000;
            }
        }
        panic!(
            "seed {}: {}: sessions still exchange frames after 1000 rounds",
            self.seed, self.test
        );
    }

    /// Does what the mod would for session `i`'s events; whether there were any.
    fn handle_events(&mut self, i: usize) -> bool {
        let events = self.sessions[i].1.take_events();
        let any = !events.is_empty();
        let (mut snapshots, mut orders) = (Vec::new(), Vec::new());
        for event in events {
            match event {
                Event::Apply { ops, .. } => {
                    if i == 0 {
                        ops.iter().for_each(|op| self.apply_to_bricks(op));
                    }
                    self.logs[i].applied.extend(ops);
                }
                Event::SnapshotWanted(request) => snapshots.push(request),
                Event::OrderWanted(peer) => orders.push(peer),
                Event::Drifted | Event::UndoConflict => self.logs[i].drifts += 1,
                Event::Ended(_) => self.logs[i].ended += 1,
                Event::BadMessage { .. } => self.logs[i].bad_messages += 1,
                _ => {}
            }
        }
        for request in snapshots {
            let vehicle = Vehicle {
                archive: vec![1, 2, 3],
                groups: vec![[0, 0]; self.bricks.len()],
            };
            let bricks = self.bricks.clone();
            let _ = self.sessions[i]
                .1
                .send_snapshot(request, bricks, vehicle.encode());
        }
        for peer in orders {
            let bricks = self.bricks.clone();
            let _ = self.sessions[i].1.send_order(peer, bricks);
        }
        any
    }

    /// The subhost's list of bricks, as its editor would keep it.
    fn apply_to_bricks(&mut self, op: &Op) {
        match op {
            Op::Create { ids, .. } => {
                for &id in ids {
                    if !self.bricks.contains(&id) {
                        self.bricks.push(id);
                    }
                }
            }
            Op::Delete { id } => self.bricks.retain(|b| b != id),
            _ => {}
        }
    }

    /// Delivers a frame to session `target` as if `from` sent it, watching for panics and for
    /// big allocations.
    fn inject(&mut self, target: usize, from: PeerId, frame: &[u8]) {
        note_case(self.test, self.seed, frame);
        if self.recent.len() == REMEMBERED {
            self.recent.remove(0);
        }
        self.recent.push((target, from, frame.to_vec()));
        let session = &mut self.sessions[target].1;
        let (result, largest) =
            largest_allocation(|| catch_unwind(AssertUnwindSafe(|| session.receive(from, frame))));
        if let Err(payload) = result {
            panic!(
                "seed {}: {}: session {target} panicked on a frame from peer {from}: {}\n  frame: {}\n  frames before: {}",
                self.seed,
                self.test,
                panic_text(&*payload),
                hex(frame),
                self.recent_frames(),
            );
        }
        assert!(
            largest <= SESSION_ALLOC_LIMIT,
            "seed {}: {}: session {target} allocated {largest} bytes at once for a {} byte frame from peer {from}: {}",
            self.seed,
            self.test,
            frame.len(),
            hex(frame),
        );
    }

    fn recent_frames(&self) -> String {
        self.recent
            .iter()
            .map(|(target, from, frame)| format!("[to {target} from {from}: {}]", hex(frame)))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn is_member(&self, i: usize) -> bool {
        let session = &self.sessions[i].1;
        !session.is_idle() && session.is_loaded()
    }

    /// An honest edit by session `i`: a new brick, or a change to one of the subhost's.
    fn honest_edit(&mut self, rng: &mut Rng, i: usize) {
        let existing = if self.bricks.is_empty() {
            id(0, 1)
        } else {
            self.bricks[rng.below(self.bricks.len())]
        };
        let ops = match rng.below(4) {
            0 => {
                let Ok(ids) = self.sessions[i].1.new_ids(1) else {
                    return;
                };
                vec![Op::Create {
                    ids,
                    archive: rng.bytes_below_at_least_one(30),
                }]
            }
            1 => vec![Op::Set {
                id: existing,
                property: "Color".into(),
                value: Value::Raw(rng.bytes(4)),
            }],
            2 => vec![Op::Move {
                id: existing,
                transform: Transform {
                    location: [rng.below(100) as f32; 3],
                    rotation: [0.0; 3],
                },
            }],
            _ => vec![Op::Delete { id: existing }],
        };
        // Not loaded or not in a session any more (a hostile peer ended it): nothing to edit.
        if self.sessions[i].1.edit(ops.clone()).is_ok() && i == 0 {
            ops.iter().for_each(|op| self.apply_to_bricks(op));
        }
    }

    /// Session `from` makes a new brick; what went wrong if it couldn't be made, or if a session
    /// in `receivers` didn't hear of it.
    fn edit_failure(&mut self, from: usize, receivers: &[usize]) -> Option<String> {
        let ids = match self.sessions[from].1.new_ids(1) {
            Ok(ids) => ids,
            Err(error) => return Some(format!("no ids for session {from}: {error:?}")),
        };
        let op = Op::Create {
            ids,
            archive: vec![42; 9],
        };
        self.logs.iter_mut().for_each(|log| log.applied.clear());
        if let Err(error) = self.sessions[from].1.edit(vec![op.clone()]) {
            return Some(format!("session {from} can't edit: {error:?}"));
        }
        if from == 0 {
            self.apply_to_bricks(&op);
        }
        self.settle();
        let missing: Vec<usize> = receivers
            .iter()
            .copied()
            .filter(|&to| !self.logs[to].applied.contains(&op))
            .collect();
        (!missing.is_empty()).then(|| {
            format!(
                "the edit of session {from} ({:?}) never reached sessions {missing:?};                  members: {:?}, drifts: {:?}",
                self.sessions[from].1.slot(),
                self.sessions[0].1.members(),
                self.logs.iter().map(|l| l.drifts).collect::<Vec<_>>(),
            )
        })
    }

    /// Whether member `i`'s edits reach the subhost. Several, as a member that was told the wrong
    /// slot can get one through by chance (undoing a delete of its own is allowed to make an old
    /// id again).
    fn member_edits_flow(&mut self, i: usize) -> bool {
        (0..3).all(|_| self.edit_flows(i, &[0]))
    }

    /// Whether [`Rig::edit_failure`] found nothing.
    fn edit_flows(&mut self, from: usize, receivers: &[usize]) -> bool {
        self.edit_failure(from, receivers).is_none()
    }

    /// Like [`Rig::edit_flows`], but a failure is a test failure.
    fn assert_edit_flows(&mut self, from: usize, receivers: &[usize]) {
        if let Some(problem) = self.edit_failure(from, receivers) {
            panic!(
                "seed {}: {}: {problem}
  last frames: {}",
                self.seed,
                self.test,
                self.recent_frames()
            );
        }
    }

    /// Fails if a session that should have been left alone isn't a member any more, or had to
    /// resync.
    fn assert_untouched(&self, honest: &[usize]) {
        for &i in honest {
            let log = &self.logs[i];
            assert!(
                self.is_member(i),
                "seed {}: {}: session {i} was thrown out of the session\n  last frames: {}",
                self.seed,
                self.test,
                self.recent_frames()
            );
            assert_eq!(
                (log.ended, log.drifts),
                (0, 0),
                "seed {}: {}: session {i} ended or drifted\n  last frames: {}",
                self.seed,
                self.test,
                self.recent_frames()
            );
        }
    }

    /// Gets session `i` (a member) back into the session if hostile frames threw it out or left
    /// it with a wrong idea of the vehicle, and clears the logs.
    fn recover_member(&mut self, i: usize) {
        for attempt in 0..4 {
            self.settle();
            if self.is_member(i) {
                self.sessions[i].1.request_resync();
                self.settle();
                // A hostile subhost can also leave it with another session id or slot than the
                // real subhost gives, which only leaving and joining again mends.
                if self.is_member(i) && self.edit_flows(0, &[i]) && self.member_edits_flow(i) {
                    self.logs.iter_mut().for_each(|log| *log = Log::default());
                    return;
                }
            }
            if attempt < 3 {
                self.sessions[i].1.leave();
                self.settle();
                self.sessions[i]
                    .1
                    .join(HOST, CODE.into(), "guest".into())
                    .unwrap();
            }
        }
        panic!(
            "seed {}: {}: session {i} can't get back into the session
  last frames: {}",
            self.seed,
            self.test,
            self.recent_frames()
        );
    }
}

/// Peers that are not in the session, including odd ids.
fn stranger(rng: &mut Rng) -> PeerId {
    match rng.below(6) {
        0 => 0,
        1 => u64::MAX,
        2 => HOST - 1,
        3 => 76561198000000000 + rng.below(50) as u64,
        _ => 900 + rng.below(10) as u64,
    }
}

const STEPS: usize = 40;

/// Strangers send anything to the subhost and to a member: nobody in the session notices, and
/// edits keep flowing.
#[test]
fn strangers_cannot_disturb_a_session() {
    start_watchdog();
    let messages = sample_messages();
    for seed in seeds(120) {
        let mut rng = Rng::new(seed);
        let mut rig = Rig::new("strangers", seed, 2, true);
        for _ in 0..STEPS {
            match rng.below(10) {
                0..=6 => {
                    let target = rng.below(rig.sessions.len());
                    let from = stranger(&mut rng);
                    // Right session id most of the time, so frames get as far as they can.
                    let session = if rng.chance(80) {
                        SESSION
                    } else {
                        rng.next() as u32
                    };
                    for frame in hostile_frames(&mut rng, &messages, session) {
                        rig.inject(target, from, &frame);
                    }
                }
                7 | 8 => {
                    let i = rng.below(rig.sessions.len());
                    rig.honest_edit(&mut rng, i);
                    rig.settle();
                }
                _ => rig.settle(),
            }
        }
        rig.settle();
        rig.assert_untouched(&[1, 2]);
        rig.assert_edit_flows(1, &[0, 2]);
        rig.assert_edit_flows(0, &[1, 2]);
        rig.assert_edit_flows(2, &[0, 1]);
        rig.assert_untouched(&[1, 2]);
    }
}

/// A member sends the subhost anything, with the session's id: it may get itself removed, but the
/// other member must not notice.
#[test]
fn a_hostile_member_cannot_disturb_the_others() {
    start_watchdog();
    let messages = sample_messages();
    for seed in seeds(120) {
        let mut rng = Rng::new(seed);
        let mut rig = Rig::new("hostile member", seed, 2, true);
        let hostile = HOST + 1;
        for _ in 0..STEPS {
            match rng.below(10) {
                0..=6 => {
                    let session = if rng.chance(90) {
                        SESSION
                    } else {
                        rng.next() as u32
                    };
                    for frame in hostile_frames(&mut rng, &messages, session) {
                        rig.inject(0, hostile, &frame);
                    }
                    if rng.chance(50) {
                        rig.settle();
                    }
                }
                7 | 8 => {
                    // Honest members: the subhost and the second member.
                    let i = *rng.pick(&[0, 2]);
                    rig.honest_edit(&mut rng, i);
                    rig.settle();
                }
                _ => rig.settle(),
            }
        }
        rig.settle();
        rig.assert_untouched(&[2]);
        rig.assert_edit_flows(2, &[0]);
        rig.assert_edit_flows(0, &[2]);
        rig.assert_untouched(&[2]);
    }
}

/// The subhost sends a member anything, with the session's id: it may end the session for the
/// member or fill its head with nonsense, but the member can get back in and edit.
#[test]
fn a_hostile_subhost_cannot_wedge_a_member() {
    start_watchdog();
    let messages = sample_messages();
    for seed in seeds(120) {
        let mut rng = Rng::new(seed);
        // Half the time the member is still joining when it starts.
        let joined = rng.chance(50);
        let mut rig = Rig::new("hostile subhost", seed, 1, joined);
        for _ in 0..STEPS {
            match rng.below(10) {
                0..=7 => {
                    let session = if rng.chance(90) {
                        SESSION
                    } else {
                        rng.next() as u32
                    };
                    for frame in hostile_frames(&mut rng, &messages, session) {
                        rig.inject(1, HOST, &frame);
                    }
                    if rng.chance(30) {
                        rig.settle();
                    }
                }
                8 => rig.honest_edit(&mut rng, 0),
                _ => {
                    // The member edits, if it can still.
                    rig.honest_edit(&mut rng, 1);
                    rig.settle();
                }
            }
        }
        rig.recover_member(1);
        rig.assert_edit_flows(1, &[0]);
        rig.assert_edit_flows(0, &[1]);
        rig.assert_untouched(&[1]);
    }
}

/// Sessions that are idle or still joining get everything too, from the subhost's address and
/// from others.
#[test]
fn idle_and_joining_sessions_survive_junk() {
    start_watchdog();
    let messages = sample_messages();
    for seed in seeds(120) {
        let mut rng = Rng::new(seed);
        let mut rig = Rig::new("idle and joining", seed, 2, false);
        // Session 1 keeps waiting to join; session 2 never asked to.
        rig.sessions[2].1 = Session::new(peer_info(false));
        for _ in 0..STEPS {
            let target = 1 + rng.below(2);
            let other = stranger(&mut rng);
            let from = *rng.pick(&[HOST, HOST, other]);
            for frame in hostile_frames(&mut rng, &messages, SESSION) {
                rig.inject(target, from, &frame);
            }
        }
        // Whatever they ended up as, they can start over.
        for i in 1..=2 {
            rig.sessions[i].1.leave();
        }
        rig.sessions[0].1.take_outgoing();
        rig.settle();
        for i in 1..=2 {
            assert!(rig.sessions[i].1.is_idle(), "seed {seed}: session {i}");
        }
        rig.sessions[2]
            .1
            .host(
                7,
                "other".into(),
                HostSettings {
                    code: "x".into(),
                    max_members: 1,
                    approve: false,
                },
                0,
            )
            .unwrap();
    }
}

/// Not a check, a measurement: how much a stranger can make the receiver hold with parts that
/// never complete. Every peer id gets its own half-assembled message, and nothing limits how
/// many peer ids there are, so the receiver's memory grows with the number of strangers.
#[test]
fn unfinished_parts_of_strangers_are_not_kept() {
    // A hello in two parts from a stranger: strangers only send what fits one frame, so the first
    // part is dropped, not kept for the second.
    let mut session = Session::new(peer_info(false));
    let hello = Message::Hello(peer_info(false)).encode(1);
    let (first, second) = hello.split_at(hello.len() / 2);
    for (index, data) in [first, second].into_iter().enumerate() {
        let part = Message::Part {
            index: index as u32,
            count: 2,
            data: data.to_vec(),
        };
        session.receive(1000, &part.encode(1));
    }
    assert!(
        session.take_outgoing().is_empty(),
        "a stranger's parts were put together"
    );
    // In one frame, the same hello is answered.
    session.receive(1000, &hello);
    assert_eq!(session.take_outgoing().len(), 1);
}
