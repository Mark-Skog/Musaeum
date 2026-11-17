//! Annotations in the reader: made from a selection, found again on the document as it is now, and
//! listed, edited and searched by the inspector.
//!
//! Every write goes to the library's one writer; the page shows a change at once and puts it back if
//! the writer refuses it. Finding annotations again happens here, in Rust, against what the workers
//! read (a fixed layout's characters) or the normalized package (a flowing book), and what was found
//! is written back so the next opening starts from it.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use musaeum_core::FixedFormat;
use musaeum_flow::NORMALIZER_VERSION;
use musaeum_flow::cfi;
use musaeum_flow::locator;
use musaeum_library::anchoring::region::{self, Region, RegionLine, RegionTool};
use musaeum_library::anchoring::{self, fixed, flow as flow_anchor};
use musaeum_library::{
    AnchorState, Annotation, AnnotationEdit, AnnotationHit, AnnotationKind, Library, NewAnnotation,
    QuoteSource, Reanchoring, Segment, SegmentUpdate, SelectorKind,
};
use musaeum_shm::chars::{chars_in_quads, line_quads};
use musaeum_shm::{CHARS_VERSION, CharLayer};
use musaeum_vision::LINES_VERSION;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::ipc::Response;
use tauri::{AppHandle, State};

use crate::book::{self, BookState};
use crate::flow::{FlowBook, on_large_stack};
use crate::session::{AppState, Session};

/// How long a page's characters may take to come from the worker.
const CHARS_TIMEOUT: Duration = Duration::from_secs(20);
/// Pages whose characters are kept, for every open document together.
const KEPT_PAGES: usize = 96;
/// The most pages or documents one selection can cover.
const MAX_SPANS: usize = 64;
/// The longest search query, in bytes.
const MAX_QUERY: usize = 1024;

/// A page's characters, kept by session and page.
type KeptPage = ((u32, u32), Arc<CharLayer>);

/// Character layers already read, by session and page.
#[derive(Default)]
pub struct CharCache(Mutex<VecDeque<KeptPage>>);

impl CharCache {
    /// A page's characters, read by the pool the first time they are asked for.
    pub fn get(&self, session: &Session, page: u32) -> Option<Arc<CharLayer>> {
        let key = (session.id, page);
        if let Some((_, layer)) = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|(kept, _)| *kept == key)
        {
            return Some(Arc::clone(layer));
        }
        let layer = match session.pool.request_chars(page).recv_timeout(CHARS_TIMEOUT) {
            Ok(Ok(layer)) => Arc::new(layer),
            Ok(Err(error)) => {
                eprintln!("page {page}'s characters: {error}");
                return None;
            }
            Err(_) => return None,
        };
        let mut kept = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if kept.len() >= KEPT_PAGES {
            kept.pop_front();
        }
        kept.push_back((key, Arc::clone(&layer)));
        Some(layer)
    }
}

struct SessionPages<'a> {
    session: &'a Session,
    cache: &'a CharCache,
}

impl fixed::Pages for SessionPages<'_> {
    fn count(&self) -> u32 {
        self.session.pool.pages().len() as u32
    }

    fn layer(&mut self, page: u32) -> Option<Arc<CharLayer>> {
        self.cache.get(self.session, page)
    }
}

/// A mark on one page, as the compositor draws it and the selection meets it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageMark {
    page: u32,
    start: usize,
    end: usize,
    quads: Vec<[f32; 8]>,
}

/// An annotation of a fixed layout, with where it is on the pages now.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixedAnnotationView {
    annotation: Annotation,
    marks: Vec<PageMark>,
}

/// A stretch of a flowing book, as the reader's CFIs of where it starts and ends.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowRange {
    start: String,
    end: String,
}

/// An annotation of a flowing book, with where it is in the book now.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowAnnotationView {
    annotation: Annotation,
    ranges: Vec<FlowRange>,
}

/// What a person chose for a new annotation.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkArgs {
    kind: AnnotationKind,
    color: String,
    #[serde(default)]
    note: String,
}

/// Characters `start..end` of a page, as the reader selected them.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageSpan {
    page: u32,
    start: usize,
    end: usize,
}

fn library(state: &AppState) -> Result<Arc<Library>, String> {
    state
        .shelf
        .library()
        .ok_or_else(|| "there is no library open".to_owned())
}

fn selector_data(segment: &Segment, kind: SelectorKind) -> Option<&Value> {
    segment
        .selectors
        .iter()
        .find(|selector| selector.kind == kind)
        .map(|selector| &selector.data)
}

fn quads_of(data: &Value) -> Vec<[f32; 8]> {
    data.get("quads")
        .and_then(Value::as_array)
        .map(|quads| {
            quads
                .iter()
                .filter_map(|quad| {
                    let values: Vec<f32> = quad
                        .as_array()?
                        .iter()
                        .filter_map(|value| value.as_f64().map(|v| v as f32))
                        .collect();
                    values.try_into().ok()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn region_mark(region: Region) -> PageMark {
    PageMark {
        page: region.page,
        start: 0,
        end: 0,
        quads: region.quads,
    }
}

/// The hash of a page of the open document's picture, as the line detector's model keeps it.
fn page_hash_of(library: &Library, session: &Session, page: u32) -> Option<u64> {
    let blake3 = session.blake3?;
    match library.line_model(&blake3, page, LINES_VERSION) {
        Ok(record) => record.map(|record| record.page_hash),
        Err(error) => {
            eprintln!("page {page}'s lines: {error}");
            None
        }
    }
}

/// Where a fixed annotation's segments are on `revision`, worked out again if they were found on
/// another revision or text layer, and what to record if they were. A mark on the picture of a
/// page is checked against `page_hash`, the hash of that page now; while a page it needs has
/// none yet, the annotation shows where it was and nothing is recorded, so it is checked again
/// next time.
fn anchor_fixed(
    annotation: &Annotation,
    pages: &mut dyn fixed::Pages,
    revision: i64,
    page_hash: &dyn Fn(u32) -> Option<u64>,
) -> (Vec<PageMark>, Option<Reanchoring>) {
    if annotation.segments.is_empty() {
        return (Vec::new(), None);
    }
    let current = annotation.resolved_revision_id == Some(revision)
        && annotation.resolved_version == Some(CHARS_VERSION);
    if current {
        let marks = annotation
            .segments
            .iter()
            .filter(|segment| segment.anchor_state != AnchorState::Orphan)
            .filter_map(|segment| {
                if let Some(found) = region::region_of(segment) {
                    return Some(region_mark(found));
                }
                let range = selector_data(segment, SelectorKind::PageCharRange)?;
                let quads = selector_data(segment, SelectorKind::PageQuads).map(quads_of)?;
                Some(PageMark {
                    page: u32::try_from(range.get("page")?.as_u64()?).ok()?,
                    start: usize::try_from(range.get("start")?.as_u64()?).ok()?,
                    end: usize::try_from(range.get("end")?.as_u64()?).ok()?,
                    quads,
                })
            })
            .collect();
        return (marks, None);
    }
    let count = pages.count().max(1);
    let mut marks = Vec::new();
    let mut updates = Vec::new();
    let mut undecided = false;
    for segment in &annotation.segments {
        if let Some(found) = region::region_of(segment) {
            let now = page_hash(found.page);
            match region::resolve(found, pages.count(), now) {
                region::Resolution::Found(found, state) => {
                    updates.push(SegmentUpdate {
                        ordinal: segment.ordinal,
                        page: Some(found.page),
                        href: None,
                        anchor_state: state,
                        selectors: vec![found.selector(revision, LINES_VERSION)],
                    });
                    marks.push(region_mark(found));
                }
                region::Resolution::Orphan => updates.push(SegmentUpdate {
                    ordinal: segment.ordinal,
                    page: segment.page,
                    href: None,
                    anchor_state: AnchorState::Orphan,
                    selectors: Vec::new(),
                }),
                region::Resolution::Undecided(found) => {
                    undecided = true;
                    marks.push(region_mark(found));
                }
            }
            continue;
        }
        match fixed::resolve(segment, pages, revision) {
            Some(place) => {
                updates.push(SegmentUpdate {
                    ordinal: segment.ordinal,
                    page: Some(place.range.page),
                    href: None,
                    anchor_state: place.state,
                    selectors: fixed::place_selectors(place.range, &place.quads, revision),
                });
                marks.push(PageMark {
                    page: place.range.page,
                    start: place.range.start,
                    end: place.range.end,
                    quads: place.quads,
                });
            }
            None => updates.push(SegmentUpdate {
                ordinal: segment.ordinal,
                page: segment.page,
                href: None,
                anchor_state: AnchorState::Orphan,
                selectors: Vec::new(),
            }),
        }
    }
    if undecided {
        return (marks, None);
    }
    let progression = marks
        .first()
        .map(|mark| fixed_progression(mark.page, &mark.quads, count));
    let state = anchoring::overall(updates.iter().map(|update| update.anchor_state));
    (
        marks,
        Some(Reanchoring {
            uuid: annotation.uuid.clone(),
            anchor_state: state,
            progression,
            revision_id: revision,
            version: Some(CHARS_VERSION),
            segments: updates,
        }),
    )
}

/// How far into the document a mark on a page starts.
fn fixed_progression(page: u32, quads: &[[f32; 8]], count: u32) -> f64 {
    let top = quads
        .first()
        .map_or(0.0, |quad| f64::from(quad[1]).clamp(0.0, 1.0));
    ((f64::from(page) + top) / f64::from(count.max(1))).clamp(0.0, 1.0)
}

/// Writes what re-anchoring found. A failure is logged: the annotations stay as they were and are
/// found again next time.
fn record(library: &Library, outcomes: Vec<Reanchoring>) {
    if outcomes.is_empty() {
        return;
    }
    if let Err(error) = library.record_anchoring(outcomes) {
        eprintln!("what re-anchoring found could not be kept: {error}");
    }
}

/// The open fixed document's annotations, with their marks on the pages as they are now. Marks on
/// pictures that quote nothing yet are read in the background.
#[tauri::command(async)]
pub fn fixed_annotations(
    app: AppHandle,
    state: State<'_, AppState>,
    cache: State<'_, CharCache>,
    session: u32,
) -> Result<Vec<FixedAnnotationView>, String> {
    let session = state
        .session(session)
        .ok_or("that document is no longer open")?;
    let Some((document, revision)) = session.document else {
        return Ok(Vec::new());
    };
    let library = library(&state)?;
    let annotations = library
        .annotations(document, false)
        .map_err(|error| error.to_string())?;
    let mut pages = SessionPages {
        session: &session,
        cache: &cache,
    };
    let mut views = Vec::with_capacity(annotations.len());
    let mut outcomes = Vec::new();
    let page_hash = |page: u32| page_hash_of(&library, &session, page);
    for mut annotation in annotations {
        let (marks, outcome) = anchor_fixed(&annotation, &mut pages, revision, &page_hash);
        if let Some(outcome) = outcome {
            annotation.anchor_state = outcome.anchor_state;
            annotation.progression = outcome.progression.or(annotation.progression);
            outcomes.push(outcome);
        }
        if awaits_quote(&annotation) {
            state
                .ocr
                .enqueue(&app, Arc::clone(&session), annotation.uuid.clone());
        }
        views.push(FixedAnnotationView { annotation, marks });
    }
    record(&library, outcomes);
    Ok(views)
}

/// A page's characters and their quads, as the worker read them, for the compositor to select and
/// hit-test with. The bytes are the layout of `musaeum_shm::chars`.
#[tauri::command(async)]
pub fn page_chars(
    state: State<'_, AppState>,
    cache: State<'_, CharCache>,
    session: u32,
    page: u32,
) -> Result<Response, String> {
    let session = state
        .session(session)
        .ok_or("that document is no longer open")?;
    if page as usize >= session.pool.pages().len() {
        return Err(format!("there is no page {page}"));
    }
    let layer = cache
        .get(&session, page)
        .ok_or_else(|| format!("page {page}'s characters could not be read"))?;
    let mut bytes = vec![0u8; layer.encoded_len()];
    layer
        .encode_into(&mut bytes)
        .ok_or("the characters do not fit")?;
    Ok(Response::new(bytes))
}

/// Makes an annotation from characters selected on the open fixed document.
#[tauri::command(async)]
pub fn create_fixed_annotation(
    state: State<'_, AppState>,
    cache: State<'_, CharCache>,
    session: u32,
    spans: Vec<PageSpan>,
    mark: MarkArgs,
) -> Result<FixedAnnotationView, String> {
    let session = state
        .session(session)
        .ok_or("that document is no longer open")?;
    let (document, revision) = session
        .document
        .ok_or("este documento não está na biblioteca, e só os da biblioteca guardam anotações")?;
    if spans.is_empty() || spans.len() > MAX_SPANS {
        return Err("a selection covers from one page to a few dozen".to_owned());
    }
    let count = session.pool.pages().len() as u32;
    let mut segments = Vec::with_capacity(spans.len());
    let mut quotes = Vec::with_capacity(spans.len());
    let mut marks = Vec::with_capacity(spans.len());
    for (ordinal, span) in (0u32..).zip(&spans) {
        if span.page >= count || span.start >= span.end {
            return Err(format!("page {} has no such characters", span.page));
        }
        let layer = cache
            .get(&session, span.page)
            .ok_or_else(|| format!("page {}'s characters could not be read", span.page))?;
        if span.end > layer.chars.len() {
            return Err(format!("page {} has no such characters", span.page));
        }
        let range = fixed::PageRange {
            page: span.page,
            start: span.start,
            end: span.end,
        };
        let (segment, quote) = fixed::segment_for(ordinal, range, &layer, revision);
        marks.push(PageMark {
            page: span.page,
            start: span.start,
            end: span.end,
            quads: line_quads(&layer.chars, span.start, span.end),
        });
        quotes.push(quote.exact);
        segments.push(segment);
    }
    let progression = marks
        .first()
        .map(|mark| fixed_progression(mark.page, &mark.quads, count));
    let annotation = library(&state)?
        .create_annotation(NewAnnotation {
            document_id: document,
            revision_id: revision,
            kind: mark.kind,
            color: mark.color,
            note: mark.note,
            quote: Some(quotes.join(" ")),
            // DjVu's text is what OCR read off the scan.
            quote_source: if matches!(session.format, FixedFormat::Djvu) {
                QuoteSource::Ocr
            } else {
                QuoteSource::TextLayer
            },
            progression,
            anchor_state: AnchorState::Exact,
            resolved_version: Some(CHARS_VERSION),
            pdf_nm: None,
            segments,
        })
        .map_err(|error| error.to_string())?;
    Ok(FixedAnnotationView { annotation, marks })
}

/// A mark on the picture of a page, as the compositor drew it.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionArgs {
    page: u32,
    tool: RegionTool,
    quads: Vec<[f32; 8]>,
    clip: [f32; 4],
    #[serde(default)]
    lines: Vec<RegionLine>,
    #[serde(default)]
    path: Vec<f32>,
    #[serde(default)]
    region_hash: Option<String>,
}

/// A region drawn on a page of the open document, checked, with the hash of the page's picture
/// when the background has traced it.
pub fn found_region(
    library: Option<&Library>,
    session: &Session,
    region: RegionArgs,
) -> Result<Region, String> {
    if region.page as usize >= session.pool.pages().len() {
        return Err(format!("there is no page {}", region.page));
    }
    let found = Region {
        page: region.page,
        tool: region.tool,
        quads: region.quads,
        clip: region.clip,
        lines: region.lines,
        path: region.path,
        page_hash: library
            .and_then(|library| page_hash_of(library, session, region.page))
            .map(region::hash_text),
        region_hash: region.region_hash,
    };
    match found.problem() {
        Some(problem) => Err(problem),
        None => Ok(found),
    }
}

/// Whether an annotation is a mark on a picture that quotes nothing yet.
fn awaits_quote(annotation: &Annotation) -> bool {
    annotation.quote.is_none()
        && annotation.deleted_at.is_none()
        && annotation
            .segments
            .first()
            .is_some_and(|segment| selector_data(segment, SelectorKind::PageRegion).is_some())
}

/// Makes an annotation from a mark on the picture of a page of the open fixed document: along the
/// lines the detector found, a box, or a stroke. The page's hash comes from its line model, when
/// the background has traced it. Its quote, what search finds it by, is the text under it: on a
/// page that is no picture, its characters; on a scan, with or without a text layer over it, what
/// the OCR reads on the lines under it, afterwards, in the background (see [`crate::ocr`]).
#[tauri::command(async)]
pub fn create_region_annotation(
    app: AppHandle,
    state: State<'_, AppState>,
    cache: State<'_, CharCache>,
    session: u32,
    region: RegionArgs,
    mark: MarkArgs,
) -> Result<FixedAnnotationView, String> {
    let session = state
        .session(session)
        .ok_or("that document is no longer open")?;
    let (document, revision) = session
        .document
        .ok_or("este documento não está na biblioteca, e só os da biblioteca guardam anotações")?;
    let count = session.pool.pages().len() as u32;
    let library = library(&state)?;
    let found = found_region(Some(&library), &session, region)?;
    // On a picture, the OCR reads the mark, in the background; the text layer of a scan stands
    // in only when it has nothing (`crate::ocr::region_reading`).
    let picture = crate::ocr::picture_page(&state, &session, found.page) == Some(true);
    let quote = cache
        .get(&session, found.page)
        .filter(|_| !picture)
        .and_then(|layer| {
            let (start, end) = chars_in_quads(&layer.chars, &found.quads)?;
            let quote = fixed::quote_at(&layer, start, end).exact;
            (!quote.trim().is_empty()).then_some(quote)
        });
    let quote_source = match (&quote, session.format) {
        (None, _) => QuoteSource::None,
        (Some(_), FixedFormat::Djvu) => QuoteSource::Ocr,
        (Some(_), _) => QuoteSource::TextLayer,
    };
    let progression = fixed_progression(found.page, &found.quads, count);
    let segment = found.segment(0, revision, LINES_VERSION);
    let annotation = library
        .create_annotation(NewAnnotation {
            document_id: document,
            revision_id: revision,
            kind: mark.kind,
            color: mark.color,
            note: mark.note,
            quote,
            quote_source,
            progression: Some(progression),
            anchor_state: AnchorState::Exact,
            resolved_version: Some(CHARS_VERSION),
            pdf_nm: None,
            segments: vec![segment],
        })
        .map_err(|error| error.to_string())?;
    if awaits_quote(&annotation) {
        state
            .ocr
            .enqueue(&app, Arc::clone(&session), annotation.uuid.clone());
    }
    Ok(FixedAnnotationView {
        annotation,
        marks: vec![region_mark(found)],
    })
}

/// Where a flowing book's annotation is now, worked out again if it was found on another revision or
/// normalizer version, and what to record if it was.
fn anchor_flow(
    annotation: &Annotation,
    book: &FlowBook,
    revision: i64,
) -> (Vec<FlowRange>, Option<Reanchoring>) {
    if annotation.segments.is_empty() {
        return (Vec::new(), None);
    }
    let manifest = &book.manifest;
    let current = annotation.resolved_revision_id == Some(revision)
        && annotation.resolved_version == Some(NORMALIZER_VERSION);
    if current {
        let ranges = annotation
            .segments
            .iter()
            .filter(|segment| segment.anchor_state != AnchorState::Orphan)
            .filter_map(|segment| {
                let cfi = selector_data(segment, SelectorKind::Cfi)?;
                Some(FlowRange {
                    start: locator::canonical_to_reader(manifest, cfi.get("start")?.as_str()?)
                        .ok()?,
                    end: locator::canonical_to_reader(manifest, cfi.get("end")?.as_str()?).ok()?,
                })
            })
            .collect();
        return (ranges, None);
    }
    let total = manifest.total_text().max(1);
    let mut ranges = Vec::new();
    let mut updates = Vec::new();
    let mut progression = None;
    for segment in &annotation.segments {
        let found = flow_anchor::resolve(segment, book, revision).and_then(|place| {
            let document = book.document(place.document).ok()?;
            let (start, end) =
                flow_anchor::reader_range(manifest, &document, place.start, place.end)?;
            let selectors = flow_anchor::place_selectors(
                manifest,
                &document,
                place.start,
                place.end,
                revision,
            )?;
            Some((place, FlowRange { start, end }, selectors))
        });
        match found {
            Some((place, range, selectors)) => {
                progression.get_or_insert(
                    ((manifest.text_before(place.document) + place.start) as f64 / total as f64)
                        .clamp(0.0, 1.0),
                );
                updates.push(SegmentUpdate {
                    ordinal: segment.ordinal,
                    page: None,
                    href: Some(manifest.documents[place.document].href.clone()),
                    anchor_state: place.state,
                    selectors,
                });
                ranges.push(range);
            }
            None => updates.push(SegmentUpdate {
                ordinal: segment.ordinal,
                page: None,
                href: segment.href.clone(),
                anchor_state: AnchorState::Orphan,
                selectors: Vec::new(),
            }),
        }
    }
    let state = anchoring::overall(updates.iter().map(|update| update.anchor_state));
    (
        ranges,
        Some(Reanchoring {
            uuid: annotation.uuid.clone(),
            anchor_state: state,
            progression,
            revision_id: revision,
            version: Some(NORMALIZER_VERSION),
            segments: updates,
        }),
    )
}

/// The open flowing book's annotations, with where they are in it now.
#[tauri::command(async)]
pub fn flow_annotations(
    state: State<'_, AppState>,
    books: State<'_, BookState>,
    document: i64,
) -> Result<Vec<FlowAnnotationView>, String> {
    let book = book::open_flow_book(&books, document)
        .ok_or_else(|| format!("document {document} is not the one open"))?;
    let Some(revision) = book.revision_id else {
        return Ok(Vec::new());
    };
    let library = library(&state)?;
    let annotations = library
        .annotations(document, false)
        .map_err(|error| error.to_string())?;
    let (views, outcomes) = on_large_stack(move || {
        let mut views = Vec::with_capacity(annotations.len());
        let mut outcomes = Vec::new();
        for mut annotation in annotations {
            let (ranges, outcome) = anchor_flow(&annotation, &book, revision);
            if let Some(outcome) = outcome {
                annotation.anchor_state = outcome.anchor_state;
                annotation.progression = outcome.progression.or(annotation.progression);
                outcomes.push(outcome);
            }
            views.push(FlowAnnotationView { annotation, ranges });
        }
        Ok((views, outcomes))
    })?;
    record(&library, outcomes);
    Ok(views)
}

/// Where a reader CFI is: the book's document and the offset into its text.
fn flow_offset(book: &FlowBook, reader: &str) -> Result<(usize, u64), String> {
    if reader.len() > 4096 {
        return Err("that position is too long".to_owned());
    }
    let canonical =
        locator::reader_to_canonical(&book.manifest, reader).map_err(|error| error.to_string())?;
    let parsed = cfi::parse(&canonical).map_err(|error| error.to_string())?;
    let index = locator::document_of(&book.manifest, &parsed).map_err(|error| error.to_string())?;
    let document = book.document(index)?;
    let (point, _) = cfi::resolve(&document.dom, &parsed.path, parsed.offset)
        .map_err(|error| error.to_string())?;
    Ok((
        index,
        document
            .text_offset(point)
            .map_err(|error| error.to_string())?,
    ))
}

/// Makes an annotation from a selection in the open flowing book, from where it starts to where it
/// ends as the reader's CFIs. One that crosses documents gets a segment in each.
#[tauri::command(async)]
pub fn create_flow_annotation(
    state: State<'_, AppState>,
    books: State<'_, BookState>,
    document: i64,
    start: String,
    end: String,
    mark: MarkArgs,
) -> Result<FlowAnnotationView, String> {
    let book = book::open_flow_book(&books, document)
        .ok_or_else(|| format!("document {document} is not the one open"))?;
    let revision = book
        .revision_id
        .ok_or("este livro não está na biblioteca, e só os da biblioteca guardam anotações")?;
    let library = library(&state)?;
    let (new, ranges) = on_large_stack(move || {
        let (mut first, mut from) = flow_offset(&book, &start)?;
        let (mut last, mut to) = flow_offset(&book, &end)?;
        if (last, to) < (first, from) {
            std::mem::swap(&mut first, &mut last);
            std::mem::swap(&mut from, &mut to);
        }
        if last - first >= MAX_SPANS {
            return Err("a selection covers from one chapter to a few dozen".to_owned());
        }
        let manifest = &book.manifest;
        let mut segments = Vec::new();
        let mut quotes = Vec::new();
        let mut ranges = Vec::new();
        for index in first..=last {
            let whole = book.document(index)?;
            let a = if index == first { from } else { 0 };
            let b = if index == last { to } else { whole.text_length };
            if a >= b {
                continue;
            }
            let ordinal = segments.len() as u32;
            let (segment, quote) =
                flow_anchor::segment_for(ordinal, manifest, &whole, a, b, revision)
                    .ok_or("that selection is not in the book's text")?;
            if quote.exact.is_empty() {
                continue;
            }
            let (reader_start, reader_end) = flow_anchor::reader_range(manifest, &whole, a, b)
                .ok_or("that selection is not in the book's text")?;
            ranges.push(FlowRange {
                start: reader_start,
                end: reader_end,
            });
            quotes.push(quote.exact);
            segments.push(segment);
        }
        if segments.is_empty() {
            return Err("nada foi selecionado".to_owned());
        }
        let total = manifest.total_text().max(1);
        let progression =
            ((manifest.text_before(first) + from) as f64 / total as f64).clamp(0.0, 1.0);
        Ok((
            NewAnnotation {
                document_id: book.document_id,
                revision_id: revision,
                kind: mark.kind,
                color: mark.color,
                note: mark.note,
                quote: Some(quotes.join("\n")),
                quote_source: QuoteSource::Flow,
                progression: Some(progression),
                anchor_state: AnchorState::Exact,
                resolved_version: Some(NORMALIZER_VERSION),
                pdf_nm: None,
                segments,
            },
            ranges,
        ))
    })?;
    let annotation = library
        .create_annotation(new)
        .map_err(|error| error.to_string())?;
    Ok(FlowAnnotationView { annotation, ranges })
}

/// Changes an annotation's kind, color or note.
#[tauri::command(async)]
pub fn edit_annotation(
    state: State<'_, AppState>,
    uuid: String,
    edit: AnnotationEdit,
) -> Result<Annotation, String> {
    library(&state)?
        .edit_annotation(&uuid, edit)
        .map_err(|error| error.to_string())
}

/// Deletes an annotation logically.
#[tauri::command(async)]
pub fn delete_annotation(state: State<'_, AppState>, uuid: String) -> Result<Annotation, String> {
    library(&state)?
        .delete_annotation(&uuid)
        .map_err(|error| error.to_string())
}

/// Brings back an annotation deleted logically.
#[tauri::command(async)]
pub fn restore_annotation(state: State<'_, AppState>, uuid: String) -> Result<Annotation, String> {
    library(&state)?
        .restore_annotation(&uuid)
        .map_err(|error| error.to_string())
}

/// Finds annotations by their quotes and notes, in one document or in the whole library.
#[tauri::command(async)]
pub fn search_annotations(
    state: State<'_, AppState>,
    document: Option<i64>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<AnnotationHit>, String> {
    if query.len() > MAX_QUERY {
        return Err("that search is too long".to_owned());
    }
    library(&state)?
        .search_annotations(document, &query, limit.unwrap_or(200).min(1000))
        .map_err(|error| error.to_string())
}
