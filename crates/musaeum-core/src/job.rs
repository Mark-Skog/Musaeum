//! Windows job object: render workers can never outlive the host.

use std::io;
use std::mem;
use std::os::windows::io::AsRawHandle;
use std::process::Child;
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};

/// A job whose processes die with it, even if the host crashes without cleaning up.
#[derive(Debug)]
pub struct JobObject(HANDLE);

// SAFETY: a job handle is a process-wide kernel object with no thread affinity.
unsafe impl Send for JobObject {}

impl JobObject {
    /// Creates a job, optionally capping the committed memory of each process in it.
    pub fn create(process_memory_limit: Option<usize>) -> io::Result<Self> {
        // SAFETY: null attributes and a null name are documented as valid.
        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self(handle);

        // SAFETY: the structure is plain data for which all zeroes means "no limits".
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        if let Some(limit) = process_memory_limit {
            limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
            limits.ProcessMemoryLimit = limit;
        }
        // SAFETY: `limits` has the type and the size the information class requires.
        let ok = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                ptr::from_ref(&limits).cast(),
                mem::size_of_val(&limits) as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    /// Places a freshly spawned process in the job.
    pub fn assign(&self, child: &Child) -> io::Result<()> {
        // SAFETY: both handles are valid for the duration of the call.
        let ok = unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

impl Drop for JobObject {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by this value and is not used after this.
        unsafe { CloseHandle(self.0) };
    }
}
