//! Windows job objects: limits that hold no matter what the process inside them does.

use std::io;
use std::mem;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    JOB_OBJECT_UILIMIT_DESKTOP, JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
    JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD,
    JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
    JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, QueryInformationJobObject,
    SetInformationJobObject,
};

/// The limits a job really has, read back from the kernel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobReport {
    /// Closing the job, which happens if the host dies, kills the process inside.
    pub kills_process_when_closed: bool,
    /// How many processes it may hold, if limited.
    pub active_process_limit: Option<u32>,
    /// How much memory the process may commit, if limited.
    pub memory_limit: Option<usize>,
    /// It may neither read nor write the clipboard.
    pub restricts_clipboard: bool,
    /// It may not use USER handles belonging to anything outside the job.
    pub restricts_foreign_user_handles: bool,
    /// It may not create or switch desktops.
    pub restricts_desktops: bool,
}

/// A job holding one process, which dies with this value, even if the host crashes without
/// cleaning up. Because the kernel closes the handle in that case, workers can never
/// outlive the host.
#[derive(Debug)]
pub(crate) struct JobObject(OwnedHandle);

impl JobObject {
    /// Creates a job that kills its process when closed. With `lock_down` it also allows
    /// exactly one process in it, so no child processes, and takes away USER objects
    /// belonging to anything outside the job.
    pub(crate) fn create(memory_limit: Option<usize>, lock_down: bool) -> io::Result<Self> {
        // SAFETY: null attributes and a null name are documented as valid.
        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: just created, so nothing else owns this handle.
        let job = Self(unsafe { OwnedHandle::from_raw_handle(handle) });

        // SAFETY: the structure is plain data for which all zeroes means "no limits".
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
        let basic = &mut limits.BasicLimitInformation;
        basic.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        if lock_down {
            basic.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            basic.ActiveProcessLimit = 1;
        }
        if let Some(limit) = memory_limit {
            basic.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
            limits.ProcessMemoryLimit = limit;
        }
        job.set(JobObjectExtendedLimitInformation, &limits)?;

        if lock_down {
            let restrictions = JOBOBJECT_BASIC_UI_RESTRICTIONS {
                UIRestrictionsClass: JOB_OBJECT_UILIMIT_DESKTOP
                    | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
                    | JOB_OBJECT_UILIMIT_EXITWINDOWS
                    | JOB_OBJECT_UILIMIT_GLOBALATOMS
                    | JOB_OBJECT_UILIMIT_HANDLES
                    | JOB_OBJECT_UILIMIT_READCLIPBOARD
                    | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
                    | JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
            };
            job.set(JobObjectBasicUIRestrictions, &restrictions)?;
        }
        Ok(job)
    }

    fn set<T>(
        &self,
        class: windows_sys::Win32::System::JobObjects::JOBOBJECTINFOCLASS,
        value: &T,
    ) -> io::Result<()> {
        // SAFETY: the caller pairs each class with the type it expects, and the pointer and
        // size describe `value` exactly.
        let ok = unsafe {
            SetInformationJobObject(
                self.handle(),
                class,
                ptr::from_ref(value).cast(),
                mem::size_of::<T>() as u32,
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Reads back the limits the kernel holds for this job.
    pub(crate) fn report(&self) -> io::Result<JobReport> {
        // SAFETY: the structure is plain data, filled in by the call below.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
        // SAFETY: as above.
        let mut restrictions: JOBOBJECT_BASIC_UI_RESTRICTIONS = unsafe { mem::zeroed() };
        self.query(JobObjectExtendedLimitInformation, &mut limits)?;
        self.query(JobObjectBasicUIRestrictions, &mut restrictions)?;
        let basic = &limits.BasicLimitInformation;
        Ok(JobReport {
            kills_process_when_closed: basic.LimitFlags & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE != 0,
            active_process_limit: (basic.LimitFlags & JOB_OBJECT_LIMIT_ACTIVE_PROCESS != 0)
                .then_some(basic.ActiveProcessLimit),
            memory_limit: (basic.LimitFlags & JOB_OBJECT_LIMIT_PROCESS_MEMORY != 0)
                .then_some(limits.ProcessMemoryLimit),
            restricts_clipboard: restrictions.UIRestrictionsClass
                & (JOB_OBJECT_UILIMIT_READCLIPBOARD | JOB_OBJECT_UILIMIT_WRITECLIPBOARD)
                == JOB_OBJECT_UILIMIT_READCLIPBOARD | JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
            restricts_foreign_user_handles: restrictions.UIRestrictionsClass
                & JOB_OBJECT_UILIMIT_HANDLES
                != 0,
            restricts_desktops: restrictions.UIRestrictionsClass & JOB_OBJECT_UILIMIT_DESKTOP != 0,
        })
    }

    fn query<T>(
        &self,
        class: windows_sys::Win32::System::JobObjects::JOBOBJECTINFOCLASS,
        value: &mut T,
    ) -> io::Result<()> {
        // SAFETY: the caller pairs each class with the type it expects, and the pointer and
        // size describe `value` exactly. A null length pointer is allowed.
        let ok = unsafe {
            QueryInformationJobObject(
                self.handle(),
                class,
                ptr::from_mut(value).cast(),
                mem::size_of::<T>() as u32,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// The job's handle, valid for as long as this value lives.
    pub(crate) fn handle(&self) -> HANDLE {
        self.0.as_raw_handle()
    }
}
