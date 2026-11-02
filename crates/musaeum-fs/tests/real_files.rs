//! Against a real NTFS volume, because what matters here is what Windows really reports.

use std::ffi::OsString;
use std::fs;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use musaeum_fs::{
    EntryKind, MoveError, OpenError, ReadPattern, has_recycle_bin, list_directory, move_no_replace,
    open_content, stat,
};

/// A temporary directory that is removed at the end of the test.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("musaeum-fs-{}-{number}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn file(&self, name: &str, contents: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, contents).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn entry<'a>(listing: &'a musaeum_fs::Listing, name: &str) -> &'a musaeum_fs::Entry {
    listing
        .entries
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("{name} is not in the listing"))
}

#[test]
fn a_listing_reports_what_a_handle_reports() {
    let dir = TempDir::new();
    let path = dir.file("book.pdf", b"contents");
    fs::create_dir(dir.0.join("sub")).unwrap();

    let listing = list_directory(&dir.0).unwrap();
    let listed = entry(&listing, "book.pdf");
    let opened = stat(&path).unwrap();

    assert_eq!(listing.entries.len(), 2, "no dot entries");
    assert_eq!(listed.kind, EntryKind::File);
    assert_eq!(listed.size, 8);
    assert_eq!(listed.file_id, opened.file_id);
    assert!(listed.file_id.is_some(), "NTFS has lasting IDs");
    assert_eq!(listed.mtime_ns, opened.mtime_ns);
    assert_eq!(listing.volume, opened.volume);
    assert_eq!(entry(&listing, "sub").kind, EntryKind::Directory);
}

#[test]
fn a_rename_keeps_the_id_and_a_replacement_does_not() {
    let dir = TempDir::new();
    let path = dir.file("a.pdf", b"one");
    let id = stat(&path).unwrap().file_id;

    let renamed = dir.0.join("b.pdf");
    fs::rename(&path, &renamed).unwrap();
    assert_eq!(stat(&renamed).unwrap().file_id, id);

    let temporary = dir.file("b.tmp", b"two");
    let temporary_id = stat(&temporary).unwrap().file_id;
    fs::rename(&temporary, &renamed).unwrap();
    assert_eq!(stat(&renamed).unwrap().file_id, temporary_id);
    assert_ne!(temporary_id, id);
}

#[test]
fn a_moved_directory_keeps_its_id_and_everything_in_it_keeps_theirs() {
    let dir = TempDir::new();
    fs::create_dir(dir.0.join("before")).unwrap();
    let inner = dir.0.join("before").join("book.pdf");
    fs::write(&inner, b"x").unwrap();
    let directory_id = list_directory(&dir.0.join("before")).unwrap().directory_id;
    let file_id = stat(&inner).unwrap().file_id;

    fs::rename(dir.0.join("before"), dir.0.join("after")).unwrap();

    let listing = list_directory(&dir.0.join("after")).unwrap();
    assert_eq!(listing.directory_id, directory_id);
    assert_eq!(entry(&listing, "book.pdf").file_id, file_id);
}

#[test]
fn names_too_long_for_the_old_limit_work() {
    let dir = TempDir::new();
    let mut deep = dir.0.clone();
    for _ in 0..6 {
        deep.push("a".repeat(60));
    }
    fs::create_dir_all(&deep).unwrap();
    assert!(deep.as_os_str().len() > 300);
    let file = deep.join("book.pdf");
    fs::write(&file, b"deep").unwrap();

    assert_eq!(entry(&list_directory(&deep).unwrap(), "book.pdf").size, 4);
    assert_eq!(stat(&file).unwrap().size, 4);
    open_content(&file, ReadPattern::Whole).unwrap();
}

#[test]
fn a_name_that_is_not_unicode_survives() {
    let dir = TempDir::new();
    // An unpaired surrogate: not text, but a valid NTFS name.
    let name = OsString::from_wide(&[0xD800, b'x' as u16, b'.' as u16, b'p' as u16]);
    let path = dir.0.join(&name);
    fs::write(&path, b"odd").unwrap();

    let listing = list_directory(&dir.0).unwrap();
    assert_eq!(listing.entries[0].name, name);
    assert_eq!(stat(&path).unwrap().size, 3);
}

#[test]
fn a_trailing_dot_is_a_real_part_of_a_name() {
    let dir = TempDir::new();
    let plain = dir.file("name", b"plain");
    let mut dotted = OsString::from(r"\\?\");
    dotted.push(dir.0.as_os_str());
    dotted.push(r"\name.");
    let dotted = PathBuf::from(dotted);
    fs::write(&dotted, b"dotted!").unwrap();

    assert_eq!(stat(&plain).unwrap().size, 5);
    let listing = list_directory(&dir.0).unwrap();
    assert_eq!(entry(&listing, "name.").size, 7);

    // Built the way the library builds paths, from the directory and the listed name.
    let joined = dir.0.join("name.");
    assert_eq!(stat(&joined).unwrap().size, 7);
    assert_eq!(
        open_content(&joined, ReadPattern::Whole).unwrap().stat.size,
        7
    );
}

#[test]
fn a_junction_is_listed_as_a_link() {
    let dir = TempDir::new();
    let target = dir.0.join("target");
    fs::create_dir(&target).unwrap();
    make_junction(&dir.0.join("link"), &target);

    let listing = list_directory(&dir.0).unwrap();
    assert_eq!(entry(&listing, "link").kind, EntryKind::Link);
    assert_eq!(entry(&listing, "target").kind, EntryKind::Directory);
}

fn make_junction(link: &Path, target: &Path) {
    let status = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn moving_never_overwrites() {
    let dir = TempDir::new();
    let from = dir.file("from.pdf", b"from");
    let to = dir.file("to.pdf", b"to");

    assert!(matches!(
        move_no_replace(&from, &to),
        Err(MoveError::TargetExists)
    ));
    assert_eq!(fs::read(&to).unwrap(), b"to");
    assert_eq!(fs::read(&from).unwrap(), b"from");

    let free = dir.0.join("free.pdf");
    move_no_replace(&from, &free).unwrap();
    assert!(!from.exists());
    assert_eq!(fs::read(&free).unwrap(), b"from");
}

#[test]
fn changing_only_the_case_of_a_name_is_a_move_and_not_a_collision() {
    let dir = TempDir::new();
    let lower = dir.file("book.pdf", b"x");
    let id = stat(&lower).unwrap().file_id;
    move_no_replace(&lower, &dir.0.join("BOOK.pdf")).unwrap();

    let listing = list_directory(&dir.0).unwrap();
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.entries[0].name, "BOOK.pdf");
    assert_eq!(listing.entries[0].file_id, id);
}

#[test]
fn opening_a_missing_file_says_so() {
    let dir = TempDir::new();
    assert!(matches!(
        open_content(&dir.0.join("none.pdf"), ReadPattern::Whole),
        Err(OpenError::Missing)
    ));
    assert!(matches!(
        open_content(&dir.0.join("none").join("none.pdf"), ReadPattern::Whole),
        Err(OpenError::Missing)
    ));
}

#[test]
fn a_file_marked_offline_is_not_opened() {
    let dir = TempDir::new();
    let path = dir.file("offline.pdf", b"x");
    set_attributes(&path, "+O");

    assert!(stat(&path).unwrap().is_cloud_only());
    assert!(entry(&list_directory(&dir.0).unwrap(), "offline.pdf").is_cloud_only());
    assert!(matches!(
        open_content(&path, ReadPattern::Whole),
        Err(OpenError::CloudOnly)
    ));

    set_attributes(&path, "-O");
    assert!(!stat(&path).unwrap().is_cloud_only());
    open_content(&path, ReadPattern::Whole).unwrap();
}

fn set_attributes(path: &Path, change: &str) {
    let status = Command::new("attrib")
        .arg(change)
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
}

/// This is a limit of Windows and not of this code, and it is why reading a file in the background is
/// not completely invisible: while it lasts, someone renaming a folder gets told that it is in use.
#[test]
fn a_held_file_blocks_renaming_the_folder_it_is_in_whatever_the_sharing_mode() {
    let dir = TempDir::new();
    let folder = dir.0.join("folder");
    fs::create_dir(&folder).unwrap();
    let path = folder.join("held.pdf");
    fs::write(&path, b"x").unwrap();
    let renamed = dir.0.join("renamed");

    let held = open_content(&path, ReadPattern::Whole).unwrap();
    let error = fs::rename(&folder, &renamed).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);

    drop(held);
    fs::rename(&folder, &renamed).unwrap();
}

#[test]
fn a_held_file_can_still_be_renamed_and_deleted_by_others() {
    let dir = TempDir::new();
    let path = dir.file("held.pdf", b"x");
    let held = open_content(&path, ReadPattern::Whole).unwrap();
    let renamed = dir.0.join("renamed.pdf");
    fs::rename(&path, &renamed).unwrap();
    fs::remove_file(&renamed).unwrap();
    drop(held);
    assert!(!renamed.exists());
}

#[test]
fn the_system_disk_has_a_recycle_bin() {
    assert!(has_recycle_bin(&std::env::temp_dir()));
}
