//! Isolated worker process: rasterizes PDF tiles with PDFium into shared memory.
//!
//! PDFium's header states that none of its APIs are thread-safe, so parallelism comes
//! from running several of these processes rather than several threads. Isolation also
//! means a malicious or merely broken PDF can only take down this process.
//!
//! The worker receives everything through its stdio pipes: a slab handle and a read-only
//! file handle duplicated by the host. It runs inside `musaeum-sandbox`, and by the time it
//! gets to those handles it could not open a file by path if it tried. Logs go to stderr,
//! because stdout carries the binary control protocol.

use std::io::{self, BufWriter, Write};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering, fence};
use std::time::Instant;

use musaeum_render_pdf::engine::{CancelProbe, Document, OpenError, RenderOutcome};
use musaeum_render_pdf::startup::{self, Attached};
use musaeum_shm::PixelSlab;
use musaeum_shm::wire::{
    FatalCode, HostMessage, PROTOCOL_VERSION, RenderJob, TileFailure, WorkerMessage, read_frame,
    write_frame,
};

fn main() -> ExitCode {
    musaeum_sandbox::target::die_quietly();
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

    // The process is locked down from here on, before reading anything from anywhere.
    let opened_at = Instant::now();
    let pdfium = match startup::load_pdfium_and_lock_down() {
        Ok(pdfium) => pdfium,
        Err(error) => {
            eprintln!("render worker: {error}");
            return outbox.fatal(error.code, error.detail);
        }
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

    let Attached { slab, document } = match startup::attach(&init) {
        Ok(attached) => attached,
        Err(error) => {
            eprintln!("render worker: {error}");
            return outbox.fatal(error.code, error.detail);
        }
    };
    let mut document = match Document::open(&pdfium, &document) {
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
            HostMessage::Trim => {
                document.release_pages();
                startup::release_memory();
            }
            HostMessage::Init(_) => return outbox.fatal(FatalCode::BadHandshake, 1),
        }
    }
    Ok(())
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
