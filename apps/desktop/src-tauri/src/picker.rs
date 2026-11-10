//! The system's folder picker, the one the File Explorer uses.
//!
//! It runs on a thread of its own, in a single-threaded COM apartment, owned by the app's window,
//! which stays disabled while it is open. Only the path chosen comes back; nothing in the page can
//! open it, so the page never learns more of the disk than a person picks.

use std::path::PathBuf;

use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog,
    SIGDN_FILESYSPATH,
};
use windows::core::HSTRING;

/// Asks for a folder, with `title` over the dialog. `None` if the person cancelled.
pub fn pick_folder(owner: Option<isize>, title: &str) -> Result<Option<PathBuf>, String> {
    let title = title.to_owned();
    std::thread::Builder::new()
        .name("folder-picker".to_owned())
        .spawn(move || show(owner, &title))
        .map_err(|error| error.to_string())?
        .join()
        .map_err(|_| "the folder picker stopped".to_owned())?
}

fn show(owner: Option<isize>, title: &str) -> Result<Option<PathBuf>, String> {
    // SAFETY: COM is set up on this thread, which is ours alone and ends after the dialog, and
    // undone below on every path through this function.
    let initialized =
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    if initialized.is_err() {
        return Err(format!("COM could not start: {initialized:?}"));
    }
    let result = (|| -> windows::core::Result<Option<PathBuf>> {
        // SAFETY: plain COM calls on interfaces this thread created; every pointer they return is
        // owned by the wrappers or freed with CoTaskMemFree right after use.
        unsafe {
            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            let options = dialog.GetOptions()?;
            dialog
                .SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST)?;
            dialog.SetTitle(&HSTRING::from(title))?;
            let parent = owner.map(|handle| HWND(handle as *mut core::ffi::c_void));
            if let Err(error) = dialog.Show(parent) {
                if error.code() == ERROR_CANCELLED.to_hresult() {
                    return Ok(None);
                }
                return Err(error);
            }
            let item = dialog.GetResult()?;
            let name = item.GetDisplayName(SIGDN_FILESYSPATH)?;
            let path = name.to_string();
            CoTaskMemFree(Some(name.0 as *const core::ffi::c_void));
            Ok(path.ok().map(PathBuf::from))
        }
    })();
    // SAFETY: matches the successful CoInitializeEx above.
    unsafe { CoUninitialize() };
    result.map_err(|error| error.to_string())
}
