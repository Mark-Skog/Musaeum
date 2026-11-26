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

/// A mapped view of a shared-memory section owned by this instance.
#[derive(Debug)]
pub struct ShmRegion {
    ptr: NonNull<u8>,
    len: usize,
    section: HANDLE,
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
        // SAFETY: `section` was just created with read/write access and is `len` bytes;
        // `from_raw_handle` takes sole ownership and closes it if mapping fails.
        unsafe { Self::from_raw_handle(section as usize as u64, len) }
    }

    /// Takes ownership of a section handle received from the host and maps it.
    /// If mapping fails, `raw_handle` is closed before returning the error.
    ///
    /// # Safety
    ///
    /// `raw_handle` must be a valid section handle in this process, not owned by
    /// anything else, and the section must be at least `len` bytes long.
    pub unsafe fn from_raw_handle(raw_handle: u64, len: usize) -> io::Result<Self> {
        let section = raw_handle as usize as HANDLE;
        if len == 0 {
            // SAFETY: the caller transferred ownership of `section` to this function.
            unsafe { CloseHandle(section) };
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a shared-memory region cannot be empty",
            ));
        }
        // SAFETY: the caller guarantees `section` is a valid section handle.
        match unsafe { map_view(section, len) } {
            Ok(ptr) => Ok(Self { ptr, len, section }),
            Err(error) => {
                // SAFETY: `section` is owned here and is not used after this error path.
                unsafe { CloseHandle(section) };
                Err(error)
            }
        }
    }

    /// Duplicates `section` into this process and maps an independent view of `len` bytes.
    ///
    /// Because both the kernel handle and the virtual-address mapping belong to the new
    /// [`ShmRegion`], closing the caller's original handle or unmapping any view the caller
    /// holds (such as `ICoreWebView2SharedBuffer::Close`) cannot invalidate this region
    /// while it or any `Arc` holding it remains alive.
    pub fn from_borrowed_section(section: BorrowedHandle<'_>, len: usize) -> io::Result<Self> {
        if len == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a shared-memory region cannot be empty",
            ));
        }
        // SAFETY: `GetCurrentProcess` returns a valid pseudo-handle for this process, and
        // `section` stays open for the duration of the borrow.
        let raw = unsafe {
            let current = BorrowedHandle::borrow_raw(GetCurrentProcess());
            duplicate_handle_into(
                section.as_raw_handle() as HANDLE,
                current,
                SECTION_RW_ACCESS,
            )?
        };
        // SAFETY: `raw` is a fresh handle duplicated into this process and owned solely here.
        unsafe { Self::from_raw_handle(raw, len) }
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
        // SAFETY: deliberately asks for too much; `from_raw_handle` closes `raw` and fails.
        let result = unsafe { ShmRegion::from_raw_handle(raw, 1 << 30) };
        assert!(result.is_err());
    }

    #[test]
    fn borrowed_section_survives_caller_unmap_and_close() {
        use crate::ring::{DeliveryRing, TileAnnouncement, ring_len};
        use crate::slab::{PixelSlab, slab_len};

        let ring_bytes = ring_len(64);
        // SAFETY: anonymous page-file-backed section with valid size and null attributes/name.
        let external_ring_section = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                ptr::null(),
                PAGE_READWRITE,
                0,
                ring_bytes as u32,
                ptr::null(),
            )
        };
        assert!(!external_ring_section.is_null());
        // SAFETY: `external_ring_section` is valid and `ring_bytes` matches its size.
        let external_ring_view = unsafe { map_view(external_ring_section, ring_bytes) }.unwrap();

        let slab_bytes = slab_len(4);
        // SAFETY: anonymous page-file-backed section with valid size and null attributes/name.
        let external_slab_section = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                ptr::null(),
                PAGE_READWRITE,
                0,
                slab_bytes as u32,
                ptr::null(),
            )
        };
        assert!(!external_slab_section.is_null());
        // SAFETY: `external_slab_section` is valid and `slab_bytes` matches its size.
        let external_slab_view = unsafe { map_view(external_slab_section, slab_bytes) }.unwrap();

        // SAFETY: both external handles remain open throughout `from_borrowed_section`.
        let ring_region = unsafe {
            ShmRegion::from_borrowed_section(
                BorrowedHandle::borrow_raw(external_ring_section),
                ring_bytes,
            )
        }
        .unwrap();
        // SAFETY: as above.
        let slab_region = unsafe {
            ShmRegion::from_borrowed_section(
                BorrowedHandle::borrow_raw(external_slab_section),
                slab_bytes,
            )
        }
        .unwrap();

        let mut ring = DeliveryRing::init(ring_region, 64).unwrap();
        let slab = PixelSlab::init(slab_region, 4).unwrap();

        // Verify shared underlying physical pages before closing the external WebView2-like side.
        // SAFETY: `external_ring_view` maps `ring_bytes` bytes; word 0 holds `RING_MAGIC`.
        let magic = unsafe { external_ring_view.as_ptr().cast::<u32>().read_volatile() };
        assert_eq!(magic, crate::ring::RING_MAGIC);

        // Simulate `ICoreWebView2SharedBuffer::Close()` tearing down the external view and handle
        // while `RenderPool::start` / `DeliveryRing::consumed` and `PixelSlab` are still active.
        // SAFETY: unmapping and closing the caller-owned views/handles once, never touching them again.
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: external_ring_view.as_ptr().cast(),
            });
            CloseHandle(external_ring_section);
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: external_slab_view.as_ptr().cast(),
            });
            CloseHandle(external_slab_section);
        }

        // Previously this atomic load faulted with `0xc0000005` (`STATUS_ACCESS_VIOLATION`).
        assert_eq!(ring.consumed(), 0);
        let mut consumer = ring.consumer();
        let announced = TileAnnouncement {
            slot: 0,
            page: 2,
            scale: 65_536,
            x: 0,
            y: 0,
            epoch: 1,
            render_us: 100,
            done_us: 200,
        };
        ring.publish(announced).unwrap();
        assert_eq!(consumer.peek(), Some(announced));
        consumer.advance();
        assert_eq!(ring.consumed(), 1);

        let dup_raw = slab.region().duplicate_into(current_process()).unwrap();
        // SAFETY: `dup_raw` is a fresh handle to `slab_bytes` bytes owned by `worker_view`.
        let worker_view = unsafe { ShmRegion::from_raw_handle(dup_raw, slab_bytes) }.unwrap();
        assert_eq!(worker_view.len(), slab_bytes);
    }
}
