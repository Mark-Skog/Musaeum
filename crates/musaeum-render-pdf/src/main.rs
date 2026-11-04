//! Isolated worker process: rasterizes PDF tiles with PDFium into shared memory.
//!
//! PDFium's header states that none of its APIs are thread-safe, so parallelism comes
//! from running several of these processes rather than several threads. Isolation also
//! means a malicious or merely broken PDF can only take down this process.
//!
//! What every worker shares, from the handshake to the control loop, is `musaeum-worker`; this
//! file only opens a PDF.

use std::io;
use std::process::ExitCode;
use std::time::Instant;

use musaeum_render_pdf::engine::{Document, OpenError};
use musaeum_render_pdf::startup::{self, Attached};
use musaeum_shm::wire::FatalCode;
use musaeum_worker::Outbox;

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

fn run() -> io::Result<()> {
    let mut inbox = io::stdin().lock();
    let mut outbox = Outbox::new(io::stdout().lock());

    // The process is locked down from here on, before reading anything from anywhere.
    let opened_at = Instant::now();
    let pdfium = match startup::load_pdfium_and_lock_down() {
        Ok(pdfium) => pdfium,
        Err(error) => {
            eprintln!("render worker: {error}");
            return outbox.fatal(error.code, error.detail);
        }
    };

    let Some(init) = musaeum_worker::handshake(&mut inbox, &mut outbox)? else {
        return Ok(());
    };
    let Attached {
        slab,
        document: bytes,
    } = match startup::attach(&init) {
        Ok(attached) => attached,
        Err(error) => {
            eprintln!("render worker: {error}");
            return outbox.fatal(error.code, error.detail);
        }
    };
    let mut document = match Document::open(&pdfium, &bytes) {
        Ok(document) => document,
        Err(OpenError { pdfium_error }) => {
            return outbox.fatal(FatalCode::DocumentOpen, pdfium_error);
        }
    };
    musaeum_worker::serve(
        &mut document,
        &slab,
        &init,
        opened_at,
        &mut inbox,
        &mut outbox,
    )
}
