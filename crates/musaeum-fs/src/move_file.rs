//! Moves a file or a directory without ever overwriting anything.

use std::io;
use std::path::Path;

use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, ERROR_NOT_SAME_DEVICE,
};
use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};

use crate::path::verbatim;

/// Why a move did not happen.
#[derive(Debug, thiserror::Error)]
pub enum MoveError {
    /// Something already has that name.
    #[error("the target already exists")]
    TargetExists,
    /// The two paths are on different volumes, which takes a copy, not a move.
    #[error("the target is on another volume")]
    OtherVolume,
    /// Anything else.
    #[error("{0}")]
    Io(#[from] io::Error),
}

/// Moves `from` to `to` on the same volume, atomically, returning only once it is on disk. Without
/// `MOVEFILE_REPLACE_EXISTING` and `MOVEFILE_COPY_ALLOWED`, this can neither overwrite a file nor
/// turn into a copy that a crash could leave half done.
pub fn move_no_replace(from: &Path, to: &Path) -> Result<(), MoveError> {
    let from = verbatim(from)?;
    let to = verbatim(to)?;
    // SAFETY: both paths are null-terminated and outlive the call.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) } != 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error().map(|code| code as u32) {
        Some(ERROR_ALREADY_EXISTS | ERROR_FILE_EXISTS) => Err(MoveError::TargetExists),
        Some(ERROR_NOT_SAME_DEVICE) => Err(MoveError::OtherVolume),
        _ => Err(MoveError::Io(error)),
    }
}
