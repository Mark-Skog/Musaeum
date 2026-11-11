//! What every isolated render worker shares, whatever format it reads.
//!
//! A worker receives everything through its stdio pipes: a slab handle and a read-only file handle
//! duplicated by the host. It runs inside `musaeum-sandbox`, and by the time it reads those handles
//! it could not open a file by path if it tried. Logs go to stderr, because stdout carries the
//! binary control protocol.
//!
//! A worker binary locks itself down, calls [`handshake`] and [`attach`], opens its document with
//! the format's library, and hands an [`Engine`] to [`serve`]. Everything about the protocol lives
//! here, so the formats cannot drift apart in how they answer.

#[cfg(feature = "harness")]
pub mod harness;

use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::sync::atomic::{AtomicU64, Ordering, fence};
use std::time::Instant;

use memmap2::Mmap;
use musaeum_shm::outline::{self, OUTLINE_PAGE, OutlineEntry};
use musaeum_shm::slab::SlabError;
use musaeum_shm::wire::{
    AnalyzeJob, FatalCode, HostMessage, InitParams, PROTOCOL_VERSION, RenderJob, TextJob,
    TileFailure, WorkerMessage, read_frame, write_frame,
};
use musaeum_shm::{
    CharLayer, PageContent, PageGeometry, PixelSlab, ShmRegion, TILE_BYTES, TextLayer, TileKey,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};

/// The control pipe towards the host.
pub struct Outbox<W: Write> {
    writer: BufWriter<W>,
}

impl<W: Write> Outbox<W> {
    /// Wraps the pipe.
    pub fn new(writer: W) -> Self {
        Self {
            writer: BufWriter::with_capacity(64 * 1024, writer),
        }
    }

    /// Queues a message without sending it yet.
    pub fn queue(&mut self, message: WorkerMessage) -> io::Result<()> {
        write_frame(&mut self.writer, &message.encode())
    }

    /// Sends a message, and everything queued before it.
    pub fn send(&mut self, message: WorkerMessage) -> io::Result<()> {
        self.queue(message)?;
        self.writer.flush()
    }

    /// Reports why the worker cannot continue, then gives up.
    pub fn fatal(&mut self, code: FatalCode, detail: u32) -> io::Result<()> {
        self.send(WorkerMessage::Fatal { code, detail })?;
        Err(io::Error::other(format!("{code} (detail {detail})")))
    }
}

/// Decides whether the current job should stop.
pub struct CancelProbe<'a> {
    cell: &'a AtomicU64,
    job_seq: u64,
}

impl<'a> CancelProbe<'a> {
    /// A probe for `job_seq`, watching the worker's cancel cell.
    pub fn new(cell: &'a AtomicU64, job_seq: u64) -> Self {
        Self { cell, job_seq }
    }

    /// Whether the host has cancelled this job, or a later one.
    pub fn is_cancelled(&self) -> bool {
        self.cell.load(Ordering::Acquire) >= self.job_seq
    }
}

/// How a render ended, when it did not fail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderOutcome {
    /// The buffer holds the complete tile.
    Finished {
        /// Microseconds spent loading the page; 0 if it was cached.
        load_us: u32,
    },
    /// Cancelled; the buffer's contents are undefined.
    Cancelled,
}

/// What a document asks of a reader, in the codes of [`WorkerMessage::Hints`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hints {
    /// `/PageLayout`, or its equivalent; 0 if the document says nothing.
    pub page_layout: u8,
    /// Reading direction: 0 if unstated, 1 left to right, 2 right to left.
    pub direction: u8,
}

/// One format's way of doing the jobs.
pub trait Engine {
    /// Every page's size, in page order, as measured when the document opened.
    fn page_sizes(&self) -> &[PageGeometry];

    /// Microseconds spent measuring the pages when the document opened.
    fn geometry_us(&self) -> u32;

    /// What the document asks of a reader. Called once, after the geometry.
    fn hints(&mut self) -> Hints;

    /// Renders one tile into `target`, checking `probe` before and during the work. A
    /// `background` job is one nobody waits for, so what it loads should not push out what
    /// visible pages need.
    ///
    /// # Safety
    ///
    /// `target` must be valid for writes of one whole tile (`TILE_BYTES`) and nothing else may
    /// access that memory until this returns.
    unsafe fn render_tile(
        &mut self,
        target: *mut u8,
        tile: TileKey,
        background: bool,
        probe: &CancelProbe<'_>,
    ) -> Result<RenderOutcome, TileFailure>;

    /// Measures what a page is made of.
    fn analyze(&mut self, page: u32) -> Result<PageContent, TileFailure>;

    /// Reads a page's text layer. A page without one has an empty layer.
    fn text_layer(&mut self, page: u32) -> Result<TextLayer, TileFailure>;

    /// Reads a page's characters and their quads in the unrotated crop box. A page without a text
    /// layer, or a format without one, has none.
    fn char_layer(&mut self, page: u32) -> Result<CharLayer, TileFailure> {
        let _ = page;
        Ok(CharLayer::default())
    }

    /// The document's outline, in reading order. Formats without one have none.
    fn outline(&mut self) -> Vec<OutlineEntry> {
        Vec::new()
    }

    /// Gives back whatever memory can be rebuilt later.
    fn trim(&mut self);
}

/// Why a worker cannot start, in the terms it reports to the host.
#[derive(Debug, thiserror::Error)]
#[error("{context}: {message}")]
pub struct StartupError {
    /// What to tell the host.
    pub code: FatalCode,
    /// Code-specific detail for the host.
    pub detail: u32,
    /// What was being attempted.
    pub context: &'static str,
    /// What went wrong.
    pub message: String,
}

impl StartupError {
    /// An error from an operating-system call.
    pub fn os(code: FatalCode, context: &'static str, error: &io::Error) -> Self {
        Self {
            code,
            detail: error.raw_os_error().unwrap_or(0) as u32,
            context,
            message: error.to_string(),
        }
    }
}

/// Hands this process's memory back to the system while it has nothing to do. Whatever is needed
/// again comes back on demand, a page fault at a time.
pub fn release_memory() {
    // SAFETY: the pseudo-handle is always valid, and these two values are how the call is
    // documented to mean "remove as many pages as possible".
    unsafe { SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX) };
}

/// Reads the host's first message. `Ok(None)` means the host went away before sending it; an
/// error has already been reported to the host.
pub fn handshake<R: Read, W: Write>(
    inbox: &mut R,
    outbox: &mut Outbox<W>,
) -> io::Result<Option<InitParams>> {
    let Some(frame) = read_frame(inbox).map_err(io::Error::other)? else {
        return Ok(None);
    };
    let init = match HostMessage::decode(&frame) {
        Ok(HostMessage::Init(init)) => init,
        _ => return outbox.fatal(FatalCode::BadHandshake, 0).map(|()| None),
    };
    if init.protocol != PROTOCOL_VERSION {
        return outbox
            .fatal(FatalCode::ProtocolMismatch, init.protocol)
            .map(|()| None);
    }
    Ok(Some(init))
}

/// What the host shared with this process.
#[derive(Debug)]
pub struct Attached {
    /// The tile slab.
    pub slab: PixelSlab,
    /// The document's bytes.
    pub document: Mmap,
}

/// Maps what the host shared. These two handles are all this process ever gets to work with.
pub fn attach(init: &InitParams) -> Result<Attached, StartupError> {
    let slab_len = usize::try_from(init.slab_len).map_err(|_| StartupError {
        code: FatalCode::SlabAttach,
        detail: 0,
        context: "mapping the slab",
        message: "its length does not fit this process".to_owned(),
    })?;
    // SAFETY: the host duplicated this handle into this process for this purpose, nothing
    // else here owns it, and the host states the section's length.
    let slab = unsafe { ShmRegion::from_raw_handle(init.slab_handle, slab_len) }
        .map_err(SlabAttachError::from)
        .and_then(|region| PixelSlab::attach(region).map_err(SlabAttachError::from))
        .map_err(|error| StartupError {
            code: FatalCode::SlabAttach,
            detail: 0,
            context: "attaching the slab",
            message: error.to_string(),
        })?;

    // SAFETY: the host duplicated this read-only file handle for this process, and `File`
    // becomes its only owner.
    let file = unsafe { File::from_raw_handle(init.doc_handle as usize as RawHandle) };
    // SAFETY: if another process truncates the file while it is mapped, a read here can
    // fault. That kills this worker, which is the failure the host already handles; the
    // format's library treats the mapped bytes as untrusted input regardless.
    let document = unsafe { Mmap::map(&file) }.map_err(|error| {
        StartupError::os(FatalCode::DocumentMap, "mapping the document", &error)
    })?;
    Ok(Attached { slab, document })
}

#[derive(Debug, thiserror::Error)]
enum SlabAttachError {
    #[error("mapping: {0}")]
    Map(#[from] io::Error),
    #[error("validating: {0}")]
    Layout(#[from] SlabError),
}

/// Announces the open document, reports its geometry if asked, then does jobs until the host says
/// to stop or goes away. `opened_at` is when the worker started, for the reported open time.
pub fn serve<E: Engine, R: Read, W: Write>(
    engine: &mut E,
    slab: &PixelSlab,
    init: &InitParams,
    opened_at: Instant,
    inbox: &mut R,
    outbox: &mut Outbox<W>,
) -> io::Result<()> {
    outbox.send(WorkerMessage::Ready {
        protocol: PROTOCOL_VERSION,
        page_count: engine.page_sizes().len() as u32,
        open_us: u64::from(micros_since(opened_at)),
    })?;
    if init.send_geometry {
        send_geometry(engine, outbox)?;
    }

    let cancel = slab
        .cancel_cell(init.worker_index as usize)
        .map_err(io::Error::other)?;

    while let Some(frame) = read_frame(inbox).map_err(io::Error::other)? {
        match HostMessage::decode(&frame).map_err(io::Error::other)? {
            HostMessage::Render(job) => {
                let report = render(engine, slab, cancel, job);
                // Pairs with the host's acquire fence: every pixel written above is
                // visible to it by the time it reads this message.
                fence(Ordering::Release);
                outbox.send(report)?;
            }
            HostMessage::Analyze(job) => outbox.send(analyze(engine, job))?,
            HostMessage::ExtractText(job) => {
                let report = extract_text(engine, slab, job);
                fence(Ordering::Release);
                outbox.send(report)?;
            }
            HostMessage::ExtractChars(job) => {
                let report = extract_chars(engine, slab, job);
                fence(Ordering::Release);
                outbox.send(report)?;
            }
            HostMessage::Shutdown => break,
            HostMessage::Trim => {
                engine.trim();
                release_memory();
            }
            HostMessage::SendGeometry => send_geometry(engine, outbox)?,
            HostMessage::Init(_) => return outbox.fatal(FatalCode::BadHandshake, 1),
        }
    }
    Ok(())
}

/// Reports every page's size and what the document asks of a reader, without drawing anything.
fn send_geometry<E: Engine, W: Write>(engine: &mut E, outbox: &mut Outbox<W>) -> io::Result<()> {
    let started = Instant::now();
    for (page, &size) in (0u32..).zip(engine.page_sizes()) {
        outbox.queue(WorkerMessage::Geometry { page, size })?;
    }
    let hints = engine.hints();
    outbox.queue(WorkerMessage::Hints {
        page_layout: hints.page_layout,
        direction: hints.direction,
    })?;
    outbox.send(WorkerMessage::GeometryEnd {
        page_count: engine.page_sizes().len() as u32,
        elapsed_us: u64::from(micros_since(started)) + u64::from(engine.geometry_us()),
    })
}

fn render<E: Engine>(
    engine: &mut E,
    slab: &PixelSlab,
    cancel: &AtomicU64,
    job: RenderJob,
) -> WorkerMessage {
    let started = Instant::now();
    let RenderJob {
        job_seq,
        slot,
        tile,
        background,
    } = job;
    let failed = |failure| WorkerMessage::TileFailed {
        job_seq,
        slot,
        failure,
    };

    let target = match slab.slot_ptr(slot) {
        Ok(target) => target,
        Err(_) => return failed(TileFailure::InvalidSlot),
    };
    let probe = CancelProbe::new(cancel, job_seq);
    // SAFETY: the host gave this worker exclusive write access to the slot until the
    // report below goes out (slab invariant 1), and `slot_ptr` checked that it is a
    // whole tile inside the mapped slab.
    match unsafe { engine.render_tile(target, tile, background, &probe) } {
        Ok(RenderOutcome::Finished { load_us }) => WorkerMessage::TileDone {
            job_seq,
            slot,
            render_us: micros_since(started),
            load_us,
        },
        Ok(RenderOutcome::Cancelled) => WorkerMessage::TileCancelled {
            job_seq,
            slot,
            elapsed_us: micros_since(started),
        },
        Err(failure) => failed(failure),
    }
}

fn analyze<E: Engine>(engine: &mut E, job: AnalyzeJob) -> WorkerMessage {
    let started = Instant::now();
    if job.page as usize >= engine.page_sizes().len() {
        return WorkerMessage::AnalysisFailed {
            job_seq: job.job_seq,
            failure: TileFailure::OutOfBounds,
        };
    }
    match engine.analyze(job.page) {
        Ok(content) if content.is_plausible() => WorkerMessage::PageContent {
            job_seq: job.job_seq,
            page: job.page,
            content,
            elapsed_us: micros_since(started),
        },
        // A measurement the host would refuse anyway is reported as the failure it is.
        Ok(_) => WorkerMessage::AnalysisFailed {
            job_seq: job.job_seq,
            failure: TileFailure::DecodeFailed,
        },
        Err(failure) => WorkerMessage::AnalysisFailed {
            job_seq: job.job_seq,
            failure,
        },
    }
}

fn extract_text<E: Engine>(engine: &mut E, slab: &PixelSlab, job: TextJob) -> WorkerMessage {
    let started = Instant::now();
    let TextJob {
        job_seq,
        slot,
        page,
    } = job;
    let failed = |failure| WorkerMessage::TileFailed {
        job_seq,
        slot,
        failure,
    };
    let target = match slab.slot_ptr(slot) {
        Ok(target) => target,
        Err(_) => return failed(TileFailure::InvalidSlot),
    };
    let layer = if page == OUTLINE_PAGE {
        // The outline travels as the text layer of a page no document has.
        outline::to_text_layer(&engine.outline())
    } else if page as usize >= engine.page_sizes().len() {
        return failed(TileFailure::OutOfBounds);
    } else {
        match engine.text_layer(page) {
            Ok(layer) => layer,
            Err(failure) => return failed(failure),
        }
    };
    // SAFETY: as for a render, the host gave this worker the slot until the report goes out,
    // and `slot_ptr` checked that the whole tile is mapped.
    let out = unsafe { std::slice::from_raw_parts_mut(target, TILE_BYTES) };
    match layer.encode_into(out) {
        Some(len) => WorkerMessage::TextDone {
            job_seq,
            slot,
            len: len as u32,
            elapsed_us: micros_since(started),
        },
        None => failed(TileFailure::TooLarge),
    }
}

fn extract_chars<E: Engine>(engine: &mut E, slab: &PixelSlab, job: TextJob) -> WorkerMessage {
    let started = Instant::now();
    let TextJob {
        job_seq,
        slot,
        page,
    } = job;
    let failed = |failure| WorkerMessage::TileFailed {
        job_seq,
        slot,
        failure,
    };
    let target = match slab.slot_ptr(slot) {
        Ok(target) => target,
        Err(_) => return failed(TileFailure::InvalidSlot),
    };
    if page as usize >= engine.page_sizes().len() {
        return failed(TileFailure::OutOfBounds);
    }
    let layer = match engine.char_layer(page) {
        Ok(layer) => layer,
        Err(failure) => return failed(failure),
    };
    // SAFETY: as for a text layer.
    let out = unsafe { std::slice::from_raw_parts_mut(target, TILE_BYTES) };
    match layer.encode_into(out) {
        Some(len) => WorkerMessage::TextDone {
            job_seq,
            slot,
            len: len as u32,
            elapsed_us: micros_since(started),
        },
        None => failed(TileFailure::TooLarge),
    }
}

/// Microseconds since `start`, saturating.
pub fn micros_since(start: Instant) -> u32 {
    u32::try_from(start.elapsed().as_micros()).unwrap_or(u32::MAX)
}
