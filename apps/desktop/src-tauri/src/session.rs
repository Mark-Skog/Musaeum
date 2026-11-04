//! One open document: its render pool, its delivery path, and the commands around it.
//!
//! Commands carry low-frequency control data, where JSON is fine. Pixels and page
//! geometry never pass through here; they use [`crate::protocol`] or shared memory.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock, PoisonError, RwLock};
use std::time::{Duration, Instant};

use musaeum_core::{
    DeliveryMode, DocumentOrientation, FixedFormat, PoolConfig, PoolStats, RenderPool,
    geometry_bytes, summarize_orientation,
};
use musaeum_library::Library;
use musaeum_sandbox::Policy;
use musaeum_shm::slab::slab_len;
use musaeum_shm::{PixelSlab, ShmRegion};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, LogicalSize, Manager, State, WebviewWindow};

use crate::background::{self, Pending};
use crate::memory::MemorySampler;
use crate::options::{DeliveryFault, DeliveryPath, StartupOptions};
use crate::reading::{self, SavedReading};
use crate::webview2;

/// Tile slots in the slab: 64 MiB.
const SLOT_COUNT: u32 = 64;
/// Entries in the Path B delivery ring.
const RING_CAPACITY: u32 = 64;
/// Tiles announced to the main thread but not yet consumed. Small, so that finished tiles
/// the compositor stops wanting can still be recalled instead of being copied for nothing.
const RING_WINDOW: u32 = 16;
/// Committed memory allowed per render worker, not counting the shared slab.
const WORKER_MEMORY_LIMIT: usize = 2 << 30;
/// How long workers get to open a document.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

/// A document being read.
pub struct Session {
    /// Identifies this session in tile URLs, so stragglers cannot reach a later one.
    pub id: u32,
    /// The running pool.
    pub pool: RenderPool,
    /// The page-geometry table, ready to serve.
    pub geometry: Vec<u8>,
    /// The library's document and revision, if the file is in the library.
    pub document: Option<(i64, i64)>,
}

/// State shared by commands and the tile protocol.
pub struct AppState {
    options: StartupOptions,
    current: RwLock<Option<Arc<Session>>>,
    next_id: AtomicU32,
    /// Only runs in the memory benchmark. With this on in the others, frames stalled for up to
    /// 500 ms, probably from what reading another process's private working set costs it.
    memory: Option<MemorySampler>,
    started: Instant,
    /// Opened the first time a document is, off the main thread. Shared with the thread that keeps
    /// what background work produces.
    library: OnceLock<Option<Arc<Library>>>,
}

impl AppState {
    /// Creates the state with no session open.
    pub fn new(options: StartupOptions) -> Self {
        Self {
            memory: options
                .bench
                .as_ref()
                .filter(|bench| bench.scenario == "memory")
                .map(|_| MemorySampler::start()),
            started: Instant::now(),
            options,
            current: RwLock::new(None),
            next_id: AtomicU32::new(1),
            library: OnceLock::new(),
        }
    }

    /// The library, if there is one to remember things in.
    fn library(&self) -> Option<&Arc<Library>> {
        self.library
            .get_or_init(|| reading::open_library(self.options.library.as_deref()).map(Arc::new))
            .as_ref()
    }

    /// The open session, if it has this id.
    pub fn session(&self, id: u32) -> Option<Arc<Session>> {
        self.current
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .filter(|session| session.id == id)
            .cloned()
    }

    /// Stops the current session's pool, joining its thread. Once this returns, nothing
    /// in this process uses the slab, so WebView2 buffers behind it may be closed.
    pub fn stop_current_session(&self) {
        let session = self
            .current
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(session) = session {
            session.pool.shutdown();
        }
    }

    fn current_stats(&self) -> Option<PoolStats> {
        self.current
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|session| session.pool.stats())
    }

    /// The command-line options.
    pub fn options(&self) -> &StartupOptions {
        &self.options
    }

    /// Fails the way a runtime without shared buffers would, if that is being simulated.
    fn check_shared_buffers_allowed(&self) -> Result<(), String> {
        if self
            .options
            .delivery_faults
            .contains(&DeliveryFault::NoSharedBuffers)
        {
            return Err("this WebView2 runtime lacks shared buffers (simulated)".to_owned());
        }
        Ok(())
    }
}

/// Arguments of [`open_session`].
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSessionArgs {
    document: PathBuf,
    delivery: DeliveryPath,
    workers: usize,
}

/// What the frontend needs to know about a new session.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    id: u32,
    format: FixedFormat,
    delivery: DeliveryPath,
    page_count: usize,
    orientation: DocumentOrientation,
    slot_count: u32,
    sandboxed: bool,
    open_us: u64,
    geometry_us: u64,
    page_layout: Option<&'static str>,
    direction: Option<&'static str>,
    geometry_cached: bool,
    document_id: Option<i64>,
    reading_state: Option<SavedReading>,
    background: Pending,
}

/// Returns the command-line options.
#[tauri::command]
pub fn startup_options(state: State<'_, AppState>) -> StartupOptions {
    state.options.clone()
}

/// Why [`open_session`] failed. The frontend tries the next delivery path only when
/// `delivery_failed` is set: a corrupt document would fail the same way on all of them.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSessionError {
    delivery_failed: bool,
    message: String,
}

impl OpenSessionError {
    fn delivery(message: String) -> Self {
        Self {
            delivery_failed: true,
            message,
        }
    }

    fn other(message: String) -> Self {
        Self {
            delivery_failed: false,
            message,
        }
    }
}

/// Milliseconds since the app started, so the page can tell how long a cold start took.
#[tauri::command]
pub fn uptime_ms(state: State<'_, AppState>) -> f64 {
    state.started.elapsed().as_secs_f64() * 1000.0
}

/// Creates the small buffers that let the page test what this runtime can do with shared
/// memory. Runs off the main thread, because creating them needs the main thread free.
#[tauri::command(async)]
pub fn open_delivery_probe(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.check_shared_buffers_allowed()?;
    webview2::create_probe_buffers(&window)
}

/// Opens a document. Runs off the main thread: starting workers blocks for a while, and
/// Paths B and C need the main thread free to create their WebView2 buffers.
#[tauri::command(async)]
pub fn open_session(
    window: WebviewWindow,
    state: State<'_, AppState>,
    args: OpenSessionArgs,
) -> Result<SessionInfo, OpenSessionError> {
    stop_and_release(&window, &state);

    // Only the first bytes are read here, to choose the worker; nothing in them is interpreted.
    let format = FixedFormat::of_file(&args.document)
        .map_err(|error| {
            OpenSessionError::other(format!("reading {}: {error}", args.document.display()))
        })?
        .ok_or_else(|| OpenSessionError::other("not a PDF, CBZ, CBR or DjVu file".to_owned()))?;
    let worker_exe = std::env::current_exe()
        .map_err(|error| OpenSessionError::other(format!("locating the app: {error}")))?
        .with_file_name(format.worker_exe());
    // A benchmark measures opening a document from scratch, so it does not use the library.
    let known = match state.library() {
        Some(library) if state.options.bench.is_none() => reading::look_up(library, &args.document),
        _ => reading::Known::default(),
    };
    let config = PoolConfig {
        worker_exe,
        document: args.document.clone(),
        worker_count: args.workers,
        process_memory_limit: Some(WORKER_MEMORY_LIMIT),
        policy: if state.options.no_sandbox {
            Policy::Unrestricted
        } else {
            format.policy()
        },
        startup_timeout: STARTUP_TIMEOUT,
        known_geometry: known.geometry.as_ref().map(reading::known_geometry),
    };

    let (slab, delivery) = match args.delivery {
        DeliveryPath::A => {
            let region = ShmRegion::create(slab_len(SLOT_COUNT)).map_err(|error| {
                OpenSessionError::other(format!("allocating the slab: {error}"))
            })?;
            let slab = PixelSlab::init(region, SLOT_COUNT)
                .map_err(|error| OpenSessionError::other(error.to_string()))?;
            (
                slab,
                DeliveryMode::Pull {
                    max_batch: state.options.pull_batch,
                },
            )
        }
        // To the host, Paths B and C are the same: tiles are announced through the ring.
        // They only differ in which thread of the WebView reads them.
        DeliveryPath::B | DeliveryPath::C => {
            state
                .check_shared_buffers_allowed()
                .map_err(OpenSessionError::delivery)?;
            let buffers = webview2::create_ring_buffers(&window, SLOT_COUNT, RING_CAPACITY)
                .map_err(|message| {
                    webview2::release_buffers(&window);
                    OpenSessionError::delivery(message)
                })?;
            (
                buffers.slab,
                DeliveryMode::Ring {
                    ring: buffers.ring,
                    window: RING_WINDOW,
                },
            )
        }
    };

    let pool = RenderPool::start(config, Arc::new(slab), delivery).map_err(|error| {
        webview2::release_buffers(&window);
        OpenSessionError::other(error.to_string())
    })?;

    let hints = pool.hints();
    let geometry_cached = !pool.measured_geometry();
    if let (Some(library), Some(hash), false) = (
        state.library(),
        known.file.and_then(|file| file.blake3),
        geometry_cached,
    ) {
        // Measured without drawing anything; next time this file opens with its layout known.
        if let Err(error) =
            library.store_page_geometry(&hash, &reading::measured(pool.pages(), hints))
        {
            eprintln!("could not keep the page sizes: {error}");
        }
    }

    // Classes and thumbnails are kept by the file's bytes, so only a file the library has hashed
    // gets them; a benchmark leaves them alone, like everything else the library keeps.
    let background = match (state.library(), known.file.and_then(|file| file.blake3)) {
        (Some(library), Some(hash)) if state.options.bench.is_none() => {
            background::start(&pool, Arc::clone(library), hash)
        }
        _ => Pending::default(),
    };

    let stats = pool.stats().unwrap_or_default();
    let session = Arc::new(Session {
        id: state.next_id.fetch_add(1, Ordering::Relaxed),
        geometry: geometry_bytes(pool.pages()),
        document: known.file.map(|file| (file.document, file.revision)),
        pool,
    });
    let info = SessionInfo {
        id: session.id,
        format,
        delivery: args.delivery,
        page_count: session.pool.pages().len(),
        orientation: summarize_orientation(session.pool.pages()),
        slot_count: session.pool.slab().slot_count(),
        sandboxed: !state.options.no_sandbox,
        open_us: stats.open_us,
        geometry_us: stats.geometry_us,
        page_layout: hints.page_layout.map(|layout| layout.as_str()),
        direction: hints.direction.map(|direction| direction.as_str()),
        geometry_cached,
        document_id: known.file.map(|file| file.document),
        reading_state: known.reading,
        background,
    };
    *state
        .current
        .write()
        .unwrap_or_else(PoisonError::into_inner) = Some(session);
    Ok(info)
}

fn stop_and_release(window: &WebviewWindow, state: &AppState) {
    state.stop_current_session();
    webview2::release_buffers(window);
}

/// Remembers where the reader is in the open document. The page can only save for the document it
/// has open, on the revision it is reading.
#[tauri::command(async)]
pub fn save_reading_state(
    state: State<'_, AppState>,
    document: i64,
    reading: SavedReading,
) -> Result<(), String> {
    let current = state
        .current
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|session| session.document);
    let Some((open, revision)) = current.filter(|(open, _)| *open == document) else {
        return Err(format!("document {document} is not the one open"));
    };
    let library = state
        .library()
        .ok_or("there is no library to remember this in")?;
    library
        .save_reading_state(open, &reading.into_state(revision))
        .map_err(|error| error.to_string())
}

/// Closes the current document, if any.
#[tauri::command(async)]
pub fn close_session(window: WebviewWindow, state: State<'_, AppState>) {
    stop_and_release(&window, &state);
}

/// Writes `value` into every probe buffer, and returns what scripts wrote back into them.
/// Together with `probe.ts`, this shows whether a transferred buffer is still live memory.
#[tauri::command(async)]
pub fn probe_exchange(value: u32) -> Vec<u32> {
    webview2::probe_exchange(value)
}

/// Returns the render pool's measurements.
#[tauri::command(async)]
pub fn pool_stats(state: State<'_, AppState>) -> Option<PoolStats> {
    state.current_stats()
}

/// Writes the benchmark report and exits. `frontend` is the JSON text from the WebView.
#[tauri::command(async)]
pub fn bench_finish(
    app: AppHandle,
    state: State<'_, AppState>,
    frontend: String,
) -> Result<(), String> {
    let bench = state
        .options
        .bench
        .as_ref()
        .ok_or("not running as a benchmark")?;
    let frontend: serde_json::Value =
        serde_json::from_str(&frontend).map_err(|error| format!("bad report: {error}"))?;
    let report = serde_json::json!({
        "options": state.options,
        "pool": state.current_stats(),
        "memory": state.memory.as_ref().map(MemorySampler::report),
        "frontend": frontend,
    });
    let text = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
    write_report(&bench.out, &text)?;
    app.exit(0);
    Ok(())
}

/// Starts counting what follows under another phase of the benchmark.
#[tauri::command(async)]
pub fn bench_mark_phase(state: State<'_, AppState>, name: String) {
    if let Some(memory) = &state.memory {
        memory.mark(&name);
    }
}

/// Resizes the window, for the benchmark of how fast a book lays itself out again. Reachable
/// from what the reader sends, so it does nothing outside a benchmark.
#[tauri::command(async)]
pub fn bench_resize_window(
    window: WebviewWindow,
    state: State<'_, AppState>,
    width: u32,
    height: u32,
) -> Result<(), String> {
    if state.options.bench.is_none() {
        return Err("not running as a benchmark".to_owned());
    }
    window
        .set_size(LogicalSize::new(f64::from(width), f64::from(height)))
        .map_err(|error| error.to_string())
}

/// Records why a benchmark could not run, then exits with an error code.
#[tauri::command(async)]
pub fn bench_fail(app: AppHandle, state: State<'_, AppState>, message: String) {
    // Ending the app is only for a benchmark, whose harness is waiting for an exit code.
    let Some(bench) = &state.options.bench else {
        eprintln!("ignoring a benchmark failure outside a benchmark: {message}");
        return;
    };
    let text = serde_json::json!({ "error": message }).to_string();
    if let Err(error) = write_report(&bench.out, &text) {
        eprintln!("{error}");
    }
    eprintln!("benchmark failed: {message}");
    app.exit(1);
}

fn write_report(path: &PathBuf, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("creating {}: {error}", parent.display()))?;
    }
    std::fs::write(path, text).map_err(|error| format!("writing {}: {error}", path.display()))
}

/// Used by `Manager::state` callers that only have an app handle.
pub fn app_state(app: &AppHandle) -> State<'_, AppState> {
    app.state::<AppState>()
}
