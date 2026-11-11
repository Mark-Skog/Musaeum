//! Anchoring in flowing books: EPUB, MOBI, AZW3, FB2, plain text and Markdown, as normalized.
//!
//! A segment is a stretch of one of the book's own documents, whatever parts the normalizer cut it
//! into: a selection across a cut is one segment, and one across chapters is one per chapter. It
//! keeps three selectors, tried in this order:
//!
//! 1. the canonical CFIs of where it starts and ends (`cfi`), which count in the book's documents
//!    and are the CFIs any reader of the EPUB would write;
//! 2. where it starts and ends in the document's normalized text, in UTF-16 code units
//!    (`text_position`), which survives a normalizer that changes the tree but not the text;
//! 3. the quote with what comes before and after it (`text_quote`), in its document first and then
//!    in the others.
//!
//! Each candidate is checked against the quote. Selectors found again are worked out anew for the
//! normalizer version and revision now, so the next opening takes the first step.

use std::sync::Arc;

use musaeum_flow::NORMALIZER_VERSION;
use musaeum_flow::cfi;
use musaeum_flow::locator::{self, LogicalDocument};
use musaeum_flow::package::Manifest;
use serde_json::{Value, json};

use super::fixed::{quote_of, quote_selector};
use super::text::{self, CONTEXT_CHARS, Folded, Quality, Quote, collapse, fold};
use crate::annotations::{AnchorState, Segment, Selector, SelectorKind};
use crate::model::RevisionId;

/// The book as it is now.
pub trait Documents {
    fn manifest(&self) -> &Manifest;
    /// One of its documents, whole.
    fn document(&self, index: usize) -> Option<Arc<LogicalDocument>>;
}

/// Where a segment is now: a document, and UTF-16 code units into its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlowPlace {
    pub document: usize,
    pub start: u64,
    pub end: u64,
    pub state: AnchorState,
}

/// The text of a document between two offsets.
pub fn text_between(document: &LogicalDocument, start: u64, end: u64) -> Option<&str> {
    let from = document.byte_of(start)?;
    let to = document.byte_of(end)?;
    document.text.get(from..to.max(from))
}

/// What a stretch of a document quotes, with what comes before and after it.
pub fn quote_at(document: &LogicalDocument, start: u64, end: u64) -> Option<Quote> {
    let from = document.byte_of(start)?;
    let to = document.byte_of(end)?;
    let text = &document.text;
    let before_from = text[..from]
        .char_indices()
        .rev()
        .nth(CONTEXT_CHARS * 4)
        .map_or(0, |(at, _)| at);
    let after_to = text[to..]
        .char_indices()
        .nth(CONTEXT_CHARS * 4)
        .map_or(text.len(), |(at, _)| to + at);
    Some(Quote {
        exact: collapse(&text[from..to]),
        prefix: text::tail_chars(&collapse(&text[before_from..from]), CONTEXT_CHARS),
        suffix: text::head_chars(&collapse(&text[to..after_to]), CONTEXT_CHARS),
    })
}

/// The canonical CFIs of a stretch of a document.
pub fn canonical_range(
    manifest: &Manifest,
    document: &LogicalDocument,
    start: u64,
    end: u64,
) -> Option<(String, String)> {
    let from = document.point_at(start)?;
    let to = document.end_point_at(end)?;
    Some((
        locator::canonical_cfi(manifest, document, from),
        locator::canonical_cfi(manifest, document, to),
    ))
}

/// The reader's CFIs of a stretch of a document, each in the part that holds it.
pub fn reader_range(
    manifest: &Manifest,
    document: &LogicalDocument,
    start: u64,
    end: u64,
) -> Option<(String, String)> {
    let (from, to) = canonical_range(manifest, document, start, end)?;
    Some((
        locator::canonical_to_reader(manifest, &from).ok()?,
        locator::canonical_to_reader(manifest, &to).ok()?,
    ))
}

/// The selectors that say where a stretch is with this normalizer, the quote aside.
pub fn place_selectors(
    manifest: &Manifest,
    document: &LogicalDocument,
    start: u64,
    end: u64,
    revision: RevisionId,
) -> Option<Vec<Selector>> {
    let (from, to) = canonical_range(manifest, document, start, end)?;
    Some(vec![
        Selector {
            kind: SelectorKind::Cfi,
            data: json!({"start": from, "end": to}),
            priority: 0,
            normalizer_version: Some(NORMALIZER_VERSION),
            revision_id: Some(revision),
        },
        Selector {
            kind: SelectorKind::TextPosition,
            data: json!({"start": start, "end": end}),
            priority: 1,
            normalizer_version: Some(NORMALIZER_VERSION),
            revision_id: Some(revision),
        },
    ])
}

/// A new segment for a stretch the reader selected, with the quote it makes.
pub fn segment_for(
    ordinal: u32,
    manifest: &Manifest,
    document: &LogicalDocument,
    start: u64,
    end: u64,
    revision: RevisionId,
) -> Option<(Segment, Quote)> {
    let quote = quote_at(document, start, end)?;
    let mut selectors = place_selectors(manifest, document, start, end, revision)?;
    selectors.push(quote_selector(&quote));
    Some((
        Segment {
            ordinal,
            page: None,
            href: Some(manifest.documents.get(document.index)?.href.clone()),
            clip: None,
            anchor_state: AnchorState::Exact,
            selectors,
        },
        quote,
    ))
}

fn selector(segment: &Segment, kind: SelectorKind) -> Option<&Selector> {
    segment
        .selectors
        .iter()
        .find(|selector| selector.kind == kind)
}

/// Where a canonical CFI is: its document and the offset into its text.
fn offset_of(documents: &dyn Documents, canonical: &str) -> Option<(usize, u64)> {
    let parsed = cfi::parse(canonical).ok()?;
    let index = locator::document_of(documents.manifest(), &parsed).ok()?;
    let document = documents.document(index)?;
    let (point, _) = cfi::resolve(&document.dom, &parsed.path, parsed.offset).ok()?;
    Some((index, document.text_offset(point).ok()?))
}

/// Whether a stretch of a document says what the quote says. With no quote to compare, only
/// selectors from the same bytes and the same normalizer are trusted.
fn holds(
    document: &LogicalDocument,
    start: u64,
    end: u64,
    quote: Option<&Quote>,
    trusted: bool,
) -> bool {
    if start >= end || end > document.text_length {
        return false;
    }
    match quote {
        Some(quote) => {
            text_between(document, start, end).is_some_and(|text| fold(text) == fold(&quote.exact))
        }
        None => trusted,
    }
}

fn find_in(
    document: &LogicalDocument,
    quote: &Quote,
    hint: Option<u64>,
    edits: bool,
) -> Option<(u64, u64, Quality)> {
    let folded = Folded::new(&document.text);
    let hint = hint.and_then(|units| document.byte_of(units));
    let (from, to, quality) = folded.find(quote, hint)?;
    if !edits && fold(&document.text[from..to]) != fold(&quote.exact) {
        return None;
    }
    Some((document.units_of(from), document.units_of(to), quality))
}

fn number(data: &Value, key: &str) -> Option<u64> {
    data.get(key)?.as_u64()
}

/// Where a segment is in the book as it is now, if anywhere. `revision` is the revision the book is
/// now; selectors worked out on it, with this normalizer, are taken at their word.
pub fn resolve(
    segment: &Segment,
    documents: &dyn Documents,
    revision: RevisionId,
) -> Option<FlowPlace> {
    let quote = quote_of(segment);
    let manifest = documents.manifest();
    let current = |selector: &Selector| {
        selector.revision_id == Some(revision)
            && selector.normalizer_version == Some(NORMALIZER_VERSION)
    };
    let exact = |document: usize, start: u64, end: u64| FlowPlace {
        document,
        start,
        end,
        state: AnchorState::Exact,
    };

    // 1. The canonical CFIs.
    if let Some(selector) = selector(segment, SelectorKind::Cfi)
        && let (Some(from), Some(to)) = (
            selector.data.get("start").and_then(Value::as_str),
            selector.data.get("end").and_then(Value::as_str),
        )
        && let (Some((document, start)), Some((other, end))) =
            (offset_of(documents, from), offset_of(documents, to))
        && document == other
        && let Some(whole) = documents.document(document)
        && holds(&whole, start, end, quote.as_ref(), current(selector))
    {
        return Some(exact(document, start, end));
    }

    // 2. The position in the normalized text of its document.
    let home = segment.href.as_ref().and_then(|href| {
        manifest
            .documents
            .iter()
            .position(|document| &document.href == href)
    });
    let position = selector(segment, SelectorKind::TextPosition).and_then(|selector| {
        Some((
            number(&selector.data, "start")?,
            number(&selector.data, "end")?,
            current(selector),
        ))
    });
    if let (Some(index), Some((start, end, trusted))) = (home, position)
        && let Some(whole) = documents.document(index)
        && holds(&whole, start, end, quote.as_ref(), trusted)
    {
        return Some(exact(index, start, end));
    }

    // 3. The quote: in its document, then in the others, nearest first; as it was before edited.
    let quote = quote?;
    let count = manifest.documents.len();
    let home = home.unwrap_or(0).min(count.saturating_sub(1));
    let mut order = vec![home];
    for step in 1..count {
        if home + step < count {
            order.push(home + step);
        }
        if let Some(before) = home.checked_sub(step) {
            order.push(before);
        }
    }
    let hint = position.map(|(start, ..)| start);
    for &index in &order {
        let Some(whole) = documents.document(index) else {
            continue;
        };
        let hint = (index == home).then_some(hint).flatten();
        if let Some((start, end, quality)) = find_in(&whole, &quote, hint, false) {
            return Some(FlowPlace {
                document: index,
                start,
                end,
                state: match quality {
                    Quality::Exact => AnchorState::Exact,
                    Quality::Approximate => AnchorState::Approximate,
                },
            });
        }
    }
    // Edited, only in its document and the ones beside it: further away, a near match is a guess.
    for &index in order.iter().take(3) {
        let Some(whole) = documents.document(index) else {
            continue;
        };
        let hint = (index == home).then_some(hint).flatten();
        if let Some((start, end, _)) = find_in(&whole, &quote, hint, true) {
            return Some(FlowPlace {
                document: index,
                start,
                end,
                state: AnchorState::Approximate,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use musaeum_flow::locator::LogicalDocument;
    use musaeum_flow::package::{
        Document, Manifest, Metadata, PartInfo, Source, SourceFormat, Stats,
    };
    use musaeum_flow::xml;

    use super::*;

    fn dom(body: &str) -> musaeum_flow::dom::Dom {
        let text = format!(
            "<?xml version=\"1.0\"?><html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>t</title></head><body>{body}</body></html>"
        );
        xml::parse(&text, &xml::Limits::default()).unwrap()
    }

    struct Book {
        manifest: Manifest,
        documents: Vec<Arc<LogicalDocument>>,
    }

    impl Documents for Book {
        fn manifest(&self) -> &Manifest {
            &self.manifest
        }
        fn document(&self, index: usize) -> Option<Arc<LogicalDocument>> {
            self.documents.get(index).cloned()
        }
    }

    fn book(chapters: &[&str]) -> Book {
        let mut manifest = Manifest {
            normalizer: NORMALIZER_VERSION,
            source: Source {
                format: SourceFormat::Epub,
                version: None,
                fixed_layout: false,
                direction: None,
            },
            metadata: Metadata::default(),
            documents: Vec::new(),
            resources: Vec::new(),
            toc: Vec::new(),
            page_list: Vec::new(),
            landmarks: Vec::new(),
            stats: Stats::default(),
            warnings: Vec::new(),
        };
        let mut documents = Vec::new();
        for (index, body) in chapters.iter().enumerate() {
            let parsed = dom(body);
            let whole = LogicalDocument::new(index, parsed);
            manifest.documents.push(Document {
                href: format!("OEBPS/chapter{index}.xhtml"),
                id: Some(format!("c{index}")),
                body_step: Some(4),
                parts: vec![PartInfo {
                    href: format!("p{index}.xhtml"),
                    start: Vec::new(),
                    text_length: whole.text_length,
                    ..PartInfo::default()
                }],
                text_length: whole.text_length,
                linear: true,
                ..Document::default()
            });
            documents.push(Arc::new(whole));
        }
        Book {
            manifest,
            documents,
        }
    }

    const ONE: &str =
        "<p>Capitu tinha olhos de ressaca.</p><p>Olhos de cigana oblíqua e dissimulada.</p>";
    const TWO: &str =
        "<p>Nada se emenda bem nos livros confusos, mas tudo se pode meter nos livros omissos.</p>";

    fn marked(book: &Book, document: usize, needle: &str, revision: RevisionId) -> Segment {
        let whole = &book.documents[document];
        let byte = whole.text.find(needle).unwrap();
        let start = whole.units_of(byte);
        let end = whole.units_of(byte + needle.len());
        segment_for(0, &book.manifest, whole, start, end, revision)
            .unwrap()
            .0
    }

    fn text_of(book: &Book, place: FlowPlace) -> String {
        text_between(&book.documents[place.document], place.start, place.end)
            .unwrap()
            .to_owned()
    }

    #[test]
    fn the_cfi_finds_it_on_the_same_book() {
        let original = book(&[ONE, TWO]);
        let segment = marked(&original, 0, "cigana oblíqua", 3);
        let cfi = &segment.selectors[0].data;
        assert!(
            cfi["start"]
                .as_str()
                .unwrap()
                .starts_with("epubcfi(/6/2[c0]!/4/4/1:"),
            "{cfi}"
        );
        let place = resolve(&segment, &original, 3).unwrap();
        assert_eq!(place.state, AnchorState::Exact);
        assert_eq!(text_of(&original, place), "cigana oblíqua");
        let (start, end) = reader_range(
            &original.manifest,
            &original.documents[0],
            place.start,
            place.end,
        )
        .unwrap();
        assert!(start.starts_with("epubcfi(/6/2!/4/4/1:"), "{start}");
        assert!(end.ends_with(":23)"), "{end}");
    }

    #[test]
    fn a_new_paragraph_before_it_breaks_the_cfi_and_the_quote_finds_it() {
        let original = book(&[ONE, TWO]);
        let segment = marked(&original, 0, "cigana oblíqua", 3);
        let revised = book(&[
            "<p>Prefácio.</p><p>Capitu tinha olhos de ressaca.</p><p>Olhos de cigana oblíqua e dissimulada.</p>",
            TWO,
        ]);
        let place = resolve(&segment, &revised, 4).unwrap();
        assert_eq!(place.state, AnchorState::Exact);
        assert_eq!(text_of(&revised, place), "cigana oblíqua");
    }

    #[test]
    fn a_changed_tree_with_the_same_text_is_found_by_position() {
        let original = book(&[ONE, TWO]);
        let segment = marked(&original, 1, "livros confusos", 3);
        // A normalizer that wraps the paragraph: the CFI points into the wrapper, the text is the same.
        let rewrapped = book(&[ONE, &format!("<div>{TWO}</div>")]);
        let place = resolve(&segment, &rewrapped, 3).unwrap();
        assert_eq!(place.state, AnchorState::Exact);
        assert_eq!(text_of(&rewrapped, place), "livros confusos");
    }

    #[test]
    fn chapters_reordered_and_lightly_edited() {
        let original = book(&[ONE, TWO]);
        let moved = marked(&original, 1, "tudo se pode meter nos livros omissos", 3);
        let edited = marked(&original, 0, "Olhos de cigana oblíqua e dissimulada", 3);
        let revised = book(&[
            TWO,
            "<p>Capitu tinha olhos de ressaca.</p><p>Olhos de cigana obliqua, e dissimulada.</p>",
        ]);
        let place = resolve(&moved, &revised, 4).unwrap();
        assert_eq!((place.document, place.state), (0, AnchorState::Exact));
        assert_eq!(
            text_of(&revised, place),
            "tudo se pode meter nos livros omissos"
        );
        let place = resolve(&edited, &revised, 4).unwrap();
        assert_eq!((place.document, place.state), (1, AnchorState::Approximate));
        assert_eq!(
            text_of(&revised, place),
            "Olhos de cigana obliqua, e dissimulada"
        );
    }

    #[test]
    fn a_removed_chapter_leaves_an_orphan() {
        let original = book(&[ONE, TWO]);
        let segment = marked(&original, 1, "livros omissos", 3);
        assert_eq!(resolve(&segment, &book(&[ONE]), 4), None);
    }
}
