//! The line detector of Phase 5 on real scanned pages and on the labeled corpus.
//!
//! `--lines`: a sample of the collection's PDFs, read only. For each, the sampled pages are
//! classified and drawn for the detector by the same sandboxed pool and background work the reader
//! uses; every page that is a picture (scanned, or scanned with OCR's text over it) is analyzed
//! and timed. On a hybrid page the OCR text layer has lines of its own, found by another program
//! from the same picture: the detector's lines are scored against them, an independent check on
//! real scans, which no labels exist for. The models are written to the work folder, for the
//! compositor's snap to be timed on them. With `--ocr`, the body lines found on hybrid pages that
//! match one of the text layer's are read by the sandboxed OCR worker, as the reader reads them
//! (straightened along the baseline, the page drawn so its small letters are about 22 pixels high),
//! and what it reads is compared with the text layer's, character by character: an estimate of the
//! OCR on real scans, against another program's OCR, itself not free of mistakes.
//!
//! `--lines-synthetic`: the labeled corpus of `musaeum_vision::synth`, timed in this build and
//! scored against its answers, for the gate's F1.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use musaeum_core::{BackgroundResult, BackgroundWork, DeliveryMode, DocumentHints, RenderPool};
use musaeum_ocr::{Gray as OcrGray, LinePath, OcrClient, OcrConfig, join_lines, read_line};
use musaeum_shm::chars::{GENERATED, LINE_END};
use musaeum_shm::tile::page_pixels;
use musaeum_shm::{CharLayer, ContentClass};
use musaeum_vision::score::{Score, matches, score};
use musaeum_vision::synth::{self, Layout, TruthLine};
use musaeum_vision::{DetectOptions, Gray, detect, dhash, to_shown};
use serde::Serialize;
use serde_json::json;

use crate::render::{self, OpenFailure, config, pool_error, slab};
use crate::snapshot;

/// Longest one file's sampled pages may take.
const WAIT: Duration = Duration::from_secs(600);

/// Percentiles of a list of milliseconds.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Millis {
    pub count: usize,
    pub p50: f64,
    pub p95: f64,
    pub max: f64,
    pub mean: f64,
}

impl Millis {
    pub fn of(values: &[f64]) -> Self {
        if values.is_empty() {
            return Self::default();
        }
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
        Self {
            count: sorted.len(),
            p50: at(0.5),
            p95: at(0.95),
            max: sorted[sorted.len() - 1],
            mean: sorted.iter().sum::<f64>() / sorted.len() as f64,
        }
    }
}

#[derive(Debug, Serialize)]
struct PageRow {
    file: String,
    page: u32,
    class: &'static str,
    width: u32,
    height: u32,
    ms: f64,
    /// Binarizing, labeling, skew, lines.
    steps_ms: [f32; 4],
    lines: usize,
    columns: usize,
    glyph_height: f32,
    skew_degrees: f32,
    page_hash: String,
    /// Against the OCR text layer's lines, on a hybrid page with enough of them: all of them, and
    /// body lines only, at least eight letters long on both sides.
    ocr: Option<ScoreRow>,
    ocr_body: Option<ScoreRow>,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct ScoreRow {
    truth: usize,
    found: usize,
    matched: usize,
    f1: f64,
}

impl From<Score> for ScoreRow {
    fn from(score: Score) -> Self {
        Self {
            truth: score.truth,
            found: score.found,
            matched: score.matched,
            f1: score.f1(),
        }
    }
}

/// What the OCR read on the hybrid pages, line by line.
#[derive(Default)]
struct Readings {
    lines: usize,
    exact: usize,
    chars: usize,
    edits: usize,
    /// The same, with typography folded (curly quotes, dashes, bullets) and without the pairs whose
    /// lengths differ by a fifth or more: where the text layer's line runs on into the next column,
    /// or stops short, the two are not the same stretch of the page.
    fair: Tally,
    dpi: Vec<f64>,
    ms: Vec<f64>,
    /// Every line read: file, page, the text layer's, the OCR's, and the edits between them.
    all: Vec<serde_json::Value>,
}

#[derive(Default, Serialize)]
struct Tally {
    lines: usize,
    exact: usize,
    chars: usize,
    edits: usize,
    /// Pairs left out for their lengths.
    unmatched: usize,
}

/// A line with what typesetting, not reading, makes differ: quotes straight, dashes as hyphens,
/// bullets and arrows at its start left out.
fn folded(text: &str) -> String {
    let mapped: String = text
        .chars()
        .map(|ch| match ch {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{2032}' | '`' | '\u{00B4}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{2033}' => '"',
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2212}' => '-',
            other => other,
        })
        .collect();
    let trimmed = mapped.trim_start_matches(|ch: char| {
        matches!(
            ch,
            '\u{2022}' | '\u{27A4}' | '\u{25AA}' | '\u{25CF}' | '\u{00BB}' | '>' | '\u{2023}'
        ) || ch.is_whitespace()
    });
    collapse(&trimmed.replace('\u{FB01}', "fi").replace('\u{FB02}', "fl"))
}

/// Whitespace as one space, none at the ends.
pub(crate) fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The OCR text layer's lines, as lines on the picture as shown, from the first character to the
/// last. The baseline is a straight line through the bottoms of the characters' boxes, fitted by
/// the median of the slopes (Theil and Sen) with the deepest fifth left out, so descenders and
/// stray boxes do not pull it: the hybrid pages of the sample are flat scans. Lines of fewer than
/// three characters, and generated characters, are left out.
fn ocr_lines(layer: &CharLayer, width: u32, height: u32) -> Vec<(TruthLine, String)> {
    let (w, h) = (width as f32, height as f32);
    let mut lines = Vec::new();
    // Per character: centre x, bottom, height, left, right.
    let mut current: Vec<[f32; 5]> = Vec::new();
    let mut text = String::new();
    let mut finish = |current: &mut Vec<[f32; 5]>, text: &mut String| {
        if current.len() >= 3 {
            let left = current.iter().map(|c| c[3]).fold(f32::INFINITY, f32::min);
            let right = current
                .iter()
                .map(|c| c[4])
                .fold(f32::NEG_INFINITY, f32::max);
            let mut heights: Vec<f32> = current.iter().map(|c| c[2]).collect();
            heights.sort_by(f32::total_cmp);
            let glyph = heights[heights.len() / 2];
            let mut bottoms: Vec<f32> = current.iter().map(|c| c[1]).collect();
            bottoms.sort_by(f32::total_cmp);
            let cut = bottoms[(bottoms.len() * 4 / 5).min(bottoms.len() - 1)];
            let kept: Vec<[f32; 2]> = current
                .iter()
                .filter(|c| c[1] <= cut)
                .map(|c| [c[0], c[1]])
                .collect();
            let mut slopes = Vec::new();
            let step = (kept.len() / 40).max(1);
            for a in (0..kept.len()).step_by(step) {
                for b in (a + 1..kept.len()).step_by(step) {
                    let dx = kept[b][0] - kept[a][0];
                    if dx.abs() > glyph {
                        slopes.push((kept[b][1] - kept[a][1]) / dx);
                    }
                }
            }
            slopes.sort_by(f32::total_cmp);
            let slope = slopes.get(slopes.len() / 2).copied().unwrap_or(0.0);
            let mut intercepts: Vec<f32> = kept.iter().map(|p| p[1] - slope * p[0]).collect();
            intercepts.sort_by(f32::total_cmp);
            let intercept = intercepts[intercepts.len() / 2];
            lines.push((
                TruthLine {
                    column: 0,
                    baseline: vec![
                        [left, intercept + slope * left],
                        [right, intercept + slope * right],
                    ],
                    x_height: glyph * 0.6,
                    turned: false,
                },
                collapse(text),
            ));
        }
        current.clear();
        text.clear();
    };
    for glyph in &layer.chars {
        if glyph.flags & GENERATED == 0 || glyph.ch == ' ' {
            text.push(glyph.ch);
        }
        if glyph.flags & GENERATED == 0 {
            let corners: Vec<[f32; 2]> = (0..4)
                .map(|corner| {
                    let [x, y] = to_shown(
                        glyph.quad[corner * 2],
                        glyph.quad[corner * 2 + 1],
                        layer.rotation,
                    );
                    [x * w, y * h]
                })
                .collect();
            let left = corners.iter().map(|c| c[0]).fold(f32::INFINITY, f32::min);
            let right = corners
                .iter()
                .map(|c| c[0])
                .fold(f32::NEG_INFINITY, f32::max);
            let top = corners.iter().map(|c| c[1]).fold(f32::INFINITY, f32::min);
            let bottom = corners
                .iter()
                .map(|c| c[1])
                .fold(f32::NEG_INFINITY, f32::max);
            if right > left && bottom > top {
                current.push([(left + right) / 2.0, bottom, bottom - top, left, right]);
            }
        }
        if glyph.flags & LINE_END != 0 {
            finish(&mut current, &mut text);
        }
    }
    finish(&mut current, &mut text);
    lines
}

fn extent(points: &[[f32; 2]]) -> f32 {
    match (points.first(), points.last()) {
        (Some(first), Some(last)) => last[0] - first[0],
        _ => 0.0,
    }
}

/// The page with the lines found drawn in red and the OCR layer's in green, for looking at.
pub(crate) fn overlay(
    picture: &Gray,
    found: &[musaeum_vision::lines::PixelLine],
    truth: &[TruthLine],
) -> Vec<u8> {
    let (w, h) = (picture.width as usize, picture.height as usize);
    let mut rgba = Vec::with_capacity(w * h * 4);
    for &value in &picture.pixels {
        rgba.extend_from_slice(&[value, value, value, 255]);
    }
    fn put(rgba: &mut [u8], w: usize, h: usize, x: f32, y: f32, color: [u8; 3]) {
        let (x, y) = (x.round() as i64, y.round() as i64);
        if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
            let at = (y as usize * w + x as usize) * 4;
            rgba[at..at + 3].copy_from_slice(&color);
        }
    }
    fn polyline(
        rgba: &mut [u8],
        w: usize,
        h: usize,
        points: &[[f32; 2]],
        color: [u8; 3],
        offset: f32,
    ) {
        for pair in points.windows(2) {
            let ([ax, ay], [bx, by]) = (pair[0], pair[1]);
            let steps = (bx - ax).abs().max((by - ay).abs()).ceil().max(1.0) as usize;
            for step in 0..=steps {
                let t = step as f32 / steps as f32;
                put(
                    rgba,
                    w,
                    h,
                    ax + (bx - ax) * t,
                    ay + (by - ay) * t + offset,
                    color,
                );
            }
        }
    }
    for line in truth {
        polyline(&mut rgba, w, h, &line.baseline, [0, 170, 0], 1.0);
    }
    for line in found {
        polyline(&mut rgba, w, h, &line.baseline, [230, 0, 0], 0.0);
        // The line's ends, as short ticks: red at the start, blue at the end.
        if let (Some(first), Some(last)) = (line.baseline.first(), line.baseline.last()) {
            for dy in -4..=0 {
                put(&mut rgba, w, h, first[0], first[1] + dy as f32, [230, 0, 0]);
                put(&mut rgba, w, h, last[0], last[1] + dy as f32, [0, 0, 230]);
            }
        }
    }
    rgba
}

/// Files of the collection worth sampling: PDFs, in a fixed order.
fn pdfs(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                folders.push(path);
            } else if kind.is_file()
                && path
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

/// `count` pages spread through a book of `pages`, away from its covers.
fn spread(pages: u32, count: u32) -> Vec<u32> {
    if pages <= 4 {
        return (0..pages).collect();
    }
    let (first, last) = (2, pages - 2);
    let count = count.min(last - first);
    (0..count)
        .map(|step| first + (last - first) * (2 * step + 1) / (2 * count))
        .collect()
}

/// The real sample. Returns whether the collection was left exactly as it was and the time per
/// page met the gate.
pub fn run_lines(
    dump: Option<&Path>,
    corpus: &Path,
    work: &Path,
    out: Option<&Path>,
    files: usize,
    pages_per_file: u32,
    read: bool,
) -> Result<bool, String> {
    let started = Instant::now();
    let mut reader = read.then(|| (OcrClient::new(OcrConfig::default()), Readings::default()));
    eprintln!("listing {} before anything else", corpus.display());
    let before = snapshot::take(corpus).map_err(|error| format!("listing: {error}"))?;
    let models_dir = work.join("line-models");
    std::fs::create_dir_all(&models_dir).map_err(|error| format!("the models folder: {error}"))?;

    let all = pdfs(corpus);
    let step = (all.len() / files.max(1)).max(1);
    let chosen: Vec<&PathBuf> = all.iter().step_by(step).take(files).collect();
    eprintln!(
        "{} PDFs in the collection, {} sampled",
        all.len(),
        chosen.len()
    );
    let worker = render::worker_exe();
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    let mut skipped_text = 0usize;
    for (index, path) in chosen.iter().enumerate() {
        let relative = path
            .strip_prefix(corpus)
            .unwrap_or(path)
            .display()
            .to_string();
        eprintln!("[{}/{}] {relative}", index + 1, chosen.len());
        let measured = match render::measure(&worker, path) {
            Ok(measured) => measured,
            Err(failure) => {
                failures.push(json!({ "file": relative, "failure": format!("{failure:?}") }));
                continue;
            }
        };
        let picks = spread(measured.pages.len() as u32, pages_per_file);
        match trace_file(
            &worker,
            path,
            &measured.pages,
            &picks,
            dump.map(|dir| (dir, index)),
            reader
                .as_mut()
                .map(|(client, readings)| (client, readings, relative.as_str())),
        ) {
            Ok(traced) => {
                for page in traced {
                    let Some(page) = page else {
                        skipped_text += 1;
                        continue;
                    };
                    let file_name = format!("{:04}-{:05}.mlin", index, page.page);
                    let _ = std::fs::write(models_dir.join(file_name), &page.model);
                    rows.push(PageRow {
                        file: relative.clone(),
                        ..page.row
                    });
                }
            }
            Err(failure) => {
                failures.push(json!({ "file": relative, "failure": format!("{failure:?}") }))
            }
        }
    }

    eprintln!("listing {} again", corpus.display());
    let after = snapshot::take(corpus).map_err(|error| format!("listing again: {error}"))?;
    let comparison = snapshot::compare(&before, &after);

    let times: Vec<f64> = rows.iter().map(|row| row.ms).collect();
    let millis = Millis::of(&times);
    let scanned = rows.iter().filter(|row| row.class == "scanned").count();
    let hybrid = rows.iter().filter(|row| row.class == "hybrid").count();
    let mut ocr = Score::default();
    let mut ocr_body = Score::default();
    let mut ocr_pages = 0;
    let as_score = |scored: ScoreRow| Score {
        truth: scored.truth,
        found: scored.found,
        matched: scored.matched,
    };
    for row in &rows {
        if let Some(scored) = row.ocr {
            ocr.add(as_score(scored));
            ocr_pages += 1;
        }
        if let Some(scored) = row.ocr_body {
            ocr_body.add(as_score(scored));
        }
    }
    let unchanged = comparison.unchanged();
    let fast = millis.p95 <= 50.0;
    let ocr_reading = reader.as_ref().map(|(client, readings)| {
        json!({
            "lines": readings.lines,
            "exact": readings.exact,
            "chars": readings.chars,
            "edits": readings.edits,
            "cer": readings.edits as f64 / readings.chars.max(1) as f64,
            "fair": readings.fair,
            "fairCer": readings.fair.edits as f64 / readings.fair.chars.max(1) as f64,
            "dpi": Millis::of(&readings.dpi),
            "msPerLine": Millis::of(&readings.ms),
            "restarts": client.restarts(),
            "lines_read": readings.all,
        })
    });
    if let Some((_, readings)) = &reader {
        eprintln!(
            "OCR against the text layer: {} body lines, {} read the same, {:.2}% of characters differ; typography folded, on the {} lines of matching length, {} the same and {:.2}% differ",
            readings.lines,
            readings.exact,
            100.0 * readings.edits as f64 / readings.chars.max(1) as f64,
            readings.fair.lines,
            readings.fair.exact,
            100.0 * readings.fair.edits as f64 / readings.fair.chars.max(1) as f64
        );
    }
    let report = json!({
        "corpus": corpus.display().to_string(),
        "readOnly": { "unchanged": unchanged, "comparison": comparison },
        "files": chosen.len(),
        "pagesAnalyzed": rows.len(),
        "scanned": scanned,
        "hybrid": hybrid,
        "textPagesSkipped": skipped_text,
        "detectMs": millis,
        "linesPerPage": Millis::of(&rows.iter().map(|row| row.lines as f64).collect::<Vec<_>>()),
        "againstOcr": {
            "pages": ocr_pages,
            "truth": ocr.truth,
            "found": ocr.found,
            "matched": ocr.matched,
            "precision": ocr.precision(),
            "recall": ocr.recall(),
            "f1": ocr.f1(),
        },
        "againstOcrBodyLines": {
            "truth": ocr_body.truth,
            "found": ocr_body.found,
            "matched": ocr_body.matched,
            "precision": ocr_body.precision(),
            "recall": ocr_body.recall(),
            "f1": ocr_body.f1(),
        },
        "ocrAgainstTextLayer": ocr_reading,
        "failures": failures,
        "pages": rows,
        "modelsDir": models_dir.display().to_string(),
        "wallS": started.elapsed().as_secs_f64(),
    });
    let text = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
    if let Some(out) = out {
        std::fs::write(out, &text).map_err(|error| format!("the report: {error}"))?;
    }
    eprintln!(
        "{} pages ({scanned} scanned, {hybrid} hybrid): detect p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms; against OCR on {ocr_pages} pages F1 {:.3}, body lines F1 {:.3}; collection unchanged: {unchanged}",
        rows.len(),
        millis.p50,
        millis.p95,
        millis.max,
        ocr.f1(),
        ocr_body.f1()
    );
    Ok(unchanged && fast && !rows.is_empty())
}

struct TracedPicture {
    page: u32,
    row: PageRow,
    model: Vec<u8>,
}

/// Classifies `picks` and analyzes those that are pictures.
fn trace_file(
    worker: &Path,
    pdf: &Path,
    pages: &[musaeum_shm::PageGeometry],
    picks: &[u32],
    dump: Option<(&Path, usize)>,
    mut reader: Option<(&mut OcrClient, &mut Readings, &str)>,
) -> Result<Vec<Option<TracedPicture>>, OpenFailure> {
    let mut configuration = config(worker, pdf, 2);
    configuration.known_geometry = Some((pages.to_vec(), DocumentHints::default()));
    let pool = RenderPool::start(
        configuration,
        slab(32).map_err(OpenFailure::Core)?,
        DeliveryMode::Pull { max_batch: 1 },
    )
    .map_err(pool_error)?;
    let (sender, results) = mpsc::channel();
    pool.start_background(
        BackgroundWork {
            analyze: picks.to_vec(),
            lines: picks.to_vec(),
            ..BackgroundWork::default()
        },
        Box::new(move |result| {
            let _ = sender.send(result);
        }),
    );
    let mut classes = std::collections::HashMap::new();
    let mut pictures = Vec::new();
    loop {
        match results.recv_timeout(WAIT) {
            Ok(BackgroundResult::Content { page, content }) => {
                classes.insert(page, content.class());
            }
            Ok(BackgroundResult::Lines {
                page,
                width,
                height,
                gray,
            }) => pictures.push((page, width, height, gray)),
            Ok(BackgroundResult::Finished) => break,
            Ok(_) => {}
            Err(_) => {
                pool.shutdown();
                return Err(OpenFailure::Core(
                    "the background work did not finish".to_owned(),
                ));
            }
        }
    }
    let mut traced = Vec::new();
    for (page, width, height, gray) in pictures {
        let class = match classes.get(&page) {
            Some(ContentClass::Scanned) => "scanned",
            Some(ContentClass::Hybrid) => "hybrid",
            _ => {
                traced.push(None);
                continue;
            }
        };
        let layer = pool
            .request_chars(page)
            .recv_timeout(WAIT)
            .ok()
            .and_then(Result::ok);
        let rotation = layer.as_ref().map_or(0, |layer| layer.rotation);
        let picture = Gray {
            width,
            height,
            pixels: gray,
        };
        let begun = Instant::now();
        let found = detect(
            &picture,
            &DetectOptions {
                rotation,
                ..DetectOptions::default()
            },
        );
        let ms = begun.elapsed().as_secs_f64() * 1000.0;
        let page_hash = dhash(&picture, 0, 0, i64::from(width), i64::from(height));
        let with_text = match (&layer, class) {
            (Some(layer), "hybrid") => ocr_lines(layer, width, height),
            _ => Vec::new(),
        };
        let truth: Vec<TruthLine> = with_text.iter().map(|(line, _)| line.clone()).collect();
        let ocr = (truth.len() >= 5).then(|| score(&found.pixel_lines, &truth).into());
        let long = 8.0 * found.stats.glyph_height;
        let body_truth: Vec<TruthLine> = truth
            .iter()
            .filter(|line| extent(&line.baseline) >= long)
            .cloned()
            .collect();
        let body_found: Vec<musaeum_vision::lines::PixelLine> = found
            .pixel_lines
            .iter()
            .filter(|line| extent(&line.baseline) >= long)
            .cloned()
            .collect();
        let ocr_body = (body_truth.len() >= 5).then(|| score(&body_found, &body_truth).into());
        if let (Some((client, readings, file)), true) = (reader.as_mut(), body_truth.len() >= 5) {
            let texts: Vec<&String> = with_text
                .iter()
                .filter(|(line, _)| extent(&line.baseline) >= long)
                .map(|(_, text)| text)
                .collect();
            let pairs = matches(&body_found, &body_truth);
            let matched: Vec<(&musaeum_vision::lines::PixelLine, &String)> = body_found
                .iter()
                .zip(&pairs)
                .filter_map(|(line, pair)| pair.map(|t| (line, texts[t])))
                .collect();
            if !matched.is_empty() {
                // As the reader draws a page for its OCR: small letters about 22 pixels high.
                let size = pages[page as usize];
                let mut heights: Vec<f32> = matched.iter().map(|(line, _)| line.x_height).collect();
                heights.sort_by(f32::total_cmp);
                let x_height_pt = heights[heights.len() / 2] * size.width_pt / width as f32;
                let mut dpi = (72.0 * 22.0 / x_height_pt.max(0.5)).clamp(300.0, 600.0);
                let area = size.width_pt * size.height_pt;
                let most = musaeum_core::MAX_REGION_PIXELS as f32 * 0.9;
                if area * (dpi / 72.0).powi(2) > most {
                    dpi = 72.0 * (most / area).sqrt();
                }
                let scale = crate::ocr::scale_of(dpi);
                match crate::ocr::draw_page(&pool, page, scale) {
                    Ok((ocr_w, ocr_h, ocr_gray)) => {
                        readings.dpi.push(f64::from(dpi));
                        let picture = OcrGray {
                            width: ocr_w,
                            height: ocr_h,
                            pixels: ocr_gray,
                        };
                        let k = page_pixels(size.width_pt, scale) as f32 / width as f32;
                        for (line, expected) in matched {
                            let path = LinePath {
                                baseline: line
                                    .baseline
                                    .iter()
                                    .map(|p| [p[0] * k, p[1] * k])
                                    .collect(),
                                ascent: line.ascent * k,
                                descent: line.descent * k,
                                x_height: line.x_height * k,
                            };
                            let begun = Instant::now();
                            let Ok(read) =
                                read_line(client, "por+eng", &picture, &path, dpi.round() as u32)
                            else {
                                continue;
                            };
                            readings.ms.push(begun.elapsed().as_secs_f64() * 1000.0);
                            let text = collapse(&join_lines([read.words.iter().collect()], "\n"));
                            let edits = crate::ocr::distance(&text, expected);
                            readings.lines += 1;
                            readings.chars += expected.chars().count();
                            readings.edits += edits;
                            if edits == 0 {
                                readings.exact += 1;
                            }
                            let (layer, ours) = (folded(expected), folded(&text));
                            let (a, b) =
                                (layer.chars().count() as f32, ours.chars().count() as f32);
                            if a > 0.0 && (0.8..=1.25).contains(&(b / a)) {
                                let fair = crate::ocr::distance(&ours, &layer);
                                readings.fair.lines += 1;
                                readings.fair.chars += layer.chars().count();
                                readings.fair.edits += fair;
                                readings.fair.exact += usize::from(fair == 0);
                            } else {
                                readings.fair.unmatched += 1;
                            }
                            readings.all.push(json!({
                                "file": file,
                                "page": page,
                                "layer": expected,
                                "read": text,
                                "edits": edits,
                            }));
                        }
                    }
                    Err(error) => eprintln!("page {page} could not be drawn for the OCR: {error}"),
                }
            }
        }
        if let Some((dir, file)) = dump {
            let _ = std::fs::create_dir_all(dir);
            let mut overlay = overlay(&picture, &found.pixel_lines, &truth);
            // The bands and their columns, in orange.
            let w = width as usize;
            for band in &found.regions {
                for area in band {
                    for x in area.left..area.right.min(width) {
                        for y in [area.top, area.bottom.saturating_sub(1).min(height - 1)] {
                            let at = (y as usize * w + x as usize) * 4;
                            overlay[at..at + 3].copy_from_slice(&[255, 140, 0]);
                        }
                    }
                    for y in area.top..area.bottom.min(height) {
                        for x in [area.left, area.right.saturating_sub(1).min(width - 1)] {
                            let at = (y as usize * w + x as usize) * 4;
                            overlay[at..at + 3].copy_from_slice(&[255, 140, 0]);
                        }
                    }
                }
            }
            eprintln!(
                "page {page}: glyph {:.1}, glyphs {}, specks {}, figures {}, bands {:?}",
                found.stats.glyph_height,
                found.stats.glyphs,
                found.stats.specks,
                found.stats.figures,
                found.regions.iter().map(Vec::len).collect::<Vec<_>>()
            );
            if let Ok(jpeg) = musaeum_core::thumbnail::encode_jpeg(width, height, &overlay) {
                let _ = std::fs::write(dir.join(format!("{file:04}-{page:05}.jpg")), jpeg);
            }
        }
        let mut model = page_hash.to_le_bytes().to_vec();
        model.extend_from_slice(&found.model.encode());
        traced.push(Some(TracedPicture {
            page,
            row: PageRow {
                file: String::new(),
                page,
                class,
                width,
                height,
                ms,
                steps_ms: found.stats.ms,
                lines: found.stats.lines,
                columns: found.stats.columns,
                glyph_height: found.stats.glyph_height,
                skew_degrees: found.stats.skew_degrees,
                page_hash: format!("{page_hash:016x}"),
                ocr,
                ocr_body,
            },
            model,
        }));
    }
    pool.shutdown();
    Ok(traced)
}

/// The labeled corpus, timed and scored.
pub fn run_synthetic(out: Option<&Path>, pages: u64) -> Result<bool, String> {
    let mut layouts = Vec::new();
    let mut passed = true;
    for layout in Layout::ALL {
        let mut total = Score::default();
        let mut times = Vec::new();
        for seed in 0..pages {
            let page = synth::page(layout, seed);
            let begun = Instant::now();
            let found = detect(&page.gray, &DetectOptions::default());
            times.push(begun.elapsed().as_secs_f64() * 1000.0);
            total.add(score(&found.pixel_lines, &page.lines));
        }
        let goal = if layout == Layout::Curved { 0.85 } else { 0.95 };
        let millis = Millis::of(&times);
        passed &= total.f1() >= goal && millis.p95 <= 50.0;
        eprintln!(
            "{}: {} pages, F1 {:.3} (precision {:.3}, recall {:.3}), detect p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms",
            layout.name(),
            pages,
            total.f1(),
            total.precision(),
            total.recall(),
            millis.p50,
            millis.p95,
            millis.max
        );
        layouts.push(json!({
            "layout": layout.name(),
            "pages": pages,
            "goal": goal,
            "truth": total.truth,
            "found": total.found,
            "matched": total.matched,
            "precision": total.precision(),
            "recall": total.recall(),
            "f1": total.f1(),
            "detectMs": millis,
        }));
    }
    let report = json!({ "layouts": layouts, "passed": passed });
    if let Some(out) = out {
        let text = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
        std::fs::write(out, text).map_err(|error| format!("the report: {error}"))?;
    }
    Ok(passed)
}
