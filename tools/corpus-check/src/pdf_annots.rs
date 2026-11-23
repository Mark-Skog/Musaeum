//! Phase 7 on the real collection, which it only reads: every PDF's own annotations read by the
//! sandboxed annotations worker, with both of its readers where both can, and copies of a sample
//! written into the work folder with annotations added, read back by PDFium.
//!
//! A copy is made the way the app makes one: the book's bytes, copied and hashed, then the update
//! the worker wrote. Nothing is ever written in the collection, which is listed before and after.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use musaeum_pdf_annot::client::{AnnotClient, AnnotWorkerConfig};
use musaeum_pdf_annot::model::{Annot, FLAG_PRINT, Subtype};
use musaeum_pdf_annot::protocol::{Found, Reader};
use musaeum_pdf_annot::write::WriteOptions;
use serde::Serialize;
use serde_json::json;

use crate::snapshot;

#[derive(Debug, Default, Serialize)]
struct FileReport {
    path: String,
    bytes: u64,
    pages: usize,
    encrypted: bool,
    /// Why no update can be written to it, when none can.
    not_writable: Option<String>,
    /// The annotations Musaeum keeps, by kind, as read with the reader the app uses.
    kept: BTreeMap<String, u32>,
    /// The others, by kind.
    skipped: BTreeMap<String, u32>,
    /// Whether this crate's reader and PDFium's found the same annotations, where both read them.
    readers_agree: Option<bool>,
    read_ms: f64,
    failure: Option<String>,
    /// For the sample: the copy written with annotations added.
    copy: Option<CopyReport>,
}

#[derive(Debug, Default, Serialize)]
struct CopyReport {
    added: usize,
    tail_bytes: usize,
    export_ms: f64,
    /// Every annotation added found by PDFium in the copy, of its kind, at its place.
    found: bool,
    /// Every annotation the file had still in the copy.
    kept_existing: bool,
    /// The copy begins with every byte of the book.
    prefix_identical: bool,
    failure: Option<String>,
}

fn summary(found: &Found) -> BTreeMap<String, u32> {
    let mut kinds = BTreeMap::new();
    for annot in &found.reading.annots {
        *kinds
            .entry(annot.subtype.map_or("?", Subtype::name).to_owned())
            .or_insert(0) += 1;
    }
    kinds
}

fn pdfs_under(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(folder) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Annotations to add to a book: a highlight near the top of its first page, a box on its last,
/// a stroke on its middle one, and a note with accents and characters outside PDFDocEncoding.
fn test_marks(pages: usize) -> Vec<Annot> {
    let base = Annot {
        opacity: 1.0,
        flags: FLAG_PRINT,
        created: Some(1_795_437_296_000),
        modified: Some(1_795_437_296_000),
        ..Annot::default()
    };
    let last = pages.saturating_sub(1) as u32;
    let middle = (pages / 2) as u32;
    vec![
        Annot {
            page: 0,
            subtype: Some(Subtype::Highlight),
            rect: [0.1, 0.1, 0.4, 0.12],
            quads: vec![[0.1, 0.1, 0.4, 0.1, 0.4, 0.12, 0.1, 0.12]],
            color: Some([1.0, 0.89, 0.42]),
            contents: "Teste: ação, coração — “aspas” → ✓".into(),
            name: Some("019ab7c2-0000-7000-8000-c0a0c0a00001".into()),
            ..base.clone()
        },
        Annot {
            page: last,
            subtype: Some(Subtype::Square),
            rect: [0.2, 0.2, 0.5, 0.3],
            color: Some([0.66, 0.9, 0.63]),
            interior: Some([0.66, 0.9, 0.63]),
            width: Some(0.0),
            name: Some("019ab7c2-0000-7000-8000-c0a0c0a00002".into()),
            ..base.clone()
        },
        Annot {
            page: middle,
            subtype: Some(Subtype::Ink),
            rect: [0.3, 0.5, 0.6, 0.55],
            ink: vec![vec![0.3, 0.5, 0.45, 0.55, 0.6, 0.52]],
            color: Some([0.78, 0.71, 0.99]),
            width: Some(9.0),
            name: Some("019ab7c2-0000-7000-8000-c0a0c0a00003".into()),
            ..base
        },
    ]
}

fn copy_of(
    client: &mut AnnotClient,
    book: &Path,
    before: &Found,
    work: &Path,
    index: usize,
) -> CopyReport {
    let mut report = CopyReport::default();
    let marks = test_marks(before.reading.frames.len());
    let started = Instant::now();
    let result = (|| -> Result<(), String> {
        let mut source = File::open(book).map_err(|error| error.to_string())?;
        let exported = client.export(&source, marks.clone(), WriteOptions::default())?;
        report.export_ms = started.elapsed().as_secs_f64() * 1000.0;
        report.added = exported.written.len();
        report.tail_bytes = exported.tail.len();
        let path = work.join(format!("copia-{index:03}.pdf"));
        let mut out = File::create(&path).map_err(|error| error.to_string())?;
        let mut original = blake3::Hasher::new();
        let mut buffer = vec![0u8; 1 << 20];
        let mut length = 0u64;
        loop {
            let read = source
                .read(&mut buffer)
                .map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            original.update(&buffer[..read]);
            out.write_all(&buffer[..read])
                .map_err(|error| error.to_string())?;
            length += read as u64;
        }
        out.write_all(&exported.tail)
            .and_then(|()| out.sync_all())
            .map_err(|error| error.to_string())?;
        drop(out);
        let written = std::fs::read(&path).map_err(|error| error.to_string())?;
        report.prefix_identical = length == exported.base
            && blake3::hash(&written[..length as usize]) == original.finalize();
        let copy = File::open(&path).map_err(|error| error.to_string())?;
        let after = client.read(&copy, Reader::Pdfium)?;
        report.found = marks.iter().all(|mark| {
            after.reading.annots.iter().any(|annot| {
                annot.name == mark.name
                    && annot.page == mark.page
                    && annot.subtype == mark.subtype
                    && annot.contents == mark.contents
                    && annot
                        .quads
                        .iter()
                        .flatten()
                        .zip(mark.quads.iter().flatten())
                        .all(|(a, b)| (a - b).abs() < 1e-3)
            })
        });
        report.kept_existing = before.reading.annots.iter().all(|old| {
            after.reading.annots.iter().any(|annot| {
                annot.page == old.page && annot.subtype == old.subtype && annot.name == old.name
            })
        });
        let _ = std::fs::remove_file(&path);
        Ok(())
    })();
    report.failure = result.err();
    report
}

pub fn run_pdf_annots(
    corpus: &Path,
    work: &Path,
    out: Option<&Path>,
    sample: usize,
) -> Result<bool, String> {
    let before = snapshot::take(corpus).map_err(|error| format!("listing the corpus: {error}"))?;
    let exe = std::env::current_exe()
        .map_err(|error| error.to_string())?
        .with_file_name("musaeum-pdf-annots.exe");
    let mut client = AnnotClient::new(AnnotWorkerConfig {
        exe: Some(exe),
        ..AnnotWorkerConfig::default()
    });
    let copies = work.join("copias");
    std::fs::create_dir_all(&copies).map_err(|error| error.to_string())?;
    let files = pdfs_under(corpus);
    let started = Instant::now();
    let mut reports = Vec::with_capacity(files.len());
    let mut written = 0;
    for (index, path) in files.iter().enumerate() {
        let mut report = FileReport {
            path: path
                .strip_prefix(corpus)
                .unwrap_or(path)
                .display()
                .to_string(),
            bytes: std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0),
            ..FileReport::default()
        };
        let read_started = Instant::now();
        let found = File::open(path)
            .map_err(|error| error.to_string())
            .and_then(|file| client.read(&file, Reader::Auto));
        report.read_ms = read_started.elapsed().as_secs_f64() * 1000.0;
        match found {
            Ok(found) => {
                report.pages = found.reading.frames.len();
                report.encrypted = found.encrypted;
                report.not_writable = found.not_writable.clone();
                report.kept = summary(&found);
                report.skipped = found.reading.skipped.clone();
                // Both readers, where both can read: the same annotations, by kind, page and name.
                if found.not_writable.is_none() && !found.reading.annots.is_empty() {
                    let pdfium = File::open(path)
                        .map_err(|error| error.to_string())
                        .and_then(|file| client.read(&file, Reader::Pdfium));
                    report.readers_agree = Some(pdfium.is_ok_and(|pdfium| {
                        let key = |annot: &Annot| {
                            (
                                annot.page,
                                annot.subtype.map(Subtype::name),
                                annot.name.clone(),
                                annot.contents.clone(),
                            )
                        };
                        let mut a: Vec<_> = found.reading.annots.iter().map(key).collect();
                        let mut b: Vec<_> = pdfium.reading.annots.iter().map(key).collect();
                        a.sort();
                        b.sort();
                        a == b
                    }));
                }
                let worth = found.not_writable.is_none()
                    && report.pages > 0
                    && (written < sample || !found.reading.annots.is_empty());
                if worth {
                    report.copy = Some(copy_of(&mut client, path, &found, &copies, index));
                    written += 1;
                }
            }
            Err(error) => report.failure = Some(error),
        }
        println!(
            "{:>3}/{} {} {} pages, kept {:?}, skipped {:?}{}{}",
            index + 1,
            files.len(),
            report.path,
            report.pages,
            report.kept,
            report.skipped,
            report
                .not_writable
                .as_ref()
                .map_or(String::new(), |why| format!(" [not writable: {why}]")),
            report.copy.as_ref().map_or(String::new(), |copy| format!(
                " copy: found {} kept {} prefix {}{}",
                copy.found,
                copy.kept_existing,
                copy.prefix_identical,
                copy.failure
                    .as_ref()
                    .map_or(String::new(), |why| format!(" FAILED {why}"))
            )),
        );
        reports.push(report);
    }
    let after = snapshot::take(corpus).map_err(|error| format!("listing the corpus: {error}"))?;
    let comparison = snapshot::compare(&before, &after);
    let copies_made: Vec<&CopyReport> = reports
        .iter()
        .filter_map(|report| report.copy.as_ref())
        .collect();
    let copies_fine = copies_made.iter().all(|copy| {
        copy.failure.is_none() && copy.found && copy.kept_existing && copy.prefix_identical
    });
    let unreadable = reports
        .iter()
        .filter(|report| report.failure.is_some())
        .count();
    let mut kinds: BTreeMap<String, u32> = BTreeMap::new();
    let mut others: BTreeMap<String, u32> = BTreeMap::new();
    for report in &reports {
        for (kind, count) in &report.kept {
            *kinds.entry(kind.clone()).or_default() += count;
        }
        for (kind, count) in &report.skipped {
            *others.entry(kind.clone()).or_default() += count;
        }
    }
    let disagreements = reports
        .iter()
        .filter(|report| report.readers_agree == Some(false))
        .count();
    let summary = json!({
        "files": reports.len(),
        "unreadable": unreadable,
        "encrypted": reports.iter().filter(|report| report.encrypted).count(),
        "not_writable": reports.iter().filter(|report| report.not_writable.is_some()).count(),
        "with_annotations": reports.iter().filter(|report| !report.kept.is_empty()).count(),
        "kept": kinds,
        "skipped": others,
        "readers_compared": reports.iter().filter(|report| report.readers_agree.is_some()).count(),
        "readers_disagree": disagreements,
        "copies": copies_made.len(),
        "copies_fine": copies_fine,
        "seconds": started.elapsed().as_secs_f64(),
        "corpus_unchanged": comparison.unchanged(),
        "corpus": comparison,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&summary).unwrap_or_default()
    );
    if let Some(out) = out {
        let full = json!({ "summary": summary, "files": reports });
        std::fs::write(out, serde_json::to_vec_pretty(&full).unwrap_or_default())
            .map_err(|error| format!("writing the report: {error}"))?;
    }
    Ok(comparison.unchanged() && copies_fine && disagreements == 0)
}
