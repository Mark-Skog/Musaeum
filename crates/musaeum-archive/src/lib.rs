//! Reads ZIP and RAR archives from bytes already in memory, with a vendored build of libarchive
//! that has no code to write, to open files or to start programs.
//!
//! libarchive reads an archive as a stream: entries come one after another, and an entry's data can
//! only be read while it is the current one. [`Archive`] keeps that shape. To read an earlier entry
//! again, open the archive again; for ZIP that costs a read of its central directory, since its
//! reader seeks past the data it skips.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::marker::PhantomData;
use std::ptr;

// Only libarchive calls zlib, so nothing in Rust would make the linker keep it.
use libz_sys as _;
use thiserror::Error;

#[allow(non_camel_case_types)]
#[repr(C)]
struct archive {
    _private: [u8; 0],
}

#[allow(non_camel_case_types)]
#[repr(C)]
struct archive_entry {
    _private: [u8; 0],
}

// From archive.h.
const ARCHIVE_EOF: c_int = 1;
const ARCHIVE_OK: c_int = 0;
const ARCHIVE_WARN: c_int = -20;
/// `AE_IFREG` from archive_entry.h: a regular file.
const AE_IFREG: u32 = 0o100000;
/// `AE_IFMT`: the bits of the mode that say what kind of entry it is.
const AE_IFMT: u32 = 0o170000;

unsafe extern "C" {
    fn archive_read_new() -> *mut archive;
    fn archive_read_support_format_zip_seekable(a: *mut archive) -> c_int;
    fn archive_read_support_format_rar(a: *mut archive) -> c_int;
    fn archive_read_support_format_rar5(a: *mut archive) -> c_int;
    fn archive_read_open_memory(a: *mut archive, buffer: *const c_void, size: usize) -> c_int;
    fn archive_read_next_header(a: *mut archive, entry: *mut *mut archive_entry) -> c_int;
    fn archive_read_data(a: *mut archive, buffer: *mut c_void, size: usize) -> isize;
    fn archive_read_free(a: *mut archive) -> c_int;
    fn archive_error_string(a: *mut archive) -> *const c_char;
    fn archive_format_name(a: *mut archive) -> *const c_char;
    fn archive_entry_pathname_utf8(entry: *mut archive_entry) -> *const c_char;
    fn archive_entry_pathname_w(entry: *mut archive_entry) -> *const u16;
    fn archive_entry_size(entry: *mut archive_entry) -> i64;
    fn archive_entry_size_is_set(entry: *mut archive_entry) -> c_int;
    fn archive_entry_filetype(entry: *mut archive_entry) -> u16;
    fn archive_entry_is_encrypted(entry: *mut archive_entry) -> c_int;
}

/// What went wrong, in libarchive's words.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ArchiveError {
    /// libarchive could not even allocate its reader.
    #[error("libarchive could not create a reader")]
    NoReader,
    /// The bytes are not an archive this build can read, or it is damaged.
    #[error("{0}")]
    Read(String),
    /// An entry is larger than the caller allows.
    #[error("the entry is larger than {0} bytes")]
    TooLarge(usize),
}

/// One entry of an archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its position in the archive, counting from zero.
    pub index: usize,
    /// Its path inside the archive, as UTF-8.
    pub name: String,
    /// Its size once extracted, if the archive states it.
    pub size: Option<u64>,
    /// Whether it is a regular file, rather than a directory or a link.
    pub is_file: bool,
    /// Whether its data is encrypted, which this build cannot read.
    pub encrypted: bool,
}

/// An archive being read from memory.
pub struct Archive<'data> {
    raw: *mut archive,
    next_index: usize,
    // libarchive reads from this memory for as long as the reader is open.
    _data: PhantomData<&'data [u8]>,
}

impl<'data> Archive<'data> {
    /// Opens a ZIP, RAR or RAR5 archive held in `data`.
    pub fn open(data: &'data [u8]) -> Result<Self, ArchiveError> {
        // SAFETY: no preconditions; a null result is handled below.
        let raw = unsafe { archive_read_new() };
        if raw.is_null() {
            return Err(ArchiveError::NoReader);
        }
        let archive = Self {
            raw,
            next_index: 0,
            _data: PhantomData,
        };
        // SAFETY: `raw` is a fresh reader. The seekable ZIP reader trusts the central directory,
        // as other readers of the format do, and needs the seeking that memory gives it.
        let status = unsafe {
            archive_read_support_format_zip_seekable(raw)
                .min(archive_read_support_format_rar(raw))
                .min(archive_read_support_format_rar5(raw))
        };
        archive.check(status)?;
        // SAFETY: `data` outlives the reader thanks to `'data`, and libarchive only reads it.
        let status = unsafe { archive_read_open_memory(raw, data.as_ptr().cast(), data.len()) };
        archive.check(status)?;
        Ok(archive)
    }

    /// The format libarchive recognized, such as "ZIP 2.0 (deflation)" or "RAR5".
    pub fn format_name(&self) -> String {
        // SAFETY: the reader is open; the result is null or a string libarchive owns.
        let name = unsafe { archive_format_name(self.raw) };
        if name.is_null() {
            return String::new();
        }
        // SAFETY: checked for null above, and libarchive returns a NUL-terminated string.
        unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned()
    }

    /// The next entry, or `None` after the last one. Moving on skips whatever was not read of the
    /// current entry's data.
    pub fn next_entry(&mut self) -> Result<Option<Entry>, ArchiveError> {
        let mut entry = ptr::null_mut();
        // SAFETY: the reader is open and `entry` is a valid out-pointer.
        let status = unsafe { archive_read_next_header(self.raw, &mut entry) };
        if status == ARCHIVE_EOF {
            return Ok(None);
        }
        self.check(status)?;
        if entry.is_null() {
            return Err(ArchiveError::Read(
                "libarchive returned no entry".to_owned(),
            ));
        }
        let index = self.next_index;
        self.next_index += 1;
        // SAFETY: `entry` is the current entry, valid until the next call on this reader.
        unsafe {
            Ok(Some(Entry {
                index,
                name: entry_name(entry),
                size: (archive_entry_size_is_set(entry) != 0)
                    .then(|| u64::try_from(archive_entry_size(entry)).unwrap_or(0)),
                is_file: u32::from(archive_entry_filetype(entry)) & AE_IFMT == AE_IFREG,
                encrypted: archive_entry_is_encrypted(entry) != 0,
            }))
        }
    }

    /// Reads the current entry's data, failing if it is longer than `limit` bytes.
    pub fn read_all(&mut self, limit: usize) -> Result<Vec<u8>, ArchiveError> {
        let mut out = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let read = self.read_chunk(&mut chunk)?;
            if read == 0 {
                return Ok(out);
            }
            if out.len() + read > limit {
                return Err(ArchiveError::TooLarge(limit));
            }
            out.extend_from_slice(&chunk[..read]);
        }
    }

    /// Reads up to `len` bytes from the start of the current entry's data: enough to read an
    /// image's header without extracting the whole image.
    pub fn read_prefix(&mut self, len: usize) -> Result<Vec<u8>, ArchiveError> {
        let mut out = vec![0u8; len];
        let mut filled = 0;
        while filled < len {
            let read = self.read_chunk(&mut out[filled..])?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        out.truncate(filled);
        Ok(out)
    }

    fn read_chunk(&mut self, buffer: &mut [u8]) -> Result<usize, ArchiveError> {
        // SAFETY: the reader is open and `buffer` is writable for its whole length.
        let read = unsafe { archive_read_data(self.raw, buffer.as_mut_ptr().cast(), buffer.len()) };
        if read < 0 {
            self.check(read as c_int)?;
            // A warning still delivered no data.
            return Ok(0);
        }
        Ok(read as usize)
    }

    /// Turns a status into an error, with libarchive's message. Warnings are not errors.
    fn check(&self, status: c_int) -> Result<(), ArchiveError> {
        if status == ARCHIVE_OK || status == ARCHIVE_WARN {
            return Ok(());
        }
        // SAFETY: the reader is open; the result is null or a string libarchive owns.
        let message = unsafe { archive_error_string(self.raw) };
        let message = if message.is_null() {
            format!("libarchive status {status}")
        } else {
            // SAFETY: checked for null above, and libarchive returns a NUL-terminated string.
            unsafe { CStr::from_ptr(message) }
                .to_string_lossy()
                .into_owned()
        };
        Err(ArchiveError::Read(message))
    }
}

/// The entry's path. libarchive converts it to UTF-8 when it can; otherwise the wide form is
/// converted here, replacing what is not valid.
///
/// # Safety
///
/// `entry` must be the reader's current entry.
unsafe fn entry_name(entry: *mut archive_entry) -> String {
    // SAFETY: the caller guarantees a valid entry; the result is null or owned by the entry.
    let utf8 = unsafe { archive_entry_pathname_utf8(entry) };
    if !utf8.is_null() {
        // SAFETY: checked for null; libarchive returns a NUL-terminated string.
        return unsafe { CStr::from_ptr(utf8) }
            .to_string_lossy()
            .into_owned();
    }
    // SAFETY: as above, for the wide form.
    let wide = unsafe { archive_entry_pathname_w(entry) };
    if wide.is_null() {
        return String::new();
    }
    let mut len = 0;
    // SAFETY: the string is NUL-terminated, so every unit up to the terminator is readable.
    while unsafe { *wide.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` units were just read one by one.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(wide, len) })
}

impl Drop for Archive<'_> {
    fn drop(&mut self) {
        // SAFETY: the reader was created by `archive_read_new` and is freed once, here.
        unsafe { archive_read_free(self.raw) };
    }
}

#[cfg(test)]
mod tests;
