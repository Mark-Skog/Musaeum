//! Lists a directory from the directory itself, without opening what is in it.

use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::mem::{self, offset_of};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::{Path, PathBuf};
use std::ptr;
use std::slice;

use windows_sys::Win32::Foundation::{ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_EXTD_DIR_INFO, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdExtdDirectoryInfo,
    GetFileInformationByHandleEx, OPEN_EXISTING,
};

use crate::path::verbatim;
use crate::stat::{identity_of_handle, is_cloud_only, is_unsupported};
use crate::{FileId, unix_nanos};

/// The bit of a reparse tag that says it stands for another name, as a symlink or a junction do.
/// Cloud, deduplication and compression tags do not have it: those are ordinary files.
const REPARSE_TAG_NAME_SURROGATE: u32 = 0x2000_0000;

/// What a directory entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// A file.
    File,
    /// A directory.
    Directory,
    /// A symlink or junction, which a scan does not follow: it can make a cycle, and can lead
    /// outside the folder the user chose.
    Link,
}

/// One entry of a directory, as the directory records it. If the file is open for writing or has
/// other hard links, the size and the date may be behind, and [`crate::stat`] has the real ones.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its name, exactly as stored.
    pub name: OsString,
    /// What it is.
    pub kind: EntryKind,
    /// Its length in bytes.
    pub size: u64,
    /// When its contents last changed, in nanoseconds since the Unix epoch.
    pub mtime_ns: i64,
    /// `FILE_ATTRIBUTE_*`.
    pub attributes: u32,
    /// `None` on a volume whose IDs do not last.
    pub file_id: Option<FileId>,
}

impl Entry {
    /// Whether this file's bytes are somewhere else.
    pub fn is_cloud_only(&self) -> bool {
        is_cloud_only(self.attributes)
    }
}

/// A directory and what is in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listing {
    /// The volume's serial number.
    pub volume: u64,
    /// The directory's own ID, by which a renamed directory is recognized.
    pub directory_id: Option<FileId>,
    /// Everything in it, except `.` and `..`.
    pub entries: Vec<Entry>,
}

/// Lists `path`, opening the directory and nothing in it.
pub fn list_directory(path: &Path) -> io::Result<Listing> {
    let directory = open_directory(path)?;
    let (volume, directory_id) = identity_of_handle(directory.as_raw_handle())?;
    let entries = match read_with_ids(&directory, directory_id.is_some()) {
        Ok(entries) => entries,
        Err(error) if is_unsupported(&error) => read_without_ids(path)?,
        Err(error) => return Err(error),
    };
    Ok(Listing {
        volume,
        directory_id,
        entries,
    })
}

fn open_directory(path: &Path) -> io::Result<File> {
    let wide = verbatim(path)?;
    // SAFETY: the path is null-terminated and outlives the call, and no security attributes or
    // template are given.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a fresh handle owned by nothing else.
    Ok(unsafe { File::from_raw_handle(handle) })
}

fn read_with_ids(directory: &File, ids_last: bool) -> io::Result<Vec<Entry>> {
    const BUFFER_WORDS: usize = 8 * 1024;
    // Words, because the records have 8-byte alignment.
    let mut buffer = vec![0u64; BUFFER_WORDS];
    let mut entries = Vec::new();
    loop {
        // SAFETY: the handle is open, and the pointer and the length describe `buffer`.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                directory.as_raw_handle(),
                FileIdExtdDirectoryInfo,
                buffer.as_mut_ptr().cast(),
                (BUFFER_WORDS * mem::size_of::<u64>()) as u32,
            )
        };
        if ok == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                return Ok(entries);
            }
            return Err(error);
        }
        parse_records(&buffer, ids_last, &mut entries)?;
    }
}

fn parse_records(buffer: &[u64], ids_last: bool, entries: &mut Vec<Entry>) -> io::Result<()> {
    let header = offset_of!(FILE_ID_EXTD_DIR_INFO, FileName);
    let total = mem::size_of_val(buffer);
    let base = buffer.as_ptr().cast::<u8>();
    let mut offset = 0;
    loop {
        if offset + header > total {
            return Err(malformed());
        }
        // SAFETY: the header fits inside the buffer, as just checked, and `offset` keeps the
        // alignment because the kernel aligns records to 8 bytes and this checks it below.
        let record = unsafe { base.add(offset).cast::<FILE_ID_EXTD_DIR_INFO>() };
        if !record.is_aligned() {
            return Err(malformed());
        }
        // SAFETY: aligned and inside the buffer, and the fields read here are plain integers.
        let (next, name_bytes, attributes, tag, last_write, end_of_file, id) = unsafe {
            (
                (*record).NextEntryOffset as usize,
                (*record).FileNameLength as usize,
                (*record).FileAttributes,
                (*record).ReparsePointTag,
                (*record).LastWriteTime,
                (*record).EndOfFile,
                (*record).FileId.Identifier,
            )
        };
        if offset + header + name_bytes > total {
            return Err(malformed());
        }
        // SAFETY: the name fits inside the buffer, as just checked, UTF-16 needs the alignment
        // of the record's own field, and a pointer from `base` may reach all of the buffer.
        let name = unsafe {
            slice::from_raw_parts(
                base.add(offset + header).cast::<u16>(),
                name_bytes / mem::size_of::<u16>(),
            )
        };
        if !is_dot_entry(name) {
            entries.push(Entry {
                name: OsString::from_wide(name),
                kind: kind_of(attributes, tag),
                size: end_of_file.max(0) as u64,
                mtime_ns: unix_nanos(last_write),
                attributes,
                file_id: if ids_last { FileId::from_raw(id) } else { None },
            });
        }
        if next == 0 {
            return Ok(());
        }
        offset += next;
    }
}

fn malformed() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "a directory record falls outside its buffer",
    )
}

fn is_dot_entry(name: &[u16]) -> bool {
    const DOT: u16 = b'.' as u16;
    matches!(name, [DOT] | [DOT, DOT])
}

fn kind_of(attributes: u32, reparse_tag: u32) -> EntryKind {
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        && reparse_tag & REPARSE_TAG_NAME_SURROGATE != 0
    {
        EntryKind::Link
    } else if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        EntryKind::Directory
    } else {
        EntryKind::File
    }
}

/// For what cannot list IDs at all, such as FAT or some network shares. `std::fs` gets all of
/// this from `FindFirstFile`, which opens none of the files either.
fn read_without_ids(path: &Path) -> io::Result<Vec<Entry>> {
    let path = PathBuf::from(OsString::from_wide(
        verbatim(path)?
            .split_last()
            .map_or(&[][..], |(_, rest)| rest),
    ));
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        let kind = if metadata.file_type().is_symlink() {
            EntryKind::Link
        } else if metadata.is_dir() {
            EntryKind::Directory
        } else {
            EntryKind::File
        };
        entries.push(Entry {
            name: entry.file_name(),
            kind,
            size: metadata.file_size(),
            mtime_ns: unix_nanos(metadata.last_write_time() as i64),
            attributes: metadata.file_attributes(),
            file_id: None,
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_told_apart_from_cloud_files() {
        const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;
        const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
        const IO_REPARSE_TAG_CLOUD_6: u32 = 0x9000_601A;
        const IO_REPARSE_TAG_ONEDRIVE: u32 = 0x8000_0021;

        let link = FILE_ATTRIBUTE_REPARSE_POINT;
        let directory = FILE_ATTRIBUTE_DIRECTORY;
        assert_eq!(kind_of(link, IO_REPARSE_TAG_SYMLINK), EntryKind::Link);
        assert_eq!(
            kind_of(link | directory, IO_REPARSE_TAG_MOUNT_POINT),
            EntryKind::Link
        );
        assert_eq!(kind_of(link, IO_REPARSE_TAG_CLOUD_6), EntryKind::File);
        assert_eq!(
            kind_of(link | directory, IO_REPARSE_TAG_ONEDRIVE),
            EntryKind::Directory
        );
        assert_eq!(kind_of(0, 0), EntryKind::File);
        assert_eq!(kind_of(directory, 0), EntryKind::Directory);
    }
}
