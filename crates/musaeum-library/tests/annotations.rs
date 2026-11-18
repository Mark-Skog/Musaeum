//! Annotations in `library.db`: what is written comes back whole, the search finds it in any
//! accent, and nothing the schema guarantees can be undone by a statement.

mod common;

use common::{Fixture, TempDir, anchored_highlight, options, pdf};

use musaeum_library::{
    AnchorState, AnnotationEdit, AnnotationKind, Library, LibraryPaths, MatchKind, Reanchoring,
    SegmentUpdate, Selector, SelectorKind,
};
use serde_json::json;

fn book(fixture: &Fixture) -> common::Identity {
    let path = fixture.write("livro.pdf", &pdf(7));
    fixture.settle();
    fixture.identity(&path)
}

#[test]
fn an_annotation_comes_back_as_it_was_written() {
    let fixture = Fixture::new();
    let identity = book(&fixture);
    let library = fixture.library();
    let made = anchored_highlight(identity.document, identity.revision);
    let stored = library.create_annotation(made.clone()).unwrap();

    assert_eq!(stored.uuid.len(), 36);
    assert_eq!(&stored.uuid[14..15], "7", "a UUIDv7");
    assert_eq!(stored.kind, AnnotationKind::Highlight);
    assert_eq!(stored.color, "green");
    assert_eq!(stored.note, made.note);
    assert_eq!(stored.quote, made.quote);
    assert_eq!(stored.anchor_state, AnchorState::Exact);
    assert_eq!(stored.resolved_revision_id, Some(identity.revision));
    assert_eq!(stored.segments, made.segments);

    let listed = library.annotations(identity.document, false).unwrap();
    assert_eq!(listed, vec![stored.clone()]);
    assert_eq!(library.annotation(&stored.uuid).unwrap(), stored);
}

#[test]
fn edits_change_only_what_a_person_can_change() {
    let fixture = Fixture::new();
    let identity = book(&fixture);
    let library = fixture.library();
    let stored = library
        .create_annotation(anchored_highlight(identity.document, identity.revision))
        .unwrap();
    let edited = library
        .edit_annotation(
            &stored.uuid,
            AnnotationEdit {
                kind: Some(AnnotationKind::Underline),
                color: Some("blue".to_owned()),
                note: Some("# Uma nota\n\ncom *Markdown*".to_owned()),
            },
        )
        .unwrap();
    assert_eq!(edited.kind, AnnotationKind::Underline);
    assert_eq!(edited.color, "blue");
    assert_eq!(edited.note, "# Uma nota\n\ncom *Markdown*");
    assert_eq!(edited.quote, stored.quote);
    assert_eq!(edited.segments, stored.segments);
    assert!(edited.updated_at >= stored.updated_at);

    let untouched = library
        .edit_annotation(&stored.uuid, AnnotationEdit::default())
        .unwrap();
    assert_eq!(untouched.color, "blue");
    assert!(
        library
            .edit_annotation(
                &stored.uuid,
                AnnotationEdit {
                    color: Some("red; DROP TABLE annotation".to_owned()),
                    ..AnnotationEdit::default()
                },
            )
            .is_err()
    );
}

#[test]
fn deleting_is_logical_and_restoring_brings_it_back_to_the_search() {
    let fixture = Fixture::new();
    let identity = book(&fixture);
    let library = fixture.library();
    let stored = library
        .create_annotation(anchored_highlight(identity.document, identity.revision))
        .unwrap();
    assert_eq!(
        library
            .search_annotations(None, "quoted", 10)
            .unwrap()
            .len(),
        1
    );

    let deleted = library.delete_annotation(&stored.uuid).unwrap();
    assert!(deleted.deleted_at.is_some());
    assert!(
        library
            .annotations(identity.document, false)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        library.annotations(identity.document, true).unwrap().len(),
        1
    );
    assert!(
        library
            .search_annotations(None, "quoted", 10)
            .unwrap()
            .is_empty()
    );

    let restored = library.restore_annotation(&stored.uuid).unwrap();
    assert_eq!(restored.deleted_at, None);
    assert_eq!(restored.segments, stored.segments);
    assert_eq!(
        library
            .search_annotations(None, "quoted", 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn the_search_finds_quotes_and_notes_in_any_accent_and_inside_words() {
    let fixture = Fixture::new();
    let identity = book(&fixture);
    let library = fixture.library();
    let mut made = anchored_highlight(identity.document, identity.revision);
    made.quote = Some("Capitu tinha olhos de ressaca, de cigana oblíqua e dissimulada".to_owned());
    made.note = "O **coração** de Bentinho".to_owned();
    let first = library.create_annotation(made.clone()).unwrap();
    made.quote = Some("Nada se emenda bem nos livros confusos".to_owned());
    made.note = String::new();
    let second = library.create_annotation(made).unwrap();

    let found = |query: &str| -> Vec<String> {
        library
            .search_annotations(Some(identity.document), query, 10)
            .unwrap()
            .into_iter()
            .map(|hit| hit.annotation.uuid)
            .collect()
    };
    assert_eq!(
        found("obliqua"),
        vec![first.uuid.clone()],
        "without the accent"
    );
    assert_eq!(
        found("CORACAO"),
        vec![first.uuid.clone()],
        "in the note, any case"
    );
    assert_eq!(found("dissim"), vec![first.uuid.clone()], "a word's start");
    assert_eq!(
        found("menda"),
        vec![second.uuid.clone()],
        "inside a word, by trigrams"
    );
    assert_eq!(
        found("livros capitu"),
        Vec::<String>::new(),
        "every word has to be there"
    );
    assert!(found("x").is_empty());
    let hits = library.search_annotations(None, "emenda", 10).unwrap();
    assert_eq!(hits[0].matched, MatchKind::WordStart);
    assert!(
        library
            .search_annotations(Some(identity.document + 1000), "capitu", 10)
            .unwrap()
            .is_empty(),
        "only in the document asked for"
    );
}

#[test]
fn an_orphan_keeps_its_quote_and_note_and_is_still_found() {
    let fixture = Fixture::new();
    let identity = book(&fixture);
    let library = fixture.library();
    let stored = library
        .create_annotation(anchored_highlight(identity.document, identity.revision))
        .unwrap();
    library
        .record_anchoring(vec![Reanchoring {
            uuid: stored.uuid.clone(),
            anchor_state: AnchorState::Orphan,
            progression: None,
            revision_id: identity.revision,
            version: Some(2),
            segments: vec![SegmentUpdate {
                ordinal: 0,
                page: Some(0),
                href: None,
                anchor_state: AnchorState::Orphan,
                selectors: Vec::new(),
            }],
        }])
        .unwrap();
    let orphan = library.annotation(&stored.uuid).unwrap();
    assert_eq!(orphan.anchor_state, AnchorState::Orphan);
    assert_eq!(orphan.segments[0].anchor_state, AnchorState::Orphan);
    assert_eq!(orphan.quote, stored.quote);
    assert_eq!(orphan.note, stored.note);
    assert_eq!(
        orphan.segments[0].selectors, stored.segments[0].selectors,
        "selectors kept as they were"
    );
    assert_eq!(
        library.annotations(identity.document, false).unwrap().len(),
        1
    );
    assert_eq!(
        library
            .search_annotations(None, "passage", 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn reanchoring_replaces_selectors_but_never_the_quote() {
    let fixture = Fixture::new();
    let identity = book(&fixture);
    let library = fixture.library();
    let stored = library
        .create_annotation(anchored_highlight(identity.document, identity.revision))
        .unwrap();
    let moved = Selector {
        kind: SelectorKind::PageCharRange,
        data: json!({"page": 1, "start": 3, "end": 19}),
        priority: 0,
        normalizer_version: Some(2),
        revision_id: Some(identity.revision),
    };
    let sneaky_quote = Selector {
        kind: SelectorKind::TextQuote,
        data: json!({"exact": "something else"}),
        priority: 2,
        normalizer_version: None,
        revision_id: None,
    };
    library
        .record_anchoring(vec![Reanchoring {
            uuid: stored.uuid.clone(),
            anchor_state: AnchorState::Approximate,
            progression: Some(0.5),
            revision_id: identity.revision,
            version: Some(2),
            segments: vec![SegmentUpdate {
                ordinal: 0,
                page: Some(1),
                href: None,
                anchor_state: AnchorState::Approximate,
                selectors: vec![moved.clone(), sneaky_quote],
            }],
        }])
        .unwrap();
    let again = library.annotation(&stored.uuid).unwrap();
    assert_eq!(again.anchor_state, AnchorState::Approximate);
    assert_eq!(again.progression, Some(0.5));
    assert_eq!(again.resolved_version, Some(2));
    assert_eq!(again.segments[0].page, Some(1));
    let selectors = &again.segments[0].selectors;
    assert_eq!(selectors.len(), 3);
    assert_eq!(
        selectors
            .iter()
            .find(|s| s.kind == SelectorKind::PageCharRange),
        Some(&moved)
    );
    assert_eq!(
        selectors
            .iter()
            .find(|s| s.kind == SelectorKind::TextQuote)
            .unwrap()
            .data,
        stored.segments[0].selectors[2].data,
        "the quote stays what was quoted"
    );
}

#[test]
fn the_schema_refuses_what_would_lose_an_annotation() {
    let fixture = Fixture::new();
    let identity = book(&fixture);
    let stored = fixture
        .library()
        .create_annotation(anchored_highlight(identity.document, identity.revision))
        .unwrap();
    // A writable connection of the test's own, so a refusal can only come from the schema.
    let connection = rusqlite::Connection::open(fixture.paths.library_db()).unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    connection
        .busy_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    assert_eq!(
        connection
            .execute("UPDATE annotation SET color = 'pink'", [])
            .unwrap(),
        1,
        "what may change, changes"
    );
    for statement in [
        "DELETE FROM annotation",
        "DELETE FROM annotation_segment",
        "DELETE FROM anchor_selector",
        "UPDATE annotation SET uuid = randomblob(16)",
        "UPDATE annotation SET quote = 'rewritten'",
        "UPDATE annotation SET quote = NULL",
        "UPDATE anchor_selector SET data = '{}' WHERE kind = 'text_quote'",
        "UPDATE anchor_selector SET kind = 'cfi' WHERE kind = 'text_quote'",
        // A UUID of another version than 7 (a 4). Not `randomblob(16)`: one time in 16 its version
        // digit is a 7, the row is a valid annotation, and the test failed by chance.
        "INSERT INTO annotation (uuid, document_id, revision_id, kind, color, quote_source, anchor_state, created_at, updated_at)
             VALUES (X'0190A1B2C3D44E5F8A0B1C2D3E4F5061', 1, 1, 'highlight', 'yellow', 'none', 'exact', 0, 0)",
        "INSERT INTO annotation_segment (annotation_id, ordinal, page, href, anchor_state)
             VALUES (1, 9, 1, 'chapter.xhtml', 'exact')",
    ] {
        assert!(connection.execute(statement, []).is_err(), "{statement} was allowed");
    }
    let count: i64 = connection
        .query_row("SELECT count(*) FROM anchor_selector", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 3);
    let quote: String = connection
        .query_row(
            "SELECT quote FROM annotation WHERE uuid = ?1",
            [uuid_bytes(&stored.uuid)],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(quote, "a quoted passage");
}

fn uuid_bytes(text: &str) -> Vec<u8> {
    let hex: String = text.chars().filter(|c| *c != '-').collect();
    (0..16)
        .map(|at| u8::from_str_radix(&hex[at * 2..at * 2 + 2], 16).unwrap())
        .collect()
}

#[test]
fn a_phase_1_note_survives_the_upgrade_with_a_uuid_from_when_it_was_made() {
    let directory = TempDir::new();
    let paths = LibraryPaths::in_directory(&directory.0.join("library"));
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    let connection = rusqlite::Connection::open(paths.library_db()).unwrap();
    for migration in [
        include_str!("../migrations/library/0001_initial.sql"),
        include_str!("../migrations/library/0002_reading.sql"),
        include_str!("../migrations/library/0003_flow_reading.sql"),
    ] {
        connection.execute_batch(migration).unwrap();
    }
    // 2026-11-09 12:00:00 UTC.
    let made_at: i64 = 1_794_225_600_000;
    connection
        .execute_batch(&format!(
            "PRAGMA user_version = 3;
             PRAGMA application_id = 1297437516;
             INSERT INTO work (id, title, created_at, updated_at) VALUES (1, 'Dom Casmurro', 1, 1);
             INSERT INTO document (id, work_id, layout, format, created_at) VALUES (1, 1, 'flow', 'epub', 1);
             INSERT INTO revision (id, document_id, size, origin, created_at) VALUES (1, 1, 10, 'import', 1);
             INSERT INTO annotation (id, document_id, revision_id, note, created_at, updated_at)
                 VALUES (5, 1, 1, 'uma nota antiga sobre o coração', {made_at}, {made_at});
             INSERT INTO annotation (id, document_id, revision_id, note, created_at, updated_at, deleted_at)
                 VALUES (6, 1, 1, 'apagada', {made_at}, {made_at}, {made_at});"
        ))
        .unwrap();
    drop(connection);

    let library = Library::open(paths, options()).unwrap();
    let kept = library.annotations(1, true).unwrap();
    assert_eq!(kept.len(), 2);
    let note = &kept[0];
    assert_eq!(note.id, 5);
    assert_eq!(note.note, "uma nota antiga sobre o coração");
    assert_eq!(note.kind, AnnotationKind::Note);
    assert_eq!(note.anchor_state, AnchorState::Exact);
    assert!(note.segments.is_empty());
    assert_eq!(note.created_at, made_at);
    let bytes = uuid_bytes(&note.uuid);
    let millis = bytes[..6]
        .iter()
        .fold(0i64, |sum, byte| (sum << 8) | i64::from(*byte));
    assert_eq!(millis, made_at, "the UUID says when it was made");
    assert_eq!(bytes[6] >> 4, 7);
    assert_eq!(bytes[8] >> 6, 2, "the RFC 9562 variant");
    assert_ne!(kept[0].uuid, kept[1].uuid);
    assert!(kept[1].deleted_at.is_some());
    let found = library.search_annotations(None, "coracao", 10).unwrap();
    assert_eq!(
        found.len(),
        1,
        "the old note is searchable, the deleted one is not"
    );
    assert_eq!(found[0].annotation.id, 5);
}

#[test]
fn a_phase_4_library_keeps_its_selectors_and_learns_marks_on_pictures() {
    use musaeum_library::anchoring::region::{self, Region, RegionLine, RegionTool};

    let directory = TempDir::new();
    let paths = LibraryPaths::in_directory(&directory.0.join("library"));
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    let connection = rusqlite::Connection::open(paths.library_db()).unwrap();
    for migration in [
        include_str!("../migrations/library/0001_initial.sql"),
        include_str!("../migrations/library/0002_reading.sql"),
        include_str!("../migrations/library/0003_flow_reading.sql"),
        include_str!("../migrations/library/0004_annotations.sql"),
    ] {
        connection.execute_batch(migration).unwrap();
    }
    connection
        .execute_batch(
            r#"PRAGMA user_version = 4;
             PRAGMA application_id = 1297437516;
             INSERT INTO work (id, title, created_at, updated_at) VALUES (1, 'Os Lusíadas', 1, 1);
             INSERT INTO document (id, work_id, layout, format, created_at) VALUES (1, 1, 'fixed', 'pdf', 1);
             INSERT INTO revision (id, document_id, size, origin, created_at) VALUES (1, 1, 10, 'import', 1);
             INSERT INTO annotation (id, uuid, document_id, revision_id, kind, color, note, quote,
                     quote_source, progression, anchor_state, resolved_revision_id, resolved_version,
                     created_at, updated_at)
                 VALUES (3, x'019a6f0e8c007000800000000000abcd', 1, 1, 'highlight', 'yellow', '',
                     'as armas e os barões', 'text_layer', 0.0, 'exact', 1, 1, 1, 1);
             INSERT INTO annotation_segment (id, annotation_id, ordinal, page, anchor_state)
                 VALUES (8, 3, 0, 0, 'exact');
             INSERT INTO anchor_selector (id, segment_id, kind, data, priority, normalizer_version, revision_id)
                 VALUES (21, 8, 'page_char_range', '{"page":0,"start":0,"end":20}', 0, 1, 1),
                        (22, 8, 'text_quote', '{"exact":"as armas e os barões","prefix":"","suffix":""}', 2, NULL, NULL);"#,
        )
        .unwrap();
    drop(connection);

    let library = Library::open(paths.clone(), options()).unwrap();
    let kept = library.annotations(1, false).unwrap();
    assert_eq!(kept.len(), 1);
    let kinds: Vec<SelectorKind> = kept[0].segments[0]
        .selectors
        .iter()
        .map(|selector| selector.kind)
        .collect();
    assert_eq!(
        kinds,
        [SelectorKind::PageCharRange, SelectorKind::TextQuote]
    );

    // A mark along two lines of a scan, on the same document.
    let lines = Region {
        page: 0,
        tool: RegionTool::Lines,
        quads: vec![
            [0.12, 0.30, 0.88, 0.30, 0.88, 0.33, 0.12, 0.33],
            [0.12, 0.34, 0.47, 0.34, 0.47, 0.37, 0.12, 0.37],
        ],
        clip: [0.12, 0.30, 0.88, 0.37],
        lines: vec![
            RegionLine {
                index: 6,
                from: 0.0,
                to: 1.0,
            },
            RegionLine {
                index: 7,
                from: 0.0,
                to: 0.46,
            },
        ],
        path: Vec::new(),
        page_hash: Some(region::hash_text(u64::MAX - 1)),
        region_hash: Some(region::hash_text(42)),
    };
    let made = library
        .create_annotation(musaeum_library::NewAnnotation {
            document_id: 1,
            revision_id: 1,
            kind: AnnotationKind::Highlight,
            color: "pink".to_owned(),
            note: String::new(),
            quote: None,
            quote_source: musaeum_library::QuoteSource::None,
            progression: Some(0.3),
            anchor_state: AnchorState::Exact,
            resolved_version: Some(1),
            pdf_nm: None,
            segments: vec![lines.segment(0, 1, 1)],
        })
        .unwrap();
    drop(library);
    let library = Library::open(paths.clone(), options()).unwrap();
    let again = library.annotation(&made.uuid).unwrap();
    assert_eq!(region::region_of(&again.segments[0]), Some(lines));
    assert_eq!(again.segments[0].clip, made.segments[0].clip);
    drop(library);

    // The triggers came back with the table.
    let connection = rusqlite::Connection::open(paths.library_db()).unwrap();
    for (sql, why) in [
        (
            "DELETE FROM anchor_selector WHERE id = 21",
            "a selector is never deleted",
        ),
        (
            "UPDATE anchor_selector SET data = '{\"exact\":\"outra\"}' WHERE id = 22",
            "a quote keeps what it quoted",
        ),
        (
            "UPDATE anchor_selector SET kind = 'page_pixels' WHERE id = 21",
            "a kind that does not exist",
        ),
    ] {
        assert!(connection.execute(sql, []).is_err(), "{why}");
    }
}

#[test]
fn a_mark_on_a_scan_learns_its_quote_once_and_is_found_by_it() {
    use musaeum_library::QuoteSource;
    use musaeum_library::anchoring::region::{self, Region, RegionLine, RegionTool};

    let fixture = Fixture::new();
    let identity = book(&fixture);
    let library = fixture.library();
    let lines = Region {
        page: 0,
        tool: RegionTool::Lines,
        quads: vec![[0.12, 0.30, 0.88, 0.30, 0.88, 0.33, 0.12, 0.33]],
        clip: [0.12, 0.30, 0.88, 0.33],
        lines: vec![RegionLine {
            index: 2,
            from: 0.0,
            to: 1.0,
        }],
        path: Vec::new(),
        page_hash: Some(region::hash_text(7)),
        region_hash: Some(region::hash_text(8)),
    };
    let made = library
        .create_annotation(musaeum_library::NewAnnotation {
            document_id: identity.document,
            revision_id: identity.revision,
            kind: AnnotationKind::Highlight,
            color: "yellow".to_owned(),
            note: String::new(),
            quote: None,
            quote_source: QuoteSource::None,
            progression: Some(0.0),
            anchor_state: AnchorState::Exact,
            resolved_version: Some(1),
            pdf_nm: None,
            segments: vec![lines.segment(0, identity.revision, 2)],
        })
        .unwrap();
    let found = |query: &str| -> Vec<String> {
        library
            .search_annotations(Some(identity.document), query, 10)
            .unwrap()
            .into_iter()
            .map(|hit| hit.annotation.uuid)
            .collect()
    };
    assert!(found("baroes").is_empty(), "nothing read yet");

    let read = "As armas e os barões assinalados";
    let quote_selector = |exact: &str| Selector {
        kind: SelectorKind::TextQuote,
        data: json!({"exact": exact, "prefix": "", "suffix": ""}),
        priority: 2,
        normalizer_version: None,
        revision_id: None,
    };
    let filled = library
        .fill_quote(
            &made.uuid,
            read,
            QuoteSource::Ocr,
            // A segment the annotation does not have is passed over.
            vec![(0, quote_selector(read)), (5, quote_selector("nada"))],
        )
        .unwrap();
    assert_eq!(filled.quote.as_deref(), Some(read));
    assert_eq!(filled.quote_source, QuoteSource::Ocr);
    let kinds: Vec<SelectorKind> = filled.segments[0]
        .selectors
        .iter()
        .map(|selector| selector.kind)
        .collect();
    assert_eq!(kinds, [SelectorKind::PageRegion, SelectorKind::TextQuote]);
    assert_eq!(region::region_of(&filled.segments[0]), Some(lines));
    assert_eq!(found("baroes"), vec![made.uuid.clone()], "found at once");
    assert_eq!(found("assinal"), vec![made.uuid.clone()]);

    // Read again, by another OCR: what it quoted stays.
    let again = library
        .fill_quote(
            &made.uuid,
            "As armas e os varões",
            QuoteSource::Ocr,
            vec![(0, quote_selector("As armas e os varões"))],
        )
        .unwrap();
    assert_eq!(again, filled);
    assert!(found("varoes").is_empty());

    // A change of color leaves the search as it was; deleting takes it out.
    library
        .edit_annotation(
            &made.uuid,
            AnnotationEdit {
                color: Some("blue".to_owned()),
                ..AnnotationEdit::default()
            },
        )
        .unwrap();
    assert_eq!(found("baroes"), vec![made.uuid.clone()]);

    for empty in ["", "   "] {
        assert!(matches!(
            library.fill_quote(&made.uuid, empty, QuoteSource::Ocr, Vec::new()),
            Err(musaeum_library::Error::Refused(_))
        ));
    }
    let connection = rusqlite::Connection::open(fixture.paths.library_db()).unwrap();
    assert!(
        connection
            .execute(
                "UPDATE annotation SET quote = 'outra' WHERE quote IS NOT NULL",
                []
            )
            .is_err(),
        "a quote once filled keeps what it quoted"
    );
}
