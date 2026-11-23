//! What can go wrong.

use std::io;
use std::path::PathBuf;

/// Everything in this crate that can fail fails with this.
pub type Result<T> = std::result::Result<T, Error>;

/// Why something did not happen.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// SQLite refused or failed.
    #[error("database: {0}")]
    Database(#[from] rusqlite::Error),
    /// The file system refused or failed.
    #[error("{context}: {source}")]
    Io {
        /// What was being done.
        context: String,
        /// What went wrong.
        #[source]
        source: io::Error,
    },
    /// Another process has this library open for writing.
    #[error("another Musaeum is already writing to this library")]
    LockedByAnotherProcess,
    /// The file is a database, but not one of ours.
    #[error("{0} is not a Musaeum database")]
    NotOurDatabase(PathBuf),
    /// The file was written by a newer version of this program, which this one must not touch.
    #[error("this library has schema {found}, and this program only knows up to {known}")]
    NewerSchema {
        /// The version in the file.
        found: i64,
        /// The newest version this program knows.
        known: i64,
    },
    /// Opened for reading only, and the library needs an upgrade first, or a write was attempted.
    #[error("{0}")]
    ReadOnly(&'static str),
    /// What was asked about is not in the library.
    #[error("{0}")]
    NotFound(String),
    /// What was asked for cannot be done safely as things stand.
    #[error("{0}")]
    Refused(String),
    /// The thread that does all the writing has stopped, which only happens after a panic.
    #[error("the database writer has stopped")]
    WriterStopped,
    /// The full-text index could not do what was asked. It can be made again from the books.
    #[error("the full-text index: {0}")]
    FullText(String),
}

impl From<tantivy::TantivyError> for Error {
    fn from(error: tantivy::TantivyError) -> Self {
        Error::FullText(error.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Error::Refused(error.to_string())
    }
}

/// For `map_err`: names what was being done when the file system failed.
pub(crate) fn io_error(context: impl Into<String>) -> impl FnOnce(io::Error) -> Error {
    let context = context.into();
    move |source| Error::Io { context, source }
}
