//! What reading keeps in the library: the position in `library.db` and the pages in `cache.db`, and
//! the first upgrade of an existing library to hold them.

mod common;

use common::{Fixture, TempDir, options, pdf, pdf_with_id};
use musaeum_library::{
    DocumentGeometry, Error, Library, LibraryPaths, PageOrientation, PageRecord, ReadingMode,
    ReadingState, Spreads,
};

fn state(page: u32) -> ReadingState {
    ReadingState {
        revision_id: None,
        page,
        u: 0.25,
        v: 0.75,
        progress: 0.5,
        mode: ReadingMode::Continuous,
        page_turn_animation: false,
        zoom: 1.5,
        spreads: Spreads::Double,
        updated_at: 0,
    }
}

#[test]
fn a_position_is_kept_exactly_as_it_was_saved() {
    let mut fixture = Fixture::new();
    let path = fixture.write("book.pdf", &pdf(1));
    fixture.settle();
    let identity = fixture.identity(&path);
    assert_eq!(
        fixture.library().reading_state(identity.document).unwrap(),
        None
    );

    let saved = ReadingState {
        revision_id: Some(identity.revision),
        ..state(41)
    };
    fixture
        .library()
        .save_reading_state(identity.document, &saved)
        .unwrap();
    fixture.close();
    fixture.reopen();

    let loaded = fixture
        .library()
        .reading_state(identity.document)
        .unwrap()
        .unwrap();
    assert!(loaded.updated_at > 0, "the library dates it");
    assert_eq!(
        ReadingState {
            updated_at: 0,
            ..loaded
        },
        saved,
        "every field comes back, the position to the last bit"
    );

    let paginated = ReadingState {
        mode: ReadingMode::Paginated,
        page_turn_animation: true,
        ..saved
    };
    fixture
        .library()
        .save_reading_state(identity.document, &paginated)
        .unwrap();
    let loaded = fixture
        .library()
        .reading_state(identity.document)
        .unwrap()
        .unwrap();
    assert_eq!(loaded.mode, ReadingMode::Paginated);
    assert!(loaded.page_turn_animation);
    assert_eq!((loaded.page, loaded.u, loaded.v), (41, 0.25, 0.75));
}

#[test]
fn a_position_that_makes_no_sense_is_refused() {
    let fixture = Fixture::new();
    let book = fixture.write("book.pdf", &pdf(1));
    let other = fixture.write("other.pdf", &pdf(2));
    fixture.settle();
    let book = fixture.identity(&book);
    let other = fixture.identity(&other);

    let refused = |document, state: &ReadingState| {
        fixture
            .library()
            .save_reading_state(document, state)
            .unwrap_err()
    };
    assert!(matches!(
        refused(
            book.document,
            &ReadingState {
                u: 1.01,
                ..state(0)
            }
        ),
        Error::Refused(_)
    ));
    assert!(matches!(
        refused(
            book.document,
            &ReadingState {
                zoom: f64::NAN,
                ..state(0)
            }
        ),
        Error::Refused(_)
    ));
    assert!(matches!(
        refused(
            book.document,
            &ReadingState {
                revision_id: Some(other.revision),
                ..state(0)
            }
        ),
        Error::Refused(_)
    ));
    assert!(matches!(refused(9_999, &state(0)), Error::NotFound(_)));
    assert_eq!(
        fixture.library().reading_state(book.document).unwrap(),
        None
    );
}

#[test]
fn merging_two_documents_keeps_the_most_recent_position() {
    let fixture = Fixture::new();
    let id = "aabbccddeeff00112233445566778899";
    let original = fixture.write("original.pdf", &pdf_with_id(id, 1));
    fixture.settle();
    let edition = fixture.write("edition.pdf", &pdf_with_id(id, 2));
    fixture.settle();
    let original = fixture.identity(&original);
    let edition = fixture.identity(&edition);

    fixture
        .library()
        .save_reading_state(original.document, &state(3))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    fixture
        .library()
        .save_reading_state(edition.document, &state(7))
        .unwrap();

    let suggestion = fixture.library().suggestions(true).unwrap()[0].id;
    let merged = fixture.library().accept_suggestion(suggestion).unwrap();
    assert_eq!(merged, original.document);
    let kept = fixture.library().reading_state(merged).unwrap().unwrap();
    assert_eq!(kept.page, 7, "the edition was read last");
}

#[test]
fn the_pages_of_a_file_are_kept_by_its_bytes() {
    let fixture = Fixture::new();
    let hash = [7u8; 32];
    assert_eq!(fixture.library().page_geometry(&hash).unwrap(), None);

    let page = |width_pt, height_pt, orientation| PageRecord {
        width_pt,
        height_pt,
        orientation,
    };
    let geometry = DocumentGeometry {
        pages: vec![
            page(612.0, 792.0, PageOrientation::Portrait),
            page(1224.0, 792.0, PageOrientation::Landscape),
            page(500.0, 510.0, PageOrientation::Square),
        ],
        page_layout: Some("TwoPageRight".to_owned()),
        direction: Some("R2L".to_owned()),
    };
    assert_eq!(geometry.summary(), "mixed");
    fixture
        .library()
        .store_page_geometry(&hash, &geometry)
        .unwrap();
    assert_eq!(
        fixture.library().page_geometry(&hash).unwrap(),
        Some(geometry)
    );

    let shorter = DocumentGeometry {
        pages: vec![page(100.0, 200.0, PageOrientation::Portrait)],
        ..DocumentGeometry::default()
    };
    fixture
        .library()
        .store_page_geometry(&hash, &shorter)
        .unwrap();
    assert_eq!(
        fixture.library().page_geometry(&hash).unwrap(),
        Some(shorter),
        "measuring again replaces all of it"
    );
}

/// The schema as Phase 1 shipped it, and what that phase's library would have written in it.
fn a_phase_1_library(paths: &LibraryPaths) {
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    let connection = rusqlite::Connection::open(paths.library_db()).unwrap();
    connection
        .execute_batch(include_str!("../migrations/library/0001_initial.sql"))
        .unwrap();
    connection
        .execute_batch(
            "PRAGMA user_version = 1;
             PRAGMA application_id = 1297437516;
             INSERT INTO work (id, title, created_at, updated_at) VALUES (1, 'Dom Casmurro', 1, 1);
             INSERT INTO document (id, work_id, layout, format, created_at)
                 VALUES (1, 1, 'fixed', 'pdf', 1);
             INSERT INTO revision (id, document_id, size, origin, created_at)
                 VALUES (1, 1, 10, 'import', 1);",
        )
        .unwrap();
}

#[test]
fn a_phase_1_library_is_copied_and_then_upgraded() {
    let directory = TempDir::new();
    let paths = LibraryPaths::in_directory(&directory.0.join("library"));
    a_phase_1_library(&paths);

    let library = Library::open(paths.clone(), options()).unwrap();

    let backups = library.backups().unwrap();
    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0].before_upgrade_from, Some(1));
    let copy = rusqlite::Connection::open(&backups[0].path).unwrap();
    let version: i64 = copy
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 1, "the copy is the way back to the old schema");

    assert_eq!(library.document(1).unwrap().title, "Dom Casmurro");
    library
        .save_reading_state(
            1,
            &ReadingState {
                revision_id: Some(1),
                ..state(0)
            },
        )
        .unwrap();
    assert_eq!(
        library.reading_state(1).unwrap().unwrap().revision_id,
        Some(1)
    );
    drop(library);

    let reopened = Library::open(paths, options()).unwrap();
    assert_eq!(
        reopened.backups().unwrap().len(),
        1,
        "nothing to upgrade the second time"
    );
}
