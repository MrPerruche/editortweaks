//! The threads measuring bricks against the references ([`model_io::fit`]), so the game never waits
//! on them: the game thread sends the references' triangles when they change and a job per brick
//! to measure, and takes the results back when it builds their meshes. One thread receives the
//! jobs and spreads each batch over a few more, which share the references.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use model_io::fit::{self, Sampled, Surface};

use crate::game::editor_object::Vector;
use crate::math::{self, Quat};

/// A brick's surface in its mesh's space (unscaled), wound the way the engine wants (front faces
/// clockwise).
pub struct Source {
    pub positions: Vec<[f32; 3]>,
    pub corners: Vec<[u32; 3]>,
    /// Whether it's the brick's own surface (its complex collision), not a stand-in for it (its
    /// simple shapes, a spinner swept again, its box).
    pub exact: bool,
}

/// A brick to measure: its surface, scaled by `scale`, then placed at `location` turned by
/// `rotation` (where its mesh component is, in the editor root's space like the references).
pub struct Job {
    pub brick: usize,
    /// Which of the brick's jobs this is: results older than what's shown are thrown away.
    pub serial: u64,
    pub source: Arc<Source>,
    pub scale: Vector,
    pub location: Vector,
    pub rotation: Vector,
}

/// A brick measured: its surface, subdivided, with each vertex's distance in its U.
pub struct Done {
    pub brick: usize,
    pub serial: u64,
    pub sampled: Sampled,
    pub took: Duration,
}

enum Message {
    /// The references' triangles changed (in the editor root's space).
    References(Vec<[[f32; 3]; 3]>),
    Measure(Job),
}

pub struct Worker {
    messages: Sender<Message>,
    results: Receiver<Done>,
    threads: usize,
}

/// How many threads measure: the cores but two (the game's and its render thread), at least one.
fn thread_count() -> usize {
    std::thread::available_parallelism()
        .map_or(1, NonZeroUsize::get)
        .saturating_sub(2)
        .max(1)
}

impl Worker {
    pub fn start() -> Result<Self, String> {
        let (messages, inbox) = mpsc::channel();
        let (outbox, results) = mpsc::channel();
        let threads = thread_count();
        std::thread::Builder::new()
            .name("editortweaks reference fit".into())
            .spawn(move || run(&inbox, &outbox, threads))
            .map_err(|error| format!("no thread for the reference fit: {error}"))?;
        Ok(Self {
            messages,
            results,
            threads,
        })
    }

    /// How many threads measure.
    pub fn threads(&self) -> usize {
        self.threads
    }

    /// Measures against `triangles` from now on. Whether the thread is still there.
    pub fn set_references(&self, triangles: Vec<[[f32; 3]; 3]>) -> bool {
        self.messages.send(Message::References(triangles)).is_ok()
    }

    /// Whether the thread is still there.
    pub fn measure(&self, job: Job) -> bool {
        self.messages.send(Message::Measure(job)).is_ok()
    }

    /// A finished job, if any.
    pub fn finished(&self) -> Option<Done> {
        self.results.try_recv().ok()
    }
}

fn run(inbox: &Receiver<Message>, outbox: &Sender<Done>, threads: usize) {
    let mut surface = Surface::default();
    while let Ok(first) = inbox.recv() {
        // Everything waiting at once: the latest references, then the latest job of each brick
        // (older ones would be thrown away anyway).
        let mut jobs = BTreeMap::new();
        for message in core::iter::once(first).chain(inbox.try_iter()) {
            match message {
                Message::References(triangles) => surface = Surface::new(&triangles),
                Message::Measure(job) => {
                    jobs.insert(job.brick, job);
                }
            }
        }
        let jobs: Vec<Job> = jobs.into_values().collect();
        if !measure_all(&jobs, &surface, outbox, threads) {
            // The game side is gone.
            return;
        }
    }
}

/// Measures `jobs` on up to `threads` threads, each taking the next job not taken yet, and sends
/// the results as they're done. Whether the game side is still there to take them.
fn measure_all(jobs: &[Job], surface: &Surface, outbox: &Sender<Done>, threads: usize) -> bool {
    let next = AtomicUsize::new(0);
    let measure_next = |outbox: Sender<Done>| {
        while let Some(job) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
            let started = Instant::now();
            let sampled = measure(job, surface);
            let done = Done {
                brick: job.brick,
                serial: job.serial,
                sampled,
                took: started.elapsed(),
            };
            if outbox.send(done).is_err() {
                return false;
            }
        }
        true
    };
    std::thread::scope(|scope| {
        let helpers: Vec<_> = (1..threads.min(jobs.len()))
            .map(|_| {
                let outbox = outbox.clone();
                scope.spawn(move || measure_next(outbox))
            })
            .collect();
        let mine = measure_next(outbox.clone());
        // Every helper is joined, even after one found the game side gone.
        let helpers_ok: Vec<bool> = helpers
            .into_iter()
            .map(|helper| helper.join().unwrap_or(false))
            .collect();
        mine && helpers_ok.into_iter().all(|ok| ok)
    })
}

fn measure(job: &Job, surface: &Surface) -> Sampled {
    let rotation = Quat::from_rotator(math::widen(job.rotation));
    let location = math::widen(job.location);
    // As the engine places the brick's mesh: `parent = Rotate(scale * local) + location`.
    let to_reference = |point: [f32; 3]| {
        let scaled = core::array::from_fn(|axis| f64::from(point[axis] * job.scale[axis]));
        math::narrow(math::add(rotation.rotate(scaled), location))
    };
    fit::sample(
        &job.source.positions,
        &job.source.corners,
        &to_reference,
        surface,
    )
}
