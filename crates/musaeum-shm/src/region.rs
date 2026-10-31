//! Anonymous shared-memory sections and the handle plumbing to share them.
//!
//! Sections are never named. A named section could be opened by anything in the
//! same session; here a worker can only reach one if the host explicitly
//! duplicates a handle into it, which is what the sandbox in Experiment D needs.

use std::fs::File;
use std::io;
use std::os::windows::io::{AsRawHandle, BorrowedHandle};
use std::ptr::{self, NonNull};

use windows_sys::Win32::Foundation::{CloseHandle, DuplicateHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::FILE_GENERIC_READ;
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// Access rights a render worker needs on a shared-memory section.
pub const SECTION_RW_ACCESS: u32 = FILE_MAP_READ | FILE_MAP_WRITE;

/// Who is responsible for unmapping the view and closing the section handle.
#[derive(Debug)]
enum Ownership {
    Owned,
    /// Memory that belongs to someone else, such as an `ICoreWebView2SharedBuffer`.
    External,
}

/// A mapped view of a shared-memory section.
#[derive(Debug)]
pub struct ShmRegion {
    ptr: NonNull<u8>,
    len: usize,
    section: HANDLE,
    ownership: Ownership,
}

// SAFETY: the region only stores a pointer to process-wide mapped memory and a
// kernel handle; neither has thread affinity. All access to the bytes goes through
// raw pointers and atomics in `slab` and `ring`, which define their own protocols.
unsafe impl Send for ShmRegion {}
// SAFETY: see the `Send` impl; `&ShmRegion` exposes no non-atomic mutation.
unsafe impl Sync for ShmRegion {}

impl ShmRegion {
    /// Creates a new zero-filled section backed by the page file and maps it.
    pub fn create(len: usize) -> io::Result<Self> {
        if len == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a shared-memory region cannot be empty",
            ));
        }
        let size = len as u64;
        // SAFETY: an invalid file handle requests a page-file-backed section; the
        // null security attributes and null name are documented as valid.
        let section = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                ptr::null(),
                PAGE_READWRITE,
                (size >> 32) as u32,
                size as u32,
                ptr::null(),
            )
        };
        if section.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `section` was just created with read/write access and is `len` bytes.
        match unsafe { map_view(section, len) } {
            Ok(ptr) => Ok(Self {
                ptr,
                len,
                section,
                ownership: Ownership::Owned,
            }),
            Err(error) => {
                // SAFETY: `section` is a valid handle owned here and not used again.
                unsafe { CloseHandle(section) };
                Err(error)
            }
        }
    }

    /// Takes ownership of a section handle received from the host and maps it.
    ///
    /// # Safety
    ///
    /// `raw_handle` must be a valid section handle in this process, not owned by
    /// anything else, and the section must be at least `len` bytes long.
    pub unsafe fn from_raw_handle(raw_handle: u64, len: usize) -> io::Result<Self> {
        let section = raw_handle as usize as HANDLE;
        // SAFETY: the caller guarantees the handle and the length.
        let ptr = unsafe { map_view(section, len) }?;
        Ok(Self {
            ptr,
            len,
            section,
            ownership: Ownership::Owned,
        })
    }

    /// Wraps memory mapped by someone else. Dropping the region leaves it mapped.
    ///
    /// # Safety
    ///
    /// `ptr` must stay valid for reads and writes of `len` bytes and `section` must
    /// stay open for as long as the region, or anything duplicated from it, is used.
    pub unsafe fn from_external(ptr: NonNull<u8>, len: usize, section: HANDLE) -> Self {
        Self {
            ptr,
            len,
            section,
            ownership: Ownership::External,
        }
    }

    /// Duplicates the section handle into `target_process`, returning the value of
    /// the new handle there. The caller's only remaining job is to tell the target
    /// what that value is.
    pub fn duplicate_into(&self, target_process: BorrowedHandle<'_>) -> io::Result<u64> {
        // SAFETY: the section stays open for as long as this region exists.
        unsafe { duplicate_handle_into(self.section, target_process, SECTION_RW_ACCESS) }
    }

    /// Start of the mapped view.
    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr.as_ptr()
    }

    /// Length of the mapped view in bytes.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Regions are never empty; provided for API symmetry with `len`.
    pub fn is_empty(&self) -> bool {
        false
    }
}

impl Drop for ShmRegion {
    fn drop(&mut self) {
        if matches!(self.ownership, Ownership::Owned) {
            let view = MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.ptr.as_ptr().cast(),
            };
            // SAFETY: the view and the handle are owned by this region and are never
            // used after this point. Failures here cannot be acted upon.
            unsafe {
                UnmapViewOfFile(view);
                CloseHandle(self.section);
            }
        }
    }
}

/// Duplicates a kernel handle into `target_process` with exactly `access` rights.
///
/// # Safety
///
/// `source` must be a valid handle in this process for the duration of the call.
unsafe fn duplicate_handle_into(
    source: HANDLE,
    target_process: BorrowedHandle<'_>,
    access: u32,
) -> io::Result<u64> {
    let mut remote: HANDLE = ptr::null_mut();
    // SAFETY: the caller guarantees `source`, the borrow keeps `target_process` open, the
    // pseudo-handle from `GetCurrentProcess` needs no closing, and `remote` is a valid
    // out-pointer. The new handle is not inheritable and has no extra options.
    let ok = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            source,
            target_process.as_raw_handle() as HANDLE,
            &mut remote,
            access,
            0,
            0,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(remote as usize as u64)
    }
}

/// Duplicates a file handle into `target_process` with read access only, so a worker
/// can read that one document without being able to open any path itself.
pub fn duplicate_file_read_only(
    file: &File,
    target_process: BorrowedHandle<'_>,
) -> io::Result<u64> {
    // SAFETY: the borrow of `file` keeps its handle open for the duration of the call.
    unsafe {
        duplicate_handle_into(
            file.as_raw_handle() as HANDLE,
            target_process,
            FILE_GENERIC_READ,
        )
    }
}

/// # Safety
///
/// `section` must be a valid section handle with map access and at least `len` bytes.
unsafe fn map_view(section: HANDLE, len: usize) -> io::Result<NonNull<u8>> {
    // SAFETY: guaranteed by the caller.
    let view = unsafe { MapViewOfFile(section, SECTION_RW_ACCESS, 0, 0, len) };
    NonNull::new(view.Value.cast::<u8>()).ok_or_else(io::Error::last_os_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current_process() -> BorrowedHandle<'static> {
        // SAFETY: `GetCurrentProcess` has no preconditions, and its pseudo-handle is valid
        // for the whole life of the process.
        unsafe { BorrowedHandle::borrow_raw(GetCurrentProcess()) }
    }

    #[test]
    fn rejects_empty_regions() {
        assert!(ShmRegion::create(0).is_err());
    }

    #[test]
    fn duplicated_handle_maps_the_same_memory() {
        let original = ShmRegion::create(4096).unwrap();
        let raw = original.duplicate_into(current_process()).unwrap();
        // SAFETY: `raw` is a fresh handle to a 4096-byte section, owned by `copy` alone.
        let copy = unsafe { ShmRegion::from_raw_handle(raw, 4096) }.unwrap();

        assert_ne!(original.as_ptr(), copy.as_ptr(), "views must be distinct");
        // SAFETY: both views are 4096 bytes long, and this test is single-threaded.
        unsafe {
            original.as_ptr().add(100).write(0xAB);
            assert_eq!(copy.as_ptr().add(100).read(), 0xAB);
            copy.as_ptr().add(200).write(0xCD);
            assert_eq!(original.as_ptr().add(200).read(), 0xCD);
        }
    }

    #[test]
    fn mapping_more_than_the_section_fails_cleanly() {
        let original = ShmRegion::create(4096).unwrap();
        let raw = original.duplicate_into(current_process()).unwrap();
        // SAFETY: deliberately asks for too much; the call must fail, not map it.
        // The handle leaks on this error path, which is acceptable in a test.
        let result = unsafe { ShmRegion::from_raw_handle(raw, 1 << 30) };
        assert!(result.is_err());
    }
}
