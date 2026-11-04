//! Isolated worker process: renders DjVu documents into shared memory with DjVuLibre.
//!
//! DjVuLibre is linked in, so there is no library to load and the process locks itself down before
//! anything else. It runs under the strictest policy, with win32k off and only Microsoft-signed
//! DLLs allowed.

use std::io;
use std::process::ExitCode;
use std::time::Instant;

use musaeum_render_djvu::book::Book;
use musaeum_shm::wire::FatalCode;
use musaeum_worker::{Attached, Outbox, StartupError};

fn main() -> ExitCode {
    musaeum_sandbox::target::die_quietly();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("djvu worker: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> io::Result<()> {
    let mut inbox = io::stdin().lock();
    let mut outbox = Outbox::new(io::stdout().lock());

    let opened_at = Instant::now();
    if let Err(error) = musaeum_sandbox::target::enter_lockdown() {
        let error = StartupError::os(FatalCode::Sandbox, "dropping the startup token", &error);
        eprintln!("djvu worker: {error}");
        return outbox.fatal(error.code, error.detail);
    }

    let Some(init) = musaeum_worker::handshake(&mut inbox, &mut outbox)? else {
        return Ok(());
    };
    let Attached {
        slab,
        document: bytes,
    } = match musaeum_worker::attach(&init) {
        Ok(attached) => attached,
        Err(error) => {
            eprintln!("djvu worker: {error}");
            return outbox.fatal(error.code, error.detail);
        }
    };
    let mut book = match Book::open(&bytes) {
        Ok(book) => book,
        Err(error) => {
            eprintln!("djvu worker: cannot read the document: {error:?}");
            return outbox.fatal(FatalCode::DocumentOpen, error.code());
        }
    };
    // DjVuLibre keeps its own copy of the bytes; the mapping is no longer needed.
    drop(bytes);
    musaeum_worker::serve(&mut book, &slab, &init, opened_at, &mut inbox, &mut outbox)
}
