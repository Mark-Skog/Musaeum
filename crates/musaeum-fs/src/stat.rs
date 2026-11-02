//! What a file is, without reading it.

use std::fs::File;
use std::io;
use std::mem;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{
    ERROR_INVALID_FUNCTION, ERROR_INVALID_LEVEL, ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED,
    HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_OFFLINE,
    FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, FILE_ATTRIBUTE_RECALL_ON_OPEN, FILE_BASIC_INFO,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_NO_RECALL, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_STANDARD_INFO, FileBasicInfo, FileIdInfo, FileStandardInfo, GetFileAttributesW,
    GetFileInformationByHandleEx, GetVolumeInformationByHandleW, INVALID_FILE_ATTRIBUTES,
    OPEN_EXISTING,
};

use crate::path::verbatim;
use crate::{FileId, unix_nanos};

// From `winnt.h`, which `windows-sys` does not carry under this name. NTFS and ReFS set it, and
// FAT does not: its IDs change when the volume is mounted again.
const FILE_SUPPORTS_OPEN_BY_FILE_ID: u32 = 0x0100_0000;

/// Whether these attributes mean a file's bytes are not on this disk. Reading such a file is
/// what starts the download, and with `RECALL_ON_OPEN`, so is opening it.
pub fn is_cloud_only(attributes: u32) -> bool {
    attributes
        & (FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
            | FILE_ATTRIBUTE_RECALL_ON_OPEN
            | FILE_ATTRIBUTE_OFFLINE)
        != 0
}

/// A file's attributes, read from its directory without opening it.
pub fn attributes_of(path: &Path) -> io::Result<u32> {
    let wide = verbatim(path)?;
    // SAFETY: the path is null-terminated and outlives the call.
    let attributes = unsafe { GetFileAttributesW(wide.as_ptr()) };
    if attributes == INVALID_FILE_ATTRIBUTES {
        return Err(io::Error::last_os_error());
    }
    Ok(attributes)
}

/// What a file is, from an open handle, so unlike a directory entry this is never out of date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stat {
    /// The volume's 64-bit serial number.
    pub volume: u64,
    /// `None` on a volume whose IDs do not last.
    pub file_id: Option<FileId>,
    /// Its length in bytes, which for a placeholder is the length of the file in the cloud.
    pub size: u64,
    /// When its contents last changed, in nanoseconds since the Unix epoch.
    pub mtime_ns: i64,
    /// `FILE_ATTRIBUTE_*`.
    pub attributes: u32,
}

impl Stat {
    /// Whether this is a directory.
    pub fn is_directory(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_DIRECTORY != 0
    }

    /// Whether this file's bytes are somewhere else.
    pub fn is_cloud_only(&self) -> bool {
        is_cloud_only(self.attributes)
    }
}

/// Looks at a file or directory without reading it. The attributes are checked first and the
/// open asks for no data and no recall, so this cannot start a download.
pub fn stat(path: &Path) -> io::Result<Stat> {
    attributes_of(path)?;
    let file = open_attributes_only(path)?;
    stat_of_handle(&file)
}

fn open_attributes_only(path: &Path) -> io::Result<File> {
    let wide = verbatim(path)?;
    // SAFETY: the path is null-terminated and outlives the call, and no security attributes or
    // template are given.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OPEN_NO_RECALL,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a fresh handle owned by nothing else.
    Ok(unsafe { File::from_raw_handle(handle) })
}

pub(crate) fn stat_of_handle(file: &File) -> io::Result<Stat> {
    let handle = file.as_raw_handle();
    let (volume, file_id) = identity_of_handle(handle)?;
    let basic: FILE_BASIC_INFO = query(handle, FileBasicInfo)?;
    let standard: FILE_STANDARD_INFO = query(handle, FileStandardInfo)?;
    Ok(Stat {
        volume,
        file_id,
        size: standard.EndOfFile.max(0) as u64,
        mtime_ns: unix_nanos(basic.LastWriteTime),
        attributes: basic.FileAttributes,
    })
}

/// The volume and the File ID behind a handle. Where the file system has no IDs to give or
/// they do not last, as on FAT, the ID is `None`, and only the volume means anything.
pub(crate) fn identity_of_handle(handle: HANDLE) -> io::Result<(u64, Option<FileId>)> {
    let (serial, flags) = volume_of_handle(handle)?;
    let stable = flags & FILE_SUPPORTS_OPEN_BY_FILE_ID != 0;
    match query::<FILE_ID_INFO>(handle, FileIdInfo) {
        Ok(info) => {
            let file_id = if stable {
                FileId::from_raw(info.FileId.Identifier)
            } else {
                None
            };
            Ok((info.VolumeSerialNumber, file_id))
        }
        Err(error) if is_unsupported(&error) => Ok((u64::from(serial), None)),
        Err(error) => Err(error),
    }
}

fn volume_of_handle(handle: HANDLE) -> io::Result<(u32, u32)> {
    let mut serial = 0;
    let mut flags = 0;
    // SAFETY: the handle is open, both out-pointers are valid, and null buffers with a zero
    // length are how this call is told that the names are not wanted.
    let ok = unsafe {
        GetVolumeInformationByHandleW(
            handle,
            ptr::null_mut(),
            0,
            &mut serial,
            ptr::null_mut(),
            &mut flags,
            ptr::null_mut(),
            0,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((serial, flags))
}

/// The errors a file system gives for an information class it does not implement.
pub(crate) fn is_unsupported(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error().map(|code| code as u32),
        Some(
            ERROR_INVALID_FUNCTION
                | ERROR_INVALID_LEVEL
                | ERROR_INVALID_PARAMETER
                | ERROR_NOT_SUPPORTED
        )
    )
}

/// Reads a fixed-size information class.
fn query<T>(
    handle: HANDLE,
    class: windows_sys::Win32::Storage::FileSystem::FILE_INFO_BY_HANDLE_CLASS,
) -> io::Result<T> {
    // SAFETY: every `T` used here is plain integers, for which zeroes are valid.
    let mut value: T = unsafe { mem::zeroed() };
    // SAFETY: the handle is open, and each caller pairs the class with its `T`, whose size is
    // what this passes.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            class,
            ptr::from_mut(&mut value).cast(),
            mem::size_of::<T>() as u32,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_cloud_attribute_counts() {
        assert!(is_cloud_only(FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS));
        assert!(is_cloud_only(FILE_ATTRIBUTE_RECALL_ON_OPEN));
        assert!(is_cloud_only(FILE_ATTRIBUTE_OFFLINE));
        assert!(!is_cloud_only(FILE_ATTRIBUTE_DIRECTORY));
        assert!(!is_cloud_only(0));
    }
}
