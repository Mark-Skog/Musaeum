//! What the fixed-layout pipeline does with real files: their pages measured, tiles rendered at the
//! scales a reader would see, renders stopped part way, and workers killed in the middle of them.
//! Every worker is a real one, in the same sandbox the reader runs it in.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use musaeum_core::pool::BATCH_MAGIC;
use musaeum_core::{
    DeliveryMode, DocumentHints, PoolConfig, PoolError, PoolStats, RenderPool, encode_demand,
};
use musaeum_sandbox::Policy;
use musaeum_shm::slab::slab_len;
use musaeum_shm::tile::{SCALE_ONE, page_pixels, tile_grid};
use musaeum_shm::{PageGeometry, PixelSlab, ShmRegion, TILE_BYTES, TileAnnouncement, TileKey};
use serde::Serialize;

const STARTUP: Duration = Duration::from_secs(120);
const TILE_WAIT: Duration = Duration::from_secs(60);

fn config(worker_exe: &Path, pdf: &Path, workers: usize) -> PoolConfig {
    PoolConfig {
        worker_exe: worker_exe.to_owned(),
        document: pdf.to_owned(),
        worker_count: workers,
        process_memory_limit: Some(2 << 30),
        policy: Policy::Lockdown,
        startup_timeout: STARTUP,
        known_geometry: None,
    }
}

fn slab(slots: u32) -> Result<Arc<PixelSlab>, String> {
    let region = ShmRegion::create(slab_len(slots)).map_err(|error| error.to_string())?;
    Ok(Arc::new(
        PixelSlab::init(region, slots).map_err(|error| error.to_string())?,
    ))
}

/// How a document failed to open: refused by PDFium, or a failure of the pipeline itself.
#[derive(Debug, Serialize)]
pub enum OpenFailure {
    /// PDFium's own error code: 3 is a damaged file, 4 one that needs a password.
    Refused { pdfium_error: u32 },
    /// Anything else, which would be a fault of the Core.
    Core(String),
}

fn classify(error: PoolError) -> OpenFailure {
    match error {
        PoolError::WorkerFatal {
            code: musaeum_shm::wire::FatalCode::DocumentOpen,
            detail,
        } => OpenFailure::Refused {
            pdfium_error: detail,
        },
        other => OpenFailure::Core(other.to_string()),
    }
}

pub struct Measured {
    pub pages: Vec<PageGeometry>,
    pub hints: DocumentHints,
    pub open_us: u64,
    pub geometry_us: u64,
    pub wall_ms: f64,
}

/// Every page's size and the catalog's hints, read by one sandboxed worker without drawing.
pub fn measure(worker_exe: &Path, pdf: &Path) -> Result<Measured, OpenFailure> {
    let started = Instant::now();
    let slab = slab(2).map_err(OpenFailure::Core)?;
    let pool = RenderPool::start(
        config(worker_exe, pdf, 1),
        slab,
        DeliveryMode::Pull { max_batch: 1 },
    )
    .map_err(classify)?;
    let stats = pool
        .stats()
        .ok_or_else(|| OpenFailure::Core("the pool stopped answering".into()))?;
    Ok(Measured {
        pages: pool.pages().to_vec(),
        hints: pool.hints(),
        open_us: stats.open_us,
        geometry_us: stats.geometry_us,
        wall_ms: started.elapsed().as_secs_f64() * 1000.0,
    })
}

/// A tile as it arrived, reduced to what the checks need.
struct Arrived {
    tile: TileAnnouncement,
    /// Whether the pixels vary at all: a page with content gives tiles that are not one colour.
    varied: bool,
    opaque: bool,
}

/// Keeps pollers parked on a pool, and hands what arrives to a channel.
struct Puller {
    pool: Arc<RenderPool>,
    sender: Mutex<mpsc::Sender<Arrived>>,
}

impl Puller {
    fn start(pool: Arc<RenderPool>) -> Receiver<Arrived> {
        let (sender, receiver) = mpsc::channel();
        let puller = Arc::new(Self {
            pool,
            sender: Mutex::new(sender),
        });
        for _ in 0..3 {
            Arc::clone(&puller).park();
        }
        receiver
    }

    fn park(self: Arc<Self>) {
        let pool = Arc::clone(&self.pool);
        pool.park_poller(Box::new(move |batch| {
            if batch.is_empty() {
                return;
            }
            for arrived in parse_batch(&batch) {
                let _ = self
                    .sender
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .send(arrived);
            }
            self.park();
        }));
    }
}

fn parse_batch(batch: &[u8]) -> Vec<Arrived> {
    let word = |at: usize| u32::from_le_bytes(batch[at..at + 4].try_into().unwrap_or_default());
    if batch.len() < 8 || word(0) != BATCH_MAGIC {
        return Vec::new();
    }
    let count = word(4) as usize;
    let stride = 32 + TILE_BYTES;
    (0..count)
        .filter(|index| 8 + (index + 1) * stride <= batch.len())
        .map(|index| {
            let at = 8 + index * stride;
            let words: [u32; 8] = std::array::from_fn(|n| word(at + n * 4));
            let pixels = &batch[at + 32..at + 32 + TILE_BYTES];
            // A sample of the pixels is enough to tell a page with something on it from paper.
            let (rgba, _) = pixels.as_chunks::<4>();
            let first = rgba[0];
            let varied = rgba
                .iter()
                .step_by(97)
                .any(|pixel| pixel[..3] != first[..3]);
            let opaque = rgba.iter().step_by(97).all(|pixel| pixel[3] == 255);
            Arrived {
                tile: TileAnnouncement::from_words(words),
                varied,
                opaque,
            }
        })
        .collect()
}

#[derive(Debug, Default, Serialize)]
pub struct PageRender {
    pub page: u32,
    pub scale: f64,
    pub tiles: usize,
    /// From asking to the last tile of the page arriving.
    pub all_tiles_ms: f64,
    pub any_content: bool,
    pub all_opaque: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct SampleRender {
    pub pages: Vec<PageRender>,
    pub tiles_failed: u64,
    pub render_us: musaeum_core::Summary,
    pub load_us: musaeum_core::Summary,
    pub missing: usize,
}

/// The scale a reader would see this page at: as wide as a 1600-pixel window, which no power of
/// two is likely to be.
pub fn reading_scale(page: PageGeometry) -> u32 {
    let scale = (1600.0 / f64::from(page.width_pt.max(1.0)) * f64::from(SCALE_ONE)).round() as u32;
    let mut scale = scale.clamp(SCALE_ONE / 8, SCALE_ONE * 8);
    while page_pixels(page.width_pt, scale) > 1 << 19
        || page_pixels(page.height_pt, scale) > 1 << 19
    {
        scale /= 2;
    }
    scale
}

/// Renders whole pages at the scale a reader would see them, the first, the middle and the last.
pub fn render_sample(
    worker_exe: &Path,
    pdf: &Path,
    pages: &[PageGeometry],
) -> Result<SampleRender, OpenFailure> {
    let pool = Arc::new(
        RenderPool::start(
            config(worker_exe, pdf, 2),
            slab(16).map_err(OpenFailure::Core)?,
            DeliveryMode::Pull { max_batch: 4 },
        )
        .map_err(classify)?,
    );
    let arrivals = Puller::start(Arc::clone(&pool));
    let mut report = SampleRender::default();
    let last = pages.len().saturating_sub(1);
    let mut chosen = vec![0, last / 2, last];
    chosen.dedup();
    for (epoch, &page) in (1u32..).zip(&chosen) {
        let geometry = pages[page];
        let scale = reading_scale(geometry);
        let (columns, rows) = tile_grid(geometry, scale);
        let tiles: Vec<TileKey> = (0..rows)
            .flat_map(|y| {
                (0..columns).map(move |x| TileKey {
                    page: page as u32,
                    scale,
                    x: x as u16,
                    y: y as u16,
                })
            })
            .collect();
        let asked = Instant::now();
        pool.submit_demand(encode_demand(epoch, &tiles));
        let mut waiting: std::collections::HashSet<(u32, u32)> = tiles
            .iter()
            .map(|tile| (u32::from(tile.x), u32::from(tile.y)))
            .collect();
        let mut render = PageRender {
            page: page as u32,
            scale: f64::from(scale) / f64::from(SCALE_ONE),
            tiles: tiles.len(),
            all_opaque: true,
            ..PageRender::default()
        };
        let deadline = Instant::now() + TILE_WAIT;
        while !waiting.is_empty() && Instant::now() < deadline {
            match arrivals.recv_timeout(Duration::from_millis(200)) {
                Ok(arrived) => {
                    if arrived.tile.page == page as u32 && arrived.tile.scale == scale {
                        waiting.remove(&(arrived.tile.x, arrived.tile.y));
                        render.any_content |= arrived.varied;
                        render.all_opaque &= arrived.opaque;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    // A tile that failed will never arrive; stop waiting for what is accounted for.
                    let stats = pool.stats().unwrap_or_default();
                    if stats.tiles_failed > report.tiles_failed {
                        report.tiles_failed = stats.tiles_failed;
                        break;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        render.all_tiles_ms = asked.elapsed().as_secs_f64() * 1000.0;
        report.missing += waiting.len();
        report.pages.push(render);
    }
    let stats = pool
        .stats()
        .ok_or_else(|| OpenFailure::Core("the pool stopped answering".into()))?;
    report.tiles_failed = stats.tiles_failed;
    report.render_us = stats.render_us;
    report.load_us = stats.load_us;
    Ok(report)
}

#[derive(Debug, Default, Serialize)]
pub struct CancelReport {
    pub attempts: usize,
    /// How long after asking each render was taken back, in milliseconds.
    pub cancel_after_ms: f64,
    /// Renders that stopped part way.
    pub cancelled: u64,
    /// Renders that finished before the cancellation reached them, and were thrown away.
    pub finished_anyway: u64,
    /// From asking a render to stop to the worker saying it had, in microseconds.
    pub latency_us: musaeum_core::Summary,
    /// What a whole first render of a page takes, loading included, for comparison.
    pub full_render_us: musaeum_core::Summary,
}

/// Starts first renders of real pages and takes them back a quarter of the way through, to see how
/// soon a real page stops. Every request is for a page this worker has not loaded, since that is
/// what an abandoned page usually is, and no tile is ever asked for twice.
pub fn cancel_mid_render(
    worker_exe: &Path,
    pdf: &Path,
    pages: &[PageGeometry],
    attempts: usize,
) -> Result<CancelReport, OpenFailure> {
    let pool = Arc::new(
        RenderPool::start(
            config(worker_exe, pdf, 1),
            slab(8).map_err(OpenFailure::Core)?,
            DeliveryMode::Pull { max_batch: 1 },
        )
        .map_err(classify)?,
    );
    let arrivals = Puller::start(Arc::clone(&pool));
    let tile = |page: usize| TileKey {
        page: page as u32,
        scale: (reading_scale(pages[page]) * 2).min(SCALE_ONE * 8),
        x: 0,
        y: 0,
    };
    // Pages spread through the document, half to time, half to cancel, each used once.
    let wanted = (2 * attempts).min(pages.len());
    let mut chosen: Vec<usize> = (0..wanted)
        .map(|n| (n * pages.len()) / wanted.max(1))
        .collect();
    chosen.dedup();
    let (timed, cancelled) = chosen.split_at(chosen.len() / 2);
    let mut epoch = 1;

    let mut full = Vec::new();
    for &page in timed {
        pool.submit_demand(encode_demand(epoch, &[tile(page)]));
        epoch += 1;
        let started = Instant::now();
        while let Ok(arrived) = arrivals.recv_timeout(TILE_WAIT) {
            if arrived.tile.page == page as u32 {
                full.push(started.elapsed().as_micros() as u32);
                break;
            }
        }
    }
    let full = musaeum_core::Summary::of(&full);
    let delay = Duration::from_micros(u64::from(full.p50 / 4).max(2_000));
    let settle = Duration::from_micros(u64::from(full.max) * 2) + Duration::from_millis(500);

    let before = pool.stats().unwrap_or_default();
    for &page in cancelled {
        let base = pool.stats().unwrap_or_default();
        pool.submit_demand(encode_demand(epoch, &[tile(page)]));
        epoch += 1;
        std::thread::sleep(delay);
        pool.submit_demand(encode_demand(epoch, &[]));
        epoch += 1;
        // Until the worker has answered one way or the other.
        let deadline = Instant::now() + settle;
        let answered = |stats: &PoolStats| {
            stats.tiles_cancelled + stats.tiles_done + stats.tiles_failed
                > base.tiles_cancelled + base.tiles_done + base.tiles_failed
        };
        while Instant::now() < deadline && !pool.stats().is_some_and(|stats| answered(&stats)) {
            std::thread::sleep(Duration::from_millis(5));
        }
        while arrivals.try_recv().is_ok() {}
    }
    let after = pool
        .stats()
        .ok_or_else(|| OpenFailure::Core("the pool stopped answering".into()))?;
    Ok(CancelReport {
        attempts: cancelled.len(),
        cancel_after_ms: delay.as_secs_f64() * 1000.0,
        cancelled: after.tiles_cancelled - before.tiles_cancelled,
        finished_anyway: (after.results_discarded - before.results_discarded)
            + (after.tiles_done - before.tiles_done),
        latency_us: after.cancel_latency_us,
        full_render_us: full,
    })
}

#[derive(Debug, Default, Serialize)]
pub struct KillReport {
    pub kills: usize,
    pub restarts: u32,
    pub recovery_us: musaeum_core::Summary,
    /// From the kill to the next tile the reader asked for arriving.
    pub kill_to_next_tile_ms: Vec<f64>,
    pub reading_went_on: bool,
}

fn terminate(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};

    // SAFETY: plain calls with no pointers; the handle is checked and closed.
    unsafe {
        let process = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if process.is_null() {
            return false;
        }
        let ended = TerminateProcess(process, 0xDEAD) != 0;
        CloseHandle(process);
        ended
    }
}

/// Kills the only worker while it renders a heavy page, and asks for the next page right away.
pub fn kill_while_reading(
    worker_exe: &Path,
    pdf: &Path,
    pages: &[PageGeometry],
    kills: usize,
) -> Result<KillReport, OpenFailure> {
    let pool = Arc::new(
        RenderPool::start(
            config(worker_exe, pdf, 1),
            slab(8).map_err(OpenFailure::Core)?,
            DeliveryMode::Pull { max_batch: 1 },
        )
        .map_err(classify)?,
    );
    let arrivals = Puller::start(Arc::clone(&pool));
    let mut report = KillReport {
        reading_went_on: true,
        ..KillReport::default()
    };
    let mut epoch = 1;
    for round in 0..kills.min(pages.len().max(1)) {
        let page = (round * 7) % pages.len();
        let heavy = TileKey {
            page: page as u32,
            scale: (reading_scale(pages[page]) * 2).min(SCALE_ONE * 8),
            x: 0,
            y: 0,
        };
        pool.submit_demand(encode_demand(epoch, &[heavy]));
        epoch += 1;
        std::thread::sleep(Duration::from_millis(15));
        let Some(Some(pid)) = pool.worker_pids().first().copied() else {
            report.reading_went_on = false;
            break;
        };
        let killed_at = Instant::now();
        if !terminate(pid) {
            continue;
        }
        report.kills += 1;
        let next_page = (page + 1) % pages.len();
        let next = TileKey {
            page: next_page as u32,
            scale: musaeum_shm::tile::scale_of_lod(-1),
            x: 0,
            y: 0,
        };
        pool.submit_demand(encode_demand(epoch, &[next]));
        epoch += 1;
        let deadline = Instant::now() + TILE_WAIT;
        let mut arrived = false;
        while Instant::now() < deadline {
            match arrivals.recv_timeout(Duration::from_millis(500)) {
                Ok(tile) if tile.tile.page == next.page && tile.tile.scale == next.scale => {
                    arrived = true;
                    break;
                }
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        report
            .kill_to_next_tile_ms
            .push(killed_at.elapsed().as_secs_f64() * 1000.0);
        report.reading_went_on &= arrived;
    }
    let stats: PoolStats = pool
        .stats()
        .ok_or_else(|| OpenFailure::Core("the pool stopped answering".into()))?;
    report.restarts = stats.worker_restarts;
    report.recovery_us = stats.recovery_us;
    Ok(report)
}

/// The worker binary next to this one.
pub fn worker_exe() -> PathBuf {
    std::env::current_exe()
        .map(|exe| exe.with_file_name("musaeum-render-pdf.exe"))
        .unwrap_or_else(|_| PathBuf::from("musaeum-render-pdf.exe"))
}
