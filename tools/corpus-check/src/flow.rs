//! Flowing books in a real collection: each one normalized in the sandboxed worker, twice, and then
//! every position in it written as a CFI and a Locator and found again.
//!
//! For an EPUB the check goes one step further: the source documents are parsed here, as they are,
//! without anything taken out, and every canonical CFI written on the normalized document is looked
//! for in them. Where both lead to the same text, a CFI from Musaeum means the same place in any
//! other reader. That parse of the books happens in this process, which is a test tool; the app
//! itself only ever reads a book in the worker.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Instant;

use musaeum_flow::cfi::{self, Point};
use musaeum_flow::dom::NodeId;
use musaeum_flow::locator::{self, Found, LogicalDocument};
use musaeum_flow::package::{Manifest, NavPoint};
use musaeum_flow::sanitize::find_danger;
use musaeum_flow::split::PART_BUDGET;
use musaeum_flow::{NORMALIZER_VERSION, xml};
use musaeum_ingest::protocol::Format;
use musaeum_library::{FlowStore, IngestClient, IngestConfig, NormalizeOutcome, NormalizedFiles};
use musaeum_sandbox::Policy;
use serde::Serialize;

/// The extensions of the formats that flow.
const EXTENSIONS: [&str; 9] = [
    "epub", "mobi", "prc", "azw", "azw3", "fb2", "txt", "md", "markdown",
];

/// Everything found in one book.
#[derive(Debug, Default, Serialize)]
pub struct BookReport {
    pub path: String,
    pub bytes: u64,
    pub format: String,
    pub failure: Option<String>,
    pub normalize_ms: f64,
    pub normalize_again_ms: f64,
    pub deterministic: bool,
    pub title: Option<String>,
    pub creators: Vec<String>,
    pub language: Option<String>,
    pub identifiers: usize,
    pub has_cover: bool,
    pub source_version: Option<String>,
    pub fixed_layout: bool,
    pub toc_entries: usize,
    pub toc_with_cfi: usize,
    pub page_list: usize,
    pub landmarks: usize,
    pub documents: u32,
    pub parts: u32,
    pub split_documents: u32,
    pub oversize_parts: u32,
    pub largest_part: u64,
    pub largest_document: u64,
    pub html_fallbacks: u32,
    pub resources: u32,
    pub resource_bytes: u64,
    pub dropped_files: u32,
    pub warnings: usize,
    pub sanitized: musaeum_flow::sanitize::Report,
    /// Parts that are not well-formed XML when read back: should be 0.
    pub parts_not_well_formed: usize,
    /// Parts in which the independent check still finds something that could run or load: 0.
    pub parts_with_danger: usize,
    pub danger: Vec<String>,
    /// Positions written as CFI and Locator and found again.
    pub positions: u64,
    pub position_failures: u64,
    pub position_examples: Vec<String>,
    /// Positions found again by their quote alone, with the CFI made useless.
    pub quotes_tried: u64,
    pub quotes_exact: u64,
    pub quotes_elsewhere: u64,
    pub quotes_not_found: u64,
    /// EPUB only: positions whose canonical CFI leads to the same text in the source document.
    pub source_documents_compared: usize,
    pub source_documents_not_xml: usize,
    pub source_positions: u64,
    pub source_matches: u64,
    pub source_examples: Vec<String>,
    pub check_ms: f64,
}

/// Every flowing book under `root`, sorted, the way the listing walks it.
pub fn find_books(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

fn format_of(path: &Path) -> Option<Format> {
    Format::from_extension(&path.extension()?.to_str()?.to_ascii_lowercase())
}

/// The worker in the strictest policy, with room for a whole book.
pub fn client() -> IngestClient {
    IngestClient::new(IngestConfig {
        policy: Policy::LockdownParser,
        ..IngestConfig::for_normalizing()
    })
}

fn documents(
    normalized: &NormalizedFiles,
    report: &mut BookReport,
) -> Vec<Option<LogicalDocument>> {
    let file = |path: &str| {
        normalized
            .files
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, d)| d.as_slice())
    };
    normalized
        .manifest
        .documents
        .iter()
        .enumerate()
        .map(|(index, document)| {
            let mut parts = Vec::new();
            for part in &document.parts {
                let text = file(&part.href)
                    .map(String::from_utf8_lossy)
                    .unwrap_or_default();
                match xml::parse(&text, &xml::Limits::default()) {
                    Ok(dom) => {
                        if let Some(danger) = find_danger(&dom) {
                            report.parts_with_danger += 1;
                            if report.danger.len() < 5 {
                                report.danger.push(format!("{}: {danger}", part.href));
                            }
                        }
                        parts.push((part.start.clone(), dom));
                    }
                    Err(_) => report.parts_not_well_formed += 1,
                }
            }
            if parts.len() != document.parts.len() {
                return None;
            }
            LogicalDocument::assemble(index, parts).ok()
        })
        .collect()
}

/// Every text node and every element of every document, as a CFI and a Locator, and back.
fn check_positions(
    manifest: &Manifest,
    documents: &[Option<LogicalDocument>],
    report: &mut BookReport,
) {
    for document in documents.iter().flatten() {
        let Some(body) = document.body else { continue };
        let nodes: Vec<NodeId> = document.dom.descendants(body).collect();
        let mut text_points = 0u64;
        for node in nodes {
            let points: Vec<Point> = match document.dom.text(node) {
                Some(text) => {
                    let length = cfi::utf16_len(text);
                    let middle = (0..=length / 2)
                        .rev()
                        .find(|&o| cfi::utf16_to_byte(text, o).is_some())
                        .unwrap_or(0);
                    let mut points = vec![
                        Point::Text { node, offset: 0 },
                        Point::Text {
                            node,
                            offset: length,
                        },
                    ];
                    if middle > 0 && middle < length {
                        points.push(Point::Text {
                            node,
                            offset: middle,
                        });
                    }
                    points
                }
                None => vec![Point::Element(node)],
            };
            for point in points {
                report.positions += 1;
                let failure = (|| -> Result<(), String> {
                    let locator =
                        locator::locate(manifest, document, point).map_err(|e| e.to_string())?;
                    let json = serde_json::to_string(&locator).map_err(|e| e.to_string())?;
                    let read: locator::Locator =
                        serde_json::from_str(&json).map_err(|e| e.to_string())?;
                    let (found, how) =
                        locator::resolve(&read, document).map_err(|e| e.to_string())?;
                    if how != Found::Cfi || found != point {
                        return Err(format!("{} came back as {how:?}", locator.locations.cfi));
                    }
                    if locator::locate(manifest, document, found).map_err(|e| e.to_string())?
                        != locator
                    {
                        return Err(format!("{}: the Locator changed", locator.locations.cfi));
                    }
                    let reader = locator::canonical_to_reader(manifest, &locator.locations.cfi)
                        .map_err(|e| e.to_string())?;
                    if locator::reader_to_canonical(manifest, &reader).map_err(|e| e.to_string())?
                        != locator.locations.cfi
                    {
                        return Err(format!(
                            "{}: the reader's CFI {reader} does not come back",
                            locator.locations.cfi
                        ));
                    }
                    // Some positions are found again by their quote alone.
                    if matches!(point, Point::Text { .. }) {
                        text_points += 1;
                        if text_points.is_multiple_of(97) {
                            report.quotes_tried += 1;
                            let mut lost = locator.clone();
                            lost.normalizer = NORMALIZER_VERSION + 1;
                            lost.locations.cfi = "epubcfi(/6/2!/0)".to_owned();
                            match locator::resolve(&lost, document) {
                                Ok((found, Found::Quote)) => {
                                    if document.text_offset(found).ok()
                                        == document.text_offset(point).ok()
                                    {
                                        report.quotes_exact += 1;
                                    } else {
                                        report.quotes_elsewhere += 1;
                                    }
                                }
                                _ => report.quotes_not_found += 1,
                            }
                        }
                    }
                    Ok(())
                })();
                if let Err(message) = failure {
                    report.position_failures += 1;
                    if report.position_examples.len() < 5 {
                        report.position_examples.push(message);
                    }
                }
            }
        }
    }
}

/// For an EPUB: the canonical CFIs of the normalized documents, looked for in the source documents.
fn check_against_source(
    bytes: &[u8],
    documents: &[Option<LogicalDocument>],
    report: &mut BookReport,
) {
    let Ok(source) =
        musaeum_flow::formats::epub::read(bytes, &musaeum_flow::source::Limits::default())
    else {
        return;
    };
    for (index, original) in source.documents.iter().enumerate() {
        let Some(Some(document)) = documents.get(index) else {
            continue;
        };
        if original.parsed != musaeum_flow::source::Parsed::Xml {
            report.source_documents_not_xml += 1;
            continue;
        }
        report.source_documents_compared += 1;
        let Some(body) = document.body else { continue };
        for node in document.dom.descendants(body) {
            let Some(text) = document.dom.text(node) else {
                continue;
            };
            report.source_positions += 1;
            let (steps, _) = cfi::point_to_path(&document.dom, Point::Text { node, offset: 0 });
            let length = cfi::utf16_len(text);
            match cfi::resolve(&original.dom, &steps, Some(length)) {
                Ok((
                    Point::Text {
                        node: there,
                        offset,
                    },
                    _,
                )) if original.dom.text(there) == Some(text) && offset == length => {
                    report.source_matches += 1;
                }
                other => {
                    if report.source_examples.len() < 5 {
                        let path: Vec<String> = steps.iter().map(|s| s.index.to_string()).collect();
                        report.source_examples.push(format!(
                            "{} /{}: {:?} -> {:?}",
                            original.path,
                            path.join("/"),
                            text.chars().take(30).collect::<String>(),
                            other.map(|(p, _)| match p {
                                Point::Text { node, .. } => original
                                    .dom
                                    .text(node)
                                    .unwrap_or("")
                                    .chars()
                                    .take(30)
                                    .collect::<String>(),
                                other => format!("{other:?}"),
                            })
                        ));
                    }
                }
            }
        }
    }
}

fn count_cfi(points: &[NavPoint]) -> usize {
    points
        .iter()
        .map(|point| usize::from(point.cfi.is_some()) + count_cfi(&point.children))
        .sum()
}

/// Normalizes and checks one book. Nothing is written but into `store`.
pub fn check_book(client: &mut IngestClient, store: &FlowStore, path: &Path) -> BookReport {
    let mut report = BookReport {
        path: path.display().to_string(),
        ..BookReport::default()
    };
    let Some(format) = format_of(path) else {
        report.failure = Some("unknown extension".to_owned());
        return report;
    };
    report.format = format!("{format:?}");
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            report.failure = Some(format!("reading: {error}"));
            return report;
        }
    };
    report.bytes = bytes.len() as u64;
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            report.failure = Some(format!("opening: {error}"));
            return report;
        }
    };
    let started = Instant::now();
    let first = client.normalize(&file, format);
    report.normalize_ms = started.elapsed().as_secs_f64() * 1000.0;
    let normalized = match first {
        NormalizeOutcome::Normalized(normalized) => normalized,
        NormalizeOutcome::Nothing(reason) | NormalizeOutcome::Unavailable(reason) => {
            report.failure = Some(reason);
            return report;
        }
    };
    let started = Instant::now();
    let second = client.normalize(&file, format);
    report.normalize_again_ms = started.elapsed().as_secs_f64() * 1000.0;
    report.deterministic = match &second {
        NormalizeOutcome::Normalized(again) => again.files == normalized.files,
        NormalizeOutcome::Nothing(reason) | NormalizeOutcome::Unavailable(reason) => {
            report.failure = Some(format!("the second normalization failed: {reason}"));
            false
        }
    };
    drop(file);

    let hash = *blake3::hash(&bytes).as_bytes();
    if let Err(error) = store.put(&hash, &normalized.files) {
        report.failure = Some(format!("keeping it: {error}"));
    }

    let manifest = &normalized.manifest;
    let m = &manifest.metadata;
    report.title = m.title().map(str::to_owned);
    report.creators = m.creators.iter().map(|c| c.name.clone()).collect();
    report.language = m.languages.first().cloned();
    report.identifiers = m.identifiers.len();
    report.has_cover = m.cover.is_some();
    report.source_version = manifest.source.version.clone();
    report.fixed_layout = manifest.source.fixed_layout;
    report.toc_entries = NavPoint::count(&manifest.toc);
    report.toc_with_cfi = count_cfi(&manifest.toc);
    report.page_list = NavPoint::count(&manifest.page_list);
    report.landmarks = NavPoint::count(&manifest.landmarks);
    let stats = &manifest.stats;
    report.documents = stats.documents;
    report.parts = stats.parts;
    report.split_documents = stats.split_documents;
    report.oversize_parts = stats.oversize_parts;
    report.largest_part = stats.largest_part;
    report.largest_document = stats.largest_document;
    report.html_fallbacks = stats.html_fallbacks;
    report.resources = stats.resources;
    report.resource_bytes = stats.resource_bytes;
    report.dropped_files = stats.dropped_files;
    report.warnings = manifest.warnings.len();
    report.sanitized = stats.sanitized;

    let started = Instant::now();
    let documents = documents(&normalized, &mut report);
    check_positions(manifest, &documents, &mut report);
    if format == Format::Epub {
        check_against_source(&bytes, &documents, &mut report);
    }
    report.check_ms = started.elapsed().as_secs_f64() * 1000.0;
    if report.largest_part as usize > PART_BUDGET
        && report.oversize_parts == 0
        && report.parts > report.documents
    {
        report
            .failure
            .get_or_insert_with(|| "a part over the budget is not counted as oversize".to_owned());
    }
    report
}
