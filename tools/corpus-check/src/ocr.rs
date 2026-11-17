//! The line detector and the OCR on pages of real type whose every line is known.
//!
//! `--ocr-book`: `gen_fixtures::ocr` writes a native book, with a text layer, of prose, two
//! columns over a halftone plate, a chart, a ruled table and blocks of text turned 90°, 270°, 45°
//! and −30°. The same sandboxed pool the reader uses draws its pages at 300 dpi, and
//! `gen_fixtures::scan` makes three scanned twins of them: clean, turned 1.5°, and curled toward the
//! spine and turned; each is written as a PDF of pictures only. Each twin's pages are then drawn
//! again by the pool, for the detector and for the OCR, as the reader does: the detector's lines are
//! scored against the true lines, carried through the same scan; lines found inside the plate, the
//! chart or the table that are none of their labels or cells are counted; and every line found is
//! read by the sandboxed OCR worker, straightened along its baseline, and what it reads compared
//! with the line's true text, character by character.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gen_fixtures::ocr::{LineKind, OcrBook, OcrLine, PAGE, ocr_book};
use gen_fixtures::scan::{GrayPage, Scanning, image_pdf, scan, warp};
use musaeum_core::{DeliveryMode, RegionRequest, RenderPool};
use musaeum_ocr::{Gray as OcrGray, LinePath, OcrClient, OcrConfig, join_lines, read_line};
use musaeum_shm::tile::{SCALE_ONE, lines_scale, page_pixels};
use musaeum_vision::score::matches;
use musaeum_vision::synth::TruthLine;
use musaeum_vision::{DetectOptions, Gray, detect};
use serde::Serialize;
use serde_json::json;

use crate::lines::Millis;
use crate::render::{self, config, pool_error, slab};

const WAIT: Duration = Duration::from_secs(120);
const DPI: f32 = 300.0;

/// The scans the twins are made by.
fn twins() -> [(&'static str, Scanning); 3] {
    [
        (
            "clean",
            Scanning {
                noise: 1.0,
                seed: 1,
                ..Scanning::default()
            },
        ),
        (
            "turned",
            Scanning {
                skew_degrees: 1.5,
                noise: 1.0,
                seed: 2,
                ..Scanning::default()
            },
        ),
        (
            "curled",
            Scanning {
                skew_degrees: -0.6,
                curl: 0.012,
                noise: 1.5,
                seed: 3,
            },
        ),
    ]
}

/// The scale of `dpi` pixels per inch.
pub(crate) fn scale_of(dpi: f32) -> u32 {
    (SCALE_ONE as f32 * dpi / 72.0).round() as u32
}

/// A whole page drawn by the pool, gray.
pub(crate) fn draw_page(
    pool: &RenderPool,
    page: u32,
    scale: u32,
) -> Result<(u32, u32, Vec<u8>), String> {
    let size = pool.pages()[page as usize];
    let request = RegionRequest {
        page,
        scale,
        left: 0,
        top: 0,
        width: page_pixels(size.width_pt, scale) as u32,
        height: page_pixels(size.height_pt, scale) as u32,
    };
    let picture = pool
        .request_region(request)
        .recv_timeout(WAIT)
        .map_err(|_| format!("page {page} was not drawn in time"))?
        .map_err(|error| format!("page {page}: {error:?}"))?;
    Ok((picture.request.width, picture.request.height, picture.gray))
}

fn start(pdf: &Path) -> Result<RenderPool, String> {
    RenderPool::start(
        config(&render::worker_exe(), pdf, 2),
        slab(64).map_err(|error| format!("{error:?}"))?,
        DeliveryMode::Pull { max_batch: 1 },
    )
    .map_err(|error| format!("{:?}", pool_error(error)))
}

/// Where a point of the native page, in points from its bottom-left corner, lands on the twin's
/// picture of `width` × `height` pixels.
fn on_twin(scanning: &Scanning, page: u32, width: f32, height: f32, point: [f32; 2]) -> [f32; 2] {
    let k = DPI / 72.0;
    let (flat_w, flat_h) = (PAGE[0] * k, PAGE[1] * k);
    let [x, y] = warp(
        scanning,
        flat_w,
        flat_h,
        page.is_multiple_of(2),
        point[0] * k,
        (PAGE[1] - point[1]) * k,
    );
    [x * width / flat_w, y * height / flat_h]
}

/// A true line as the detector's picture of the twin shows it.
fn truth_line(line: &OcrLine, scanning: &Scanning, width: f32, height: f32) -> TruthLine {
    let steps = 16;
    let baseline = (0..=steps)
        .map(|step| {
            let t = step as f32 / steps as f32;
            let point = [
                line.start[0] + (line.end[0] - line.start[0]) * t,
                line.start[1] + (line.end[1] - line.start[1]) * t,
            ];
            on_twin(scanning, line.page, width, height, point)
        })
        .collect();
    TruthLine {
        column: 0,
        baseline,
        // The x-height of Times and Helvetica is about half their size.
        x_height: line.size * 0.47 * height / PAGE[1],
        turned: line.kind == LineKind::Turned || line.angle_degrees().abs() > 5.0,
    }
}

/// Levenshtein's distance between two strings, by characters.
pub(crate) fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let current = row[j + 1];
            row[j + 1] = (row[j] + 1)
                .min(row[j + 1] + 1)
                .min(previous + usize::from(ca != cb));
            previous = current;
        }
    }
    row[b.len()]
}

/// Totals for one kind of line on one twin.
#[derive(Clone, Debug, Default, Serialize)]
struct Kind {
    truth: usize,
    matched: usize,
    /// Characters of the true text, and edits to get there from what was read.
    chars: usize,
    edits: usize,
    /// Lines read exactly.
    exact: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
struct TwinReport {
    name: String,
    found: usize,
    truth: usize,
    matched: usize,
    false_in_graphics: usize,
    /// What the OCR read on lines found in graphics that match no true line.
    read_in_graphics: Vec<String>,
    kinds: BTreeMap<String, Kind>,
    detect_ms: Millis,
    ocr_line_ms: Millis,
    flipped: usize,
    mistakes: Vec<String>,
}

/// The native book, its twins, and the measures.
pub fn run_ocr_book(work: &Path, out: Option<&Path>) -> Result<bool, String> {
    std::fs::create_dir_all(work).map_err(|error| format!("the work folder: {error}"))?;
    let book: OcrBook = ocr_book().map_err(|error| error.to_string())?;
    let native = work.join("ocr-native.pdf");
    std::fs::write(&native, &book.pdf).map_err(|error| format!("{}: {error}", native.display()))?;
    std::fs::write(
        work.join("ocr-native.json"),
        gen_fixtures::ocr::truth_json(&book),
    )
    .map_err(|error| format!("the answers: {error}"))?;
    eprintln!("drawing the native book at {DPI} dpi");
    let pool = start(&native)?;
    let count = pool.pages().len() as u32;
    let mut flat = Vec::new();
    for page in 0..count {
        let (width, height, pixels) = draw_page(&pool, page, scale_of(DPI))?;
        flat.push(GrayPage {
            width,
            height,
            pixels,
        });
    }
    pool.shutdown();
    let mut client = OcrClient::new(OcrConfig::default());
    let mut reports = Vec::new();
    let mut passed = true;
    for (name, scanning) in twins() {
        let pages: Vec<GrayPage> = flat
            .iter()
            .enumerate()
            .map(|(page, picture)| scan(picture, &scanning, page as u32))
            .collect();
        let pdf = image_pdf(&pages, PAGE).map_err(|error| error.to_string())?;
        let path: PathBuf = work.join(format!("ocr-{name}.pdf"));
        std::fs::write(&path, &pdf).map_err(|error| format!("{}: {error}", path.display()))?;
        eprintln!(
            "{name}: {} pages, {:.1} MB",
            pages.len(),
            pdf.len() as f64 / 1e6
        );
        let report = measure_twin(name, &path, &scanning, &book, &mut client)?;
        passed &= report.false_in_graphics == 0;
        reports.push(report);
    }
    for report in &reports {
        eprintln!(
            "{}: {} true, {} found, {} matched, {} false in graphics; detect p50 {:.1} ms, OCR p50 {:.0} ms a line",
            report.name,
            report.truth,
            report.found,
            report.matched,
            report.false_in_graphics,
            report.detect_ms.p50,
            report.ocr_line_ms.p50
        );
        for (kind, totals) in &report.kinds {
            eprintln!(
                "  {kind}: {}/{} found, {} read exactly, {:.2}% of characters wrong",
                totals.matched,
                totals.truth,
                totals.exact,
                100.0 * totals.edits as f64 / totals.chars.max(1) as f64
            );
        }
    }
    let report = json!({ "twins": reports, "passed": passed, "ocrRestarts": client.restarts() });
    if let Some(out) = out {
        let text = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
        std::fs::write(out, text).map_err(|error| format!("the report: {error}"))?;
    }
    Ok(passed)
}

fn measure_twin(
    name: &str,
    pdf: &Path,
    scanning: &Scanning,
    book: &OcrBook,
    client: &mut OcrClient,
) -> Result<TwinReport, String> {
    let pool = start(pdf)?;
    let mut report = TwinReport {
        name: name.to_owned(),
        ..TwinReport::default()
    };
    let mut detect_ms = Vec::new();
    let mut ocr_ms = Vec::new();
    for page in 0..pool.pages().len() as u32 {
        let size = pool.pages()[page as usize];
        let (width, height, gray) = draw_page(&pool, page, lines_scale(size))?;
        let picture = Gray {
            width,
            height,
            pixels: gray,
        };
        let begun = Instant::now();
        let found = detect(&picture, &DetectOptions::default());
        detect_ms.push(begun.elapsed().as_secs_f64() * 1000.0);
        let (w, h) = (width as f32, height as f32);
        let lines: Vec<&OcrLine> = book.lines.iter().filter(|line| line.page == page).collect();
        let truth: Vec<TruthLine> = lines
            .iter()
            .map(|line| truth_line(line, scanning, w, h))
            .collect();
        let matched = matches(&found.pixel_lines, &truth);
        // What the detector made of the page, for looking at: true lines green, found red.
        let dump = pdf.with_file_name(format!("dump-{name}-{page}.jpg"));
        let overlay = crate::lines::overlay(&picture, &found.pixel_lines, &truth);
        let mut pgm = format!("P5\n{width} {height}\n255\n").into_bytes();
        pgm.extend_from_slice(&picture.pixels);
        let _ = std::fs::write(pdf.with_file_name(format!("page-{name}-{page}.pgm")), pgm);
        if let Ok(jpeg) = musaeum_core::thumbnail::encode_jpeg(width, height, &overlay) {
            let _ = std::fs::write(&dump, jpeg);
        }
        eprintln!(
            "{name} page {page}: glyph {:.1}, blocks {:?}, dropped {:?}, picture cells {}",
            found.stats.glyph_height,
            found
                .stats
                .turned_blocks
                .iter()
                .map(|a| a.round())
                .collect::<Vec<_>>(),
            found.stats.dropped,
            found.stats.picture_cells
        );
        report.found += found.pixel_lines.len();
        report.truth += truth.len();
        for line in &lines {
            report
                .kinds
                .entry(line.kind.name().to_owned())
                .or_default()
                .truth += 1;
        }
        // The page at 300 dpi for the OCR.
        let (ocr_w, ocr_h, ocr_gray) = draw_page(&pool, page, scale_of(DPI))?;
        let ocr_picture = OcrGray {
            width: ocr_w,
            height: ocr_h,
            pixels: ocr_gray,
        };
        let k = ocr_w as f32 / w;
        let graphics: Vec<[f32; 4]> = book
            .graphics
            .iter()
            .filter(|graphic| graphic.page == page)
            .map(|graphic| {
                let [left, bottom, right, top] = graphic.rect;
                let a = on_twin(scanning, page, w, h, [left, top]);
                let b = on_twin(scanning, page, w, h, [right, bottom]);
                [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[0].max(b[0]),
                    a[1].max(b[1]),
                ]
            })
            .collect();
        for (line, matched) in found.pixel_lines.iter().zip(&matched) {
            let path = LinePath {
                baseline: line.baseline.iter().map(|p| [p[0] * k, p[1] * k]).collect(),
                ascent: line.ascent * k,
                descent: line.descent * k,
                x_height: line.x_height * k,
            };
            let begun = Instant::now();
            let read = read_line(client, "por", &ocr_picture, &path, DPI as u32)
                .map_err(|error| error.to_string())?;
            ocr_ms.push(begun.elapsed().as_secs_f64() * 1000.0);
            report.flipped += usize::from(read.flipped);
            let text = join_lines([read.words.iter().collect()], "\n");
            match matched {
                Some(index) => {
                    report.matched += 1;
                    let truth = lines[*index];
                    let kind = report
                        .kinds
                        .entry(truth.kind.name().to_owned())
                        .or_default();
                    kind.matched += 1;
                    let edits = distance(&text, &truth.text);
                    kind.chars += truth.text.chars().count();
                    kind.edits += edits;
                    if edits == 0 {
                        kind.exact += 1;
                    } else if report.mistakes.len() < 40 {
                        // The strip as the worker saw it, for looking at.
                        let strip = musaeum_ocr::straighten(&ocr_picture, &path);
                        let mut pgm = format!(
                            "P5\n{} {}\n255\n",
                            strip.picture.width, strip.picture.height
                        )
                        .into_bytes();
                        pgm.extend_from_slice(&strip.picture.pixels);
                        let _ = std::fs::write(
                            pdf.with_file_name(format!(
                                "strip-{name}-{}.pgm",
                                report.mistakes.len()
                            )),
                            pgm,
                        );
                        report
                            .mistakes
                            .push(format!("page {page}: {:?} read {text:?}", truth.text));
                    }
                }
                None => {
                    let middle = line.baseline[line.baseline.len() / 2];
                    let inside = graphics.iter().any(|[left, top, right, bottom]| {
                        middle[0] >= *left
                            && middle[0] <= *right
                            && middle[1] >= *top
                            && middle[1] <= *bottom
                    });
                    if inside {
                        report.false_in_graphics += 1;
                        report.read_in_graphics.push(text);
                    }
                }
            }
        }
    }
    pool.shutdown();
    report.detect_ms = Millis::of(&detect_ms);
    report.ocr_line_ms = Millis::of(&ocr_ms);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_are_counted_by_characters() {
        assert_eq!(distance("ação", "acao"), 2);
        assert_eq!(distance("", "abc"), 3);
        assert_eq!(distance("kitten", "sitting"), 3);
    }
}
