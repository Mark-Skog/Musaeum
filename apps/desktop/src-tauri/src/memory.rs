//! Measures what the whole app costs in memory: this process and everything it started.
//!
//! The number that matters is the private working set, what Task Manager shows by default: the
//! RAM that only this process is using, so that adding up processes does not count anything
//! twice. The WebView2 runtime is several processes, and that is most of the app's footprint.
//!
//! Memory on the GPU's side is not in here. On a discrete GPU, the compositor's texture array
//! lives in VRAM; on an integrated GPU, that VRAM is system memory the sum below does not show.

use std::collections::{BTreeMap, HashMap};
use std::mem;
use std::ptr;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use windows_sys::Wdk::System::Threading::{
    NtQueryInformationProcess, ProcessCommandLineInformation,
};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, UNICODE_STRING};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX2,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};

const SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

/// One process in one sample.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessMemory {
    pid: u32,
    /// `core`, `render-worker`, or `webview-` followed by Chromium's process type.
    role: String,
    private_working_set: u64,
    working_set: u64,
    private_commit: u64,
}

/// Picks `root` and everything descended from it out of `(pid, parent)` pairs.
fn descendants(processes: &[(u32, u32)], root: u32) -> Vec<u32> {
    let mut found = vec![root];
    let mut at = 0;
    while at < found.len() {
        let parent = found[at];
        for &(pid, process_parent) in processes {
            if process_parent == parent && pid != parent && !found.contains(&pid) {
                found.push(pid);
            }
        }
        at += 1;
    }
    found
}

/// Names a process from its executable and command line.
fn role_of(pid: u32, exe: &str, command_line: &str) -> String {
    if pid == std::process::id() {
        return "core".to_owned();
    }
    if [
        "musaeum-render-pdf.exe",
        "musaeum-render-raster.exe",
        "musaeum-render-djvu.exe",
    ]
    .iter()
    .any(|worker| exe.eq_ignore_ascii_case(worker))
    {
        return "render-worker".to_owned();
    }
    let kind = command_line
        .split_ascii_whitespace()
        .find_map(|argument| argument.strip_prefix("--type="));
    match kind {
        Some(kind) => {
            // Utility processes do several different jobs, and say which in a sub-type.
            let sub_type = command_line
                .split_ascii_whitespace()
                .find_map(|argument| argument.strip_prefix("--utility-sub-type="));
            match sub_type {
                Some(sub_type) => format!("webview-{kind}-{sub_type}"),
                None => format!("webview-{kind}"),
            }
        }
        None if exe.eq_ignore_ascii_case("msedgewebview2.exe") => "webview-browser".to_owned(),
        None => format!("other-{exe}"),
    }
}

/// Every process on the system, as `(pid, parent, executable name)`.
fn all_processes() -> Vec<(u32, u32, String)> {
    let mut processes = Vec::new();
    // SAFETY: the flag asks for processes, for which the process id argument is ignored.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return processes;
    }
    // SAFETY: the structure is plain data, and the calls below only need `dwSize` to be set.
    let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: the snapshot is valid, and `entry` is a valid out-pointer with its size set.
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while more {
        let len = entry
            .szExeFile
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(entry.szExeFile.len());
        processes.push((
            entry.th32ProcessID,
            entry.th32ParentProcessID,
            String::from_utf16_lossy(&entry.szExeFile[..len]),
        ));
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    // SAFETY: the handle was opened above and is not used again.
    unsafe { CloseHandle(snapshot) };
    processes
}

fn command_line_of(process: HANDLE) -> String {
    let mut buffer = vec![0u64; 4096];
    let mut needed = 0;
    // SAFETY: the buffer and its size match, and `needed` is a valid out-pointer.
    let status = unsafe {
        NtQueryInformationProcess(
            process,
            ProcessCommandLineInformation,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 8) as u32,
            &mut needed,
        )
    };
    if status < 0 {
        return String::new();
    }
    // SAFETY: on success the buffer starts with a `UNICODE_STRING` whose text is inside it.
    unsafe {
        let text = &*buffer.as_ptr().cast::<UNICODE_STRING>();
        if text.Buffer.is_null() {
            return String::new();
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(
            text.Buffer,
            usize::from(text.Length) / 2,
        ))
    }
}

fn read_process(pid: u32, exe: &str) -> Option<ProcessMemory> {
    // SAFETY: `OpenProcess` takes no pointers.
    let process =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid) };
    if process.is_null() {
        return None;
    }
    // SAFETY: the structure is plain data, filled in by the call below.
    let mut counters: PROCESS_MEMORY_COUNTERS_EX2 = unsafe { mem::zeroed() };
    counters.cb = mem::size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32;
    // SAFETY: the handle is valid, and the pointer and size describe `counters`. This
    // structure is a superset of the one the signature names, which `cb` tells the call.
    let ok =
        unsafe { GetProcessMemoryInfo(process, ptr::from_mut(&mut counters).cast(), counters.cb) };
    let role = role_of(pid, exe, &command_line_of(process));
    // SAFETY: the handle was opened above and is not used again.
    unsafe { CloseHandle(process) };
    (ok != 0).then_some(ProcessMemory {
        pid,
        role,
        private_working_set: counters.PrivateWorkingSetSize as u64,
        working_set: counters.WorkingSetSize as u64,
        private_commit: counters.PrivateUsage as u64,
    })
}

/// Reads this process and all of its descendants.
pub fn sample_process_tree() -> Vec<ProcessMemory> {
    let processes = all_processes();
    let pairs: Vec<(u32, u32)> = processes
        .iter()
        .map(|&(pid, parent, _)| (pid, parent))
        .collect();
    let names: HashMap<u32, &str> = processes
        .iter()
        .map(|(pid, _, name)| (*pid, name.as_str()))
        .collect();
    descendants(&pairs, std::process::id())
        .into_iter()
        .filter_map(|pid| read_process(pid, names.get(&pid).copied().unwrap_or_default()))
        .collect()
}

#[derive(Debug)]
struct Sample {
    phase: String,
    processes: Vec<ProcessMemory>,
}

/// A sample with its processes added up.
fn total_private(sample: &Sample) -> u64 {
    sample
        .processes
        .iter()
        .map(|process| process.private_working_set)
        .sum()
}

fn by_role(processes: &[ProcessMemory]) -> BTreeMap<String, RoleTotal> {
    let mut roles: BTreeMap<String, RoleTotal> = BTreeMap::new();
    for process in processes {
        let role = roles.entry(process.role.clone()).or_default();
        role.processes += 1;
        role.private_working_set += process.private_working_set;
    }
    roles
}

/// Everything in one role.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleTotal {
    processes: u32,
    private_working_set: u64,
}

/// What one phase of a scenario cost.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseReport {
    phase: String,
    samples: usize,
    /// Mean of the sum of private working sets.
    mean_total: u64,
    /// The largest sum of private working sets, at any moment.
    peak_total: u64,
    /// The sum in the phase's last sample, once whatever it was doing has settled.
    last_total: u64,
    /// The processes, grouped by role, at the last sample.
    last_by_role: BTreeMap<String, RoleTotal>,
    /// The same at the moment of the peak.
    peak_by_role: BTreeMap<String, RoleTotal>,
}

fn summarize(samples: &[Sample]) -> Vec<PhaseReport> {
    let mut order: Vec<&str> = Vec::new();
    for sample in samples {
        if !order.contains(&sample.phase.as_str()) {
            order.push(&sample.phase);
        }
    }
    order
        .into_iter()
        .filter_map(|phase| {
            let in_phase: Vec<&Sample> = samples
                .iter()
                .filter(|sample| sample.phase == phase)
                .collect();
            let peak = in_phase
                .iter()
                .copied()
                .max_by_key(|sample| total_private(sample))?;
            let last = *in_phase.last()?;
            let sum: u64 = in_phase.iter().map(|sample| total_private(sample)).sum();
            Some(PhaseReport {
                phase: phase.to_owned(),
                samples: in_phase.len(),
                mean_total: sum / in_phase.len() as u64,
                peak_total: total_private(peak),
                last_total: total_private(last),
                last_by_role: by_role(&last.processes),
                peak_by_role: by_role(&peak.processes),
            })
        })
        .collect()
}

#[derive(Debug, Default)]
struct SamplerState {
    phase: String,
    samples: Vec<Sample>,
}

/// Samples in the background for as long as the app runs, which is what a benchmark needs.
#[derive(Debug, Clone)]
pub struct MemorySampler {
    state: Arc<Mutex<SamplerState>>,
    started: Instant,
}

impl MemorySampler {
    /// Starts sampling, under a phase named `starting`.
    pub fn start() -> Self {
        let state = Arc::new(Mutex::new(SamplerState {
            phase: "starting".to_owned(),
            samples: Vec::new(),
        }));
        let shared = Arc::clone(&state);
        thread::spawn(move || {
            loop {
                let processes = sample_process_tree();
                {
                    let mut state = shared.lock().unwrap_or_else(PoisonError::into_inner);
                    let phase = state.phase.clone();
                    state.samples.push(Sample { phase, processes });
                }
                thread::sleep(SAMPLE_INTERVAL);
            }
        });
        Self {
            state,
            started: Instant::now(),
        }
    }

    /// Counts what follows under another phase.
    pub fn mark(&self, phase: &str) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .phase = phase.to_owned();
    }

    /// What has been measured so far.
    pub fn report(&self) -> serde_json::Value {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        serde_json::json!({
            "elapsedSeconds": self.started.elapsed().as_secs_f32(),
            "phases": summarize(&state.samples),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(role: &str, private: u64) -> ProcessMemory {
        ProcessMemory {
            pid: 1,
            role: role.to_owned(),
            private_working_set: private,
            working_set: private * 2,
            private_commit: private,
        }
    }

    #[test]
    fn finds_the_whole_tree_and_nothing_else() {
        let processes = [(10, 1), (20, 10), (30, 20), (40, 10), (50, 99), (60, 50)];
        let mut found = descendants(&processes, 10);
        found.sort_unstable();
        assert_eq!(found, [10, 20, 30, 40]);
    }

    #[test]
    fn a_process_that_lists_itself_as_parent_does_not_hang() {
        assert_eq!(descendants(&[(4, 4), (0, 0)], 4), [4]);
    }

    #[test]
    fn names_each_kind_of_process() {
        let me = std::process::id();
        assert_eq!(role_of(me, "musaeum-desktop.exe", ""), "core");
        assert_eq!(role_of(2, "musaeum-render-pdf.exe", ""), "render-worker");
        assert_eq!(
            role_of(2, "msedgewebview2.exe", "x.exe --embedded"),
            "webview-browser"
        );
        assert_eq!(
            role_of(2, "msedgewebview2.exe", "x.exe --type=renderer --y"),
            "webview-renderer"
        );
        assert_eq!(
            role_of(2, "msedgewebview2.exe", "x.exe --type=gpu-process"),
            "webview-gpu-process"
        );
        assert_eq!(
            role_of(
                2,
                "msedgewebview2.exe",
                "x --type=utility --utility-sub-type=network.mojom.NetworkService"
            ),
            "webview-utility-network.mojom.NetworkService"
        );
        assert_eq!(role_of(2, "conhost.exe", ""), "other-conhost.exe");
    }

    #[test]
    fn this_process_can_be_measured() {
        let sample = sample_process_tree();
        let me = sample
            .iter()
            .find(|process| process.pid == std::process::id())
            .unwrap();
        assert_eq!(me.role, "core");
        assert!(me.private_working_set > 0);
        assert!(me.working_set >= me.private_working_set);
    }

    #[test]
    fn phases_keep_their_order_and_their_own_samples() {
        let sample = |phase: &str, private: u64| Sample {
            phase: phase.to_owned(),
            processes: vec![process("core", private), process("render-worker", 10)],
        };
        let report = summarize(&[
            sample("a", 100),
            sample("a", 300),
            sample("a", 200),
            sample("b", 50),
        ]);
        assert_eq!(report.len(), 2);
        assert_eq!(report[0].phase, "a");
        assert_eq!(report[0].samples, 3);
        assert_eq!(report[0].mean_total, 210);
        assert_eq!(report[0].peak_total, 310);
        assert_eq!(report[0].last_total, 210);
        assert_eq!(report[0].peak_by_role["core"].private_working_set, 300);
        assert_eq!(report[1].phase, "b");
        assert_eq!(report[1].last_by_role["render-worker"].processes, 1);
    }
}
