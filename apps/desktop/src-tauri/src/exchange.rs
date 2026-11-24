//! Annotations out of the library and back in as W3C Web Annotations (JSON-LD), and study reports
//! of them in Markdown and PDF: for the book open in the reader, for works chosen in the library, or
//! for the whole library.
//!
//! Every file is written where the person chose, never inside a folder of the library: into a
//! temporary file beside it, flushed to the disk, and renamed into place, so what is there is the
//! whole file or the one that was there before. A PDF report is read back by PDFium, in its sandbox,
//! before it takes its name. A JSON-LD file is read whole and checked by the library before
//! anything of it is written ([`musaeum_library::exchange`]); what it brings from other bytes than
//! a book's is found again on that book right away, by the same cascade the reader uses, and the
//! person is told how each one was found.

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use musaeum_core::{DeliveryMode, FixedFormat, PoolConfig, RenderPool};
use musaeum_flow::cfi;
use musaeum_flow::locator;
use musaeum_flow::package::NavPoint;
use musaeum_i18n::{T, tr};
use musaeum_ingest::protocol::Format;
use musaeum_library::anchoring::fixed;
use musaeum_library::anchoring::region::{self, RegionTool};
use musaeum_library::{
    AnchorState, Annotation, AnnotationKind, BookFile, ExchangeImport, ExchangeScope,
    ExchangeTarget, ImportedInto, IngestClient, Library, MAX_JSONLD_BYTES, Reanchoring,
    SelectorKind, UnmatchedWork,
};
use musaeum_report::{
    Creator, Found, Heading, Identifier, Item, Kind, Layout, Mark, Report, WorkReport,
};
use musaeum_shm::CharLayer;
use musaeum_shm::slab::slab_len;
use musaeum_shm::{PixelSlab, ShmRegion};
use musaeum_vision::LINES_VERSION;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State, WebviewWindow};

use crate::annotations::{
    CharCache, anchor_fixed, anchor_flow, fixed_annotations, flow_annotations, record,
};
use crate::book::{self, BookState};
use crate::flow::{FlowBook, on_large_stack};
use crate::pdf_annotations::{chosen, open_to_read};
use crate::picker::{FileKind, pick_file, pick_save};
use crate::reading;
use crate::session::AppState;
use crate::shelf::owner_of;

const JSONLD: FileKind = FileKind {
    name: T::FileKindJsonLd,
    pattern: "*.jsonld",
};
const MARKDOWN: FileKind = FileKind {
    name: T::FileKindMarkdown,
    pattern: "*.md",
};
const PDF: FileKind = FileKind {
    name: T::FileKindPdf,
    pattern: "*.pdf",
};
/// How long a worker may take to start, and to give one page's characters or the outline.
const START_TIMEOUT: Duration = Duration::from_secs(60);
const PAGE_TIMEOUT: Duration = Duration::from_secs(30);

fn library(state: &AppState) -> Result<Arc<Library>, String> {
    state
        .shelf
        .library()
        .ok_or_else(|| "there is no library open".to_owned())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// A path as it is compared: absolute, its links resolved as far as it exists, in lower case.
fn comparable(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut rest = Vec::new();
    while !existing.exists() {
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                existing = parent.to_path_buf();
            }
            _ => break,
        }
    }
    let mut full = std::fs::canonicalize(&existing).unwrap_or(existing);
    for name in rest.into_iter().rev() {
        full.push(name);
    }
    PathBuf::from(full.to_string_lossy().to_lowercase())
}

/// Refuses a file inside a folder the library follows: what is written there would become a book
/// of the library, or replace one.
pub fn check_outside_library(library: &Library, target: &Path) -> Result<(), String> {
    let target = comparable(target);
    for root in library.roots().map_err(|error| error.to_string())? {
        if root.status == "removed" {
            continue;
        }
        if target.starts_with(comparable(&root.path)) {
            return Err(tr!(InsideLibraryFolder, path = root.path.display()));
        }
    }
    Ok(())
}

/// Writes `bytes` as `target`: into a new temporary file beside it, flushed to the disk, then
/// renamed into place. `check` reads the temporary file before it takes the name, and may refuse
/// it.
pub fn write_atomically(
    target: &Path,
    bytes: &[u8],
    check: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<u64, String> {
    let folder = target.parent().ok_or_else(|| tr!(TargetHasNoFolder))?;
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut created = None;
    for attempt in 0..16 {
        let temporary = folder.join(format!(
            ".{name}.musaeum-{}-{attempt}.tmp",
            std::process::id()
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => {
                created = Some((temporary, file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(tr!(
                    FileNotCreated,
                    folder = folder.display(),
                    error = error
                ));
            }
        }
    }
    let (temporary, mut file) = created.ok_or_else(|| tr!(TemporaryNamesInUse))?;
    let written = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| tr!(FileNotWritten, error = error));
    drop(file);
    let done = written.and_then(|()| check(&temporary)).and_then(|()| {
        std::fs::rename(&temporary, target).map_err(|error| tr!(FileNotRenamed, error = error))
    });
    if let Err(reason) = done {
        let _ = std::fs::remove_file(&temporary);
        return Err(reason);
    }
    Ok(bytes.len() as u64)
}

/// A title made fit for a file name on Windows.
fn file_name_of(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let mut name = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.chars().count() > 100 {
        name = name
            .chars()
            .take(100)
            .collect::<String>()
            .trim_end()
            .to_owned();
    }
    let name = name.trim_matches(|c: char| c == '.' || c == ' ').to_owned();
    if name.is_empty() {
        "Livro".to_owned()
    } else {
        name
    }
}

fn title_of(library: &Library, document: i64) -> String {
    library
        .document(document)
        .map(|info| info.title)
        .unwrap_or_else(|_| "Livro".to_owned())
}

// --- JSON-LD ---

/// What an export wrote.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonLdExported {
    path: String,
    annotations: usize,
    documents: usize,
    works: usize,
    bytes: u64,
}

/// Writes the annotations of a document, of works or of the whole library as W3C Web Annotations,
/// where the person chooses.
#[tauri::command(async)]
pub fn annotations_export_jsonld(
    window: WebviewWindow,
    state: State<'_, AppState>,
    scope: ExchangeScope,
    target: Option<String>,
) -> Result<Option<JsonLdExported>, String> {
    let library = library(&state)?;
    let export = library
        .export_jsonld(scope.clone(), now_ms(), env!("CARGO_PKG_VERSION"))
        .map_err(|error| error.to_string())?;
    if export.annotations == 0 {
        return Err(tr!(NoAnnotationsToExport));
    }
    let suggested = match &scope {
        ExchangeScope::Document(document) => tr!(
            NameDocumentJsonLd,
            title = file_name_of(&title_of(&library, *document))
        ),
        ExchangeScope::Works(works) if works.len() == 1 => {
            let title = library
                .work_metadata(works[0])
                .map(|work| work.title)
                .unwrap_or_default();
            tr!(NameDocumentJsonLd, title = file_name_of(&title))
        }
        ExchangeScope::Works(works) => tr!(NameWorksJsonLd, count = works.len()),
        ExchangeScope::Library => tr!(NameLibraryJsonLd),
    };
    let Some(target) = chosen(&state, target, || {
        pick_save(
            owner_of(&window),
            &tr!(PickExportJsonLd),
            JSONLD,
            None,
            &suggested,
        )
    })?
    else {
        return Ok(None);
    };
    check_outside_library(&library, &target)?;
    let bytes = write_atomically(&target, export.json.as_bytes(), |_| Ok(()))?;
    Ok(Some(JsonLdExported {
        path: target.to_string_lossy().into_owned(),
        annotations: export.annotations,
        documents: export.documents,
        works: export.works,
        bytes,
    }))
}

/// How the annotations brought into one book were found on it.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BroughtInto {
    document_id: i64,
    title: String,
    added: usize,
    exact: usize,
    approximate: usize,
    orphan: usize,
    /// Not found again yet: the book's file is not there now, or a mark on a page whose picture
    /// has not been looked at yet. They are found the next time the book is opened.
    pending: usize,
}

/// What an import brought.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonLdImported {
    path: String,
    total: usize,
    added: usize,
    already: usize,
    elsewhere: usize,
    other_work: usize,
    unmatched: Vec<UnmatchedWork>,
    books: Vec<BroughtInto>,
}

/// A fixed-layout book's pages, read by a pool of its own, outside the reader.
struct PoolPages<'a> {
    pool: &'a RenderPool,
    kept: HashMap<u32, Arc<CharLayer>>,
}

impl fixed::Pages for PoolPages<'_> {
    fn count(&self) -> u32 {
        self.pool.pages().len() as u32
    }

    fn layer(&mut self, page: u32) -> Option<Arc<CharLayer>> {
        if let Some(layer) = self.kept.get(&page) {
            return Some(Arc::clone(layer));
        }
        let layer = Arc::new(
            self.pool
                .request_chars(page)
                .recv_timeout(PAGE_TIMEOUT)
                .ok()?
                .ok()?,
        );
        self.kept.insert(page, Arc::clone(&layer));
        Some(layer)
    }
}

/// A pool of one sandboxed worker for a fixed-layout book, as the full-text index reads them.
fn pool_for(library: &Library, file: &BookFile) -> Result<(RenderPool, FixedFormat), String> {
    let format = FixedFormat::of_file(&file.path)
        .map_err(|error| error.to_string())?
        .ok_or("not a PDF, comic or DjVu file")?;
    let worker_exe = std::env::current_exe()
        .map_err(|error| error.to_string())?
        .with_file_name(format.worker_exe());
    let known = file
        .blake3
        .and_then(|hash| library.page_geometry(&hash).ok().flatten());
    let region = ShmRegion::create(slab_len(2)).map_err(|error| error.to_string())?;
    let slab = PixelSlab::init(region, 2).map_err(|error| error.to_string())?;
    let pool = RenderPool::start(
        PoolConfig {
            worker_exe,
            document: file.path.clone(),
            worker_count: 1,
            process_memory_limit: Some(1 << 30),
            policy: format.policy(),
            startup_timeout: START_TIMEOUT,
            known_geometry: known.as_ref().map(reading::known_geometry),
        },
        Arc::new(slab),
        DeliveryMode::Pull { max_batch: 1 },
    )
    .map_err(|error| error.to_string())?;
    Ok((pool, format))
}

fn flow_book_for(
    library: &Library,
    client: &Mutex<Option<IngestClient>>,
    document: i64,
    file: &BookFile,
) -> Result<FlowBook, String> {
    let format = Format::from_extension(&file.format).ok_or("not a flowing book")?;
    let hash = file.blake3.ok_or_else(|| tr!(BookNotReadYet))?;
    let (dir, manifest, _) = crate::flow::normalized(library, client, &file.path, format, &hash)?;
    Ok(FlowBook::new(
        document,
        Some(file.revision_id),
        dir,
        manifest,
    ))
}

fn tally(into: &mut BroughtInto, state: Option<AnchorState>) {
    match state {
        Some(AnchorState::Exact) => into.exact += 1,
        Some(AnchorState::Approximate) => into.approximate += 1,
        Some(AnchorState::Orphan) => into.orphan += 1,
        None => into.pending += 1,
    }
}

/// Finds the annotations an import brought again on their book now, and says how each was found.
fn anchor_brought(
    app: &AppHandle,
    state: &State<'_, AppState>,
    cache: &State<'_, CharCache>,
    books: &State<'_, BookState>,
    library: &Library,
    into: &ImportedInto,
) -> BroughtInto {
    let mut brought = BroughtInto {
        document_id: into.document_id,
        title: into.title.clone(),
        added: into.added.len(),
        ..BroughtInto::default()
    };
    let uuids: HashSet<&str> = into
        .added
        .iter()
        .map(|annotation| annotation.uuid.as_str())
        .collect();
    let stored = |brought: &mut BroughtInto| {
        for annotation in &into.added {
            tally(brought, Some(annotation.anchor_state));
        }
    };
    // The book open in the reader: listing its annotations finds them again and records it.
    if let Some(session) = state
        .current_session()
        .filter(|session| session.document.map(|(document, _)| document) == Some(into.document_id))
    {
        match fixed_annotations(app.clone(), state.clone(), cache.clone(), session.id) {
            Ok(views) => {
                for view in views
                    .iter()
                    .filter(|view| uuids.contains(view.annotation().uuid.as_str()))
                {
                    tally(&mut brought, Some(view.annotation().anchor_state));
                }
            }
            Err(error) => {
                eprintln!("the annotations brought could not be found again: {error}");
                brought.pending = brought.added;
            }
        }
        return brought;
    }
    if book::open_flow_book(books, into.document_id).is_some() {
        match flow_annotations(state.clone(), books.clone(), into.document_id) {
            Ok(views) => {
                for view in views
                    .iter()
                    .filter(|view| uuids.contains(view.annotation().uuid.as_str()))
                {
                    tally(&mut brought, Some(view.annotation().anchor_state));
                }
            }
            Err(error) => {
                eprintln!("the annotations brought could not be found again: {error}");
                brought.pending = brought.added;
            }
        }
        return brought;
    }
    if !into.needs_anchoring {
        stored(&mut brought);
        return brought;
    }
    // A book not open: read here, in a worker of its own.
    let file = match library.book_file(into.document_id) {
        Ok(Some(file)) if file.state == "present" && file.blake3.is_some() => file,
        _ => {
            brought.pending = brought.added;
            return brought;
        }
    };
    let annotations: Vec<Annotation> = match library.annotations(into.document_id, false) {
        Ok(all) => all
            .into_iter()
            .filter(|annotation| uuids.contains(annotation.uuid.as_str()))
            .collect(),
        Err(_) => {
            brought.pending = brought.added;
            return brought;
        }
    };
    let revision = file.revision_id;
    let outcomes: Vec<Option<Reanchoring>> = if file.layout == "fixed" {
        let Ok((pool, _)) = pool_for(library, &file) else {
            brought.pending = brought.added;
            return brought;
        };
        let mut pages = PoolPages {
            pool: &pool,
            kept: HashMap::new(),
        };
        let blake3 = file.blake3;
        let page_hash = |page: u32| {
            blake3
                .and_then(|hash| {
                    library
                        .line_model(&hash, page, LINES_VERSION)
                        .ok()
                        .flatten()
                })
                .map(|record| record.page_hash)
        };
        let found = annotations
            .iter()
            .map(|annotation| anchor_fixed(annotation, &mut pages, revision, &page_hash).1)
            .collect();
        pool.shutdown();
        found
    } else {
        let client = Mutex::new(None);
        let book = match flow_book_for(library, &client, into.document_id, &file) {
            Ok(book) => Arc::new(book),
            Err(_) => {
                brought.pending = brought.added;
                return brought;
            }
        };
        let list = annotations.clone();
        on_large_stack(move || {
            Ok(list
                .iter()
                .map(|annotation| anchor_flow(annotation, &book, revision).1)
                .collect())
        })
        .unwrap_or_default()
    };
    for found in &outcomes {
        tally(
            &mut brought,
            found.as_ref().map(|outcome| outcome.anchor_state),
        );
    }
    record(library, outcomes.into_iter().flatten().collect());
    brought
}

/// Brings the annotations of a JSON-LD file into the library: into the book open, a work's books,
/// or wherever their works are.
#[tauri::command(async)]
#[allow(clippy::too_many_arguments)]
pub fn annotations_import_jsonld(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    cache: State<'_, CharCache>,
    books: State<'_, BookState>,
    target: ExchangeTarget,
    source: Option<String>,
) -> Result<Option<JsonLdImported>, String> {
    let library = library(&state)?;
    let Some(source) = chosen(&state, source, || {
        pick_file(owner_of(&window), &tr!(PickImportJsonLd), JSONLD, None)
    })?
    else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    open_to_read(&source)
        .and_then(|file| file.take(MAX_JSONLD_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|error| tr!(FileNotRead, error = error))?;
    if bytes.len() as u64 > MAX_JSONLD_BYTES {
        return Err(tr!(FileTooLarge));
    }
    let import: ExchangeImport = library
        .import_jsonld(&bytes, target)
        .map_err(|error| error.to_string())?;
    let books_brought: Vec<BroughtInto> = import
        .into
        .iter()
        .map(|into| anchor_brought(&app, &state, &cache, &books, &library, into))
        .collect();
    Ok(Some(JsonLdImported {
        path: source.to_string_lossy().into_owned(),
        total: import.total,
        added: import.into.iter().map(|into| into.added.len()).sum(),
        already: import.already,
        elsewhere: import.elsewhere,
        other_work: import.other_work,
        unmatched: import.unmatched,
        books: books_brought,
    }))
}

// --- Reports ---

/// Which books a report is about.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "ids")]
pub enum ReportScope {
    Document(i64),
    Works(Vec<i64>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReportFormat {
    Markdown,
    Pdf,
}

/// What a report wrote.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportWritten {
    path: String,
    works: usize,
    annotations: usize,
    /// For a PDF.
    pages: Option<usize>,
    bytes: u64,
}

/// What a book's table of contents and pages are, as the report places annotations under them.
struct Shape {
    pages: Option<u32>,
    contents: Vec<Heading>,
    flow: Option<Arc<FlowBook>>,
}

fn flatten_outline(entries: Vec<musaeum_shm::outline::OutlineEntry>) -> Vec<Heading> {
    entries
        .into_iter()
        .filter_map(|entry| {
            Some(Heading {
                label: entry.title,
                depth: u32::from(entry.level),
                at: f64::from(entry.page?),
            })
        })
        .collect()
}

/// Where a flowing book's entries of contents start, in UTF-16 code units of its whole text.
fn flow_contents(book: &Arc<FlowBook>) -> Vec<Heading> {
    let book = Arc::clone(book);
    on_large_stack(move || {
        let manifest = &book.manifest;
        let mut parts: HashMap<&str, (usize, u64)> = HashMap::new();
        for (index, document) in manifest.documents.iter().enumerate() {
            for part in &document.parts {
                parts.insert(part.href.as_str(), (index, part.text_start));
            }
        }
        let mut headings = Vec::new();
        fn walk(
            points: &[NavPoint],
            depth: u32,
            out: &mut Vec<(String, u32, Option<String>, Option<String>)>,
        ) {
            for point in points {
                out.push((
                    point.label.clone(),
                    depth,
                    point.cfi.clone(),
                    point.href.clone(),
                ));
                walk(&point.children, depth + 1, out);
            }
        }
        let mut flat = Vec::new();
        walk(&manifest.toc, 0, &mut flat);
        for (label, depth, canonical, href) in flat {
            let from_cfi = canonical.as_deref().and_then(|text| {
                let parsed = cfi::parse(text).ok()?;
                let index = locator::document_of(manifest, &parsed).ok()?;
                let document = book.document(index).ok()?;
                let (point, _) = cfi::resolve(&document.dom, &parsed.path, parsed.offset).ok()?;
                Some((index, document.text_offset(point).ok()?))
            });
            let from_href = || {
                let part = href.as_deref()?.split('#').next()?;
                parts.get(part).copied()
            };
            if let Some((index, offset)) = from_cfi.or_else(from_href) {
                headings.push(Heading {
                    label,
                    depth,
                    at: (manifest.text_before(index) + offset) as f64,
                });
            }
        }
        Ok(headings)
    })
    .unwrap_or_default()
}

fn shape_of(
    state: &AppState,
    books: &BookState,
    library: &Library,
    document: i64,
    layout: &str,
) -> Shape {
    let empty = Shape {
        pages: None,
        contents: Vec::new(),
        flow: None,
    };
    if layout == "fixed" {
        if let Some(session) = state
            .current_session()
            .filter(|session| session.document.map(|(open, _)| open) == Some(document))
        {
            let contents = session
                .pool
                .outline(PAGE_TIMEOUT)
                .map(flatten_outline)
                .unwrap_or_default();
            return Shape {
                pages: Some(session.pool.pages().len() as u32),
                contents,
                flow: None,
            };
        }
        let Ok(Some(file)) = library.book_file(document) else {
            return empty;
        };
        let Ok((pool, _)) = pool_for(library, &file) else {
            return empty;
        };
        let contents = pool
            .outline(PAGE_TIMEOUT)
            .map(flatten_outline)
            .unwrap_or_default();
        let pages = Some(pool.pages().len() as u32);
        pool.shutdown();
        return Shape {
            pages,
            contents,
            flow: None,
        };
    }
    let book = match book::open_flow_book(books, document) {
        Some(book) => Some(book),
        None => library
            .book_file(document)
            .ok()
            .flatten()
            .and_then(|file| flow_book_for(library, &state.flow_client, document, &file).ok())
            .map(Arc::new),
    };
    match book {
        Some(book) => Shape {
            pages: None,
            contents: flow_contents(&book),
            flow: Some(book),
        },
        None => empty,
    }
}

fn mark_of(annotation: &Annotation) -> Mark {
    let Some(segment) = annotation.segments.first() else {
        return Mark::Text;
    };
    match region::region_of(segment).map(|found| found.tool) {
        Some(RegionTool::Lines) => Mark::Lines,
        Some(RegionTool::Rect) => Mark::Box,
        Some(RegionTool::Free) => Mark::Stroke,
        Some(RegionTool::Point) => Mark::Point,
        Some(RegionTool::Quads) => Mark::Quads,
        None => Mark::Text,
    }
}

fn item_of(annotation: &Annotation, layout: Layout, shape: &Shape) -> Item {
    let first = annotation.segments.first();
    let page = first.and_then(|segment| segment.page);
    let at = match (layout, first) {
        (_, None) => None,
        (Layout::Fixed, Some(_)) => match (page, annotation.progression, shape.pages) {
            (Some(page), Some(progression), Some(count)) if count > 0 => Some(
                (progression * f64::from(count))
                    .max(f64::from(page))
                    .min(f64::from(page) + 0.999),
            ),
            (Some(page), ..) => Some(f64::from(page)),
            (None, progression, _) => progression,
        },
        (Layout::Flow, Some(segment)) => {
            let placed = shape.flow.as_ref().and_then(|book| {
                let manifest = &book.manifest;
                let index = manifest
                    .documents
                    .iter()
                    .position(|document| Some(&document.href) == segment.href.as_ref())?;
                let start = segment
                    .selectors
                    .iter()
                    .find(|selector| selector.kind == SelectorKind::TextPosition)
                    .and_then(|selector| selector.data.get("start")?.as_u64())
                    .unwrap_or(0);
                Some((manifest.text_before(index) + start) as f64)
            });
            placed.or_else(|| {
                let total = shape
                    .flow
                    .as_ref()
                    .map_or(1, |book| book.manifest.total_text().max(1));
                annotation
                    .progression
                    .map(|progression| progression * total as f64)
            })
        }
    };
    Item {
        uuid: annotation.uuid.clone(),
        kind: match annotation.kind {
            AnnotationKind::Highlight => Kind::Highlight,
            AnnotationKind::Underline => Kind::Underline,
            AnnotationKind::Note => Kind::Note,
        },
        mark: mark_of(annotation),
        color: annotation.color.clone(),
        quote: annotation.quote.clone(),
        note: annotation.note.clone(),
        at,
        page,
        progression: annotation.progression,
        found: match annotation.anchor_state {
            AnchorState::Exact => Found::Exact,
            AnchorState::Approximate => Found::Approximate,
            AnchorState::Orphan => Found::Orphan,
        },
        created_at: annotation.created_at,
        updated_at: annotation.updated_at,
    }
}

fn work_report(
    state: &AppState,
    books: &BookState,
    library: &Library,
    document: i64,
) -> Result<Option<WorkReport>, String> {
    let annotations = library
        .annotations(document, false)
        .map_err(|error| error.to_string())?;
    if annotations.is_empty() {
        return Ok(None);
    }
    let info = library
        .document(document)
        .map_err(|error| error.to_string())?;
    let work = library
        .work_metadata(info.work_id)
        .map_err(|error| error.to_string())?;
    let layout = if info.layout == "fixed" {
        Layout::Fixed
    } else {
        Layout::Flow
    };
    let shape = shape_of(state, books, library, document, &info.layout);
    let items = annotations
        .iter()
        .map(|annotation| item_of(annotation, layout, &shape))
        .collect();
    Ok(Some(WorkReport {
        title: work.title,
        creators: work
            .creators
            .into_iter()
            .map(|credit| Creator {
                name: credit.name,
                role: credit.role,
            })
            .collect(),
        series: work.series,
        series_position: work.series_position,
        year: work.year,
        language: work.language,
        identifiers: work
            .identifiers
            .into_iter()
            .map(|id| Identifier {
                scheme: id.scheme,
                value: id.value,
            })
            .collect(),
        format: info.format,
        layout,
        pages: shape.pages,
        contents: shape.contents,
        items,
    }))
}

/// Writes a study report of a book or of works, as Markdown or as a PDF, where the person
/// chooses.
#[tauri::command(async)]
#[allow(clippy::too_many_arguments)]
pub fn study_report(
    window: WebviewWindow,
    state: State<'_, AppState>,
    books: State<'_, BookState>,
    format: ReportFormat,
    scope: ReportScope,
    utc_offset_minutes: i32,
    target: Option<String>,
    generated_at: Option<i64>,
) -> Result<Option<ReportWritten>, String> {
    let library = library(&state)?;
    if !(-14 * 60..=14 * 60).contains(&utc_offset_minutes) {
        return Err("that offset from UTC is no time zone".to_owned());
    }
    let documents: Vec<i64> = match &scope {
        ReportScope::Document(document) => vec![*document],
        ReportScope::Works(works) => {
            if works.is_empty() || works.len() > 2000 {
                return Err(tr!(ReportTooManyWorks));
            }
            let mut documents = Vec::new();
            for work in works {
                documents.extend(
                    library
                        .work_metadata(*work)
                        .map_err(|error| error.to_string())?
                        .documents,
                );
            }
            documents
        }
    };
    let mut works = Vec::new();
    for document in documents {
        if let Some(report) = work_report(&state, &books, &library, document)? {
            works.push(report);
        }
    }
    if works.is_empty() {
        return Err(tr!(NoAnnotationsForReport));
    }
    let generated_at = match generated_at {
        Some(at) if state.is_e2e() => at,
        Some(_) => return Err("a time is only given under a test".to_owned()),
        None => now_ms(),
    };
    let report = Report {
        generated_at,
        utc_offset_minutes,
        generator: format!("Musaeum {}", env!("CARGO_PKG_VERSION")),
        works,
        // The words in the person's language, and their name when they gave one.
        locale: musaeum_i18n::locale(),
        annotator: state.preferences.chosen_author(),
    };
    let annotations = report.works.iter().map(|work| work.items.len()).sum();
    let base = match report.works.as_slice() {
        [work] => tr!(NameWorkReport, title = file_name_of(&work.title)),
        works => tr!(NameWorksReport, count = works.len()),
    };
    let (kind, suggested) = match format {
        ReportFormat::Markdown => (MARKDOWN, format!("{base}.md")),
        ReportFormat::Pdf => (PDF, format!("{base}.pdf")),
    };
    let title = match format {
        ReportFormat::Markdown => tr!(PickExportReportMarkdown),
        ReportFormat::Pdf => tr!(PickExportReportPdf),
    };
    let Some(target) = chosen(&state, target, || {
        pick_save(owner_of(&window), &title, kind, None, &suggested)
    })?
    else {
        return Ok(None);
    };
    check_outside_library(&library, &target)?;
    let (bytes, pages) = match format {
        ReportFormat::Markdown => (
            musaeum_report::markdown::markdown(&report).into_bytes(),
            None,
        ),
        ReportFormat::Pdf => {
            let (bytes, pages) = musaeum_report::pdf::render(&report);
            (bytes, Some(pages))
        }
    };
    let written = write_atomically(&target, &bytes, |temporary| match pages {
        // PDFium, in its sandbox, opens it and reads its first page's text before it is kept.
        Some(pages) => check_report_pdf(temporary, pages),
        None => Ok(()),
    })?;
    Ok(Some(ReportWritten {
        path: target.to_string_lossy().into_owned(),
        works: report.works.len(),
        annotations,
        pages,
        bytes: written,
    }))
}

/// Reads the pages of a PDF in a worker of its own: their text, as PDFium gives it.
fn pdf_pages_text(path: &Path) -> Result<Vec<String>, String> {
    let format = FixedFormat::of_file(path)
        .map_err(|error| error.to_string())?
        .ok_or("not a PDF")?;
    let worker_exe = std::env::current_exe()
        .map_err(|error| error.to_string())?
        .with_file_name(format.worker_exe());
    let region = ShmRegion::create(slab_len(2)).map_err(|error| error.to_string())?;
    let slab = PixelSlab::init(region, 2).map_err(|error| error.to_string())?;
    let pool = RenderPool::start(
        PoolConfig {
            worker_exe,
            document: path.to_path_buf(),
            worker_count: 1,
            process_memory_limit: Some(1 << 30),
            policy: format.policy(),
            startup_timeout: START_TIMEOUT,
            known_geometry: None,
        },
        Arc::new(slab),
        DeliveryMode::Pull { max_batch: 1 },
    )
    .map_err(|error| error.to_string())?;
    let mut pages = Vec::new();
    for page in 0..pool.pages().len() as u32 {
        match pool.request_chars(page).recv_timeout(PAGE_TIMEOUT) {
            Ok(Ok(layer)) => pages.push(layer.chars.iter().map(|glyph| glyph.ch).collect()),
            _ => {
                pool.shutdown();
                return Err(format!("page {} could not be read", page + 1));
            }
        }
    }
    pool.shutdown();
    Ok(pages)
}

fn check_report_pdf(path: &Path, expected: usize) -> Result<(), String> {
    let pages = pdf_pages_text(path).map_err(|error| tr!(ReportPdfUnreadable, error = error))?;
    if pages.len() != expected || pages.first().is_none_or(|text| text.trim().is_empty()) {
        return Err(tr!(ReportPdfMismatch));
    }
    Ok(())
}

/// The text of each page of a PDF as PDFium reads it, for the window tests.
#[tauri::command(async)]
pub fn e2e_pdf_text(state: State<'_, AppState>, path: String) -> Result<Vec<String>, String> {
    if !state.is_e2e() {
        return Err("only under a test".to_owned());
    }
    pdf_pages_text(Path::new(&path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_become_file_names() {
        assert_eq!(file_name_of("Física: <Básica>? / 2"), "Física Básica 2");
        assert_eq!(file_name_of("  ...  "), "Livro");
        assert_eq!(file_name_of(&"a".repeat(300)).len(), 100);
    }

    #[test]
    fn a_file_is_written_whole_or_not_at_all() {
        let folder = std::env::temp_dir().join(format!("musaeum-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let target = folder.join("relatório.md");
        assert_eq!(write_atomically(&target, b"um", |_| Ok(())).unwrap(), 2);
        assert_eq!(std::fs::read(&target).unwrap(), b"um");
        let refused = write_atomically(&target, b"dois", |_| Err("no".to_owned()));
        assert!(refused.is_err());
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"um",
            "the file before stays"
        );
        assert_eq!(
            std::fs::read_dir(&folder).unwrap().count(),
            1,
            "no temporary file left"
        );
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
