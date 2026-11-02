//! Tells when something under a directory changes.
//!
//! These are hints and never facts: the system can lose them, and when its buffer fills up it does.
//! So the one thing this has to get right is saying when that happened, which is why this exists
//! instead of a crate. The `notify` crate, at 8.2.1, has no case for `ERROR_NOTIFY_ENUM_DIR`, and on
//! an error it does not know it stops watching the directory, without a word to anyone.

use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::mem::{self, offset_of};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr;
use std::slice;
use std::thread::{self, JoinHandle};

use windows_sys::Win32::Foundation::{
    ERROR_NOTIFY_ENUM_DIR, ERROR_OPERATION_ABORTED, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_DIR_NAME,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
    FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    ReadDirectoryChangesW,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects,
};

use crate::path::verbatim;

/// The largest buffer a watch over the network can ask for, and plenty on a local disk.
pub const DEFAULT_WATCH_BUFFER: usize = 64 * 1024;

/// What a watch has to say.
#[derive(Debug)]
pub enum WatchEvent {
    /// Something happened to this path. It may no longer exist, and may be in its short 8.3 form.
    Changed(PathBuf),
    /// Too much happened too fast and the system lost track of it. Anything may have changed.
    Overflow,
    /// The watch has ended because of this, such as the directory having been deleted. It needs
    /// starting again, once there is something to watch.
    Lost(io::Error),
}

/// Watches a directory and everything under it until this is dropped.
#[derive(Debug)]
pub struct DirectoryWatch {
    stop: OwnedHandle,
    thread: Option<JoinHandle<()>>,
}

impl Drop for DirectoryWatch {
    fn drop(&mut self) {
        // SAFETY: the event is open for as long as `self` is.
        unsafe { SetEvent(self.stop.as_raw_handle()) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Starts watching `path`, calling `sink` from a thread of its own for everything that happens.
/// The buffer is how much can happen between two looks, and `sink` is what stands in between, so
/// it should be quick.
pub fn watch_directory(
    path: &Path,
    buffer_bytes: usize,
    sink: impl FnMut(WatchEvent) + Send + 'static,
) -> io::Result<DirectoryWatch> {
    let directory = open_for_watching(path)?;
    let stop = new_event()?;
    let wake = new_event()?;
    let stop_for_thread = stop.try_clone()?;
    let root = path.to_owned();
    let thread = thread::Builder::new()
        .name("musaeum-watch".to_owned())
        .spawn(move || {
            let mut watcher = Watcher {
                directory,
                stop: stop_for_thread,
                wake,
                root,
                sink,
                buffer: vec![0; buffer_bytes.div_ceil(4)],
            };
            watcher.run();
        })?;
    Ok(DirectoryWatch {
        stop,
        thread: Some(thread),
    })
}

fn open_for_watching(path: &Path) -> io::Result<File> {
    let wide = verbatim(path)?;
    // SAFETY: the path is null-terminated and outlives the call, and no security attributes or
    // template are given.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a fresh handle owned by nothing else.
    Ok(unsafe { File::from_raw_handle(handle) })
}

/// A manual-reset event, not signaled.
fn new_event() -> io::Result<OwnedHandle> {
    // SAFETY: no attributes or name are given, which is valid.
    let event: HANDLE = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
    if event.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a fresh handle owned by nothing else.
    Ok(unsafe { OwnedHandle::from_raw_handle(event) })
}

struct Watcher<F> {
    directory: File,
    stop: OwnedHandle,
    wake: OwnedHandle,
    root: PathBuf,
    sink: F,
    // Words, because the records have 4-byte alignment.
    buffer: Vec<u32>,
}

enum Waited {
    Bytes(u32),
    Overflowed,
    Stopped,
    Failed(io::Error),
}

impl<F: FnMut(WatchEvent)> Watcher<F> {
    fn run(&mut self) {
        loop {
            match self.next_changes() {
                Waited::Bytes(0) | Waited::Overflowed => (self.sink)(WatchEvent::Overflow),
                Waited::Bytes(bytes) => self.report(bytes as usize),
                Waited::Stopped => return,
                Waited::Failed(error) => {
                    (self.sink)(WatchEvent::Lost(error));
                    return;
                }
            }
        }
    }

    fn next_changes(&mut self) -> Waited {
        const CHANGES: u32 = FILE_NOTIFY_CHANGE_FILE_NAME
            | FILE_NOTIFY_CHANGE_DIR_NAME
            | FILE_NOTIFY_CHANGE_ATTRIBUTES
            | FILE_NOTIFY_CHANGE_SIZE
            | FILE_NOTIFY_CHANGE_LAST_WRITE
            | FILE_NOTIFY_CHANGE_CREATION;

        // SAFETY: the structure is plain data for which all zeroes means no offset and no event.
        let mut overlapped: OVERLAPPED = unsafe { mem::zeroed() };
        overlapped.hEvent = self.wake.as_raw_handle();
        // SAFETY: the handle was opened for overlapped I/O, the pointer and the length describe
        // `buffer`, and both it and `overlapped` stay in place until the operation has ended,
        // because every path below waits for that before returning.
        let started = unsafe {
            ReadDirectoryChangesW(
                self.directory.as_raw_handle(),
                self.buffer.as_mut_ptr().cast(),
                (self.buffer.len() * mem::size_of::<u32>()) as u32,
                1,
                CHANGES,
                ptr::null_mut(),
                &mut overlapped,
                None,
            )
        };
        if started == 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_NOTIFY_ENUM_DIR as i32) {
                Waited::Overflowed
            } else {
                Waited::Failed(error)
            };
        }

        let events = [self.wake.as_raw_handle(), self.stop.as_raw_handle()];
        // SAFETY: both events are open for as long as `self` is.
        let waited = unsafe { WaitForMultipleObjects(2, events.as_ptr(), 0, INFINITE) };
        if waited != WAIT_OBJECT_0 {
            // SAFETY: the operation above is still running, on this handle.
            unsafe { CancelIoEx(self.directory.as_raw_handle(), &overlapped) };
        }

        let mut bytes = 0;
        // SAFETY: `overlapped` is the structure of the operation started above, and waiting here
        // is what makes it safe for the buffer and that structure to go away afterwards.
        let finished = unsafe {
            GetOverlappedResult(self.directory.as_raw_handle(), &overlapped, &mut bytes, 1)
        };
        // SAFETY: as above.
        unsafe { windows_sys::Win32::System::Threading::ResetEvent(self.wake.as_raw_handle()) };

        if waited != WAIT_OBJECT_0 {
            return Waited::Stopped;
        }
        if finished != 0 {
            return Waited::Bytes(bytes);
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error().map(|code| code as u32) {
            Some(ERROR_NOTIFY_ENUM_DIR) => Waited::Overflowed,
            Some(ERROR_OPERATION_ABORTED) => Waited::Stopped,
            _ => Waited::Failed(error),
        }
    }

    fn report(&mut self, bytes: usize) {
        let header = offset_of!(FILE_NOTIFY_INFORMATION, FileName);
        let base = self.buffer.as_ptr().cast::<u8>();
        let mut offset = 0;
        let mut names: Vec<OsString> = Vec::new();
        while offset + header <= bytes {
            // SAFETY: the header fits in what the kernel wrote, as just checked, and it aligns
            // records to 4 bytes, so the cast is aligned.
            let record = unsafe { base.add(offset).cast::<FILE_NOTIFY_INFORMATION>() };
            // SAFETY: aligned and inside the buffer, and these fields are plain integers.
            let (next, name_bytes) = unsafe {
                (
                    (*record).NextEntryOffset as usize,
                    (*record).FileNameLength as usize,
                )
            };
            if offset + header + name_bytes > bytes {
                break;
            }
            // SAFETY: the name fits in what the kernel wrote, as just checked, and a pointer
            // from `base` may reach all of the buffer.
            let name = unsafe {
                slice::from_raw_parts(
                    base.add(offset + header).cast::<u16>(),
                    name_bytes / mem::size_of::<u16>(),
                )
            };
            names.push(OsString::from_wide(name));
            if next == 0 {
                break;
            }
            offset += next;
        }
        for name in names {
            (self.sink)(WatchEvent::Changed(self.root.join(name)));
        }
    }
}
