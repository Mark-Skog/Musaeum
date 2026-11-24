//! Starts a process inside the sandbox, or outside it for comparison.

use std::ffi::{OsString, c_void};
use std::fs::File;
use std::io;
use std::mem;
use std::os::windows::io::{
    AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle, RawHandle,
};
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{
    HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW,
    CreateProcessW, DETACHED_PROCESS, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_JOB_LIST, PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY, PROCESS_INFORMATION,
    ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, SetThreadToken, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

use crate::audit::ProcessAudit;
use crate::command_line::{command_line, minimal_environment, wide};
use crate::desktop::sandbox_desktop_path;
use crate::inspect::{SandboxReport, read_report, thread_is_impersonating};
use crate::job::JobObject;
use crate::mitigation::{CHILD_PROCESS_RESTRICTED, LOCKDOWN_POLICY, PARSER_POLICY};
use crate::token;

/// How much a process is allowed to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// The full sandbox. This is what anything reading untrusted bytes must run under.
    Lockdown,
    /// The full sandbox without win32k or DLLs Microsoft did not sign, for a process that only
    /// parses. `pdfium.dll` needs both, so a renderer cannot use this.
    LockdownParser,
    /// An ordinary child process of the host, in a job so it cannot outlive it. Exists for
    /// comparing against the sandbox, in tests and benchmarks.
    Unrestricted,
}

/// What to start.
#[derive(Clone, Copy, Debug)]
pub struct SpawnConfig<'a> {
    /// The executable.
    pub exe: &'a Path,
    /// Its arguments.
    pub args: &'a [OsString],
    /// How much it may do.
    pub policy: Policy,
    /// Cap on its committed memory.
    pub memory_limit: Option<usize>,
}

/// A running process, with its stdio pipes. Dropping this kills it.
#[derive(Debug)]
pub struct SandboxedChild {
    process: OwnedHandle,
    main_thread: OwnedHandle,
    // Closing the job kills the process, so this has to stay for as long as the process does.
    _job: JobObject,
    id: u32,
    /// Writes to the process's stdin.
    pub stdin: Option<File>,
    /// Reads from the process's stdout.
    pub stdout: Option<File>,
    /// Reads from the process's stderr.
    pub stderr: Option<File>,
}

impl SandboxedChild {
    /// The process id.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Asks the OS to end the process. Does not wait for that to happen.
    pub fn kill(&self) {
        // SAFETY: the handle is valid and has terminate access. A process that already
        // exited makes this fail harmlessly.
        unsafe { TerminateProcess(self.process.as_raw_handle(), 1) };
    }

    /// The exit code, if the process has ended.
    pub fn try_wait(&self) -> io::Result<Option<u32>> {
        self.wait_for(0)
    }

    /// Blocks until the process ends and returns its exit code.
    pub fn wait(&self) -> io::Result<u32> {
        self.wait_for(INFINITE)?
            .ok_or_else(|| io::Error::other("waiting forever timed out"))
    }

    fn wait_for(&self, timeout_ms: u32) -> io::Result<Option<u32>> {
        // SAFETY: the handle is valid and has synchronize access.
        let result = unsafe { WaitForSingleObject(self.process.as_raw_handle(), timeout_ms) };
        match result {
            WAIT_OBJECT_0 => {
                let mut code = 0;
                // SAFETY: the handle is valid, and `code` is a valid out-pointer.
                if unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Some(code))
            }
            WAIT_TIMEOUT => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }

    /// Whether the main thread still holds the startup token. It must not, from the moment
    /// the process says it is ready.
    pub fn is_impersonating(&self) -> io::Result<bool> {
        thread_is_impersonating(self.main_thread.as_handle())
    }

    /// Reads, as anyone outside could, its token, mitigations and every handle it holds.
    pub fn audit(&self) -> io::Result<ProcessAudit> {
        ProcessAudit::of(self.process.as_handle())
    }

    /// Reads back what this process really runs with.
    pub fn report(&self) -> io::Result<SandboxReport> {
        read_report(
            self.process.as_handle(),
            self.main_thread.as_handle(),
            self._job.report()?,
        )
    }
}

impl AsHandle for SandboxedChild {
    fn as_handle(&self) -> BorrowedHandle<'_> {
        self.process.as_handle()
    }
}

impl Drop for SandboxedChild {
    fn drop(&mut self) {
        self.kill();
    }
}

/// An attribute list, plus the values its entries point to, which must outlive it.
struct AttributeList {
    storage: Vec<u64>,
}

impl AttributeList {
    fn new(count: u32) -> io::Result<Self> {
        let mut size = 0;
        // SAFETY: a null list is how the call reports the size it needs. It fails with
        // "insufficient buffer" by design, so the result is not checked here.
        unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), count, 0, &mut size) };
        let mut list = Self {
            storage: vec![0; size.div_ceil(8)],
        };
        // SAFETY: the buffer is as large as the call above asked for, and aligned.
        let ok = unsafe { InitializeProcThreadAttributeList(list.as_raw(), count, 0, &mut size) };
        if ok == 0 {
            let error = io::Error::last_os_error();
            // An empty list tells `drop` that there is nothing to delete.
            list.storage.clear();
            return Err(error);
        }
        Ok(list)
    }

    fn as_raw(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_mut_ptr().cast()
    }

    /// Adds an attribute.
    ///
    /// # Safety
    ///
    /// `value` must stay valid until this list has been used to create a process, and must
    /// have the type `attribute` expects.
    unsafe fn set<T>(&mut self, attribute: usize, value: *const T, size: usize) -> io::Result<()> {
        // SAFETY: the list is initialized, and the caller vouches for `value`.
        let ok = unsafe {
            UpdateProcThreadAttribute(
                self.as_raw(),
                0,
                attribute,
                value.cast::<c_void>(),
                size,
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        if !self.storage.is_empty() {
            // SAFETY: the list was initialized and is not used after this.
            unsafe { DeleteProcThreadAttributeList(self.as_raw()) };
        }
    }
}

/// One end of a pipe goes to the child, one stays here.
struct Pipe {
    child_end: OwnedHandle,
    our_end: File,
}

impl Pipe {
    /// A pipe whose reading end goes to the child.
    fn towards_child() -> io::Result<Self> {
        let (read, write) = create_pipe()?;
        allow_inheritance(&read)?;
        Ok(Self {
            child_end: read,
            our_end: File::from(write),
        })
    }

    /// A pipe whose writing end goes to the child.
    fn from_child() -> io::Result<Self> {
        let (read, write) = create_pipe()?;
        allow_inheritance(&write)?;
        Ok(Self {
            child_end: write,
            our_end: File::from(read),
        })
    }
}

fn create_pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let mut read: HANDLE = ptr::null_mut();
    let mut write: HANDLE = ptr::null_mut();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 0,
    };
    // SAFETY: both out-pointers and the attributes are valid for the call. Size 0 means the
    // default buffer.
    if unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: on success these are fresh handles owned by nothing else.
    Ok(unsafe {
        (
            OwnedHandle::from_raw_handle(read),
            OwnedHandle::from_raw_handle(write),
        )
    })
}

/// Marks a handle inheritable. The handle list below keeps it from reaching anything other
/// than the intended child, and it is closed right after the child is created.
fn allow_inheritance(handle: &OwnedHandle) -> io::Result<()> {
    // SAFETY: the handle is valid for the duration of the call.
    let ok = unsafe {
        SetHandleInformation(
            handle.as_raw_handle(),
            HANDLE_FLAG_INHERIT,
            HANDLE_FLAG_INHERIT,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// The null-terminated path of `%SystemRoot%\\System32`.
fn system_directory() -> io::Result<Vec<u16>> {
    let mut buffer = [0u16; 260];
    // SAFETY: the pointer and the length describe `buffer`.
    let len = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if len == 0 || len >= buffer.len() {
        return Err(io::Error::last_os_error());
    }
    Ok(buffer[..=len].to_vec())
}

/// Starts `config.exe`. A locked-down process gets three inherited pipes and nothing else:
/// any other handle it ever holds is one the host chose to duplicate into it.
pub fn spawn(config: SpawnConfig<'_>) -> io::Result<SandboxedChild> {
    let lock_down = config.policy != Policy::Unrestricted;
    let mitigations = match config.policy {
        Policy::LockdownParser => PARSER_POLICY,
        Policy::Lockdown | Policy::Unrestricted => LOCKDOWN_POLICY,
    };
    let mut command = command_line(config.exe, config.args)?;
    let exe = wide(config.exe.as_os_str());
    let job = JobObject::create(config.memory_limit, lock_down)?;

    let stdin = Pipe::towards_child()?;
    let stdout = Pipe::from_child()?;
    let stderr = Pipe::from_child()?;

    let inherited: [RawHandle; 3] = [
        stdin.child_end.as_raw_handle(),
        stdout.child_end.as_raw_handle(),
        stderr.child_end.as_raw_handle(),
    ];
    let job_list: [HANDLE; 1] = [job.handle()];
    let child_policy: u32 = CHILD_PROCESS_RESTRICTED;

    let mut attributes = AttributeList::new(if lock_down { 4 } else { 2 })?;
    // SAFETY: every value below is a local that outlives the `CreateProcess` call, and has
    // the type its attribute documents.
    unsafe {
        attributes.set(
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            inherited.as_ptr(),
            mem::size_of_val(&inherited),
        )?;
        attributes.set(
            PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
            job_list.as_ptr(),
            mem::size_of_val(&job_list),
        )?;
        if lock_down {
            attributes.set(
                PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY as usize,
                mitigations.as_ptr(),
                mem::size_of_val(&mitigations),
            )?;
            attributes.set(
                PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY as usize,
                ptr::from_ref(&child_policy),
                mem::size_of_val(&child_policy),
            )?;
        }
    }

    // SAFETY: the structure is plain data for which all zeroes means "no options".
    let mut startup: STARTUPINFOEXW = unsafe { mem::zeroed() };
    startup.StartupInfo.cb = mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = inherited[0];
    startup.StartupInfo.hStdOutput = inherited[1];
    startup.StartupInfo.hStdError = inherited[2];
    startup.lpAttributeList = attributes.as_raw();

    // SAFETY: the structure is plain data that the call below fills in.
    let mut created: PROCESS_INFORMATION = unsafe { mem::zeroed() };
    let flags = EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED;

    let result = if lock_down {
        let tokens = token::build()?;
        let environment = minimal_environment();
        let desktop = sandbox_desktop_path()?;
        startup.StartupInfo.lpDesktop = desktop.as_ptr().cast_mut();
        // The process keeps a handle to its current directory for life, so that must not be
        // wherever the host happens to be, which may be a folder of the user's.
        let directory = system_directory()?;
        // SAFETY: all pointers are valid for the call, the command line is writable and
        // null-terminated, and the environment block is correctly terminated.
        let ok = unsafe {
            CreateProcessAsUserW(
                tokens.lockdown.as_raw_handle(),
                exe.as_ptr(),
                command.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                1,
                flags | DETACHED_PROCESS,
                environment.as_ptr().cast(),
                directory.as_ptr(),
                &startup.StartupInfo,
                &mut created,
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            // SAFETY: the thread handle is valid, and the thread is suspended, so this takes
            // effect before it runs anything.
            let set = unsafe { SetThreadToken(&created.hThread, tokens.startup.as_raw_handle()) };
            if set == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
    } else {
        // SAFETY: all pointers are valid for the call, and the command line is writable and
        // null-terminated. A null environment means the host's own.
        let ok = unsafe {
            CreateProcessW(
                exe.as_ptr(),
                command.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                1,
                flags | CREATE_NO_WINDOW,
                ptr::null(),
                ptr::null(),
                &startup.StartupInfo,
                &mut created,
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    };

    let (process, main_thread) = if created.hProcess.is_null() {
        result?;
        return Err(io::Error::other("the process was not created"));
    } else {
        // SAFETY: the process was created, so these are fresh handles owned by nothing else.
        unsafe {
            (
                OwnedHandle::from_raw_handle(created.hProcess),
                OwnedHandle::from_raw_handle(created.hThread),
            )
        }
    };

    let child = SandboxedChild {
        process,
        main_thread,
        _job: job,
        id: created.dwProcessId,
        stdin: Some(stdin.our_end),
        stdout: Some(stdout.our_end),
        stderr: Some(stderr.our_end),
    };
    // A failure from here on drops `child`, which kills the suspended process.
    result?;
    // SAFETY: the thread handle is valid and has suspend-resume access.
    if unsafe { ResumeThread(child.main_thread.as_raw_handle()) } == u32::MAX {
        return Err(io::Error::last_os_error());
    }
    Ok(child)
}
