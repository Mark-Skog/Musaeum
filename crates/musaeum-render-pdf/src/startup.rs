//! How a worker gets from an empty process to a locked-down one holding a document.
//!
//! The order is the security property. PDFium is loaded while the process can still read DLLs,
//! then the process locks itself down, and only then does it read a message from the host
//! that leads to the document's bytes. Nothing the document contains ever runs with more
//! than the lockdown token.

use musaeum_shm::wire::FatalCode;
pub use musaeum_worker::{Attached, StartupError, attach, release_memory};

use crate::engine::Pdfium;

/// Loads PDFium from next to this executable, then locks the process down. Whatever PDFium
/// needs from the disk has to be loaded by the time this returns.
pub fn load_pdfium_and_lock_down() -> Result<Pdfium, StartupError> {
    let dll = std::env::current_exe()
        .map_err(|error| StartupError::os(FatalCode::LibraryLoad, "finding this program", &error))?
        .with_file_name("pdfium.dll");
    let pdfium = Pdfium::load(&dll).map_err(|error| StartupError {
        code: FatalCode::LibraryLoad,
        detail: 0,
        context: "loading PDFium",
        message: format!("{}: {error}", dll.display()),
    })?;
    musaeum_sandbox::target::enter_lockdown().map_err(|error| {
        StartupError::os(FatalCode::Sandbox, "dropping the startup token", &error)
    })?;
    Ok(pdfium)
}
