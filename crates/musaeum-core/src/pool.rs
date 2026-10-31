//! The render pool: worker processes, their pipes, and the event loop around
//! the [`Scheduler`].
//!
//! One thread owns all the state. Each worker gets a reader thread that turns its pipe
//! into events, so the event loop never blocks on a slow or stuck worker. The only
//! things it writes to are small messages on pipes whose peers have at most one job at
//! a time, and atomics in shared memory.

use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Write};
use std::mem::take;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{Ordering, fence};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use musaeum_shm::region::duplicate_file_read_only;
use musaeum_shm::slab::MAX_WORKERS;
use musaeum_shm::wire::{
    FatalCode, HostMessage, InitParams, PROTOCOL_VERSION, RenderJob, WorkerMessage, read_frame,
    write_frame,
};
use musaeum_shm::{DeliveryRing, PageGeometry, PixelSlab, TILE_BYTES, TileAnnouncement};
use thiserror::Error;
use windows_sys::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::demand::parse_demand;
use crate::job::JobObject;
use crate::scheduler::{CancelOrder, ReadyTile, Scheduler, TileOutcome};
use crate::stats::{PoolStats, StatsCollector};

/// `"MTLB"`, the first word of a Path A tile batch.
pub const BATCH_MAGIC: u32 = u32::from_le_bytes(*b"MTLB");
/// Most pages a document may declare; bounds what a worker can make the host allocate.
const MAX_PAGES: usize = 1 << 22;
/// A worker that keeps dying is left dead rather than restarted forever.
const MAX_WORKER_RESTARTS: u32 = 8;
/// How often the ring's consumer position is checked while tiles are out for delivery.
const ACK_POLL_INTERVAL: Duration = Duration::from_millis(1);
/// How long a worker gets to exit after being asked to.
const SHUTDOWN_GRACE: Duration = Duration::from_millis(500);

/// Receives one batch of tiles. An empty batch means the session is closing.
pub type Poller = Box<dyn FnOnce(Vec<u8>) + Send + 'static>;

/// How to start a pool.
#[derive(Clone, Debug)]
pub struct PoolConfig {
    /// Path of the worker binary.
    pub worker_exe: PathBuf,
    /// Path of the PDF. Workers receive a read-only handle, never this path.
    pub document: PathBuf,
    /// Number of worker processes.
    pub worker_count: usize,
    /// Cap on each worker's committed memory.
    pub process_memory_limit: Option<usize>,
    /// How long to wait for the workers to report in.
    pub startup_timeout: Duration,
}

/// How finished tiles leave the pool.
#[derive(Debug)]
pub enum DeliveryMode {
    /// Path A: tiles are copied into byte batches for pollers parked with
    /// [`RenderPool::park_poller`].
    Pull {
        /// Most tiles in one batch.
        max_batch: usize,
    },
    /// Path B: tiles stay in the slab and are announced through a shared ring.
    Ring {
        /// The ring, already visible to its consumer.
        ring: DeliveryRing,
        /// Most tiles announced but not yet consumed. Tiles held back beyond this can
        /// still be recalled if the compositor stops wanting them.
        window: u32,
    },
}

/// Errors from starting a pool.
#[derive(Debug, Error)]
pub enum PoolError {
    /// The configuration cannot work.
    #[error("invalid pool configuration: {0}")]
    BadConfig(String),
    /// An operating-system call failed.
    #[error("{context}: {source}")]
    Os {
        /// What was being attempted.
        context: &'static str,
        /// The failure.
        source: io::Error,
    },
    /// A worker reported a fatal error while starting.
    #[error("render worker failed: {code} (detail {detail})")]
    WorkerFatal {
        /// What went wrong.
        code: FatalCode,
        /// Code-specific detail.
        detail: u32,
    },
    /// A worker exited before reporting in.
    #[error("render worker {0} exited during startup")]
    WorkerDied(usize),
    /// A worker's report does not make sense.
    #[error("render worker {worker} sent an invalid handshake: {reason}")]
    BadHandshake {
        /// Which worker.
        worker: usize,
        /// What was wrong with it.
        reason: String,
    },
    /// The workers did not report in on time.
    #[error("render workers did not start within {0:?}")]
    StartupTimeout(Duration),
}

fn os_error(context: &'static str) -> impl FnOnce(io::Error) -> PoolError {
    move |source| PoolError::Os { context, source }
}

enum Event {
    Demand(Vec<u8>),
    Park(Poller),
    Worker {
        worker: usize,
        instance: u64,
        message: WorkerMessage,
    },
    Geometry {
        pages: Vec<PageGeometry>,
        elapsed_us: u64,
    },
    Exited {
        worker: usize,
        instance: u64,
    },
    Stats(Sender<PoolStats>),
    KillWorker(usize),
    Shutdown,
}

/// A running pool. Dropping it stops the workers and releases the slab's writers.
pub struct RenderPool {
    events: Sender<Event>,
    thread: Mutex<Option<JoinHandle<()>>>,
    pages: Arc<[PageGeometry]>,
    slab: Arc<PixelSlab>,
    started: Instant,
}

impl std::fmt::Debug for RenderPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderPool")
            .field("pages", &self.pages.len())
            .finish_non_exhaustive()
    }
}

impl RenderPool {
    /// Starts the workers and waits until all of them have the document open.
    pub fn start(
        config: PoolConfig,
        slab: Arc<PixelSlab>,
        delivery: DeliveryMode,
    ) -> Result<Self, PoolError> {
        if config.worker_count == 0 || config.worker_count > MAX_WORKERS {
            return Err(PoolError::BadConfig(format!(
                "worker count must be between 1 and {MAX_WORKERS}, got {}",
                config.worker_count
            )));
        }

        let (events, receiver) = mpsc::channel();
        let startup_timeout = config.startup_timeout;
        let worker_count = config.worker_count;
        let mut spawner = Spawner::new(config, Arc::clone(&slab), events.clone())?;

        let mut workers = Vec::with_capacity(worker_count);
        for worker in 0..worker_count {
            workers.push(Some(spawner.spawn(worker, worker == 0)?));
        }

        let started = Instant::now();
        let handshake = wait_until_ready(&receiver, worker_count, startup_timeout)?;
        let pages: Arc<[PageGeometry]> = handshake.pages.into();

        let mut scheduler = Scheduler::new(worker_count, slab.slot_count());
        for worker in 0..worker_count {
            scheduler.on_worker_ready(worker);
        }
        let mut stats = StatsCollector::default();
        stats.totals.open_us = handshake.open_us;
        stats.totals.geometry_us = handshake.geometry_us;

        let event_loop = EventLoop {
            receiver,
            spawner,
            workers,
            restarts: vec![0; worker_count],
            scheduler,
            delivery: Delivery::new(delivery),
            slab: Arc::clone(&slab),
            pages: Arc::clone(&pages),
            started,
            stats,
            cancel_times: HashMap::new(),
            cancels: Vec::new(),
            released: Vec::new(),
        };
        let thread = thread::Builder::new()
            .name("render-pool".into())
            .spawn(move || event_loop.run())
            .map_err(os_error("spawning the pool thread"))?;

        Ok(Self {
            events,
            thread: Mutex::new(Some(thread)),
            pages,
            slab,
            started,
        })
    }

    /// Page sizes, in page order.
    pub fn pages(&self) -> &[PageGeometry] {
        &self.pages
    }

    /// The pixel slab.
    pub fn slab(&self) -> &PixelSlab {
        &self.slab
    }

    /// Replaces the demand list. Parsing happens on the pool's thread, so a caller on
    /// a UI thread pays nothing for a large list.
    pub fn submit_demand(&self, encoded: Vec<u8>) {
        let _ = self.events.send(Event::Demand(encoded));
    }

    /// Waits for the next batch of tiles. Only meaningful on Path A; on Path B the
    /// poller is answered straight away with an empty batch.
    pub fn park_poller(&self, poller: Poller) {
        if let Err(mpsc::SendError(Event::Park(poller))) = self.events.send(Event::Park(poller)) {
            poller(Vec::new());
        }
    }

    /// Microseconds since the pool started, on the clock used for `done_us`.
    pub fn clock_us(&self) -> u64 {
        self.started.elapsed().as_micros() as u64
    }

    /// Takes a snapshot of the pool's measurements.
    pub fn stats(&self) -> Option<PoolStats> {
        let (sender, receiver) = mpsc::channel();
        self.events.send(Event::Stats(sender)).ok()?;
        receiver.recv().ok()
    }

    /// Fault injection for tests: kills a worker as if it had crashed.
    pub fn kill_worker(&self, worker: usize) {
        let _ = self.events.send(Event::KillWorker(worker));
    }

    /// Stops the workers and waits for the pool's thread to finish, after which nothing in
    /// this process touches the slab through the pool. Safe to call more than once.
    pub fn shutdown(&self) {
        let _ = self.events.send(Event::Shutdown);
        let thread = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

impl Drop for RenderPool {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct Handshake {
    pages: Vec<PageGeometry>,
    open_us: u64,
    geometry_us: u64,
}

fn wait_until_ready(
    receiver: &Receiver<Event>,
    worker_count: usize,
    timeout: Duration,
) -> Result<Handshake, PoolError> {
    let deadline = Instant::now() + timeout;
    let mut ready = vec![false; worker_count];
    let mut page_count = None;
    let mut open_us = 0;
    let mut geometry = None;

    while !(ready.iter().all(|&is_ready| is_ready) && geometry.is_some()) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(remaining) {
            Ok(Event::Worker {
                worker,
                message:
                    WorkerMessage::Ready {
                        protocol,
                        page_count: pages,
                        open_us: elapsed,
                    },
                ..
            }) => {
                if protocol != PROTOCOL_VERSION {
                    return Err(PoolError::BadHandshake {
                        worker,
                        reason: format!("protocol {protocol}, expected {PROTOCOL_VERSION}"),
                    });
                }
                if worker == 0 {
                    open_us = elapsed;
                }
                if *page_count.get_or_insert(pages) != pages {
                    return Err(PoolError::BadHandshake {
                        worker,
                        reason: "workers disagree about the page count".into(),
                    });
                }
                ready[worker] = true;
            }
            Ok(Event::Worker {
                message: WorkerMessage::Fatal { code, detail },
                ..
            }) => return Err(PoolError::WorkerFatal { code, detail }),
            Ok(Event::Geometry {
                pages, elapsed_us, ..
            }) => geometry = Some((pages, elapsed_us)),
            Ok(Event::Exited { worker, .. }) => return Err(PoolError::WorkerDied(worker)),
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => return Err(PoolError::StartupTimeout(timeout)),
            Err(RecvTimeoutError::Disconnected) => return Err(PoolError::WorkerDied(0)),
        }
    }

    let (pages, geometry_us) = geometry.expect("checked by the loop condition");
    if page_count.is_some_and(|count| count as usize != pages.len()) {
        return Err(PoolError::BadHandshake {
            worker: 0,
            reason: "geometry does not cover every page".into(),
        });
    }
    Ok(Handshake {
        pages,
        open_us,
        geometry_us,
    })
}

/// Starts worker processes and gives them what they need, and nothing more.
struct Spawner {
    config: PoolConfig,
    job: JobObject,
    /// The document, opened so that users can still rename or move it while reading.
    document: File,
    slab: Arc<PixelSlab>,
    events: Sender<Event>,
    next_instance: u64,
}

impl Spawner {
    fn new(
        config: PoolConfig,
        slab: Arc<PixelSlab>,
        events: Sender<Event>,
    ) -> Result<Self, PoolError> {
        let document = open_document(&config.document).map_err(os_error("opening the document"))?;
        let job = JobObject::create(config.process_memory_limit)
            .map_err(os_error("creating the job object"))?;
        Ok(Self {
            config,
            job,
            document,
            slab,
            events,
            next_instance: 0,
        })
    }

    fn spawn(&mut self, worker: usize, send_geometry: bool) -> Result<WorkerProcess, PoolError> {
        let mut child = Command::new(&self.config.worker_exe)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(os_error("starting a render worker"))?;
        let mut process = WorkerProcess::from_child(&mut child, self.next_instance)?;
        process.child = Some(child);
        self.next_instance += 1;

        let child = process.child.as_ref().expect("set just above");
        self.job
            .assign(child)
            .map_err(os_error("assigning a worker to the job object"))?;

        let target = child.as_handle();
        let slab_handle = self
            .slab
            .region()
            .duplicate_into(target)
            .map_err(os_error("sharing the slab with a worker"))?;
        let doc_handle = duplicate_file_read_only(&self.document, target)
            .map_err(os_error("sharing the document with a worker"))?;

        // The worker blocks on stdin until this arrives, so nothing races the handles.
        let init = HostMessage::Init(InitParams {
            protocol: PROTOCOL_VERSION,
            worker_index: worker as u32,
            send_geometry,
            slab_handle,
            slab_len: self.slab.region().len() as u64,
            doc_handle,
        });
        write_frame(&mut process.stdin, &init.encode())
            .map_err(os_error("sending the initialization message"))?;

        let stdout = process.stdout.take().expect("taken only here");
        let events = self.events.clone();
        let instance = process.instance;
        thread::Builder::new()
            .name(format!("render-reader-{worker}"))
            .spawn(move || read_worker_events(worker, instance, stdout, events))
            .map_err(os_error("spawning a reader thread"))?;
        Ok(process)
    }
}

fn open_document(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(path)
}

#[derive(Debug)]
struct WorkerProcess {
    instance: u64,
    child: Option<Child>,
    stdin: ChildStdin,
    stdout: Option<ChildStdout>,
}

impl WorkerProcess {
    fn from_child(child: &mut Child, instance: u64) -> Result<Self, PoolError> {
        let missing = || PoolError::Os {
            context: "taking a worker's pipes",
            source: io::Error::other("the pipe was not created"),
        };
        Ok(Self {
            instance,
            child: None,
            stdin: child.stdin.take().ok_or_else(missing)?,
            stdout: Some(child.stdout.take().ok_or_else(missing)?),
        })
    }

    fn kill(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
        }
    }

    fn reap(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        self.reap();
    }
}

/// Turns one worker's pipe into events. Any malformed frame ends the stream, and the
/// event loop treats that exactly like a crash.
fn read_worker_events(worker: usize, instance: u64, stdout: ChildStdout, events: Sender<Event>) {
    let mut reader = BufReader::with_capacity(64 * 1024, stdout);
    let mut pages = Vec::new();
    while let Ok(Some(frame)) = read_frame(&mut reader) {
        let event = match WorkerMessage::decode(&frame) {
            Ok(WorkerMessage::Geometry { page, size }) => {
                if page as usize != pages.len() || pages.len() >= MAX_PAGES {
                    break;
                }
                pages.push(size);
                continue;
            }
            Ok(WorkerMessage::GeometryEnd { elapsed_us, .. }) => Event::Geometry {
                pages: take(&mut pages),
                elapsed_us,
            },
            Ok(message) => Event::Worker {
                worker,
                instance,
                message,
            },
            Err(_) => break,
        };
        if events.send(event).is_err() {
            return;
        }
    }
    let _ = events.send(Event::Exited { worker, instance });
}

/// Encodes tiles as one Path A batch: `[magic, count]`, then for each tile its eight
/// announcement words followed by its pixels. Pixels land 4-byte aligned, so the
/// receiver can view them without another copy.
pub fn encode_tile_batch(slab: &PixelSlab, tiles: &[ReadyTile]) -> Vec<u8> {
    const HEADER: usize = 8;
    const WORDS: usize = musaeum_shm::ring::ENTRY_WORDS * 4;

    let mut bytes = Vec::with_capacity(HEADER + tiles.len() * (WORDS + TILE_BYTES));
    bytes.extend_from_slice(&BATCH_MAGIC.to_le_bytes());
    bytes.extend_from_slice(&(tiles.len() as u32).to_le_bytes());
    for tile in tiles {
        for word in announcement(tile).to_words() {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        let spare = &mut bytes.spare_capacity_mut()[..TILE_BYTES];
        slab.copy_slot(tile.slot, spare)
            .expect("slots come from the scheduler, which only hands out valid ones");
        let len = bytes.len();
        // SAFETY: `copy_slot` just initialized the next `TILE_BYTES` bytes.
        unsafe { bytes.set_len(len + TILE_BYTES) };
    }
    bytes
}

fn announcement(tile: &ReadyTile) -> TileAnnouncement {
    TileAnnouncement {
        slot: tile.slot,
        page: tile.tile.page,
        lod: i32::from(tile.tile.lod),
        x: u32::from(tile.tile.x),
        y: u32::from(tile.tile.y),
        epoch: tile.epoch,
        render_us: tile.render_us,
        done_us: tile.done_us,
    }
}

enum Delivery {
    Pull {
        max_batch: usize,
        pollers: VecDeque<Poller>,
    },
    Ring {
        ring: DeliveryRing,
        window: u32,
        /// Slots announced and not yet consumed, oldest first.
        announced: VecDeque<u32>,
        /// Consumer position already turned into released slots.
        acknowledged: u32,
    },
}

impl Delivery {
    fn new(mode: DeliveryMode) -> Self {
        match mode {
            DeliveryMode::Pull { max_batch } => Self::Pull {
                max_batch: max_batch.max(1),
                pollers: VecDeque::new(),
            },
            DeliveryMode::Ring { ring, window } => {
                let window = window.clamp(1, ring.capacity());
                let acknowledged = ring.consumed();
                Self::Ring {
                    ring,
                    window,
                    announced: VecDeque::new(),
                    acknowledged,
                }
            }
        }
    }

    /// Whether the event loop has to wake up by itself to look for acknowledgements.
    fn needs_ack_polling(&self) -> bool {
        matches!(self, Self::Ring { announced, .. } if !announced.is_empty())
    }
}

/// Raises the system timer resolution, so a 1 ms wait takes about 1 ms instead of up
/// to 15.6 ms. Only held while the ring is in use.
struct HighResolutionTimer(bool);

impl HighResolutionTimer {
    fn begin(wanted: bool) -> Self {
        if wanted {
            // SAFETY: no preconditions; it is balanced by `timeEndPeriod` in `drop`.
            unsafe { timeBeginPeriod(1) };
        }
        Self(wanted)
    }
}

impl Drop for HighResolutionTimer {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the `timeBeginPeriod` call in `begin`.
            unsafe { timeEndPeriod(1) };
        }
    }
}

struct EventLoop {
    receiver: Receiver<Event>,
    spawner: Spawner,
    workers: Vec<Option<WorkerProcess>>,
    restarts: Vec<u32>,
    scheduler: Scheduler,
    delivery: Delivery,
    slab: Arc<PixelSlab>,
    pages: Arc<[PageGeometry]>,
    started: Instant,
    stats: StatsCollector,
    cancel_times: HashMap<u64, Instant>,
    cancels: Vec<CancelOrder>,
    released: Vec<u32>,
}

impl EventLoop {
    fn run(mut self) {
        let _timer = HighResolutionTimer::begin(matches!(self.delivery, Delivery::Ring { .. }));
        'events: loop {
            let first = if self.delivery.needs_ack_polling() {
                match self.receiver.recv_timeout(ACK_POLL_INTERVAL) {
                    Ok(event) => Some(event),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match self.receiver.recv() {
                    Ok(event) => Some(event),
                    Err(_) => break,
                }
            };

            if let Some(event) = first
                && !self.handle(event)
            {
                break;
            }
            // Handling everything that is already queued before scheduling means a burst
            // of demand lists only ever acts on the newest one.
            while let Ok(event) = self.receiver.try_recv() {
                if !self.handle(event) {
                    break 'events;
                }
            }
            self.pump();
        }
        self.stop_workers();
    }

    /// Returns `false` when the loop should stop.
    fn handle(&mut self, event: Event) -> bool {
        match event {
            Event::Demand(encoded) => self.on_demand(&encoded),
            Event::Park(poller) => match &mut self.delivery {
                Delivery::Pull { pollers, .. } => pollers.push_back(poller),
                Delivery::Ring { .. } => poller(Vec::new()),
            },
            Event::Worker {
                worker,
                instance,
                message,
            } => {
                if self.is_current(worker, instance) {
                    self.on_worker_message(worker, message);
                }
            }
            // Only the first worker reports geometry, and that happens during startup.
            Event::Geometry { .. } => {}
            Event::Exited { worker, instance } => {
                if self.is_current(worker, instance) {
                    self.on_worker_exit(worker);
                }
            }
            Event::Stats(reply) => {
                self.stats.totals.results_discarded = self.scheduler.discarded_results();
                let _ = reply.send(self.stats.snapshot());
            }
            Event::KillWorker(worker) => {
                if let Some(Some(process)) = self.workers.get_mut(worker) {
                    process.kill();
                }
            }
            Event::Shutdown => return false,
        }
        true
    }

    fn is_current(&self, worker: usize, instance: u64) -> bool {
        matches!(self.workers.get(worker), Some(Some(process)) if process.instance == instance)
    }

    fn on_demand(&mut self, encoded: &[u8]) {
        match parse_demand(encoded, &self.pages) {
            Ok(demand) => {
                self.stats.totals.demands += 1;
                self.cancels.clear();
                self.scheduler
                    .set_clock(self.started.elapsed().as_micros() as u64);
                self.scheduler
                    .on_demand(demand.epoch, &demand.tiles, &mut self.cancels);
                self.stats.totals.cancels_requested += self.cancels.len() as u64;
                for order in &self.cancels {
                    // A sequence number always fits the cell, and the index was validated
                    // when the pool started.
                    if let Ok(cell) = self.slab.cancel_cell(order.worker) {
                        cell.store(order.job_seq, Ordering::Release);
                        self.cancel_times.insert(order.job_seq, Instant::now());
                    }
                }
            }
            Err(error) => {
                self.stats.totals.invalid_demands += 1;
                eprintln!("rejected a demand list: {error}");
            }
        }
    }

    fn on_worker_message(&mut self, worker: usize, message: WorkerMessage) {
        let report = match message {
            WorkerMessage::TileDone {
                job_seq,
                slot,
                render_us,
                load_us,
            } => {
                // Pairs with the worker's release fence: its pixel writes are visible now.
                fence(Ordering::Acquire);
                self.stats.tile_done(render_us, load_us);
                self.cancel_times.remove(&job_seq);
                let done_us = self.started.elapsed().as_micros() as u32;
                self.scheduler.on_outcome(
                    worker,
                    job_seq,
                    slot,
                    TileOutcome::Done { render_us, done_us },
                )
            }
            WorkerMessage::TileCancelled { job_seq, slot, .. } => {
                self.stats.totals.tiles_cancelled += 1;
                if let Some(requested) = self.cancel_times.remove(&job_seq) {
                    self.stats
                        .cancel_confirmed(requested.elapsed().as_micros() as u32);
                }
                self.scheduler
                    .on_outcome(worker, job_seq, slot, TileOutcome::Cancelled)
            }
            WorkerMessage::TileFailed {
                job_seq,
                slot,
                failure,
            } => {
                self.stats.totals.tiles_failed += 1;
                self.cancel_times.remove(&job_seq);
                eprintln!("worker {worker} failed job {job_seq}: {failure}");
                self.scheduler
                    .on_outcome(worker, job_seq, slot, TileOutcome::Failed)
            }
            WorkerMessage::Ready {
                protocol: PROTOCOL_VERSION,
                ..
            } => {
                self.scheduler.on_worker_ready(worker);
                Ok(())
            }
            WorkerMessage::Fatal { code, detail } => {
                eprintln!("worker {worker} is exiting: {code} (detail {detail})");
                Ok(())
            }
            // Geometry messages are consumed by the reader thread. Anything else here
            // is a worker saying something it should not.
            WorkerMessage::Ready { .. }
            | WorkerMessage::Geometry { .. }
            | WorkerMessage::GeometryEnd { .. } => {
                self.kill_for_violation(worker, "sent an unexpected message");
                Ok(())
            }
        };
        if let Err(error) = report {
            self.kill_for_violation(worker, &error.to_string());
        }
    }

    /// A worker that contradicts the host may be compromised and its slot cannot be
    /// trusted. Killing it funnels everything into the ordinary crash path.
    fn kill_for_violation(&mut self, worker: usize, reason: &str) {
        eprintln!("killing render worker {worker}: {reason}");
        self.stats.totals.protocol_violations += 1;
        if let Some(Some(process)) = self.workers.get_mut(worker) {
            process.kill();
        }
    }

    fn on_worker_exit(&mut self, worker: usize) {
        if let Some(mut process) = self.workers[worker].take() {
            process.reap();
        }
        // The process is gone, so nothing can write to its slot any more.
        self.scheduler.on_worker_lost(worker);

        if self.restarts[worker] >= MAX_WORKER_RESTARTS {
            eprintln!("render worker {worker} keeps dying; leaving it stopped");
            return;
        }
        self.restarts[worker] += 1;
        self.stats.totals.worker_restarts += 1;
        match self.spawner.spawn(worker, false) {
            // The scheduler only uses it once its `Ready` message arrives.
            Ok(process) => self.workers[worker] = Some(process),
            Err(error) => eprintln!("could not restart render worker {worker}: {error}"),
        }
    }

    /// Moves everything that can move: releases, then deliveries, then new jobs. In that
    /// order, because releases free the slots that new jobs need.
    fn pump(&mut self) {
        self.collect_releases();
        self.deliver();
        self.dispatch();
    }

    fn collect_releases(&mut self) {
        if let Delivery::Ring {
            ring,
            announced,
            acknowledged,
            ..
        } = &mut self.delivery
        {
            let consumed = ring.consumed();
            let newly = consumed.wrapping_sub(*acknowledged) as usize;
            if newly > announced.len() {
                eprintln!(
                    "the ring consumer acknowledged {newly} tiles, but only {} are out",
                    announced.len()
                );
            }
            for _ in 0..newly.min(announced.len()) {
                self.released.extend(announced.pop_front());
            }
            *acknowledged = consumed;
        }
        self.apply_releases();
    }

    fn apply_releases(&mut self) {
        self.scheduler
            .set_clock(self.started.elapsed().as_micros() as u64);
        for slot in self.released.drain(..) {
            if let Err(error) = self.scheduler.on_released(slot) {
                eprintln!("slot accounting error: {error}");
            }
        }
    }

    fn deliver(&mut self) {
        match &mut self.delivery {
            Delivery::Pull { max_batch, pollers } => {
                while !pollers.is_empty() {
                    let mut tiles = Vec::with_capacity(*max_batch);
                    while tiles.len() < *max_batch {
                        match self.scheduler.next_ready() {
                            Some(tile) => tiles.push(tile),
                            None => break,
                        }
                    }
                    if tiles.is_empty() {
                        break;
                    }
                    let copy_started = Instant::now();
                    let batch = encode_tile_batch(&self.slab, &tiles);
                    self.stats
                        .batch_copied(copy_started.elapsed().as_micros() as u32);
                    // Everything is in the batch now, so the slots can be reused at once.
                    self.released.extend(tiles.iter().map(|tile| tile.slot));

                    let poller = pollers.pop_front().expect("checked by the loop condition");
                    poller(batch);
                }
            }
            Delivery::Ring {
                ring,
                window,
                announced,
                ..
            } => {
                while (announced.len() as u32) < *window {
                    let Some(tile) = self.scheduler.next_ready() else {
                        break;
                    };
                    // The window never exceeds the capacity and everything announced is
                    // tracked, so a full ring here would mean corrupted counters.
                    if ring.publish(announcement(&tile)).is_ok() {
                        announced.push_back(tile.slot);
                    } else {
                        eprintln!("delivery ring unexpectedly full; dropping a tile");
                        self.released.push(tile.slot);
                    }
                }
            }
        }
        self.apply_releases();
    }

    fn dispatch(&mut self) {
        while let Some(order) = self.scheduler.next_dispatch() {
            let Some(process) = self.workers[order.worker].as_mut() else {
                // Cannot happen: the scheduler only uses workers that reported ready.
                self.scheduler.on_worker_lost(order.worker);
                continue;
            };
            let job = HostMessage::Render(RenderJob {
                job_seq: order.job_seq,
                slot: order.slot,
                tile: order.tile,
            });
            if write_frame(&mut process.stdin, &job.encode())
                .and_then(|()| process.stdin.flush())
                .is_err()
            {
                // The reader thread will report the exit, which recovers the slot.
                process.kill();
            }
        }
    }

    fn stop_workers(&mut self) {
        if let Delivery::Pull { pollers, .. } = &mut self.delivery {
            for poller in pollers.drain(..) {
                poller(Vec::new());
            }
        }

        for process in self.workers.iter_mut().flatten() {
            let _ = write_frame(&mut process.stdin, &HostMessage::Shutdown.encode());
        }
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        for process in self.workers.iter_mut().flatten() {
            if let Some(child) = &mut process.child {
                while Instant::now() < deadline {
                    match child.try_wait() {
                        Ok(None) => thread::sleep(Duration::from_millis(5)),
                        _ => break,
                    }
                }
            }
            process.reap();
        }
    }
}
