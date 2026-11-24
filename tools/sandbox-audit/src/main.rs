//! Audits, from outside, every worker a running Musaeum has started: its token, its mitigations,
//! every handle it holds and every DLL it has loaded, against the policy the app's own code gives
//! it. For the app as it is installed, which the tests of each worker cannot reach.
//!
//!   sandbox-audit --under DIR [--only NAME] [--require NAME+NAME...] [--follow-ms N]
//!                 [--out FILE.json]
//!
//! Looks at every process whose executable is a Musaeum worker inside `DIR` (the installation), or
//! only the one named by `--only`, and fails (exit code 1) if one falls short of its policy, or if a
//! worker named in `--require` is not running. The book files a worker holds may be any, as long as
//! they are read only: books come and go while the app runs.
//!
//! With `--follow-ms N`, it keeps looking for N milliseconds and reports every look, each with how
//! old the process was then and whether it had entered its lockdown: for a worker the host starts
//! for one request and ends after it. The report goes to `--out`, or to stdout.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::mem;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::ptr;
use std::time::{Duration, Instant};

use musaeum_core::FixedFormat;
use musaeum_library::IngestConfig;
use musaeum_ocr::OcrConfig;
use musaeum_pdf_annot::client::AnnotWorkerConfig;
use musaeum_sandbox::{Documents, Policy, ProcessAudit};
use serde_json::{Value, json};
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_NO_TOKEN, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::TOKEN_QUERY;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, OpenThread, OpenThreadToken, PROCESS_QUERY_LIMITED_INFORMATION,
    THREAD_QUERY_INFORMATION,
};

/// Each worker's executable and the policy the app starts it under, from the app's own code.
fn workers() -> Vec<(String, Policy)> {
    let mut table: Vec<(String, Policy)> =
        [FixedFormat::Pdf, FixedFormat::Comic, FixedFormat::Djvu]
            .into_iter()
            .map(|format| (format.worker_exe().to_owned(), format.policy()))
            .collect();
    table.push(("musaeum-ocr.exe".to_owned(), OcrConfig::default().policy));
    table.push((
        "musaeum-pdf-annots.exe".to_owned(),
        AnnotWorkerConfig::default().policy,
    ));
    table.push((
        "musaeum-ingest.exe".to_owned(),
        IngestConfig::default().policy,
    ));
    table
}

struct Options {
    under: PathBuf,
    only: Option<String>,
    require: Vec<String>,
    follow: Option<Duration>,
    out: Option<PathBuf>,
}

fn parse() -> Result<Options, String> {
    let mut under = None;
    let mut only = None;
    let mut require = Vec::new();
    let mut follow = None;
    let mut out = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("{} needs a value", flag.to_string_lossy()))
        };
        match flag.to_str() {
            Some("--under") => under = Some(PathBuf::from(value()?)),
            Some("--only") => only = Some(value()?.to_string_lossy().to_lowercase()),
            Some("--require") => {
                require = value()?
                    .to_string_lossy()
                    .split('+')
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
            Some("--follow-ms") => {
                let text = value()?.to_string_lossy().into_owned();
                let ms: u64 = text
                    .parse()
                    .map_err(|_| format!("--follow-ms takes milliseconds, not {text}"))?;
                follow = Some(Duration::from_millis(ms));
            }
            Some("--out") => out = Some(PathBuf::from(value()?)),
            _ => return Err(format!("unknown argument {}", flag.to_string_lossy())),
        }
    }
    Ok(Options {
        under: under.ok_or("--under DIR is needed")?,
        only,
        require,
        follow,
        out,
    })
}

/// Every process running now: its id and the file name of its executable.
fn processes() -> std::io::Result<Vec<(u32, String)>> {
    // SAFETY: a snapshot of processes takes no process id.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    let mut found = Vec::new();
    // SAFETY: the structure is plain data, for which zeroes are valid.
    let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: the snapshot is valid and `entry` says its own size.
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while more {
        let len = entry
            .szExeFile
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = OsString::from_wide(&entry.szExeFile[..len])
            .to_string_lossy()
            .into_owned();
        found.push((entry.th32ProcessID, name));
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    // SAFETY: the snapshot is a handle this function owns.
    unsafe { CloseHandle(snapshot) };
    Ok(found)
}

/// Whether a thread of `pid` still holds an impersonation token: a worker starts with its main
/// thread holding the startup token, and drops it for good when it enters its lockdown, before it
/// reads anything from the host. `None` when a thread could not be asked.
fn impersonating(pid: u32) -> Option<bool> {
    // SAFETY: a snapshot of threads takes no process id.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }
    // SAFETY: the structure is plain data, for which zeroes are valid.
    let mut entry: THREADENTRY32 = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<THREADENTRY32>() as u32;
    let mut answer = Some(false);
    let mut threads = 0;
    // SAFETY: the snapshot is valid and `entry` says its own size.
    let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == pid {
            threads += 1;
            // SAFETY: `OpenThread` takes any id, and returns null on failure.
            let thread = unsafe { OpenThread(THREAD_QUERY_INFORMATION, 0, entry.th32ThreadID) };
            if thread.is_null() {
                answer = None;
            } else {
                let mut token: HANDLE = ptr::null_mut();
                // SAFETY: the thread handle is valid, and `token` is a valid out-pointer; the check
                // uses this process's identity, as the token may belong to anything.
                let opened = unsafe { OpenThreadToken(thread, TOKEN_QUERY, 1, &mut token) };
                if opened != 0 {
                    answer = answer.map(|_| true);
                    // SAFETY: the call above succeeded, so this is a handle this function owns.
                    unsafe { CloseHandle(token) };
                } else if std::io::Error::last_os_error().raw_os_error()
                    != Some(ERROR_NO_TOKEN as i32)
                {
                    answer = None;
                }
                // SAFETY: a handle this function owns.
                unsafe { CloseHandle(thread) };
            }
        }
        // SAFETY: as above.
        more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    // SAFETY: the snapshot is a handle this function owns.
    unsafe { CloseHandle(snapshot) };
    // A process whose threads the snapshot did not list cannot be said to be either.
    if threads == 0 { None } else { answer }
}

/// How long `pid` has been running, in milliseconds, now.
fn age_now(pid: u32) -> Option<u64> {
    // SAFETY: `OpenProcess` takes any id, and returns null on failure.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return None;
    }
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user, mut now) = (zero, zero, zero, zero, zero);
    // SAFETY: the handle is valid, and every out-pointer is valid.
    let ok = unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) };
    // SAFETY: the out-pointer is valid.
    unsafe { GetSystemTimeAsFileTime(&mut now) };
    // SAFETY: a handle this function owns.
    unsafe { CloseHandle(process) };
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    (ok != 0).then(|| ticks(now).saturating_sub(ticks(created)) / 10_000)
}

fn inside(image: &str, under: &Path) -> bool {
    let image = image.to_lowercase();
    let mut under = under.to_string_lossy().to_lowercase();
    if !under.ends_with('\\') {
        under.push('\\');
    }
    image.starts_with(&under)
}

/// One look at one worker, as JSON, and whether it passed. `locked` is whether, when the look
/// began, no thread of it held the startup token: the lockdown cannot be undone, so a look that
/// began after it saw only what comes after it.
fn look(
    audit: &ProcessAudit,
    name: &str,
    policy: Policy,
    locked: Option<bool>,
    began: Option<u64>,
) -> (Value, bool) {
    let violations = audit.violations(policy, Documents::ReadOnly);
    let mut kinds = BTreeMap::<String, usize>::new();
    for handle in &audit.handles {
        *kinds.entry(handle.kind.clone()).or_default() += 1;
    }
    let files: Vec<&str> = audit
        .handles
        .iter()
        .filter(|handle| handle.kind == "File" && handle.name != "<pipe>")
        .map(|handle| handle.name.as_str())
        .collect();
    let root = audit.system_root.to_lowercase();
    let modules: Value = match &audit.modules {
        Ok(modules) => json!(
            modules
                .iter()
                .filter(|module| !module.to_lowercase().starts_with(&root))
                .collect::<Vec<_>>()
        ),
        Err(error) => json!({ "unreadable": error }),
    };
    let passed = violations.is_empty();
    (
        json!({
            "pid": audit.pid,
            "exe": name,
            "image": audit.image,
            // How old it was when the look began, and when it ended.
            "beganAtMs": began,
            "ageMs": audit.age_ms,
            // A thread still holds the startup token: the worker has not entered its lockdown yet,
            // and reads nothing from the host until it has.
            "impersonating": locked.map(|locked| !locked),
            "policy": format!("{policy:?}"),
            "token": {
                "restricted": audit.token.restricted,
                "integrity": audit.token.integrity_sid,
                "privileges": audit.token.privileges,
                "usableGroups": audit.token.usable_groups,
            },
            "win32kDisabled": audit.mitigations.win32k_disabled,
            "microsoftSignedOnly": audit.mitigations.microsoft_signed_only,
            "handles": kinds,
            "files": files,
            "modulesOutsideWindows": modules,
            "violations": violations,
        }),
        passed,
    )
}

fn run(options: &Options) -> Result<(Value, bool), String> {
    let under = std::path::absolute(&options.under).map_err(|error| error.to_string())?;
    let table = workers();
    let started = Instant::now();
    let mut looks = Vec::new();
    let mut passed = true;
    let mut seen = Vec::new();
    loop {
        for (pid, name) in processes().map_err(|error| error.to_string())? {
            if options
                .only
                .as_ref()
                .is_some_and(|only| *only != name.to_lowercase())
            {
                continue;
            }
            let Some((_, policy)) = table
                .iter()
                .find(|(exe, _)| exe.eq_ignore_ascii_case(&name))
            else {
                continue;
            };
            // Asked first: the audit itself takes milliseconds, and the worker may lock down in them.
            let began = age_now(pid);
            let locked = impersonating(pid).map(|holds| !holds);
            let audit = match ProcessAudit::open(pid) {
                Ok(audit) => audit,
                // One that ended between the listing and now, or someone else's.
                Err(_) => continue,
            };
            if !inside(&audit.image, &under) {
                continue;
            }
            let (entry, ok) = look(&audit, &name, *policy, locked, began);
            passed &= ok;
            seen.push(name.to_lowercase());
            looks.push(entry);
        }
        match options.follow {
            Some(follow) if started.elapsed() < follow => {
                std::thread::sleep(Duration::from_millis(2));
            }
            _ => break,
        }
    }
    let missing: Vec<&String> = options
        .require
        .iter()
        .filter(|name| !seen.contains(&name.to_lowercase()))
        .collect();
    passed &= missing.is_empty();
    let report = json!({
        "under": under,
        "followedMs": options.follow.map(|follow| follow.as_millis() as u64),
        "workers": looks,
        "missing": missing,
        "passed": passed,
    });
    Ok((report, passed))
}

fn main() -> ExitCode {
    let options = match parse() {
        Ok(options) => options,
        Err(error) => {
            eprintln!("sandbox-audit: {error}");
            return ExitCode::from(2);
        }
    };
    match run(&options) {
        Ok((report, passed)) => {
            let text = serde_json::to_string_pretty(&report).unwrap_or_default();
            match &options.out {
                Some(path) => {
                    if let Err(error) = std::fs::write(path, &text) {
                        eprintln!("sandbox-audit: writing {}: {error}", path.display());
                        return ExitCode::from(2);
                    }
                }
                None => println!("{text}"),
            }
            if passed {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("sandbox-audit: {error}");
            ExitCode::from(2)
        }
    }
}
