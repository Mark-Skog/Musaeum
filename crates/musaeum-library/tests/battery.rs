//! The Phase 1 gate: a file's identity survives what happens to files in real life.
//!
//! Each scenario does the real thing to a real file on a real volume, and then checks both halves of
//! the promise: the library still knows which file it is, and what a person put on it is still
//! there. Where a scenario promises not to read the file, the test proves that as well. The one
//! thing a test machine cannot produce on demand is a second volume, which the `changing_volume`
//! test simulates and says so.

mod common;

use std::fs;
use std::path::Path;

use common::{
    Fixture, Identity, TempDir, epub, file_id_of, pdf, pdf_with_id, redirecting_fixture, safe_save,
};
use musaeum_fs::FileId;
use musaeum_library::{
    Error, InstanceState, Library, ReconcileScope, RevisionOrigin, SuggestionKind,
};
use std::os::windows::fs::OpenOptionsExt;
use std::time::Duration;

/// A file held open with no sharing at all, so that anything trying to read it would fail.
fn hold_exclusively(path: &Path) -> fs::File {
    fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .unwrap()
}

/// A file scanned and hashed, with something a person cares about on it.
fn cared_for_book(fixture: &Fixture, relative: &str, seed: u64) -> (std::path::PathBuf, Identity) {
    let path = fixture.write(relative, &pdf(seed));
    fixture.settle();
    let identity = fixture.mark_as_cared_for(&path);
    (path, identity)
}

// --- Renaming a file ---

#[test]
fn renaming_a_file_keeps_its_identity_without_reading_it() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for_book(&fixture, "before.pdf", 1);

    let renamed = fixture.path("after.pdf");
    fs::rename(&path, &renamed).unwrap();
    let _held = hold_exclusively(&renamed);

    let (scan, content) = fixture.settle();
    assert_eq!(scan.moved, 1);
    assert_eq!(scan.files_opened, 0, "the file must not be opened");
    assert_eq!(scan.bytes_read, 0, "the file must not be read");
    assert_eq!(content.hashed, 0, "there was nothing to hash again");

    assert_eq!(fixture.identity(&renamed), identity);
    assert_eq!(fixture.state_of(&renamed), InstanceState::Present);
    assert_eq!(fixture.document_count(), 1);
    fixture.assert_still_cared_for(&identity);
    assert!(
        fixture.library().instance_at(&path).unwrap().is_none(),
        "the old name is not a record of its own"
    );
}

#[test]
fn renaming_a_file_only_by_case_is_a_rename() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for_book(&fixture, "book.pdf", 1);

    let renamed = fixture.path("BOOK.pdf");
    fs::rename(&path, &renamed).unwrap();

    let (scan, _) = fixture.settle();
    assert_eq!(scan.moved, 1);
    assert_eq!(scan.bytes_read, 0);
    assert_eq!(fixture.identity(&renamed), identity);
}

#[test]
fn two_files_swapping_names_are_each_still_themselves() {
    let fixture = Fixture::new();
    let (a, identity_a) = cared_for_book(&fixture, "a.pdf", 1);
    let b = fixture.write("b.pdf", &pdf(2));
    fixture.settle();
    let identity_b = fixture.identity(&b);

    let middle = fixture.dir.0.join("middle");
    fs::rename(&a, &middle).unwrap();
    fs::rename(&b, &a).unwrap();
    fs::rename(&middle, &b).unwrap();

    let (scan, _) = fixture.settle();
    assert_eq!(scan.moved, 2);
    assert_eq!(scan.bytes_read, 0);
    assert_eq!(fixture.identity(&b).document, identity_a.document);
    assert_eq!(fixture.identity(&a).document, identity_b.document);
    assert_eq!(fixture.library().status().unwrap().absent, 0);
    fixture.assert_still_cared_for(&identity_a);
}

#[test]
fn moving_a_file_to_another_folder_keeps_its_identity() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for_book(&fixture, "here/book.pdf", 1);

    fs::create_dir(fixture.path("there")).unwrap();
    let moved = fixture.path("there/book.pdf");
    fs::rename(&path, &moved).unwrap();

    let (scan, _) = fixture.settle();
    assert_eq!(scan.moved, 1);
    assert_eq!(scan.bytes_read, 0);
    assert_eq!(fixture.identity(&moved), identity);
    fixture.assert_still_cared_for(&identity);
}

// --- Moving a whole folder ---

/// Every row of a table, as text, to tell whether anything at all changed.
fn dump(connection: &rusqlite::Connection, table: &str) -> Vec<String> {
    let mut statement = connection
        .prepare(&format!("SELECT * FROM {table} ORDER BY id"))
        .unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| {
            let mut text = String::new();
            for column in 0..columns {
                let value: rusqlite::types::Value = row.get(column)?;
                text.push_str(&format!("{value:?}|"));
            }
            Ok(text)
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn renaming_a_folder_changes_one_row_and_reads_nothing() {
    let fixture = Fixture::new();
    for number in 0..200 {
        fixture.write(
            &format!("shelf/sub{}/book{number}.pdf", number % 5),
            &pdf(number),
        );
    }
    fixture.settle();
    let identity = fixture.mark_as_cared_for(&fixture.path("shelf/sub0/book0.pdf"));

    let raw = fixture.raw();
    let folders_before = dump(&raw, "folder");
    let files_before = dump(&raw, "file_instance");

    fs::rename(fixture.path("shelf"), fixture.path("renamed")).unwrap();

    let (scan, content) = fixture.settle();
    assert_eq!(scan.folders_moved, 1);
    assert_eq!(
        scan.moved, 0,
        "the files did not move relative to their folder"
    );
    assert_eq!(scan.bytes_read, 0);
    assert_eq!(scan.files_opened, 0);
    assert_eq!(content.hashed, 0);

    assert_eq!(
        dump(&raw, "file_instance"),
        files_before,
        "not one row of file_instance may change"
    );
    let folders_after = dump(&raw, "folder");
    let changed = folders_before
        .iter()
        .zip(&folders_after)
        .filter(|(before, after)| before != after)
        .count();
    assert_eq!(changed, 1, "exactly one row of folder must change");
    assert_eq!(folders_before.len(), folders_after.len());

    let moved = fixture.path("renamed/sub0/book0.pdf");
    assert_eq!(fixture.identity(&moved), identity);
    fixture.assert_still_cared_for(&identity);
}

#[test]
fn moving_a_folder_into_another_folder_keeps_everything_in_it() {
    let fixture = Fixture::new();
    let (_, identity) = cared_for_book(&fixture, "a/b/book.pdf", 1);
    fs::create_dir(fixture.path("c")).unwrap();

    fs::rename(fixture.path("a/b"), fixture.path("c/b")).unwrap();

    let (scan, _) = fixture.settle();
    assert_eq!(scan.folders_moved, 1);
    assert_eq!(scan.bytes_read, 0);
    assert_eq!(fixture.identity(&fixture.path("c/b/book.pdf")), identity);
    assert_eq!(fixture.library().status().unwrap().absent, 0);
}

#[test]
fn two_folders_swapping_names_keep_what_is_in_them() {
    let fixture = Fixture::new();
    let (_, identity_a) = cared_for_book(&fixture, "a/book.pdf", 1);
    let b = fixture.write("b/book.pdf", &pdf(2));
    fixture.settle();
    let identity_b = fixture.identity(&b);

    fs::rename(fixture.path("a"), fixture.path("x")).unwrap();
    fs::rename(fixture.path("b"), fixture.path("a")).unwrap();
    fs::rename(fixture.path("x"), fixture.path("b")).unwrap();

    let (scan, _) = fixture.settle();
    assert_eq!(scan.bytes_read, 0);
    assert_eq!(fixture.identity(&fixture.path("b/book.pdf")), identity_a);
    assert_eq!(fixture.identity(&fixture.path("a/book.pdf")), identity_b);
}

#[test]
fn a_folder_moved_between_roots_keeps_what_is_in_it() {
    let fixture = Fixture::new();
    let (_, identity) = cared_for_book(&fixture, "shelf/deep/book.pdf", 1);

    let second = fixture.dir.0.join("other-books");
    fs::create_dir(&second).unwrap();
    let second_root = fixture.library().add_root(&second, &[]).unwrap().id;

    fs::rename(fixture.path("shelf"), second.join("shelf")).unwrap();
    fixture.library().reconcile_all().unwrap();

    let moved = second.join("shelf").join("deep").join("book.pdf");
    assert_eq!(fixture.identity(&moved), identity);
    let raw = fixture.raw();
    let roots: Vec<i64> = raw
        .prepare("SELECT DISTINCT root_id FROM folder WHERE name IN ('shelf', 'deep')")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(roots, [second_root], "the whole subtree changed root");
}

// --- Changing volume ---

/// SIMULATED: this machine has one volume with a letter, and creating another takes administrator
/// rights. The files and the bytes are real, and the file really gets a new File ID from being
/// copied. What the probe replaces is the volume's serial number, which the identity pipeline only
/// compares. `tests/real_volume.rs` does this for real, on a mounted disk image.
#[test]
fn changing_volume_keeps_identity_by_full_hash() {
    let (fixture, probe) = redirecting_fixture();
    let (path, identity) = cared_for_book(&fixture, "book.pdf", 1);
    let old_id = file_id_of(&path);

    let elsewhere = fixture.dir.0.join("second-volume");
    fs::create_dir(&elsewhere).unwrap();
    probe
        .volumes
        .lock()
        .unwrap()
        .push((elsewhere.clone(), 0xABCD_0000_1234));
    let second_root = fixture.library().add_root(&elsewhere, &[]).unwrap().id;

    // What moving across volumes is: a copy, and then deleting the original.
    let copied = elsewhere.join("book.pdf");
    fs::copy(&path, &copied).unwrap();
    fs::remove_file(&path).unwrap();
    assert_ne!(file_id_of(&copied), old_id, "a copy is another file");

    fixture.library().reconcile_all().unwrap();
    let after_scan = fixture.identity(&copied);
    assert_ne!(
        after_scan.document, identity.document,
        "nothing is concluded before the full hash"
    );
    assert_eq!(fixture.state_of(&path), InstanceState::Absent);

    let content = fixture.process();
    assert_eq!(content.reattached, 1);

    let confirmed = fixture.identity(&copied);
    assert_eq!(
        confirmed, identity,
        "the old row now says where the file is"
    );
    assert_eq!(fixture.state_of(&copied), InstanceState::Present);
    assert_eq!(fixture.document_count(), 1);
    fixture.assert_still_cared_for(&identity);
    assert!(
        fixture
            .library()
            .status()
            .unwrap()
            .roots
            .iter()
            .any(|root| root.id == second_root)
    );
}

#[test]
fn changing_volume_with_edits_along_the_way_is_a_new_document_and_not_a_guess() {
    let (fixture, probe) = redirecting_fixture();
    let (path, identity) = cared_for_book(&fixture, "book.pdf", 1);

    let elsewhere = fixture.dir.0.join("second-volume");
    fs::create_dir(&elsewhere).unwrap();
    probe
        .volumes
        .lock()
        .unwrap()
        .push((elsewhere.clone(), 0xABCD));
    fixture.library().add_root(&elsewhere, &[]).unwrap();

    let moved = elsewhere.join("book.pdf");
    fs::write(&moved, pdf(2)).unwrap();
    fs::remove_file(&path).unwrap();

    fixture.library().reconcile_all().unwrap();
    fixture.process();

    assert_ne!(fixture.identity(&moved).document, identity.document);
    assert_eq!(fixture.state_of(&path), InstanceState::Absent);
    fixture.assert_still_cared_for(&identity);
}

// --- Safe save ---

#[test]
fn a_safe_save_with_new_contents_is_a_new_revision_of_the_same_document() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for_book(&fixture, "book.pdf", 1);
    let old_file_id = file_id_of(&path);

    safe_save(&path, &pdf(2));
    assert_ne!(file_id_of(&path), old_file_id, "this is what makes it hard");

    let (scan, content) = fixture.settle();
    assert_eq!(scan.replaced_at_path, 1);
    assert_eq!(content.new_revisions, 1);

    let document = fixture.document_at(&path);
    assert_eq!(document.id, identity.document, "still the same document");
    assert_eq!(document.revisions.len(), 2, "the old revision stays");
    let new = &document.revisions[1];
    assert_eq!(new.origin, RevisionOrigin::ReplacedAtPath);
    assert_eq!(
        new.previous_revision_id,
        Some(identity.revision),
        "the link that re-anchoring needs"
    );
    assert!(new.blake3.is_some());
    assert_eq!(new.instances[0].id, identity.instance, "the same instance");
    assert_eq!(document.annotations[0].revision_id, identity.revision);
    assert_eq!(fixture.document_count(), 1);
    fixture.assert_still_cared_for(&identity);
}

#[test]
fn a_safe_save_with_the_same_contents_changes_nothing_but_the_file_id() {
    let fixture = Fixture::new();
    let contents = pdf(1);
    let path = fixture.write("book.pdf", &contents);
    fixture.settle();
    let identity = fixture.mark_as_cared_for(&path);

    safe_save(&path, &contents);

    let (_, content) = fixture.settle();
    assert_eq!(content.same_as_previous, 1);
    assert_eq!(fixture.identity(&path), identity);
    let document = fixture.document_at(&path);
    assert_eq!(
        document.revisions.len(),
        1,
        "the same bytes are the same revision"
    );
    assert_eq!(
        document.revisions[0].instances[0].file_id.as_deref(),
        Some(hex(file_id_of(&path)).as_str())
    );
    fixture.assert_still_cared_for(&identity);
}

fn hex(id: FileId) -> String {
    id.0.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `ReplaceFile`, which is what Office and others call.
fn replace_file(replaced: &Path, replacement: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

    let wide = |path: &Path| -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    let (replaced, replacement) = (wide(replaced), wide(replacement));
    // SAFETY: both paths are null-terminated and outlive the call, and the rest are optional.
    let ok = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_ne!(ok, 0, "{}", std::io::Error::last_os_error());
}

#[test]
fn replace_file_is_a_new_revision_of_the_same_document() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for_book(&fixture, "book.pdf", 1);

    let temporary = fixture.path("book.pdf.tmp");
    fs::write(&temporary, pdf(2)).unwrap();
    replace_file(&path, &temporary);

    let (_, content) = fixture.settle();
    assert_eq!(content.new_revisions, 1);
    let document = fixture.document_at(&path);
    assert_eq!(document.id, identity.document);
    assert_eq!(document.revisions.len(), 2);
    assert_eq!(
        document.revisions[1].previous_revision_id,
        Some(identity.revision)
    );
    fixture.assert_still_cared_for(&identity);
}

#[test]
fn replacing_in_steps_between_two_scans_looks_the_same_as_in_one() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for_book(&fixture, "book.pdf", 1);

    // What simpler programs do: there is a moment when the file is not there at all.
    let temporary = fixture.path("book.tmp");
    fs::write(&temporary, pdf(2)).unwrap();
    fs::remove_file(&path).unwrap();
    fs::rename(&temporary, &path).unwrap();

    fixture.settle();
    let document = fixture.document_at(&path);
    assert_eq!(document.id, identity.document);
    assert_eq!(document.revisions.len(), 2);
    fixture.assert_still_cared_for(&identity);
}

#[test]
fn keeping_the_old_version_beside_the_new_one_keeps_both() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for_book(&fixture, "book.pdf", 1);

    // The old file moves aside with its identity, and a new file takes its place.
    let backup = fixture.path("book-old.pdf");
    fs::rename(&path, &backup).unwrap();
    fs::write(&path, pdf(2)).unwrap();

    let (scan, _) = fixture.settle();
    assert_eq!(scan.moved, 1);
    assert_eq!(
        fixture.identity(&backup),
        identity,
        "identity follows the file"
    );
    assert_ne!(fixture.identity(&path).document, identity.document);
    fixture.assert_still_cared_for(&identity);
}

// --- Editing ---

#[test]
fn an_edit_in_place_is_a_new_revision_of_the_same_document() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for_book(&fixture, "book.pdf", 1);
    let file_id = file_id_of(&path);

    fs::write(&path, pdf(2)).unwrap();
    assert_eq!(file_id_of(&path), file_id);

    let (scan, _) = fixture.settle();
    assert_eq!(scan.changed_in_place, 1);
    let document = fixture.document_at(&path);
    assert_eq!(document.id, identity.document);
    assert_eq!(document.revisions.len(), 2);
    assert_eq!(document.revisions[1].origin, RevisionOrigin::ChangedInPlace);
    assert_eq!(
        document.revisions[1].previous_revision_id,
        Some(identity.revision)
    );
    fixture.assert_still_cared_for(&identity);
}

#[test]
fn a_new_date_on_the_same_bytes_is_not_a_new_revision() {
    let fixture = Fixture::new();
    let contents = pdf(1);
    let path = fixture.write("book.pdf", &contents);
    fixture.settle();
    let identity = fixture.identity(&path);

    let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        .unwrap();
    drop(file);

    let (scan, content) = fixture.settle();
    assert_eq!(scan.changed_in_place, 1, "only the bytes can tell");
    assert_eq!(content.same_as_previous, 1);
    assert_eq!(fixture.identity(&path), identity);
    assert_eq!(fixture.document_at(&path).revisions.len(), 1);
}

#[test]
fn going_back_to_old_bytes_is_the_old_revision() {
    let fixture = Fixture::new();
    let (first, second) = (pdf(1), pdf(2));
    let path = fixture.write("book.pdf", &first);
    fixture.settle();
    let original = fixture.identity(&path);
    fs::write(&path, &second).unwrap();
    fixture.settle();

    fs::write(&path, &first).unwrap();
    fixture.settle();

    assert_eq!(fixture.identity(&path).revision, original.revision);
    let document = fixture.document_at(&path);
    assert_eq!(
        document.revisions.len(),
        2,
        "no third copy of the same bytes"
    );
}

#[test]
fn a_file_that_keeps_changing_before_being_hashed_leaves_no_trail() {
    let fixture = Fixture::new();
    let path = fixture.write("book.pdf", &pdf(1));
    fixture.reconcile();
    fs::write(&path, pdf(2)).unwrap();
    fixture.reconcile();
    fs::write(&path, pdf(3)).unwrap();
    fixture.reconcile();

    fixture.process();
    assert_eq!(
        fixture.document_at(&path).revisions.len(),
        1,
        "nothing was ever concluded about the versions in between"
    );
}

// --- Restoring what went missing ---

#[test]
fn a_deleted_file_goes_absent_and_nothing_is_lost() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for_book(&fixture, "book.pdf", 1);

    fs::remove_file(&path).unwrap();
    let (scan, _) = fixture.settle();

    assert_eq!(scan.became_absent, 1);
    assert_eq!(fixture.state_of(&path), InstanceState::Absent);
    assert_eq!(fixture.identity(&path), identity);
    assert_eq!(fixture.document_at(&path).revisions.len(), 1);
    fixture.assert_still_cared_for(&identity);
}

#[test]
fn a_file_moved_out_of_the_root_goes_absent_and_moving_it_back_restores_it_without_reading() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for_book(&fixture, "book.pdf", 1);

    let outside = fixture.dir.0.join("outside.pdf");
    fs::rename(&path, &outside).unwrap();
    fixture.settle();
    assert_eq!(fixture.state_of(&path), InstanceState::Absent);

    fs::rename(&outside, &path).unwrap();
    let (scan, content) = fixture.settle();

    assert_eq!(scan.restored, 1);
    assert_eq!(scan.bytes_read, 0);
    assert_eq!(content.hashed, 0);
    assert_eq!(fixture.state_of(&path), InstanceState::Present);
    assert_eq!(fixture.identity(&path), identity);
    fixture.assert_still_cared_for(&identity);
}

#[test]
fn a_file_coming_back_under_another_name_is_recognized_without_reading() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for_book(&fixture, "book.pdf", 1);

    let outside = fixture.dir.0.join("outside.pdf");
    fs::rename(&path, &outside).unwrap();
    fixture.settle();

    let back = fixture.path("come-back.pdf");
    fs::rename(&outside, &back).unwrap();
    let (scan, _) = fixture.settle();

    assert_eq!(scan.restored, 1);
    assert_eq!(scan.bytes_read, 0);
    assert_eq!(fixture.identity(&back), identity);
}

#[test]
fn a_restored_copy_of_the_same_bytes_is_recognized_by_its_full_hash() {
    let fixture = Fixture::new();
    let contents = pdf(1);
    let path = fixture.write("book.pdf", &contents);
    fixture.settle();
    let identity = fixture.mark_as_cared_for(&path);

    fs::remove_file(&path).unwrap();
    fixture.settle();

    // A backup restored: the same bytes, and in every other way a different file.
    let restored = fixture.write("restored/book.pdf", &contents);
    let (scan, content) = fixture.settle();

    assert_eq!(
        scan.candidates_named, 1,
        "the fingerprint named the candidate"
    );
    assert_eq!(content.reattached, 1, "the full hash confirmed it");
    assert_eq!(fixture.identity(&restored), identity);
    assert_eq!(fixture.state_of(&restored), InstanceState::Present);
    assert_eq!(fixture.document_count(), 1);
    fixture.assert_still_cared_for(&identity);
}

#[test]
fn what_the_fingerprint_cannot_tell_apart_the_full_hash_does() {
    let fixture = Fixture::new();
    // Different only in a place the fast fingerprint does not look.
    let mut contents = vec![b'x'; 4_000_000];
    contents[..8].copy_from_slice(b"%PDF-1.7");
    let path = fixture.write("book.pdf", &contents);
    fixture.settle();
    let identity = fixture.mark_as_cared_for(&path);
    fs::remove_file(&path).unwrap();
    fixture.settle();

    let mut almost = contents.clone();
    almost[64 * 1024 + 40_000] = b'y';
    let other = fixture.write("other.pdf", &almost);
    let (scan, content) = fixture.settle();

    assert_eq!(
        scan.candidates_named, 1,
        "the fingerprint is fooled, as it may be"
    );
    assert_eq!(content.reattached, 0, "the full hash is not");
    assert_ne!(fixture.identity(&other).document, identity.document);
    assert_eq!(fixture.state_of(&path), InstanceState::Absent);
    fixture.assert_still_cared_for(&identity);
}

#[test]
fn something_else_at_the_path_of_a_missing_file_is_only_a_suggestion() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for_book(&fixture, "book.pdf", 1);
    fs::remove_file(&path).unwrap();
    fixture.settle();

    fs::write(&path, pdf(2)).unwrap();
    fixture.settle();

    let newcomer = fixture.identity(&path);
    assert_ne!(newcomer.document, identity.document);
    let suggestions = fixture.library().suggestions(true).unwrap();
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].kind, SuggestionKind::SamePath);
    assert_eq!(suggestions[0].document_id, newcomer.document);
    assert_eq!(suggestions[0].candidate_document_id, identity.document);
    fixture.assert_still_cared_for(&identity);
}

#[test]
fn a_missing_file_that_was_never_hashed_can_only_be_suggested() {
    let fixture = Fixture::new();
    let contents = pdf(1);
    let path = fixture.write("book.pdf", &contents);
    fixture.reconcile();
    let original = fixture.identity(&path);

    fs::remove_file(&path).unwrap();
    fixture.reconcile();

    let copy = fixture.write("copy.pdf", &contents);
    fixture.settle();

    assert_ne!(fixture.identity(&copy).document, original.document);
    let suggestions = fixture.library().suggestions(true).unwrap();
    assert!(
        suggestions.iter().any(
            |suggestion| suggestion.kind == SuggestionKind::SameFingerprint
                && suggestion.candidate_document_id == original.document
        ),
        "{suggestions:?}"
    );
}

// --- Changes while closed ---

#[test]
fn everything_that_happens_while_the_program_is_closed_is_found_at_the_next_start() {
    let mut fixture = Fixture::new();
    let renamed_from = fixture.write("rename.pdf", &pdf(1));
    let moved_from = fixture.write("move/book.pdf", &pdf(2));
    let edited = fixture.write("edit.pdf", &pdf(3));
    let saved = fixture.write("save.pdf", &pdf(4));
    let deleted = fixture.write("delete.pdf", &pdf(5));
    let folder_book = fixture.write("folder/inner/book.pdf", &pdf(6));
    let copied_contents = pdf(7);
    let replaced_by_copy = fixture.write("recreate.pdf", &copied_contents);
    fixture.settle();

    let identities = [
        fixture.mark_as_cared_for(&renamed_from),
        fixture.mark_as_cared_for(&moved_from),
        fixture.mark_as_cared_for(&edited),
        fixture.mark_as_cared_for(&saved),
        fixture.mark_as_cared_for(&deleted),
        fixture.mark_as_cared_for(&folder_book),
        fixture.mark_as_cared_for(&replaced_by_copy),
    ];

    fixture.close();

    let renamed_to = fixture.path("renamed.pdf");
    fs::rename(&renamed_from, &renamed_to).unwrap();
    fs::create_dir(fixture.path("moved")).unwrap();
    let moved_to = fixture.path("moved/book.pdf");
    fs::rename(&moved_from, &moved_to).unwrap();
    fs::write(&edited, pdf(30)).unwrap();
    safe_save(&saved, &pdf(40));
    fs::remove_file(&deleted).unwrap();
    fs::rename(fixture.path("folder"), fixture.path("folder-renamed")).unwrap();
    fs::remove_file(&replaced_by_copy).unwrap();
    let recreated = fixture.write("recreated-elsewhere.pdf", &copied_contents);
    let added = fixture.write("added.pdf", &pdf(8));

    fixture.reopen();
    let (scan, content) = fixture.settle();

    assert_eq!(fixture.identity(&renamed_to), identities[0], "renamed");
    assert_eq!(fixture.identity(&moved_to), identities[1], "moved");
    assert_eq!(
        fixture.identity(&edited).document,
        identities[2].document,
        "edited"
    );
    assert_eq!(fixture.document_at(&edited).revisions.len(), 2);
    assert_eq!(
        fixture.identity(&saved).document,
        identities[3].document,
        "safe-saved"
    );
    assert_eq!(fixture.document_at(&saved).revisions.len(), 2);
    assert_eq!(fixture.state_of(&deleted), InstanceState::Absent, "deleted");
    assert_eq!(
        fixture.identity(&fixture.path("folder-renamed/inner/book.pdf")),
        identities[5],
        "folder renamed"
    );
    assert_eq!(
        fixture.identity(&recreated),
        identities[6],
        "same bytes, somewhere else"
    );
    let new_document = fixture.identity(&added).document;
    assert!(
        identities
            .iter()
            .all(|identity| identity.document != new_document),
        "added"
    );

    assert_eq!(scan.moved, 2);
    assert_eq!(scan.folders_moved, 1);
    assert_eq!(scan.changed_in_place, 1);
    assert_eq!(scan.replaced_at_path, 1);
    assert_eq!(content.reattached, 1);
    assert_eq!(fixture.document_count(), 8, "7 that were there, and 1 new");
    for identity in &identities {
        fixture.assert_still_cared_for(identity);
    }
}

// --- Step 4 ---

#[test]
fn new_bytes_with_the_same_pdf_id_are_a_suggestion_and_not_a_merge() {
    let fixture = Fixture::new();
    let id = "aabbccddeeff00112233445566778899";
    let original = fixture.write("original.pdf", &pdf_with_id(id, 1));
    fixture.settle();
    let identity = fixture.mark_as_cared_for(&original);

    let edition = fixture.write("edition.pdf", &pdf_with_id(id, 2));
    fixture.settle();

    let new = fixture.identity(&edition);
    assert_ne!(
        new.document, identity.document,
        "nothing is merged by itself"
    );
    assert_eq!(fixture.document_count(), 2);
    assert_eq!(fixture.document_at(&original).pdf_id0.as_deref(), Some(id));

    let suggestions = fixture.library().suggestions(true).unwrap();
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].kind, SuggestionKind::SameInternalId);
    assert_eq!(suggestions[0].document_id, new.document);
    assert_eq!(suggestions[0].candidate_document_id, identity.document);
    assert!(suggestions[0].evidence.contains(id));
    fixture.assert_still_cared_for(&identity);
}

#[test]
fn new_bytes_with_the_same_dc_identifier_are_a_suggestion_and_not_a_merge() {
    let fixture = Fixture::new();
    let id = "urn:uuid:12345678-aaaa-bbbb-cccc-1234567890ab";
    let original = fixture.write("original.epub", &epub("Um Livro", &["Autora"], Some(id), 1));
    fixture.settle();
    let identity = fixture.identity(&original);

    let edition = fixture.write("edition.epub", &epub("Um Livro", &["Autora"], Some(id), 2));
    fixture.settle();

    assert_ne!(fixture.identity(&edition).document, identity.document);
    let suggestions = fixture.library().suggestions(true).unwrap();
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].kind, SuggestionKind::SameInternalId);
}

#[test]
fn accepting_a_suggestion_is_the_one_way_to_merge_and_loses_nothing() {
    let fixture = Fixture::new();
    let id = "aabbccddeeff00112233445566778899";
    let original = fixture.write("original.pdf", &pdf_with_id(id, 1));
    fixture.settle();
    let identity = fixture.mark_as_cared_for(&original);
    let edition = fixture.write("edition.pdf", &pdf_with_id(id, 2));
    fixture.settle();
    let edition_identity = fixture.identity(&edition);
    fixture
        .library()
        .add_annotation(edition_identity.document, "second note")
        .unwrap();

    let suggestion = fixture.library().suggestions(true).unwrap()[0].id;
    fixture.library().accept_suggestion(suggestion).unwrap();

    let document = fixture.document_at(&edition);
    assert_eq!(document.id, identity.document);
    assert_eq!(document.revisions.len(), 2);
    assert_eq!(document.annotations.len(), 2);
    assert_eq!(fixture.document_count(), 1);
    assert_eq!(fixture.library().suggestions(true).unwrap().len(), 0);
}

#[test]
fn a_dismissed_suggestion_stays_dismissed() {
    let fixture = Fixture::new();
    let id = "aabbccddeeff00112233445566778899";
    fixture.write("original.pdf", &pdf_with_id(id, 1));
    fixture.settle();
    fixture.write("edition.pdf", &pdf_with_id(id, 2));
    fixture.settle();

    let suggestion = fixture.library().suggestions(true).unwrap()[0].id;
    fixture.library().dismiss_suggestion(suggestion).unwrap();
    fixture.settle();
    assert!(fixture.library().suggestions(true).unwrap().is_empty());
    assert_eq!(fixture.library().suggestions(false).unwrap().len(), 1);
}

// --- Step 5 ---

#[test]
fn a_file_with_nothing_in_common_is_a_new_document() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for_book(&fixture, "book.pdf", 1);
    let other = fixture.write("other.pdf", &pdf(2));
    let (scan, _) = fixture.settle();

    assert_eq!(scan.new_files, 1);
    assert_ne!(fixture.identity(&other).document, identity.document);
    assert_eq!(fixture.identity(&path), identity);
    assert!(fixture.library().suggestions(true).unwrap().is_empty());
}

// --- The File ID is never enough ---

/// SIMULATED: Windows reuses File IDs, but not on demand. The probe makes a new file report the ID
/// of one that went missing, which is what that reuse looks like from here.
#[test]
fn a_reused_file_id_is_not_taken_for_the_file_it_used_to_belong_to() {
    let (fixture, probe) = redirecting_fixture();
    let (path, identity) = cared_for_book(&fixture, "book.pdf", 1);
    let old_id = file_id_of(&path);

    fs::remove_file(&path).unwrap();
    fixture.settle();

    probe
        .file_ids
        .lock()
        .unwrap()
        .push(("unrelated.pdf".to_owned(), old_id));
    let other = fixture.write("unrelated.pdf", &pdf(2));
    fixture.settle();

    let new = fixture.identity(&other);
    assert_ne!(
        new.document, identity.document,
        "same ID, and still another file"
    );
    assert_eq!(fixture.state_of(&path), InstanceState::Absent);
    fixture.assert_still_cared_for(&identity);
    let suggestions = fixture.library().suggestions(true).unwrap();
    assert!(
        suggestions
            .iter()
            .any(|suggestion| suggestion.kind == SuggestionKind::SameFileId)
    );
}

#[test]
fn a_file_moved_and_edited_while_closed_is_left_for_a_person_to_recognize() {
    let mut fixture = Fixture::new();
    let (path, identity) = cared_for_book(&fixture, "book.pdf", 1);

    fixture.close();
    let moved = fixture.path("moved.pdf");
    fs::rename(&path, &moved).unwrap();
    fs::write(&moved, pdf(2)).unwrap();
    fixture.reopen();
    fixture.settle();

    // By the plan's rule, a known File ID with another path and other contents proves nothing.
    assert_ne!(fixture.identity(&moved).document, identity.document);
    assert_eq!(fixture.state_of(&path), InstanceState::Absent);
    let suggestions = fixture.library().suggestions(true).unwrap();
    assert!(
        suggestions.iter().any(|suggestion| {
            suggestion.kind == SuggestionKind::SameFileId
                && suggestion.candidate_document_id == identity.document
        }),
        "{suggestions:?}"
    );
    fixture.assert_still_cared_for(&identity);
}

// --- Duplicates ---

#[test]
fn the_same_bytes_in_two_places_are_one_revision_with_two_instances() {
    let fixture = Fixture::new();
    let contents = pdf(1);
    let first = fixture.write("first.pdf", &contents);
    fixture.settle();
    let identity = fixture.mark_as_cared_for(&first);

    let second = fixture.write("copies/second.pdf", &contents);
    let (_, content) = fixture.settle();

    assert_eq!(content.duplicates, 1);
    assert_eq!(fixture.identity(&second).revision, identity.revision);
    let document = fixture.document_at(&first);
    assert_eq!(document.revisions.len(), 1);
    assert_eq!(document.revisions[0].instances.len(), 2);
    assert_eq!(fixture.document_count(), 1);

    // Deleting one leaves the other, and the note is on both.
    fs::remove_file(&first).unwrap();
    fixture.settle();
    assert_eq!(fixture.state_of(&first), InstanceState::Absent);
    assert_eq!(fixture.state_of(&second), InstanceState::Present);
    fixture.assert_still_cared_for(&identity);
}

#[test]
fn copies_found_in_the_same_scan_become_one_revision() {
    let fixture = Fixture::new();
    let contents = pdf(1);
    let paths: Vec<_> = (0..5)
        .map(|number| fixture.write(&format!("copy{number}.pdf"), &contents))
        .collect();
    fixture.settle();

    let revision = fixture.identity(&paths[0]).revision;
    for path in &paths {
        assert_eq!(fixture.identity(path).revision, revision);
    }
    assert_eq!(fixture.document_count(), 1);
    assert_eq!(fixture.library().status().unwrap().works, 1, "no leftovers");
}

#[test]
fn hard_links_are_two_instances_of_the_same_file() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for_book(&fixture, "book.pdf", 1);

    let link = fixture.path("link.pdf");
    fs::hard_link(&path, &link).unwrap();
    let (first, _) = fixture.settle();
    assert_eq!(first.hard_links, 1);
    assert_eq!(first.bytes_read, 0);

    // A second look finds nothing new, and neither name takes over the other.
    let (second, _) = fixture.settle();
    assert_eq!(second.moved, 0);
    assert_eq!(second.hard_links, 0);
    assert_eq!(second.unchanged, 2);
    assert_eq!(fixture.identity(&path), identity);
    assert_eq!(fixture.identity(&link).revision, identity.revision);
}

// --- Nothing is ever deleted ---

#[test]
fn the_database_itself_refuses_to_delete_what_a_person_cares_about() {
    let mut fixture = Fixture::new();
    let (path, _) = cared_for_book(&fixture, "book.pdf", 1);
    let identity = fixture.identity(&path);
    fixture.close();

    let raw = rusqlite::Connection::open(fixture.paths.library_db()).unwrap();
    for (sql, row) in [
        (
            "DELETE FROM annotation WHERE document_id = ?1",
            identity.document,
        ),
        ("DELETE FROM file_instance WHERE id = ?1", identity.instance),
        ("DELETE FROM revision WHERE id = ?1", identity.revision),
    ] {
        let error = raw.execute(sql, [row]).unwrap_err().to_string();
        assert!(
            error.contains("never") || error.contains("only ever"),
            "{sql}: {error}"
        );
    }
}

#[test]
fn removing_a_root_loses_nothing_and_adding_it_back_recognizes_everything() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for_book(&fixture, "shelf/book.pdf", 1);

    fixture.library().remove_root(fixture.root_id).unwrap();
    assert_eq!(fixture.library().status().unwrap().absent, 1);
    assert!(fixture.library().roots().unwrap().is_empty());
    fixture.assert_still_cared_for(&identity);

    let root = fixture.library().add_root(&fixture.root, &[]).unwrap();
    assert_eq!(root.id, fixture.root_id);
    let scan = fixture
        .library()
        .reconcile(root.id, &ReconcileScope::Everything)
        .unwrap();
    assert_eq!(scan.bytes_read, 0);
    assert_eq!(fixture.identity(&path), identity);
    assert_eq!(fixture.state_of(&path), InstanceState::Present);
}

// --- A root that is not there ---

#[test]
fn a_root_that_cannot_be_read_is_not_a_root_whose_files_are_gone() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for_book(&fixture, "shelf/book.pdf", 1);

    let unplugged = fixture.dir.0.join("unplugged");
    fs::rename(&fixture.root, &unplugged).unwrap();

    let scan = fixture.reconcile();
    assert!(!scan.root_available);
    assert_eq!(
        fixture.library().status().unwrap().roots[0].status,
        "unavailable"
    );
    assert_eq!(
        fixture.state_of(&path),
        InstanceState::Present,
        "unplugging a disk says nothing about the files on it"
    );

    fs::rename(&unplugged, &fixture.root).unwrap();
    let scan = fixture.reconcile();
    assert!(scan.root_available);
    assert_eq!(scan.bytes_read, 0);
    assert_eq!(
        fixture.library().status().unwrap().roots[0].status,
        "available"
    );
    assert_eq!(fixture.identity(&path), identity);
}

#[test]
fn relocating_a_root_recognizes_everything_without_reading() {
    let fixture = Fixture::new();
    let (_, identity) = cared_for_book(&fixture, "shelf/book.pdf", 1);

    let moved = fixture.dir.0.join("books-moved");
    fs::rename(&fixture.root, &moved).unwrap();
    fixture
        .library()
        .relocate_root(fixture.root_id, &moved)
        .unwrap();

    let scan = fixture.reconcile();
    assert_eq!(scan.bytes_read, 0);
    assert_eq!(scan.unchanged, 1);
    assert_eq!(scan.became_absent, 0);
    assert_eq!(
        fixture.identity(&moved.join("shelf").join("book.pdf")),
        identity
    );
}

#[test]
fn overlapping_roots_are_refused() {
    let fixture = Fixture::new();
    fixture.write("inner/book.pdf", &pdf(1));
    let inside = fixture.path("inner");
    assert!(matches!(
        fixture.library().add_root(&inside, &[]),
        Err(Error::Refused(_))
    ));
    assert!(matches!(
        fixture.library().add_root(&fixture.dir.0, &[]),
        Err(Error::Refused(_))
    ));
    assert_eq!(
        fixture.library().add_root(&fixture.root, &[]).unwrap().id,
        fixture.root_id,
        "adding the same one again is not an error"
    );
}

// --- What a scan looks at ---

#[test]
fn what_is_not_a_book_or_was_left_out_is_not_seen() {
    let fixture = Fixture::new();
    fixture.write("book.pdf", &pdf(1));
    fixture.write("notes.docx", b"not a book");
    fixture.write("README.md", b"# not a book");
    fixture.write("LICENSE.txt", b"not a book");
    fixture.write("book.pdf.tmp", b"not a book");
    fixture.write("~$lock.pdf", b"not a book");
    fixture.write("node_modules/dep/manual.pdf", &pdf(2));
    fixture.write(".git/objects/thing.pdf", &pdf(3));
    let (scan, _) = fixture.settle();

    assert_eq!(scan.files_seen, 1);
    assert_eq!(fixture.document_count(), 1);
}

#[test]
fn a_root_can_leave_out_more() {
    let dir = TempDir::new();
    let root = dir.0.join("books");
    fs::create_dir_all(root.join("drafts")).unwrap();
    fs::write(root.join("book.pdf"), pdf(1)).unwrap();
    fs::write(root.join("drafts").join("draft.pdf"), pdf(2)).unwrap();
    fs::write(root.join("old-copy.pdf"), pdf(3)).unwrap();

    let library = Library::open(
        musaeum_library::LibraryPaths::in_directory(&dir.0.join("library")),
        common::options(),
    )
    .unwrap();
    let root_id = library
        .add_root(&root, &["drafts".to_owned(), "old-*".to_owned()])
        .unwrap()
        .id;
    let scan = library
        .reconcile(root_id, &ReconcileScope::Everything)
        .unwrap();
    assert_eq!(scan.files_seen, 1);
}

#[test]
fn what_a_root_leaves_out_can_change_later_and_nothing_is_lost_by_it() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for_book(&fixture, "drafts/book.pdf", 1);

    let root = fixture
        .library()
        .add_root(&fixture.root, &["drafts".to_owned()])
        .unwrap();
    assert_eq!(root.id, fixture.root_id);
    assert_eq!(root.exclusions, ["drafts"]);
    let (scan, _) = fixture.settle();
    assert_eq!(scan.folders_absent, 1);
    assert_eq!(fixture.state_of(&path), InstanceState::Absent);
    fixture.assert_still_cared_for(&identity);

    // Adding it with nothing to say about that leaves it as it is.
    let root = fixture.library().add_root(&fixture.root, &[]).unwrap();
    assert_eq!(root.exclusions, ["drafts"]);

    let root = fixture
        .library()
        .add_root(&fixture.root, &["other".to_owned()])
        .unwrap();
    assert_eq!(root.exclusions, ["other"]);
    let (scan, _) = fixture.settle();
    assert_eq!(scan.bytes_read, 0);
    assert_eq!(fixture.identity(&path), identity);
    assert_eq!(fixture.state_of(&path), InstanceState::Present);
}

#[test]
fn a_junction_is_not_followed() {
    let fixture = Fixture::new();
    fixture.write("book.pdf", &pdf(1));
    let outside = fixture.dir.0.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("other.pdf"), pdf(2)).unwrap();
    // A cycle too, which is what following links would get stuck on.
    make_junction(&fixture.path("to-outside"), &outside);
    make_junction(&fixture.path("to-itself"), &fixture.root);

    let (scan, _) = fixture.settle();
    assert_eq!(scan.files_seen, 1);
    assert_eq!(scan.directories_listed, 1);
}

fn make_junction(link: &Path, target: &Path) {
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
}

// --- Unusual files ---

#[test]
fn a_path_too_long_for_the_old_limit_works_throughout() {
    let fixture = Fixture::new();
    let deep = format!("{0}/{0}/{0}/{0}/{0}", "d".repeat(60));
    let (path, identity) = cared_for_book(&fixture, &format!("{deep}/book.pdf"), 1);
    assert!(path.as_os_str().len() > 260);

    let renamed = fixture.path(&format!("{deep}/renamed.pdf"));
    fs::rename(&path, &renamed).unwrap();
    let (scan, _) = fixture.settle();
    assert_eq!(scan.moved, 1);
    assert_eq!(fixture.identity(&renamed), identity);
}

#[test]
fn a_name_that_is_not_unicode_works_throughout() {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    let fixture = Fixture::new();
    let name: Vec<u16> = [0xD800]
        .into_iter()
        .chain("book.pdf".encode_utf16())
        .collect();
    let path = fixture.root.join(OsString::from_wide(&name));
    fs::write(&path, pdf(1)).unwrap();
    let (scan, content) = fixture.settle();
    assert_eq!(scan.new_files, 1);
    assert_eq!(content.hashed, 1);
    let identity = fixture.mark_as_cared_for(&path);

    let name: Vec<u16> = [0xD801]
        .into_iter()
        .chain("renamed.pdf".encode_utf16())
        .collect();
    let renamed = fixture.root.join(OsString::from_wide(&name));
    fs::rename(&path, &renamed).unwrap();
    let (scan, _) = fixture.settle();
    assert_eq!(scan.moved, 1);
    assert_eq!(fixture.identity(&renamed), identity);
}

#[test]
fn accents_and_other_scripts_in_names_work_throughout() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for_book(&fixture, "Crítica da Razão/Введение 日本語 🧪.pdf", 1);
    assert_eq!(fixture.identity(&path), identity);
    let (scan, _) = fixture.settle();
    assert_eq!(scan.unchanged, 1);
    assert_eq!(
        fixture.library().instance(identity.instance).unwrap().path,
        path
    );
}

#[test]
fn a_file_someone_is_holding_exclusively_waits_and_is_not_lost() {
    let fixture = Fixture::new();
    let path = fixture.write("book.pdf", &pdf(1));
    let held = hold_exclusively(&path);

    let (scan, content) = fixture.settle();
    assert_eq!(scan.new_files, 1);
    assert_eq!(scan.unreadable, 1);
    assert_eq!(content.unreadable, 1);
    assert_eq!(fixture.state_of(&path), InstanceState::Unreadable);

    drop(held);
    let (_, content) = fixture.settle();
    assert_eq!(content.hashed, 1);
    assert_eq!(fixture.state_of(&path), InstanceState::Present);
}

#[test]
fn an_empty_file_is_a_file() {
    let fixture = Fixture::new();
    let path = fixture.write("empty.pdf", b"");
    let (_, content) = fixture.settle();
    assert_eq!(content.hashed, 1);
    assert_eq!(fixture.state_of(&path), InstanceState::Present);
}

#[test]
fn nothing_changed_writes_nothing() {
    let fixture = Fixture::new();
    for number in 0..50 {
        fixture.write(
            &format!("shelf{}/book{number}.pdf", number % 3),
            &pdf(number),
        );
    }
    fixture.settle();
    let raw = fixture.raw();
    let before = dump(&raw, "file_instance");
    let revisions_before = dump(&raw, "revision");

    let (scan, content) = fixture.settle();

    assert_eq!(scan.unchanged, 50);
    assert_eq!(scan.bytes_read, 0);
    assert_eq!(scan.files_opened, 0);
    assert_eq!(content.hashed, 0);
    assert_eq!(dump(&raw, "file_instance"), before);
    assert_eq!(dump(&raw, "revision"), revisions_before);
}

#[test]
fn a_scan_of_a_few_directories_settles_them_and_leaves_the_rest_alone() {
    let fixture = Fixture::new();
    let (kept, kept_identity) = cared_for_book(&fixture, "kept/book.pdf", 1);
    let moved_from = fixture.write("from/book.pdf", &pdf(2));
    fs::create_dir(fixture.path("to")).unwrap();
    fixture.settle();
    let moved_identity = fixture.identity(&moved_from);

    let moved_to = fixture.path("to/book.pdf");
    fs::rename(&moved_from, &moved_to).unwrap();

    let scope = ReconcileScope::Directories(
        [fixture.path("from"), fixture.path("to")]
            .into_iter()
            .collect(),
    );
    let scan = fixture
        .library()
        .reconcile(fixture.root_id, &scope)
        .unwrap();

    assert_eq!(scan.directories_listed, 2);
    assert_eq!(scan.moved, 1);
    assert_eq!(fixture.identity(&moved_to), moved_identity);
    assert_eq!(fixture.identity(&kept), kept_identity);
}

#[test]
fn a_scan_of_one_directory_can_tell_a_move_from_a_hard_link() {
    let fixture = Fixture::new();
    let (path, identity) = cared_for_book(&fixture, "here/book.pdf", 1);
    fs::create_dir(fixture.path("there")).unwrap();
    fixture.settle();

    let link = fixture.path("there/link.pdf");
    fs::hard_link(&path, &link).unwrap();
    let scope = ReconcileScope::Directories([fixture.path("there")].into_iter().collect());
    let scan = fixture
        .library()
        .reconcile(fixture.root_id, &scope)
        .unwrap();

    assert_eq!(scan.hard_links, 1);
    assert_eq!(
        fixture.identity(&path),
        identity,
        "the original did not go anywhere"
    );
    assert_eq!(fixture.identity(&link).revision, identity.revision);
}

#[test]
fn a_new_folder_found_by_looking_at_its_parent_is_walked() {
    let fixture = Fixture::new();
    fixture.settle();
    let path = fixture.write("new/deep/er/book.pdf", &pdf(1));

    let scope = ReconcileScope::Directories([fixture.root.clone()].into_iter().collect());
    let scan = fixture
        .library()
        .reconcile(fixture.root_id, &scope)
        .unwrap();
    assert_eq!(scan.new_files, 1);
    assert_eq!(scan.folders_new, 3);
    assert_eq!(fixture.state_of(&path), InstanceState::Present);
}

#[test]
fn a_deleted_folder_takes_what_is_in_it_to_absent_and_recreating_it_brings_it_back() {
    let fixture = Fixture::new();
    let contents = pdf(1);
    let path = fixture.write("shelf/deep/book.pdf", &contents);
    fixture.settle();
    let identity = fixture.mark_as_cared_for(&path);

    fs::remove_dir_all(fixture.path("shelf")).unwrap();
    let (scan, _) = fixture.settle();
    assert_eq!(scan.folders_absent, 2);
    assert_eq!(scan.became_absent, 1);
    assert_eq!(fixture.state_of(&path), InstanceState::Absent);
    fixture.assert_still_cared_for(&identity);

    fixture.write("shelf/deep/book.pdf", &contents);
    let (_, content) = fixture.settle();
    assert_eq!(content.reattached, 1);
    assert_eq!(fixture.identity(&path), identity);
    assert_eq!(fixture.state_of(&path), InstanceState::Present);
    fixture.assert_still_cared_for(&identity);
    assert_eq!(
        fixture.library().status().unwrap().folders,
        3,
        "root, shelf and deep"
    );
}

#[test]
fn a_file_changing_while_it_is_hashed_does_not_get_that_hash() {
    let fixture = Fixture::new();
    let path = fixture.write("book.pdf", &pdf(1));
    fixture.reconcile();
    let before = fixture.identity(&path);

    // Changed after the scan and before the hash, which is what the check after hashing is for.
    fs::write(&path, pdf(2)).unwrap();
    let content = fixture.process();
    assert_eq!(content.hashed, 0);
    assert_eq!(content.changed_meanwhile, 1);

    let (_, content) = fixture.settle();
    assert_eq!(content.hashed, 1);
    let document = fixture.document_at(&path);
    assert_eq!(document.id, before.document);
    let expected = blake3::hash(&pdf(2)).to_hex().to_string();
    assert_eq!(
        document.revisions[0].blake3.as_deref(),
        Some(expected.as_str())
    );
}

#[test]
fn metadata_a_person_edited_survives_reading_the_file_again() {
    let fixture = Fixture::new();
    let path = fixture.write(
        "file-name.epub",
        &epub(
            "Título do Livro",
            &["Autora Um", "Autor Dois"],
            Some("urn:x:1"),
            1,
        ),
    );
    fixture.settle();

    let document = fixture.document_at(&path);
    let work = fixture.library().work(document.work_id).unwrap();
    assert_eq!(
        work.title, "Título do Livro",
        "read from the book, not the file name"
    );
    assert_eq!(work.creators, ["Autora Um", "Autor Dois"]);
    assert_eq!(work.language.as_deref(), Some("pt-BR"));
    assert_eq!(work.year, Some(1899));
    assert_eq!(document.dc_identifier.as_deref(), Some("urn:x:1"));

    fixture
        .library()
        .edit_work(
            work.id,
            musaeum_library::WorkEdit {
                title: Some("Meu Título".to_owned()),
                ..musaeum_library::WorkEdit::default()
            },
        )
        .unwrap();

    fixture.library().reimport(document.id).unwrap();

    let work = fixture.library().work(document.work_id).unwrap();
    assert_eq!(work.title, "Meu Título", "a person's edit stays");
    assert_eq!(
        work.creators,
        ["Autora Um", "Autor Dois"],
        "the rest is read again"
    );

    // A new revision with a new title in it does not change that either.
    fs::write(
        &path,
        epub("Outro Título", &["Autora Um"], Some("urn:x:1"), 2),
    )
    .unwrap();
    fixture.settle();
    let work = fixture.library().work(document.work_id).unwrap();
    assert_eq!(work.title, "Meu Título");
    assert_eq!(
        work.creators,
        ["Autora Um"],
        "what nobody edited follows the file"
    );
}

#[test]
fn notes_made_before_the_hash_arrives_follow_the_file() {
    let fixture = Fixture::new();
    let contents = pdf(1);
    let path = fixture.write("book.pdf", &contents);
    fixture.settle();
    let identity = fixture.mark_as_cared_for(&path);

    // The same book reappears as another file, and someone rates it and writes on it right away.
    fs::remove_file(&path).unwrap();
    let again = fixture.write("again.pdf", &contents);
    fixture.reconcile();
    let provisional = fixture.identity(&again);
    assert_ne!(provisional.document, identity.document);
    fixture
        .library()
        .add_annotation(provisional.document, "written before the hash")
        .unwrap();
    fixture
        .library()
        .edit_work(
            provisional.work,
            musaeum_library::WorkEdit {
                series: Some("A Series".to_owned()),
                rating: Some(1),
                ..musaeum_library::WorkEdit::default()
            },
        )
        .unwrap();

    let content = fixture.process();
    assert_eq!(content.reattached, 1);

    let document = fixture.document_at(&again);
    assert_eq!(document.id, identity.document);
    assert_eq!(document.annotations.len(), 2, "both notes");
    let work = fixture.library().work(document.work_id).unwrap();
    assert_eq!(
        work.rating,
        Some(5),
        "where both set something, what was there stays"
    );
    assert_eq!(
        work.series.as_deref(),
        Some("A Series"),
        "and the rest moves over"
    );
    assert_eq!(fixture.document_count(), 1);
    assert_eq!(fixture.library().status().unwrap().works, 1, "no leftovers");
}

#[test]
fn a_book_the_worker_cannot_read_is_still_a_book() {
    let fixture = Fixture::new();
    let path = fixture.write("damaged.epub", b"this is not a zip file at all");
    let (_, content) = fixture.settle();
    assert_eq!(content.hashed, 1);
    assert_eq!(content.extraction_failed, 1);
    assert_eq!(fixture.state_of(&path), InstanceState::Present);
    assert_eq!(fixture.document_at(&path).title, "damaged");

    let (_, content) = fixture.settle();
    assert_eq!(content.extraction_failed, 0, "it is not asked about again");
}
