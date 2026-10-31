//! Isolated worker process: rasterizes PDF tiles with PDFium into shared memory.
//!
//! PDFium's header states that none of its APIs are thread-safe, so parallelism comes
//! from running several of these processes rather than several threads. Isolation also
//! means a malicious or merely broken PDF can only take down this process.
//!
//! The worker receives everything through its stdio pipes: a slab handle and a read-only
//! file handle duplicated by the host. It never opens a file by path, which is what will
//! let Experiment D take that right away entirely. Logs go to stderr, because stdout
//! carries the binary control protocol.

mod engine;

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering, fence};
use std::time::Instant;

use memmap2::Mmap;
use musaeum_shm::slab::SlabError;
use musaeum_shm::wire::{
    FatalCode, HostMessage, InitParams, PROTOCOL_VERSION, RenderJob, TileFailure, WorkerMessage,
    read_frame, write_frame,
};
use musaeum_shm::{PixelSlab, ShmRegion};
use windows_sys::Win32::System::Diagnostics::Debug::{
    SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX, SetErrorMode,
};

use engine::{CancelProbe, Document, OpenError, Pdfium, RenderOutcome};

fn main() -> ExitCode {
    // A worker that dies must simply die: no crash dialog, no Windows Error Reporting
    // prompt waiting on a user who cannot see this process. The host restarts it.
    // SAFETY: `SetErrorMode` has no preconditions.
    unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX)
    };
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        std::process::abort();
    }));

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("render worker: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The control pipe towards the host.
struct Outbox<W: Write> {
    writer: BufWriter<W>,
}

impl<W: Write> Outbox<W> {
    fn queue(&mut self, message: WorkerMessage) -> io::Result<()> {
        write_frame(&mut self.writer, &message.encode())
    }

    fn send(&mut self, message: WorkerMessage) -> io::Result<()> {
        self.queue(message)?;
        self.writer.flush()
    }

    /// Reports why the worker cannot continue, then gives up.
    fn fatal(&mut self, code: FatalCode, detail: u32) -> io::Result<()> {
        self.send(WorkerMessage::Fatal { code, detail })?;
        Err(io::Error::other(format!("{code} (detail {detail})")))
    }
}

fn run() -> io::Result<()> {
    let mut inbox = io::stdin().lock();
    let mut outbox = Outbox {
        writer: BufWriter::with_capacity(64 * 1024, io::stdout().lock()),
    };

    let Some(frame) = read_frame(&mut inbox).map_err(io::Error::other)? else {
        return Ok(());
    };
    let init = match HostMessage::decode(&frame) {
        Ok(HostMessage::Init(init)) => init,
        _ => return outbox.fatal(FatalCode::BadHandshake, 0),
    };
    if init.protocol != PROTOCOL_VERSION {
        return outbox.fatal(FatalCode::ProtocolMismatch, init.protocol);
    }

    let opened_at = Instant::now();
    let Some((slab, mapped)) = attach(&init, &mut outbox)? else {
        return Ok(());
    };
    let dll = std::env::current_exe()?.with_file_name("pdfium.dll");
    let pdfium = match Pdfium::load(&dll) {
        Ok(pdfium) => pdfium,
        Err(error) => {
            eprintln!("render worker: loading {}: {error}", dll.display());
            return outbox.fatal(FatalCode::LibraryLoad, 0);
        }
    };
    let mut document = match Document::open(&pdfium, &mapped) {
        Ok(document) => document,
        Err(OpenError { pdfium_error }) => {
            return outbox.fatal(FatalCode::DocumentOpen, pdfium_error);
        }
    };

    let open_us = micros_since(opened_at);
    outbox.send(WorkerMessage::Ready {
        protocol: PROTOCOL_VERSION,
        page_count: document.page_count(),
        open_us: u64::from(open_us),
    })?;

    if init.send_geometry {
        let started = Instant::now();
        for (page, &size) in (0u32..).zip(document.page_sizes()) {
            outbox.queue(WorkerMessage::Geometry { page, size })?;
        }
        outbox.send(WorkerMessage::GeometryEnd {
            page_count: document.page_count(),
            elapsed_us: u64::from(micros_since(started)) + u64::from(document.geometry_us()),
        })?;
    }

    let cancel = slab
        .cancel_cell(init.worker_index as usize)
        .map_err(io::Error::other)?;

    while let Some(frame) = read_frame(&mut inbox).map_err(io::Error::other)? {
        match HostMessage::decode(&frame).map_err(io::Error::other)? {
            HostMessage::Render(job) => {
                let report = render(&mut document, &slab, cancel, job);
                // Pairs with the host's acquire fence: every pixel written above is
                // visible to it by the time it reads this message.
                fence(Ordering::Release);
                outbox.send(report)?;
            }
            HostMessage::Shutdown => break,
            HostMessage::Init(_) => return outbox.fatal(FatalCode::BadHandshake, 1),
        }
    }
    Ok(())
}

/// Maps what the host shared with this process. Returns `None` after reporting failure.
fn attach<W: Write>(
    init: &InitParams,
    outbox: &mut Outbox<W>,
) -> io::Result<Option<(PixelSlab, Mmap)>> {
    let Ok(slab_len) = usize::try_from(init.slab_len) else {
        outbox.fatal(FatalCode::SlabAttach, 0)?;
        return Ok(None);
    };
    // SAFETY: the host duplicated this handle into this process for this purpose, nothing
    // else here owns it, and the host states the section's length.
    let slab = unsafe { ShmRegion::from_raw_handle(init.slab_handle, slab_len) }
        .map_err(SlabAttachError::from)
        .and_then(|region| PixelSlab::attach(region).map_err(SlabAttachError::from));
    let slab = match slab {
        Ok(slab) => slab,
        Err(error) => {
            eprintln!("render worker: {error}");
            outbox.fatal(FatalCode::SlabAttach, 0)?;
            return Ok(None);
        }
    };

    // SAFETY: the host duplicated this read-only file handle for this process, and
    // `File` becomes its only owner.
    let file = unsafe { File::from_raw_handle(init.doc_handle as usize as RawHandle) };
    // SAFETY: if another process truncates the file while it is mapped, a read here can
    // fault. That kills this worker, which is the failure the host already handles;
    // PDFium treats the mapped bytes as untrusted input regardless.
    let mapped = match unsafe { Mmap::map(&file) } {
        Ok(mapped) => mapped,
        Err(error) => {
            eprintln!("render worker: mapping the document: {error}");
            outbox.fatal(
                FatalCode::DocumentMap,
                error.raw_os_error().unwrap_or(0) as u32,
            )?;
            return Ok(None);
        }
    };
    Ok(Some((slab, mapped)))
}

#[derive(Debug, thiserror::Error)]
enum SlabAttachError {
    #[error("mapping the slab: {0}")]
    Map(#[from] io::Error),
    #[error("validating the slab: {0}")]
    Layout(#[from] SlabError),
}

fn render(
    document: &mut Document<'_, '_>,
    slab: &PixelSlab,
    cancel: &AtomicU64,
    job: RenderJob,
) -> WorkerMessage {
    let started = Instant::now();
    let RenderJob {
        job_seq,
        slot,
        tile,
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
    match unsafe { document.render_tile(target, tile, &probe) } {
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

fn micros_since(start: Instant) -> u32 {
    u32::try_from(start.elapsed().as_micros()).unwrap_or(u32::MAX)
}
