//! The text under a mark on the picture of a page: what a copy of it puts on the clipboard, and the
//! quote its annotation is found by.
//!
//! A mark is drawn on the picture of a page, along its lines, as a box or as a stroke, and the
//! picture is what it reads: the lines the detector found under the mark are read by the
//! sandboxed Tesseract worker, one line at a time, each straightened along its baseline
//! (`musaeum_ocr`), and what a line says is kept in the cache: a line is read once, whatever
//! selects it later. Words come back as stretches of their line, fractions of its baseline's
//! length, which is how a mark along lines names where it starts and ends; a box or a stroke takes
//! the words whose middles it covers. On a page that is a scan with a text layer over it (another
//! program's OCR, a DjVu's hidden text), the layer's characters under the mark stand in only when
//! the OCR has nothing there; the Text tool selects the layer's characters themselves, as on a
//! native page. A page that is no picture has only its characters.
//! (`region_reading`; the audit of Phase 5 decided it so.)
//!
//! A mark made where nothing was read yet gets its quote afterwards, on one thread in Windows'
//! background mode: the quote is filled once, the search indexes follow it, and the page learns of
//! it from an `annotation-text` event.

use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use musaeum_core::{BackgroundWork, FixedFormat, MAX_REGION_PIXELS, RegionRequest};
use musaeum_library::anchoring::region::{self, Region, RegionLine, RegionTool};
use musaeum_library::{
    Annotation, Library, OcrLineRecord, OcrReading, QuoteSource, Selector, SelectorKind,
};
use musaeum_ocr::{
    Gray as OcrGray, LinePath, LineText, LineWord, OCR_VERSION, OcrClient, OcrConfig, read_line,
};
use musaeum_sandbox::Policy;
use musaeum_shm::chars::{CharLayer, chars_in_quads};
use musaeum_shm::tile::{SCALE_ONE, page_pixels};
use musaeum_vision::{LINES_VERSION, LineModel, to_shown};
use serde::Serialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::annotations::{CharCache, RegionArgs, found_region};
use crate::session::{AppState, Session};

/// The languages read, of those there are models for: Portuguese and English, the ones shipped.
const PREFERRED: [&str; 2] = ["por", "eng"];
/// The height of the small letters Tesseract reads best, in pixels; a page is drawn for it at the
/// resolution that gives its lines about that, within what a scan holds.
const TARGET_X_HEIGHT: f32 = 22.0;
const MIN_DPI: f32 = 300.0;
const MAX_DPI: f32 = 600.0;
/// How long a page's picture may take to draw.
const PICTURE_WAIT: Duration = Duration::from_secs(60);
/// How long a copy waits for the background to trace a page's lines, and a mark's quote does.
const COPY_WAIT: Duration = Duration::from_secs(20);
const FILL_WAIT: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(150);
/// What a box or a stroke has to cover of a line, across it: the middle of its small letters.
const MIDDLE_OF_LETTERS: f32 = 0.5;
/// Samples along a line, per height of its small letters, to find where a box covers it.
const SAMPLES_PER_X_HEIGHT: f32 = 4.0;

/// A mark whose quote is to be read.
struct Job {
    session: Arc<Session>,
    uuid: String,
}

/// The OCR worker, shared by copies and by the quotes read in the background.
#[derive(Default)]
pub struct Ocr {
    client: Mutex<Option<OcrClient>>,
    languages: OnceLock<Option<String>>,
    jobs: Mutex<Option<Sender<Job>>>,
    queued: Mutex<HashSet<String>>,
}

/// What lies under a mark.
#[derive(Clone, Debug, PartialEq)]
pub enum Reading {
    /// Its text: lines in reading order, one `\n` between them, as a copy of a native page gives.
    Text(String, QuoteSource),
    /// Nothing readable: a photograph, a figure, a margin.
    Nothing,
    /// The page's lines are not traced yet.
    NotYet,
}

impl Ocr {
    /// Stops the worker. The next line to read starts another.
    pub fn release(&self) {
        let client = self
            .client
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        drop(client);
    }

    /// Stops reading quotes in the background, and the worker.
    pub fn shutdown(&self) {
        self.jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        self.release();
    }

    /// The languages the OCR reads, as Tesseract names them (`por+eng`), or none without models.
    fn languages(&self, policy: Policy) -> Option<String> {
        self.languages
            .get_or_init(|| {
                let available = OcrClient::new(config(policy)).languages();
                let chosen: Vec<&str> = PREFERRED
                    .iter()
                    .copied()
                    .filter(|wanted| available.iter().any(|name| name == wanted))
                    .collect();
                if chosen.is_empty() {
                    available.first().cloned()
                } else {
                    Some(chosen.join("+"))
                }
            })
            .clone()
    }

    /// Reads a mark's quote in the background, once, unless it is already waiting to be read.
    pub fn enqueue(&self, app: &AppHandle, session: Arc<Session>, uuid: String) {
        if !self
            .queued
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(uuid.clone())
        {
            return;
        }
        let mut jobs = self.jobs.lock().unwrap_or_else(PoisonError::into_inner);
        if jobs.is_none() {
            let (sender, receiver) = mpsc::channel();
            let app = app.clone();
            let spawned = thread::Builder::new()
                .name("ocr-quotes".into())
                .spawn(move || fill_quotes(&app, &receiver));
            if let Err(error) = spawned {
                eprintln!("could not start reading quotes: {error}");
                return;
            }
            *jobs = Some(sender);
        }
        if let Some(sender) = jobs.as_ref() {
            let _ = sender.send(Job { session, uuid });
        }
    }

    fn done(&self, uuid: &str) {
        self.queued
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(uuid);
    }

    /// Reads lines of a page's picture, starting the worker if there is none.
    fn read_lines(
        &self,
        policy: Policy,
        languages: &str,
        picture: &OcrGray,
        lines: &[LinePath],
        ppi: u32,
    ) -> Result<Vec<LineText>, String> {
        let mut client = self.client.lock().unwrap_or_else(PoisonError::into_inner);
        let client = client.get_or_insert_with(|| OcrClient::new(config(policy)));
        lines
            .iter()
            .map(|line| {
                read_line(client, languages, picture, line, ppi).map_err(|error| error.to_string())
            })
            .collect()
    }
}

fn config(policy: Policy) -> OcrConfig {
    OcrConfig {
        policy,
        ..OcrConfig::default()
    }
}

fn policy_of(state: &AppState) -> Policy {
    if state.options().no_sandbox {
        Policy::Unrestricted
    } else {
        Policy::LockdownParser
    }
}

/// The quotes' thread: one mark after another, until the app closes.
fn fill_quotes(app: &AppHandle, jobs: &Receiver<Job>) {
    let _background = musaeum_fs::enter_background();
    while let Ok(job) = jobs.recv() {
        let state = app.state::<AppState>();
        let filled = fill_quote(app, &state, &job);
        state.ocr.done(&job.uuid);
        match filled {
            Ok(Some(annotation)) => {
                let _ = app.emit("annotation-text", annotation);
            }
            Ok(None) => {}
            Err(error) => eprintln!("the quote of {} could not be read: {error}", job.uuid),
        }
    }
}

/// Reads the text under a mark that quotes nothing and keeps it as its quote.
fn fill_quote(app: &AppHandle, state: &AppState, job: &Job) -> Result<Option<Annotation>, String> {
    let library = state.library().ok_or("there is no library open")?;
    let annotation = library
        .annotation(&job.uuid)
        .map_err(|error| error.to_string())?;
    if annotation.quote.is_some() || annotation.deleted_at.is_some() {
        return Ok(None);
    }
    let Some(segment) = annotation.segments.first() else {
        return Ok(None);
    };
    let Some(found) = region::region_of(segment) else {
        return Ok(None);
    };
    let lines_version = segment
        .selectors
        .iter()
        .find(|selector| selector.kind == SelectorKind::PageRegion)
        .and_then(|selector| selector.normalizer_version);
    let cache = app.state::<CharCache>();
    let reading = read_region(
        state,
        &cache,
        &job.session,
        &found,
        lines_version,
        FILL_WAIT,
    )?;
    let Reading::Text(text, source) = reading else {
        return Ok(None);
    };
    let quote = collapse(&text);
    let selector = Selector {
        kind: SelectorKind::TextQuote,
        data: json!({"exact": quote, "prefix": "", "suffix": ""}),
        priority: 2,
        normalizer_version: None,
        revision_id: None,
    };
    library
        .fill_quote(&job.uuid, &quote, source, vec![(segment.ordinal, selector)])
        .map(Some)
        .map_err(|error| error.to_string())
}

/// Whitespace as one space, and none at the ends: a quote as the library keeps it.
pub fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What a copy of a mark on the picture of a page gives.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionTextView {
    text: String,
    source: QuoteSource,
    /// The page's lines were not traced in time; asking again later may find text.
    pending: bool,
}

/// The text under a region drawn on the open fixed document, for the clipboard.
#[tauri::command(async)]
pub fn region_text(
    state: State<'_, AppState>,
    cache: State<'_, CharCache>,
    session: u32,
    region: RegionArgs,
) -> Result<RegionTextView, String> {
    let session = state
        .session(session)
        .ok_or("that document is no longer open")?;
    let library = state.library();
    let found = found_region(library.as_deref(), &session, region)?;
    let view = |text: String, source: QuoteSource, pending: bool| RegionTextView {
        text,
        source,
        pending,
    };
    Ok(
        match read_region(
            &state,
            &cache,
            &session,
            &found,
            Some(LINES_VERSION),
            COPY_WAIT,
        )? {
            Reading::Text(text, source) => view(text, source, false),
            Reading::Nothing => view(String::new(), QuoteSource::None, false),
            Reading::NotYet => view(String::new(), QuoteSource::None, true),
        },
    )
}

/// Whether a page of the open document is a picture (a scan, with or without a text layer over
/// it), as the background classified it; None before it has.
pub fn picture_page(state: &AppState, session: &Session, page: u32) -> Option<bool> {
    let (library, blake3) = (state.library()?, session.blake3?);
    let content = library.page_content(&blake3).ok()??;
    let record = (*content.pages.get(page as usize)?)?;
    Some(crate::background::is_picture(record.class))
}

/// What a mark on a page reads, from whether the page is a picture (None before it is known), the
/// characters of its text layer under the mark, and the OCR. On a picture the OCR reads it, and
/// the layer stands in when the OCR has nothing: no line under the mark, no model to read with,
/// the lines not traced in time. On a page that is no picture, its characters, or nothing. On a
/// page not classified yet, as before the classes: its characters if it has some there, else the
/// OCR.
pub fn region_reading(
    picture: Option<bool>,
    layer: Option<(String, QuoteSource)>,
    ocr: impl FnOnce() -> Result<Reading, String>,
) -> Result<Reading, String> {
    let from_layer = |(text, source): (String, QuoteSource)| Reading::Text(text, source);
    match (picture, layer) {
        (Some(true), layer) => match ocr() {
            Ok(Reading::Text(text, source)) => Ok(Reading::Text(text, source)),
            Ok(other) => Ok(layer.map_or(other, from_layer)),
            Err(error) => layer.map(from_layer).ok_or(error),
        },
        (_, Some(layer)) => Ok(from_layer(layer)),
        (Some(false), None) => Ok(Reading::Nothing),
        (None, None) => ocr(),
    }
}

/// What lies under `found` on the open document (see [`region_reading`]). `lines_version` is the
/// detector's version the mark's lines are counted in; on another, a mark along lines is read as
/// the box it covers.
pub fn read_region(
    state: &AppState,
    cache: &CharCache,
    session: &Session,
    found: &Region,
    lines_version: Option<u32>,
    wait: Duration,
) -> Result<Reading, String> {
    let layer = cache.get(session, found.page).and_then(|layer| {
        let text = layer_text(&layer, &found.quads)?;
        let source = if matches!(session.format, FixedFormat::Djvu) {
            QuoteSource::Ocr
        } else {
            QuoteSource::TextLayer
        };
        Some((text, source))
    });
    let picture = picture_page(state, session, found.page);
    region_reading(picture, layer, || {
        read_region_by_ocr(state, session, found, lines_version, wait)
    })
}

/// What the OCR reads on the lines under `found`.
fn read_region_by_ocr(
    state: &AppState,
    session: &Session,
    found: &Region,
    lines_version: Option<u32>,
    wait: Duration,
) -> Result<Reading, String> {
    let (Some(library), Some(blake3)) = (state.library(), session.blake3) else {
        return Ok(Reading::Nothing);
    };
    let policy = policy_of(state);
    let Some(languages) = state.ocr.languages(policy) else {
        return Err("there is no OCR model: run tools/fetch-ocr.ps1".to_owned());
    };
    let Some(model) = wait_for_model(&library, session, found.page, wait)? else {
        return Ok(Reading::NotYet);
    };
    let size = session.pool.pages()[found.page as usize];
    let page = PagePoints {
        width: size.width_pt,
        height: size.height_pt,
        rotation: model.rotation,
    };
    let lines: Vec<LinePath> = model.lines.iter().map(|line| page.line(line)).collect();
    let pieces = match found.tool {
        RegionTool::Lines
            if lines_version == Some(LINES_VERSION)
                && found
                    .lines
                    .iter()
                    .all(|piece| (piece.index as usize) < lines.len()) =>
        {
            found.lines.clone()
        }
        _ => {
            let quads: Vec<[f32; 8]> = found.quads.iter().map(|quad| page.quad(quad)).collect();
            pieces_inside(&lines, &quads)
        }
    };
    if pieces.is_empty() {
        return Ok(Reading::Nothing);
    }
    let reading = OcrReading {
        lines_version: LINES_VERSION,
        ocr_version: OCR_VERSION,
        languages: &languages,
    };
    let mut wanted: Vec<u32> = pieces.iter().map(|piece| piece.index).collect();
    wanted.sort_unstable();
    wanted.dedup();
    let mut read: Vec<(u32, LineText)> = library
        .ocr_lines(&blake3, found.page, reading, &wanted)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter_map(|record| Some((record.line, decode_line(&record.words)?)))
        .collect();
    let missing: Vec<u32> = wanted
        .iter()
        .copied()
        .filter(|index| !read.iter().any(|(line, _)| line == index))
        .collect();
    if !missing.is_empty() {
        let paths: Vec<&LinePath> = missing
            .iter()
            .map(|&index| &lines[index as usize])
            .collect();
        let fresh = read_on_page(state, session, found.page, &page, &paths, &languages)?;
        let records: Vec<OcrLineRecord> = missing
            .iter()
            .zip(&fresh)
            .map(|(&line, text)| OcrLineRecord {
                line,
                words: encode_line(text),
            })
            .collect();
        match library.store_ocr_lines(&blake3, found.page, reading, &records) {
            // The file's text in the index now has fewer lines than the cache: it is read again,
            // giving way to the reader while the book is open.
            Ok(()) => state.shelf.wake_fulltext(),
            Err(error) => eprintln!("what the OCR read could not be kept: {error}"),
        }
        read.extend(missing.into_iter().zip(fresh));
    }
    let text = pieces_text(&pieces, &read);
    Ok(if text.trim().is_empty() {
        Reading::Nothing
    } else {
        Reading::Text(text, QuoteSource::Ocr)
    })
}

/// The characters of a text layer under quads, as a native selection copies them: line ends as
/// `\n`, nothing at the ends. None when there are none, or only spaces.
fn layer_text(layer: &CharLayer, quads: &[[f32; 8]]) -> Option<String> {
    let (start, end) = chars_in_quads(&layer.chars, quads)?;
    let raw: String = layer.chars[start..end.min(layer.chars.len())]
        .iter()
        .map(|glyph| glyph.ch)
        .collect();
    let text = raw.replace("\r\n", "\n").replace('\r', "\n");
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// The model of a page, waiting for the background to trace it, first, if it has not yet.
fn wait_for_model(
    library: &Library,
    session: &Session,
    page: u32,
    wait: Duration,
) -> Result<Option<LineModel>, String> {
    let Some(blake3) = session.blake3 else {
        return Ok(None);
    };
    let begun = Instant::now();
    let mut asked = false;
    loop {
        if let Some(record) = library
            .line_model(&blake3, page, LINES_VERSION)
            .map_err(|error| error.to_string())?
        {
            return LineModel::decode(&record.model)
                .map(Some)
                .map_err(|error| error.to_string());
        }
        if !asked {
            session.pool.add_background_first(BackgroundWork {
                lines: vec![page],
                ..BackgroundWork::default()
            });
            asked = true;
        }
        if begun.elapsed() >= wait {
            return Ok(None);
        }
        thread::sleep(POLL);
    }
}

/// A page as shown, in points, which have the same length along x and y.
struct PagePoints {
    width: f32,
    height: f32,
    rotation: u8,
}

impl PagePoints {
    fn point(&self, u: f32, v: f32) -> [f32; 2] {
        let [x, y] = to_shown(u, v, self.rotation);
        [x * self.width, y * self.height]
    }

    /// A line of the model, in points of the page as shown.
    fn line(&self, line: &musaeum_vision::VisualLine) -> LinePath {
        LinePath {
            baseline: line
                .baseline
                .iter()
                .map(|point| self.point(point[0], point[1]))
                .collect(),
            ascent: line.ascent * self.height,
            descent: line.descent * self.height,
            x_height: line.x_height * self.height,
        }
    }

    /// A quad as stored, in points of the page as shown.
    fn quad(&self, quad: &[f32; 8]) -> [f32; 8] {
        let mut out = [0.0; 8];
        for corner in 0..4 {
            let [x, y] = self.point(quad[corner * 2], quad[corner * 2 + 1]);
            out[corner * 2] = x;
            out[corner * 2 + 1] = y;
        }
        out
    }
}

/// Inside a convex quad: on the same side of its four edges.
fn inside(x: f32, y: f32, quad: &[f32; 8]) -> bool {
    let mut sign = 0.0f32;
    for corner in 0..4 {
        let (ax, ay) = (quad[corner * 2], quad[corner * 2 + 1]);
        let (bx, by) = (quad[(corner * 2 + 2) % 8], quad[(corner * 2 + 3) % 8]);
        let cross = (bx - ax) * (y - ay) - (by - ay) * (x - ax);
        if cross.abs() < 1e-9 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

/// The stretches of lines a box or a stroke covers: along each line, from the first place to the
/// last where the middle of its small letters is inside one of `quads`, all in the same unit.
pub fn pieces_inside(lines: &[LinePath], quads: &[[f32; 8]]) -> Vec<RegionLine> {
    let mut pieces = Vec::new();
    for (index, line) in (0u32..).zip(lines) {
        let length = line.length();
        let x_height = line.x_height.max(1e-3);
        if length <= 0.0 {
            continue;
        }
        let steps = ((length / x_height) * SAMPLES_PER_X_HEIGHT)
            .ceil()
            .clamp(8.0, 4000.0) as u32;
        let mut first = None;
        let mut last = None;
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            let (point, d) = line.at(t * length);
            // Upward across a line going right, with y growing downward.
            let up = [d[1], -d[0]];
            let rise = x_height * MIDDLE_OF_LETTERS;
            let (x, y) = (point[0] + up[0] * rise, point[1] + up[1] * rise);
            if quads.iter().any(|quad| inside(x, y, quad)) {
                first.get_or_insert(t);
                last = Some(t);
            }
        }
        if let (Some(from), Some(to)) = (first, last) {
            // Half a step out at each end: a word whose middle is on the edge of the box counts.
            let half = 0.5 / steps as f32;
            pieces.push(RegionLine {
                index,
                from: (from - half).max(0.0),
                to: (to + half).min(1.0),
            });
        }
    }
    pieces
}

/// Lines of a page drawn at a resolution the OCR reads well, and read.
fn read_on_page(
    state: &AppState,
    session: &Session,
    page: u32,
    points: &PagePoints,
    lines: &[&LinePath],
    languages: &str,
) -> Result<Vec<LineText>, String> {
    // The resolution that brings the lines' small letters to the height the OCR reads best.
    let mut heights: Vec<f32> = lines.iter().map(|line| line.x_height).collect();
    heights.sort_by(f32::total_cmp);
    let x_height = heights[heights.len() / 2].max(0.5);
    let mut dpi = (72.0 * TARGET_X_HEIGHT / x_height).clamp(MIN_DPI, MAX_DPI);
    // Around the lines, as far as their strips reach.
    let (mut left, mut top, mut right, mut bottom) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for line in lines {
        let reach = line.ascent.max(line.x_height * 1.8) + line.descent.max(line.x_height * 0.75);
        for point in &line.baseline {
            left = left.min(point[0] - reach * 1.6);
            right = right.max(point[0] + reach * 1.6);
            top = top.min(point[1] - reach * 1.6);
            bottom = bottom.max(point[1] + reach * 1.6);
        }
    }
    let (left, top) = (left.max(0.0), top.max(0.0));
    let (right, bottom) = (right.min(points.width), bottom.min(points.height));
    if right <= left || bottom <= top {
        return Ok(vec![LineText::default(); lines.len()]);
    }
    let area = (right - left) * (bottom - top);
    let most = MAX_REGION_PIXELS as f32 * 0.9;
    if area * (dpi / 72.0).powi(2) > most {
        dpi = 72.0 * (most / area).sqrt();
    }
    let scale = (SCALE_ONE as f32 * dpi / 72.0).round() as u32;
    let k = scale as f32 / SCALE_ONE as f32;
    let request = RegionRequest {
        page,
        scale,
        left: (left * k).floor() as u32,
        top: (top * k).floor() as u32,
        width: (((right - left) * k).ceil() as u32)
            .min(page_pixels(points.width, scale) as u32)
            .max(1),
        height: (((bottom - top) * k).ceil() as u32)
            .min(page_pixels(points.height, scale) as u32)
            .max(1),
    };
    let drawn = session
        .pool
        .request_region(request)
        .recv_timeout(PICTURE_WAIT)
        .map_err(|_| format!("page {page} was not drawn in time"))?
        .map_err(|error| format!("page {page} could not be drawn: {error}"))?;
    let picture = OcrGray {
        width: drawn.request.width,
        height: drawn.request.height,
        pixels: drawn.gray,
    };
    let (dx, dy) = (drawn.request.left as f32, drawn.request.top as f32);
    let paths: Vec<LinePath> = lines
        .iter()
        .map(|line| LinePath {
            baseline: line
                .baseline
                .iter()
                .map(|point| [point[0] * k - dx, point[1] * k - dy])
                .collect(),
            ascent: line.ascent * k,
            descent: line.descent * k,
            x_height: line.x_height * k,
        })
        .collect();
    state.ocr.read_lines(
        policy_of(state),
        languages,
        &picture,
        &paths,
        dpi.round() as u32,
    )
}

/// The words of the pieces, line after line in reading order: one space between words, one `\n`
/// between lines.
pub fn pieces_text(pieces: &[RegionLine], read: &[(u32, LineText)]) -> String {
    let mut pieces = pieces.to_vec();
    pieces.sort_by_key(|piece| piece.index);
    let lines = pieces.iter().filter_map(|piece| {
        let (_, text) = read.iter().find(|(line, _)| *line == piece.index)?;
        Some(
            text.between(piece.from, piece.to)
                .collect::<Vec<&LineWord>>(),
        )
    });
    musaeum_ocr::join_lines(lines, "\n")
}

/// A line's words as the cache keeps them.
pub fn encode_line(text: &LineText) -> String {
    json!({
        "words": text
            .words
            .iter()
            .map(|word| json!([word.text, word.from, word.to, word.confidence]))
            .collect::<Vec<_>>(),
        "confidence": text.confidence,
        "flipped": text.flipped,
    })
    .to_string()
}

/// A line's words as the cache kept them, or None if they make no sense.
pub fn decode_line(words: &str) -> Option<LineText> {
    let value: Value = serde_json::from_str(words).ok()?;
    let words = value
        .get("words")?
        .as_array()?
        .iter()
        .map(|word| {
            let word = word.as_array()?;
            Some(LineWord {
                text: word.first()?.as_str()?.to_owned(),
                from: word.get(1)?.as_f64()? as f32,
                to: word.get(2)?.as_f64()? as f32,
                confidence: word.get(3)?.as_f64()? as f32,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(LineText {
        words,
        confidence: value.get("confidence")?.as_f64()? as f32,
        flipped: value.get("flipped")?.as_bool()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, from: f32, to: f32) -> LineWord {
        LineWord {
            text: text.to_owned(),
            from,
            to,
            confidence: 90.0,
        }
    }

    fn level(y: f32) -> LinePath {
        LinePath {
            baseline: vec![[100.0, y], [500.0, y]],
            ascent: 9.0,
            descent: 3.0,
            x_height: 6.0,
        }
    }

    #[test]
    fn a_mark_on_a_picture_reads_the_picture_and_the_layer_stands_in() {
        let layer = || Some(("layer".to_owned(), QuoteSource::TextLayer));
        let read = || Ok(Reading::Text("ocr".to_owned(), QuoteSource::Ocr));
        let text = |reading: Result<Reading, String>| match reading {
            Ok(Reading::Text(text, _)) => Some(text),
            _ => None,
        };
        // A scan with a text layer over it: the OCR, whatever the layer says.
        assert_eq!(
            text(region_reading(Some(true), layer(), read)).as_deref(),
            Some("ocr")
        );
        // The OCR has nothing there, or cannot read: the layer.
        let nothing = || Ok(Reading::Nothing);
        let not_yet = || Ok(Reading::NotYet);
        let no_model = || Err("there is no OCR model".to_owned());
        assert_eq!(
            text(region_reading(Some(true), layer(), nothing)).as_deref(),
            Some("layer")
        );
        assert_eq!(
            text(region_reading(Some(true), layer(), not_yet)).as_deref(),
            Some("layer")
        );
        assert_eq!(
            text(region_reading(Some(true), layer(), no_model)).as_deref(),
            Some("layer")
        );
        // A scan with no layer: the OCR, or why it could not.
        assert!(matches!(
            region_reading(Some(true), None, not_yet),
            Ok(Reading::NotYet)
        ));
        assert!(region_reading(Some(true), None, no_model).is_err());
        // A native page: its characters, and no OCR, ever.
        let never =
            || -> Result<Reading, String> { panic!("a native page is not read by the OCR") };
        assert_eq!(
            text(region_reading(Some(false), layer(), never)).as_deref(),
            Some("layer")
        );
        assert!(matches!(
            region_reading(Some(false), None, never),
            Ok(Reading::Nothing)
        ));
        // Not classified yet: its characters if it has some there, else the OCR.
        assert_eq!(
            text(region_reading(None, layer(), never)).as_deref(),
            Some("layer")
        );
        assert_eq!(
            text(region_reading(None, None, read)).as_deref(),
            Some("ocr")
        );
    }

    #[test]
    fn a_line_read_comes_back_from_the_cache_as_it_was() {
        let text = LineText {
            words: vec![word("Olá", 0.0, 0.2), word("\"mundo\"", 0.3, 0.6)],
            confidence: 90.0,
            flipped: true,
        };
        // The shape `0005_ocr_line.sql` documents, which the full-text index reads too.
        let kept: Value = serde_json::from_str(&encode_line(&text)).unwrap();
        assert_eq!(kept["words"][0][0], "Olá");
        assert_eq!(kept["words"][1][0], "\"mundo\"");
        assert_eq!(decode_line(&encode_line(&text)), Some(text));
        assert_eq!(decode_line("[1, 2]"), None);
        assert_eq!(
            decode_line(r#"{"words":[["a",0,1]],"confidence":1,"flipped":false}"#),
            None
        );
    }

    #[test]
    fn a_box_takes_the_words_whose_middles_it_covers() {
        let lines = [level(100.0), level(120.0), level(140.0)];
        // Over the middle third of two lines, across the middle of their small letters; the third
        // line's letters are below it.
        let quad = [233.0, 90.0, 367.0, 90.0, 367.0, 118.0, 233.0, 118.0];
        let pieces = pieces_inside(&lines, &[quad]);
        assert_eq!(pieces.len(), 2);
        assert_eq!((pieces[0].index, pieces[1].index), (0, 1));
        assert!((pieces[0].from - 1.0 / 3.0).abs() < 0.01, "{:?}", pieces[0]);
        assert!((pieces[1].to - 2.0 / 3.0).abs() < 0.01, "{:?}", pieces[1]);
        let read = [
            (
                0,
                LineText {
                    words: vec![
                        word("um", 0.0, 0.2),
                        word("dois", 0.3, 0.5),
                        word("três", 0.7, 0.9),
                    ],
                    confidence: 90.0,
                    flipped: false,
                },
            ),
            (
                1,
                LineText {
                    words: vec![word("quatro", 0.35, 0.55), word("cinco", 0.7, 1.0)],
                    confidence: 90.0,
                    flipped: false,
                },
            ),
        ];
        assert_eq!(pieces_text(&pieces, &read), "dois\nquatro");
    }

    #[test]
    fn a_box_over_a_turned_line_finds_it_along_its_length() {
        // Read upward, as a label beside a figure: from (300, 500) to (300, 100).
        let turned = LinePath {
            baseline: vec![[300.0, 500.0], [300.0, 100.0]],
            ascent: 9.0,
            descent: 3.0,
            x_height: 6.0,
        };
        // Its small letters lie to the left of the baseline: x from 294 to 300.
        let quad = [290.0, 300.0, 299.0, 300.0, 299.0, 520.0, 290.0, 520.0];
        let pieces = pieces_inside(std::slice::from_ref(&turned), &[quad]);
        assert_eq!(pieces.len(), 1);
        assert!(
            pieces[0].from < 0.01 && (pieces[0].to - 0.5).abs() < 0.01,
            "{:?}",
            pieces[0]
        );
        // Beside the baseline on the side of the descenders, nothing.
        let other_side = [301.0, 100.0, 310.0, 100.0, 310.0, 500.0, 301.0, 500.0];
        assert!(pieces_inside(&[turned], &[other_side]).is_empty());
    }

    #[test]
    fn a_layer_copies_as_a_native_selection_does() {
        use musaeum_shm::chars::Glyph;
        let glyph = |ch: char, x: f32, flags: u16| Glyph {
            ch,
            quad: [x, 0.1, x + 0.01, 0.1, x + 0.01, 0.12, x, 0.12],
            flags,
        };
        let generated = musaeum_shm::chars::GENERATED;
        let layer = CharLayer {
            rotation: 0,
            chars: vec![
                glyph('a', 0.10, 0),
                glyph('b', 0.11, 0),
                glyph('\r', 0.12, generated),
                glyph('\n', 0.12, generated),
                glyph('c', 0.13, 0),
            ],
        };
        let all = [[0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0]];
        assert_eq!(layer_text(&layer, &all).as_deref(), Some("ab\nc"));
        let nothing = [[0.5, 0.5, 0.6, 0.5, 0.6, 0.6, 0.5, 0.6]];
        assert_eq!(layer_text(&layer, &nothing), None);
    }

    #[test]
    fn a_quote_is_its_words_with_one_space_between() {
        assert_eq!(
            collapse("  As armas\n e os\tbarões "),
            "As armas e os barões"
        );
    }
}
