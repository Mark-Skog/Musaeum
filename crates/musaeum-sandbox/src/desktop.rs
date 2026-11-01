//! A separate window station and desktop for locked-down processes.
//!
//! `pdfium.dll` imports `user32.dll`, and that connects a process to a desktop as soon as it
//! loads. On the user's desktop, a process can read the screen through a device context and
//! post input to whatever accepts it, and it keeps a handle to the user's window station with
//! full rights. In a window station of their own there is nothing to read and nobody to talk
//! to, and the clipboard and the atom table are not the user's.

use std::ffi::OsString;
use std::io;
use std::mem;
use std::ptr;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::Storage::FileSystem::READ_CONTROL;
use windows_sys::Win32::System::StationsAndDesktops::{
    CreateDesktopW, CreateWindowStationW, DESKTOP_CREATEWINDOW, DESKTOP_READOBJECTS,
    DESKTOP_WRITEOBJECTS, GetProcessWindowStation, GetUserObjectInformationW, HDESK, HWINSTA,
    SetProcessWindowStation,
};

use crate::command_line::wide;

// From `winuser.h`; `windows-sys` does not carry them.
const WINSTA_CREATEDESKTOP: u32 = 0x0008;
const WINSTA_READATTRIBUTES: u32 = 0x0002;
const UOI_NAME: i32 = 2;

/// The desktop's path, for `STARTUPINFOW::lpDesktop`. Windows destroys a window station when
/// its last handle closes. The handles are deliberately never closed, so the name stays valid
/// for new workers until this process exits.
static DESKTOP_PATH: OnceLock<Result<Vec<u16>, i32>> = OnceLock::new();

/// A security descriptor that only adds a mandatory label of low integrity, with no-write-up,
/// and leaves the access list to the default. Without this label, `user32.dll` fails to
/// initialize when a low-integrity process loads it, and takes `pdfium.dll` down with it.
struct LowIntegrityLabel(PSECURITY_DESCRIPTOR);

impl LowIntegrityLabel {
    fn new() -> io::Result<Self> {
        let description = wide(&OsString::from("S:(ML;;NW;;;LW)"));
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: the string is null-terminated, and `descriptor` is a valid out-pointer.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                description.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(descriptor))
        }
    }

    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}

impl Drop for LowIntegrityLabel {
    fn drop(&mut self) {
        // SAFETY: the conversion above allocated it with `LocalAlloc`, and nothing uses it now.
        unsafe { LocalFree(self.0.cast()) };
    }
}

/// The name of a window station, as UTF-16 without a terminator.
fn station_name(station: HWINSTA) -> io::Result<Vec<u16>> {
    let mut name = [0u16; 128];
    let mut needed = 0;
    // SAFETY: the pointer and size describe `name`, and `needed` is a valid out-pointer.
    let ok = unsafe {
        GetUserObjectInformationW(
            station,
            UOI_NAME,
            name.as_mut_ptr().cast(),
            mem::size_of_val(&name) as u32,
            &mut needed,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let len = name
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(name.len());
    Ok(name[..len].to_vec())
}

fn create() -> io::Result<Vec<u16>> {
    let desktop_name = format!("sandbox-{}", std::process::id());
    let wide_desktop = wide(&OsString::from(&desktop_name));
    let label = LowIntegrityLabel::new()?;
    let attributes = label.attributes();

    // Windows only lets an ordinary user create the window station that belongs to its logon
    // session, which a null name asks for. Another Musaeum may be using it, but the desktop
    // below is this process's alone.
    // SAFETY: a null name is documented as valid, and the attributes outlive the call.
    let station: HWINSTA = unsafe {
        CreateWindowStationW(
            ptr::null(),
            0,
            WINSTA_CREATEDESKTOP | READ_CONTROL | WINSTA_READATTRIBUTES,
            &attributes,
        )
    };
    if station.is_null() {
        return Err(io::Error::last_os_error());
    }
    let mut path = station_name(station)?;

    // `CreateDesktopW` creates the desktop in the process's current window station, and has no
    // parameter for it. Every other thread here has long been connected to its own desktop, so
    // this short switch only affects the call below.
    // SAFETY: `GetProcessWindowStation` takes no arguments, and returns a handle that must not
    // be closed. `SetProcessWindowStation` is given valid handles, and switched back below.
    unsafe {
        let original = GetProcessWindowStation();
        if original.is_null() || SetProcessWindowStation(station) == 0 {
            return Err(io::Error::last_os_error());
        }
        let desktop: HDESK = CreateDesktopW(
            wide_desktop.as_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            DESKTOP_CREATEWINDOW | DESKTOP_READOBJECTS | DESKTOP_WRITEOBJECTS | READ_CONTROL,
            &attributes,
        );
        let error = io::Error::last_os_error();
        if SetProcessWindowStation(original) == 0 {
            return Err(io::Error::last_os_error());
        }
        if desktop.is_null() {
            return Err(error);
        }
    }
    path.push(u16::from(b'\\'));
    path.extend(wide_desktop);
    Ok(path)
}

/// The null-terminated path of the shared sandbox desktop, creating it on first use.
pub(crate) fn sandbox_desktop_path() -> io::Result<&'static [u16]> {
    let result =
        DESKTOP_PATH.get_or_init(|| create().map_err(|error| error.raw_os_error().unwrap_or(0)));
    match result {
        Ok(path) => Ok(path),
        Err(code) => Err(io::Error::from_raw_os_error(*code)),
    }
}
