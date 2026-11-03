//! The render pool: worker processes, their pipes, and the event loop around
//! the [`Scheduler`].
//!
//! One thread owns all the state. Each worker gets a reader thread that turns its pipe
//! into events, so the event loop never blocks on a slow or stuck worker. The only
//! things it writes to are small messages on pipes whose peers have at most one job at
//! a time, and atomics in shared memory.

use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::mem::take;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsHandle;
use std::path::{Path, PathBuf};
use std::sync::atomic::{Ordering, fence};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use musaeum_sandbox::{Policy, SandboxedChild, SpawnConfig};
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

use crate::demand::parse_demand;
use crate::geometry::{Direction, DocumentHints, PageLayout};
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
/// How long the pool waits with nothing to do before its workers give memory back. Long enough
/// that it only happens to someone who stopped to read.
const TRIM_AFTER: Duration = Duration::from_secs(2);
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
    /// How much the workers may do. Anything but `Lockdown` is for comparing against it.
    pub policy: Policy,
    /// How long to wait for the workers to report in.
    pub startup_timeout: Duration,
    /// The document's pages and hints, if they were measured before, so that no worker reads them
    /// again. If the document turns out to have another number of pages, they are measured after
    /// all: a damaged cache costs time, never a document.
    pub known_geometry: Option<(Vec<PageGeometry>, DocumentHints)>,
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
        hints: DocumentHints,
        elapsed_us: u64,
    },
    Exited {
        worker: usize,
        instance: u64,
    },
    Stats(Sender<PoolStats>),
    Pids(Sender<Vec<Option<u32>>>),
    KillWorker(usize),
    Shutdown,
}

/// A running pool. Dropping it stops the workers and releases the slab's writers.
pub struct RenderPool {
    events: Sender<Event>,
    thread: Mutex<Option<JoinHandle<()>>>,
    pages: Arc<[PageGeometry]>,
    hints: DocumentHints,
    measured_geometry: bool,
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
        let known_geometry = config.known_geometry.clone();
        let mut spawner = Spawner::new(config, Arc::clone(&slab), events.clone())?;

        let mut workers = Vec::with_capacity(worker_count);
        for worker in 0..worker_count {
            let measures = worker == 0 && known_geometry.is_none();
            workers.push(Some(spawner.spawn(worker, measures)?));
        }

        let started = Instant::now();
        let handshake = wait_until_ready(&receiver, &mut workers, startup_timeout, known_geometry)?;
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
            lost_at: vec![None; worker_count],
            scheduler,
            delivery: Delivery::new(delivery),
            slab: Arc::clone(&slab),
            pages: Arc::clone(&pages),
            started,
            stats,
            cancel_times: HashMap::new(),
            cancels: Vec::new(),
            released: Vec::new(),
            trimmed: false,
        };
        let thread = thread::Builder::new()
            .name("render-pool".into())
            .spawn(move || event_loop.run())
            .map_err(os_error("spawning the pool thread"))?;

        Ok(Self {
            events,
            thread: Mutex::new(Some(thread)),
            pages,
            hints: handshake.hints,
            measured_geometry: handshake.measured,
            slab,
            started,
        })
    }

    /// Whether a worker measured the pages, rather than them being known beforehand.
    pub fn measured_geometry(&self) -> bool {
        self.measured_geometry
    }

    /// Page sizes, in page order.
    pub fn pages(&self) -> &[PageGeometry] {
        &self.pages
    }

    /// What the document's catalog asks of a reader.
    pub fn hints(&self) -> DocumentHints {
        self.hints
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

    /// The process ID of each worker, or `None` while one is being replaced. For killing one from
    /// outside, the way a crash or the task manager would.
    pub fn worker_pids(&self) -> Vec<Option<u32>> {
        let (sender, receiver) = mpsc::channel();
        if self.events.send(Event::Pids(sender)).is_err() {
            return Vec::new();
        }
        receiver.recv().unwrap_or_default()
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
    hints: DocumentHints,
    open_us: u64,
    geometry_us: u64,
    measured: bool,
}

fn wait_until_ready(
    receiver: &Receiver<Event>,
    workers: &mut [Option<WorkerProcess>],
    timeout: Duration,
    known_geometry: Option<(Vec<PageGeometry>, DocumentHints)>,
) -> Result<Handshake, PoolError> {
    let deadline = Instant::now() + timeout;
    let mut ready = vec![false; workers.len()];
    let mut page_count = None;
    let mut open_us = 0;
    let mut measured = known_geometry.is_none();
    let mut geometry = known_geometry.map(|(pages, hints)| (pages, hints, 0));

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
                if !measured
                    && geometry
                        .as_ref()
                        .is_some_and(|known| known.0.len() != pages as usize)
                {
                    // Not the document that was measured: this worker measures it after all.
                    eprintln!("the known geometry does not fit this document; measuring it");
                    measured = true;
                    geometry = None;
                    if let Some(Some(process)) = workers.get_mut(worker) {
                        write_frame(&mut process.stdin, &HostMessage::SendGeometry.encode())
                            .and_then(|()| process.stdin.flush())
                            .map_err(os_error("asking a worker for the geometry"))?;
                    }
                }
                if let Some(Some(process)) = workers.get(worker) {
                    process
                        .check_locked_down()
                        .map_err(|reason| PoolError::BadHandshake { worker, reason })?;
                }
                ready[worker] = true;
            }
            Ok(Event::Worker {
                message: WorkerMessage::Fatal { code, detail },
                ..
            }) => return Err(PoolError::WorkerFatal { code, detail }),
            Ok(Event::Geometry {
                pages,
                hints,
                elapsed_us,
            }) => geometry = Some((pages, hints, elapsed_us)),
            Ok(Event::Exited { worker, .. }) => return Err(PoolError::WorkerDied(worker)),
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => return Err(PoolError::StartupTimeout(timeout)),
            Err(RecvTimeoutError::Disconnected) => return Err(PoolError::WorkerDied(0)),
        }
    }

    let (pages, hints, geometry_us) = geometry.expect("checked by the loop condition");
    if page_count.is_some_and(|count| count as usize != pages.len()) {
        return Err(PoolError::BadHandshake {
            worker: 0,
            reason: "geometry does not cover every page".into(),
        });
    }
    Ok(Handshake {
        pages,
        hints,
        open_us,
        geometry_us,
        measured,
    })
}

/// Starts worker processes and gives them what they need, and nothing more.
struct Spawner {
    config: PoolConfig,
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
        Ok(Self {
            config,
            document,
            slab,
            events,
            next_instance: 0,
        })
    }

    fn spawn(&mut self, worker: usize, send_geometry: bool) -> Result<WorkerProcess, PoolError> {
        let mut child = musaeum_sandbox::spawn(SpawnConfig {
            exe: &self.config.worker_exe,
            args: &[],
            policy: self.config.policy,
            memory_limit: self.config.process_memory_limit,
        })
        .map_err(os_error("starting a render worker"))?;
        let missing = || PoolError::Os {
            context: "taking a worker's pipes",
            source: io::Error::other("the pipe was not created"),
        };
        let mut stdin = child.stdin.take().ok_or_else(missing)?;
        let stdout = child.stdout.take().ok_or_else(missing)?;
        let stderr = child.stderr.take().ok_or_else(missing)?;
        let instance = self.next_instance;
        self.next_instance += 1;

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
        write_frame(&mut stdin, &init.encode())
            .map_err(os_error("sending the initialization message"))?;

        let events = self.events.clone();
        thread::Builder::new()
            .name(format!("render-reader-{worker}"))
            .spawn(move || read_worker_events(worker, instance, stdout, events))
            .map_err(os_error("spawning a reader thread"))?;
        thread::Builder::new()
            .name(format!("render-stderr-{worker}"))
            .spawn(move || forward_worker_log(worker, stderr))
            .map_err(os_error("spawning a log thread"))?;
        Ok(WorkerProcess {
            instance,
            child,
            stdin,
            locked_down: self.config.policy == Policy::Lockdown,
        })
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
    child: SandboxedChild,
    stdin: File,
    locked_down: bool,
}

impl WorkerProcess {
    fn kill(&self) {
        self.child.kill();
    }

    fn reap(&self) {
        self.child.kill();
        let _ = self.child.wait();
    }

    /// A worker must have dropped its startup token before it announces itself, since by
    /// then it has parsed the document. Still holding it would mean a bug in the worker, and
    /// that everything the sandbox promises is false.
    fn check_locked_down(&self) -> Result<(), String> {
        if !self.locked_down {
            return Ok(());
        }
        match self.child.is_impersonating() {
            Ok(false) => Ok(()),
            Ok(true) => Err("it still holds its startup token".to_owned()),
            Err(error) => Err(format!("could not check its token: {error}")),
        }
    }
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        self.reap();
    }
}

/// Passes a worker's log on to this process's stderr. The pipe has to keep being drained even
/// if nobody can see the result, or a worker writing a lot would block forever.
fn forward_worker_log(worker: usize, mut stderr: File) {
    let mut buffer = [0u8; 4096];
    while let Ok(read @ 1..) = stderr.read(&mut buffer) {
        let text = String::from_utf8_lossy(&buffer[..read]);
        let _ = writeln!(io::stderr(), "[render worker {worker}] {}", text.trim_end());
    }
}

/// Turns one worker's pipe into events. Any malformed frame ends the stream, and the
/// event loop treats that exactly like a crash.
fn read_worker_events(worker: usize, instance: u64, stdout: File, events: Sender<Event>) {
    let mut reader = BufReader::with_capacity(64 * 1024, stdout);
    let mut pages = Vec::new();
    let mut hints = DocumentHints::default();
    while let Ok(Some(frame)) = read_frame(&mut reader) {
        let event = match WorkerMessage::decode(&frame) {
            Ok(WorkerMessage::Geometry { page, size }) => {
                if page as usize != pages.len() || pages.len() >= MAX_PAGES {
                    break;
                }
                pages.push(size);
                continue;
            }
            Ok(WorkerMessage::Hints {
                page_layout,
                direction,
            }) => {
                hints = DocumentHints {
                    page_layout: PageLayout::from_code(page_layout),
                    direction: Direction::from_code(direction),
                };
                continue;
            }
            Ok(WorkerMessage::GeometryEnd { elapsed_us, .. }) => Event::Geometry {
                pages: take(&mut pages),
                hints: take(&mut hints),
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
        scale: tile.tile.scale,
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
    /// When each worker being replaced was found dead, for measuring how long recovery takes.
    lost_at: Vec<Option<Instant>>,
    scheduler: Scheduler,
    delivery: Delivery,
    slab: Arc<PixelSlab>,
    pages: Arc<[PageGeometry]>,
    started: Instant,
    stats: StatsCollector,
    cancel_times: HashMap<u64, Instant>,
    cancels: Vec<CancelOrder>,
    released: Vec<u32>,
    /// Whether the workers have already been told to give memory back since the last work.
    trimmed: bool,
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
            } else if self.trimmed {
                match self.receiver.recv() {
                    Ok(event) => Some(event),
                    Err(_) => break,
                }
            } else {
                match self.receiver.recv_timeout(TRIM_AFTER) {
                    Ok(event) => Some(event),
                    Err(RecvTimeoutError::Timeout) => {
                        self.trim_if_idle();
                        None
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
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

    fn trim_if_idle(&mut self) {
        if self.scheduler.has_outstanding_work() {
            return;
        }
        self.trimmed = true;
        for process in self.workers.iter_mut().flatten() {
            let frame = HostMessage::Trim.encode();
            if write_frame(&mut process.stdin, &frame)
                .and_then(|()| process.stdin.flush())
                .is_err()
            {
                // The reader thread will report the exit.
                process.kill();
            }
        }
    }

    /// Returns `false` when the loop should stop.
    fn handle(&mut self, event: Event) -> bool {
        // Only work counts. A poll of the statistics is not a reason to keep memory.
        if matches!(event, Event::Demand(_) | Event::Worker { .. }) {
            self.trimmed = false;
        }
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
            Event::Pids(reply) => {
                let pids = self
                    .workers
                    .iter()
                    .map(|process| process.as_ref().map(|process| process.child.id()))
                    .collect();
                let _ = reply.send(pids);
            }
            Event::KillWorker(worker) => {
                if let Some(Some(process)) = self.workers.get(worker) {
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
                let check = self.workers[worker]
                    .as_ref()
                    .map_or(Ok(()), WorkerProcess::check_locked_down);
                match check {
                    Ok(()) => {
                        self.scheduler.on_worker_ready(worker);
                        if let Some(lost) = self.lost_at[worker].take() {
                            self.stats.recovered(lost.elapsed().as_micros() as u32);
                        }
                    }
                    Err(reason) => self.kill_for_violation(worker, &reason),
                }
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
            | WorkerMessage::Hints { .. }
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
        if let Some(Some(process)) = self.workers.get(worker) {
            process.kill();
        }
    }

    fn on_worker_exit(&mut self, worker: usize) {
        self.lost_at[worker] = Some(Instant::now());
        // Dropping it reaps the process.
        self.workers[worker] = None;
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
        for process in self.workers.iter().flatten() {
            while Instant::now() < deadline {
                match process.child.try_wait() {
                    Ok(None) => thread::sleep(Duration::from_millis(5)),
                    _ => break,
                }
            }
            process.reap();
        }
    }
}
