//! Annotations out as W3C Web Annotations in JSON-LD and back in: into a clean library with the same
//! files, every one comes back as it was (UUID, kind, color, Markdown note, dates, quote, every
//! selector with its data, version and revision); again, nothing more; into a library with another
//! edition, onto the same work, to be found again there; and a hostile file changes nothing.

mod common;

use std::collections::HashMap;

use common::{Fixture, anchored_highlight, epub, pdf};
use musaeum_library::{
    AnchorState, Annotation, AnnotationKind, ExchangeScope, ExchangeTarget, Library, NewAnnotation,
    QuoteSource, RevisionId, Segment, Selector, SelectorKind,
};
use serde_json::{Value, json};

const NOW: i64 = 1_795_466_405_123;

struct Books {
    pdf: common::Identity,
    epub: common::Identity,
}

fn shelf(fixture: &Fixture, epub_seed: u64) -> Books {
    let pdf_path = fixture.write("livros/física.pdf", &pdf(7));
    let epub_path = fixture.write(
        "livros/dom casmurro.epub",
        &epub(
            "Dom Casmurro",
            &["Machado de Assis"],
            Some("urn:isbn:9788535902778"),
            epub_seed,
        ),
    );
    fixture.settle();
    Books {
        pdf: fixture.identity(&pdf_path),
        epub: fixture.identity(&epub_path),
    }
}

fn selector(
    kind: SelectorKind,
    priority: i64,
    data: Value,
    version: Option<u32>,
    revision: Option<RevisionId>,
) -> Selector {
    Selector {
        kind,
        data,
        priority,
        normalizer_version: version,
        revision_id: revision,
    }
}

/// A highlight, a mark on the picture of a turned page, a note about the whole book and a note with
/// a PDF's name on the PDF; a highlight over two documents and a note on the EPUB.
fn annotate(library: &Library, books: &Books) -> Vec<Annotation> {
    let (document, revision) = (books.pdf.document, books.pdf.revision);
    let mut made = Vec::new();
    let mut highlight = anchored_highlight(document, revision);
    highlight.note =
        "# Ação\n\n**coração** e *pão* — “aspas” → ✓\n\n- um\n- dois\n\n```\nlet x = 1;\n```"
            .to_owned();
    made.push(highlight);
    made.push(NewAnnotation {
        document_id: document,
        revision_id: revision,
        kind: AnnotationKind::Highlight,
        color: "purple".to_owned(),
        note: String::new(),
        quote: Some("texto lido pelo OCR".to_owned()),
        quote_source: QuoteSource::Ocr,
        // A number the parser has to read to its last bit (one that came back changed by one).
        progression: Some(0.0014092306675042153),
        anchor_state: AnchorState::Approximate,
        resolved_version: Some(2),
        pdf_nm: None,
        segments: vec![Segment {
            ordinal: 0,
            page: Some(1),
            href: None,
            clip: Some([0.1, 0.2, 0.5, 0.25]),
            anchor_state: AnchorState::Approximate,
            selectors: vec![selector(
                SelectorKind::PageRegion,
                0,
                json!({
                    "page": 1, "tool": "free",
                    "quads": [[0.1, 0.2, 0.5, 0.2, 0.5, 0.25, 0.1, 0.25]],
                    "clip": [0.1, 0.2, 0.5, 0.25], "lines": [],
                    "path": [0.1, 0.22, 0.3, 0.23, 0.5, 0.21],
                    "width": 2.5, "pageHash": "0123456789abcdef", "regionHash": "fedcba9876543210"
                }),
                None,
                Some(revision),
            )],
        }],
    });
    made.push(NewAnnotation {
        document_id: document,
        revision_id: revision,
        kind: AnnotationKind::Note,
        color: "yellow".to_owned(),
        note: "Sobre o livro todo.".to_owned(),
        quote: None,
        quote_source: QuoteSource::None,
        progression: None,
        anchor_state: AnchorState::Exact,
        resolved_version: None,
        pdf_nm: None,
        segments: Vec::new(),
    });
    let mut named = anchored_highlight(document, revision);
    named.kind = AnnotationKind::Underline;
    named.color = "#12ab34cc".to_owned();
    named.pdf_nm = Some("existing-highlight".to_owned());
    named.note = String::new();
    made.push(named);

    let (document, revision) = (books.epub.document, books.epub.revision);
    let flow = |ordinal: u32, href: &str, start: u64, end: u64, quote: &str| Segment {
        ordinal,
        page: None,
        href: Some(href.to_owned()),
        clip: None,
        anchor_state: AnchorState::Exact,
        selectors: vec![
            selector(
                SelectorKind::Cfi,
                0,
                json!({"start": format!("/6/{}!/4/2/1:{start}", 4 + ordinal * 2), "end": format!("/6/{}!/4/2/1:{end}", 4 + ordinal * 2)}),
                Some(3),
                Some(revision),
            ),
            selector(
                SelectorKind::TextPosition,
                1,
                json!({"start": start, "end": end}),
                Some(3),
                Some(revision),
            ),
            selector(
                SelectorKind::TextQuote,
                2,
                json!({"exact": quote, "prefix": "antes ", "suffix": " depois"}),
                None,
                None,
            ),
        ],
    };
    made.push(NewAnnotation {
        document_id: document,
        revision_id: revision,
        kind: AnnotationKind::Highlight,
        color: "blue".to_owned(),
        note: "Uma nota **em Markdown** com acentos: ação.".to_owned(),
        quote: Some("olhos de ressaca\ncigana oblíqua".to_owned()),
        quote_source: QuoteSource::Flow,
        progression: Some(0.125),
        anchor_state: AnchorState::Exact,
        resolved_version: Some(3),
        pdf_nm: None,
        segments: vec![
            flow(0, "text/ch1.xhtml", 10, 26, "olhos de ressaca"),
            flow(1, "text/ch2.xhtml", 0, 14, "cigana oblíqua"),
        ],
    });
    let mut note = made.last().unwrap().clone();
    note.kind = AnnotationKind::Note;
    note.color = "pink".to_owned();
    note.segments.truncate(1);
    made.push(note);
    made.into_iter()
        .map(|new| library.create_annotation(new).unwrap())
        .collect()
}

fn hashes(library: &Library, document: i64) -> HashMap<RevisionId, String> {
    library
        .document(document)
        .unwrap()
        .revisions
        .into_iter()
        .filter_map(|revision| Some((revision.id, revision.blake3?)))
        .collect()
}

/// An annotation with its revisions named by their bytes, which is what is the same in two
/// libraries.
fn essence(library: &Library, annotation: &Annotation) -> Value {
    let named = hashes(library, annotation.document_id);
    let mut value = serde_json::to_value(annotation).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("id");
    object.remove("documentId");
    for key in ["revisionId", "resolvedRevisionId"] {
        let id = object[key].as_i64();
        object.insert(
            key.to_owned(),
            json!(id.and_then(|id| named.get(&id).cloned())),
        );
    }
    for segment in object["segments"].as_array_mut().unwrap() {
        for selector in segment["selectors"].as_array_mut().unwrap() {
            let id = selector["revisionId"].as_i64();
            selector["revisionId"] = json!(id.and_then(|id| named.get(&id).cloned()));
        }
    }
    value
}

fn all(library: &Library, books: &Books) -> Vec<Annotation> {
    let mut found = library.annotations(books.pdf.document, true).unwrap();
    found.extend(library.annotations(books.epub.document, true).unwrap());
    found.sort_by(|a, b| a.uuid.cmp(&b.uuid));
    found
}

#[test]
fn everything_comes_back_into_a_clean_library_as_it_was_and_only_once() {
    let first = Fixture::new();
    let books = shelf(&first, 1);
    let mut made = annotate(first.library(), &books);
    made.sort_by(|a, b| a.uuid.cmp(&b.uuid));

    let export = first
        .library()
        .export_jsonld(ExchangeScope::Library, NOW, "0.0.1")
        .unwrap();
    assert_eq!(
        (export.annotations, export.documents, export.works),
        (6, 2, 2)
    );
    let file: Value = serde_json::from_str(&export.json).unwrap();
    assert_eq!(file["@context"][0], "http://www.w3.org/ns/anno.jsonld");
    assert_eq!(file["type"], "AnnotationCollection");
    assert_eq!(file["generated"], "2026-11-23T20:40:05.123Z");
    let items = file["first"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 6);
    for item in items {
        assert_eq!(item["type"], "Annotation");
        assert!(item["id"].as_str().unwrap().starts_with("urn:uuid:"));
        let source = &item["target"][0]["source"];
        assert!(
            source["id"].as_str().unwrap().starts_with("urn:blake3:"),
            "{source}"
        );
    }
    let types: Vec<&str> = items
        .iter()
        .flat_map(|item| item["target"].as_array().unwrap())
        .flat_map(|target| target["selector"].as_array().into_iter().flatten())
        .map(|selector| selector["type"].as_str().unwrap())
        .collect();
    for expected in [
        "TextQuoteSelector",
        "TextPositionSelector",
        "RangeSelector",
        "FragmentSelector",
        "musaeum:PageCharRangeSelector",
        "musaeum:PageQuadsSelector",
        "musaeum:PageRegionSelector",
    ] {
        assert!(types.contains(&expected), "{expected} in {types:?}");
    }

    // A clean library with the same files: everything, as it was.
    let second = Fixture::new();
    let there = shelf(&second, 1);
    let brought = second
        .library()
        .import_jsonld(export.json.as_bytes(), ExchangeTarget::Library)
        .unwrap();
    assert_eq!(brought.total, 6);
    assert_eq!(
        brought
            .into
            .iter()
            .map(|into| into.added.len())
            .sum::<usize>(),
        6
    );
    assert!(
        brought.into.iter().all(|into| !into.needs_anchoring),
        "the same bytes: nothing to find again"
    );
    assert_eq!(
        (brought.already, brought.elsewhere, brought.other_work),
        (0, 0, 0)
    );
    assert!(brought.unmatched.is_empty());
    let back = all(second.library(), &there);
    assert_eq!(back.len(), 6);
    for (original, restored) in made.iter().zip(&back) {
        assert_eq!(
            essence(second.library(), restored),
            essence(first.library(), original)
        );
    }
    // And the clean library writes the very same annotations back, item for item.
    let written_back = second
        .library()
        .export_jsonld(ExchangeScope::Library, NOW, "0.0.1")
        .unwrap();
    let back_file: Value = serde_json::from_str(&written_back.json).unwrap();
    let by_id = |file: &Value| -> Vec<Value> {
        let mut items = file["first"]["items"].as_array().unwrap().clone();
        items.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        items
    };
    for (original, restored) in by_id(&file).iter().zip(by_id(&back_file)) {
        assert_eq!(&restored, original);
    }
    // The search knows them.
    assert_eq!(
        second
            .library()
            .search_annotations(None, "coracao", 10)
            .unwrap()
            .len(),
        1
    );

    // Again: nothing more, here and where they came from.
    let again = second
        .library()
        .import_jsonld(export.json.as_bytes(), ExchangeTarget::Library)
        .unwrap();
    assert_eq!((again.already, again.into.len()), (6, 0));
    let home = first
        .library()
        .import_jsonld(export.json.as_bytes(), ExchangeTarget::Library)
        .unwrap();
    assert_eq!((home.already, home.into.len()), (6, 0));
    assert_eq!(all(second.library(), &there).len(), 6);

    // One document's file, brought into that document only.
    let one = first
        .library()
        .export_jsonld(ExchangeScope::Document(books.epub.document), NOW, "0.0.1")
        .unwrap();
    assert_eq!(one.annotations, 2);
    let third = Fixture::new();
    let elsewhere = shelf(&third, 1);
    let into_pdf = third
        .library()
        .import_jsonld(
            one.json.as_bytes(),
            ExchangeTarget::Document(elsewhere.pdf.document),
        )
        .unwrap();
    assert_eq!(
        (into_pdf.into.len(), into_pdf.elsewhere),
        (0, 2),
        "the EPUB's annotations are not the PDF's"
    );
    let into_work = third
        .library()
        .import_jsonld(
            one.json.as_bytes(),
            ExchangeTarget::Work(elsewhere.epub.work),
        )
        .unwrap();
    assert_eq!(
        into_work
            .into
            .iter()
            .map(|into| into.added.len())
            .sum::<usize>(),
        2
    );
}

#[test]
fn another_edition_takes_them_by_its_work_to_find_them_again() {
    let first = Fixture::new();
    let books = shelf(&first, 1);
    annotate(first.library(), &books);
    let export = first
        .library()
        .export_jsonld(ExchangeScope::Library, NOW, "0.0.1")
        .unwrap();

    // Only the EPUB, in another edition: other bytes, the same title, author and ISBN.
    let second = Fixture::new();
    let path = second.write(
        "outra edição.epub",
        &epub(
            "Dom Casmurro",
            &["Machado de Assis"],
            Some("urn:isbn:9788535902778"),
            99,
        ),
    );
    second.settle();
    let edition = second.identity(&path);
    let brought = second
        .library()
        .import_jsonld(export.json.as_bytes(), ExchangeTarget::Library)
        .unwrap();
    assert_eq!(brought.into.len(), 1);
    let into = &brought.into[0];
    assert_eq!(into.document_id, edition.document);
    assert_eq!(into.added.len(), 2);
    assert!(into.needs_anchoring);
    for annotation in &into.added {
        assert_eq!(
            annotation.revision_id, edition.revision,
            "made on a revision this library has"
        );
        assert_eq!(
            annotation.resolved_revision_id, None,
            "to be found again on this one"
        );
        assert_eq!(annotation.resolved_version, None);
        assert!(
            annotation
                .segments
                .iter()
                .flat_map(|segment| &segment.selectors)
                .all(|selector| selector.revision_id.is_none())
        );
        assert!(annotation.quote.is_some(), "the quote it is found again by");
    }
    // The PDF is not in this library: its four wait in the file, named.
    assert_eq!(brought.unmatched.len(), 1);
    assert_eq!(brought.unmatched[0].annotations, 4);

    // Into a book chosen by hand whose work is not the file's one work: taken, and said.
    let third = Fixture::new();
    let other = third.write(
        "outro.epub",
        &epub("Outro Livro", &["Outra Pessoa"], None, 5),
    );
    third.settle();
    let chosen = third.identity(&other);
    let one = first
        .library()
        .export_jsonld(ExchangeScope::Document(books.epub.document), NOW, "0.0.1")
        .unwrap();
    let taken = third
        .library()
        .import_jsonld(
            one.json.as_bytes(),
            ExchangeTarget::Document(chosen.document),
        )
        .unwrap();
    assert_eq!((taken.into.len(), taken.other_work), (1, 2));
}

#[test]
fn a_hostile_or_broken_file_is_refused_and_changes_nothing() {
    let first = Fixture::new();
    let books = shelf(&first, 1);
    annotate(first.library(), &books);
    let export = first
        .library()
        .export_jsonld(ExchangeScope::Library, NOW, "0.0.1")
        .unwrap();
    let good: Value = serde_json::from_str(&export.json).unwrap();

    let second = Fixture::new();
    let there = shelf(&second, 1);
    let edit = |change: &dyn Fn(&mut Value)| {
        let mut value = good.clone();
        change(&mut value);
        serde_json::to_vec(&value).unwrap()
    };
    let deep = format!("{}{}", "[".repeat(40), "]".repeat(40)).into_bytes();
    let duplicated = export
        .json
        .replacen("\"label\": ", "\"label\": \"x\", \"label\": ", 1)
        .into_bytes();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("not JSON", b"<xml/>".to_vec()),
        ("nested too deep", deep),
        (
            "a context to fetch",
            edit(&|v| v["@context"] = json!("https://evil.example/context.jsonld")),
        ),
        (
            "an @import",
            edit(&|v| v["@context"][1]["@import"] = json!("https://evil.example/x")),
        ),
        (
            "an unknown property",
            edit(&|v| v["first"]["items"][0]["evil"] = json!(true)),
        ),
        ("a property twice", duplicated),
        ("a total that lies", edit(&|v| v["total"] = json!(99))),
        (
            "not version 7",
            edit(&|v| {
                v["first"]["items"][2]["id"] =
                    json!("urn:uuid:6ba7b810-9dad-11d1-80b4-00c04fd430c8")
            }),
        ),
        (
            "a date that is none",
            edit(&|v| v["first"]["items"][1]["created"] = json!("ontem")),
        ),
        (
            "a source that is not a file",
            edit(&|v| {
                v["first"]["items"][1]["target"][0]["source"]["id"] =
                    json!("https://evil.example/book")
            }),
        ),
        (
            "a note too long, after good ones",
            edit(&|v| v["first"]["items"][5]["body"]["value"] = json!("x".repeat(2 << 20))),
        ),
        (
            "a color that is no color",
            edit(&|v| v["first"]["items"][4]["musaeum:color"] = json!("red; drop")),
        ),
        (
            "a newer version",
            edit(&|v| v["musaeum:formatVersion"] = json!(2)),
        ),
    ];
    for (what, bytes) in cases {
        let result = second
            .library()
            .import_jsonld(&bytes, ExchangeTarget::Library);
        assert!(result.is_err(), "{what}: {result:?}");
        assert!(
            all(second.library(), &there).is_empty(),
            "{what}: nothing written"
        );
    }
    // And the file itself still comes in.
    let fine = second
        .library()
        .import_jsonld(export.json.as_bytes(), ExchangeTarget::Library)
        .unwrap();
    assert_eq!(
        fine.into.iter().map(|into| into.added.len()).sum::<usize>(),
        6
    );
}
