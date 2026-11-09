//! The isolated process itself.

use std::fs::File;
use std::io;
use std::os::windows::fs::FileExt;
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::process::ExitCode;

use crate::parse;
use crate::protocol::{
    PROTOCOL_VERSION, Reply, Request, Task, read_frame, write_files, write_frame,
};

/// The largest book this worker normalizes.
const MAX_BOOK: u64 = 1 << 30;

/// Normalizing walks trees as deep as a document nests, so it gets a stack to match.
const NORMALIZE_STACK: usize = 64 << 20;

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

/// A normalized book's files, or why there is none.
fn normalize(request: &Request, file: File) -> Result<Vec<(String, Vec<u8>)>, String> {
    let format = request
        .format
        .flow()
        .ok_or_else(|| "this format does not flow".to_owned())?;
    let size = file.metadata().map_err(|error| error.to_string())?.len();
    if size > MAX_BOOK {
        return Err(format!("{size} bytes is more than this worker reads"));
    }
    // Read by position: the handle shares its file pointer with the host's, which a previous
    // request may have left anywhere.
    let mut bytes = vec![0u8; size as usize];
    let mut filled = 0usize;
    while filled < bytes.len() {
        let read = file
            .seek_read(&mut bytes[filled..], filled as u64)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    bytes.truncate(filled);
    drop(file);
    let worker = std::thread::Builder::new()
        .name("normalize".to_owned())
        .stack_size(NORMALIZE_STACK)
        .spawn(move || {
            musaeum_flow::normalize::normalize(
                format,
                &bytes,
                &musaeum_flow::normalize::Options::default(),
            )
        })
        .map_err(|error| error.to_string())?;
    let book = worker
        .join()
        .map_err(|_| "the normalizer panicked".to_owned())?
        .map_err(|error| error.to_string())?;
    Ok(book.files)
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
        match request.task {
            Task::Extract => {
                let reply = match parse::extract(request.format, &file) {
                    Ok(extracted) => Reply::Extracted(extracted),
                    Err(error) => Reply::Rejected {
                        reason: error.to_string(),
                    },
                };
                drop(file);
                write_frame(&mut output, &reply)?;
            }
            Task::Normalize => match normalize(&request, file) {
                Ok(files) => {
                    let bytes = files.iter().map(|(_, data)| data.len() as u64).sum();
                    write_frame(
                        &mut output,
                        &Reply::Normalized {
                            files: files.len() as u32,
                            bytes,
                        },
                    )?;
                    write_files(&mut output, &files)?;
                }
                Err(reason) => write_frame(&mut output, &Reply::Rejected { reason })?,
            },
        }
    }
    Ok(())
}
