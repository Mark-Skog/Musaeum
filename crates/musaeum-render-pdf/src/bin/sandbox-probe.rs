//! Stands in for a compromised render worker, to test what the sandbox lets it do.
//!
//! The real worker contains no code that opens a file or a socket, so it cannot show that the
//! sandbox would stop one. This program starts up along the same path as the worker, in the
//! same process setup, and with PDFium loaded and a document open it tries everything an
//! attacker in control of the worker would try next. Each attempt prints one line:
//!
//! ```text
//! name<TAB>allowed|denied<TAB>detail
//! ```
//!
//! Only `tests/sandbox.rs` runs it. It takes no untrusted input, and is not part of a release.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString, c_void};
use std::io::{self, Read};
use std::mem;
use std::os::windows::ffi::OsStrExt;
use std::process::ExitCode;
use std::ptr;

use musaeum_render_pdf::engine::{CancelProbe, Document, RenderOutcome};
use musaeum_render_pdf::startup::{self, Attached};
use musaeum_shm::TileKey;
use musaeum_shm::wire::{HostMessage, read_frame};
use windows_sys::Wdk::Foundation::{NtQueryObject, OBJECT_ATTRIBUTES};
use windows_sys::Wdk::Storage::FileSystem::{FILE_OPEN, NtCreateFile};
use windows_sys::Wdk::System::Threading::NtQueryInformationProcess;
use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, UNICODE_STRING,
};
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetPixel,
    ReleaseDC, SRCCOPY, SelectObject,
};
use windows_sys::Win32::Networking::WinSock::{
    AF_INET, IN_ADDR, IN_ADDR_0, INVALID_SOCKET, IPPROTO_TCP, IPPROTO_UDP, SOCK_DGRAM, SOCK_STREAM,
    SOCKADDR, SOCKADDR_IN, SOCKET, WSADATA, WSAGetLastError, WSAStartup, bind, closesocket,
    connect, listen, socket,
};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateFileW, DeleteFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TYPE_PIPE, FindClose, FindFirstFileW, GetFileType,
    OPEN_EXISTING, SYNCHRONIZE, WIN32_FIND_DATAW,
};
use windows_sys::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;
use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
use windows_sys::Win32::System::Memory::{
    FILE_MAP_READ, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, OpenFileMappingW, PAGE_EXECUTE_READWRITE,
    PAGE_READWRITE, VirtualAlloc, VirtualFree,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW,
};
use windows_sys::Win32::System::StationsAndDesktops::{CloseDesktop, OpenDesktopW};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CreateProcessW, GetCurrentProcess, GetCurrentThread, GetExitCodeProcess,
    OpenEventW, OpenProcess, OpenThreadToken, PROCESS_INFORMATION, PROCESS_VM_READ, STARTUPINFOW,
    SYNCHRONIZATION_SYNCHRONIZE, TerminateProcess, WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW;

/// `STATUS_ACCESS_DENIED`.
const ACCESS_DENIED: i32 = 0xC000_0022_u32 as i32;

fn wide(text: impl AsRef<OsStr>) -> Vec<u16> {
    text.as_ref()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn report(name: &str, outcome: Result<String, String>) {
    let (verdict, detail) = match outcome {
        Ok(detail) => ("allowed", detail),
        Err(detail) => ("denied", detail),
    };
    let detail = detail.replace(['\t', '\r', '\n'], " ");
    println!("{name}\t{verdict}\t{detail}");
}

fn os_error() -> String {
    io::Error::last_os_error().raw_os_error().map_or_else(
        || "unknown error".to_owned(),
        |code| format!("os error {code}"),
    )
}

/// Opens a path with the given access.
fn open_path(path: impl AsRef<OsStr>, access: u32) -> Result<String, String> {
    let path = wide(path);
    // SAFETY: the path is null-terminated, and null attributes and template are valid.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(os_error());
    }
    // SAFETY: the handle was just opened and is not used again.
    unsafe { CloseHandle(handle) };
    Ok("opened".to_owned())
}

/// Creates a new file, and removes it again if that worked.
fn create_file(path: impl AsRef<OsStr>) -> Result<String, String> {
    let path = wide(path);
    // SAFETY: the path is null-terminated, and null attributes and template are valid.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_WRITE,
            0,
            ptr::null(),
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(os_error());
    }
    // SAFETY: the handle was just opened and the path is null-terminated.
    unsafe {
        CloseHandle(handle);
        DeleteFileW(path.as_ptr());
    }
    Ok("created".to_owned())
}

fn list_directory(pattern: impl AsRef<OsStr>) -> Result<String, String> {
    let pattern = wide(pattern);
    // SAFETY: the structure is plain data, filled in by the call below.
    let mut found: WIN32_FIND_DATAW = unsafe { mem::zeroed() };
    // SAFETY: the pattern is null-terminated, and `found` is a valid out-pointer.
    let handle = unsafe { FindFirstFileW(pattern.as_ptr(), &mut found) };
    if handle == INVALID_HANDLE_VALUE {
        return Err(os_error());
    }
    // SAFETY: the search handle was just opened and is not used again.
    unsafe { FindClose(handle) };
    Ok("listed".to_owned())
}

/// Goes straight to the kernel, the way shellcode would, instead of through a DLL that may have
/// failed to load. Only access denied counts as the sandbox's doing: any other failure means
/// the call got as far as the device.
fn open_device(path: &str) -> Result<String, String> {
    let mut units: Vec<u16> = path.encode_utf16().collect();
    let name = UNICODE_STRING {
        Length: (units.len() * 2) as u16,
        MaximumLength: (units.len() * 2) as u16,
        Buffer: units.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: ptr::null_mut(),
        ObjectName: &name,
        Attributes: 0x40, // OBJ_CASE_INSENSITIVE
        SecurityDescriptor: ptr::null(),
        SecurityQualityOfService: ptr::null(),
    };
    let mut handle: HANDLE = ptr::null_mut();
    // SAFETY: the structure is plain data, filled in by the call below.
    let mut status: IO_STATUS_BLOCK = unsafe { mem::zeroed() };
    // SAFETY: every pointer is valid for the call, and `units` outlives it.
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            SYNCHRONIZE,
            &attributes,
            &mut status,
            ptr::null(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_OPEN,
            0,
            ptr::null(),
            0,
        )
    };
    if result == ACCESS_DENIED {
        return Err("access denied by the kernel".to_owned());
    }
    if result >= 0 {
        // SAFETY: the call succeeded, so this handle is valid and owned here.
        unsafe { CloseHandle(handle) };
    }
    Ok(format!(
        "reached the device, NTSTATUS {:#010x}",
        result as u32
    ))
}

fn open_registry_key(root: HKEY, path: &str) -> Result<String, String> {
    let path = wide(path);
    let mut key: HKEY = ptr::null_mut();
    // SAFETY: the path is null-terminated, and `key` is a valid out-pointer.
    let code = unsafe { RegOpenKeyExW(root, path.as_ptr(), 0, KEY_READ, &mut key) };
    if code != 0 {
        return Err(format!("os error {code}"));
    }
    // SAFETY: the key was just opened and is not used again.
    unsafe { RegCloseKey(key) };
    Ok("opened".to_owned())
}

fn spawn_process(system_root: &OsStr) -> Result<String, String> {
    let mut path = system_root.to_owned();
    path.push("\\System32\\cmd.exe");
    let path = wide(path);
    let mut command = wide("cmd.exe /c exit 0");
    // SAFETY: the structure is plain data for which zeroes means "no options".
    let mut startup: STARTUPINFOW = unsafe { mem::zeroed() };
    startup.cb = mem::size_of::<STARTUPINFOW>() as u32;
    // SAFETY: the structure is plain data, filled in by the call below.
    let mut created: PROCESS_INFORMATION = unsafe { mem::zeroed() };
    // SAFETY: the command line is writable and null-terminated, and the rest may be null.
    let ok = unsafe {
        CreateProcessW(
            path.as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            CREATE_NO_WINDOW,
            ptr::null(),
            ptr::null(),
            &startup,
            &mut created,
        )
    };
    if ok == 0 {
        return Err(os_error());
    }
    // SAFETY: the handles were just created and are not used again.
    unsafe {
        WaitForSingleObject(created.hProcess, 5000);
        let mut code = 0;
        GetExitCodeProcess(created.hProcess, &mut code);
        TerminateProcess(created.hProcess, 0);
        CloseHandle(created.hProcess);
        CloseHandle(created.hThread);
    }
    Ok("started".to_owned())
}

fn open_process(pid: u32) -> Result<String, String> {
    // SAFETY: `OpenProcess` takes no pointers.
    let handle = unsafe { OpenProcess(PROCESS_VM_READ, 0, pid) };
    if handle.is_null() {
        return Err(os_error());
    }
    // SAFETY: the handle was just opened and is not used again.
    unsafe { CloseHandle(handle) };
    Ok("opened".to_owned())
}

fn open_section(name: &str) -> Result<String, String> {
    let name = wide(name);
    // SAFETY: the name is null-terminated.
    let handle = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, name.as_ptr()) };
    if handle.is_null() {
        return Err(os_error());
    }
    // SAFETY: the handle was just opened and is not used again.
    unsafe { CloseHandle(handle) };
    Ok("opened".to_owned())
}

fn open_event(name: &str) -> Result<String, String> {
    let name = wide(name);
    // SAFETY: the name is null-terminated.
    let handle = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()) };
    if handle.is_null() {
        return Err(os_error());
    }
    // SAFETY: the handle was just opened and is not used again.
    unsafe { CloseHandle(handle) };
    Ok("opened".to_owned())
}

fn make_executable_memory() -> Result<String, String> {
    // SAFETY: a null address lets the system choose, and the memory is released right away.
    unsafe {
        let memory = VirtualAlloc(
            ptr::null(),
            4096,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_EXECUTE_READWRITE,
        );
        if memory.is_null() {
            return Err(os_error());
        }
        VirtualFree(memory, 0, MEM_RELEASE);
    }
    Ok("got writable and executable memory".to_owned())
}

fn allocate(bytes: usize) -> Result<String, String> {
    // SAFETY: a null address lets the system choose, and the memory is released right away.
    unsafe {
        let memory: *mut c_void =
            VirtualAlloc(ptr::null(), bytes, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
        if memory.is_null() {
            return Err(os_error());
        }
        VirtualFree(memory, 0, MEM_RELEASE);
    }
    Ok(format!("committed {bytes} bytes"))
}

fn open_user_desktop() -> Result<String, String> {
    let name = wide("Default");
    // SAFETY: the name is null-terminated, and the rest are plain values.
    let desktop = unsafe { OpenDesktopW(name.as_ptr(), 0, 0, GENERIC_READ) };
    if desktop.is_null() {
        return Err(os_error());
    }
    // SAFETY: the handle was just opened and is not used again.
    unsafe { CloseDesktop(desktop) };
    Ok("opened".to_owned())
}

fn find_taskbar() -> Result<String, String> {
    let class = wide("Shell_TrayWnd");
    // SAFETY: the class name is null-terminated, and a null title means any.
    let window = unsafe { FindWindowW(class.as_ptr(), ptr::null()) };
    if window.is_null() {
        return Err(os_error());
    }
    Ok("found the user's taskbar".to_owned())
}

/// Opening the clipboard is allowed, and tells nothing. Reading is what the job forbids.
fn read_clipboard() -> Result<String, String> {
    const CF_UNICODETEXT: u32 = 13;
    // SAFETY: a null window associates the clipboard with this task.
    if unsafe { OpenClipboard(ptr::null_mut()) } == 0 {
        return Err(os_error());
    }
    // SAFETY: the clipboard was just opened by this thread, and is closed right after.
    let data = unsafe { GetClipboardData(CF_UNICODETEXT) };
    let outcome = if data.is_null() {
        Err(os_error())
    } else {
        Ok("read it".to_owned())
    };
    // SAFETY: as above.
    unsafe { CloseClipboard() };
    outcome
}

/// Prints every handle this process holds. What it opened during startup stays open after the
/// lockdown with the rights it had, so this list is the rest of what the worker can touch.
fn list_own_handles() -> Result<(), String> {
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
    const PROCESS_HANDLE_INFORMATION: i32 = 51;
    const HEADER: usize = 2 * mem::size_of::<usize>();
    const OBJECT_NAME_INFORMATION: i32 = 1;
    const OBJECT_TYPE_INFORMATION: i32 = 2;

    let mut buffer = vec![0u64; 1 << 14];
    let mut needed = 0;
    // SAFETY: the buffer and its size match, and `needed` is a valid out-pointer.
    let status = unsafe {
        NtQueryInformationProcess(
            GetCurrentProcess(),
            PROCESS_HANDLE_INFORMATION,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 8) as u32,
            &mut needed,
        )
    };
    if status < 0 {
        return Err(format!("NTSTATUS {:#010x}", status as u32));
    }
    let count = buffer[0] as usize;
    // SAFETY: the kernel wrote `count` entries after the header, inside the buffer.
    let entries = unsafe {
        std::slice::from_raw_parts(
            buffer.as_ptr().cast::<u8>().add(HEADER).cast::<Entry>(),
            count,
        )
    };

    // Reads the string at the start of an `OBJECT_*_INFORMATION`, a `UNICODE_STRING`.
    let query = |handle: HANDLE, class: i32| -> String {
        let mut text = vec![0u64; 512];
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
    };

    for entry in entries {
        let kind = query(entry.handle, OBJECT_TYPE_INFORMATION);
        // Asking a pipe for its name can block forever, so files go by their type instead.
        // SAFETY: `GetFileType` only reads the handle's kind.
        let is_pipe = kind == "File" && unsafe { GetFileType(entry.handle) } == FILE_TYPE_PIPE;
        let name = if is_pipe {
            "<pipe>".to_owned()
        } else {
            query(entry.handle, OBJECT_NAME_INFORMATION)
        };
        println!("handle\t{kind}\t{:#x}\t{name}", entry.granted_access);
    }
    Ok(())
}

/// Whether a copy of the screen holds anything but one colour, meaning something is visible.
fn read_screen() -> Result<String, String> {
    const SIZE: i32 = 64;
    // SAFETY: GDI objects are created, used and released within this block. Any of these can
    // fail in a sandbox, in which case the later calls return an error rather than fault.
    unsafe {
        let screen = GetDC(ptr::null_mut());
        if screen.is_null() {
            return Err(os_error());
        }
        let copy = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, SIZE, SIZE);
        let previous = SelectObject(copy, bitmap);
        let copied = BitBlt(copy, 0, 0, SIZE, SIZE, screen, 0, 0, SRCCOPY);
        let first = GetPixel(copy, 0, 0);
        let varied = (0..SIZE)
            .flat_map(|y| (0..SIZE).map(move |x| (x, y)))
            .any(|(x, y)| GetPixel(copy, x, y) != first);
        SelectObject(copy, previous);
        DeleteObject(bitmap);
        DeleteDC(copy);
        ReleaseDC(ptr::null_mut(), screen);
        if copied == 0 {
            return Err(os_error());
        }
        if varied {
            Ok("copied something visible".to_owned())
        } else {
            Err("the copy holds a single colour: nothing to see here".to_owned())
        }
    }
}

fn read_own_token() -> String {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: the pseudo-handle is always valid, and `token` is a valid out-pointer.
    let thread = unsafe { OpenThreadToken(GetCurrentThread(), 0x0008, 1, &mut token) } != 0;
    if thread {
        // SAFETY: the handle was just opened and is not used again.
        unsafe { CloseHandle(token) };
    }
    format!("thread token present: {thread}")
}

/// Goes through Winsock directly, because `std::net` panics when Winsock cannot initialize,
/// and that is exactly what happens here.
fn start_winsock() -> Result<(), String> {
    // SAFETY: the structure is plain data, filled in by the call below.
    let mut data: WSADATA = unsafe { mem::zeroed() };
    // SAFETY: `data` is a valid out-pointer.
    let code = unsafe { WSAStartup(0x0202, &mut data) };
    if code == 0 {
        Ok(())
    } else {
        Err(format!("Winsock would not start, error {code}"))
    }
}

fn new_socket(kind: i32, protocol: i32) -> Result<SOCKET, String> {
    start_winsock()?;
    // SAFETY: `socket` takes no pointers.
    let new = unsafe { socket(i32::from(AF_INET), kind, protocol) };
    if new == INVALID_SOCKET {
        // SAFETY: reads this thread's last Winsock error.
        return Err(format!("no socket, error {}", unsafe { WSAGetLastError() }));
    }
    Ok(new)
}

fn loopback(port: u16) -> SOCKADDR_IN {
    SOCKADDR_IN {
        sin_family: AF_INET,
        sin_port: port.to_be(),
        sin_addr: IN_ADDR {
            S_un: IN_ADDR_0 {
                S_addr: u32::from_ne_bytes([127, 0, 0, 1]),
            },
        },
        sin_zero: [0; 8],
    }
}

/// Runs a socket operation given the address, and closes the socket afterwards.
fn with_socket(
    kind: i32,
    protocol: i32,
    port: u16,
    operation: impl FnOnce(SOCKET, *const SOCKADDR, i32) -> i32,
    done: &str,
) -> Result<String, String> {
    let open = new_socket(kind, protocol)?;
    let address = loopback(port);
    let result = operation(
        open,
        ptr::from_ref(&address).cast(),
        mem::size_of_val(&address) as i32,
    );
    // SAFETY: reads this thread's last Winsock error, and then closes a socket opened above.
    let outcome = unsafe {
        let error = WSAGetLastError();
        closesocket(open);
        error
    };
    if result == 0 {
        Ok(done.to_owned())
    } else {
        Err(format!("error {outcome}"))
    }
}

fn connect_tcp(port: u16) -> Result<String, String> {
    with_socket(
        SOCK_STREAM,
        IPPROTO_TCP,
        port,
        // SAFETY: the socket is open, and the address is valid for the call.
        |open, address, len| unsafe { connect(open, address, len) },
        "connected",
    )
}

fn listen_tcp() -> Result<String, String> {
    with_socket(
        SOCK_STREAM,
        IPPROTO_TCP,
        0,
        // SAFETY: the socket is open, and the address is valid for the call.
        |open, address, len| unsafe {
            match bind(open, address, len) {
                0 => listen(open, 1),
                error => error,
            }
        },
        "listening",
    )
}

fn bind_udp() -> Result<String, String> {
    with_socket(
        SOCK_DGRAM,
        IPPROTO_UDP,
        0,
        // SAFETY: the socket is open, and the address is valid for the call.
        |open, address, len| unsafe { bind(open, address, len) },
        "bound",
    )
}

fn main() -> ExitCode {
    startup::die_quietly();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            println!("failed\t{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let pdfium = startup::load_pdfium_and_lock_down()?;

    let args: HashMap<String, OsString> = std::env::args_os()
        .skip(1)
        .filter_map(|argument| {
            let text = argument.to_string_lossy().into_owned();
            let (key, value) = text.split_once('=')?;
            Some((key.to_owned(), OsString::from(value)))
        })
        .collect();
    let arg = |key: &str| args.get(key).cloned().unwrap_or_default();
    let text_arg = |key: &str| arg(key).to_string_lossy().into_owned();

    let mut inbox = io::stdin().lock();
    let frame = read_frame(&mut inbox)?.ok_or("the host sent nothing")?;
    let HostMessage::Init(init) = HostMessage::decode(&frame)? else {
        return Err("the first message was not Init".into());
    };

    // What the real worker does, with the handles the host duplicated into this process.
    let Attached { slab, document } = startup::attach(&init)?;
    report(
        "use_document_handle",
        if document.starts_with(b"%PDF") {
            Ok(format!("read {} bytes", document.len()))
        } else {
            Err("the mapped bytes are not a PDF".to_owned())
        },
    );
    let mut opened = Document::open(&pdfium, &document)
        .map_err(|error| format!("PDFium error {}", error.pdfium_error))?;
    let cancel = slab.cancel_cell(0)?;
    let probe = CancelProbe::new(cancel, u64::MAX);
    let target = slab.slot_ptr(0)?;
    let tile = TileKey {
        page: 0,
        lod: 0,
        x: 0,
        y: 0,
    };
    // SAFETY: nothing else writes to this slot, and `slot_ptr` checked it holds a whole tile.
    let rendered = unsafe { opened.render_tile(target, tile, &probe) };
    report(
        "render_into_slab",
        match rendered {
            Ok(RenderOutcome::Finished { .. }) => Ok("rendered a tile".to_owned()),
            other => Err(format!("{other:?}")),
        },
    );

    report("own_token", Ok(read_own_token()));

    // Files, by path.
    report("read_document", open_path(arg("document"), GENERIC_READ));
    report("read_sentinel", open_path(arg("sentinel"), GENERIC_READ));
    let windows = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let under_windows = |tail: &str| {
        let mut path = windows.clone();
        path.push(tail);
        path
    };
    report(
        "read_win_ini",
        open_path(under_windows("\\win.ini"), GENERIC_READ),
    );
    report(
        "read_system_dll",
        open_path(under_windows("\\System32\\kernel32.dll"), GENERIC_READ),
    );
    report(
        "read_system_font",
        open_path(under_windows("\\Fonts\\arial.ttf"), GENERIC_READ),
    );
    report(
        "read_own_executable",
        open_path(std::env::current_exe()?, GENERIC_READ),
    );
    let mut new_in_temp = arg("temp");
    new_in_temp.push("\\probe-new-file.txt");
    report("create_file_in_temp", create_file(new_in_temp));
    let mut new_in_public = arg("public");
    new_in_public.push("\\musaeum-probe-new-file.txt");
    report("create_file_in_public", create_file(new_in_public));
    let mut listing = arg("temp");
    listing.push("\\*");
    report("list_temp", list_directory(listing));
    report("list_drive_root", list_directory("C:\\*"));
    report("open_volume", open_path("\\\\.\\C:", GENERIC_READ));
    report(
        "open_disk",
        open_path("\\\\.\\PhysicalDrive0", GENERIC_READ),
    );
    report(
        "open_with_long_path_prefix",
        open_path(
            {
                let mut path = OsString::from("\\\\?\\");
                path.push(under_windows("\\win.ini"));
                path
            },
            GENERIC_READ,
        ),
    );
    let library = wide("version.dll");
    // SAFETY: the name is null-terminated. A module that loads stays loaded, which is fine.
    let loaded = unsafe { LoadLibraryW(library.as_ptr()) };
    report(
        "load_library",
        if loaded.is_null() {
            Err(os_error())
        } else {
            Ok("loaded".to_owned())
        },
    );

    // The network.
    report(
        "connect_tcp",
        connect_tcp(text_arg("tcp_port").parse().unwrap_or(1)),
    );
    report("listen_tcp", listen_tcp());
    report("bind_udp", bind_udp());
    report("open_afd_driver", open_device("\\Device\\Afd\\Endpoint"));
    report("open_tcp_driver", open_device("\\Device\\Tcp"));
    report("open_nsi_driver", open_device("\\Device\\Nsi"));

    // Other processes and named objects.
    report("start_process", spawn_process(&windows));
    let parent: u32 = text_arg("parent").parse().unwrap_or(0);
    report("open_host_process", open_process(parent));
    report(
        "open_named_pipe",
        open_path(arg("pipe"), GENERIC_READ | GENERIC_WRITE),
    );
    report("open_named_section", open_section(&text_arg("section")));
    report("open_named_event", open_event(&text_arg("event")));
    report(
        "read_user_registry",
        open_registry_key(HKEY_CURRENT_USER, "Software"),
    );
    report(
        "read_machine_registry",
        open_registry_key(HKEY_LOCAL_MACHINE, "SOFTWARE\\Microsoft"),
    );

    // What is left of the user interface.
    report("open_user_desktop", open_user_desktop());
    report("find_taskbar", find_taskbar());
    report("read_clipboard", read_clipboard());
    report("read_screen", read_screen());

    // Mitigations.
    report("make_executable_memory", make_executable_memory());
    report("exceed_memory_limit", allocate(3 << 30));

    list_own_handles()?;

    let mut rest = Vec::new();
    let _ = inbox.read_to_end(&mut rest);
    Ok(())
}
