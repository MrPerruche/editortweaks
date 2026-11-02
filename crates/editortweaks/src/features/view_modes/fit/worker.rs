//! The threads measuring bricks against the references ([`model_io::fit`]), so the game never waits
//! on them: the game thread sends the references' triangles when they change and a job per brick
//! to measure, and takes the results back when it builds their meshes. One thread receives the
//! jobs and spreads them over a few more, which share the references.
//!
//! Each brick is measured twice ([`Stage`]): first a quick preview, so every brick has colors soon
//! after a change, then finely. New jobs (a brick dragged, the references changed) always get their
//! previews before any more bricks are refined, so the colors of what's being changed keep up.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use model_io::fit::{self, Detail, Sampled, Surface};

use crate::game::editor_object::Vector;
use crate::math::{self, Quat};

/// How many bricks each thread refines before the inbox is looked at again: the longest a new
/// job waits is about that many fine measures.
const REFINES_PER_LOOK: usize = 2;

/// A brick's surface in its mesh's space (unscaled), wound the way the engine wants (front faces
/// clockwise).
pub struct Source {
    pub positions: Vec<[f32; 3]>,
    pub corners: Vec<[u32; 3]>,
}

/// A brick to measure: its surface, scaled by `scale`, then placed at `location` turned by
/// `rotation` (where its mesh component is, in the editor root's space like the references).
#[derive(Clone)]
pub struct Job {
    pub brick: usize,
    /// Which of the brick's jobs this is: results older than what's shown are thrown away.
    pub serial: u64,
    pub source: Arc<Source>,
    pub scale: Vector,
    pub location: Vector,
    pub rotation: Vector,
}

/// Which measure of a job: the quick one comes first, and the fine one replaces it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Stage {
    Preview,
    Fine,
}

impl Stage {
    fn detail(self) -> &'static Detail {
        match self {
            Self::Preview => &Detail::PREVIEW,
            Self::Fine => &Detail::FINE,
        }
    }
}

/// A brick measured: its surface, subdivided, with each vertex's distance in its U.
pub struct Done {
    pub brick: usize,
    pub serial: u64,
    pub stage: Stage,
    /// `None` for a fine measure just like the preview, which then stays.
    pub sampled: Option<Sampled>,
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

/// What's waiting to be measured, one job per brick (older ones would be thrown away anyway).
#[derive(Default)]
struct Queue {
    surface: Surface,
    previews: BTreeMap<usize, Job>,
    /// Bricks whose preview was sent, with that preview: a fine measure just like it isn't sent.
    refines: BTreeMap<usize, (Job, Sampled)>,
}

impl Queue {
    fn is_empty(&self) -> bool {
        self.previews.is_empty() && self.refines.is_empty()
    }

    fn take(&mut self, message: Message) {
        match message {
            Message::References(triangles) => {
                self.surface = Surface::new(&triangles);
                // Previewed against the old references: the game sends every brick again.
                self.refines.clear();
            }
            Message::Measure(job) => {
                self.refines.remove(&job.brick);
                self.previews.insert(job.brick, job);
            }
        }
    }
}

fn run(inbox: &Receiver<Message>, outbox: &Sender<Done>, threads: usize) {
    let mut queue = Queue::default();
    loop {
        if queue.is_empty() {
            let Ok(message) = inbox.recv() else {
                // The game side is gone.
                return;
            };
            queue.take(message);
        }
        for message in inbox.try_iter() {
            queue.take(message);
        }
        let game_side_there = if queue.previews.is_empty() {
            refine_some(&mut queue, outbox, threads)
        } else {
            preview_all(&mut queue, outbox, threads)
        };
        if !game_side_there {
            return;
        }
    }
}

/// Previews every brick waiting for it, and queues them to be refined. Whether the game side is
/// still there.
fn preview_all(queue: &mut Queue, outbox: &Sender<Done>, threads: usize) -> bool {
    let jobs: Vec<Job> = std::mem::take(&mut queue.previews).into_values().collect();
    let previewed = Mutex::new(Vec::with_capacity(jobs.len()));
    let surface = &queue.surface;
    let there = in_parallel(&jobs, threads, |job| {
        let started = Instant::now();
        let sampled = measure(job, surface, Stage::Preview);
        previewed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((job.clone(), sampled.clone()));
        outbox
            .send(Done {
                brick: job.brick,
                serial: job.serial,
                stage: Stage::Preview,
                sampled: Some(sampled),
                took: started.elapsed(),
            })
            .is_ok()
    });
    let previewed = previewed
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner);
    for (job, sampled) in previewed {
        queue.refines.insert(job.brick, (job, sampled));
    }
    there
}

/// Refines a few of the bricks waiting for it ([`REFINES_PER_LOOK`] per thread), so new jobs
/// don't wait long. Whether the game side is still there.
fn refine_some(queue: &mut Queue, outbox: &Sender<Done>, threads: usize) -> bool {
    let batch: Vec<(Job, Sampled)> = (0..threads * REFINES_PER_LOOK)
        .map_while(|_| queue.refines.pop_first().map(|(_, waiting)| waiting))
        .collect();
    let surface = &queue.surface;
    in_parallel(&batch, threads, |(job, preview)| {
        let started = Instant::now();
        let sampled = measure(job, surface, Stage::Fine);
        outbox
            .send(Done {
                brick: job.brick,
                serial: job.serial,
                stage: Stage::Fine,
                sampled: (sampled != *preview).then_some(sampled),
                took: started.elapsed(),
            })
            .is_ok()
    })
}

/// Calls `work` with every item on up to `threads` threads, each taking the next item not taken
/// yet, until `work` returns false (the game side is gone). Whether it never did.
fn in_parallel<T: Sync>(items: &[T], threads: usize, work: impl Fn(&T) -> bool + Sync) -> bool {
    let next = AtomicUsize::new(0);
    let gone = AtomicBool::new(false);
    let work_through = || {
        while !gone.load(Ordering::Relaxed)
            && let Some(item) = items.get(next.fetch_add(1, Ordering::Relaxed))
        {
            if !work(item) {
                gone.store(true, Ordering::Relaxed);
            }
        }
    };
    std::thread::scope(|scope| {
        let helpers: Vec<_> = (1..threads.min(items.len()))
            .map(|_| scope.spawn(work_through))
            .collect();
        work_through();
        // Every helper is joined; one that panicked counts as the game side gone.
        for helper in helpers {
            if helper.join().is_err() {
                gone.store(true, Ordering::Relaxed);
            }
        }
    });
    !gone.load(Ordering::Relaxed)
}

fn measure(job: &Job, surface: &Surface, stage: Stage) -> Sampled {
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
        stage.detail(),
    )
}
