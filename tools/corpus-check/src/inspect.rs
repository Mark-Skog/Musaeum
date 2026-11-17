//! `--inspect FILE --at PAGES`: the line detector, and with `--ocr` the OCR, on chosen pages of one
//! book of the collection, for looking at what they make of a real page.
//!
//! The pages are numbered as the reader shows them, from 1, as a list and ranges (`14,78,100-120`,
//! or with `+` between them, which `cmd` does not split on).
//! Each is drawn for the detector by the same sandboxed pool and background work the reader uses,
//! and its lines found. The picture is written to the work folder as a PGM, which the detector's
//! `dump` example reads again, and with the lines found drawn over it as a JPEG. With `--ocr`,
//! every line found is read as the reader reads it, at the resolution that brings its small
//! letters to about 22 pixels high, and the lines are printed with what was read, in order. The
//! book is only read: the collection is listed before and after, and the run fails if anything in
//! it differs.

use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use musaeum_core::{BackgroundResult, BackgroundWork, DeliveryMode, DocumentHints, RenderPool};
use musaeum_ocr::{Gray as OcrGray, LinePath, OcrClient, OcrConfig, join_lines, read_line};
use musaeum_shm::ContentClass;
use musaeum_shm::tile::page_pixels;
use musaeum_vision::{DetectOptions, Gray, detect};
use serde_json::json;

use crate::lines::{collapse, overlay};
use crate::render::{self, OpenFailure, config, pool_error, slab};
use crate::snapshot;

/// Longest a few pages may take to be drawn.
const WAIT: Duration = Duration::from_secs(600);

/// Pages drawn at once, so a long book's pictures are not all held together.
const CHUNK: usize = 12;

/// `14,78,100-120` or `14+78+100-120`, numbered from 1, as page indices from 0.
pub fn parse_pages(text: &str) -> Result<Vec<u32>, String> {
    let mut pages = Vec::new();
    for part in text
        .split([',', '+'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let number = |text: &str| -> Result<u32, String> {
            text.trim()
                .parse::<u32>()
                .ok()
                .filter(|&n| n >= 1)
                .ok_or_else(|| format!("--at: {text} is not a page number"))
        };
        match part.split_once('-') {
            Some((from, to)) => {
                let (from, to) = (number(from)?, number(to)?);
                pages.extend((from.min(to)..=from.max(to)).map(|n| n - 1));
            }
            None => pages.push(number(part)? - 1),
        }
    }
    pages.sort_unstable();
    pages.dedup();
    Ok(pages)
}

/// Returns whether the collection was left exactly as it was.
pub fn run_inspect(
    corpus: &Path,
    work: &Path,
    file: &Path,
    pages: &[u32],
    read: bool,
    out: Option<&Path>,
) -> Result<bool, String> {
    let file = std::fs::canonicalize(file).map_err(|error| format!("the book: {error}"))?;
    if !file.starts_with(corpus) {
        return Err("the book must be inside the corpus, which is listed before and after".into());
    }
    eprintln!("listing {} before anything else", corpus.display());
    let before = snapshot::take(corpus).map_err(|error| format!("listing: {error}"))?;
    let folder = work.join("inspect");
    std::fs::create_dir_all(&folder).map_err(|error| format!("the inspect folder: {error}"))?;

    let worker = render::worker_exe();
    let measured = render::measure(&worker, &file).map_err(|failure| format!("{failure:?}"))?;
    let count = measured.pages.len() as u32;
    let pages: Vec<u32> = pages.iter().copied().filter(|&page| page < count).collect();
    eprintln!("{count} pages; inspecting {}", pages.len());
    let mut client = read.then(|| OcrClient::new(OcrConfig::default()));
    let mut rows = Vec::new();
    for chunk in pages.chunks(CHUNK) {
        let pool =
            start(&worker, &file, &measured.pages).map_err(|failure| format!("{failure:?}"))?;
        let drawn = draw_for_lines(&pool, chunk);
        let drawn = match drawn {
            Ok(drawn) => drawn,
            Err(error) => {
                pool.shutdown();
                return Err(error);
            }
        };
        for (page, class, picture) in drawn {
            let layer = pool
                .request_chars(page)
                .recv_timeout(WAIT)
                .ok()
                .and_then(Result::ok);
            let rotation = layer.as_ref().map_or(0, |layer| layer.rotation);
            let begun = Instant::now();
            let found = detect(
                &picture,
                &DetectOptions {
                    rotation,
                    ..DetectOptions::default()
                },
            );
            let ms = begun.elapsed().as_secs_f64() * 1000.0;
            let shown = page + 1;
            let (width, height) = (picture.width, picture.height);
            let mut pgm = format!("P5\n{width} {height}\n255\n").into_bytes();
            pgm.extend_from_slice(&picture.pixels);
            let _ = std::fs::write(folder.join(format!("{shown:04}.pgm")), pgm);
            let drawing = overlay(&picture, &found.pixel_lines, &[]);
            if let Ok(jpeg) = musaeum_core::thumbnail::encode_jpeg(width, height, &drawing) {
                let _ = std::fs::write(folder.join(format!("{shown:04}.jpg")), jpeg);
            }
            println!(
                "page {shown} ({class}): {width}x{height}, glyph {:.1}, {} lines, turned blocks {:?}, {ms:.1} ms",
                found.stats.glyph_height,
                found.pixel_lines.len(),
                found.stats.turned_blocks,
            );
            let texts = match client.as_mut() {
                Some(client) if !found.pixel_lines.is_empty() => read_all(
                    client,
                    &pool,
                    page,
                    &measured.pages,
                    &found.pixel_lines,
                    width,
                ),
                _ => vec![String::new(); found.pixel_lines.len()],
            };
            let mut lines = Vec::new();
            for (index, (line, text)) in found.pixel_lines.iter().zip(&texts).enumerate() {
                let (first, last) = (line.baseline[0], line.baseline[line.baseline.len() - 1]);
                let angle = (last[1] - first[1]).atan2(last[0] - first[0]).to_degrees();
                println!(
                    "  #{index:<3} ({:>5.0},{:>5.0}) -> ({:>5.0},{:>5.0}) {angle:>6.1}° x {:>4.1} | {text}",
                    first[0], first[1], last[0], last[1], line.x_height
                );
                lines.push(json!({
                    "index": index,
                    "baseline": line.baseline,
                    "xHeight": line.x_height,
                    "angle": angle,
                    "text": text,
                }));
            }
            rows.push(json!({
                "page": shown,
                "class": class,
                "rotation": rotation,
                "width": width,
                "height": height,
                "glyphHeight": found.stats.glyph_height,
                "detectMs": ms,
                "lines": lines,
            }));
        }
        pool.shutdown();
    }

    eprintln!("listing {} again", corpus.display());
    let after = snapshot::take(corpus).map_err(|error| format!("listing again: {error}"))?;
    let comparison = snapshot::compare(&before, &after);
    let unchanged = comparison.unchanged();
    eprintln!(
        "the collection is {}",
        if unchanged { "unchanged" } else { "CHANGED" }
    );
    if let Some(out) = out {
        let report = json!({
            "file": file.strip_prefix(corpus).unwrap_or(&file).display().to_string(),
            "pages": rows,
            "corpusUnchanged": unchanged,
        });
        let text = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
        std::fs::write(out, text).map_err(|error| format!("the report: {error}"))?;
    }
    Ok(unchanged)
}

fn start(
    worker: &Path,
    pdf: &Path,
    pages: &[musaeum_shm::PageGeometry],
) -> Result<RenderPool, OpenFailure> {
    let mut configuration = config(worker, pdf, 2);
    configuration.known_geometry = Some((pages.to_vec(), DocumentHints::default()));
    RenderPool::start(
        configuration,
        slab(32).map_err(OpenFailure::Core)?,
        DeliveryMode::Pull { max_batch: 1 },
    )
    .map_err(pool_error)
}

/// The pages' pictures for the detector, with their classes, as the reader's background work
/// draws them.
fn draw_for_lines(
    pool: &RenderPool,
    pages: &[u32],
) -> Result<Vec<(u32, &'static str, Gray)>, String> {
    let (sender, results) = mpsc::channel();
    pool.start_background(
        BackgroundWork {
            analyze: pages.to_vec(),
            lines: pages.to_vec(),
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
            }) => pictures.push((
                page,
                Gray {
                    width,
                    height,
                    pixels: gray,
                },
            )),
            Ok(BackgroundResult::Finished) => break,
            Ok(_) => {}
            Err(_) => return Err("the background work did not finish".to_owned()),
        }
    }
    pictures.sort_by_key(|(page, _)| *page);
    Ok(pictures
        .into_iter()
        .map(|(page, picture)| {
            let class = match classes.get(&page) {
                Some(ContentClass::Scanned) => "scanned",
                Some(ContentClass::Hybrid) => "hybrid",
                Some(_) => "text",
                None => "unknown",
            };
            (page, class, picture)
        })
        .collect())
}

/// Every line found, read as the reader reads it.
fn read_all(
    client: &mut OcrClient,
    pool: &RenderPool,
    page: u32,
    pages: &[musaeum_shm::PageGeometry],
    found: &[musaeum_vision::lines::PixelLine],
    width: u32,
) -> Vec<String> {
    let size = pages[page as usize];
    let mut heights: Vec<f32> = found.iter().map(|line| line.x_height).collect();
    heights.sort_by(f32::total_cmp);
    let x_height_pt = heights[heights.len() / 2] * size.width_pt / width as f32;
    let mut dpi = (72.0 * 22.0 / x_height_pt.max(0.5)).clamp(300.0, 600.0);
    let area = size.width_pt * size.height_pt;
    let most = musaeum_core::MAX_REGION_PIXELS as f32 * 0.9;
    if area * (dpi / 72.0).powi(2) > most {
        dpi = 72.0 * (most / area).sqrt();
    }
    let scale = crate::ocr::scale_of(dpi);
    let (ocr_w, ocr_h, gray) = match crate::ocr::draw_page(pool, page, scale) {
        Ok(drawn) => drawn,
        Err(error) => {
            eprintln!("page {} could not be drawn for the OCR: {error}", page + 1);
            return vec![String::new(); found.len()];
        }
    };
    let picture = OcrGray {
        width: ocr_w,
        height: ocr_h,
        pixels: gray,
    };
    let k = page_pixels(size.width_pt, scale) as f32 / width as f32;
    // With MUSAEUM_INSPECT_STRIPS set to a folder, the strip the OCR gets for each line goes there,
    // and what it says of each word is printed, for looking at.
    let strips = std::env::var_os("MUSAEUM_INSPECT_STRIPS");
    found
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let path = LinePath {
                baseline: line.baseline.iter().map(|p| [p[0] * k, p[1] * k]).collect(),
                ascent: line.ascent * k,
                descent: line.descent * k,
                x_height: line.x_height * k,
            };
            if let Some(folder) = &strips {
                let strip = musaeum_ocr::straighten(&picture, &path);
                let (w, h) = (strip.picture.width, strip.picture.height);
                let mut pgm = format!("P5\n{w} {h}\n255\n").into_bytes();
                pgm.extend_from_slice(&strip.picture.pixels);
                let name = format!("strip-{:04}-{index:03}.pgm", page + 1);
                let _ = std::fs::write(Path::new(folder).join(name), pgm);
            }
            match read_line(client, "por+eng", &picture, &path, dpi.round() as u32) {
                Ok(read) => {
                    if strips.is_some() {
                        let words: Vec<(&str, f32)> = read
                            .words
                            .iter()
                            .map(|word| (word.text.as_str(), word.confidence.round()))
                            .collect();
                        eprintln!("  line {index}: {words:?}, flipped {}", read.flipped);
                    }
                    collapse(&join_lines([read.words.iter().collect()], "\n"))
                }
                Err(error) => format!("<{error:?}>"),
            }
        })
        .collect()
}
