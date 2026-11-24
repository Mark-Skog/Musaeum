//! What the library does to the disk: a journal, a way back, and surviving a crash in the middle.
//!
//! The tests that use the Recycle Bin use the real one, and always take back what they put there.

mod common;

use std::fs;
use std::path::Path;

use common::{Fixture, file_id_of, pdf};
use musaeum_library::{Error, InstanceId, InstanceState};
use rusqlite::params;

fn last_operation_state(fixture: &Fixture) -> String {
    fixture.library().operations(1).unwrap()[0].state.clone()
}

#[test]
fn a_rename_goes_through_the_journal_and_can_be_undone() {
    let fixture = Fixture::new();
    let path = fixture.write("book.pdf", &pdf(1));
    fixture.settle();
    let identity = fixture.mark_as_cared_for(&path);
    let file_id = file_id_of(&path);

    let renamed = fixture.path("renamed.pdf");
    let operation = fixture
        .library()
        .move_file(identity.instance, &renamed)
        .unwrap();

    assert!(!path.exists());
    assert!(renamed.exists());
    assert_eq!(file_id_of(&renamed), file_id);
    assert_eq!(last_operation_state(&fixture), "done");
    assert_eq!(
        fixture.identity(&renamed),
        identity,
        "the library knows right away"
    );

    // A scan finds nothing to do, because it was all recorded.
    let (scan, _) = fixture.settle();
    assert_eq!(scan.unchanged, 1);
    assert_eq!(scan.moved, 0);

    let undo = fixture.library().undo(operation).unwrap();
    assert!(path.exists());
    assert!(!renamed.exists());
    assert_eq!(fixture.identity(&path), identity);
    fixture.assert_still_cared_for(&identity);

    let journal = fixture.library().operations(10).unwrap();
    assert_eq!(journal[0].id, undo);
    assert_eq!(journal[0].undo_of, Some(operation));
    assert_eq!(journal[0].state, "done");
    assert_eq!(journal[1].id, operation);
    assert_eq!(journal[1].state, "undone");
}

#[test]
fn a_move_to_another_folder_can_be_undone() {
    let fixture = Fixture::new();
    let path = fixture.write("here/book.pdf", &pdf(1));
    fs::create_dir(fixture.path("there")).unwrap();
    fixture.settle();
    let identity = fixture.identity(&path);

    let moved = fixture.path("there/book.pdf");
    let operation = fixture
        .library()
        .move_file(identity.instance, &moved)
        .unwrap();
    assert!(moved.exists());
    assert_eq!(fixture.identity(&moved), identity);

    fixture.library().undo(operation).unwrap();
    assert!(path.exists());
    assert!(!moved.exists());
    assert_eq!(fixture.identity(&path), identity);
}

#[test]
fn a_move_never_overwrites_anything() {
    let fixture = Fixture::new();
    let path = fixture.write("book.pdf", &pdf(1));
    let other = fixture.write("other.pdf", &pdf(2));
    fixture.settle();
    let identity = fixture.identity(&path);

    let result = fixture.library().move_file(identity.instance, &other);

    assert!(matches!(result, Err(Error::Refused(_))));
    assert_eq!(
        fs::read(&other).unwrap(),
        pdf(2),
        "the other file is untouched"
    );
    assert_eq!(fs::read(&path).unwrap(), pdf(1), "and so is this one");
    assert_eq!(fixture.identity(&path), identity);
    assert_eq!(last_operation_state(&fixture), "failed");
    assert!(
        fixture.library().operations(1).unwrap()[0]
            .error
            .as_deref()
            .unwrap()
            // "já existe", or "already exists": the message is in the app's language.
            .contains("exist")
    );
}

#[test]
fn a_file_is_not_moved_on_the_strength_of_an_old_record() {
    let fixture = Fixture::new();
    let path = fixture.write("book.pdf", &pdf(1));
    fixture.settle();
    let instance = fixture.identity(&path).instance;

    // A different file at that path, which the library has not seen yet.
    common::safe_save(&path, &pdf(2));

    let result = fixture
        .library()
        .move_file(instance, &fixture.path("renamed.pdf"));
    assert!(matches!(result, Err(Error::Refused(_))), "{result:?}");
    assert!(path.exists());
    assert!(
        fixture.library().operations(1).unwrap().is_empty(),
        "nothing was even attempted"
    );
}

#[test]
fn a_file_cannot_be_moved_out_of_the_library_or_into_a_name_it_would_not_see() {
    let fixture = Fixture::new();
    let path = fixture.write("book.pdf", &pdf(1));
    fixture.settle();
    let instance = fixture.identity(&path).instance;

    let outside = fixture.dir.0.join("outside.pdf");
    assert!(matches!(
        fixture.library().move_file(instance, &outside),
        Err(Error::Refused(_))
    ));
    assert!(matches!(
        fixture
            .library()
            .move_file(instance, &fixture.path("book.docx")),
        Err(Error::Refused(_))
    ));
    assert!(path.exists());
}

#[test]
fn renaming_a_folder_goes_through_the_journal_changes_one_row_and_can_be_undone() {
    let fixture = Fixture::new();
    for number in 0..10 {
        fixture.write(&format!("shelf/deep/book{number}.pdf"), &pdf(number));
    }
    fixture.settle();
    let identity = fixture.mark_as_cared_for(&fixture.path("shelf/deep/book0.pdf"));
    let folder = fixture
        .library()
        .instance(identity.instance)
        .unwrap()
        .folder_id;
    let shelf: i64 = fixture
        .raw()
        .query_row(
            "SELECT parent_id FROM folder WHERE id = ?1",
            [folder],
            |row| row.get(0),
        )
        .unwrap();

    let renamed = fixture.path("renamed");
    let operation = fixture.library().move_folder(shelf, &renamed).unwrap();

    assert!(renamed.join("deep").join("book0.pdf").exists());
    assert_eq!(
        fixture.identity(&renamed.join("deep").join("book0.pdf")),
        identity
    );
    let (scan, _) = fixture.settle();
    assert_eq!(scan.folders_moved, 0, "already recorded");
    assert_eq!(scan.unchanged, 10);
    assert_eq!(scan.bytes_read, 0);

    fixture.library().undo(operation).unwrap();
    assert!(fixture.path("shelf/deep/book0.pdf").exists());
    assert_eq!(
        fixture.identity(&fixture.path("shelf/deep/book0.pdf")),
        identity
    );
    fixture.assert_still_cared_for(&identity);
}

#[test]
fn a_root_cannot_be_renamed_like_a_folder_and_a_folder_cannot_move_into_itself() {
    let fixture = Fixture::new();
    fixture.write("shelf/deep/book.pdf", &pdf(1));
    fixture.settle();
    let raw = fixture.raw();
    let top: i64 = raw
        .query_row("SELECT id FROM folder WHERE parent_id IS NULL", [], |row| {
            row.get(0)
        })
        .unwrap();
    let shelf: i64 = raw
        .query_row("SELECT id FROM folder WHERE name = 'shelf'", [], |row| {
            row.get(0)
        })
        .unwrap();

    assert!(matches!(
        fixture
            .library()
            .move_folder(top, &fixture.dir.0.join("elsewhere")),
        Err(Error::Refused(_))
    ));
    assert!(matches!(
        fixture
            .library()
            .move_folder(shelf, &fixture.path("shelf/deep/inner")),
        Err(Error::Refused(_))
    ));
}

// --- The Recycle Bin ---

/// The record the Recycle Bin keeps for the file it holds at this path.
fn record_of(held_at: &Path) -> std::path::PathBuf {
    let name = held_at.file_name().unwrap().to_str().unwrap();
    held_at.with_file_name(format!("$I{}", name.strip_prefix("$R").unwrap()))
}

/// Whatever happens to a test, nothing of its stays in the user's Recycle Bin. Only items whose original
/// path is inside the test's own temporary folder are touched.
struct LeavesNothingInTheBin(String);

impl LeavesNothingInTheBin {
    fn for_fixture(fixture: &Fixture) -> Self {
        Self(fixture.dir.0.to_string_lossy().to_lowercase())
    }
}

impl Drop for LeavesNothingInTheBin {
    fn drop(&mut self) {
        let Ok(items) = trash::os_limited::list() else {
            return;
        };
        let ours: Vec<_> = items
            .into_iter()
            .filter(|item| {
                item.original_path()
                    .to_string_lossy()
                    .to_lowercase()
                    .starts_with(&self.0)
            })
            .collect();
        if !ours.is_empty() {
            let _ = trash::os_limited::purge_all(ours);
        }
    }
}

#[test]
fn deleting_goes_to_the_recycle_bin_loses_nothing_and_can_be_undone() {
    let fixture = Fixture::new();
    let contents = pdf(1);
    let path = fixture.write("book.pdf", &contents);
    fixture.settle();
    let identity = fixture.mark_as_cared_for(&path);
    let file_id = file_id_of(&path);

    let _clean = LeavesNothingInTheBin::for_fixture(&fixture);
    let operation = fixture.library().recycle_file(identity.instance).unwrap();

    assert!(!path.exists());
    assert_eq!(last_operation_state(&fixture), "done");
    assert_eq!(fixture.state_of(&path), InstanceState::Absent);
    assert_eq!(fixture.identity(&path), identity);
    fixture.assert_still_cared_for(&identity);

    // A scan agrees.
    let (scan, _) = fixture.settle();
    assert_eq!(scan.became_absent, 0);

    let wanted = path.to_string_lossy().to_lowercase();
    let held_at = trash::os_limited::list()
        .unwrap()
        .into_iter()
        .find(|item| item.original_path().to_string_lossy().to_lowercase() == wanted)
        .map(|item| std::path::PathBuf::from(&item.id))
        .expect("the file is in the Recycle Bin");
    fixture.library().undo(operation).unwrap();
    assert!(
        !record_of(&held_at).exists(),
        "nothing stays behind in there"
    );

    assert_eq!(fs::read(&path).unwrap(), contents);
    assert_eq!(file_id_of(&path), file_id, "the same file, not a copy");
    assert_eq!(fixture.state_of(&path), InstanceState::Present);
    assert_eq!(fixture.identity(&path), identity);
    fixture.assert_still_cared_for(&identity);

    let (scan, content) = fixture.settle();
    assert_eq!(scan.unchanged, 1);
    assert_eq!(content.hashed, 0, "nothing needs checking");
    assert_eq!(fixture.library().operations(1).unwrap()[0].kind, "restore");
}

#[test]
fn a_file_the_user_takes_out_of_the_recycle_bin_is_recognized_without_reading_it() {
    let fixture = Fixture::new();
    let path = fixture.write("book.pdf", &pdf(1));
    fixture.settle();
    let identity = fixture.mark_as_cared_for(&path);
    let file_id = file_id_of(&path);

    let _clean = LeavesNothingInTheBin::for_fixture(&fixture);
    let operation = fixture.library().recycle_file(identity.instance).unwrap();
    fixture.settle();

    // The user restores it from the Recycle Bin, and the library is not told anything.
    let wanted = path.to_string_lossy().to_lowercase();
    let item = trash::os_limited::list()
        .unwrap()
        .into_iter()
        .find(|item| item.original_path().to_string_lossy().to_lowercase() == wanted)
        .expect("the file is in the Recycle Bin");
    let held_at = std::path::PathBuf::from(&item.id);
    trash::os_limited::restore_all([item]).unwrap();
    let record = record_of(&held_at);
    assert!(record.exists(), "what restoring this way leaves behind");
    fs::remove_file(record).unwrap();
    assert_eq!(
        file_id_of(&path),
        file_id,
        "the Recycle Bin keeps the File ID"
    );

    let (scan, content) = fixture.settle();
    assert_eq!(scan.restored, 1);
    assert_eq!(scan.bytes_read, 0);
    assert_eq!(content.hashed, 0);
    assert_eq!(fixture.identity(&path), identity);
    assert_eq!(fixture.state_of(&path), InstanceState::Present);
    fixture.assert_still_cared_for(&identity);

    // There is nothing left to undo, and trying says so and changes nothing.
    assert!(fixture.library().undo(operation).is_err());
    assert_eq!(fs::read(&path).unwrap(), pdf(1));
}

#[test]
fn restoring_never_overwrites_what_took_its_place() {
    let fixture = Fixture::new();
    let path = fixture.write("book.pdf", &pdf(1));
    fixture.settle();
    let instance = fixture.identity(&path).instance;

    let _clean = LeavesNothingInTheBin::for_fixture(&fixture);
    let operation = fixture.library().recycle_file(instance).unwrap();
    let new_file = fixture.write("book.pdf", &pdf(2));

    let result = fixture.library().undo(operation);
    assert!(matches!(result, Err(Error::Refused(_))), "{result:?}");
    assert_eq!(fs::read(&new_file).unwrap(), pdf(2));

    // Out of the way, the undo works, and nothing is left in the user's Recycle Bin.
    fs::remove_file(&new_file).unwrap();
    fixture.library().undo(operation).unwrap();
    assert_eq!(fs::read(&path).unwrap(), pdf(1));
}

/// The shell takes these in and then will not give them back, as this test found out the hard way.
#[test]
fn a_path_too_long_for_the_recycle_bin_to_give_back_does_not_go_in() {
    let fixture = Fixture::new();
    let deep = format!("{0}/{0}/{0}/{0}/{0}", "d".repeat(60));
    let path = fixture.write(&format!("{deep}/book.pdf"), &pdf(1));
    assert!(path.as_os_str().len() > 260);
    fixture.settle();
    let instance = fixture.identity(&path).instance;

    let _clean = LeavesNothingInTheBin::for_fixture(&fixture);
    let result = fixture.library().recycle_file(instance);

    assert!(matches!(result, Err(Error::Refused(_))), "{result:?}");
    assert_eq!(fs::read(&path).unwrap(), pdf(1));
    assert_eq!(fixture.state_of(&path), InstanceState::Present);
    assert!(
        fixture.library().operations(1).unwrap().is_empty(),
        "nothing was even attempted"
    );
}

#[test]
fn a_path_right_at_the_limit_goes_in_and_comes_back() {
    let fixture = Fixture::new();
    // 259 characters in all, with the rest of the path being whatever the temporary folder is.
    let name_length = 259 - fixture.root.as_os_str().len() - 1 - "d/".len() - ".pdf".len();
    let relative = format!("d/{}.pdf", "n".repeat(name_length));
    let path = fixture.write(&relative, &pdf(1));
    assert_eq!(path.as_os_str().len(), 259);
    fixture.settle();
    let identity = fixture.identity(&path);

    let _clean = LeavesNothingInTheBin::for_fixture(&fixture);
    let operation = fixture.library().recycle_file(identity.instance).unwrap();
    assert!(!path.exists());
    fixture.library().undo(operation).unwrap();

    assert_eq!(fs::read(&path).unwrap(), pdf(1));
    assert_eq!(fixture.identity(&path), identity);
}

/// This one found out the hard way too: `trash::os_limited::list` fails on the whole Recycle Bin if any
/// one item in it has a name like this. So it must never get there, and this test must not put it there.
#[test]
fn a_name_that_is_not_unicode_does_not_go_in_because_it_would_break_the_whole_bin() {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    let fixture = Fixture::new();
    let name: Vec<u16> = [0xD800]
        .into_iter()
        .chain("book.pdf".encode_utf16())
        .collect();
    let path = fixture.root.join(OsString::from_wide(&name));
    fs::write(&path, pdf(1)).unwrap();
    fixture.settle();
    let instance = fixture.identity(&path).instance;

    let result = fixture.library().recycle_file(instance);

    assert!(matches!(result, Err(Error::Refused(_))), "{result:?}");
    assert_eq!(fs::read(&path).unwrap(), pdf(1));
    assert_eq!(fixture.state_of(&path), InstanceState::Present);
    assert!(
        fixture.library().operations(1).unwrap().is_empty(),
        "nothing was even attempted"
    );
}

#[test]
fn deleting_what_is_not_there_is_refused() {
    let fixture = Fixture::new();
    let path = fixture.write("book.pdf", &pdf(1));
    fixture.settle();
    let instance = fixture.identity(&path).instance;
    fs::remove_file(&path).unwrap();
    fixture.settle();
    assert!(matches!(
        fixture.library().recycle_file(instance),
        Err(Error::Refused(_))
    ));
}

#[test]
fn an_undo_is_not_undone_and_only_what_finished_can_be_undone() {
    let fixture = Fixture::new();
    let path = fixture.write("book.pdf", &pdf(1));
    fixture.settle();
    let instance = fixture.identity(&path).instance;
    let operation = fixture
        .library()
        .move_file(instance, &fixture.path("renamed.pdf"))
        .unwrap();
    let undo = fixture.library().undo(operation).unwrap();

    assert!(matches!(
        fixture.library().undo(undo),
        Err(Error::Refused(_))
    ));
    assert!(matches!(
        fixture.library().undo(operation),
        Err(Error::Refused(_))
    ));
    assert!(matches!(
        fixture.library().undo(9999),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn the_journal_itself_cannot_be_deleted() {
    let mut fixture = Fixture::new();
    let path = fixture.write("book.pdf", &pdf(1));
    fixture.settle();
    let instance = fixture.identity(&path).instance;
    fixture
        .library()
        .move_file(instance, &fixture.path("renamed.pdf"))
        .unwrap();
    fixture.close();

    let raw = rusqlite::Connection::open(fixture.paths.library_db()).unwrap();
    let error = raw
        .execute("DELETE FROM fs_operation", [])
        .unwrap_err()
        .to_string();
    assert!(error.contains("never rewritten"), "{error}");
}

// --- Surviving a crash ---

/// Writes the entry a crash would have left, after closing the library so that this is the only
/// writer, exactly as the row is written before the disk is touched.
fn left_behind_by_a_crash(
    fixture: &mut Fixture,
    kind: &str,
    instance: InstanceId,
    source: &Path,
    target: Option<&Path>,
) {
    fixture.close();
    let raw = rusqlite::Connection::open(fixture.paths.library_db()).unwrap();
    let file_id: Vec<u8> = raw
        .query_row(
            "SELECT file_id FROM file_instance WHERE id = ?1",
            [instance],
            |row| row.get(0),
        )
        .unwrap();
    let hex: String = file_id.iter().map(|byte| format!("{byte:02x}")).collect();
    let volume: i64 = raw
        .query_row(
            "SELECT volume_serial FROM file_instance WHERE id = ?1",
            [instance],
            |row| row.get(0),
        )
        .unwrap();
    let undo = format!(r#"{{"volume":{},"file_id":"{hex}"}}"#, volume as u64);
    raw.execute(
        "INSERT INTO fs_operation (kind, state, instance_id, source_path, target_path, undo_data, created_at)
         VALUES (?1, 'intent', ?2, ?3, ?4, ?5, 0)",
        params![
            kind,
            instance,
            source.to_str().unwrap(),
            target.map(|path| path.to_str().unwrap()),
            undo
        ],
    )
    .unwrap();
}

#[test]
fn a_crash_before_touching_the_disk_means_it_never_happened() {
    let mut fixture = Fixture::new();
    let path = fixture.write("book.pdf", &pdf(1));
    fixture.settle();
    let identity = fixture.identity(&path);
    let target = fixture.path("renamed.pdf");

    left_behind_by_a_crash(
        &mut fixture,
        "move_file",
        identity.instance,
        &path,
        Some(&target),
    );
    fixture.reopen();

    assert_eq!(fixture.library().recovery_report().aborted, 1);
    assert_eq!(last_operation_state(&fixture), "aborted");
    assert_eq!(fixture.identity(&path), identity);
    assert!(path.exists());
    assert!(!target.exists());
}

#[test]
fn a_crash_after_touching_the_disk_means_it_happened() {
    let mut fixture = Fixture::new();
    let path = fixture.write("book.pdf", &pdf(1));
    fixture.settle();
    let identity = fixture.identity(&path);
    let target = fixture.path("renamed.pdf");

    left_behind_by_a_crash(
        &mut fixture,
        "move_file",
        identity.instance,
        &path,
        Some(&target),
    );
    // The move reached the disk, and the crash came before the outcome was recorded.
    fs::rename(&path, &target).unwrap();
    fixture.reopen();

    assert_eq!(fixture.library().recovery_report().completed, 1);
    assert_eq!(last_operation_state(&fixture), "done");
    assert_eq!(fixture.identity(&target), identity);
    assert_eq!(fixture.state_of(&target), InstanceState::Present);
}

#[test]
fn a_crash_with_the_disk_in_neither_state_is_left_for_the_next_scan() {
    let mut fixture = Fixture::new();
    let path = fixture.write("book.pdf", &pdf(1));
    fixture.settle();
    let identity = fixture.identity(&path);
    let target = fixture.path("renamed.pdf");

    left_behind_by_a_crash(
        &mut fixture,
        "move_file",
        identity.instance,
        &path,
        Some(&target),
    );
    fs::remove_file(&path).unwrap();
    fixture.reopen();

    assert_eq!(fixture.library().recovery_report().failed, 1);
    assert_eq!(last_operation_state(&fixture), "failed");
    fixture.settle();
    assert_eq!(fixture.state_of(&path), InstanceState::Absent);
}

#[test]
fn a_crash_while_recycling_is_settled_by_whether_the_file_is_still_there() {
    // Never reached the Recycle Bin.
    let mut fixture = Fixture::new();
    let path = fixture.write("book.pdf", &pdf(1));
    fixture.settle();
    let identity = fixture.identity(&path);
    left_behind_by_a_crash(&mut fixture, "recycle", identity.instance, &path, None);
    fixture.reopen();
    assert_eq!(fixture.library().recovery_report().aborted, 1);
    assert_eq!(fixture.state_of(&path), InstanceState::Present);

    // Reached it.
    left_behind_by_a_crash(&mut fixture, "recycle", identity.instance, &path, None);
    fs::remove_file(&path).unwrap();
    fixture.reopen();
    assert_eq!(fixture.library().recovery_report().completed, 1);
    assert_eq!(fixture.state_of(&path), InstanceState::Absent);
    assert_eq!(fixture.identity(&path), identity);
}
