//! What a locked-down process does to itself, once it has loaded what it needs.

use std::io;

use windows_sys::Win32::Security::RevertToSelf;
use windows_sys::Win32::System::Diagnostics::Debug::{
    SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX, SetErrorMode,
};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RegCloseKey};

/// A worker that dies must simply die: no crash dialog, no Windows Error Reporting prompt
/// waiting on a user who cannot see this process. The host restarts it.
pub fn die_quietly() {
    // SAFETY: `SetErrorMode` has no preconditions.
    unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX)
    };
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        std::process::abort();
    }));
}

/// Drops the startup token, leaving only the lockdown token. There is no way back: nothing in
/// this process holds a handle to the startup token, and the lockdown token cannot open one.
///
/// Must run before the process reads any untrusted input. Harmless in a process that was not
/// started in a sandbox, where there is nothing to drop.
pub fn enter_lockdown() -> io::Result<()> {
    // SAFETY: `RevertToSelf` has no preconditions, and applies to the calling thread.
    if unsafe { RevertToSelf() } == 0 {
        return Err(io::Error::last_os_error());
    }
    // Handles opened during startup keep the rights they were opened with, and system DLLs
    // cache these two when they load. Closing a predefined key only empties that cache, so
    // later use tries to open it again, and fails like everything else does.
    // SAFETY: `RegCloseKey` takes these predefined values, and fails harmlessly if unopened.
    unsafe {
        RegCloseKey(HKEY_CURRENT_USER);
        RegCloseKey(HKEY_LOCAL_MACHINE);
    }
    Ok(())
}
