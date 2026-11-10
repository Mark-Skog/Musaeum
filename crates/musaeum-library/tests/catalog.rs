//! What the library surface lists: books by title, by a search, in a folder; its folders; the file
//! each book opens; and where the reader is in a flowing book.

mod common;

use common::{Fixture, epub, pdf};
use musaeum_library::{BrowseQuery, FlowReadingState, ReadingMode};

fn query(text: &str) -> BrowseQuery {
    BrowseQuery {
        text: text.to_owned(),
        limit: 100,
        ..BrowseQuery::default()
    }
}

fn titles(fixture: &Fixture, query: &BrowseQuery) -> Vec<String> {
    fixture
        .library()
        .browse(query)
        .unwrap()
        .rows
        .into_iter()
        .map(|row| row.title)
        .collect()
}

/// Three books in three folders, one of them inside another.
fn shelves() -> Fixture {
    let fixture = Fixture::new();
    fixture.write(
        "romances/dom.epub",
        &epub("Dom Casmurro", &["Machado de Assis"], None, 1),
    );
    fixture.write(
        "romances/antigos/memorias.epub",
        &epub("Memórias Póstumas", &["Machado de Assis"], None, 2),
    );
    fixture.write("tecnicos/zeta.pdf", &pdf(3));
    fixture.settle();
    fixture
}

#[test]
fn every_book_is_listed_once_by_title() {
    let fixture = shelves();
    let page = fixture.library().browse(&query("")).unwrap();
    assert_eq!(page.total, 3);
    let titles: Vec<&str> = page.rows.iter().map(|row| row.title.as_str()).collect();
    assert_eq!(titles[..2], ["Dom Casmurro", "Memórias Póstumas"]);
    let dom = &page.rows[0];
    assert_eq!(dom.layout, "flow");
    assert_eq!(dom.format, "epub");
    assert_eq!(dom.state, "present");
    assert_eq!(dom.creators.as_deref(), Some("Machado de Assis"));
    assert!(dom.blake3.as_ref().is_some_and(|hash| hash.len() == 64));
    assert!(dom.path.ends_with("dom.epub"), "{}", dom.path);
    assert_eq!(dom.progress, None);
    let zeta = page.rows.iter().find(|row| row.format == "pdf").unwrap();
    assert_eq!(zeta.layout, "fixed");
}

#[test]
fn a_listing_comes_in_pages() {
    let fixture = shelves();
    let first = fixture
        .library()
        .browse(&BrowseQuery {
            limit: 2,
            ..BrowseQuery::default()
        })
        .unwrap();
    let rest = fixture
        .library()
        .browse(&BrowseQuery {
            offset: 2,
            limit: 2,
            ..BrowseQuery::default()
        })
        .unwrap();
    assert_eq!((first.total, first.rows.len()), (3, 2));
    assert_eq!((rest.total, rest.rows.len(), rest.offset), (3, 1, 2));
    assert!(
        first
            .rows
            .iter()
            .all(|row| row.document_id != rest.rows[0].document_id)
    );
}

#[test]
fn a_search_lists_what_it_finds_best_first() {
    let fixture = shelves();
    assert_eq!(titles(&fixture, &query("memórias")), ["Memórias Póstumas"]);
    assert_eq!(titles(&fixture, &query("machado")).len(), 2);
    assert_eq!(titles(&fixture, &query("smur")), ["Dom Casmurro"]);
    let nothing = fixture.library().browse(&query("Dostoiévski")).unwrap();
    assert_eq!((nothing.total, nothing.rows.len()), (0, 0));
}

#[test]
fn a_folder_lists_what_is_in_it_and_below_it() {
    let fixture = shelves();
    let folders = fixture.library().folders().unwrap();
    let named = |name: &str| folders.iter().find(|folder| folder.name == name).unwrap();
    let romances = named("romances");
    let antigos = named("antigos");
    assert_eq!(antigos.parent_id, Some(romances.id));
    assert_eq!((romances.books, antigos.books), (1, 1));
    let top = folders
        .iter()
        .find(|folder| folder.parent_id.is_none())
        .unwrap();
    assert!(
        top.name.len() > 3,
        "a root shows its whole path: {}",
        top.name
    );

    let in_folder = |folder, text: &str| {
        titles(
            &fixture,
            &BrowseQuery {
                folder: Some(folder),
                ..query(text)
            },
        )
    };
    assert_eq!(
        in_folder(romances.id, ""),
        ["Dom Casmurro", "Memórias Póstumas"]
    );
    assert_eq!(in_folder(antigos.id, ""), ["Memórias Póstumas"]);
    assert_eq!(in_folder(antigos.id, "dom"), Vec::<String>::new());
    assert_eq!(in_folder(top.id, "").len(), 3);
}

#[test]
fn a_book_opens_from_where_it_is() {
    let fixture = shelves();
    let row = fixture.library().browse(&query("dom")).unwrap().rows[0].clone();
    let file = fixture
        .library()
        .book_file(row.document_id)
        .unwrap()
        .unwrap();
    assert_eq!(file.path, fixture.path("romances/dom.epub"));
    assert_eq!(
        (file.layout.as_str(), file.format.as_str()),
        ("flow", "epub")
    );
    assert_eq!(file.revision_id, row.revision_id);
    assert!(file.blake3.is_some());
    assert_eq!(fixture.library().hashed_book_files().unwrap().len(), 3);

    // Gone from the disk, it is no longer listed, and there is nothing to open.
    std::fs::remove_file(fixture.path("romances/dom.epub")).unwrap();
    fixture.reconcile();
    assert_eq!(fixture.library().browse(&query("")).unwrap().total, 2);
    assert_eq!(fixture.library().book_file(row.document_id).unwrap(), None);
}

#[test]
fn a_flowing_book_keeps_its_locator() {
    let fixture = shelves();
    let row = fixture.library().browse(&query("dom")).unwrap().rows[0].clone();
    let library = fixture.library();
    assert_eq!(library.flow_reading_state(row.document_id).unwrap(), None);

    let locator = r#"{"href":"OEBPS/c1.xhtml","type":"application/xhtml+xml","locations":{"cfi":"epubcfi(/6/2!/4/2/1:10)","progression":0.25,"totalProgression":0.1},"text":{"before":"a","highlight":"b","after":"c"},"normalizer":1}"#;
    let state = FlowReadingState {
        revision_id: Some(row.revision_id),
        locator: locator.to_owned(),
        progress: 0.1,
        mode: ReadingMode::Continuous,
        page_turn_animation: false,
        updated_at: 0,
    };
    library
        .save_flow_reading_state(row.document_id, &state)
        .unwrap();
    let saved = library
        .flow_reading_state(row.document_id)
        .unwrap()
        .unwrap();
    assert_eq!(saved.locator, locator);
    assert_eq!(saved.mode, ReadingMode::Continuous);
    assert!(!saved.page_turn_animation);
    assert!(saved.updated_at > 0);
    let listed = library.browse(&query("dom")).unwrap().rows[0].clone();
    assert_eq!(listed.progress, Some(0.1));

    // Only a JSON object of a sensible size, with a progress between 0 and 1, and a revision of
    // this document.
    for broken in [
        FlowReadingState {
            locator: "not json".to_owned(),
            ..state.clone()
        },
        FlowReadingState {
            locator: "[1, 2]".to_owned(),
            ..state.clone()
        },
        FlowReadingState {
            locator: format!(r#"{{"pad":"{}"}}"#, "x".repeat(20_000)),
            ..state.clone()
        },
        FlowReadingState {
            progress: 1.5,
            ..state.clone()
        },
        FlowReadingState {
            revision_id: Some(row.revision_id + 1000),
            ..state.clone()
        },
    ] {
        assert!(
            library
                .save_flow_reading_state(row.document_id, &broken)
                .is_err()
        );
    }
    assert_eq!(
        library
            .flow_reading_state(row.document_id)
            .unwrap()
            .unwrap()
            .locator,
        locator
    );
}

#[test]
fn settings_are_kept_as_json() {
    let fixture = Fixture::new();
    let library = fixture.library();
    assert_eq!(library.preference("typography").unwrap(), None);
    library
        .set_preference("typography", r#"{"fontSize":120,"theme":"sepia"}"#)
        .unwrap();
    library
        .set_preference("typography", r#"{"fontSize":130,"theme":"dark"}"#)
        .unwrap();
    assert_eq!(
        library.preference("typography").unwrap().as_deref(),
        Some(r#"{"fontSize":130,"theme":"dark"}"#)
    );
    assert!(library.set_preference("typography", "{oops").is_err());
    assert!(library.set_preference("", "{}").is_err());
}
