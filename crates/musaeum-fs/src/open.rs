//! Opens a file to read its bytes, unless those bytes are in the cloud.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_RANDOM_ACCESS, FILE_FLAG_SEQUENTIAL_SCAN, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE,
};

use crate::path::verbatim;
use crate::stat::{Stat, attributes_of, is_cloud_only, stat_of_handle};

/// How a file is about to be read, so the cache can behave accordingly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadPattern {
    /// Start to end, for a full hash.
    Whole,
    /// A few pieces, for a fingerprint.
    Samples,
}

/// A file open for reading, and what it was at the moment it was opened.
#[derive(Debug)]
pub struct ContentFile {
    /// Allows others to read, write, rename and delete it. The one thing Windows does not allow while
    /// this is open is renaming a folder it is in, whatever the sharing mode.
    pub file: File,
    /// What it is.
    pub stat: Stat,
}

/// Why a file cannot be read right now.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// Its bytes are not on this disk, and reading them would start a download.
    #[error("its bytes are in the cloud")]
    CloudOnly,
    /// It is not there.
    #[error("it no longer exists")]
    Missing,
    /// It is there, but cannot be opened, such as when someone holds it exclusively.
    #[error("{0}")]
    Unreadable(io::Error),
}

impl From<io::Error> for OpenError {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => Self::Missing,
            _ => Self::Unreadable(error),
        }
    }
}

/// Opens a file for reading its bytes, and refuses a placeholder, since reading one starts a download.
/// Only the attributes can prevent that: `FILE_FLAG_OPEN_NO_RECALL` looks like it should, but a read
/// through such a handle still downloads, as `tests/cloud.rs` in `musaeum-library` shows.
///
/// The attributes are checked before opening anything, because with `RECALL_ON_OPEN` the open is what
/// starts the download. They are checked again from the handle, because the file may have changed
/// between the two. And from then on it cannot change: while this handle is open, Windows refuses to
/// free the file's bytes.
pub fn open_content(path: &Path, pattern: ReadPattern) -> Result<ContentFile, OpenError> {
    if is_cloud_only(attributes_of(path)?) {
        return Err(OpenError::CloudOnly);
    }
    let cache_hint = match pattern {
        ReadPattern::Whole => FILE_FLAG_SEQUENTIAL_SCAN,
        ReadPattern::Samples => FILE_FLAG_RANDOM_ACCESS,
    };
    let wide = verbatim(path)?;
    let exact_path = PathBuf::from(OsString::from_wide(&wide[..wide.len() - 1]));
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(cache_hint)
        .open(exact_path)?;
    let stat = stat_of_handle(&file)?;
    if stat.is_cloud_only() {
        return Err(OpenError::CloudOnly);
    }
    Ok(ContentFile { file, stat })
}
