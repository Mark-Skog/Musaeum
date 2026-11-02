//! Facts about the volume a path is on.

use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumePathNameW};
use windows_sys::Win32::System::WindowsProgramming::DRIVE_FIXED;

use crate::path::verbatim;

/// Whether deleting from this volume through the shell goes to a Recycle Bin. Only fixed disks
/// have one: on a USB stick or a network share, the same call destroys the file for good.
pub fn has_recycle_bin(path: &Path) -> bool {
    let Ok(wide) = verbatim(path) else {
        return false;
    };
    let mut root = vec![0u16; wide.len() + 1];
    // SAFETY: the path is null-terminated, and the pointer and the length describe `root`.
    let ok = unsafe { GetVolumePathNameW(wide.as_ptr(), root.as_mut_ptr(), root.len() as u32) };
    if ok == 0 {
        return false;
    }
    // SAFETY: on success, `root` holds a null-terminated path.
    unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_FIXED }
}
