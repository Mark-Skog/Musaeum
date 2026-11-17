//! Checks Phases 1 to 3 on a real collection of books, without changing anything in it.
//!
//! ```text
//! corpus-check --corpus DIR --work DIR [--out REPORT.json] [--sample N] [--skip-render]
//!              [--skip-content] [--flow-only]
//! corpus-check --lines --corpus DIR --work DIR [--out REPORT.json] [--sample N] [--pages-per-file N] [--ocr]
//! corpus-check --lines-synthetic [--out REPORT.json] [--pages N]
//! corpus-check --ocr-book --work DIR [--out REPORT.json]
//! corpus-check --inspect BOOK.pdf --at 14,78,100-120 --corpus DIR --work DIR [--ocr] [--out REPORT.json]
//! ```
//!
//! The collection is only ever read. Every file and folder in it, with its size, dates and
//! attributes, is listed before anything else and again at the end, and the run fails if anything
//! differs. The library, its cache and the report go in `--work`, which must be outside the
//! collection. Everything runs below normal priority, and so do the workers it starts.
//!
//! What it does, in order:
//!
//! 1. indexes the whole tree with the library, hashing every book and reading what it says about
//!    itself in the sandboxed worker, which is where `/ID[0]` comes from;
//! 2. measures every page of every PDF in a sandboxed render worker, without drawing anything,
//!    keeps the result in `cache.db` and reads it back;
//! 3. renders a sample of whole pages at the scale a reader would see, chosen for being heavy
//!    scans, mixing page shapes, being long, and being ordinary;
//! 4. takes heavy renders back part way, and kills a worker in the middle of one;
//! 5. classifies every page of every PDF as text, scanned or hybrid, as background work in the
//!    sandboxed workers, keeps the classes in `cache.db`, and draws the thumbnails of the sample
//!    into the store on disk.
//!
//! With `--flow-only`, it does something else: every flowing book (EPUB, MOBI, AZW3, FB2, TXT,
//! Markdown) is normalized twice in the sandboxed ingest worker, kept in the store on disk, and every
//! position in it written as a CFI and a Locator and found again (see `flow.rs`).
//!
//! With `--lines`, the line detector of Phase 5 on the scanned pages of a sample of PDFs, and with
//! `--lines-synthetic` on the labeled corpus, which needs no collection (see `lines.rs`).
//!
//! With `--inspect`, the detector and the OCR on chosen pages of one book of the collection, for
//! looking at (see `inspect.rs`).

mod content;
mod flow;
mod inspect;
mod lines;
mod ocr;
mod render;
mod snapshot;

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use musaeum_core::{Orientation, summarize_orientation};
use musaeum_library::{
    DocumentGeometry, InstanceState, Library, LibraryOptions, LibraryPaths, PageOrientation,
    PageRecord,
};
use serde::Serialize;
use serde_json::json;

use render::OpenFailure;

struct Options {
    corpus: PathBuf,
    work: PathBuf,
    out: Option<PathBuf>,
    sample: usize,
    skip_render: bool,
    skip_content: bool,
    flow_only: bool,
    lines: bool,
    lines_synthetic: bool,
    ocr_book: bool,
    /// With `--lines`: the OCR reads the hybrid pages' body lines too, against their text layer.
    read_lines: bool,
    pages_per_file: u32,
    dump: Option<PathBuf>,
    /// With `--inspect`: the book, and its pages from 0.
    inspect: Option<(PathBuf, Vec<u32>)>,
}

fn parse_options() -> Result<Options, String> {
    let mut corpus = None;
    let mut work = None;
    let mut out = None;
    let mut sample = 10;
    let mut skip_render = false;
    let mut skip_content = false;
    let mut flow_only = false;
    let mut lines = false;
    let mut lines_synthetic = false;
    let mut ocr_book = false;
    let mut read_lines = false;
    let mut pages_per_file = 6;
    let mut dump = None;
    let mut inspect_file = None;
    let mut inspect_pages = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--corpus" => corpus = Some(PathBuf::from(value()?)),
            "--work" => work = Some(PathBuf::from(value()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--sample" => {
                sample = value()?
                    .parse()
                    .map_err(|error| format!("--sample: {error}"))?;
            }
            "--skip-render" => skip_render = true,
            "--skip-content" => skip_content = true,
            "--flow-only" => flow_only = true,
            "--lines" => lines = true,
            "--dump" => dump = Some(PathBuf::from(value()?)),
            "--lines-synthetic" => lines_synthetic = true,
            "--ocr-book" => ocr_book = true,
            "--ocr" => read_lines = true,
            "--inspect" => inspect_file = Some(PathBuf::from(value()?)),
            "--at" => inspect_pages = inspect::parse_pages(&value()?)?,
            "--pages-per-file" | "--pages" => {
                pages_per_file = value()?
                    .parse()
                    .map_err(|error| format!("{flag}: {error}"))?;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if lines_synthetic || ocr_book {
        let work = match work {
            Some(work) if ocr_book => {
                std::fs::create_dir_all(&work)
                    .map_err(|error| format!("the work folder: {error}"))?;
                work
            }
            None if ocr_book => return Err("--work is required".into()),
            _ => PathBuf::new(),
        };
        return Ok(Options {
            corpus: PathBuf::new(),
            work,
            out,
            sample,
            skip_render,
            skip_content,
            flow_only,
            lines,
            lines_synthetic,
            ocr_book,
            read_lines,
            pages_per_file,
            dump,
            inspect: None,
        });
    }
    let corpus = std::fs::canonicalize(corpus.ok_or("--corpus is required")?)
        .map_err(|error| format!("the corpus: {error}"))?;
    let work = work.ok_or("--work is required")?;
    std::fs::create_dir_all(&work).map_err(|error| format!("the work folder: {error}"))?;
    let work = std::fs::canonicalize(work).map_err(|error| format!("the work folder: {error}"))?;
    if work.starts_with(&corpus) || corpus.starts_with(&work) {
        return Err("the work folder must be outside the corpus, and the other way round".into());
    }
    Ok(Options {
        corpus,
        work,
        out,
        sample,
        skip_render,
        skip_content,
        flow_only,
        lines,
        lines_synthetic,
        ocr_book,
        read_lines,
        pages_per_file,
        dump,
        inspect: inspect_file.map(|file| (file, inspect_pages)),
    })
}

/// Below normal priority, which the workers inherit, so a reader using the machine comes first.
fn lower_priority() {
    use windows_sys::Win32::System::Threading::{
        BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, SetPriorityClass,
    };
    // SAFETY: the pseudo-handle of this process is always valid.
    unsafe {
        SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS);
    }
}

#[derive(Debug, Serialize)]
struct PdfReport {
    path: String,
    bytes: u64,
    pdf_id0: Option<String>,
    pages: Option<usize>,
    summary: Option<String>,
    portrait: usize,
    landscape: usize,
    square: usize,
    page_layout: Option<&'static str>,
    direction: Option<&'static str>,
    open_ms: Option<f64>,
    geometry_ms: Option<f64>,
    wall_ms: Option<f64>,
    cached_round_trip: Option<bool>,
    failure: Option<OpenFailure>,
}

fn orientation_of(page: musaeum_shm::PageGeometry) -> PageOrientation {
    match Orientation::of(page) {
        Orientation::Portrait => PageOrientation::Portrait,
        Orientation::Landscape => PageOrientation::Landscape,
        Orientation::Square => PageOrientation::Square,
    }
}

fn main() {
    let options = match parse_options() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    lower_priority();
    let result = if let Some((file, pages)) = &options.inspect {
        inspect::run_inspect(
            &options.corpus,
            &options.work,
            file,
            pages,
            options.read_lines,
            options.out.as_deref(),
        )
    } else if options.ocr_book {
        ocr::run_ocr_book(&options.work, options.out.as_deref())
    } else if options.lines_synthetic {
        lines::run_synthetic(options.out.as_deref(), u64::from(options.pages_per_file))
    } else if options.lines {
        lines::run_lines(
            options.dump.as_deref(),
            &options.corpus,
            &options.work,
            options.out.as_deref(),
            options.sample,
            options.pages_per_file,
            options.read_lines,
        )
    } else if options.flow_only {
        run_flow(&options)
    } else {
        run(&options)
    };
    match result {
        Ok(passed) => std::process::exit(if passed { 0 } else { 1 }),
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    }
}

fn run(options: &Options) -> Result<bool, String> {
    let started = Instant::now();
    eprintln!("listing {} before anything else", options.corpus.display());
    let before = snapshot::take(&options.corpus).map_err(|error| format!("listing: {error}"))?;

    // --- 1. The library ---
    let paths = LibraryPaths::in_directory(&options.work.join("library"));
    let library = Library::open(paths, LibraryOptions::default())
        .map_err(|error| format!("opening the library: {error}"))?;
    let root = library
        .add_root(&options.corpus, &[])
        .map_err(|error| format!("adding the root: {error}"))?;
    eprintln!("indexing");
    let scan_started = Instant::now();
    let scan = library
        .reconcile(root.id, &musaeum_library::ReconcileScope::Everything)
        .map_err(|error| format!("scanning: {error}"))?;
    let scan_ms = scan_started.elapsed().as_secs_f64() * 1000.0;
    let content_started = Instant::now();
    let content = library
        .process_content()
        .map_err(|error| format!("hashing: {error}"))?;
    let content_ms = content_started.elapsed().as_secs_f64() * 1000.0;
    let rescan = library
        .reconcile(root.id, &musaeum_library::ReconcileScope::Everything)
        .map_err(|error| format!("scanning again: {error}"))?;
    let status = library.status().map_err(|error| error.to_string())?;
    let suggestions = library
        .suggestions(true)
        .map_err(|error| error.to_string())?;

    let instances = library
        .instances(None, 1_000_000)
        .map_err(|error| error.to_string())?;
    let mut formats: BTreeMap<String, usize> = BTreeMap::new();
    let mut pdfs = Vec::new();
    let mut dc_identifiers = 0;
    let mut not_present = Vec::new();
    for instance in &instances {
        let document = library
            .document(instance.document_id)
            .map_err(|error| error.to_string())?;
        *formats.entry(document.format.clone()).or_default() += 1;
        if document.dc_identifier.is_some() {
            dc_identifiers += 1;
        }
        if instance.state != InstanceState::Present {
            not_present.push(format!("{} ({})", instance.path.display(), instance.state));
            continue;
        }
        if document.format == "pdf" {
            let bytes = document
                .revisions
                .iter()
                .find(|revision| revision.id == instance.revision_id)
                .map_or(0, |revision| revision.size);
            pdfs.push((instance.path.clone(), bytes, document.pdf_id0.clone()));
        }
    }
    pdfs.sort();
    eprintln!(
        "{} files indexed, {} of them PDFs",
        instances.len(),
        pdfs.len()
    );

    // --- 2. Every page of every PDF ---
    let worker = render::worker_exe();
    let mut reports = Vec::new();
    let mut geometries = BTreeMap::new();
    let mut core_failures = Vec::new();
    for (index, (path, bytes, pdf_id0)) in pdfs.iter().enumerate() {
        eprintln!(
            "[{}/{}] measuring {}",
            index + 1,
            pdfs.len(),
            path.display()
        );
        let mut report = PdfReport {
            path: path
                .strip_prefix(&options.corpus)
                .unwrap_or(path)
                .display()
                .to_string(),
            bytes: *bytes,
            pdf_id0: pdf_id0.clone(),
            pages: None,
            summary: None,
            portrait: 0,
            landscape: 0,
            square: 0,
            page_layout: None,
            direction: None,
            open_ms: None,
            geometry_ms: None,
            wall_ms: None,
            cached_round_trip: None,
            failure: None,
        };
        match render::measure(&worker, path) {
            Ok(measured) => {
                let records: Vec<PageRecord> = measured
                    .pages
                    .iter()
                    .map(|&page| PageRecord {
                        width_pt: page.width_pt,
                        height_pt: page.height_pt,
                        orientation: orientation_of(page),
                    })
                    .collect();
                let geometry = DocumentGeometry {
                    pages: records,
                    page_layout: measured
                        .hints
                        .page_layout
                        .map(|layout| layout.as_str().to_owned()),
                    direction: measured
                        .hints
                        .direction
                        .map(|direction| direction.as_str().to_owned()),
                };
                let count = |orientation| {
                    geometry
                        .pages
                        .iter()
                        .filter(|page| page.orientation == orientation)
                        .count()
                };
                report.pages = Some(measured.pages.len());
                report.summary =
                    Some(format!("{:?}", summarize_orientation(&measured.pages)).to_lowercase());
                report.portrait = count(PageOrientation::Portrait);
                report.landscape = count(PageOrientation::Landscape);
                report.square = count(PageOrientation::Square);
                report.page_layout = measured.hints.page_layout.map(|layout| layout.as_str());
                report.direction = measured.hints.direction.map(|direction| direction.as_str());
                report.open_ms = Some(measured.open_us as f64 / 1000.0);
                report.geometry_ms = Some(measured.geometry_us as f64 / 1000.0);
                report.wall_ms = Some(measured.wall_ms);
                // Kept by the exact bytes, and read back as it was kept.
                if let Ok(Some(file)) = library.opened_file(path)
                    && let Some(hash) = file.blake3
                {
                    let kept = library.store_page_geometry(&hash, &geometry).is_ok()
                        && library.page_geometry(&hash).ok().flatten().as_ref() == Some(&geometry);
                    report.cached_round_trip = Some(kept);
                }
                geometries.insert(path.clone(), measured.pages);
            }
            Err(failure) => {
                if let OpenFailure::Core(message) = &failure {
                    core_failures.push(format!("{}: {message}", path.display()));
                }
                report.failure = Some(failure);
            }
        }
        reports.push(report);
    }

    let chosen = choose_sample(&reports, options.sample);

    // --- 5. What every page is made of, and the sample's thumbnails ---
    let mut contents = Vec::new();
    if !options.skip_content {
        let sampled: HashSet<&str> = chosen
            .iter()
            .map(|(_, relative)| relative.as_str())
            .collect();
        for (index, report) in reports.iter().enumerate() {
            let path = options.corpus.join(&report.path);
            let Some(pages) = geometries.get(&path) else {
                continue;
            };
            let thumbnails = sampled.contains(report.path.as_str());
            eprintln!(
                "[{}/{}] classifying {}{}",
                index + 1,
                reports.len(),
                report.path,
                if thumbnails { ", with thumbnails" } else { "" }
            );
            let hash = library
                .opened_file(&path)
                .ok()
                .flatten()
                .and_then(|file| file.blake3);
            let run = content::classify(&worker, &path, pages, 4, thumbnails, &library, hash);
            if let Err(OpenFailure::Core(message)) = &run {
                core_failures.push(format!("{}: {message}", report.path));
            }
            contents.push(json!({
                "path": report.path,
                "thumbnails": thumbnails,
                "result": run.map_err(|error| format!("{error:?}")),
            }));
        }
    }

    // --- 3 and 4. A sample, rendered, cancelled and killed ---
    let mut samples = Vec::new();
    let mut cancel = json!(null);
    let mut kill = json!(null);
    if !options.skip_render {
        for (reason, relative) in &chosen {
            let path = options.corpus.join(relative);
            let Some(pages) = geometries.get(&path) else {
                continue;
            };
            eprintln!("rendering {relative} ({reason})");
            let rendered = render::render_sample(&worker, &path, pages);
            if let Err(OpenFailure::Core(message)) = &rendered {
                core_failures.push(format!("{relative}: {message}"));
            }
            samples.push(json!({ "path": relative, "why": reason, "result": rendered.map_err(|error| format!("{error:?}")) }));
        }
        if let Some((_, heaviest)) = chosen
            .iter()
            .find(|(reason, _)| reason.starts_with("heavy scan"))
        {
            let path = options.corpus.join(heaviest);
            if let Some(pages) = geometries.get(&path) {
                eprintln!("cancelling renders of {heaviest}");
                let result = render::cancel_mid_render(&worker, &path, pages, 10);
                if let Err(OpenFailure::Core(message)) = &result {
                    core_failures.push(format!("{heaviest}: {message}"));
                }
                cancel = json!({ "path": heaviest, "result": result.map_err(|error| format!("{error:?}")) });
                eprintln!("killing workers reading {heaviest}");
                let result = render::kill_while_reading(&worker, &path, pages, 6);
                if let Err(OpenFailure::Core(message)) = &result {
                    core_failures.push(format!("{heaviest}: {message}"));
                }
                kill = json!({ "path": heaviest, "result": result.map_err(|error| format!("{error:?}")) });
            }
        }
    }
    drop(library);

    eprintln!("listing {} again", options.corpus.display());
    let after =
        snapshot::take(&options.corpus).map_err(|error| format!("listing again: {error}"))?;
    let comparison = snapshot::compare(&before, &after);

    let with_id = reports
        .iter()
        .filter(|report| report.pdf_id0.is_some())
        .count();
    let refused = reports
        .iter()
        .filter(|report| matches!(report.failure, Some(OpenFailure::Refused { .. })))
        .count();
    let mixed = reports
        .iter()
        .filter(|report| report.summary.as_deref() == Some("mixed"))
        .count();
    let pages_total: usize = reports.iter().filter_map(|report| report.pages).sum();
    let mut layouts: BTreeMap<&str, usize> = BTreeMap::new();
    for report in &reports {
        *layouts
            .entry(report.page_layout.unwrap_or("(none)"))
            .or_default() += 1;
    }
    let mut ids: BTreeMap<&str, usize> = BTreeMap::new();
    for report in &reports {
        if let Some(id) = &report.pdf_id0 {
            *ids.entry(id).or_default() += 1;
        }
    }
    let shared_ids: Vec<_> = ids.iter().filter(|(_, count)| **count > 1).collect();

    let passed = comparison.unchanged() && core_failures.is_empty();
    let report = json!({
        "corpus": options.corpus.display().to_string(),
        "work": options.work.display().to_string(),
        "passed": passed,
        "read_only": comparison,
        "unreadable_directories": before.unreadable_dirs.len(),
        "elapsed_s": started.elapsed().as_secs_f64(),
        "library": {
            "scan_ms": scan_ms,
            "content_ms": content_ms,
            "scan": scan,
            "content": content,
            "rescan": rescan,
            "status": {
                "works": status.works,
                "documents": status.documents,
                "revisions": status.revisions,
                "unconfirmed_revisions": status.unconfirmed_revisions,
                "present": status.present,
                "absent": status.absent,
                "cloud_only": status.cloud_only,
                "unreadable": status.unreadable,
                "open_suggestions": status.open_suggestions,
            },
            "formats": formats,
            "not_present": not_present,
            "suggestions": suggestions.iter().map(|suggestion| json!({
                "kind": suggestion.kind,
                "evidence": suggestion.evidence,
            })).collect::<Vec<_>>(),
            "epubs_with_dc_identifier": dc_identifiers,
        },
        "pdf": {
            "count": reports.len(),
            "with_id0": with_id,
            "id0_shared_by_several_files": shared_ids,
            "pages": pages_total,
            "mixed_documents": mixed,
            "refused_by_pdfium": refused,
            "page_layouts": layouts,
            "documents": reports,
        },
        "samples": samples,
        "cancel": cancel,
        "kill": kill,
        "content": contents,
        "core_failures": core_failures,
    });
    let text = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
    let out = options
        .out
        .clone()
        .unwrap_or_else(|| options.work.join("corpus-report.json"));
    std::fs::write(&out, text).map_err(|error| format!("writing {}: {error}", out.display()))?;
    eprintln!(
        "{} files, {} folders, {:.1} GiB; unchanged: {}; access times updated by Windows: {}",
        comparison.files,
        comparison.directories,
        comparison.bytes as f64 / f64::from(1u32 << 30),
        comparison.unchanged(),
        comparison.access_time_updated
    );
    eprintln!(
        "{} PDFs, {pages_total} pages, {with_id} with /ID, {mixed} mixed, {refused} refused by PDFium; Core failures: {}",
        reports.len(),
        core_failures.len()
    );
    eprintln!("report: {}", out.display());
    Ok(passed)
}

/// Every flowing book in the corpus, normalized and checked; the corpus listed before and after.
fn run_flow(options: &Options) -> Result<bool, String> {
    let started = Instant::now();
    eprintln!("listing {} before anything else", options.corpus.display());
    let before = snapshot::take(&options.corpus).map_err(|error| format!("listing: {error}"))?;
    let books = flow::find_books(&options.corpus);
    let store = musaeum_library::FlowStore::new(&options.work.join("cache"));
    let mut client = flow::client();
    let mut reports = Vec::new();
    let mut core_failures = Vec::new();
    for path in &books {
        eprintln!("flow: {}", path.display());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            flow::check_book(&mut client, &store, path)
        }));
        match result {
            Ok(report) => {
                eprintln!(
                    "  {} documents, {} parts, {} positions ({} failed), source CFIs {}/{}, {:.0} ms{}",
                    report.documents,
                    report.parts,
                    report.positions,
                    report.position_failures,
                    report.source_matches,
                    report.source_positions,
                    report.normalize_ms,
                    report
                        .failure
                        .as_deref()
                        .map(|f| format!(", FAILED: {f}"))
                        .unwrap_or_default()
                );
                reports.push(report);
            }
            Err(_) => core_failures.push(path.display().to_string()),
        }
    }
    let restarts = client.restarts();
    drop(client);

    eprintln!("listing {} again", options.corpus.display());
    let after =
        snapshot::take(&options.corpus).map_err(|error| format!("listing again: {error}"))?;
    let comparison = snapshot::compare(&before, &after);
    let failed: Vec<&str> = reports
        .iter()
        .filter(|report| report.failure.is_some())
        .map(|report| report.path.as_str())
        .collect();
    let sum = |value: fn(&flow::BookReport) -> u64| reports.iter().map(value).sum::<u64>();
    let positions = sum(|r| r.positions);
    let position_failures = sum(|r| r.position_failures);
    let not_well_formed = sum(|r| r.parts_not_well_formed as u64);
    let danger = sum(|r| r.parts_with_danger as u64);
    let nondeterministic = reports
        .iter()
        .filter(|r| r.failure.is_none() && !r.deterministic)
        .count();
    let passed = comparison.unchanged()
        && core_failures.is_empty()
        && restarts == 0
        && failed.is_empty()
        && position_failures == 0
        && not_well_formed == 0
        && danger == 0
        && nondeterministic == 0;
    let report = json!({
        "passed": passed,
        "elapsed_s": started.elapsed().as_secs_f64(),
        "corpus": options.corpus.display().to_string(),
        "work": options.work.display().to_string(),
        "read_only": comparison,
        "unreadable_directories": before.unreadable_dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>(),
        "normalizer": musaeum_flow::NORMALIZER_VERSION,
        "books": reports.len(),
        "failed": failed,
        "worker_restarts": restarts,
        "core_failures": core_failures,
        "positions": positions,
        "position_failures": position_failures,
        "parts_not_well_formed": not_well_formed,
        "parts_with_danger": danger,
        "nondeterministic": nondeterministic,
        "source_positions": sum(|r| r.source_positions),
        "source_matches": sum(|r| r.source_matches),
        "quotes_tried": sum(|r| r.quotes_tried),
        "quotes_exact": sum(|r| r.quotes_exact),
        "reports": reports,
    });
    let text = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
    let out = options
        .out
        .clone()
        .unwrap_or_else(|| options.work.join("flow-report.json"));
    std::fs::write(&out, text).map_err(|error| format!("writing {}: {error}", out.display()))?;
    eprintln!(
        "{} files, {} folders, {:.1} GiB; unchanged: {}; access times updated by Windows: {}",
        comparison.files,
        comparison.directories,
        comparison.bytes as f64 / f64::from(1u32 << 30),
        comparison.unchanged(),
        comparison.access_time_updated
    );
    eprintln!(
        "{} flowing books, {} failed; {positions} positions, {position_failures} failed; worker restarts {restarts}; Core failures {}",
        reports.len(),
        failed.len(),
        core_failures.len()
    );
    eprintln!("report: {}", out.display());
    Ok(passed)
}

/// Picks documents worth rendering: the heaviest scans per page, the most mixed, the longest, and
/// some ordinary ones, each once.
fn choose_sample(reports: &[PdfReport], size: usize) -> Vec<(String, String)> {
    let measured: Vec<&PdfReport> = reports
        .iter()
        .filter(|report| report.pages.is_some())
        .collect();
    let per_page =
        |report: &&PdfReport| report.bytes as f64 / report.pages.unwrap_or(1).max(1) as f64;
    let mut chosen: Vec<(String, String)> = Vec::new();
    let take = |reason: String, report: &PdfReport, chosen: &mut Vec<(String, String)>| {
        if chosen.len() < size && !chosen.iter().any(|(_, path)| *path == report.path) {
            chosen.push((reason, report.path.clone()));
        }
    };

    let mut heavy = measured.clone();
    heavy.sort_by(|a, b| per_page(b).total_cmp(&per_page(a)));
    for report in heavy.iter().take(3) {
        take(
            format!("heavy scan, {:.0} KiB a page", per_page(report) / 1024.0),
            report,
            &mut chosen,
        );
    }
    let mut mixed: Vec<&&PdfReport> = measured
        .iter()
        .filter(|report| report.summary.as_deref() == Some("mixed"))
        .collect();
    mixed.sort_by_key(|report| std::cmp::Reverse(report.landscape.min(report.portrait)));
    for report in mixed.iter().take(3) {
        take(
            format!(
                "mixed: {} portrait, {} landscape, {} square",
                report.portrait, report.landscape, report.square
            ),
            report,
            &mut chosen,
        );
    }
    if let Some(longest) = measured.iter().max_by_key(|report| report.pages) {
        take(
            format!("longest, {} pages", longest.pages.unwrap_or(0)),
            longest,
            &mut chosen,
        );
    }
    for report in measured
        .iter()
        .filter(|report| report.page_layout.is_some() || report.direction.is_some())
        .take(1)
    {
        take("asks for a layout".to_owned(), report, &mut chosen);
    }
    let mut ordinary = measured.clone();
    ordinary.sort_by(|a, b| per_page(a).total_cmp(&per_page(b)));
    let middle = ordinary.len() / 2;
    for report in ordinary.iter().skip(middle).take(size) {
        take("ordinary".to_owned(), report, &mut chosen);
    }
    chosen
}
