//! Reads back, from outside, what a sandboxed process really runs with.
//!
//! This is what makes the sandbox checkable: instead of trusting that the calls above did
//! what was intended, the host and the tests ask the kernel.

use std::io;
use std::os::windows::io::{AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::ptr;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Foundation::{ERROR_NO_TOKEN, HANDLE};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{
    IsTokenRestricted, TOKEN_GROUPS, TOKEN_MANDATORY_LABEL, TOKEN_PRIVILEGES, TOKEN_QUERY,
    TOKEN_USER, TokenGroups, TokenIntegrityLevel, TokenPrivileges, TokenRestrictedSids, TokenUser,
};
use windows_sys::Win32::System::JobObjects::IsProcessInJob;
use windows_sys::Win32::System::SystemServices::SE_GROUP_USE_FOR_DENY_ONLY;
use windows_sys::Win32::System::Threading::{OpenProcessToken, OpenThreadToken};

use crate::job::JobReport;
use crate::mitigation::MitigationReport;
use crate::token::read_info;

/// The primary token of a process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenReport {
    /// Whether the kernel treats it as a restricted token.
    pub restricted: bool,
    /// Its restricting SIDs, as strings.
    pub restricting_sids: Vec<String>,
    /// Its integrity level's SID, as a string. `S-1-16-4096` is low.
    pub integrity_sid: String,
    /// How many privileges it has left.
    pub privileges: usize,
    /// How many of its groups can still grant access.
    pub usable_groups: usize,
    /// Whether the user's own SID can still grant access.
    pub user_is_usable: bool,
}

/// Everything that can be checked about a sandboxed process from the outside.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxReport {
    /// Its primary token.
    pub token: TokenReport,
    /// Whether it is inside a job object.
    pub in_job: bool,
    /// The limits of its job.
    pub job: JobReport,
    /// Whether its main thread still holds the startup token.
    pub main_thread_impersonating: bool,
    /// Its mitigation policies.
    pub mitigations: MitigationReport,
}

fn sid_string(sid: *mut core::ffi::c_void) -> io::Result<String> {
    let mut text: *mut u16 = ptr::null_mut();
    // SAFETY: `sid` comes from a token structure that outlives this call, and `text` is a
    // valid out-pointer.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: on success `text` is a null-terminated string, which `LocalFree` releases.
    unsafe {
        let len = (0..).take_while(|&at| *text.add(at) != 0).count();
        let result = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text.cast());
        Ok(result)
    }
}

impl TokenReport {
    /// Reads the primary token of `process`, which needs query access.
    pub fn read(process: BorrowedHandle<'_>) -> io::Result<Self> {
        let mut raw: HANDLE = ptr::null_mut();
        // SAFETY: the borrow keeps the process handle open, and `raw` is a valid out-pointer.
        if unsafe { OpenProcessToken(process.as_raw_handle(), TOKEN_QUERY, &mut raw) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the call above succeeded, so this is a fresh handle owned by nothing else.
        let token = unsafe { OwnedHandle::from_raw_handle(raw) };

        // SAFETY: the handle is valid and has query access.
        let restricted = unsafe { IsTokenRestricted(token.as_raw_handle()) } != 0;

        let restricting = read_info::<TOKEN_GROUPS>(&token, TokenRestrictedSids)?;
        // SAFETY: the kernel filled this buffer, and its array has `GroupCount` entries.
        let restricting_sids = unsafe {
            std::slice::from_raw_parts(
                (*restricting.as_ptr()).Groups.as_ptr(),
                (*restricting.as_ptr()).GroupCount as usize,
            )
        }
        .iter()
        .map(|group| sid_string(group.Sid))
        .collect::<io::Result<_>>()?;

        let label = read_info::<TOKEN_MANDATORY_LABEL>(&token, TokenIntegrityLevel)?;
        // SAFETY: the kernel filled this buffer for this class.
        let integrity_sid = sid_string(unsafe { (*label.as_ptr()).Label.Sid })?;

        let privileges = read_info::<TOKEN_PRIVILEGES>(&token, TokenPrivileges)?;
        // SAFETY: the kernel filled this buffer for this class.
        let privileges = unsafe { (*privileges.as_ptr()).PrivilegeCount } as usize;

        let groups = read_info::<TOKEN_GROUPS>(&token, TokenGroups)?;
        // SAFETY: the kernel filled this buffer, and its array has `GroupCount` entries.
        let usable_groups = unsafe {
            std::slice::from_raw_parts(
                (*groups.as_ptr()).Groups.as_ptr(),
                (*groups.as_ptr()).GroupCount as usize,
            )
        }
        .iter()
        .filter(|group| group.Attributes & SE_GROUP_USE_FOR_DENY_ONLY as u32 == 0)
        .count();

        let user = read_info::<TOKEN_USER>(&token, TokenUser)?;
        // SAFETY: the kernel filled this buffer for this class.
        let user_attributes = unsafe { (*user.as_ptr()).User.Attributes };

        Ok(Self {
            restricted,
            restricting_sids,
            integrity_sid,
            privileges,
            usable_groups,
            user_is_usable: user_attributes & SE_GROUP_USE_FOR_DENY_ONLY as u32 == 0,
        })
    }
}

/// Reads everything about a process, given handles with query access.
pub(crate) fn read_report(
    process: BorrowedHandle<'_>,
    main_thread: BorrowedHandle<'_>,
    job: JobReport,
) -> io::Result<SandboxReport> {
    let mut in_job = 0;
    // SAFETY: the borrow keeps the handle open, a null job means "any job", and `in_job` is
    // a valid out-pointer.
    if unsafe { IsProcessInJob(process.as_raw_handle(), ptr::null_mut(), &mut in_job) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(SandboxReport {
        token: TokenReport::read(process)?,
        in_job: in_job != 0,
        job,
        main_thread_impersonating: thread_is_impersonating(main_thread)?,
        mitigations: MitigationReport::read(process)?,
    })
}

/// Whether a thread has an impersonation token.
pub(crate) fn thread_is_impersonating(thread: BorrowedHandle<'_>) -> io::Result<bool> {
    let mut raw: HANDLE = ptr::null_mut();
    // SAFETY: the borrow keeps the handle open, and `raw` is a valid out-pointer. The check
    // uses this process's own identity, because the token may belong to anything.
    if unsafe { OpenThreadToken(thread.as_raw_handle(), TOKEN_QUERY, 1, &mut raw) } != 0 {
        // SAFETY: the call above succeeded, so this is a fresh handle owned by nothing else.
        drop(unsafe { OwnedHandle::from_raw_handle(raw) });
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_NO_TOKEN as i32) {
        Ok(false)
    } else {
        Err(error)
    }
}
