//! The system's file and folder pickers, the ones the File Explorer uses.
//!
//! Each runs on a thread of its own, in a single-threaded COM apartment, owned by the app's window,
//! which stays disabled while it is open. Only the path chosen comes back; nothing in the page can
//! open it, so the page never learns more of the disk than a person picks.

use std::path::{Path, PathBuf};

use musaeum_i18n::{T, text};
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT,
    FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FOS_STRICTFILETYPES, FileOpenDialog, FileSaveDialog,
    IFileDialog, IFileOpenDialog, IFileSaveDialog, IShellItem, IShellItemArray,
    SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
};
use windows::core::{HSTRING, Interface, PCWSTR};

/// What kind of file a picker shows: its name in the list, and its pattern, as `*.pdf`.
#[derive(Clone, Copy, Debug)]
pub struct FileKind {
    pub name: T,
    pub pattern: &'static str,
}

/// Asks for a folder, with `title` over the dialog. `None` if the person cancelled.
pub fn pick_folder(owner: Option<isize>, title: &str) -> Result<Option<PathBuf>, String> {
    let title = title.to_owned();
    on_own_thread("folder-picker", move || {
        show(
            owner,
            |dialog: &IFileDialog| {
                // SAFETY: plain COM calls on a dialog this thread created.
                unsafe {
                    let options = dialog.GetOptions()?;
                    dialog.SetOptions(
                        options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST,
                    )?;
                    dialog.SetTitle(&HSTRING::from(title.as_str()))?;
                }
                Ok(())
            },
            false,
        )
    })
}

/// Asks for one or more folders, with `title` over the dialog. Empty if the person cancelled.
pub fn pick_folders(owner: Option<isize>, title: &str) -> Result<Vec<PathBuf>, String> {
    let title = title.to_owned();
    on_own_thread("folders-picker", move || {
        // SAFETY: COM is set up on this thread, which is ours alone and ends after the dialog, and
        // undone below on every path through this function.
        let initialized =
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        if initialized.is_err() {
            return Err(format!("COM could not start: {initialized:?}"));
        }
        let result = (|| -> windows::core::Result<Vec<PathBuf>> {
            // SAFETY: plain COM calls on interfaces this thread created; every pointer returned
            // is owned by the wrappers or freed with CoTaskMemFree right after use.
            unsafe {
                let open_dialog: IFileOpenDialog =
                    CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
                let options = open_dialog.GetOptions()?;
                open_dialog.SetOptions(
                    options
                        | FOS_PICKFOLDERS
                        | FOS_ALLOWMULTISELECT
                        | FOS_FORCEFILESYSTEM
                        | FOS_PATHMUSTEXIST,
                )?;
                open_dialog.SetTitle(&HSTRING::from(title.as_str()))?;
                let parent = owner.map(|handle| HWND(handle as *mut core::ffi::c_void));
                if let Err(error) = open_dialog.Show(parent) {
                    if error.code() == ERROR_CANCELLED.to_hresult() {
                        return Ok(Vec::new());
                    }
                    return Err(error);
                }
                let items: IShellItemArray = open_dialog.GetResults()?;
                let count = items.GetCount()?;
                let mut paths = Vec::with_capacity(count as usize);
                for index in 0..count {
                    let item = items.GetItemAt(index)?;
                    let name = item.GetDisplayName(SIGDN_FILESYSPATH)?;
                    let path = name.to_string();
                    CoTaskMemFree(Some(name.0 as *const core::ffi::c_void));
                    if let Ok(path) = path {
                        paths.push(PathBuf::from(path));
                    }
                }
                Ok(paths)
            }
        })();
        // SAFETY: matches the successful CoInitializeEx above.
        unsafe { CoUninitialize() };
        result.map_err(|error| error.to_string())
    })
}

/// Asks for a file of `kind` to open, starting in `folder`. `None` if the person cancelled.
pub fn pick_file(
    owner: Option<isize>,
    title: &str,
    kind: FileKind,
    folder: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    let title = title.to_owned();
    let folder = folder.map(Path::to_path_buf);
    on_own_thread("file-picker", move || {
        show(
            owner,
            |dialog: &IFileDialog| {
                prepare(dialog, &title, kind, folder.as_deref(), FOS_FILEMUSTEXIST)
            },
            false,
        )
    })
}

/// Asks where to save a file of `kind`, offering `name` in `folder`; asks again before writing
/// over a file that is there. `None` if the person cancelled.
pub fn pick_save(
    owner: Option<isize>,
    title: &str,
    kind: FileKind,
    folder: Option<&Path>,
    name: &str,
) -> Result<Option<PathBuf>, String> {
    let title = title.to_owned();
    let folder = folder.map(Path::to_path_buf);
    let name = name.to_owned();
    on_own_thread("save-picker", move || {
        show(
            owner,
            |dialog: &IFileDialog| {
                prepare(dialog, &title, kind, folder.as_deref(), FOS_OVERWRITEPROMPT)?;
                let extension = kind.pattern.trim_start_matches("*.");
                // SAFETY: plain COM calls on a dialog this thread created.
                unsafe {
                    dialog.SetFileName(&HSTRING::from(name.as_str()))?;
                    dialog.SetDefaultExtension(&HSTRING::from(extension))?;
                }
                Ok(())
            },
            true,
        )
    })
}

fn prepare(
    dialog: &IFileDialog,
    title: &str,
    kind: FileKind,
    folder: Option<&Path>,
    extra: windows::Win32::UI::Shell::FILEOPENDIALOGOPTIONS,
) -> windows::core::Result<()> {
    let name = HSTRING::from(text(kind.name, &[]));
    let pattern = HSTRING::from(kind.pattern);
    let filters = [COMDLG_FILTERSPEC {
        pszName: PCWSTR(name.as_ptr()),
        pszSpec: PCWSTR(pattern.as_ptr()),
    }];
    // SAFETY: plain COM calls on a dialog this thread created; the strings the filter points to
    // live until the dialog has copied them, which `SetFileTypes` does.
    unsafe {
        let options = dialog.GetOptions()?;
        dialog.SetOptions(
            options | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST | FOS_STRICTFILETYPES | extra,
        )?;
        dialog.SetTitle(&HSTRING::from(title))?;
        dialog.SetFileTypes(&filters)?;
        if let Some(folder) = folder {
            let item: windows::core::Result<IShellItem> =
                SHCreateItemFromParsingName(&HSTRING::from(folder.as_os_str()), None);
            if let Ok(item) = item {
                dialog.SetFolder(&item)?;
            }
        }
    }
    Ok(())
}

fn on_own_thread<R: Send + 'static>(
    name: &str,
    run: impl FnOnce() -> Result<R, String> + Send + 'static,
) -> Result<R, String> {
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(run)
        .map_err(|error| error.to_string())?
        .join()
        .map_err(|_| "the picker stopped".to_owned())?
}

fn show(
    owner: Option<isize>,
    set_up: impl FnOnce(&IFileDialog) -> windows::core::Result<()>,
    save: bool,
) -> Result<Option<PathBuf>, String> {
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
            let dialog: IFileDialog = if save {
                CoCreateInstance::<_, IFileSaveDialog>(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?
                    .cast()?
            } else {
                CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?
                    .cast()?
            };
            set_up(&dialog)?;
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
