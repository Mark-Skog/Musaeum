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
use musaeum_shm::outline::{self, OUTLINE_PAGE, OutlineEntry};
use musaeum_shm::region::duplicate_file_read_only;
use musaeum_shm::slab::MAX_WORKERS;
use musaeum_shm::tile::{
    SCALE_ONE, THUMBNAIL_EDGE, lines_edge, lines_scale, page_pixels, thumbnail_scale, tile_grid,
};
use musaeum_shm::wire::{
    AnalyzeJob, FatalCode, HostMessage, InitParams, PROTOCOL_VERSION, RenderJob, TextJob,
    WorkerMessage, read_frame, write_frame,
};
use musaeum_shm::{
    CharLayer, DeliveryRing, PageContent, PageGeometry, PixelSlab, TILE_BYTES, TILE_SIZE,
    TextLayer, TileAnnouncement, TileKey,
};
use thiserror::Error;
use windows_sys::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

use crate::demand::parse_demand;
use crate::geometry::{Direction, DocumentHints, PageLayout};
use crate::scheduler::{CancelOrder, ReadyTile, Scheduler, SideResult, TileOutcome, Work};
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

/// Work done at the scheduler's lowest priority, when nobody is waiting for a tile.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackgroundWork {
    /// Pages to classify, in this order.
    pub analyze: Vec<u32>,
    /// Pages to render a thumbnail of, in this order.
    pub thumbnails: Vec<u32>,
    /// Pages to draw for the line detector, in this order: a gray picture of each, its longest
    /// edge [`musaeum_shm::tile::lines_edge`] pixels.
    pub lines: Vec<u32>,
}

/// One of the things background work does to a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackgroundTask {
    /// Measuring what the page is made of.
    Classify,
    /// Drawing its thumbnail.
    Thumbnail,
    /// Drawing the picture the line detector looks at.
    Lines,
}

/// What background work produced, one result at a time.
#[derive(Debug, PartialEq)]
pub enum BackgroundResult {
    /// What a page is made of.
    Content {
        /// The page.
        page: u32,
        /// The measurements.
        content: PageContent,
    },
    /// A page's thumbnail: RGBA rows, packed, at most [`THUMBNAIL_EDGE`] pixels a side.
    Thumbnail {
        /// The page.
        page: u32,
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
        /// The pixels.
        pixels: Vec<u8>,
    },
    /// The picture of a page for the line detector: gray, one byte per pixel, rows packed, the
    /// page's longest edge [`musaeum_shm::tile::lines_edge`] pixels (1500 for a page in portrait),
    /// as the page is shown (its `/Rotate` applied).
    Lines {
        /// The page.
        page: u32,
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
        /// The pixels, 0 black and 255 white.
        gray: Vec<u8>,
    },
    /// A page could not be classified, or its thumbnail or its picture drawn.
    Failed {
        /// The page.
        page: u32,
        /// What failed.
        task: BackgroundTask,
    },
    /// Everything asked for so far is done or given up.
    Finished,
}

/// Receives background results, on the pool's thread: it should hand them on, not work on them.
pub type BackgroundSink = Box<dyn FnMut(BackgroundResult) + Send + 'static>;

/// A stretch of a page asked for as a gray picture, as the OCR reads it: pixels at `scale`, of
/// the page as shown (its `/Rotate` applied), from `left`, `top`, `width` × `height` of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionRequest {
    pub page: u32,
    pub scale: u32,
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
}

/// The most pixels one region may have.
pub const MAX_REGION_PIXELS: u64 = 48 << 20;

/// A region of a page as drawn: what was asked, clamped to the page, and its gray pixels, one byte
/// each, rows packed, 0 black.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionPicture {
    pub request: RegionRequest,
    pub gray: Vec<u8>,
}

type RegionReply = Sender<Result<RegionPicture, TextError>>;

/// Why a page's text layer could not be read.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum TextError {
    /// The document has no such page.
    #[error("page {0} does not exist")]
    NoSuchPage(u32),
    /// The worker could not read it, or crashed trying.
    #[error("the text layer could not be read")]
    Failed,
    /// The worker's answer did not hold a valid text layer.
    #[error("the worker sent a malformed text layer: {0}")]
    Malformed(String),
}

/// Answers a request for a page's text layer.
pub type TextReply = Sender<Result<TextLayer, TextError>>;

/// Answers a request for a page's characters.
pub type CharsReply = Sender<Result<CharLayer, TextError>>;

/// Marks a text job that asks for a page's characters instead of its words. Pages stop far below
/// it ([`MAX_PAGES`]), and the outline's page, which has every bit set, is told apart first.
const CHARS_BIT: u32 = 1 << 30;

/// The page a character job is for, if it is one.
fn chars_page(key: u32) -> Option<u32> {
    (key != OUTLINE_PAGE && key & CHARS_BIT != 0).then_some(key & !CHARS_BIT)
}

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
    Background(BackgroundWork, BackgroundSink),
    MoreBackground {
        work: BackgroundWork,
        first: bool,
    },
    Text(u32, TextReply),
    Chars(u32, CharsReply),
    Region(RegionRequest, RegionReply),
    Stats(Sender<PoolStats>),
    Pids(Sender<Vec<Option<u32>>>),
    KillWorker(usize),
    Shutdown,
}

/// What another thread keeps of a pool to hand it more background work and ask it for a
/// page's characters, as the thread that stores what the background makes does. It does not
/// keep the pool running: once the pool stops, its requests go nowhere.
#[derive(Clone)]
pub struct PoolHandle {
    events: Sender<Event>,
}

impl std::fmt::Debug for PoolHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolHandle").finish_non_exhaustive()
    }
}

impl PoolHandle {
    /// Adds work to the background started with [`RenderPool::start_background`], for the same
    /// sink, which hears [`BackgroundResult::Finished`] again once this is done too. Without a
    /// background started, it does nothing.
    pub fn add_background(&self, work: BackgroundWork) {
        let _ = self
            .events
            .send(Event::MoreBackground { work, first: false });
    }

    /// As [`RenderPool::request_chars`].
    pub fn request_chars(&self, page: u32) -> Receiver<Result<CharLayer, TextError>> {
        let (reply, answer) = mpsc::channel();
        let _ = self.events.send(Event::Chars(page, reply));
        answer
    }

    /// As [`RenderPool::request_region`].
    pub fn request_region(
        &self,
        request: RegionRequest,
    ) -> Receiver<Result<RegionPicture, TextError>> {
        let (reply, answer) = mpsc::channel();
        let _ = self.events.send(Event::Region(request, reply));
        answer
    }
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
            background: None,
            text_waiters: HashMap::new(),
            chars_waiters: HashMap::new(),
            lines: HashMap::new(),
            regions: Vec::new(),
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

    /// Starts classifying pages and rendering thumbnails, at the lowest priority there is: only
    /// while no wanted tile waits, and never on every worker at once. Results go to `sink`, which
    /// replaces any earlier one, ending with [`BackgroundResult::Finished`].
    pub fn start_background(&self, work: BackgroundWork, sink: BackgroundSink) {
        let _ = self.events.send(Event::Background(work, sink));
    }

    /// Adds work to the background, as [`PoolHandle::add_background`], but ahead of what waits:
    /// for a page the reader is looking at. A page whose picture for the line detector is
    /// already waiting has its tiles moved ahead instead.
    pub fn add_background_first(&self, work: BackgroundWork) {
        let _ = self
            .events
            .send(Event::MoreBackground { work, first: true });
    }

    /// A handle for another thread, to add background work and ask for characters.
    pub fn handle(&self) -> PoolHandle {
        PoolHandle {
            events: self.events.clone(),
        }
    }

    /// Asks for a page's text layer, ahead of every tile. The answer arrives on the receiver; if
    /// the pool stops first, the receiver reports a disconnection.
    pub fn request_text(&self, page: u32) -> Receiver<Result<TextLayer, TextError>> {
        let (reply, answer) = mpsc::channel();
        let _ = self.events.send(Event::Text(page, reply));
        answer
    }

    /// Asks for a page's characters and their quads, ahead of every tile, as for a text layer.
    pub fn request_chars(&self, page: u32) -> Receiver<Result<CharLayer, TextError>> {
        let (reply, answer) = mpsc::channel();
        let _ = self.events.send(Event::Chars(page, reply));
        answer
    }

    /// Asks for a stretch of a page as a gray picture at a scale of its own, for the OCR: the tiles
    /// it covers are drawn ahead of the rest of the background and put together. What lies past
    /// the page is cut off; a region of no pixels or of more than [`MAX_REGION_PIXELS`] is refused.
    pub fn request_region(
        &self,
        request: RegionRequest,
    ) -> Receiver<Result<RegionPicture, TextError>> {
        let (reply, answer) = mpsc::channel();
        let _ = self.events.send(Event::Region(request, reply));
        answer
    }

    /// The document's outline, as a table of contents. It travels as the text layer of
    /// [`OUTLINE_PAGE`], and what comes back is checked against this document's pages.
    pub fn outline(&self, timeout: Duration) -> Result<Vec<OutlineEntry>, TextError> {
        let layer = self
            .request_text(OUTLINE_PAGE)
            .recv_timeout(timeout)
            .map_err(|_| TextError::Failed)??;
        Ok(outline::from_text_layer(&layer, self.pages.len() as u32))
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
            // Both sandboxed policies start on a startup token the worker has to drop.
            locked_down: self.config.policy != Policy::Unrestricted,
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
    /// Where background results go, and whether `Finished` has been sent for the work so far.
    background: Option<(BackgroundSink, bool)>,
    /// Who is waiting for each page's text layer.
    text_waiters: HashMap<u32, Vec<TextReply>>,
    chars_waiters: HashMap<u32, Vec<CharsReply>>,
    /// The pictures for the line detector being put together from their tiles, by page.
    lines: HashMap<u32, LinesAssembly>,
    /// Regions asked for, being put together from their tiles.
    regions: Vec<RegionAssembly>,
}

/// A region of a page being put together tile by tile.
struct RegionAssembly {
    request: RegionRequest,
    gray: Vec<u8>,
    /// Tiles still to come, by column and row.
    remaining: std::collections::HashSet<(u16, u16)>,
    reply: RegionReply,
}

/// The picture of a page for the line detector, being put together tile by tile.
struct LinesAssembly {
    /// The scale its tiles are drawn at: a region of the same page may ask for others.
    scale: u32,
    width: u32,
    height: u32,
    /// One byte per pixel, rows packed; white until a tile lands.
    gray: Vec<u8>,
    /// Tiles still to come.
    remaining: usize,
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
        if matches!(
            event,
            Event::Demand(_)
                | Event::Worker { .. }
                | Event::Background(..)
                | Event::Text(..)
                | Event::Chars(..)
                | Event::Region(..)
        ) {
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
            Event::Background(work, sink) => self.on_background(work, sink),
            Event::MoreBackground { work, first } => self.on_more_background(work, first),
            Event::Text(page, reply) => {
                // The outline asks for the text of a page no document has; see musaeum_shm::outline.
                if page != OUTLINE_PAGE && page as usize >= self.pages.len() {
                    let _ = reply.send(Err(TextError::NoSuchPage(page)));
                } else {
                    let waiters = self.text_waiters.entry(page).or_default();
                    if waiters.is_empty() {
                        self.scheduler.request_text(page);
                    }
                    waiters.push(reply);
                }
            }
            Event::Chars(page, reply) => {
                if page as usize >= self.pages.len() {
                    let _ = reply.send(Err(TextError::NoSuchPage(page)));
                } else {
                    let waiters = self.chars_waiters.entry(page).or_default();
                    if waiters.is_empty() {
                        self.scheduler.request_text(page | CHARS_BIT);
                    }
                    waiters.push(reply);
                }
            }
            Event::Region(request, reply) => self.on_region(request, reply),
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
                self.send_cancels();
            }
            Err(error) => {
                self.stats.totals.invalid_demands += 1;
                eprintln!("rejected a demand list: {error}");
            }
        }
    }

    /// Asks workers to stop the jobs in `self.cancels`.
    fn send_cancels(&mut self) {
        self.stats.totals.cancels_requested += self.cancels.len() as u64;
        for order in &self.cancels {
            // A sequence number always fits the cell, and the index was validated
            // when the pool started.
            if let Ok(cell) = self.slab.cancel_cell(order.worker) {
                cell.store(order.job_seq, Ordering::Release);
                self.cancel_times.insert(order.job_seq, Instant::now());
            }
        }
        self.cancels.clear();
    }

    fn on_background(&mut self, work: BackgroundWork, sink: BackgroundSink) {
        self.queue_background(work, false);
        self.background = Some((sink, false));
    }

    fn on_more_background(&mut self, work: BackgroundWork, first: bool) {
        if self.background.is_none() {
            return;
        }
        self.queue_background(work, first);
        if let Some((_, finished)) = &mut self.background {
            *finished = false;
        }
        // With nothing of it left to do (pages out of range, or already being traced), the
        // pump that follows every event tells the sink the background is finished again.
    }

    fn queue_background(&mut self, work: BackgroundWork, first: bool) {
        let pages = &self.pages;
        let analyze = work
            .analyze
            .into_iter()
            .filter(|&page| (page as usize) < pages.len())
            .map(Work::Analyze);
        let thumbnails = work.thumbnails.into_iter().filter_map(|page| {
            let size = *pages.get(page as usize)?;
            Some(Work::Thumbnail(TileKey {
                page,
                scale: thumbnail_scale(size, THUMBNAIL_EDGE),
                x: 0,
                y: 0,
            }))
        });
        // A page for the line detector is every tile of it at the detector's scale; the tiles of
        // one page come one after another, so its picture is finished before the next page's.
        let mut assemblies = Vec::new();
        let mut lines = Vec::new();
        for page in work.lines {
            let Some(&size) = pages.get(page as usize) else {
                continue;
            };
            // A page already being put together is not asked for twice: its count of tiles
            // left would no longer match. Asked for first, what of it still waits goes ahead.
            if self.lines.contains_key(&page) {
                if first {
                    self.scheduler.promote_background(
                        |work| matches!(work, Work::Lines(tile) if tile.page == page),
                    );
                }
                continue;
            }
            if assemblies.iter().any(|(queued, _)| *queued == page) {
                continue;
            }
            let scale = lines_scale(size);
            let (columns, rows) = tile_grid(size, scale);
            let edge = u64::from(lines_edge(size));
            let width = page_pixels(size.width_pt, scale).min(edge) as u32;
            let height = page_pixels(size.height_pt, scale).min(edge) as u32;
            assemblies.push((
                page,
                LinesAssembly {
                    scale,
                    width,
                    height,
                    gray: vec![255; width as usize * height as usize],
                    remaining: (columns * rows) as usize,
                },
            ));
            for y in 0..rows {
                for x in 0..columns {
                    lines.push(Work::Lines(TileKey {
                        page,
                        scale,
                        x: x as u16,
                        y: y as u16,
                    }));
                }
            }
        }
        let work = analyze.chain(thumbnails).chain(lines);
        if first {
            self.scheduler.add_background_first(work);
        } else {
            self.scheduler.add_background(work);
        }
        for (page, assembly) in assemblies {
            self.lines.insert(page, assembly);
        }
    }

    /// A region asked for: its tiles go ahead of the rest of the background.
    fn on_region(&mut self, request: RegionRequest, reply: RegionReply) {
        let Some(&size) = self.pages.get(request.page as usize) else {
            let _ = reply.send(Err(TextError::NoSuchPage(request.page)));
            return;
        };
        let scale = request
            .scale
            .clamp(musaeum_shm::tile::MIN_SCALE, SCALE_ONE * 32);
        let page_width = page_pixels(size.width_pt, scale);
        let page_height = page_pixels(size.height_pt, scale);
        let left = u64::from(request.left).min(page_width);
        let top = u64::from(request.top).min(page_height);
        let width = u64::from(request.width).min(page_width - left);
        let height = u64::from(request.height).min(page_height - top);
        if width == 0 || height == 0 || width * height > MAX_REGION_PIXELS {
            let _ = reply.send(Err(TextError::Failed));
            return;
        }
        let request = RegionRequest {
            page: request.page,
            scale,
            left: left as u32,
            top: top as u32,
            width: width as u32,
            height: height as u32,
        };
        let tile = u64::from(TILE_SIZE);
        let mut remaining = std::collections::HashSet::new();
        for y in top / tile..(top + height).div_ceil(tile) {
            for x in left / tile..(left + width).div_ceil(tile) {
                remaining.insert((x as u16, y as u16));
            }
        }
        let work: Vec<Work> = remaining
            .iter()
            .map(|&(x, y)| {
                Work::Lines(TileKey {
                    page: request.page,
                    scale,
                    x,
                    y,
                })
            })
            .collect();
        self.scheduler.add_background_first(work);
        self.regions.push(RegionAssembly {
            request,
            gray: vec![255; (width * height) as usize],
            remaining,
            reply,
        });
    }

    /// One tile of a page's picture for the line detector has landed in `slot`.
    fn on_lines_tile(&mut self, slot: u32, tile: TileKey) {
        let size = self.pages[tile.page as usize];
        let full_width = page_pixels(size.width_pt, tile.scale);
        let full_height = page_pixels(size.height_pt, tile.scale);
        let x0 = u64::from(tile.x) * u64::from(TILE_SIZE);
        let y0 = u64::from(tile.y) * u64::from(TILE_SIZE);
        let width = full_width.saturating_sub(x0).min(u64::from(TILE_SIZE)) as u32;
        let height = full_height.saturating_sub(y0).min(u64::from(TILE_SIZE)) as u32;
        let pixels = self.slab.copy_slot_rect(slot, width, height);
        self.release_side_slot(slot);
        let Ok(pixels) = pixels else {
            self.fail_lines(tile);
            return;
        };
        self.region_tile(tile, (x0, y0), (width, height), &pixels);
        let Some(assembly) = self
            .lines
            .get_mut(&tile.page)
            .filter(|assembly| assembly.scale == tile.scale)
        else {
            // Asked for again meanwhile, given up, or a region's: this tile is nobody else's.
            return;
        };
        let (x0, y0) = (x0 as usize, y0 as usize);
        let fits = x0 + width as usize <= assembly.width as usize
            && y0 + height as usize <= assembly.height as usize;
        if fits {
            let stride = assembly.width as usize;
            for row in 0..height as usize {
                let source = &pixels[row * width as usize * 4..(row + 1) * width as usize * 4];
                let target = &mut assembly.gray
                    [(y0 + row) * stride + x0..(y0 + row) * stride + x0 + width as usize];
                for (gray, px) in target.iter_mut().zip(source.as_chunks::<4>().0) {
                    // Rec. 601 weights, in integers: the sum of the weights is 256.
                    *gray =
                        ((77 * u32::from(px[0]) + 150 * u32::from(px[1]) + 29 * u32::from(px[2]))
                            >> 8) as u8;
                }
            }
        }
        assembly.remaining = assembly.remaining.saturating_sub(1);
        if assembly.remaining == 0 {
            let assembly = self.lines.remove(&tile.page).expect("just found");
            self.send_background(BackgroundResult::Lines {
                page: tile.page,
                width: assembly.width,
                height: assembly.height,
                gray: assembly.gray,
            });
        }
    }

    /// Copies a landed tile into every region waiting for it, and answers those it finishes.
    fn region_tile(&mut self, tile: TileKey, at: (u64, u64), size: (u32, u32), rgba: &[u8]) {
        if self.regions.is_empty() {
            return;
        }
        let (x0, y0) = at;
        let (width, height) = (u64::from(size.0), u64::from(size.1));
        let mut finished = Vec::new();
        for (index, region) in self.regions.iter_mut().enumerate() {
            let r = region.request;
            if r.page != tile.page
                || r.scale != tile.scale
                || !region.remaining.remove(&(tile.x, tile.y))
            {
                continue;
            }
            // The part of the tile inside the region.
            let (left, top) = (u64::from(r.left), u64::from(r.top));
            let from_x = x0.max(left);
            let to_x = (x0 + width).min(left + u64::from(r.width));
            let from_y = y0.max(top);
            let to_y = (y0 + height).min(top + u64::from(r.height));
            for y in from_y..to_y {
                let source_row = ((y - y0) * width) as usize;
                let target_row = ((y - top) * u64::from(r.width)) as usize;
                for x in from_x..to_x {
                    let px = &rgba[(source_row + (x - x0) as usize) * 4..][..4];
                    region.gray[target_row + (x - left) as usize] =
                        ((77 * u32::from(px[0]) + 150 * u32::from(px[1]) + 29 * u32::from(px[2]))
                            >> 8) as u8;
                }
            }
            if region.remaining.is_empty() {
                finished.push(index);
            }
        }
        for index in finished.into_iter().rev() {
            let region = self.regions.remove(index);
            let _ = region.reply.send(Ok(RegionPicture {
                request: region.request,
                gray: region.gray,
            }));
        }
    }

    /// A tile of a page's picture for the line detector could not be drawn: the picture is given
    /// up, once, and so is every region that needed it.
    fn fail_lines(&mut self, tile: TileKey) {
        if self
            .lines
            .get(&tile.page)
            .is_some_and(|assembly| assembly.scale == tile.scale)
        {
            self.lines.remove(&tile.page);
            self.send_background(BackgroundResult::Failed {
                page: tile.page,
                task: BackgroundTask::Lines,
            });
        }
        let mut index = 0;
        while index < self.regions.len() {
            let r = self.regions[index].request;
            if r.page == tile.page
                && r.scale == tile.scale
                && self.regions[index].remaining.contains(&(tile.x, tile.y))
            {
                let region = self.regions.remove(index);
                let _ = region.reply.send(Err(TextError::Failed));
            } else {
                index += 1;
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
                match self.scheduler.job_work(worker, job_seq) {
                    Some(Work::Thumbnail(_)) => self.stats.thumbnail_done(render_us),
                    Some(Work::Lines(_)) => self.stats.lines_tile_done(render_us),
                    _ => self.stats.tile_done(render_us, load_us),
                }
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
            WorkerMessage::PageContent {
                job_seq,
                page,
                content,
                elapsed_us,
            } => {
                self.stats.analyzed(elapsed_us);
                self.scheduler
                    .on_analysis(worker, job_seq, Some(page), Some(content))
            }
            WorkerMessage::AnalysisFailed { job_seq, failure } => {
                self.stats.totals.side_failures += 1;
                eprintln!("worker {worker} could not classify job {job_seq}: {failure}");
                self.scheduler.on_analysis(worker, job_seq, None, None)
            }
            WorkerMessage::TextDone {
                job_seq, slot, len, ..
            } => {
                // As for a tile: what the worker wrote into the slot is visible now.
                fence(Ordering::Acquire);
                self.stats.totals.texts_done += 1;
                self.scheduler.on_text(worker, job_seq, slot, len)
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
        self.deliver_side_results();
        self.scheduler.preempt_background(&mut self.cancels);
        self.send_cancels();
        self.dispatch();
    }

    /// Hands finished side work to whoever asked for it, copying it out of its slot first so the
    /// slot can be used again at once.
    fn deliver_side_results(&mut self) {
        while let Some(result) = self.scheduler.next_side_result() {
            match result {
                SideResult::Thumbnail { slot, tile } => {
                    let size = self.pages[tile.page as usize];
                    let width = (page_pixels(size.width_pt, tile.scale) as u32).min(TILE_SIZE);
                    let height = (page_pixels(size.height_pt, tile.scale) as u32).min(TILE_SIZE);
                    let pixels = self.slab.copy_slot_rect(slot, width, height);
                    self.release_side_slot(slot);
                    if let Ok(pixels) = pixels {
                        self.send_background(BackgroundResult::Thumbnail {
                            page: tile.page,
                            width,
                            height,
                            pixels,
                        });
                    }
                }
                SideResult::Lines { slot, tile } => self.on_lines_tile(slot, tile),
                SideResult::Text { slot, page, len } if chars_page(page).is_some() => {
                    let bytes = self.slab.copy_slot_prefix(slot, len as usize);
                    self.release_side_slot(slot);
                    let answer = match bytes {
                        Ok(bytes) => CharLayer::decode(&bytes)
                            .map_err(|error| TextError::Malformed(error.to_string())),
                        Err(_) => Err(TextError::Failed),
                    };
                    self.answer_chars(page & !CHARS_BIT, &answer);
                }
                SideResult::Text { slot, page, len } => {
                    let bytes = self.slab.copy_slot_prefix(slot, len as usize);
                    self.release_side_slot(slot);
                    let answer = match bytes {
                        Ok(bytes) => TextLayer::decode(&bytes)
                            .map_err(|error| TextError::Malformed(error.to_string())),
                        Err(_) => Err(TextError::Failed),
                    };
                    self.answer_text(page, &answer);
                }
                SideResult::Content { page, content } => {
                    self.send_background(BackgroundResult::Content { page, content });
                }
                SideResult::Failed(work) => match work {
                    Work::Thumbnail(tile) => self.send_background(BackgroundResult::Failed {
                        page: tile.page,
                        task: BackgroundTask::Thumbnail,
                    }),
                    Work::Lines(tile) => self.fail_lines(tile),
                    Work::Analyze(page) => self.send_background(BackgroundResult::Failed {
                        page,
                        task: BackgroundTask::Classify,
                    }),
                    Work::Text(page) => match chars_page(page) {
                        Some(page) => self.answer_chars(page, &Err(TextError::Failed)),
                        None => self.answer_text(page, &Err(TextError::Failed)),
                    },
                    Work::Tile(_) => {}
                },
            }
        }
        if !self.scheduler.has_background_work()
            && let Some((sink, finished)) = &mut self.background
            && !*finished
        {
            *finished = true;
            sink(BackgroundResult::Finished);
        }
    }

    fn release_side_slot(&mut self, slot: u32) {
        if let Err(error) = self.scheduler.release_side_slot(slot) {
            eprintln!("slot accounting error: {error}");
        }
    }

    fn send_background(&mut self, result: BackgroundResult) {
        if let Some((sink, _)) = &mut self.background {
            sink(result);
        }
    }

    fn answer_text(&mut self, page: u32, answer: &Result<TextLayer, TextError>) {
        for reply in self.text_waiters.remove(&page).unwrap_or_default() {
            let _ = reply.send(answer.clone());
        }
    }

    fn answer_chars(&mut self, page: u32, answer: &Result<CharLayer, TextError>) {
        for reply in self.chars_waiters.remove(&page).unwrap_or_default() {
            let _ = reply.send(answer.clone());
        }
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
            let job = match order.work {
                Work::Tile(tile) | Work::Thumbnail(tile) | Work::Lines(tile) => {
                    HostMessage::Render(RenderJob {
                        job_seq: order.job_seq,
                        slot: order.slot,
                        tile,
                        background: !matches!(order.work, Work::Tile(_)),
                    })
                }
                Work::Analyze(page) => HostMessage::Analyze(AnalyzeJob {
                    job_seq: order.job_seq,
                    page,
                }),
                Work::Text(key) => match chars_page(key) {
                    Some(page) => HostMessage::ExtractChars(TextJob {
                        job_seq: order.job_seq,
                        slot: order.slot,
                        page,
                    }),
                    None => HostMessage::ExtractText(TextJob {
                        job_seq: order.job_seq,
                        slot: order.slot,
                        page: key,
                    }),
                },
            };
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
