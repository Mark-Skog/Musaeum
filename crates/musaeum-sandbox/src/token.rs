//! The two tokens a locked-down process runs with.
//!
//! The **lockdown** token is its primary token and what it is left with. Windows checks a
//! restricted token twice, and access needs both checks to pass: once against its normal
//! groups, and once against its restricting SIDs. Here every group is deny-only, so the first
//! check grants nothing that names a group, and the only restricting SID is the NULL SID, which
//! no ACL ever grants anything to. Without the privilege to bypass traversal checks, walking
//! down a path fails too, including the object-manager paths that drives live under. That
//! is what also keeps it out of volumes with no ACLs of their own.
//!
//! The **startup** token only exists so the process can load its DLLs. It has the user's
//! access but no privileges, and runs at low integrity. It is impersonated by the main
//! thread alone, until [`crate::target::enter_lockdown`] drops it.

use std::ffi::c_void;
use std::io;
use std::mem;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, GetLastError, HANDLE, LUID};
use windows_sys::Win32::Security::{
    CreateRestrictedToken, CreateWellKnownSid, DuplicateTokenEx, GetTokenInformation,
    LUID_AND_ATTRIBUTES, PSID, SECURITY_MAX_SID_SIZE, SID_AND_ATTRIBUTES, SecurityImpersonation,
    SetTokenInformation, TOKEN_ADJUST_DEFAULT, TOKEN_ALL_ACCESS, TOKEN_ASSIGN_PRIMARY,
    TOKEN_DUPLICATE, TOKEN_GROUPS, TOKEN_IMPERSONATE, TOKEN_INFORMATION_CLASS,
    TOKEN_MANDATORY_LABEL, TOKEN_PRIVILEGES, TOKEN_QUERY, TOKEN_USER, TokenGroups,
    TokenImpersonation, TokenIntegrityLevel, TokenPrivileges, TokenUser, WELL_KNOWN_SID_TYPE,
    WinLowLabelSid, WinNullSid,
};
use windows_sys::Win32::System::SystemServices::{SE_GROUP_ENABLED, SE_GROUP_INTEGRITY};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// `SE_CHANGE_NOTIFY_NAME`'s well-known LUID, the privilege that skips traversal checks.
const CHANGE_NOTIFY_LUID: u32 = 23;

pub(crate) struct Tokens {
    /// The process's primary token.
    pub lockdown: OwnedHandle,
    /// The main thread's impersonation token during startup.
    pub startup: OwnedHandle,
}

pub(crate) fn build() -> io::Result<Tokens> {
    let base = current_process_token()?;
    let groups = read_info::<TOKEN_GROUPS>(&base, TokenGroups)?;
    // SAFETY: `read_info` returned a buffer the kernel filled for this class, whose array
    // really has `GroupCount` entries.
    let groups = unsafe {
        std::slice::from_raw_parts(
            (*groups.as_ptr()).Groups.as_ptr(),
            (*groups.as_ptr()).GroupCount as usize,
        )
    };
    let user = read_info::<TOKEN_USER>(&base, TokenUser)?;
    // SAFETY: as above; the structure is fully initialized.
    let user = unsafe { (*user.as_ptr()).User };
    let privileges = read_info::<TOKEN_PRIVILEGES>(&base, TokenPrivileges)?;
    // SAFETY: as above, with `PrivilegeCount` entries.
    let privileges = unsafe {
        std::slice::from_raw_parts(
            (*privileges.as_ptr()).Privileges.as_ptr(),
            (*privileges.as_ptr()).PrivilegeCount as usize,
        )
    };

    Ok(Tokens {
        lockdown: lockdown_token(&base, groups, user, privileges)?,
        startup: startup_token(&base, groups, user, privileges)?,
    })
}

fn lockdown_token(
    base: &OwnedHandle,
    groups: &[SID_AND_ATTRIBUTES],
    user: SID_AND_ATTRIBUTES,
    privileges: &[LUID_AND_ATTRIBUTES],
) -> io::Result<OwnedHandle> {
    let mut deny_only: Vec<SID_AND_ATTRIBUTES> = groups
        .iter()
        .filter(|group| group.Attributes & SE_GROUP_INTEGRITY as u32 == 0)
        .map(|group| SID_AND_ATTRIBUTES {
            Sid: group.Sid,
            Attributes: 0,
        })
        .collect();
    deny_only.push(SID_AND_ATTRIBUTES {
        Sid: user.Sid,
        Attributes: 0,
    });

    let mut null_sid = WellKnownSid::new(WinNullSid)?;
    let restricting = [SID_AND_ATTRIBUTES {
        Sid: null_sid.as_psid(),
        Attributes: 0,
    }];

    let token = restrict(base, &deny_only, privileges, &restricting)?;
    set_low_integrity(&token)?;
    Ok(token)
}

fn startup_token(
    base: &OwnedHandle,
    groups: &[SID_AND_ATTRIBUTES],
    user: SID_AND_ATTRIBUTES,
    privileges: &[LUID_AND_ATTRIBUTES],
) -> io::Result<OwnedHandle> {
    // A restricted token that lists everything the user has, so it keeps the same access,
    // which is what lets the primary token above accept it as an impersonation token.
    let mut restricting: Vec<SID_AND_ATTRIBUTES> = groups
        .iter()
        .filter(|group| {
            group.Attributes & SE_GROUP_INTEGRITY as u32 == 0
                && group.Attributes & SE_GROUP_ENABLED as u32 != 0
        })
        .map(|group| SID_AND_ATTRIBUTES {
            Sid: group.Sid,
            Attributes: 0,
        })
        .collect();
    restricting.push(SID_AND_ATTRIBUTES {
        Sid: user.Sid,
        Attributes: 0,
    });

    // Loading a DLL from below a folder the user cannot list needs this one privilege.
    let removable: Vec<LUID_AND_ATTRIBUTES> = privileges
        .iter()
        .filter(|privilege| !is_change_notify(privilege.Luid))
        .copied()
        .collect();

    let restricted = restrict(base, &[], &removable, &restricting)?;
    let mut impersonation: HANDLE = ptr::null_mut();
    // SAFETY: the source is a valid token with duplicate access, the attributes may be null,
    // and `impersonation` is a valid out-pointer.
    let ok = unsafe {
        DuplicateTokenEx(
            restricted.as_raw_handle(),
            TOKEN_ALL_ACCESS,
            ptr::null(),
            SecurityImpersonation,
            TokenImpersonation,
            &mut impersonation,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call above succeeded, so this is a fresh handle owned by nothing else.
    let impersonation = unsafe { OwnedHandle::from_raw_handle(impersonation) };
    set_low_integrity(&impersonation)?;
    Ok(impersonation)
}

fn is_change_notify(luid: LUID) -> bool {
    luid.HighPart == 0 && luid.LowPart == CHANGE_NOTIFY_LUID
}

fn current_process_token() -> io::Result<OwnedHandle> {
    let mut token: HANDLE = ptr::null_mut();
    let access = TOKEN_DUPLICATE
        | TOKEN_ASSIGN_PRIMARY
        | TOKEN_QUERY
        | TOKEN_ADJUST_DEFAULT
        | TOKEN_IMPERSONATE;
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle, and `token` is a valid out-pointer.
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), access, &mut token) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call above succeeded, so this is a fresh handle owned by nothing else.
    Ok(unsafe { OwnedHandle::from_raw_handle(token) })
}

fn restrict(
    base: &OwnedHandle,
    deny_only: &[SID_AND_ATTRIBUTES],
    remove_privileges: &[LUID_AND_ATTRIBUTES],
    restricting: &[SID_AND_ATTRIBUTES],
) -> io::Result<OwnedHandle> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: each pointer and count describes a live slice, the SIDs they hold outlive the
    // call, and `token` is a valid out-pointer.
    let ok = unsafe {
        CreateRestrictedToken(
            base.as_raw_handle(),
            0,
            deny_only.len() as u32,
            deny_only.as_ptr(),
            remove_privileges.len() as u32,
            remove_privileges.as_ptr(),
            restricting.len() as u32,
            restricting.as_ptr(),
            &mut token,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call above succeeded, so this is a fresh handle owned by nothing else.
    Ok(unsafe { OwnedHandle::from_raw_handle(token) })
}

fn set_low_integrity(token: &OwnedHandle) -> io::Result<()> {
    let mut low = WellKnownSid::new(WinLowLabelSid)?;
    let label = TOKEN_MANDATORY_LABEL {
        Label: SID_AND_ATTRIBUTES {
            Sid: low.as_psid(),
            Attributes: SE_GROUP_INTEGRITY as u32,
        },
    };
    // SAFETY: `label` has the type and size this class needs, and its SID outlives the call.
    let ok = unsafe {
        SetTokenInformation(
            token.as_raw_handle(),
            TokenIntegrityLevel,
            ptr::from_ref(&label).cast::<c_void>(),
            mem::size_of_val(&label) as u32,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

struct WellKnownSid {
    bytes: [u8; SECURITY_MAX_SID_SIZE as usize],
}

impl WellKnownSid {
    fn new(kind: WELL_KNOWN_SID_TYPE) -> io::Result<Self> {
        let mut sid = Self {
            bytes: [0; SECURITY_MAX_SID_SIZE as usize],
        };
        let mut size = SECURITY_MAX_SID_SIZE;
        // SAFETY: the buffer holds `size` bytes, and neither of these SIDs needs a domain.
        let ok = unsafe {
            CreateWellKnownSid(
                kind,
                ptr::null_mut(),
                sid.bytes.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(sid)
    }

    fn as_psid(&mut self) -> PSID {
        self.bytes.as_mut_ptr().cast()
    }
}

/// A variable-length structure the kernel filled in, kept aligned for its own type.
pub(crate) struct TokenInfo<T> {
    storage: Vec<u64>,
    kind: std::marker::PhantomData<T>,
}

impl<T> TokenInfo<T> {
    pub(crate) fn as_ptr(&self) -> *const T {
        self.storage.as_ptr().cast()
    }
}

/// Reads one class of information about a token.
pub(crate) fn read_info<T>(
    token: &OwnedHandle,
    class: TOKEN_INFORMATION_CLASS,
) -> io::Result<TokenInfo<T>> {
    let mut needed = 0u32;
    // SAFETY: a null buffer of size 0 is how the call reports the size it needs.
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            class,
            ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    // SAFETY: reads the calling thread's last-error value, which the call above set.
    if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
        return Err(io::Error::last_os_error());
    }
    let mut storage = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: the buffer has at least `needed` bytes and is aligned for any token structure.
    let ok = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            class,
            storage.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(TokenInfo {
        storage,
        kind: std::marker::PhantomData,
    })
}
