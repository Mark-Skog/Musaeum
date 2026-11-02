//! Finds identifiers and metadata in a book's bytes, with a limit on everything it reads.
//!
//! All of this module takes hostile input, so it has no unsafe code at all, and nothing it does
//! scales with what a file claims about itself.

#![forbid(unsafe_code)]

mod epub;
mod pdf;
mod zip;

use std::fs::File;
use std::io;
use std::os::windows::fs::FileExt;

use serde::{Deserialize, Serialize};

use crate::protocol::Format;

/// What a book says about itself. Every field may be missing.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct Extracted {
    /// The PDF's `/ID[0]`, in lowercase hexadecimal. It stays the same when a PDF is edited.
    pub pdf_id0: Option<String>,
    /// The EPUB's `dc:identifier`, the one its package names as unique if it names one.
    pub dc_identifier: Option<String>,
    /// An ISBN whose check digit is right, in digits only.
    pub isbn: Option<String>,
    /// Its title.
    pub title: Option<String>,
    /// Its authors and the like, in the book's order.
    pub creators: Vec<String>,
    /// Its language, as written.
    pub language: Option<String>,
    /// The year in its date.
    pub year: Option<i32>,
}

/// Why a file could not be read as what it was supposed to be.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// Reading failed.
    #[error("reading: {0}")]
    Io(#[from] io::Error),
    /// It is not that format at all.
    #[error("not a {0}")]
    NotThisFormat(&'static str),
    /// It is damaged, or is something this does not read.
    #[error("{0}")]
    Malformed(&'static str),
    /// Something in it is larger than is reasonable.
    #[error("too large: {0}")]
    TooLarge(&'static str),
}

/// Bytes that can be read in any order.
pub trait ReadAt {
    /// Reads at `offset`, returning how much it read, which is 0 at the end.
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> io::Result<usize>;

    /// How many bytes there are.
    fn size(&self) -> io::Result<u64>;
}

impl ReadAt for File {
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
        self.seek_read(buffer, offset)
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.metadata()?.len())
    }
}

impl ReadAt for [u8] {
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(self.len());
        let available = &self[start..];
        let count = available.len().min(buffer.len());
        buffer[..count].copy_from_slice(&available[..count]);
        Ok(count)
    }

    fn size(&self) -> io::Result<u64> {
        Ok(self.len() as u64)
    }
}

/// Reads up to `length` bytes at `offset`, fewer only at the end of the file.
pub(crate) fn read_up_to(
    source: &(impl ReadAt + ?Sized),
    offset: u64,
    length: usize,
) -> io::Result<Vec<u8>> {
    let mut buffer = vec![0; length];
    let mut filled = 0;
    while filled < length {
        let read = source.read_at(offset + filled as u64, &mut buffer[filled..])?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    buffer.truncate(filled);
    Ok(buffer)
}

/// Reads what `format` says about a file.
pub fn extract(format: Format, source: &(impl ReadAt + ?Sized)) -> Result<Extracted, ParseError> {
    match format {
        Format::Pdf => pdf::extract(source),
        Format::Epub => epub::extract(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_past_the_end_gives_what_there_is() {
        let bytes: &[u8] = b"0123456789";
        assert_eq!(read_up_to(bytes, 8, 100).unwrap(), b"89");
        assert_eq!(read_up_to(bytes, 100, 10).unwrap(), b"");
    }
}
