//! Flowing books in the reader: EPUB, MOBI, AZW3, FB2, plain text and Markdown.
//!
//! A book is normalized once, in the ingest worker's sandbox, and kept in the cache by its bytes
//! and the normalizer's version; after that it opens from the cache. The reader gets the
//! normalized package from the book's own origin (see `book.rs`), and positions go both ways
//! between what the reader counts in (CFIs in the parts it shows) and what the library keeps (a
//! Locator with the canonical CFI, which counts in the book's own documents).

use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use musaeum_flow::NORMALIZER_VERSION;
use musaeum_flow::cfi;
use musaeum_flow::locator::{self, Found, Locator, LogicalDocument};
use musaeum_flow::package::{Manifest, NavPoint};
use musaeum_flow::xml;
use musaeum_ingest::protocol::Format;
use musaeum_library::{
    FlowReadingState, Hash, IngestClient, IngestConfig, Library, NormalizeOutcome, ReadingMode,
};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::book::{self, BookState};
use crate::session::AppState;

/// Whole documents kept parsed, for turning positions into Locators without parsing again.
const KEPT_DOCUMENTS: usize = 4;
/// Parsing and locating walk trees as deep as a document nests.
const STACK: usize = 32 << 20;

/// A normalized book, open in the reader.
pub struct FlowBook {
    pub document_id: i64,
    pub revision_id: Option<i64>,
    /// Where its package is, in the cache.
    pub dir: PathBuf,
    pub manifest: Manifest,
    documents: Mutex<Vec<(usize, Arc<LogicalDocument>)>>,
}

impl FlowBook {
    /// One whole document, put back together from its parts.
    fn document(&self, index: usize) -> Result<Arc<LogicalDocument>, String> {
        {
            let mut kept = self
                .documents
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(at) = kept.iter().position(|(kept, _)| *kept == index) {
                let entry = kept.remove(at);
                let document = Arc::clone(&entry.1);
                kept.push(entry);
                return Ok(document);
            }
        }
        let info = self
            .manifest
            .documents
            .get(index)
            .ok_or_else(|| format!("the book has no document {index}"))?;
        let mut parts = Vec::with_capacity(info.parts.len());
        for part in &info.parts {
            let text = std::fs::read_to_string(self.dir.join(&part.href))
                .map_err(|error| format!("{}: {error}", part.href))?;
            let dom = xml::parse(&text, &xml::Limits::default())
                .map_err(|error| format!("{}: {error}", part.href))?;
            parts.push((part.start.clone(), dom));
        }
        let document =
            Arc::new(LogicalDocument::assemble(index, parts).map_err(|error| error.to_string())?);
        let mut kept = self
            .documents
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if kept.len() >= KEPT_DOCUMENTS {
            kept.remove(0);
        }
        kept.push((index, Arc::clone(&document)));
        Ok(document)
    }

    /// The Locator of a position the reader names by its own CFI.
    pub fn locate(&self, reader_cfi: &str) -> Result<Locator, String> {
        let canonical = locator::reader_to_canonical(&self.manifest, reader_cfi)
            .map_err(|error| error.to_string())?;
        let parsed = cfi::parse(&canonical).map_err(|error| error.to_string())?;
        let index =
            locator::document_of(&self.manifest, &parsed).map_err(|error| error.to_string())?;
        let document = self.document(index)?;
        let (point, _) = cfi::resolve(&document.dom, &parsed.path, parsed.offset)
            .map_err(|error| error.to_string())?;
        locator::locate(&self.manifest, &document, point).map_err(|error| error.to_string())
    }

    /// Where a Locator is now, as the reader's CFI, and how it was found.
    pub fn resolve(&self, locator: &Locator) -> Result<(String, Found), String> {
        let index = self
            .manifest
            .documents
            .iter()
            .position(|document| document.href == locator.href)
            .or_else(|| {
                cfi::parse(&locator.locations.cfi)
                    .ok()
                    .and_then(|parsed| locator::document_of(&self.manifest, &parsed).ok())
            })
            .ok_or("the book no longer has that document")?;
        let document = self.document(index)?;
        let (point, found) =
            locator::resolve(locator, &document).map_err(|error| error.to_string())?;
        let canonical = locator::canonical_cfi(&self.manifest, &document, point);
        let reader = locator::canonical_to_reader(&self.manifest, &canonical)
            .map_err(|error| error.to_string())?;
        Ok((reader, found))
    }
}

/// Runs `work` on a thread with a stack deep enough for any document the normalizer lets through.
fn on_large_stack<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    std::thread::Builder::new()
        .name("flow-positions".to_owned())
        .stack_size(STACK)
        .spawn(work)
        .map_err(|error| error.to_string())?
        .join()
        .map_err(|_| "working out a position failed".to_owned())?
}

/// The file's BLAKE3, read now, for a book the library has not hashed yet.
fn hash_of(path: &std::path::Path) -> Result<Hash, String> {
    let mut file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(*hasher.finalize().as_bytes())
}

/// The normalized book of a file, from the cache, or normalized now in the worker and kept.
fn normalized(
    library: &Library,
    client: &Mutex<Option<IngestClient>>,
    path: &std::path::Path,
    format: Format,
    hash: &Hash,
) -> Result<(PathBuf, Manifest, bool), String> {
    let store = library.flow();
    if let Ok(Some(manifest)) = store.manifest(hash)
        && manifest.normalizer == NORMALIZER_VERSION
    {
        return Ok((store.book_dir(hash), manifest, true));
    }
    let file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let outcome = client
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(|| IngestClient::new(IngestConfig::for_normalizing()))
        .normalize(&file, format);
    let normalized = match outcome {
        NormalizeOutcome::Normalized(normalized) => normalized,
        NormalizeOutcome::Nothing(reason) | NormalizeOutcome::Unavailable(reason) => {
            return Err(format!("o livro não pôde ser preparado: {reason}"));
        }
    };
    let dir = store
        .put(hash, &normalized.files)
        .map_err(|error| error.to_string())?;
    Ok((dir, normalized.manifest, false))
}

/// One part of the reading order, as the reader lays it out.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionInfo {
    /// Its path in the package.
    href: String,
    document: usize,
    part: usize,
    /// How many parts its document was cut into.
    parts: usize,
    bytes: u64,
    text_start: u64,
    text_length: u64,
    /// Where its text starts in the whole book, in UTF-16 code units.
    book_text_start: u64,
    linear: bool,
    /// The part's first element, as even CFI steps below `<body>` of the whole document.
    start: Vec<u32>,
}

/// Where the reader was, found again.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedFlow {
    reader_cfi: String,
    /// `cfi`, `corrected-cfi`, `quote` or `progression`.
    found: &'static str,
    locator: serde_json::Value,
    mode: ReadingMode,
    page_turn_animation: bool,
}

/// What the page needs to open a flowing book.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowInfo {
    origin: String,
    reader_url: String,
    document_id: i64,
    title: String,
    creators: Vec<String>,
    language: Option<String>,
    toc: Vec<NavPoint>,
    sections: Vec<SectionInfo>,
    total_text: u64,
    saved: Option<SavedFlow>,
    /// Whether the normalized book came from the cache.
    cached: bool,
    open_ms: f64,
}

/// What the page asks to open.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenFlowArgs {
    document: i64,
}

fn found_name(found: Found) -> &'static str {
    match found {
        Found::Cfi => "cfi",
        Found::CorrectedCfi => "corrected-cfi",
        Found::Quote => "quote",
        Found::Progression => "progression",
    }
}

/// Opens a flowing book of the library in the reader.
#[tauri::command(async)]
pub fn open_flow(
    state: State<'_, AppState>,
    books: State<'_, BookState>,
    host_origin: String,
    args: OpenFlowArgs,
) -> Result<FlowInfo, String> {
    let started = Instant::now();
    let library = state.shelf.library().ok_or("there is no library open")?;
    let file = library
        .book_file(args.document)
        .map_err(|error| error.to_string())?
        .ok_or("o arquivo deste livro não está mais na pasta")?;
    let format = Format::from_extension(&file.format)
        .filter(|format| format.flow().is_some())
        .ok_or_else(|| format!("{} is not a flowing format", file.format))?;
    let hash = match file.blake3 {
        Some(hash) => hash,
        None => hash_of(&file.path)?,
    };
    // A book opens with only the reader's own book shown.
    book::close(&books);
    let (dir, manifest, cached) =
        normalized(&library, &state.flow_client, &file.path, format, &hash)?;

    let saved_state = library
        .flow_reading_state(args.document)
        .map_err(|error| error.to_string())?;
    let flow = Arc::new(FlowBook {
        document_id: args.document,
        revision_id: Some(file.revision_id),
        dir,
        manifest,
        documents: Mutex::new(Vec::new()),
    });
    let saved = match saved_state {
        Some(state) => {
            let flow = Arc::clone(&flow);
            on_large_stack(move || {
                let locator: Locator =
                    serde_json::from_str(&state.locator).map_err(|error| error.to_string())?;
                let (reader_cfi, found) = flow.resolve(&locator)?;
                Ok(Some(SavedFlow {
                    reader_cfi,
                    found: found_name(found),
                    locator: serde_json::to_value(&locator).map_err(|error| error.to_string())?,
                    mode: state.mode,
                    page_turn_animation: state.page_turn_animation,
                }))
            })
            .unwrap_or_else(|error| {
                eprintln!("the saved position could not be found again: {error}");
                None
            })
        }
        None => None,
    };

    let manifest = &flow.manifest;
    let mut sections = Vec::new();
    for (document, part) in manifest.spine() {
        let info = &manifest.documents[document];
        let piece = &info.parts[part];
        sections.push(SectionInfo {
            href: piece.href.clone(),
            document,
            part,
            parts: info.parts.len(),
            bytes: piece.bytes,
            text_start: piece.text_start,
            text_length: piece.text_length,
            book_text_start: manifest.text_before(document) + piece.text_start,
            linear: info.linear,
            start: piece.start.clone(),
        });
    }
    let metadata = &manifest.metadata;
    let info = FlowInfo {
        origin: String::new(),
        reader_url: String::new(),
        document_id: args.document,
        title: metadata
            .title()
            .map_or_else(|| file.title.clone(), str::to_owned),
        creators: metadata
            .creators
            .iter()
            .map(|creator| creator.name.clone())
            .collect(),
        language: metadata.languages.first().cloned(),
        toc: manifest.toc.clone(),
        sections,
        total_text: manifest.total_text(),
        saved,
        cached,
        open_ms: 0.0,
    };
    let (origin, reader_url) =
        book::open_package(&books, &host_origin, Arc::clone(&flow), state.options().e2e)?;
    Ok(FlowInfo {
        origin,
        reader_url,
        open_ms: started.elapsed().as_secs_f64() * 1000.0,
        ..info
    })
}

/// Where the reader is, as it reports it.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowPositionArgs {
    reader_cfi: String,
    mode: ReadingMode,
    page_turn_animation: bool,
}

/// The position as the library keeps it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowPosition {
    locator: serde_json::Value,
    progress: f64,
    saved: bool,
}

/// Turns the reader's position into a Locator and keeps it in the library.
#[tauri::command(async)]
pub fn save_flow_position(
    state: State<'_, AppState>,
    books: State<'_, BookState>,
    document: i64,
    position: FlowPositionArgs,
) -> Result<FlowPosition, String> {
    let flow = book::open_flow_book(&books, document)
        .ok_or_else(|| format!("document {document} is not the one open"))?;
    if position.reader_cfi.len() > 4096 {
        return Err("that position is too long".to_owned());
    }
    let reader_cfi = position.reader_cfi.clone();
    let located = {
        let flow = Arc::clone(&flow);
        on_large_stack(move || flow.locate(&reader_cfi))?
    };
    let progress = located.locations.total_progression.clamp(0.0, 1.0);
    let json = serde_json::to_string(&located).map_err(|error| error.to_string())?;
    let saved = match state.shelf.library() {
        Some(library) => library
            .save_flow_reading_state(
                document,
                &FlowReadingState {
                    revision_id: flow.revision_id,
                    locator: json,
                    progress,
                    mode: position.mode,
                    page_turn_animation: position.page_turn_animation,
                    updated_at: 0,
                },
            )
            .map(|()| true)
            .map_err(|error| error.to_string())?,
        None => false,
    };
    Ok(FlowPosition {
        locator: serde_json::to_value(&located).map_err(|error| error.to_string())?,
        progress,
        saved,
    })
}

/// A Locator for a position, without keeping it: for showing where a link or a search result is.
#[tauri::command(async)]
pub fn locate_flow(
    books: State<'_, BookState>,
    document: i64,
    reader_cfi: String,
) -> Result<serde_json::Value, String> {
    let flow = book::open_flow_book(&books, document)
        .ok_or_else(|| format!("document {document} is not the one open"))?;
    if reader_cfi.len() > 4096 {
        return Err("that position is too long".to_owned());
    }
    let located = on_large_stack(move || flow.locate(&reader_cfi))?;
    serde_json::to_value(&located).map_err(|error| error.to_string())
}

/// Closes the book in the reader, whichever kind it is.
#[tauri::command(async)]
pub fn close_book(books: State<'_, BookState>) {
    book::close(&books);
}

#[cfg(test)]
mod tests {
    use super::*;
    use musaeum_flow::normalize::{Options, normalize};
    use musaeum_flow::package::SourceFormat;

    /// A book long enough for its one chapter to be cut into parts, kept in a folder of its own.
    fn cut_book() -> (FlowBook, PathBuf) {
        let mut text = String::from("# Capítulo\n\n");
        for number in 0..6000 {
            text.push_str(&format!(
                "Parágrafo {number}, com texto bastante para encher as partes, linha após linha.\n\n"
            ));
        }
        let book = normalize(SourceFormat::Markdown, text.as_bytes(), &Options::default()).unwrap();
        let dir = std::env::temp_dir().join(format!(
            "musaeum-flow-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        for (path, data) in &book.files {
            let file = dir.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, data).unwrap();
        }
        let flow = FlowBook {
            document_id: 1,
            revision_id: None,
            dir: dir.clone(),
            manifest: book.manifest,
            documents: Mutex::new(Vec::new()),
        };
        (flow, dir)
    }

    #[test]
    fn a_reader_position_becomes_a_locator_and_comes_back_in_any_part() {
        let (flow, dir) = cut_book();
        let document = flow
            .manifest
            .documents
            .iter()
            .position(|info| info.parts.len() > 2)
            .expect("the chapter was cut");
        for part in [0, 1, 2] {
            let global = flow
                .manifest
                .spine()
                .position(|(d, p)| d == document && p == part)
                .unwrap();
            // The fifth character of the part's first paragraph, as the reader writes it.
            let reader = format!("epubcfi(/6/{}!/4/2/1:5)", 2 * (global + 1));
            let locator = flow.locate(&reader).unwrap();
            assert_eq!(locator.normalizer, NORMALIZER_VERSION);
            let (back, found) = flow.resolve(&locator).unwrap();
            assert_eq!(found, Found::Cfi, "part {part}");
            assert_eq!(flow.locate(&back).unwrap(), locator, "part {part}");
            if part > 0 {
                assert!(locator.locations.progression > 0.0, "part {part}");
            }

            // From another normalizer version, the CFI is not trusted, and the quote finds it.
            let mut stale = locator.clone();
            stale.normalizer += 1;
            let (again, found) = flow.resolve(&stale).unwrap();
            assert_eq!(found, Found::Quote, "part {part}");
            assert_eq!(flow.locate(&again).unwrap(), locator, "part {part}");
        }
        // Positions keep parsed documents for later, never more than a few.
        assert!(flow.documents.lock().unwrap().len() <= KEPT_DOCUMENTS);
        assert!(flow.locate("epubcfi(/6/9999!/4/2/1:0)").is_err());
        assert!(flow.locate("not a cfi").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
