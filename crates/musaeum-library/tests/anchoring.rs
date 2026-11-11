//! Anchoring on a real normalized book: a highlight across a normalizer cut, found again on the same
//! book, after the normalizer version changes, and on a revision of the book with a new paragraph
//! before it and another one rewritten.

use std::sync::Arc;

use gen_fixtures::flow;
use musaeum_flow::locator::LogicalDocument;
use musaeum_flow::normalize::{Options, normalize};
use musaeum_flow::package::{Manifest, NormalizedBook, SourceFormat};
use musaeum_flow::xml;
use musaeum_library::anchoring::flow::{self as anchoring, Documents, FlowPlace};
use musaeum_library::{AnchorState, SelectorKind};

struct Book {
    normalized: NormalizedBook,
    documents: Vec<Arc<LogicalDocument>>,
}

impl Book {
    fn of(bytes: &[u8]) -> Self {
        let normalized = normalize(SourceFormat::Epub, bytes, &Options::default()).unwrap();
        let documents = normalized
            .manifest
            .documents
            .iter()
            .enumerate()
            .map(|(index, document)| {
                let parts = document
                    .parts
                    .iter()
                    .map(|part| {
                        let text =
                            std::str::from_utf8(normalized.file(&part.href).unwrap()).unwrap();
                        (
                            part.start.clone(),
                            xml::parse(text, &xml::Limits::default()).unwrap(),
                        )
                    })
                    .collect();
                Arc::new(LogicalDocument::assemble(index, parts).unwrap())
            })
            .collect();
        Self {
            normalized,
            documents,
        }
    }

    fn text(&self, place: FlowPlace) -> String {
        anchoring::text_between(&self.documents[place.document], place.start, place.end)
            .unwrap()
            .to_owned()
    }
}

impl Documents for Book {
    fn manifest(&self) -> &Manifest {
        &self.normalized.manifest
    }

    fn document(&self, index: usize) -> Option<Arc<LogicalDocument>> {
        self.documents.get(index).cloned()
    }
}

/// The long chapter and where its second part starts, in UTF-16 code units of its text.
fn cut(book: &Book) -> (usize, u64) {
    let (index, document) = book
        .normalized
        .manifest
        .documents
        .iter()
        .enumerate()
        .find(|(_, document)| document.parts.len() > 1)
        .expect("the long chapter is cut");
    (index, document.parts[1].text_start)
}

#[test]
fn a_highlight_across_a_cut_is_one_segment_whose_ends_are_in_two_parts() {
    let book = Book::of(&flow::epub3(&flow::book(true)));
    let (index, at) = cut(&book);
    let whole = &book.documents[index];
    let (segment, quote) =
        anchoring::segment_for(0, book.manifest(), whole, at - 60, at + 60, 1).unwrap();
    assert!(quote.exact.chars().count() >= 100, "{:?}", quote.exact);

    let (start, end) = anchoring::reader_range(book.manifest(), whole, at - 60, at + 60).unwrap();
    let spine_step = |cfi: &str| cfi.split('!').next().unwrap().to_owned();
    assert_ne!(
        spine_step(&start),
        spine_step(&end),
        "the ends are in different parts: {start} {end}"
    );

    let place = anchoring::resolve(&segment, &book, 1).unwrap();
    assert_eq!(place.state, AnchorState::Exact);
    assert_eq!(
        (place.document, place.start, place.end),
        (index, at - 60, at + 60)
    );
    assert_eq!(
        musaeum_library::anchoring::text::collapse(&book.text(place)),
        quote.exact
    );
}

#[test]
fn another_normalizer_version_is_checked_against_the_quote_and_kept() {
    let book = Book::of(&flow::epub3(&flow::book(true)));
    let (index, at) = cut(&book);
    let (mut segment, _) = anchoring::segment_for(
        0,
        book.manifest(),
        &book.documents[index],
        at - 30,
        at + 30,
        1,
    )
    .unwrap();
    for selector in &mut segment.selectors {
        if selector.kind != SelectorKind::TextQuote {
            selector.normalizer_version = Some(0);
        }
    }
    let place = anchoring::resolve(&segment, &book, 1).unwrap();
    assert_eq!(
        (place.state, place.start, place.end),
        (AnchorState::Exact, at - 30, at + 30)
    );
}

#[test]
fn a_new_revision_moves_marks_finds_edits_and_keeps_what_it_lost() {
    let original = flow::book(true);
    let book = Book::of(&flow::epub3(&original));
    let manifest = book.manifest();

    // Three highlights in chapter 2: one on a sentence that stays, one that will be edited, and
    // one that will be gone.
    let chapter = manifest
        .documents
        .iter()
        .position(|document| document.href.ends_with("ch02.xhtml"))
        .unwrap();
    let whole = &book.documents[chapter];
    let paragraphs = &original.chapters[1].paragraphs;
    let units = |needle: &str| {
        let byte = whole
            .text
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} is not in chapter 2"));
        (whole.units_of(byte), whole.units_of(byte + needle.len()))
    };
    let kept_text = paragraphs[10].as_str();
    let edited_text = paragraphs[20].as_str();
    let lost_text = paragraphs[30].as_str();
    let make = |text: &str| {
        let (start, end) = units(text);
        anchoring::segment_for(0, manifest, whole, start, end, 1)
            .unwrap()
            .0
    };
    let (kept, edited, lost) = (make(kept_text), make(edited_text), make(lost_text));

    // The revision: a new paragraph at the start of the chapter, one word changed in the second
    // sentence, the third paragraph removed.
    // The same book again (its text comes from a fixed seed), then changed.
    let mut revised = flow::book(true);
    revised.chapters[1].paragraphs.insert(
        0,
        "Um parágrafo novo, posto antes de tudo nesta edição.".to_owned(),
    );
    let edit = &mut revised.chapters[1].paragraphs[21];
    let word = edit.split_whitespace().nth(3).unwrap().to_owned();
    *edit = edit.replacen(&word, "palavra", 1);
    revised.chapters[1].paragraphs.remove(31);
    let newer = Book::of(&flow::epub3(&revised));

    let found = anchoring::resolve(&kept, &newer, 2).unwrap();
    assert_eq!(found.state, AnchorState::Exact);
    assert_eq!(newer.text(found), kept_text);
    let found = anchoring::resolve(&edited, &newer, 2).unwrap();
    assert_eq!(found.state, AnchorState::Approximate);
    assert!(
        newer.text(found).contains("palavra"),
        "{}",
        newer.text(found)
    );
    assert_eq!(
        anchoring::resolve(&lost, &newer, 2),
        None,
        "an orphan, not a guess"
    );
}
