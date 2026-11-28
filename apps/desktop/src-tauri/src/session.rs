//! One open document: its render pool, its delivery path, and the commands around it.
//!
//! Commands carry low-frequency control data, where JSON is fine. Pixels and page
//! geometry never pass through here; they use [`crate::protocol`] or shared memory.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use musaeum_core::{
    DeliveryMode, DocumentOrientation, FixedFormat, PoolConfig, PoolStats, RenderPool,
    geometry_bytes, summarize_orientation,
};
use musaeum_i18n::tr;
use musaeum_library::{Hash, IngestClient, Library};
use musaeum_sandbox::Policy;
use musaeum_shm::slab::slab_len;
use musaeum_shm::{PixelSlab, ShmRegion};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, LogicalSize, Manager, State, WebviewWindow};

use crate::background::{self, Pending};
use crate::memory::MemorySampler;
use crate::ocr::Ocr;
use crate::options::{DeliveryFault, DeliveryPath, StartupOptions};
use crate::preferences::PreferencesState;
use crate::reading::{self, SavedReading};
use crate::shelf::Shelf;
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
/// The most render workers one document gets. The page asks for its number, and a page is not
/// trusted to keep it sensible.
const MAX_WORKERS: usize = 16;
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
    /// The file's exact bytes, as the library hashed them, which is what the cache keys by.
    pub blake3: Option<Hash>,
    /// What kind of document it is.
    pub format: FixedFormat,
    /// The file, as it was opened: only ever read.
    pub path: PathBuf,
}

/// State shared by commands and the tile protocol.
pub struct AppState {
    options: StartupOptions,
    /// Serializes opening and closing sessions so concurrent IPC calls never overlap worker
    /// startup, session replacement, or WebView2 shared-buffer release.
    lifecycle: Mutex<()>,
    /// Serializes delivery-probe buffer creation against concurrent probes.
    probe_lock: Mutex<()>,
    current: RwLock<Option<Arc<Session>>>,
    next_id: AtomicU32,
    /// Only runs in the memory benchmark. With this on in the others, frames stalled for up to
    /// 500 ms, probably from what reading another process's private working set costs it.
    memory: Option<MemorySampler>,
    started: Instant,
    /// The library, and what keeps it current.
    pub shelf: Shelf,
    /// The ingest worker that normalizes flowing books as they are opened, kept between books.
    pub flow_client: Mutex<Option<IngestClient>>,
    /// The OCR worker that reads the text under marks on scans, kept while a document is open.
    pub ocr: Ocr,
    /// The person's preferences.
    pub preferences: PreferencesState,
}

/// Takes `current` out of `slot` only when `expected` is `None` or matches the active id.
fn take_matching_session<T>(
    slot: &mut Option<T>,
    expected: Option<u32>,
    id_of: impl FnOnce(&T) -> u32,
) -> Option<T> {
    match (slot.as_ref(), expected) {
        (None, _) => None,
        (Some(active), Some(wanted)) if id_of(active) != wanted => None,
        (Some(_), _) => slot.take(),
    }
}

impl AppState {
    /// Whether the app runs under a test (`--e2e`), which has commands of its own.
    pub fn is_e2e(&self) -> bool {
        self.options.e2e
    }

    /// Creates the state with no session open.
    pub fn new(options: StartupOptions) -> Self {
        let preferences = PreferencesState::open(options.library.as_deref());
        let ocr = Ocr::default();
        ocr.choose_languages(&preferences.current().ocr_languages);
        Self {
            memory: options
                .bench
                .as_ref()
                .filter(|bench| bench.scenario == "memory")
                .map(|_| MemorySampler::start()),
            started: Instant::now(),
            shelf: Shelf::new(options.library.clone(), options.bench.is_some()),
            flow_client: Mutex::new(None),
            ocr,
            preferences,
            options,
            lifecycle: Mutex::new(()),
            probe_lock: Mutex::new(()),
            current: RwLock::new(None),
            next_id: AtomicU32::new(1),
        }
    }

    /// The library, if there is one to remember things in.
    pub fn library(&self) -> Option<Arc<Library>> {
        self.shelf.library()
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

    /// The open session, whichever it is.
    pub fn current_session(&self) -> Option<Arc<Session>> {
        self.current
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Stops the open session if its id matches `expected` (`None` stops whichever is open),
    /// returning the stopped session's id. Caller must hold [`Self::lifecycle`].
    fn stop_session_matching(&self, expected: Option<u32>) -> Option<u32> {
        let session = {
            let mut guard = self.current.write().unwrap_or_else(PoisonError::into_inner);
            take_matching_session(&mut guard, expected, |session| session.id)
        };
        let stopped = session.map(|session| {
            let id = session.id;
            session.pool.shutdown();
            id
        });
        if stopped.is_some() {
            // Its worker holds the models in memory; the next document starts another if it needs one.
            self.ocr.release();
        }
        stopped
    }

    /// Stops the current session's pool, joining its thread.
    pub fn stop_current_session(&self) {
        let _lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let _ = self.stop_session_matching(None);
    }

    /// Stops the current session and releases its WebView2 shared buffers.
    pub fn stop_and_release(&self, window: &WebviewWindow) {
        let _lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let _ = self.stop_session_matching(None);
        webview2::release_buffers(window);
    }

    /// Drops any idle flowing-book ingest worker so 0 helper processes linger when on Home or in
    /// a fixed-layout tab.
    pub fn release_flow_client(&self) {
        let client = self
            .flow_client
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        drop(client);
    }

    /// Tears down any active fixed-layout session and its WebView2 buffers before a flowing book
    /// opens, preserving the hard ceiling of at most 1 active session on the backend.
    pub fn stop_fixed_for_flow(
        &self,
        window: &WebviewWindow,
        cache: &crate::annotations::CharCache,
    ) {
        let _lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(previous_id) = self.stop_session_matching(None) {
            cache.remove_session(previous_id);
            webview2::release_session_buffers(window, previous_id);
        }
        webview2::release_probe_buffers(window);
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

    /// The sandbox the OCR worker runs in: the strictest, unless the command line turned it off.
    pub fn ocr_policy(&self) -> Policy {
        if self.options.no_sandbox {
            Policy::Unrestricted
        } else {
            Policy::LockdownParser
        }
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

/// Coordinates graceful window shutdown so the active tab can flush its reading position and close
/// its session before the WebView is torn down.
#[derive(Default)]
pub struct CloseGuard {
    intercepted: AtomicBool,
    confirmed: AtomicBool,
}

impl CloseGuard {
    /// Marks the first `CloseRequested` event as intercepted so the frontend can flush; returns
    /// `false` if already intercepted or confirmed.
    pub fn intercept_once(&self) -> bool {
        !self.confirmed.load(Ordering::Acquire) && !self.intercepted.swap(true, Ordering::AcqRel)
    }

    /// Marks window close as confirmed so the subsequent `window.close()` proceeds immediately.
    pub fn confirm(&self) -> bool {
        !self.confirmed.swap(true, Ordering::AcqRel)
    }

    /// Whether shutdown has been confirmed.
    #[cfg(test)]
    pub fn is_confirmed(&self) -> bool {
        self.confirmed.load(Ordering::Acquire)
    }
}

/// Arguments of [`open_session`].
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSessionArgs {
    document: PathBuf,
    #[serde(default)]
    document_id: Option<i64>,
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
    let _probe = state
        .probe_lock
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    state.check_shared_buffers_allowed()?;
    webview2::create_probe_buffers(&window)
}

/// Opens a document. Runs off the main thread: starting workers blocks for a while, and
/// Paths B and C need the main thread free to create their WebView2 buffers.
#[tauri::command(async)]
pub fn open_session(
    window: WebviewWindow,
    state: State<'_, AppState>,
    books: State<'_, crate::book::BookState>,
    cache: State<'_, crate::annotations::CharCache>,
    args: OpenSessionArgs,
) -> Result<SessionInfo, OpenSessionError> {
    let _lifecycle = state
        .lifecycle
        .lock()
        .unwrap_or_else(PoisonError::into_inner);

    // A page can name any path, so the host opens only what it shows anyway: a book of the
    // library, or the document named on the command line. A test or a benchmark opens what it
    // likes. Checked before anything is read, and before the open book is closed.
    // A benchmark measures opening a document from scratch, so it does not use the library.
    let known = match state.library() {
        Some(library) if state.options.bench.is_none() => {
            reading::look_up(&library, &args.document)
        }
        _ => reading::Known::default(),
    };
    let named = state.options.document.as_deref() == Some(args.document.as_path());
    if known.file.is_none() && !named && !state.is_e2e() && state.options.bench.is_none() {
        if let (Some(doc_id), Some(library)) = (args.document_id, state.library()) {
            match library.book_file(doc_id) {
                Ok(None) => return Err(OpenSessionError::other(tr!(BookFileGone))),
                Ok(Some(file)) if file.path == args.document && !file.path.exists() => {
                    return Err(OpenSessionError::other(tr!(BookFileGone)));
                }
                _ => {}
            }
        }
        return Err(OpenSessionError::other(format!(
            "{} is neither a book of the library nor the document the app was started with",
            args.document.display()
        )));
    }

    if let Some(previous_id) = state.stop_session_matching(None) {
        cache.remove_session(previous_id);
        webview2::release_session_buffers(&window, previous_id);
    }
    webview2::release_probe_buffers(&window);
    crate::book::close(&books);
    state.release_flow_client();
    // A book open in the reader: the full-text indexer gives way.
    state.shelf.set_reading(true);

    let reset_reading_on_err = |state: &AppState, books: &crate::book::BookState| {
        if !crate::book::has_open(books) {
            state.shelf.set_reading(false);
        }
    };

    // Only the first bytes are read here, to choose the worker; nothing in them is interpreted.
    let format = FixedFormat::of_file(&args.document)
        .map_err(|error| {
            reset_reading_on_err(&state, &books);
            if error.kind() == std::io::ErrorKind::NotFound {
                OpenSessionError::other(tr!(BookFileGone))
            } else {
                OpenSessionError::other(tr!(BookNotRead, error = error))
            }
        })?
        .ok_or_else(|| {
            reset_reading_on_err(&state, &books);
            OpenSessionError::other("not a PDF, CBZ, CBR or DjVu file".to_owned())
        })?;
    let worker_exe = std::env::current_exe()
        .map_err(|error| {
            reset_reading_on_err(&state, &books);
            OpenSessionError::other(format!("locating the app: {error}"))
        })?
        .with_file_name(format.worker_exe());
    let config = PoolConfig {
        worker_exe,
        document: args.document.clone(),
        worker_count: args.workers.clamp(1, MAX_WORKERS),
        process_memory_limit: Some(WORKER_MEMORY_LIMIT),
        policy: if state.options.no_sandbox {
            Policy::Unrestricted
        } else {
            format.policy()
        },
        startup_timeout: STARTUP_TIMEOUT,
        known_geometry: known.geometry.as_ref().map(reading::known_geometry),
    };

    let session_id = state.next_id.fetch_add(1, Ordering::Relaxed);
    let (slab, delivery) = match args.delivery {
        DeliveryPath::A => {
            let region = ShmRegion::create(slab_len(SLOT_COUNT)).map_err(|error| {
                reset_reading_on_err(&state, &books);
                OpenSessionError::other(format!("allocating the slab: {error}"))
            })?;
            let slab = PixelSlab::init(region, SLOT_COUNT).map_err(|error| {
                reset_reading_on_err(&state, &books);
                OpenSessionError::other(error.to_string())
            })?;
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
            state.check_shared_buffers_allowed().map_err(|message| {
                reset_reading_on_err(&state, &books);
                OpenSessionError::delivery(message)
            })?;
            let buffers =
                webview2::create_ring_buffers(&window, session_id, SLOT_COUNT, RING_CAPACITY)
                    .map_err(|message| {
                        webview2::release_session_buffers(&window, session_id);
                        reset_reading_on_err(&state, &books);
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
        webview2::release_session_buffers(&window, session_id);
        reset_reading_on_err(&state, &books);
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
            background::start(&pool, library, hash)
        }
        _ => Pending::default(),
    };

    let stats = pool.stats().unwrap_or_default();
    let session = Arc::new(Session {
        id: session_id,
        geometry: geometry_bytes(pool.pages()),
        document: known.file.map(|file| (file.document, file.revision)),
        blake3: known.file.and_then(|file| file.blake3),
        format,
        path: args.document.clone(),
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

/// An entry of a document's table of contents.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineItem {
    label: String,
    /// The zero-based page it leads to, if it leads anywhere.
    page: Option<u32>,
    /// 0 for the top level.
    level: u16,
}

/// The open document's table of contents, from its outline: a PDF's bookmarks. Comics have none,
/// and DjVu's outline is not read yet.
#[tauri::command(async)]
pub fn document_outline(
    state: State<'_, AppState>,
    session: u32,
) -> Result<Vec<OutlineItem>, String> {
    let session = state
        .session(session)
        .ok_or("that document is no longer open")?;
    let entries = session
        .pool
        .outline(Duration::from_secs(20))
        .map_err(|error| error.to_string())?;
    Ok(entries
        .into_iter()
        .map(|entry| OutlineItem {
            label: entry.title,
            page: entry.page,
            level: entry.level,
        })
        .collect())
}

/// Closes the matching document session (or whichever is open when `session` is `None`).
#[tauri::command(async)]
pub fn close_session(
    window: WebviewWindow,
    state: State<'_, AppState>,
    books: State<'_, crate::book::BookState>,
    cache: State<'_, crate::annotations::CharCache>,
    session: Option<u32>,
) {
    let _lifecycle = state
        .lifecycle
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(stopped_id) = state.stop_session_matching(session) {
        cache.remove_session(stopped_id);
        webview2::release_session_buffers(&window, stopped_id);
        webview2::release_probe_buffers(&window);
        if !crate::book::has_open(&books) {
            state.shelf.set_reading(false);
        }
    } else if let Some(id) = session {
        // Stale close for an earlier session: clean up any lingering buffers or chars of `id` only.
        cache.remove_session(id);
        webview2::release_session_buffers(&window, id);
        if state.current_session().is_none() {
            webview2::release_probe_buffers(&window);
        }
    } else {
        cache.clear();
        webview2::release_buffers(&window);
        if !crate::book::has_open(&books) {
            state.shelf.set_reading(false);
        }
    }
}

/// Flushes and tears down any open session/buffers/workers after the frontend finishes persisting
/// the active tab's reading progress, then closes the main window cleanly.
#[tauri::command(async)]
pub fn confirm_window_close(
    window: WebviewWindow,
    state: State<'_, AppState>,
    books: State<'_, crate::book::BookState>,
    cache: State<'_, crate::annotations::CharCache>,
    guard: State<'_, CloseGuard>,
) {
    let _ = guard.confirm();
    state.stop_and_release(&window);
    cache.clear();
    crate::book::close(&books);
    state.release_flow_client();
    state.shelf.set_reading(false);
    let _ = window.close();
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

/// Resizes the window, for a test that checks the reader keeps its place when the window changes.
/// Only a run with `--e2e` has it; the reader's iframe cannot reach commands at all.
#[tauri::command(async)]
pub fn e2e_resize_window(
    window: WebviewWindow,
    state: State<'_, AppState>,
    width: u32,
    height: u32,
) -> Result<(), String> {
    if !state.options.e2e {
        return Err("not running a test".to_owned());
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

/// Maximum length kept when synchronizing the native Win32 taskbar / `Alt+Tab` window title.
const MAX_WINDOW_TITLE_CHARS: usize = 256;

fn sanitize_window_title(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|ch| !ch.is_control())
        .take(MAX_WINDOW_TITLE_CHARS)
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "Musaeum".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Synchronizes the native OS window title (`Alt+Tab` and taskbar) with the active tab without
/// granting broad `plugin:window` capabilities to the webview.
#[tauri::command]
pub fn set_window_title(window: WebviewWindow, title: String) {
    let safe = sanitize_window_title(&title);
    let _ = window.set_title(&safe);
}

#[cfg(test)]
mod tests {
    use super::{CloseGuard, sanitize_window_title, take_matching_session};

    #[test]
    fn sanitizes_window_titles_for_taskbar_and_alt_tab() {
        assert_eq!(sanitize_window_title(""), "Musaeum");
        assert_eq!(sanitize_window_title("   \n\t  "), "Musaeum");
        assert_eq!(sanitize_window_title("Dune — Musaeum"), "Dune — Musaeum");
        assert_eq!(
            sanitize_window_title("Dom\0 Casmurro\n — Musaeum"),
            "Dom Casmurro — Musaeum"
        );
        let huge = "A".repeat(400);
        assert_eq!(sanitize_window_title(&huge).len(), 256);
    }

    #[test]
    fn stale_session_close_does_not_evict_a_newer_session() {
        let mut active = Some((2_u32, "second"));
        assert_eq!(
            take_matching_session(&mut active, Some(1), |(id, _)| *id),
            None
        );
        assert_eq!(active, Some((2, "second")));

        assert_eq!(
            take_matching_session(&mut active, Some(2), |(id, _)| *id),
            Some((2, "second"))
        );
        assert_eq!(active, None);

        let mut unkeyed = Some((5_u32, "fifth"));
        assert_eq!(
            take_matching_session(&mut unkeyed, None, |(id, _)| *id),
            Some((5, "fifth"))
        );
        assert_eq!(unkeyed, None);
    }

    #[test]
    fn close_guard_intercepts_once_and_allows_confirmed_close() {
        let guard = CloseGuard::default();
        assert!(!guard.is_confirmed());
        assert!(guard.intercept_once());
        assert!(!guard.intercept_once());
        assert!(guard.confirm());
        assert!(guard.is_confirmed());
        assert!(!guard.intercept_once());
    }

    #[test]
    fn mixed_fixed_and_flow_tab_transitions_keep_single_active_session_and_clean_home_state() {
        use crate::book::{self, BookState};
        use crate::flow::FlowBook;
        use std::path::PathBuf;
        use std::sync::Arc;

        let mut active_fixed: Option<u32> = None;
        let books = BookState::default();

        let make_flow = |doc_id: i64| {
            let normalized = musaeum_flow::normalize::normalize(
                musaeum_flow::package::SourceFormat::Markdown,
                b"# Chapter\n\nText.\n",
                &musaeum_flow::normalize::Options::default(),
            )
            .unwrap();
            Arc::new(FlowBook::new(
                doc_id,
                None,
                PathBuf::from("cache"),
                normalized.manifest,
            ))
        };

        let active_total = |fixed: &Option<u32>, books: &BookState| -> usize {
            usize::from(fixed.is_some()) + usize::from(book::has_open(books))
        };

        // 1. Open Fixed #1 -> Open Fixed #2 -> stale close for #1
        let _ = take_matching_session(&mut active_fixed, None, |id| *id);
        book::close(&books);
        active_fixed = Some(1);
        assert_eq!(active_total(&active_fixed, &books), 1);

        let evicted = take_matching_session(&mut active_fixed, None, |id| *id);
        assert_eq!(evicted, Some(1));
        book::close(&books);
        active_fixed = Some(2);
        assert_eq!(active_total(&active_fixed, &books), 1);

        assert_eq!(
            take_matching_session(&mut active_fixed, Some(1), |id| *id),
            None
        );
        assert_eq!(active_fixed, Some(2));

        // 2. Switch Fixed #2 -> Flow A: fixed session stops before flow book installs
        assert_eq!(
            take_matching_session(&mut active_fixed, None, |id| *id),
            Some(2)
        );
        book::close(&books);
        let ticket_a = book::next_open_ticket(&books);
        let (origin_a, _) = book::open_package_for_ticket(
            &books,
            Some(ticket_a),
            "http://tauri.localhost",
            make_flow(10),
            false,
        )
        .unwrap();
        assert_eq!(active_total(&active_fixed, &books), 1);

        // 3. Switch Flow A -> Flow B, then stale close for Flow A arrives late
        book::close(&books);
        let ticket_b = book::next_open_ticket(&books);
        let (origin_b, _) = book::open_package_for_ticket(
            &books,
            Some(ticket_b),
            "http://tauri.localhost",
            make_flow(20),
            false,
        )
        .unwrap();
        assert!(!book::close_matching(&books, Some(&origin_a)));
        assert_eq!(active_total(&active_fixed, &books), 1);

        // 4. Switch Flow B -> Fixed #3: flow origin is invalidated immediately
        let _ = take_matching_session(&mut active_fixed, None, |id| *id);
        book::close(&books);
        active_fixed = Some(3);
        assert!(!book::has_open(&books));
        assert!(!book::close_matching(&books, Some(&origin_b)));
        assert_eq!(active_total(&active_fixed, &books), 1);

        // 5. Suspend to Home (`🏠`): 0 active sessions of either kind
        assert_eq!(
            take_matching_session(&mut active_fixed, Some(3), |id| *id),
            Some(3)
        );
        assert_eq!(active_total(&active_fixed, &books), 0);
    }
}
