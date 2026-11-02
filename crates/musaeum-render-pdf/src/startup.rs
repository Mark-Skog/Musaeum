//! How a worker gets from an empty process to a locked-down one holding a document.
//!
//! The order is the security property. PDFium is loaded while the process can still read DLLs,
//! then the process locks itself down, and only then does it read a message from the host
//! that leads to the document's bytes. Nothing the document contains ever runs with more
//! than the lockdown token.

use std::fs::File;
use std::io;
use std::os::windows::io::{FromRawHandle, RawHandle};

use memmap2::Mmap;
use musaeum_shm::slab::SlabError;
use musaeum_shm::wire::{FatalCode, InitParams};
use musaeum_shm::{PixelSlab, ShmRegion};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};

use crate::engine::Pdfium;

/// Hands this process's memory back to the system while it has nothing to do. Whatever is needed
/// again comes back on demand, a page fault at a time.
pub fn release_memory() {
    // SAFETY: the pseudo-handle is always valid, and these two values are how the call is
    // documented to mean "remove as many pages as possible".
    unsafe { SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX) };
}

/// Why a worker cannot start, in the terms it reports to the host.
#[derive(Debug, thiserror::Error)]
#[error("{context}: {message}")]
pub struct StartupError {
    /// What to tell the host.
    pub code: FatalCode,
    /// Code-specific detail for the host.
    pub detail: u32,
    context: &'static str,
    message: String,
}

impl StartupError {
    fn new(code: FatalCode, context: &'static str, error: &io::Error) -> Self {
        Self {
            code,
            detail: error.raw_os_error().unwrap_or(0) as u32,
            context,
            message: error.to_string(),
        }
    }
}

/// Loads PDFium from next to this executable, then locks the process down. Whatever PDFium
/// needs from the disk has to be loaded by the time this returns.
pub fn load_pdfium_and_lock_down() -> Result<Pdfium, StartupError> {
    let dll = std::env::current_exe()
        .map_err(|error| StartupError::new(FatalCode::LibraryLoad, "finding this program", &error))?
        .with_file_name("pdfium.dll");
    let pdfium = Pdfium::load(&dll).map_err(|error| StartupError {
        code: FatalCode::LibraryLoad,
        detail: 0,
        context: "loading PDFium",
        message: format!("{}: {error}", dll.display()),
    })?;
    musaeum_sandbox::target::enter_lockdown().map_err(|error| {
        StartupError::new(FatalCode::Sandbox, "dropping the startup token", &error)
    })?;
    Ok(pdfium)
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
    // fault. That kills this worker, which is the failure the host already handles; PDFium
    // treats the mapped bytes as untrusted input regardless.
    let document = unsafe { Mmap::map(&file) }.map_err(|error| {
        StartupError::new(FatalCode::DocumentMap, "mapping the document", &error)
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
