//! The library kept up to date in real time, by real events from the real system.

mod common;

use std::fs;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use common::{TempDir, eventually, options, pdf, safe_save};
use musaeum_library::{
    Indexer, IndexerConfig, IndexerEvent, InstanceState, Library, LibraryPaths, RootId,
};

const PATIENCE: Duration = Duration::from_secs(30);

/// A library being indexed, as the program does it. The clock is out of the way unless a test wants
/// it, so that what these tests see comes from events.
struct Running {
    // In the order they have to end: an open library would keep its folder from going.
    indexer: Option<Indexer>,
    library: Arc<Library>,
    events: Receiver<IndexerEvent>,
    root: std::path::PathBuf,
    root_id: RootId,
    _dir: TempDir,
}

fn config() -> IndexerConfig {
    IndexerConfig {
        settle: Duration::from_millis(100),
        max_delay: Duration::from_secs(2),
        reconcile_interval: Duration::from_secs(3600),
        backup_check_interval: Duration::from_secs(3600),
        ..IndexerConfig::default()
    }
}

fn start(config: IndexerConfig) -> Running {
    let dir = TempDir::new();
    let root = dir.0.join("books");
    fs::create_dir(&root).unwrap();
    start_in(dir, root, config)
}

fn start_in(dir: TempDir, root: std::path::PathBuf, config: IndexerConfig) -> Running {
    let library = Arc::new(
        Library::open(
            LibraryPaths::in_directory(&dir.0.join("library")),
            options(),
        )
        .unwrap(),
    );
    let root_id = library.add_root(&root, &[]).unwrap().id;
    let (sender, events) = mpsc::channel();
    let indexer = Indexer::start(Arc::clone(&library), config, Some(sender)).unwrap();
    assert!(
        indexer.wait_until_idle(PATIENCE),
        "the first scan did not end"
    );
    Running {
        _dir: dir,
        root,
        library,
        root_id,
        indexer: Some(indexer),
        events,
    }
}

impl Running {
    fn path(&self, relative: &str) -> std::path::PathBuf {
        self.root.join(relative)
    }

    fn state_of(&self, relative: &str) -> Option<InstanceState> {
        self.library
            .instance_at(&self.path(relative))
            .unwrap()
            .map(|instance| instance.state)
    }

    fn is_present(&self, relative: &str) -> bool {
        self.state_of(relative) == Some(InstanceState::Present)
    }

    fn is_confirmed(&self, relative: &str) -> bool {
        self.library
            .instance_at(&self.path(relative))
            .unwrap()
            .and_then(|instance| self.library.document(instance.document_id).ok())
            .is_some_and(|document| {
                document
                    .revisions
                    .iter()
                    .all(|revision| revision.blake3.is_some())
            })
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        drop(self.indexer.take());
    }
}

#[test]
fn a_new_file_is_found_without_being_asked_and_gets_its_hash() {
    let running = start(config());

    fs::write(running.path("book.pdf"), pdf(1)).unwrap();

    assert!(eventually(PATIENCE, || running.is_present("book.pdf")));
    assert!(eventually(PATIENCE, || running.is_confirmed("book.pdf")));
}

#[test]
fn a_rename_is_followed_in_real_time() {
    let running = start(config());
    fs::write(running.path("before.pdf"), pdf(1)).unwrap();
    assert!(eventually(PATIENCE, || running.is_confirmed("before.pdf")));
    let before = running
        .library
        .instance_at(&running.path("before.pdf"))
        .unwrap()
        .unwrap();

    fs::rename(running.path("before.pdf"), running.path("after.pdf")).unwrap();

    assert!(eventually(PATIENCE, || running.is_present("after.pdf")));
    let after = running
        .library
        .instance_at(&running.path("after.pdf"))
        .unwrap()
        .unwrap();
    assert_eq!(after.id, before.id);
    assert_eq!(after.document_id, before.document_id);
}

#[test]
fn a_safe_save_is_followed_in_real_time() {
    let running = start(config());
    fs::write(running.path("book.pdf"), pdf(1)).unwrap();
    assert!(eventually(PATIENCE, || running.is_confirmed("book.pdf")));
    let document = running
        .library
        .instance_at(&running.path("book.pdf"))
        .unwrap()
        .unwrap()
        .document_id;

    safe_save(&running.path("book.pdf"), &pdf(2));

    assert!(eventually(PATIENCE, || {
        running.library.document(document).is_ok_and(|info| {
            info.revisions.len() == 2 && info.revisions.iter().all(|r| r.blake3.is_some())
        })
    }));
    assert_eq!(running.library.status().unwrap().documents, 1);
}

#[test]
fn a_folder_renamed_is_followed_in_real_time() {
    let running = start(config());
    fs::create_dir(running.path("shelf")).unwrap();
    fs::write(running.path("shelf/book.pdf"), pdf(1)).unwrap();
    assert!(eventually(PATIENCE, || running.is_confirmed("shelf/book.pdf")));
    let before = running
        .library
        .instance_at(&running.path("shelf/book.pdf"))
        .unwrap()
        .unwrap();

    fs::rename(running.path("shelf"), running.path("renamed")).unwrap();

    assert!(eventually(PATIENCE, || running.is_present("renamed/book.pdf")));
    let after = running
        .library
        .instance_at(&running.path("renamed/book.pdf"))
        .unwrap()
        .unwrap();
    assert_eq!(after.id, before.id);
    assert_eq!(running.library.status().unwrap().absent, 0);
}

#[test]
fn a_folder_coming_in_from_outside_is_walked() {
    let running = start(config());
    let outside = running._dir.0.join("outside");
    fs::create_dir_all(outside.join("deep")).unwrap();
    fs::write(outside.join("deep").join("book.pdf"), pdf(1)).unwrap();

    fs::rename(&outside, running.path("arrived")).unwrap();

    assert!(eventually(PATIENCE, || running.is_present("arrived/deep/book.pdf")));
}

#[test]
fn a_deleted_file_and_a_deleted_folder_go_absent_in_real_time() {
    let running = start(config());
    fs::create_dir(running.path("shelf")).unwrap();
    fs::write(running.path("shelf/one.pdf"), pdf(1)).unwrap();
    fs::write(running.path("two.pdf"), pdf(2)).unwrap();
    assert!(eventually(PATIENCE, || running
        .is_confirmed("shelf/one.pdf")
        && running.is_confirmed("two.pdf")));

    fs::remove_file(running.path("two.pdf")).unwrap();
    assert!(eventually(PATIENCE, || running.state_of("two.pdf")
        == Some(InstanceState::Absent)));

    fs::remove_dir_all(running.path("shelf")).unwrap();
    assert!(eventually(PATIENCE, || {
        running.state_of("shelf/one.pdf") == Some(InstanceState::Absent)
    }));
    assert_eq!(
        running.library.status().unwrap().documents,
        2,
        "nothing is deleted"
    );
}

#[test]
fn a_flood_of_files_is_all_found() {
    // Smaller than any record, so the system loses track of every single change. The promise is that
    // nothing depends on it not doing that.
    let running = start(IndexerConfig {
        watch_buffer: 4,
        ..config()
    });
    for number in 0..3000 {
        fs::write(running.path(&format!("book{number}.pdf")), pdf(number)).unwrap();
    }

    assert!(eventually(Duration::from_secs(120), || {
        let status = running.library.status().unwrap();
        status.present == 3000 && status.unconfirmed_revisions == 0
    }));

    let overflowed = running.events.try_iter().any(
        |event| matches!(event, IndexerEvent::WatchOverflowed(root) if root == running.root_id),
    );
    assert!(
        overflowed,
        "this test only means something if the buffer did overflow"
    );
    assert_eq!(running.library.status().unwrap().absent, 0);
}

#[test]
fn what_no_event_reported_is_found_by_the_clock() {
    // Changes in a folder the watch cannot see, because it was started on something else: the root
    // is swapped under a live watch, so the only way to find this is to look.
    let running = start(IndexerConfig {
        reconcile_interval: Duration::from_millis(300),
        ..config()
    });
    fs::write(running.path("first.pdf"), pdf(1)).unwrap();
    assert!(eventually(PATIENCE, || running.is_confirmed("first.pdf")));

    let aside = running._dir.0.join("aside");
    // Windows refuses this for the moment a scan has something in the folder open, and the clock is
    // set to scan all the time. That is what a person trying again looks like.
    assert!(eventually(PATIENCE, || fs::rename(&running.root, &aside).is_ok()));
    fs::create_dir(&running.root).unwrap();
    fs::write(running.path("second.pdf"), pdf(2)).unwrap();

    assert!(eventually(PATIENCE, || running.is_present("second.pdf")));
    assert!(eventually(PATIENCE, || running.state_of("first.pdf")
        == Some(InstanceState::Absent)));
}

#[test]
fn a_root_that_goes_away_and_comes_back_is_watched_again() {
    let running = start(IndexerConfig {
        reconcile_interval: Duration::from_millis(200),
        ..config()
    });
    fs::write(running.path("book.pdf"), pdf(1)).unwrap();
    assert!(eventually(PATIENCE, || running.is_confirmed("book.pdf")));

    let aside = running._dir.0.join("aside");
    assert!(eventually(PATIENCE, || fs::rename(&running.root, &aside).is_ok()));
    assert!(eventually(PATIENCE, || {
        running.library.roots().unwrap()[0].status == "unavailable"
    }));
    assert_eq!(
        running.state_of("book.pdf"),
        Some(InstanceState::Present),
        "gone for now is not deleted"
    );

    assert!(eventually(PATIENCE, || fs::rename(&aside, &running.root).is_ok()));
    assert!(eventually(PATIENCE, || {
        running.library.roots().unwrap()[0].status == "available"
    }));
    fs::write(running.path("new.pdf"), pdf(2)).unwrap();
    assert!(eventually(PATIENCE, || running.is_present("new.pdf")));
    assert_eq!(running.state_of("book.pdf"), Some(InstanceState::Present));
}

#[test]
fn changes_while_nothing_was_running_are_found_at_the_start() {
    let dir = TempDir::new();
    let root = dir.0.join("books");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("before.pdf"), pdf(1)).unwrap();
    let identity = {
        let library = Library::open(
            LibraryPaths::in_directory(&dir.0.join("library")),
            options(),
        )
        .unwrap();
        let root_id = library.add_root(&root, &[]).unwrap().id;
        library
            .reconcile(root_id, &musaeum_library::ReconcileScope::Everything)
            .unwrap();
        library.process_content().unwrap();
        library
            .instance_at(&root.join("before.pdf"))
            .unwrap()
            .unwrap()
            .id
    };

    fs::rename(root.join("before.pdf"), root.join("after.pdf")).unwrap();
    fs::write(root.join("added.pdf"), pdf(2)).unwrap();

    let running = start_in(dir, root, config());
    assert_eq!(
        running
            .library
            .instance_at(&running.path("after.pdf"))
            .unwrap()
            .unwrap()
            .id,
        identity
    );
    assert_eq!(running.state_of("added.pdf"), Some(InstanceState::Present));
    assert_eq!(running.library.status().unwrap().unconfirmed_revisions, 0);
}

#[test]
fn stopping_is_prompt_even_in_the_middle_of_a_large_file() {
    let mut running = start(config());
    let large = vec![7u8; 300 * 1024 * 1024];
    fs::write(running.path("large.pdf"), &large).unwrap();
    assert!(eventually(PATIENCE, || running
        .state_of("large.pdf")
        .is_some()));

    let indexer = running.indexer.take().unwrap();
    let started = Instant::now();
    drop(indexer);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "took {:?}",
        started.elapsed()
    );

    // And nothing half-finished is taken for finished.
    let again = Indexer::start(Arc::clone(&running.library), config(), None).unwrap();
    assert!(again.wait_until_idle(Duration::from_secs(60)));
    assert!(running.is_confirmed("large.pdf"));
}

#[test]
fn asking_for_a_look_covers_a_root_added_since_the_start() {
    let running = start(config());
    let second = running._dir.0.join("second");
    fs::create_dir(&second).unwrap();
    fs::write(second.join("book.pdf"), pdf(1)).unwrap();
    running.library.add_root(&second, &[]).unwrap();

    let indexer = running.indexer.as_ref().unwrap();
    indexer.reconcile_now();
    assert!(indexer.wait_until_idle(PATIENCE));
    assert!(
        running
            .library
            .instance_at(&second.join("book.pdf"))
            .unwrap()
            .is_some()
    );

    // And it is being watched.
    fs::write(second.join("another.pdf"), pdf(2)).unwrap();
    assert!(eventually(PATIENCE, || {
        running
            .library
            .instance_at(&second.join("another.pdf"))
            .unwrap()
            .is_some()
    }));
}

#[test]
fn a_backup_is_made_when_it_is_due() {
    let running = start(config());
    assert!(eventually(PATIENCE, || running
        .library
        .backups()
        .unwrap()
        .len()
        == 1));
}
