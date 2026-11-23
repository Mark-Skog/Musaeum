//! The books' text, read into the full-text index while nobody waits for it, and searched.
//!
//! One thread reads one book at a time, in Windows' background mode, on a budget: after each
//! stretch of work it rests twice as long, nine times as long while a book is open in the reader,
//! so the reader always comes first. A fixed-layout book's text comes from its own worker, in its
//! sandbox, page by page, as when it is read; a page with no text layer gives the lines OCR read
//! on it, if any were. A flowing book's comes from its normalized documents, which the reader
//! counts in the same way, so a match's position is a place in the book.
//!
//! | Command | Answer |
//! |---------|--------|
//! | `fulltext_status` | how far the index is, and the book being read |
//! | `fulltext_search` | the books with every word asked for, and where |
//! | `fulltext_flow_place` | a match in a flowing book, as the reader's CFIs |
//! | `fulltext_page_point` | a match on a page of the open fixed book, as a point on it |

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use musaeum_core::{DeliveryMode, FixedFormat, PoolConfig, RenderPool};
use musaeum_flow::package::Manifest;
use musaeum_ingest::protocol::Format;
use musaeum_library::anchoring::flow::reader_range;
use musaeum_library::{
    FullTextResults, FullTextStatus, IngestClient, Library, PendingText, TextSource, TextUnit,
    UnitKind,
};
use musaeum_shm::slab::slab_len;
use musaeum_shm::text::TextLayer;
use musaeum_shm::{PixelSlab, ShmRegion};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::reading;
use crate::session::AppState;

/// Sent to the page as the index grows.
pub const FULLTEXT_PROGRESS: &str = "fulltext-progress";

/// How long a worker may take to start, and to give one page's text.
const START_TIMEOUT: Duration = Duration::from_secs(60);
const PAGE_TIMEOUT: Duration = Duration::from_secs(30);
/// Work, then rest this many times as long; more while a book is being read.
const REST: u32 = 2;
const REST_WHILE_READING: u32 = 9;
/// The longest stretch of work between rests, and the longest rest.
const STRETCH: Duration = Duration::from_millis(250);
const LONGEST_REST: Duration = Duration::from_secs(3);
/// Books asked for at a time.
const BATCH: usize = 8;
/// Progress told to the page at most this often.
const TELL_EVERY: Duration = Duration::from_millis(400);

#[derive(Default)]
struct Signal {
    wanted: Mutex<bool>,
    changed: Condvar,
    stop: AtomicBool,
    /// Whether a book is open in the reader.
    reading: AtomicBool,
    progress: Mutex<Progress>,
}

/// What the indexer is doing.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub running: bool,
    /// The book being read now.
    pub current: Option<String>,
    /// Books read since the app started.
    pub read: u64,
}

/// Wakes the indexer when the library may have new books, and tells it when a book is open.
#[derive(Clone)]
pub struct FullTextWaker(Arc<Signal>);

impl FullTextWaker {
    pub fn wake(&self) {
        *self.0.wanted.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.0.changed.notify_all();
    }

    pub fn set_reading(&self, reading: bool) {
        self.0.reading.store(reading, Ordering::Relaxed);
    }

    pub fn progress(&self) -> Progress {
        self.0
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Reads the books' text into the index. Dropping it stops it within a page.
pub struct FullTextService {
    signal: Arc<Signal>,
    thread: Option<JoinHandle<()>>,
}

impl FullTextService {
    pub fn start(library: Arc<Library>, app: AppHandle) -> Self {
        let signal = Arc::new(Signal::default());
        *signal.wanted.lock().unwrap_or_else(PoisonError::into_inner) = true;
        let thread = {
            let signal = Arc::clone(&signal);
            thread::Builder::new()
                .name("fulltext".to_owned())
                // Assembling a flowing book's documents walks trees as deep as they nest.
                .stack_size(crate::flow::STACK)
                .spawn(move || run(&library, &app, &signal))
                .map_err(|error| eprintln!("could not start reading the books' text: {error}"))
                .ok()
        };
        Self { signal, thread }
    }

    pub fn waker(&self) -> FullTextWaker {
        FullTextWaker(Arc::clone(&self.signal))
    }
}

impl Drop for FullTextService {
    fn drop(&mut self) {
        self.signal.stop.store(true, Ordering::Relaxed);
        self.signal.changed.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Why reading a book's text ended without its text.
enum Unread {
    Stopped,
    Failed(String),
}

impl From<String> for Unread {
    fn from(error: String) -> Self {
        Unread::Failed(error)
    }
}

impl From<&str> for Unread {
    fn from(error: &str) -> Self {
        Unread::Failed(error.to_owned())
    }
}

/// Work and rest, measured: rests when a stretch of work is done, longer while a book is read.
struct Budget<'a> {
    signal: &'a Signal,
    since: Instant,
}

impl<'a> Budget<'a> {
    fn new(signal: &'a Signal) -> Self {
        Self {
            signal,
            since: Instant::now(),
        }
    }

    /// After a piece of work: rests if the stretch is long enough. False if it should stop.
    fn tick(&mut self) -> bool {
        let worked = self.since.elapsed();
        if worked >= STRETCH {
            self.rest(worked);
        }
        !self.signal.stop.load(Ordering::Relaxed)
    }

    fn rest(&mut self, worked: Duration) {
        let factor = if self.signal.reading.load(Ordering::Relaxed) {
            REST_WHILE_READING
        } else {
            REST
        };
        let until = Instant::now() + (worked * factor).min(LONGEST_REST);
        // Only stopping ends a rest early: a wake-up stays wanted for after it.
        let mut guard = self
            .signal
            .wanted
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        while !self.signal.stop.load(Ordering::Relaxed) {
            let now = Instant::now();
            if now >= until {
                break;
            }
            guard = self
                .signal
                .changed
                .wait_timeout(guard, until - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        drop(guard);
        self.since = Instant::now();
    }
}

fn tell(app: &AppHandle, signal: &Signal, update: impl FnOnce(&mut Progress)) {
    let progress = {
        let mut progress = signal
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        update(&mut progress);
        progress.clone()
    };
    let _ = app.emit(FULLTEXT_PROGRESS, progress);
}

fn run(library: &Library, app: &AppHandle, signal: &Signal) {
    let _background = musaeum_fs::enter_background();
    let client: Mutex<Option<IngestClient>> = Mutex::new(None);
    match library.fulltext_forget_gone() {
        Ok(0) => {}
        Ok(count) => eprintln!("{count} files gone from the library left the full-text index"),
        Err(error) => eprintln!("the full-text index could not be tidied: {error}"),
    }
    let mut told: Option<Instant> = None;
    loop {
        {
            let mut wanted = signal.wanted.lock().unwrap_or_else(PoisonError::into_inner);
            while !*wanted && !signal.stop.load(Ordering::Relaxed) {
                wanted = signal
                    .changed
                    .wait(wanted)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            *wanted = false;
        }
        if signal.stop.load(Ordering::Relaxed) {
            return;
        }
        loop {
            let pending = match library.fulltext_pending(BATCH) {
                Ok(pending) => pending,
                Err(error) => {
                    eprintln!("could not list the books to read the text of: {error}");
                    break;
                }
            };
            if pending.is_empty() {
                break;
            }
            for item in pending {
                if signal.stop.load(Ordering::Relaxed) {
                    return;
                }
                let title = item.file.title.clone();
                tell(app, signal, |progress| {
                    progress.running = true;
                    progress.current = Some(title);
                });
                let mut budget = Budget::new(signal);
                let read = if item.file.layout == "fixed" {
                    fixed_units(library, &item, &mut budget)
                } else {
                    flow_units(library, &client, &item, &mut budget)
                };
                let stored = match read {
                    Ok(units) => library.fulltext_store(&item.blake3, &units, item.ocr_lines),
                    Err(Unread::Stopped) => return,
                    Err(Unread::Failed(error)) => {
                        eprintln!(
                            "the text of {} could not be read: {error}",
                            item.file.path.display()
                        );
                        library.fulltext_failed(&item.blake3, &error)
                    }
                };
                if let Err(error) = stored {
                    eprintln!("the full-text index: {error}");
                    // Something is wrong with the index itself: try again on the next wake-up.
                    tell(app, signal, |progress| progress.running = false);
                    break;
                }
                {
                    let mut progress = signal
                        .progress
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    progress.read += 1;
                }
                if told.is_none_or(|told| told.elapsed() >= TELL_EVERY) {
                    tell(app, signal, |_| {});
                    told = Some(Instant::now());
                }
                let worked = budget.since.elapsed();
                budget.rest(worked);
            }
        }
        tell(app, signal, |progress| {
            progress.running = false;
            progress.current = None;
        });
    }
}

/// A page's text as the index keeps it: its words, a space between them, a line break where a
/// line ends.
pub fn page_text(layer: &TextLayer) -> String {
    let mut text = String::new();
    for word in &layer.words {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push(' ');
        }
        text.push_str(&word.text);
        if word.line_end {
            text.push('\n');
        }
    }
    text
}

fn fixed_units(
    library: &Library,
    item: &PendingText,
    budget: &mut Budget<'_>,
) -> Result<Vec<TextUnit>, Unread> {
    let path = &item.file.path;
    let format = FixedFormat::of_file(path)
        .map_err(|error| error.to_string())?
        .ok_or("not a PDF, comic or DjVu file")?;
    // A comic's pages are pictures, with no text to give.
    if matches!(format, FixedFormat::Comic) {
        return Ok(Vec::new());
    }
    let worker_exe = std::env::current_exe()
        .map_err(|error| error.to_string())?
        .with_file_name(format.worker_exe());
    let known = library.page_geometry(&item.blake3).ok().flatten();
    let region = ShmRegion::create(slab_len(2)).map_err(|error| error.to_string())?;
    let slab = PixelSlab::init(region, 2).map_err(|error| error.to_string())?;
    let pool = RenderPool::start(
        PoolConfig {
            worker_exe,
            document: path.clone(),
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
    let ocr = library.ocr_page_texts(&item.blake3).unwrap_or_default();
    let pages = pool.pages().len() as u32;
    let mut units = Vec::new();
    for page in 0..pages {
        let answer = pool.request_text(page).recv_timeout(PAGE_TIMEOUT);
        let text = match answer {
            Ok(Ok(layer)) => page_text(&layer),
            // A page that cannot give its text gives none; the others still do.
            Ok(Err(error)) => {
                eprintln!(
                    "page {} of {} gave no text: {error}",
                    page + 1,
                    path.display()
                );
                String::new()
            }
            Err(_) => {
                pool.shutdown();
                return Err(Unread::Failed(format!(
                    "page {} gave no text in time",
                    page + 1
                )));
            }
        };
        if !text.trim().is_empty() {
            units.push(TextUnit {
                kind: UnitKind::Page,
                index: page,
                label: None,
                source: TextSource::TextLayer,
                text,
            });
        } else if let Some(text) = ocr.get(&page) {
            units.push(TextUnit {
                kind: UnitKind::Page,
                index: page,
                label: None,
                source: TextSource::Ocr,
                text: text.clone(),
            });
        }
        if !budget.tick() {
            pool.shutdown();
            return Err(Unread::Stopped);
        }
    }
    pool.shutdown();
    Ok(units)
}

/// What the reader calls a document: its own title, or the table of contents' first entry into it.
fn labels(manifest: &Manifest) -> HashMap<usize, String> {
    let mut parts: HashMap<&str, usize> = HashMap::new();
    for (index, document) in manifest.documents.iter().enumerate() {
        for part in &document.parts {
            parts.insert(part.href.as_str(), index);
        }
    }
    let mut labels = HashMap::new();
    fn walk(
        points: &[musaeum_flow::package::NavPoint],
        parts: &HashMap<&str, usize>,
        labels: &mut HashMap<usize, String>,
    ) {
        for point in points {
            if let Some(href) = &point.href {
                let part = href.split('#').next().unwrap_or(href);
                if let Some(&index) = parts.get(part)
                    && !point.label.trim().is_empty()
                {
                    labels
                        .entry(index)
                        .or_insert_with(|| point.label.trim().to_owned());
                }
            }
            walk(&point.children, parts, labels);
        }
    }
    walk(&manifest.toc, &parts, &mut labels);
    for (index, document) in manifest.documents.iter().enumerate() {
        if let Some(title) = document
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
        {
            labels.insert(index, title.to_owned());
        }
    }
    labels
}

fn flow_units(
    library: &Library,
    client: &Mutex<Option<IngestClient>>,
    item: &PendingText,
    budget: &mut Budget<'_>,
) -> Result<Vec<TextUnit>, Unread> {
    let format =
        Format::from_extension(&item.file.format).ok_or("not a format the worker reads")?;
    let (dir, manifest, _) =
        crate::flow::normalized(library, client, &item.file.path, format, &item.blake3)?;
    let labels = labels(&manifest);
    let mut units = Vec::with_capacity(manifest.documents.len());
    for index in 0..manifest.documents.len() {
        let document = crate::flow::assemble(&dir, &manifest, index)?;
        units.push(TextUnit {
            kind: UnitKind::Document,
            index: index as u32,
            label: labels.get(&index).cloned(),
            source: TextSource::Flow,
            text: document.text.clone(),
        });
        if !budget.tick() {
            return Err(Unread::Stopped);
        }
    }
    Ok(units)
}

fn library_of(state: &AppState) -> Result<Arc<Library>, String> {
    state
        .shelf
        .library()
        .ok_or_else(|| "there is no library open".to_owned())
}

/// How far the index is, and what the indexer is doing.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FullTextView {
    #[serde(flatten)]
    status: FullTextStatus,
    #[serde(flatten)]
    progress: Progress,
}

#[tauri::command(async)]
pub fn fulltext_status(state: State<'_, AppState>) -> Result<FullTextView, String> {
    let library = library_of(&state)?;
    Ok(FullTextView {
        status: library
            .fulltext_status()
            .map_err(|error| error.to_string())?,
        progress: state.shelf.fulltext_progress(),
    })
}

#[tauri::command(async)]
pub fn fulltext_search(
    state: State<'_, AppState>,
    query: String,
    books: Option<usize>,
) -> Result<FullTextResults, String> {
    library_of(&state)?
        .fulltext_search(&query, books.unwrap_or(50))
        .map_err(|error| error.to_string())
}

/// Where a match is in a flowing book, as the reader names places.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowPlace {
    start: String,
    end: String,
}

#[tauri::command(async)]
pub fn fulltext_flow_place(
    state: State<'_, AppState>,
    document: i64,
    unit: u32,
    position: u64,
    length: u64,
) -> Result<FlowPlace, String> {
    let library = library_of(&state)?;
    let file = library
        .book_file(document)
        .map_err(|error| error.to_string())?
        .ok_or("the book has no file now")?;
    let blake3 = file.blake3.ok_or("the book has not been read yet")?;
    let format = Format::from_extension(&file.format).ok_or("not a flowing book")?;
    let client = Arc::new(Mutex::new(None::<IngestClient>));
    crate::flow::on_large_stack(move || {
        let (dir, manifest, _) =
            crate::flow::normalized(&library, &client, &file.path, format, &blake3)?;
        let document = crate::flow::assemble(&dir, &manifest, unit as usize)?;
        let (start, end) = reader_range(&manifest, &document, position, position + length.max(1))
            .ok_or("the match is not in the book as it is now")?;
        Ok(FlowPlace { start, end })
    })
}

/// A match on a page of the fixed book open now, as a point on it in fractions of the page: the
/// top left of the word it starts in. None if the page has no such text now.
#[tauri::command(async)]
pub fn fulltext_page_point(
    state: State<'_, AppState>,
    session: u32,
    page: u32,
    position: u32,
) -> Result<Option<[f32; 2]>, String> {
    let session = state
        .session(session)
        .ok_or("that document is no longer open")?;
    if page as usize >= session.pool.pages().len() {
        return Err(format!("there is no page {page}"));
    }
    let Ok(Ok(layer)) = session.pool.request_text(page).recv_timeout(PAGE_TIMEOUT) else {
        return Ok(None);
    };
    // The words laid out as `page_text` lays them, each one's place in UTF-16 code units.
    let mut at: u32 = 0;
    let mut text = String::new();
    for word in &layer.words {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push(' ');
            at += 1;
        }
        let length = word.text.encode_utf16().count() as u32;
        if position < at + length {
            return Ok(Some([word.rect[0], word.rect[1]]));
        }
        text.push_str(&word.text);
        at += length;
        if word.line_end {
            text.push('\n');
            at += 1;
        }
    }
    Ok(None)
}

/// Reads what is waiting now, for tests and after something new was read by OCR.
#[tauri::command]
pub fn fulltext_wake(state: State<'_, AppState>) {
    state.shelf.wake_fulltext();
}

#[cfg(test)]
mod tests {
    use super::*;
    use musaeum_shm::text::Word;

    #[test]
    fn a_page_s_text_keeps_its_lines() {
        let word = |text: &str, line_end: bool| Word {
            text: text.to_owned(),
            rect: [0.0, 0.0, 0.1, 0.1],
            line_end,
        };
        let layer = TextLayer {
            words: vec![
                word("No", false),
                word("meio", true),
                word("do", false),
                word("caminho", true),
            ],
        };
        assert_eq!(page_text(&layer), "No meio\ndo caminho\n");
    }
}
