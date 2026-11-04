//! Experiment D: proves what a render worker can and cannot do, by trying it.
//!
//! `sandbox-probe` plays a compromised worker. Each test that says something is denied is
//! paired with one showing the same attempt succeeding outside the sandbox, because an attempt
//! that fails for an unrelated reason would otherwise look like protection.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::net::TcpListener;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsHandle;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use musaeum_sandbox::{Policy, SandboxedChild, SpawnConfig};
use musaeum_shm::region::duplicate_file_read_only;
use musaeum_shm::slab::slab_len;
use musaeum_shm::wire::{
    HostMessage, InitParams, PROTOCOL_VERSION, RenderJob, WorkerMessage, read_frame, write_frame,
};
use musaeum_shm::{PixelSlab, ShmRegion, TILE_BYTES, TileKey};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
use windows_sys::Win32::System::Memory::{CreateFileMappingW, PAGE_READWRITE};
use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_TYPE_BYTE};
use windows_sys::Win32::System::Threading::CreateEventW;

const SLOTS: u32 = 4;
const MEMORY_LIMIT: usize = 1 << 30;

fn temp_dir(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("sandbox-{test}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn wide(text: &str) -> Vec<u16> {
    std::ffi::OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Named objects a compromised worker might go looking for, all owned by this test.
struct Targets {
    dir: PathBuf,
    document: PathBuf,
    sentinel: PathBuf,
    listener: TcpListener,
    connected: Arc<AtomicBool>,
    pipe_name: String,
    section_name: String,
    event_name: String,
    handles: Vec<HANDLE>,
}

impl Targets {
    fn new(test: &str) -> Self {
        let dir = temp_dir(test);
        let document = dir.join("document.pdf");
        gen_fixtures::write_mixed_fixture(&document).unwrap();
        let sentinel = dir.join("sentinel.txt");
        fs::write(&sentinel, b"the user's private notes").unwrap();

        let id = format!("{}-{test}", std::process::id());
        let pipe_name = format!(r"\\.\pipe\musaeum-probe-{id}");
        let section_name = format!("Local\\musaeum-probe-section-{id}");
        let event_name = format!("Local\\musaeum-probe-event-{id}");

        // SAFETY: the names are null-terminated, null security means the defaults, and every
        // handle stays open until `drop`.
        let handles = unsafe {
            vec![
                CreateNamedPipeW(
                    wide(&pipe_name).as_ptr(),
                    PIPE_ACCESS_DUPLEX,
                    PIPE_TYPE_BYTE,
                    4,
                    4096,
                    4096,
                    0,
                    ptr::null(),
                ),
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    ptr::null(),
                    PAGE_READWRITE,
                    0,
                    4096,
                    wide(&section_name).as_ptr(),
                ),
                CreateEventW(ptr::null(), 0, 0, wide(&event_name).as_ptr()),
            ]
        };
        for handle in &handles {
            assert!(!handle.is_null() && *handle != INVALID_HANDLE_VALUE);
        }

        // Anything that ever connects here means the network was reachable.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let connected = Arc::new(AtomicBool::new(false));
        let (accepting, flag) = (listener.try_clone().unwrap(), Arc::clone(&connected));
        std::thread::spawn(move || {
            while accepting.accept().is_ok() {
                flag.store(true, Ordering::SeqCst);
            }
        });

        Self {
            dir,
            document,
            sentinel,
            listener,
            connected,
            pipe_name,
            section_name,
            event_name,
            handles,
        }
    }

    fn probe_args(&self) -> Vec<OsString> {
        let public = std::env::var_os("PUBLIC").unwrap_or_else(|| r"C:\Users\Public".into());
        let pair = |key: &str, value: &std::ffi::OsStr| {
            let mut argument = OsString::from(format!("{key}="));
            argument.push(value);
            argument
        };
        vec![
            pair("document", self.document.as_os_str()),
            pair("sentinel", self.sentinel.as_os_str()),
            pair("temp", self.dir.as_os_str()),
            pair("public", &public),
            pair("pipe", self.pipe_name.as_ref()),
            pair("section", self.section_name.as_ref()),
            pair("event", self.event_name.as_ref()),
            pair(
                "tcp_port",
                self.listener
                    .local_addr()
                    .unwrap()
                    .port()
                    .to_string()
                    .as_ref(),
            ),
            pair("parent", std::process::id().to_string().as_ref()),
        ]
    }
}

impl Drop for Targets {
    fn drop(&mut self) {
        for handle in &self.handles {
            // SAFETY: each handle was created by `new` and is not used after this.
            unsafe { CloseHandle(*handle) };
        }
    }
}

/// Starts `exe`, hands it a document and a slab the way the pool does.
fn start(
    exe: &str,
    args: &[OsString],
    policy: Policy,
    document: &Path,
    slab: &PixelSlab,
    keep_stdin: bool,
) -> (SandboxedChild, Option<File>, File, File) {
    let mut child = musaeum_sandbox::spawn(SpawnConfig {
        exe: Path::new(exe),
        args,
        policy,
        memory_limit: Some(MEMORY_LIMIT),
    })
    .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let file = File::open(document).unwrap();
    let init = HostMessage::Init(InitParams {
        protocol: PROTOCOL_VERSION,
        worker_index: 0,
        send_geometry: false,
        slab_handle: slab.region().duplicate_into(child.as_handle()).unwrap(),
        slab_len: slab.region().len() as u64,
        doc_handle: duplicate_file_read_only(&file, child.as_handle()).unwrap(),
    });
    write_frame(&mut stdin, &init.encode()).unwrap();
    stdin.flush().unwrap();
    (child, keep_stdin.then_some(stdin), stdout, stderr)
}

fn new_slab() -> PixelSlab {
    PixelSlab::init(ShmRegion::create(slab_len(SLOTS)).unwrap(), SLOTS).unwrap()
}

#[derive(Debug)]
struct Attempt {
    allowed: bool,
    detail: String,
}

/// A handle the probe found in its own process.
#[derive(Debug)]
struct HeldHandle {
    kind: String,
    access: u32,
    name: String,
}

struct ProbeRun {
    attempts: HashMap<String, Attempt>,
    handles: Vec<HeldHandle>,
}

/// Runs the probe to the end and returns what happened to each attempt.
fn run_probe(targets: &Targets, policy: Policy) -> ProbeRun {
    let slab = new_slab();
    let (child, stdin, mut stdout, mut stderr) = start(
        env!("CARGO_BIN_EXE_sandbox-probe"),
        &targets.probe_args(),
        policy,
        &targets.document,
        &slab,
        false,
    );
    drop(stdin);

    let mut output = String::new();
    stdout.read_to_string(&mut output).unwrap();
    let mut log = String::new();
    stderr.read_to_string(&mut log).unwrap();
    assert_eq!(child.wait().unwrap(), 0, "output: {output}\nlog: {log}");

    let mut run = ProbeRun {
        attempts: HashMap::new(),
        handles: Vec::new(),
    };
    for line in output.lines() {
        let mut fields = line.split('\t');
        let first = fields.next().unwrap();
        if first == "handle" {
            let (kind, access, name) = (
                fields.next().unwrap(),
                fields.next().unwrap(),
                fields.next().unwrap_or_default(),
            );
            run.handles.push(HeldHandle {
                kind: kind.to_owned(),
                access: u32::from_str_radix(access.trim_start_matches("0x"), 16).unwrap(),
                name: name.to_owned(),
            });
        } else {
            let verdict = fields.next().unwrap_or_default();
            assert!(
                matches!(verdict, "allowed" | "denied"),
                "the probe failed: {line}\n{log}"
            );
            let detail = fields.next().unwrap_or_default().to_owned();
            run.attempts.insert(
                first.to_owned(),
                Attempt {
                    allowed: verdict == "allowed",
                    detail,
                },
            );
        }
    }
    run
}

/// Everything an attacker in the worker should fail at, and which of those work for an ordinary
/// process. Some need rights the test may lack, so they have no control to compare with.
const MUST_BE_DENIED: &[(&str, bool)] = &[
    // By path, on disk.
    ("read_document", true),
    ("read_sentinel", true),
    ("read_win_ini", true),
    ("read_system_dll", true),
    ("read_system_font", true),
    ("read_own_executable", true),
    ("create_file_in_temp", true),
    ("create_file_in_public", false),
    ("list_temp", true),
    ("list_drive_root", true),
    ("open_volume", false),
    ("open_disk", false),
    ("open_with_long_path_prefix", true),
    ("load_library", true),
    // The network.
    ("connect_tcp", true),
    ("listen_tcp", true),
    ("bind_udp", true),
    ("open_afd_driver", true),
    ("open_tcp_driver", false),
    ("open_nsi_driver", false),
    // Other processes and named objects.
    ("start_process", true),
    ("open_host_process", true),
    ("open_named_pipe", true),
    ("open_named_section", true),
    ("open_named_event", true),
    ("read_user_registry", true),
    ("read_machine_registry", true),
    // The user's desktop.
    ("open_user_desktop", false),
    ("find_taskbar", false),
    ("read_clipboard", false),
    ("read_screen", false),
    // Mitigations.
    ("make_executable_memory", true),
    ("exceed_memory_limit", true),
];

#[test]
fn inside_the_sandbox_nothing_is_reachable_by_name() {
    let targets = Targets::new("inside");
    let attempts = run_probe(&targets, Policy::Lockdown).attempts;

    let mut escapes = Vec::new();
    for (name, _) in MUST_BE_DENIED {
        let attempt = attempts
            .get(*name)
            .unwrap_or_else(|| panic!("the probe never tried {name}"));
        println!(
            "{name}: {} ({})",
            if attempt.allowed { "ALLOWED" } else { "denied" },
            attempt.detail
        );
        if attempt.allowed {
            escapes.push(format!("{name}: {}", attempt.detail));
        }
    }
    assert!(
        escapes.is_empty(),
        "the sandbox let these through: {escapes:#?}"
    );

    std::thread::sleep(Duration::from_millis(100));
    assert!(
        !targets.connected.load(Ordering::SeqCst),
        "a connection reached the listener"
    );
}

#[test]
fn outside_the_sandbox_the_same_attempts_succeed() {
    let targets = Targets::new("outside");
    let attempts = run_probe(&targets, Policy::Unrestricted).attempts;

    for (name, has_control) in MUST_BE_DENIED {
        // The memory limit applies to both, and is what the attempt below is really about.
        if *has_control && *name != "exceed_memory_limit" {
            let attempt = attempts
                .get(*name)
                .unwrap_or_else(|| panic!("the probe never tried {name}"));
            assert!(
                attempt.allowed,
                "{name} fails even without the sandbox, so its denial proves nothing: {}",
                attempt.detail
            );
        }
    }

    let deadline = Instant::now() + Duration::from_secs(3);
    while !targets.connected.load(Ordering::SeqCst) {
        assert!(
            Instant::now() < deadline,
            "the control connection never arrived"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_handles_from_the_host_are_all_a_worker_needs() {
    let targets = Targets::new("handles");
    let attempts = run_probe(&targets, Policy::Lockdown).attempts;
    for name in ["use_document_handle", "render_into_slab"] {
        let attempt = &attempts[name];
        assert!(attempt.allowed, "{name}: {}", attempt.detail);
    }
    // The document can be read through its handle, and not through its path.
    assert!(!attempts["read_document"].allowed);
}

/// What a process opens during startup stays open after the lockdown, with the rights it had.
/// So this reads the process's whole handle table, and every handle that could reach something
/// of the user's has to be accounted for.
#[test]
fn a_worker_holds_no_handle_that_reaches_anything_of_the_users() {
    // In `winnt.h`: KEY_SET_VALUE, KEY_CREATE_SUB_KEY, DELETE, WRITE_DAC, WRITE_OWNER.
    const KEY_WRITE_RIGHTS: u32 = 0x2 | 0x4 | 0x1_0000 | 0x4_0000 | 0x8_0000;
    /// FILE_TRAVERSE and SYNCHRONIZE: a directory that can be walked through, not read.
    const TRAVERSE_ONLY: u32 = 0x10_0020;
    /// What `duplicate_file_read_only` grants on the document.
    const DOCUMENT_ACCESS: u32 = 0x12_0089;

    let targets = Targets::new("audit");
    let handles = run_probe(&targets, Policy::Lockdown).handles;
    assert!(
        handles.len() > 10,
        "the audit found almost nothing: {handles:?}"
    );

    let document_tail = targets.document.file_name().unwrap().to_str().unwrap();
    let mut pipes = 0;
    let mut documents = 0;
    for handle in &handles {
        let HeldHandle { kind, access, name } = handle;
        match kind.as_str() {
            "File" if name == "<pipe>" => pipes += 1,
            "File" if name.ends_with(document_tail) => {
                assert_eq!(*access, DOCUMENT_ACCESS, "the document must be read-only");
                documents += 1;
            }
            // Holds no data of anyone's, and Rust's standard library opens it for random numbers.
            "File" if name == r"\Device\CNG" => {}
            "File" => assert_eq!(
                *access, TRAVERSE_ONLY,
                "a file handle other than the ones the host gave: {handle:?}"
            ),
            "Key" => {
                assert_eq!(access & KEY_WRITE_RIGHTS, 0, "a writable key: {handle:?}");
                assert!(
                    !name.starts_with(r"\REGISTRY\USER"),
                    "a key in the user's own hive: {handle:?}"
                );
            }
            "Process" | "Token" => panic!("a handle that could lead out: {handle:?}"),
            "Section" => assert!(name.is_empty(), "a named section: {handle:?}"),
            "WindowStation" | "Desktop" => assert!(
                !name.contains("WinSta0") && !name.ends_with(r"\Default"),
                "a handle on the user's desktop: {handle:?}"
            ),
            _ => {}
        }
    }
    assert_eq!(pipes, 3, "stdin, stdout and stderr, and no other pipe");
    assert_eq!(documents, 1, "the one document handle from the host");
}

#[test]
fn the_memory_limit_holds_for_any_process_in_a_job() {
    let targets = Targets::new("limit");
    for policy in [Policy::Lockdown, Policy::Unrestricted] {
        let attempts = run_probe(&targets, policy).attempts;
        assert!(
            !attempts["exceed_memory_limit"].allowed,
            "{policy:?} could commit past its limit"
        );
    }
}

#[test]
fn the_real_worker_runs_with_the_token_and_mitigations_it_should_have() {
    let targets = Targets::new("worker");
    let slab = new_slab();
    let (child, stdin, stdout, _stderr) = start(
        env!("CARGO_BIN_EXE_musaeum-render-pdf"),
        &[],
        Policy::Lockdown,
        &targets.document,
        &slab,
        true,
    );
    let mut reader = BufReader::new(stdout);
    let frame = read_frame(&mut reader)
        .unwrap()
        .expect("the worker answers");
    assert!(matches!(
        WorkerMessage::decode(&frame).unwrap(),
        WorkerMessage::Ready { .. }
    ));

    let report = child.report().unwrap();
    println!("{report:#?}");

    let token = &report.token;
    assert!(token.restricted);
    assert_eq!(token.restricting_sids, ["S-1-0-0"], "only the NULL SID");
    assert_eq!(token.integrity_sid, "S-1-16-4096", "low integrity");
    assert_eq!(token.privileges, 0);
    assert!(!token.user_is_usable, "the user's SID is deny-only");
    assert_eq!(
        token.usable_groups, 1,
        "only the integrity label stays usable"
    );

    assert!(report.in_job);
    assert!(report.job.kills_process_when_closed);
    assert_eq!(
        report.job.active_process_limit,
        Some(1),
        "no child processes"
    );
    assert_eq!(report.job.memory_limit, Some(MEMORY_LIMIT));
    assert!(report.job.restricts_clipboard);
    assert!(report.job.restricts_foreign_user_handles);
    assert!(report.job.restricts_desktops);
    assert!(
        !report.main_thread_impersonating,
        "the startup token is gone"
    );

    let mitigations = report.mitigations;
    assert!(mitigations.dynamic_code_prohibited);
    assert!(mitigations.aslr_forced);
    assert!(mitigations.strict_handle_checks);
    assert!(mitigations.extension_points_disabled);
    assert!(mitigations.non_system_fonts_disabled);
    assert!(mitigations.image_load_restricted);
    assert!(mitigations.control_flow_guard);
    // Not a goal, but a fact that changes the risk: the prebuilt PDFium imports user32.dll, so
    // win32k stays reachable. If this ever flips, the ADR and the policy should be revisited.
    assert!(!mitigations.win32k_disabled);

    drop(stdin);
    assert_eq!(child.wait().unwrap(), 0);
}

/// Renders every page at one level of detail, and returns the pixels of all of them.
fn render_all(document: &Path, pages: u32, policy: Policy) -> Vec<Vec<u8>> {
    let slab = new_slab();
    let (child, stdin, stdout, _stderr) = start(
        env!("CARGO_BIN_EXE_musaeum-render-pdf"),
        &[],
        policy,
        document,
        &slab,
        true,
    );
    let mut stdin = stdin.unwrap();
    let mut reader = BufReader::new(stdout);
    let mut next = || {
        WorkerMessage::decode(
            &read_frame(&mut reader)
                .unwrap()
                .expect("the worker answers"),
        )
        .unwrap()
    };
    assert!(matches!(next(), WorkerMessage::Ready { .. }));

    let mut tiles = Vec::new();
    for page in 0..pages {
        let job = HostMessage::Render(RenderJob {
            job_seq: u64::from(page) + 1,
            slot: 0,
            tile: TileKey {
                page,
                scale: musaeum_shm::tile::SCALE_ONE,
                x: 0,
                y: 0,
            },
            background: false,
        });
        write_frame(&mut stdin, &job.encode()).unwrap();
        stdin.flush().unwrap();
        let answer = next();
        assert!(
            matches!(answer, WorkerMessage::TileDone { .. }),
            "{answer:?}"
        );
        let start = slab.slot_ptr(0).unwrap();
        // SAFETY: the worker reported this job as finished, so nothing writes to the slot.
        tiles.push(unsafe { std::slice::from_raw_parts(start, TILE_BYTES) }.to_vec());
    }
    drop(stdin);
    assert_eq!(child.wait().unwrap(), 0);
    tiles
}

#[test]
fn the_sandbox_does_not_change_a_single_pixel() {
    let dir = temp_dir("pixels");
    let document = dir.join("fonts.pdf");
    let pages = gen_fixtures::write_font_fixture(&document).unwrap();

    let inside = render_all(&document, pages, Policy::Lockdown);
    let outside = render_all(&document, pages, Policy::Unrestricted);

    for (page, (inside, outside)) in inside.iter().zip(&outside).enumerate() {
        let black = |tile: &[u8]| {
            tile.as_chunks::<4>()
                .0
                .iter()
                .filter(|pixel| pixel[0] < 64)
                .count()
        };
        assert!(black(outside) > 100, "page {page} has no text to compare");
        let different = inside
            .as_chunks::<4>()
            .0
            .iter()
            .zip(outside.as_chunks::<4>().0)
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(different, 0, "page {page} differs in {different} pixels");
    }
}
