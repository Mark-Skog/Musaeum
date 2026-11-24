//! Checks, from outside, that a running process is as locked down as its policy promises.
//!
//! The token and the mitigations say what a process may still open. What it opened during its
//! startup, before the lockdown, stays open with the rights it had, and only its handle table
//! says what that is. So an audit reads all three, the way the kernel reports them, and lists
//! every way the process falls short. An empty list is the only passing result.
//!
//! The tests of each worker run this on the real binary, started as the app starts it, and
//! `tools/sandbox-audit` runs it on the workers of a running, installed app.
//!
//! It also lists the DLLs the process has loaded: code another program puts into it (an injected
//! DLL) shows there even when it holds no handle, and a worker only ever needs Windows' own DLLs
//! and, for PDFium, the one beside it.

use std::io;
use std::mem;
use std::os::windows::io::{AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::ptr;
use std::sync::mpsc;
use std::time::Duration;

use windows_sys::Wdk::Foundation::NtQueryObject;
use windows_sys::Wdk::System::Threading::NtQueryInformationProcess;
use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_INVALID_HANDLE, FILETIME, HANDLE, HMODULE,
    UNICODE_STRING,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_PIPE, GetFileType};
use windows_sys::Win32::System::JobObjects::IsProcessInJob;
use windows_sys::Win32::System::ProcessStatus::{
    EnumProcessModulesEx, GetModuleFileNameExW, LIST_MODULES_ALL,
};
use windows_sys::Win32::System::SystemInformation::{
    GetSystemTimeAsFileTime, GetSystemWindowsDirectoryW,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetProcessId, GetProcessIdOfThread, GetProcessTimes, OpenProcess,
    PROCESS_DUP_HANDLE, PROCESS_NAME_WIN32, PROCESS_QUERY_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ, QueryFullProcessImageNameW,
};

use crate::Policy;
use crate::inspect::{SandboxReport, TokenReport};
use crate::mitigation::MitigationReport;

/// `ProcessHandleInformation`, the class of `NtQueryInformationProcess` that lists a process's
/// handles (Windows 8 and later).
const PROCESS_HANDLE_INFORMATION: i32 = 51;
const OBJECT_NAME_INFORMATION: i32 = 1;
const OBJECT_TYPE_INFORMATION: i32 = 2;
const OBJECT_TYPES_INFORMATION: i32 = 3;
/// `STATUS_INFO_LENGTH_MISMATCH`: the buffer was too small.
const INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;

/// Rights on a file or a folder that change something: its data, its attributes, its security,
/// or its existence. In `winnt.h`: FILE_WRITE_DATA, FILE_APPEND_DATA, FILE_WRITE_EA,
/// FILE_DELETE_CHILD, FILE_WRITE_ATTRIBUTES, DELETE, WRITE_DAC, WRITE_OWNER, GENERIC_ALL and
/// GENERIC_WRITE.
pub const FILE_WRITE_RIGHTS: u32 =
    0x2 | 0x4 | 0x10 | 0x40 | 0x100 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x1000_0000 | 0x4000_0000;
/// What `musaeum_shm::region::duplicate_file_read_only` grants on a book: FILE_READ_DATA,
/// FILE_READ_EA, FILE_READ_ATTRIBUTES, READ_CONTROL and SYNCHRONIZE.
pub const DOCUMENT_ACCESS: u32 = 0x12_0089;
/// FILE_TRAVERSE and SYNCHRONIZE: a folder that can be walked through, not listed or read. The
/// loader keeps one on the folder a process started in.
pub const TRAVERSE_ONLY: u32 = 0x10_0020;
/// The kernel's cryptography and security support devices, which a process may hold for random
/// numbers without reaching anything of anyone's.
pub const KERNEL_CRYPTO: [&str; 2] = [r"\Device\CNG", r"\Device\KsecDD"];
/// The name given to a handle the process closed between the listing and the copy.
pub const CLOSED_WHILE_READ: &str = "<closed while the audit read it>";
/// Rights on a registry key that change it. In `winnt.h`: KEY_SET_VALUE, KEY_CREATE_SUB_KEY,
/// DELETE, WRITE_DAC and WRITE_OWNER.
pub const KEY_WRITE_RIGHTS: u32 = 0x2 | 0x4 | 0x1_0000 | 0x4_0000 | 0x8_0000;

/// One handle in a process's table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldHandle {
    /// The object type, as the kernel names it: `File`, `Key`, `Section`, `Thread`…
    pub kind: String,
    /// The rights the handle was opened with.
    pub access: u32,
    /// The object's name; `<pipe>` for a pipe, which is not asked for its name because that can
    /// block, and empty for an object without one.
    pub name: String,
    /// For a process or a thread, the process it belongs to.
    pub owner: Option<u32>,
    /// For a process, the path of its executable, when the handle allows asking.
    pub owner_image: Option<String>,
}

impl HeldHandle {
    fn is_pipe(&self) -> bool {
        self.kind == "File" && self.name == "<pipe>"
    }
}

/// What can be read about a process from outside.
#[derive(Clone, Debug)]
pub struct ProcessAudit {
    /// Its id.
    pub pid: u32,
    /// The path of its executable.
    pub image: String,
    /// Its primary token.
    pub token: TokenReport,
    /// Whether it is in a job.
    pub in_job: bool,
    /// Its mitigation policies.
    pub mitigations: MitigationReport,
    /// Every handle it holds.
    pub handles: Vec<HeldHandle>,
    /// The path of every module it has loaded, its executable included; or why they could not be
    /// read (in the first moments of a process, before its loader has run).
    pub modules: Result<Vec<String>, String>,
    /// How long it had been running when it was read, in milliseconds.
    pub age_ms: u64,
    /// Windows' own folder, where the system's DLLs are.
    pub system_root: String,
}

/// What the files a process holds are allowed to be, beyond the pipes, a crypto device and the
/// folder it was started in.
#[derive(Clone, Copy, Debug)]
pub enum Documents<'a> {
    /// Exactly these, read only, by the end of their path: what the host handed over.
    Only(&'a [&'a str]),
    /// Any file, as long as it is read only: a worker that books come and go through, audited
    /// while it runs.
    ReadOnly,
}

impl ProcessAudit {
    /// Reads `process`, which needs query and duplicate access.
    pub fn of(process: BorrowedHandle<'_>) -> io::Result<Self> {
        let mut in_job = 0;
        // SAFETY: the borrow keeps the handle open, a null job means "any job", and `in_job` is
        // a valid out-pointer.
        if unsafe { IsProcessInJob(process.as_raw_handle(), ptr::null_mut(), &mut in_job) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            // SAFETY: the borrow keeps the handle open.
            pid: unsafe { GetProcessId(process.as_raw_handle()) },
            image: image_of(process.as_raw_handle())?,
            token: TokenReport::read(process)?,
            in_job: in_job != 0,
            mitigations: MitigationReport::read(process)?,
            handles: handles(process)?,
            modules: modules_of(process).map_err(|error| error.to_string()),
            age_ms: age_of(process)?,
            system_root: system_root(),
        })
    }

    /// Opens the process `pid` and reads it. This process needs to be allowed to open it, which a
    /// host is for its own workers.
    pub fn open(pid: u32) -> io::Result<Self> {
        let access = PROCESS_QUERY_INFORMATION
            | PROCESS_QUERY_LIMITED_INFORMATION
            | PROCESS_DUP_HANDLE
            | PROCESS_VM_READ;
        // SAFETY: `OpenProcess` takes any id, and returns null on failure.
        let raw = unsafe { OpenProcess(access, 0, pid) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the call above succeeded, so this is a fresh handle owned by nothing else.
        let process = unsafe { OwnedHandle::from_raw_handle(raw) };
        // SAFETY: `process` outlives the borrow.
        Self::of(unsafe { BorrowedHandle::borrow_raw(process.as_raw_handle()) })
    }

    /// Everything about this process that falls short of `policy`. Empty when it holds.
    pub fn violations(&self, policy: Policy, documents: Documents<'_>) -> Vec<String> {
        let mut found = Vec::new();
        let mut check = |ok: bool, what: &str| {
            if !ok {
                found.push(what.to_owned());
            }
        };
        if policy == Policy::Unrestricted {
            check(self.in_job, "not in a job, so it could outlive the app");
            return found;
        }

        let token = &self.token;
        check(token.restricted, "the token is not restricted");
        check(
            token.restricting_sids == ["S-1-0-0"],
            "the restricting SIDs are not only the NULL SID",
        );
        check(
            token.integrity_sid == "S-1-16-4096",
            "the integrity level is not low",
        );
        check(token.privileges == 0, "the token has privileges");
        check(!token.user_is_usable, "the user's SID still grants access");
        check(
            token.usable_groups == 1,
            "a group other than the integrity label still grants access",
        );
        check(self.in_job, "not in a job");

        let mitigations = &self.mitigations;
        check(
            mitigations.dynamic_code_prohibited,
            "it may make code at run time",
        );
        check(mitigations.aslr_forced, "ASLR is not forced");
        check(
            mitigations.strict_handle_checks,
            "invalid handles are not fatal",
        );
        check(
            mitigations.extension_points_disabled,
            "extension points (AppInit DLLs, hooks) are on",
        );
        check(
            mitigations.non_system_fonts_disabled,
            "it may load fonts from outside the system",
        );
        check(
            mitigations.image_load_restricted,
            "it may load images from the network or of low integrity",
        );
        check(mitigations.control_flow_guard, "control flow guard is off");
        let parser = policy == Policy::LockdownParser;
        check(
            mitigations.win32k_disabled == parser,
            if parser {
                "win32k is reachable, which this policy forbids"
            } else {
                "win32k is off, which this worker's policy does not expect: revisit the policy"
            },
        );
        check(
            mitigations.microsoft_signed_only == parser,
            if parser {
                "it may load DLLs Microsoft did not sign"
            } else {
                "only DLLs Microsoft signed load, which this worker's policy does not expect"
            },
        );

        // Code in the process: Windows' own, and the worker's executable and what ships beside it
        // (pdfium.dll). Anything else was put there by another program.
        match &self.modules {
            Ok(modules) => {
                let root = folder_key(&self.system_root);
                let own = self
                    .image
                    .rsplit_once('\\')
                    .map(|(folder, _)| folder_key(folder))
                    .unwrap_or_default();
                for module in modules {
                    let key = module.to_lowercase();
                    let beside = key
                        .rsplit_once('\\')
                        .is_some_and(|(folder, _)| folder_key(folder) == own);
                    if !key.starts_with(&root) && !beside {
                        found.push(format!(
                            "a DLL from outside Windows and the app's folder: {module}"
                        ));
                    }
                }
            }
            Err(error) => found.push(format!("its modules could not be read: {error}")),
        }

        let mut pipes = 0;
        for handle in &self.handles {
            let HeldHandle {
                kind,
                access,
                name,
                owner,
                ..
            } = handle;
            match kind.as_str() {
                _ if handle.is_pipe() => pipes += 1,
                // What it was opened with is known even when the handle cannot be copied.
                "File" if !name.starts_with('\\') && access & FILE_WRITE_RIGHTS != 0 => {
                    found.push(format!("a file it can change: {handle:?}"));
                }
                // Closed by the process while the audit read the list: no longer there.
                _ if name == CLOSED_WHILE_READ => {}
                "File" | "Key" | "Process" | "Thread" | "Token" | "Section" | "WindowStation"
                | "Desktop"
                    if name.starts_with("<not copyable") =>
                {
                    found.push(format!("a handle that could not be inspected: {handle:?}"));
                }
                // The kernel's cryptography drivers, where random numbers come from: Rust's standard
                // library opens CNG, and CryptGenRandom (libarchive's) opens KsecDD. Devices, not
                // files: they hold no data of anyone's.
                "File" if KERNEL_CRYPTO.contains(&name.as_str()) => {}
                "File" if *access == TRAVERSE_ONLY => {}
                "File" => {
                    if access & FILE_WRITE_RIGHTS != 0 {
                        found.push(format!("a file it can change: {handle:?}"));
                    } else if let Documents::Only(given) = documents
                        && !given.iter().any(|tail| name.ends_with(tail))
                    {
                        found.push(format!("a file the host did not give: {handle:?}"));
                    } else if access & !DOCUMENT_ACCESS != 0 {
                        found.push(format!("a file with more than read access: {handle:?}"));
                    }
                }
                "Key" => {
                    if access & KEY_WRITE_RIGHTS != 0 {
                        found.push(format!("a registry key it can change: {handle:?}"));
                    }
                    if name.starts_with(r"\REGISTRY\USER") {
                        found.push(format!("a key in the user's own hive: {handle:?}"));
                    }
                }
                "Process" | "Thread" if *owner != Some(self.pid) => {
                    found.push(format!("a handle on another process: {handle:?}"));
                }
                "Token" => found.push(format!("a token, which could lead out: {handle:?}")),
                "Section" if !name.is_empty() => {
                    found.push(format!("a named section, shared by name: {handle:?}"));
                }
                "WindowStation" | "Desktop"
                    if name.contains("WinSta0") || name.ends_with(r"\Default") =>
                {
                    found.push(format!("a handle on the user's desktop: {handle:?}"));
                }
                _ => {}
            }
        }
        if pipes > 3 {
            found.push(format!(
                "{pipes} pipes, where stdin, stdout and stderr are all the host gives"
            ));
        }
        found
    }
}

impl SandboxReport {
    /// Everything about the job and the startup token, which only the host that started the
    /// process can read, that falls short of `policy`.
    pub fn violations(&self, policy: Policy) -> Vec<String> {
        let mut found = Vec::new();
        if !self.in_job || !self.job.kills_process_when_closed {
            found.push("the job does not end the process with the host".to_owned());
        }
        if policy == Policy::Unrestricted {
            return found;
        }
        let job = &self.job;
        if job.active_process_limit != Some(1) {
            found.push("the job lets it start other processes".to_owned());
        }
        if job.memory_limit.is_none() {
            found.push("the job does not cap its memory".to_owned());
        }
        if !job.restricts_clipboard
            || !job.restricts_foreign_user_handles
            || !job.restricts_desktops
        {
            found
                .push("the job leaves the clipboard, the desktop or other windows open".to_owned());
        }
        if self.main_thread_impersonating {
            found.push("the main thread still holds the startup token".to_owned());
        }
        found
    }
}

/// A folder's path as a prefix to compare others with: lowercase, ending in a backslash.
fn folder_key(folder: &str) -> String {
    let mut key = folder.trim_end_matches('\\').to_lowercase();
    key.push('\\');
    key
}

/// Windows' own folder, as `GetSystemWindowsDirectoryW` gives it.
fn system_root() -> String {
    let mut buffer = [0u16; 260];
    // SAFETY: the pointer and the length describe `buffer`.
    let len = unsafe { GetSystemWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
    if len == 0 || len as usize >= buffer.len() {
        return std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
    }
    String::from_utf16_lossy(&buffer[..len as usize])
}

/// How long `process` has been running, in milliseconds.
fn age_of(process: BorrowedHandle<'_>) -> io::Result<u64> {
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user, mut now) = (zero, zero, zero, zero, zero);
    // SAFETY: the borrow keeps the handle open, and every out-pointer is valid.
    let ok = unsafe {
        GetProcessTimes(
            process.as_raw_handle(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the out-pointer is valid.
    unsafe { GetSystemTimeAsFileTime(&mut now) };
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    Ok(ticks(now).saturating_sub(ticks(created)) / 10_000)
}

/// The paths of the modules `process` has loaded, which needs query and read access on it.
fn modules_of(process: BorrowedHandle<'_>) -> io::Result<Vec<String>> {
    let mut modules: Vec<HMODULE> = vec![ptr::null_mut(); 256];
    loop {
        let mut needed = 0;
        // SAFETY: the buffer and its size in bytes match, and `needed` is a valid out-pointer.
        let ok = unsafe {
            EnumProcessModulesEx(
                process.as_raw_handle(),
                modules.as_mut_ptr(),
                (modules.len() * mem::size_of::<HMODULE>()) as u32,
                &mut needed,
                LIST_MODULES_ALL,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let count = needed as usize / mem::size_of::<HMODULE>();
        if count <= modules.len() {
            modules.truncate(count);
            break;
        }
        modules = vec![ptr::null_mut(); count + 16];
    }
    let mut paths = Vec::with_capacity(modules.len());
    for module in modules {
        let mut buffer = vec![0u16; 1024];
        // SAFETY: the process handle is valid, `module` came from it, and the length is the
        // buffer's.
        let len = unsafe {
            GetModuleFileNameExW(
                process.as_raw_handle(),
                module,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
            )
        } as usize;
        if len == 0 {
            // Unloaded between the two calls.
            continue;
        }
        paths.push(String::from_utf16_lossy(&buffer[..len]));
    }
    Ok(paths)
}

fn image_of(process: HANDLE) -> io::Result<String> {
    let mut buffer = vec![0u16; 1024];
    let mut len = buffer.len() as u32;
    // SAFETY: the caller keeps the handle open, and `len` is the size of `buffer`.
    let ok = unsafe {
        QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut len)
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(String::from_utf16_lossy(&buffer[..len as usize]))
}

/// One entry of `PROCESS_HANDLE_SNAPSHOT_INFORMATION`.
#[repr(C)]
struct Entry {
    handle: HANDLE,
    handle_count: usize,
    pointer_count: usize,
    granted_access: u32,
    object_type_index: u32,
    attributes: u32,
    reserved: u32,
}

/// Lists every handle `process` holds, which needs query and duplicate access on it. Each one is
/// copied here to be asked what it is, and closed.
pub fn handles(process: BorrowedHandle<'_>) -> io::Result<Vec<HeldHandle>> {
    const HEADER: usize = 2 * mem::size_of::<usize>();
    let mut buffer = vec![0u64; 1 << 12];
    loop {
        let mut needed = 0;
        // SAFETY: the buffer and its size match, and `needed` is a valid out-pointer.
        let status = unsafe {
            NtQueryInformationProcess(
                process.as_raw_handle(),
                PROCESS_HANDLE_INFORMATION,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 8) as u32,
                &mut needed,
            )
        };
        if status == INFO_LENGTH_MISMATCH {
            buffer = vec![0u64; (needed as usize).div_ceil(8) + 512];
            continue;
        }
        if status < 0 {
            return Err(io::Error::other(format!(
                "listing the handles: NTSTATUS {:#010x}",
                status as u32
            )));
        }
        break;
    }
    let count = buffer[0] as usize;
    if HEADER + count * mem::size_of::<Entry>() > buffer.len() * 8 {
        return Err(io::Error::other("the handle list does not fit its buffer"));
    }
    // SAFETY: the kernel wrote `count` entries after the header, inside the buffer, as checked.
    let entries = unsafe {
        std::slice::from_raw_parts(
            buffer.as_ptr().cast::<u8>().add(HEADER).cast::<Entry>(),
            count,
        )
    };

    let types = type_names();
    let mut held = Vec::with_capacity(count);
    for entry in entries {
        let mut copy: HANDLE = ptr::null_mut();
        // SAFETY: both process handles are valid, `copy` is a valid out-pointer, and a handle
        // value the other process does not hold just makes the call fail.
        let ok = unsafe {
            DuplicateHandle(
                process.as_raw_handle(),
                entry.handle,
                GetCurrentProcess(),
                &mut copy,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if ok == 0 {
            // Some kinds cannot be copied at all (an ETW registration, for one). The kernel still
            // names their type, and the audit refuses one whose type could reach something. A
            // handle the process closed between the listing and the copy is gone: it is kept in
            // the list, as closed, and a handle that stays open is copied on the next look.
            let error = io::Error::last_os_error();
            let closed = error.raw_os_error() == Some(ERROR_INVALID_HANDLE as i32);
            held.push(HeldHandle {
                kind: types
                    .get(&entry.object_type_index)
                    .cloned()
                    .unwrap_or_else(|| format!("<type {}>", entry.object_type_index)),
                access: entry.granted_access,
                name: if closed {
                    CLOSED_WHILE_READ.to_owned()
                } else {
                    format!("<not copyable: {error}>")
                },
                owner: None,
                owner_image: None,
            });
            continue;
        }
        // SAFETY: the call above succeeded, so this is a fresh handle owned by nothing else.
        let copy = unsafe { OwnedHandle::from_raw_handle(copy) };
        let kind = query_string(copy.as_raw_handle(), OBJECT_TYPE_INFORMATION);
        let (name, owner, owner_image) = match kind.as_str() {
            "Process" => {
                // SAFETY: the handle is a valid process handle.
                let owner = unsafe { GetProcessId(copy.as_raw_handle()) };
                // Who it is, when the handle's rights allow asking: what a diagnosis needs.
                let image = image_of(copy.as_raw_handle()).ok();
                (String::new(), Some(owner), image)
            }
            "Thread" => {
                // SAFETY: the handle is a valid thread handle.
                let owner = unsafe { GetProcessIdOfThread(copy.as_raw_handle()) };
                (String::new(), Some(owner), None)
            }
            // SAFETY: `GetFileType` only reads the handle's kind, and never blocks.
            "File" if unsafe { GetFileType(copy.as_raw_handle()) } == FILE_TYPE_PIPE => {
                ("<pipe>".to_owned(), None, None)
            }
            "File" => (file_name(copy), None, None),
            _ => (
                query_string(copy.as_raw_handle(), OBJECT_NAME_INFORMATION),
                None,
                None,
            ),
        };
        held.push(HeldHandle {
            kind,
            access: entry.granted_access,
            name,
            owner,
            owner_image,
        });
    }
    Ok(held)
}

/// The kernel's object types by their index, as `ObjectTypesInformation` lists them: what a handle
/// that cannot be copied here is. Empty if the list cannot be read.
fn type_names() -> std::collections::HashMap<u32, String> {
    /// `OBJECT_TYPE_INFORMATION` on 64-bit Windows: a `UNICODE_STRING` name, counters, the
    /// generic mapping, and at 0x5a the type's index; its name's characters follow it.
    const ENTRY: usize = 0x68;
    const INDEX_AT: usize = 0x5a;
    let mut names = std::collections::HashMap::new();
    let mut buffer = vec![0u64; 1 << 13];
    let mut length = 0;
    // SAFETY: the buffer and its size match, and `length` is a valid out-pointer. No handle is
    // needed for this class.
    let status = unsafe {
        NtQueryObject(
            ptr::null_mut(),
            OBJECT_TYPES_INFORMATION,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 8) as u32,
            &mut length,
        )
    };
    if status < 0 || mem::size_of::<usize>() != 8 {
        return names;
    }
    let bytes = buffer.len() * 8;
    let base = buffer.as_ptr().cast::<u8>();
    // SAFETY: the kernel wrote the count at the start of the buffer.
    let count = unsafe { base.cast::<u32>().read() } as usize;
    let mut at = 8;
    for _ in 0..count {
        if at + ENTRY > bytes {
            break;
        }
        // SAFETY: `at + ENTRY` is inside the buffer, as checked; the entry starts with a
        // `UNICODE_STRING`, 8-aligned, and the index byte is inside the entry.
        let (string, index) = unsafe {
            (
                &*base.add(at).cast::<UNICODE_STRING>(),
                base.add(at + INDEX_AT).read(),
            )
        };
        if !string.Buffer.is_null() {
            // SAFETY: the kernel points the name at its characters, inside the same buffer.
            let name = unsafe {
                std::slice::from_raw_parts(string.Buffer, usize::from(string.Length) / 2)
            };
            names.insert(u32::from(index), String::from_utf16_lossy(name));
        }
        at += ENTRY + usize::from(string.MaximumLength).next_multiple_of(8);
    }
    names
}

/// The name of a file object. Asking waits for the file's lock when it was opened for synchronous
/// I/O and someone is in the middle of a read, so the question goes to a thread of its own, and a
/// file that does not answer within a second is reported as such.
fn file_name(handle: OwnedHandle) -> String {
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(query_string(
            handle.as_raw_handle(),
            OBJECT_NAME_INFORMATION,
        ));
    });
    receive
        .recv_timeout(Duration::from_secs(1))
        .unwrap_or_else(|_| "<name did not come>".to_owned())
}

/// Reads the `UNICODE_STRING` at the start of an `OBJECT_*_INFORMATION`.
fn query_string(handle: HANDLE, class: i32) -> String {
    let mut text = vec![0u64; 1024];
    let mut length = 0;
    // SAFETY: the buffer and its size match, and `length` is a valid out-pointer.
    let status = unsafe {
        NtQueryObject(
            handle,
            class,
            text.as_mut_ptr().cast(),
            (text.len() * 8) as u32,
            &mut length,
        )
    };
    if status < 0 {
        return format!("<NTSTATUS {:#010x}>", status as u32);
    }
    // SAFETY: on success the buffer starts with a `UNICODE_STRING` whose text is inside it.
    unsafe {
        let string = &*text.as_ptr().cast::<UNICODE_STRING>();
        if string.Buffer.is_null() {
            return String::new();
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(
            string.Buffer,
            usize::from(string.Length) / 2,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOLDER: &str = r"C:\Program Files\Musaeum";

    fn handle(kind: &str, access: u32, name: &str) -> HeldHandle {
        HeldHandle {
            kind: kind.to_owned(),
            access,
            name: name.to_owned(),
            owner: None,
            owner_image: None,
        }
    }

    /// A worker as the sandbox leaves it: everything a policy promises, a book, the pipes.
    fn locked(policy: Policy) -> ProcessAudit {
        let parser = policy == Policy::LockdownParser;
        ProcessAudit {
            pid: 100,
            image: format!(r"{FOLDER}\musaeum-render-pdf.exe"),
            token: TokenReport {
                restricted: true,
                restricting_sids: vec!["S-1-0-0".to_owned()],
                integrity_sid: "S-1-16-4096".to_owned(),
                privileges: 0,
                usable_groups: 1,
                user_is_usable: false,
            },
            in_job: true,
            mitigations: MitigationReport {
                dynamic_code_prohibited: true,
                aslr_forced: true,
                strict_handle_checks: true,
                extension_points_disabled: true,
                non_system_fonts_disabled: true,
                image_load_restricted: true,
                control_flow_guard: true,
                win32k_disabled: parser,
                microsoft_signed_only: parser,
            },
            handles: vec![
                handle("File", 0x12_019f, "<pipe>"),
                handle("File", 0x12_019f, "<pipe>"),
                handle("File", 0x12_019f, "<pipe>"),
                handle(
                    "File",
                    DOCUMENT_ACCESS,
                    r"\Device\HarddiskVolume3\Livros\livro.pdf",
                ),
                handle(
                    "File",
                    TRAVERSE_ONLY,
                    r"\Device\HarddiskVolume3\Windows\System32",
                ),
                handle("File", 0x10_0003, r"\Device\KsecDD"),
                handle(
                    "Key",
                    0x2_0019,
                    r"\REGISTRY\MACHINE\SYSTEM\Sorting\Versions",
                ),
                handle("EtwRegistration", 0x80_4000, "<not copyable: unsupported>"),
                handle("Section", 0x6, ""),
            ],
            modules: Ok(vec![
                format!(r"{FOLDER}\musaeum-render-pdf.exe"),
                r"C:\WINDOWS\SYSTEM32\ntdll.dll".to_owned(),
                format!(r"{FOLDER}\pdfium.dll"),
            ]),
            age_ms: 5_000,
            system_root: r"C:\Windows".to_owned(),
        }
    }

    #[test]
    fn a_worker_as_the_sandbox_leaves_it_passes() {
        for policy in [Policy::Lockdown, Policy::LockdownParser] {
            let audit = locked(policy);
            assert_eq!(
                audit.violations(policy, Documents::Only(&["livro.pdf"])),
                Vec::<String>::new()
            );
            assert_eq!(
                audit.violations(policy, Documents::ReadOnly),
                Vec::<String>::new()
            );
        }
    }

    #[test]
    fn code_another_program_put_in_is_a_violation() {
        let mut audit = locked(Policy::Lockdown);
        audit
            .modules
            .as_mut()
            .unwrap()
            .push(r"C:\Program Files\Hooker\64\hook.dll".to_owned());
        // A DLL of the same name in a folder that only starts like the app's is not the app's.
        audit
            .modules
            .as_mut()
            .unwrap()
            .push(r"C:\Program Files\Musaeum Extras\pdfium.dll".to_owned());
        let found = audit.violations(Policy::Lockdown, Documents::ReadOnly);
        assert_eq!(found.len(), 2, "{found:#?}");
        assert!(
            found
                .iter()
                .all(|line| line.starts_with("a DLL from outside"))
        );

        audit.modules = Err("Only part of a ReadProcessMemory request was completed".to_owned());
        let found = audit.violations(Policy::Lockdown, Documents::ReadOnly);
        assert_eq!(
            found.len(),
            1,
            "unreadable modules are not a pass: {found:#?}"
        );
    }

    #[test]
    fn a_handle_on_another_process_names_it() {
        let mut audit = locked(Policy::Lockdown);
        audit.handles.push(HeldHandle {
            owner: Some(4668),
            owner_image: Some(r"C:\Program Files\Some Service\service.exe".to_owned()),
            ..handle("Process", 0x10_1000, "")
        });
        // Its own threads are its own.
        audit.handles.push(HeldHandle {
            owner: Some(100),
            ..handle("Thread", 0x1f_ffff, "")
        });
        let found = audit.violations(Policy::Lockdown, Documents::ReadOnly);
        assert_eq!(found.len(), 1, "{found:#?}");
        assert!(found[0].contains("service.exe") && found[0].contains("4668"));
    }

    #[test]
    fn what_could_reach_the_users_things_is_a_violation() {
        let cases = [
            handle(
                "File",
                0x12_019f,
                r"\Device\HarddiskVolume3\Livros\livro.pdf",
            ),
            handle(
                "File",
                DOCUMENT_ACCESS,
                r"\Device\HarddiskVolume3\Users\x\secret.txt",
            ),
            handle("Key", 0x2_0019, r"\REGISTRY\USER\S-1-5-21-1\Software"),
            handle("Key", 0x2_0006, r"\REGISTRY\MACHINE\SOFTWARE"),
            handle("Token", 0x8, ""),
            handle("Section", 0x4, r"\Sessions\1\BaseNamedObjects\shared"),
            handle("Desktop", 0x1, r"\Default"),
            handle("Process", 0x10_1000, "<not copyable: denied>"),
            handle("File", 0x12_019f, "<pipe>"),
        ];
        for case in cases {
            let mut audit = locked(Policy::Lockdown);
            audit.handles.push(case.clone());
            let found = audit.violations(Policy::Lockdown, Documents::Only(&["livro.pdf"]));
            assert_eq!(found.len(), 1, "{case:?} gave {found:#?}");
        }
    }

    #[test]
    fn a_handle_closed_while_the_audit_read_it_is_gone_unless_it_could_write() {
        let mut audit = locked(Policy::Lockdown);
        audit
            .handles
            .push(handle("File", DOCUMENT_ACCESS, CLOSED_WHILE_READ));
        audit
            .handles
            .push(handle("Process", 0x10_1000, CLOSED_WHILE_READ));
        assert_eq!(
            audit.violations(Policy::Lockdown, Documents::Only(&["livro.pdf"])),
            Vec::<String>::new(),
            "gone, and a handle that stays open is copied on the next look"
        );
        // What it was opened with is known even when it cannot be copied.
        audit
            .handles
            .push(handle("File", 0x12_019f, CLOSED_WHILE_READ));
        audit.handles.push(handle(
            "File",
            DOCUMENT_ACCESS,
            "<not copyable: unsupported>",
        ));
        let found = audit.violations(Policy::Lockdown, Documents::ReadOnly);
        assert_eq!(found.len(), 2, "{found:#?}");
        assert!(found[0].starts_with("a file it can change"));
        assert!(found[1].starts_with("a handle that could not be inspected"));
    }

    #[test]
    fn a_policy_that_does_not_match_its_mitigations_is_a_violation() {
        let audit = locked(Policy::Lockdown);
        let found = audit.violations(Policy::LockdownParser, Documents::ReadOnly);
        assert_eq!(
            found.len(),
            2,
            "win32k and the signature policy: {found:#?}"
        );
        let mut audit = locked(Policy::LockdownParser);
        audit.token.integrity_sid = "S-1-16-8192".to_owned();
        audit.mitigations.dynamic_code_prohibited = false;
        assert_eq!(
            audit
                .violations(Policy::LockdownParser, Documents::ReadOnly)
                .len(),
            2
        );
    }
}
