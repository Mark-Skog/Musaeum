//! With real placeholders from a real sync provider.
//!
//! This registers a sync root with the Cloud Filter API, the same one OneDrive uses, and plays the
//! provider. If anything reads a placeholder, Windows asks this provider for the bytes, and that is
//! the proof: the tests count those requests. The control test reads a placeholder on purpose, to
//! show that the counter would have seen it.

mod common;

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::mem;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use common::{Fixture, pdf};
use musaeum_fs::{OpenError, ReadPattern, open_content};
use musaeum_library::InstanceState;
use windows_sys::Win32::Storage::CloudFilters::{
    CF_CALLBACK_INFO, CF_CALLBACK_PARAMETERS, CF_CALLBACK_REGISTRATION,
    CF_CALLBACK_TYPE_FETCH_DATA, CF_CALLBACK_TYPE_NONE, CF_CONNECT_FLAG_NONE, CF_CONNECTION_KEY,
    CF_CREATE_FLAG_NONE, CF_DEHYDRATE_FLAG_NONE, CF_FS_METADATA, CF_HARDLINK_POLICY_NONE,
    CF_HYDRATION_POLICY, CF_HYDRATION_POLICY_FULL, CF_HYDRATION_POLICY_MODIFIER_NONE,
    CF_INSYNC_POLICY_NONE, CF_OPERATION_INFO, CF_OPERATION_PARAMETERS, CF_OPERATION_PARAMETERS_0,
    CF_OPERATION_PARAMETERS_0_0, CF_OPERATION_TRANSFER_DATA_FLAG_NONE,
    CF_OPERATION_TYPE_TRANSFER_DATA, CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC,
    CF_PLACEHOLDER_CREATE_INFO, CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT, CF_POPULATION_POLICY,
    CF_POPULATION_POLICY_ALWAYS_FULL, CF_POPULATION_POLICY_MODIFIER_NONE, CF_REGISTER_FLAG_NONE,
    CF_REQUEST_KEY_DEFAULT, CF_SYNC_POLICIES, CF_SYNC_REGISTRATION, CfConnectSyncRoot,
    CfCreatePlaceholders, CfDehydratePlaceholder, CfDisconnectSyncRoot, CfExecute,
    CfRegisterSyncRoot, CfUnregisterSyncRoot,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_NORMAL, FILE_BASIC_INFO, FILE_FLAG_OPEN_NO_RECALL,
};
use windows_sys::core::GUID;

/// How many times Windows asked for each placeholder's bytes, by its identity.
static FETCHES: Mutex<Option<HashMap<u32, u32>>> = Mutex::new(None);
/// What each placeholder has in the cloud, by its identity.
static CLOUD: Mutex<Option<HashMap<u32, Vec<u8>>>> = Mutex::new(None);
static NEXT_IDENTITY: AtomicU32 = AtomicU32::new(1);
/// Held while a sync root is registered and connected, or disconnected and unregistered. The tests
/// run in parallel and all register roots of one provider, and Windows once refused a registration
/// with 0x80070490 (element not found) in 50 runs, with other tests registering and removing roots at
/// the same time. The Cloud Files documentation does not say these calls may overlap, so they no
/// longer do; the tests themselves still run at once.
static REGISTRATION: Mutex<()> = Mutex::new(());

/// The registration lock. A test that failed while holding it leaves nothing half done for the next.
fn registration_lock() -> MutexGuard<'static, ()> {
    REGISTRATION.lock().unwrap_or_else(PoisonError::into_inner)
}

fn fetches(identity: u32) -> u32 {
    FETCHES
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|fetches| fetches.get(&identity).copied())
        .unwrap_or(0)
}

/// What Windows calls when someone needs the bytes of a placeholder.
///
/// # Safety
///
/// Called by the system, with structures valid for the duration of the call.
unsafe extern "system" fn on_fetch_data(
    info: *const CF_CALLBACK_INFO,
    _parameters: *const CF_CALLBACK_PARAMETERS,
) {
    // SAFETY: the system passes a valid structure.
    let info = unsafe { &*info };
    // SAFETY: every placeholder here was created with a `u32` as its identity.
    let identity = unsafe { ptr::read_unaligned(info.FileIdentity.cast::<u32>()) };
    *FETCHES
        .lock()
        .unwrap()
        .get_or_insert_default()
        .entry(identity)
        .or_insert(0) += 1;
    let contents = CLOUD
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|cloud| cloud.get(&identity).cloned())
        .unwrap_or_default();

    let operation = CF_OPERATION_INFO {
        StructSize: mem::size_of::<CF_OPERATION_INFO>() as u32,
        Type: CF_OPERATION_TYPE_TRANSFER_DATA,
        ConnectionKey: info.ConnectionKey,
        TransferKey: info.TransferKey,
        CorrelationVector: ptr::null(),
        SyncStatus: ptr::null(),
        RequestKey: i64::from(CF_REQUEST_KEY_DEFAULT),
    };
    let mut parameters = CF_OPERATION_PARAMETERS {
        ParamSize: mem::size_of::<CF_OPERATION_PARAMETERS>() as u32,
        Anonymous: CF_OPERATION_PARAMETERS_0 {
            TransferData: CF_OPERATION_PARAMETERS_0_0 {
                Flags: CF_OPERATION_TRANSFER_DATA_FLAG_NONE,
                CompletionStatus: 0,
                Buffer: contents.as_ptr().cast(),
                Offset: 0,
                Length: contents.len() as i64,
            },
        },
    };
    // SAFETY: both structures are complete, and the buffer outlives the call. With no callback to
    // answer to if this fails, the read that asked will simply time out and fail the test.
    unsafe { CfExecute(&operation, &mut parameters) };
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

/// A folder registered as a sync root, with this process as its provider.
struct SyncRoot {
    path: PathBuf,
    connection: CF_CONNECTION_KEY,
}

impl SyncRoot {
    fn register(path: &Path) -> Self {
        let wide_path = wide(path.as_os_str());
        let name = wide(OsStr::new("Musaeum test"));
        let version = wide(OsStr::new("1.0"));
        let identity = b"musaeum-test";
        let registration = CF_SYNC_REGISTRATION {
            StructSize: mem::size_of::<CF_SYNC_REGISTRATION>() as u32,
            ProviderName: name.as_ptr(),
            ProviderVersion: version.as_ptr(),
            SyncRootIdentity: identity.as_ptr().cast(),
            SyncRootIdentityLength: identity.len() as u32,
            FileIdentity: ptr::null(),
            FileIdentityLength: 0,
            ProviderId: GUID::from_u128(0x6d757361_6575_6d00_8000_746573740001),
        };
        let policies = CF_SYNC_POLICIES {
            StructSize: mem::size_of::<CF_SYNC_POLICIES>() as u32,
            Hydration: CF_HYDRATION_POLICY {
                Primary: CF_HYDRATION_POLICY_FULL,
                Modifier: CF_HYDRATION_POLICY_MODIFIER_NONE,
            },
            Population: CF_POPULATION_POLICY {
                Primary: CF_POPULATION_POLICY_ALWAYS_FULL,
                Modifier: CF_POPULATION_POLICY_MODIFIER_NONE,
            },
            InSync: CF_INSYNC_POLICY_NONE,
            HardLink: CF_HARDLINK_POLICY_NONE,
            PlaceholderManagement: CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT,
        };
        let registering = registration_lock();
        // SAFETY: the path is null-terminated, and both structures are complete and outlive the call.
        let result = unsafe {
            CfRegisterSyncRoot(
                wide_path.as_ptr(),
                &registration,
                &policies,
                CF_REGISTER_FLAG_NONE,
            )
        };
        assert_eq!(result, 0, "registering the sync root: {result:#x}");

        let callbacks = [
            CF_CALLBACK_REGISTRATION {
                Type: CF_CALLBACK_TYPE_FETCH_DATA,
                Callback: Some(on_fetch_data),
            },
            CF_CALLBACK_REGISTRATION {
                Type: CF_CALLBACK_TYPE_NONE,
                Callback: None,
            },
        ];
        let mut connection = 0;
        // SAFETY: the path is null-terminated, the table ends as it must, and the rest is optional.
        let result = unsafe {
            CfConnectSyncRoot(
                wide_path.as_ptr(),
                callbacks.as_ptr(),
                ptr::null(),
                CF_CONNECT_FLAG_NONE,
                &mut connection,
            )
        };
        assert_eq!(result, 0, "connecting to the sync root: {result:#x}");
        drop(registering);
        Self {
            path: path.to_owned(),
            connection,
        }
    }

    /// Creates a file that exists only in the cloud, returning the identity that counts its fetches.
    fn add_placeholder(&self, name: &str, contents: &[u8]) -> u32 {
        // Any fixed date will do, and it does not change when the file comes down.
        const SOME_TIME_IN_2022: i64 = 133_000_000_000_000_000;

        let identity = NEXT_IDENTITY.fetch_add(1, Ordering::Relaxed);
        CLOUD
            .lock()
            .unwrap()
            .get_or_insert_default()
            .insert(identity, contents.to_vec());
        let base = wide(self.path.as_os_str());
        let relative = wide(OsStr::new(name));
        let identity_bytes = identity.to_le_bytes();
        let mut placeholder = CF_PLACEHOLDER_CREATE_INFO {
            RelativeFileName: relative.as_ptr(),
            FsMetadata: CF_FS_METADATA {
                BasicInfo: FILE_BASIC_INFO {
                    CreationTime: SOME_TIME_IN_2022,
                    LastAccessTime: SOME_TIME_IN_2022,
                    LastWriteTime: SOME_TIME_IN_2022,
                    ChangeTime: SOME_TIME_IN_2022,
                    FileAttributes: FILE_ATTRIBUTE_NORMAL,
                },
                FileSize: contents.len() as i64,
            },
            FileIdentity: identity_bytes.as_ptr().cast(),
            FileIdentityLength: identity_bytes.len() as u32,
            Flags: CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC,
            Result: 0,
            CreateUsn: 0,
        };
        let mut created = 0;
        // SAFETY: every pointer is valid and null-terminated where it should be, for the call.
        let result = unsafe {
            CfCreatePlaceholders(
                base.as_ptr(),
                &mut placeholder,
                1,
                CF_CREATE_FLAG_NONE,
                &mut created,
            )
        };
        assert_eq!(result, 0, "creating a placeholder: {result:#x}");
        assert_eq!(
            placeholder.Result, 0,
            "creating a placeholder: {:#x}",
            placeholder.Result
        );
        identity
    }

    /// Frees a file's local bytes, as "Free up space" does.
    fn dehydrate(&self, name: &str) {
        let result = self.try_dehydrate(name);
        assert_eq!(result, 0, "dehydrating: {result:#x}");
    }

    /// The same, and says how it went.
    fn try_dehydrate(&self, name: &str) -> i32 {
        let file = match fs::OpenOptions::new()
            .write(true)
            .open(self.path.join(name))
        {
            Ok(file) => file,
            Err(error) => return error.raw_os_error().unwrap_or(-1),
        };
        // SAFETY: the handle has write access and is open for the whole call, and -1 means all of it.
        unsafe {
            CfDehydratePlaceholder(
                file.as_raw_handle(),
                0,
                -1,
                CF_DEHYDRATE_FLAG_NONE,
                ptr::null_mut(),
            )
        }
    }
}

impl Drop for SyncRoot {
    fn drop(&mut self) {
        let path = wide(self.path.as_os_str());
        let _unregistering = registration_lock();
        // SAFETY: the connection came from `CfConnectSyncRoot`, and the path is null-terminated.
        unsafe {
            CfDisconnectSyncRoot(self.connection);
            CfUnregisterSyncRoot(path.as_ptr());
        }
    }
}

/// A library whose root is a sync root.
fn cloud_fixture() -> (Fixture, SyncRoot) {
    let fixture = Fixture::new();
    let sync_root = SyncRoot::register(&fixture.root);
    (fixture, sync_root)
}

#[test]
fn a_real_placeholder_has_the_attributes_the_library_looks_for() {
    let (fixture, sync_root) = cloud_fixture();
    let identity = sync_root.add_placeholder("book.pdf", &pdf(1));
    let path = fixture.path("book.pdf");

    let stat = musaeum_fs::stat(&path).unwrap();
    assert!(stat.is_cloud_only(), "attributes: {:#x}", stat.attributes);
    let listing = musaeum_fs::list_directory(&fixture.root).unwrap();
    assert!(listing.entries[0].is_cloud_only());
    assert_eq!(
        stat.size,
        pdf(1).len() as u64,
        "the size is the one in the cloud"
    );
    assert_eq!(fetches(identity), 0, "looking at it fetched nothing");
    assert!(matches!(
        open_content(&path, ReadPattern::Whole),
        Err(OpenError::CloudOnly)
    ));
    assert_eq!(fetches(identity), 0, "refusing to open it fetched nothing");
}

#[test]
fn control_reading_a_placeholder_is_what_downloads_it() {
    let (fixture, sync_root) = cloud_fixture();
    let contents = pdf(1);
    let identity = sync_root.add_placeholder("book.pdf", &contents);
    let path = fixture.path("book.pdf");

    assert_eq!(fs::read(&path).unwrap(), contents);

    assert!(
        fetches(identity) >= 1,
        "a read has to be seen, or the other tests prove nothing"
    );
    assert!(!musaeum_fs::stat(&path).unwrap().is_cloud_only());
}

#[test]
fn scanning_a_placeholder_marks_it_and_downloads_nothing() {
    let (fixture, sync_root) = cloud_fixture();
    let identity = sync_root.add_placeholder("book.pdf", &pdf(1));
    let path = fixture.path("book.pdf");

    let (scan, content) = fixture.settle();

    assert_eq!(scan.new_files, 1);
    assert_eq!(scan.files_opened, 0);
    assert_eq!(scan.bytes_read, 0);
    assert_eq!(content.hashed, 0);
    assert_eq!(fetches(identity), 0, "nothing was downloaded");
    assert_eq!(fixture.state_of(&path), InstanceState::CloudOnly);
    let document = fixture.document_at(&path);
    assert_eq!(
        document.revisions[0].blake3, None,
        "its bytes have not been seen"
    );
    assert_eq!(document.revisions[0].fast_fingerprint, None);
    assert_eq!(document.revisions[0].size, pdf(1).len() as u64);
    assert_eq!(fixture.library().status().unwrap().cloud_only, 1);
}

#[test]
fn the_hash_is_calculated_once_the_file_is_local_and_not_before() {
    let (fixture, sync_root) = cloud_fixture();
    let contents = pdf(1);
    let identity = sync_root.add_placeholder("book.pdf", &contents);
    let path = fixture.path("book.pdf");
    fixture.settle();
    assert_eq!(fetches(identity), 0);
    let before = fixture.identity(&path);

    // The user opens the book, which is what brings its bytes down.
    assert_eq!(fs::read(&path).unwrap(), contents);

    let (scan, content) = fixture.settle();
    assert_eq!(scan.became_local, 1);
    assert_eq!(content.hashed, 1);
    assert_eq!(fixture.state_of(&path), InstanceState::Present);
    assert_eq!(fixture.identity(&path), before);
    let expected = blake3::hash(&contents).to_hex().to_string();
    assert_eq!(
        fixture.document_at(&path).revisions[0].blake3.as_deref(),
        Some(expected.as_str())
    );
}

#[test]
fn freeing_up_space_changes_only_the_state() {
    let (fixture, sync_root) = cloud_fixture();
    let contents = pdf(1);
    let identity = sync_root.add_placeholder("book.pdf", &contents);
    let path = fixture.path("book.pdf");
    assert_eq!(fs::read(&path).unwrap(), contents);
    fixture.settle();
    let cared_for = fixture.mark_as_cared_for(&path);
    let fetched_so_far = fetches(identity);

    sync_root.dehydrate("book.pdf");
    assert!(musaeum_fs::stat(&path).unwrap().is_cloud_only());

    let (scan, content) = fixture.settle();
    assert_eq!(scan.became_cloud_only, 1);
    assert_eq!(scan.bytes_read, 0);
    assert_eq!(content.hashed, 0);
    assert_eq!(fixture.state_of(&path), InstanceState::CloudOnly);
    assert_eq!(fixture.identity(&path), cared_for);
    assert_eq!(fetches(identity), fetched_so_far, "nothing came back down");
    fixture.assert_still_cared_for(&cared_for);

    // And coming back down does not make it a new revision.
    assert_eq!(fs::read(&path).unwrap(), contents);
    let (scan, content) = fixture.settle();
    assert_eq!(scan.became_local, 1);
    assert_eq!(content.hashed, 0, "these bytes are already known");
    assert_eq!(fixture.identity(&path), cared_for);
}

#[test]
fn a_placeholder_can_be_renamed_and_moved_without_downloading_it() {
    let (fixture, sync_root) = cloud_fixture();
    let identity = sync_root.add_placeholder("book.pdf", &pdf(1));
    let path = fixture.path("book.pdf");
    fixture.settle();
    let before = fixture.identity(&path);

    let renamed = fixture.path("renamed.pdf");
    fs::rename(&path, &renamed).unwrap();
    let (scan, _) = fixture.settle();
    assert_eq!(scan.moved, 1);
    assert_eq!(scan.bytes_read, 0);
    assert_eq!(fixture.identity(&renamed), before);
    assert_eq!(fixture.state_of(&renamed), InstanceState::CloudOnly);

    fs::create_dir(fixture.path("shelf")).unwrap();
    let moved = fixture.path("shelf/book.pdf");
    fs::rename(&renamed, &moved).unwrap();
    fixture.settle();
    assert_eq!(fixture.identity(&moved), before);
    assert_eq!(fetches(identity), 0);
}

#[test]
fn a_book_that_comes_back_as_a_placeholder_is_recognized_once_it_is_local() {
    let (fixture, sync_root) = cloud_fixture();
    let contents = pdf(1);
    let path = fixture.write("book.pdf", &contents);
    fixture.settle();
    let cared_for = fixture.mark_as_cared_for(&path);

    // Gone from here. Later, the provider puts it back on another machine's say-so.
    fs::remove_file(&path).unwrap();
    fixture.settle();
    let again = fixture.path("again.pdf");
    let identity = sync_root.add_placeholder("again.pdf", &contents);

    let (_, content) = fixture.settle();
    assert_eq!(fetches(identity), 0);
    assert_eq!(
        content.reattached, 0,
        "there are no bytes to confirm it with yet"
    );
    assert_eq!(fixture.state_of(&again), InstanceState::CloudOnly);

    assert_eq!(fs::read(&again).unwrap(), contents);
    let (_, content) = fixture.settle();
    assert_eq!(content.reattached, 1);
    assert_eq!(fixture.identity(&again), cared_for);
    fixture.assert_still_cared_for(&cared_for);
}

/// Why the attributes have to be checked: the flag that sounds like it would do the job does not.
#[test]
fn the_no_recall_flag_does_not_stop_a_read_from_downloading() {
    let (fixture, sync_root) = cloud_fixture();
    let contents = pdf(1);
    let identity = sync_root.add_placeholder("book.pdf", &contents);

    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_NO_RECALL)
        .open(fixture.path("book.pdf"))
        .unwrap();
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).unwrap();

    assert_eq!(buffer, contents);
    assert!(
        fetches(identity) >= 1,
        "the bytes came down, flag or no flag"
    );
}

/// What makes checking the attributes enough: once the library has a file open and has seen that its
/// bytes are here, they stay here until it has finished reading.
#[test]
fn bytes_cannot_be_freed_while_the_file_is_open() {
    let (fixture, sync_root) = cloud_fixture();
    let contents = pdf(1);
    let identity = sync_root.add_placeholder("book.pdf", &contents);
    let path = fixture.path("book.pdf");
    assert_eq!(fs::read(&path).unwrap(), contents);
    let fetched = fetches(identity);

    let mut held = open_content(&path, ReadPattern::Whole).unwrap();

    let result = sync_root.try_dehydrate("book.pdf");
    assert_ne!(
        result, 0,
        "freeing the bytes has to fail while this is open"
    );

    let mut buffer = Vec::new();
    held.file.read_to_end(&mut buffer).unwrap();
    assert_eq!(buffer, contents);
    assert_eq!(fetches(identity), fetched, "nothing had to come back down");

    drop(held);
    sync_root.dehydrate("book.pdf");
    assert!(musaeum_fs::stat(&path).unwrap().is_cloud_only());
}
