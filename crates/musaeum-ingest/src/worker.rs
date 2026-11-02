//! The isolated process itself.

use std::fs::File;
use std::io;
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::process::ExitCode;

use crate::parse;
use crate::protocol::{PROTOCOL_VERSION, Reply, Request, read_frame, write_frame};

/// The whole life of a worker. Its executable is only a call to this.
pub fn main() -> ExitCode {
    musaeum_sandbox::target::die_quietly();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ingest worker: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> io::Result<()> {
    // Before reading anything from the host, so nothing from a book ever runs with more than this.
    musaeum_sandbox::target::enter_lockdown()?;

    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    write_frame(
        &mut output,
        &Reply::Ready {
            protocol: PROTOCOL_VERSION,
        },
    )?;

    while let Some(request) = read_frame::<Request>(&mut input)? {
        // SAFETY: the host duplicated this read-only handle into this process for this request,
        // and `File` becomes its only owner. A wrong value kills this process, which runs with
        // strict handle checks, and the host restarts it.
        let file = unsafe { File::from_raw_handle(request.handle as usize as RawHandle) };
        let reply = match parse::extract(request.format, &file) {
            Ok(extracted) => Reply::Extracted(extracted),
            Err(error) => Reply::Rejected {
                reason: error.to_string(),
            },
        };
        drop(file);
        write_frame(&mut output, &reply)?;
    }
    Ok(())
}
