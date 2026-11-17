//! The two databases: what cannot be replaced, what can, and who gets to write.

mod common;

use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use common::{Fixture, TempDir, options, pdf};
use musaeum_library::{BackupPolicy, Error, Library, LibraryOptions, LibraryPaths};

fn journal_mode(path: &std::path::Path) -> String {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn both_databases_are_in_wal_mode_and_in_separate_places() {
    let dir = TempDir::new();
    let paths = LibraryPaths {
        data_dir: dir.0.join("data"),
        cache_dir: dir.0.join("cache"),
    };
    let library = Library::open(paths.clone(), options()).unwrap();
    assert!(paths.library_db().exists());
    assert!(paths.cache_db().exists());
    assert_ne!(paths.library_db().parent(), paths.cache_db().parent());
    drop(library);
    assert_eq!(journal_mode(&paths.library_db()), "wal");
    assert_eq!(journal_mode(&paths.cache_db()), "wal");
}

#[test]
fn a_second_writer_is_refused_and_readers_are_not() {
    let fixture = Fixture::new();
    assert!(matches!(
        Library::open(fixture.paths.clone(), options()),
        Err(Error::LockedByAnotherProcess)
    ));
    let reader = Library::open_read_only(fixture.paths.clone()).unwrap();
    assert_eq!(reader.status().unwrap().roots.len(), 1);
}

#[test]
fn a_library_opened_for_reading_cannot_write() {
    let mut fixture = Fixture::new();
    fixture.write("book.pdf", &pdf(1));
    fixture.settle();
    fixture.close();

    let reader = Library::open_read_only(fixture.paths.clone()).unwrap();
    assert_eq!(reader.status().unwrap().documents, 1);
    let elsewhere = fixture.dir.0.join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    assert!(matches!(
        reader.add_root(&elsewhere, &[]),
        Err(Error::ReadOnly(_))
    ));
}

#[test]
fn opening_for_reading_what_does_not_exist_creates_nothing() {
    let dir = TempDir::new();
    let paths = LibraryPaths::in_directory(&dir.0.join("none"));
    assert!(matches!(
        Library::open_read_only(paths.clone()),
        Err(Error::NotFound(_))
    ));
    assert!(!paths.data_dir.exists());
}

#[test]
fn readers_do_not_wait_for_a_long_write() {
    let mut fixture = Fixture::new();
    fixture.write("book.pdf", &pdf(1));
    fixture.settle();
    fixture.close();

    // A write transaction left open, as if the writer were in the middle of something long.
    let writer = rusqlite::Connection::open(fixture.paths.library_db()).unwrap();
    writer
        .execute_batch("BEGIN IMMEDIATE; UPDATE work SET title = 'changing';")
        .unwrap();

    let reader = Library::open_read_only(fixture.paths.clone()).unwrap();
    let started = Instant::now();
    let status = reader.status().unwrap();
    let hits = reader.search("book", 10).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the read waited"
    );
    assert_eq!(status.works, 1);
    assert_eq!(hits[0].title, "book", "the read sees what was committed");

    writer.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn many_threads_read_while_the_only_writer_writes() {
    let fixture = Arc::new(Fixture::new());
    for number in 0..40 {
        fixture.write(&format!("book{number}.pdf"), &pdf(number));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let start = Arc::new(Barrier::new(9));

    let readers: Vec<_> = (0..8)
        .map(|_| {
            let fixture = Arc::clone(&fixture);
            let stop = Arc::clone(&stop);
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                let mut reads = 0;
                while !stop.load(Ordering::Relaxed) {
                    fixture.library().status().unwrap();
                    fixture.library().search("book", 10).unwrap();
                    reads += 1;
                }
                reads
            })
        })
        .collect();
    start.wait();
    fixture.settle();
    stop.store(true, Ordering::Relaxed);
    for reader in readers {
        assert!(reader.join().unwrap() > 0);
    }
    assert_eq!(fixture.library().status().unwrap().documents, 40);
}

#[test]
fn a_failed_write_leaves_no_trace() {
    let fixture = Fixture::new();
    let path = fixture.write("book.pdf", &pdf(1));
    fixture.settle();
    let identity = fixture.identity(&path);

    let result = fixture.library().edit_work(
        identity.work,
        musaeum_library::WorkEdit {
            rating: Some(9),
            title: Some("should not stick".to_owned()),
            ..musaeum_library::WorkEdit::default()
        },
    );
    assert!(result.is_err());
    assert_eq!(fixture.library().work(identity.work).unwrap().title, "book");
}

// --- Cache ---

#[test]
fn losing_the_cache_loses_nothing() {
    let mut fixture = Fixture::new();
    let path = fixture.write(
        "book.epub",
        &common::epub("O Título", &["A Autora"], Some("urn:x:1"), 1),
    );
    fixture.settle();
    let identity = fixture.mark_as_cared_for(&path);
    fixture.close();

    for suffix in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", fixture.paths.cache_db().display()));
    }

    fixture.reopen();
    assert!(fixture.paths.cache_db().exists(), "it comes back empty");
    assert_eq!(fixture.identity(&path), identity);
    fixture.assert_still_cared_for(&identity);

    // Reading the metadata again, which the cache would have saved a trip to the worker for.
    let report = fixture.library().reimport(identity.document).unwrap();
    assert_eq!(report.extracted, 1);
    assert_eq!(
        fixture.library().work(identity.work).unwrap().title,
        "O Título"
    );
}

#[test]
fn a_corrupt_cache_is_replaced_without_a_word() {
    let mut fixture = Fixture::new();
    fixture.settle();
    fixture.close();
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", fixture.paths.cache_db().display()));
    }
    fs::write(
        fixture.paths.cache_db(),
        b"this was not a database for very long",
    )
    .unwrap();
    fixture.reopen();
    let connection = rusqlite::Connection::open(fixture.paths.cache_db()).unwrap();
    let tables: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name = 'extraction'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 1);
}

#[test]
fn a_cache_from_a_newer_version_is_replaced_too() {
    let mut fixture = Fixture::new();
    fixture.settle();
    fixture.close();
    {
        let connection = rusqlite::Connection::open(fixture.paths.cache_db()).unwrap();
        connection.pragma_update(None, "user_version", 999).unwrap();
    }
    fixture.reopen();
    let connection = rusqlite::Connection::open(fixture.paths.cache_db()).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 5, "the version this program writes");
}

// --- What cannot be replaced ---

#[test]
fn a_corrupt_library_is_an_error_and_is_left_alone() {
    let mut fixture = Fixture::new();
    fixture.settle();
    fixture.close();
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", fixture.paths.library_db().display()));
    }
    let garbage = b"this file used to matter to someone".repeat(1000);
    fs::write(fixture.paths.library_db(), &garbage).unwrap();

    assert!(Library::open(fixture.paths.clone(), options()).is_err());
    assert_eq!(
        fs::read(fixture.paths.library_db()).unwrap(),
        garbage,
        "unlike the cache, this is not this program's to replace"
    );
}

#[test]
fn a_library_from_a_newer_version_is_refused_untouched() {
    let mut fixture = Fixture::new();
    fixture.settle();
    fixture.close();
    {
        let connection = rusqlite::Connection::open(fixture.paths.library_db()).unwrap();
        connection.pragma_update(None, "user_version", 999).unwrap();
    }
    assert!(matches!(
        Library::open(fixture.paths.clone(), options()),
        Err(Error::NewerSchema { found: 999, .. })
    ));
    assert!(matches!(
        Library::open_read_only(fixture.paths.clone()),
        Err(Error::NewerSchema { found: 999, .. })
    ));
    let connection = rusqlite::Connection::open(fixture.paths.library_db()).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 999);
}

#[test]
fn someone_elses_database_is_refused_untouched() {
    let dir = TempDir::new();
    let paths = LibraryPaths::in_directory(&dir.0.join("library"));
    fs::create_dir_all(&paths.data_dir).unwrap();
    {
        let connection = rusqlite::Connection::open(paths.library_db()).unwrap();
        connection
            .execute_batch("CREATE TABLE theirs (x); INSERT INTO theirs VALUES (1);")
            .unwrap();
    }
    assert!(matches!(
        Library::open(paths.clone(), options()),
        Err(Error::NotOurDatabase(_))
    ));
    let connection = rusqlite::Connection::open(paths.library_db()).unwrap();
    let count: i64 = connection
        .query_row("SELECT count(*) FROM theirs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn every_declared_constraint_of_the_schema_holds() {
    let mut fixture = Fixture::new();
    let path = fixture.write("book.pdf", &pdf(1));
    fixture.settle();
    let identity = fixture.identity(&path);
    fixture.close();

    let connection = rusqlite::Connection::open(fixture.paths.library_db()).unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    let violations: i64 = connection
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(violations, 0);
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");

    for (sql, why) in [
        (
            "UPDATE file_instance SET state = 'gone'",
            "a state that does not exist",
        ),
        (
            "UPDATE revision SET blake3 = x'00'",
            "a hash that is not 32 bytes",
        ),
        (
            "UPDATE file_instance SET file_id = x'00'",
            "a File ID that is not 128 bits",
        ),
        ("UPDATE work SET rating = 6", "a rating out of range"),
        (
            "UPDATE work SET status = 'maybe'",
            "a status that does not exist",
        ),
    ] {
        assert!(connection.execute(sql, []).is_err(), "{why}");
    }
    assert!(
        connection
            .execute(
                "INSERT INTO file_instance (revision_id, folder_id, name, mtime_ns, state, state_since)
                 SELECT revision_id, folder_id, name, 0, 'present', 0 FROM file_instance",
                [],
            )
            .is_err(),
        "two files at one path"
    );
    assert!(
        connection
            .execute("DELETE FROM document WHERE id = ?1", [identity.document])
            .is_err(),
        "a document with revisions"
    );
}

// --- Backups ---

fn backup_options() -> LibraryOptions {
    LibraryOptions {
        backup_policy: BackupPolicy {
            interval: Duration::from_secs(3600),
            keep_daily: 3,
            keep_weekly: 1,
            keep_before_upgrade: 1,
        },
        ..options()
    }
}

#[test]
fn a_backup_is_a_complete_and_valid_library() {
    let fixture = Fixture::new();
    let path = fixture.write("book.pdf", &pdf(1));
    fixture.settle();
    let identity = fixture.mark_as_cared_for(&path);

    let backup = fixture.library().backup_now().unwrap();
    assert!(backup.size > 0);

    let connection = rusqlite::Connection::open(&backup.path).unwrap();
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");
    let note: String = connection
        .query_row(
            "SELECT note FROM annotation WHERE document_id = ?1",
            [identity.document],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(note, "my note");
    // It needs nothing next to it.
    assert!(!backup.path.with_extension("db-wal").exists());
}

#[test]
fn a_backup_taken_during_writes_is_consistent_and_does_not_hold_them_up() {
    let fixture = Arc::new(Fixture::new());
    for number in 0..200 {
        fixture.write(&format!("book{number}.pdf"), &pdf(number));
    }
    let writer = {
        let fixture = Arc::clone(&fixture);
        thread::spawn(move || fixture.settle())
    };
    let mut backups = Vec::new();
    while !writer.is_finished() {
        backups.push(fixture.library().backup_now().unwrap());
        thread::sleep(Duration::from_millis(5));
    }
    writer.join().unwrap();
    backups.push(fixture.library().backup_now().unwrap());

    for backup in fixture.library().backups().unwrap() {
        let connection = rusqlite::Connection::open(&backup.path).unwrap();
        let integrity: String = connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
        // Every work has its document, its revision and its file: no half-written transaction.
        let orphans: i64 = connection
            .query_row(
                "SELECT count(*) FROM work w WHERE NOT EXISTS (
                     SELECT 1 FROM document d JOIN revision r ON r.document_id = d.id
                     JOIN file_instance i ON i.revision_id = r.id WHERE d.work_id = w.id)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0);
    }
}

#[test]
fn backups_rotate_and_touch_nothing_else_in_the_folder() {
    let fixture = Fixture::with_options(backup_options());
    fixture.settle();
    let folder = fixture.paths.backups_dir();
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("notes.txt"), b"mine").unwrap();

    // Dated as if they had been made one a day for two months.
    for day in 1..=28 {
        for month in [8, 9] {
            let name = format!("library-2026{month:02}{day:02}T120000000Z.db");
            fs::write(folder.join(name), b"old").unwrap();
        }
    }
    fixture.library().backup_now().unwrap();

    let backups = fixture.library().backups().unwrap();
    assert_eq!(backups.len(), 3 + 1, "three days and one week");
    assert!(backups[0].size > 3, "the new one, first");
    assert_eq!(fs::read(folder.join("notes.txt")).unwrap(), b"mine");
}

#[test]
fn a_backup_is_only_made_when_it_is_due() {
    let fixture = Fixture::with_options(backup_options());
    assert!(
        fixture.library().backup_if_due().unwrap().is_some(),
        "the first one is always due"
    );
    assert!(
        fixture.library().backup_if_due().unwrap().is_none(),
        "an hour has not gone by"
    );
    assert_eq!(fixture.library().backups().unwrap().len(), 1);
}

#[test]
fn a_backup_interrupted_by_a_crash_is_not_taken_for_a_backup() {
    let fixture = Fixture::new();
    fixture.settle();
    let folder = fixture.paths.backups_dir();
    fs::create_dir_all(&folder).unwrap();
    let partial = folder.join("library-20261101T120000000Z.db.partial");
    fs::write(&partial, b"half a copy").unwrap();

    assert!(fixture.library().backups().unwrap().is_empty());
    fixture.library().backup_now().unwrap();
    assert!(!partial.exists(), "the next backup cleans it up");
    assert_eq!(fixture.library().backups().unwrap().len(), 1);
}

#[test]
fn restoring_a_backup_brings_it_back_and_keeps_what_it_replaced() {
    let mut fixture = Fixture::new();
    let path = fixture.write("book.pdf", &pdf(1));
    fixture.settle();
    let identity = fixture.mark_as_cared_for(&path);
    let backup = fixture.library().backup_now().unwrap();

    // Something goes wrong afterwards.
    fixture
        .library()
        .add_annotation(identity.document, "after the backup")
        .unwrap();
    fixture.close();

    let set_aside = Library::restore_backup(&fixture.paths, &backup.path).unwrap();
    assert!(
        set_aside.join("library.db").exists(),
        "nothing was thrown away"
    );

    fixture.reopen();
    fixture.assert_still_cared_for(&identity);
    assert_eq!(
        fixture
            .library()
            .document(identity.document)
            .unwrap()
            .annotations
            .len(),
        2,
        "the note and the highlight of the backup, not the note made after it"
    );

    let replaced = rusqlite::Connection::open(set_aside.join("library.db")).unwrap();
    let notes: i64 = replaced
        .query_row("SELECT count(*) FROM annotation", [], |row| row.get(0))
        .unwrap();
    assert_eq!(notes, 3, "what was replaced is still there to go back to");
}

#[test]
fn restoring_is_refused_while_the_library_is_open() {
    let fixture = Fixture::new();
    let backup = fixture.library().backup_now().unwrap();
    assert!(matches!(
        Library::restore_backup(&fixture.paths, &backup.path),
        Err(Error::LockedByAnotherProcess)
    ));
}

#[test]
fn restoring_something_broken_is_refused_and_changes_nothing() {
    let mut fixture = Fixture::new();
    fixture.settle();
    fixture.close();
    let broken = fixture.dir.0.join("broken.db");
    fs::write(&broken, b"not a backup at all").unwrap();
    assert!(Library::restore_backup(&fixture.paths, &broken).is_err());
    fixture.reopen();
    assert_eq!(fixture.library().roots().unwrap().len(), 1);
}
