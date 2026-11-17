//! The OCR worker as the app runs it: the real binary, in the sandbox, reading lines of the OCR
//! book drawn by PDFium, level and turned, through the host's client.

use std::path::{Path, PathBuf};

use gen_fixtures::ocr::{LineKind, OcrLine, PAGE, ocr_book};
use musaeum_ocr::{Gray, LinePath, OcrClient, OcrConfig, join_lines, read_line};
use musaeum_render_pdf::engine::{Document, Pdfium};
use musaeum_sandbox::Policy;

const DPI: f32 = 300.0;

fn exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_musaeum-ocr"))
}

fn models() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/tesseract/tessdata")
}

fn client() -> OcrClient {
    OcrClient::new(OcrConfig {
        exe: Some(exe().to_path_buf()),
        models: Some(models()),
        policy: Policy::LockdownParser,
        ..OcrConfig::default()
    })
}

/// The pages of the OCR book, drawn whole at 300 dpi, gray. PDFium allows one thread at a time in
/// a process, so they are drawn once for every test.
fn pages() -> (Vec<Gray>, Vec<OcrLine>) {
    static DRAWN: std::sync::OnceLock<(Vec<Gray>, Vec<OcrLine>)> = std::sync::OnceLock::new();
    DRAWN.get_or_init(draw).clone()
}

fn draw() -> (Vec<Gray>, Vec<OcrLine>) {
    let book = ocr_book().unwrap();
    let pdfium = Pdfium::load(&exe().with_file_name("pdfium.dll")).unwrap();
    let mut document = Document::open(&pdfium, &book.pdf).unwrap();
    let scale = (4096.0 * DPI / 72.0).round() as u32;
    let pages = (0..document.page_count())
        .map(|page| {
            let (width, height, rgba) = document.render_page(page, scale).unwrap();
            Gray {
                width,
                height,
                pixels: rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|px| {
                        ((77 * u32::from(px[0]) + 150 * u32::from(px[1]) + 29 * u32::from(px[2]))
                            >> 8) as u8
                    })
                    .collect(),
            }
        })
        .collect();
    (pages, book.lines)
}

/// A line of the book as the detector would give it, in pixels of its page.
fn path_of(line: &OcrLine) -> LinePath {
    let k = DPI / 72.0;
    let at = |point: [f32; 2]| [point[0] * k, (PAGE[1] - point[1]) * k];
    LinePath {
        baseline: vec![at(line.start), at(line.end)],
        ascent: line.size * 0.72 * k,
        descent: line.size * 0.22 * k,
        x_height: line.size * 0.47 * k,
    }
}

#[test]
fn the_sandboxed_worker_reads_level_and_turned_lines() {
    let (pages, lines) = pages();
    let mut client = client();
    assert!(client.languages().contains(&"por".to_owned()));
    let mut wanted: Vec<&OcrLine> = lines
        .iter()
        .filter(|line| line.page == 0 && line.kind == LineKind::Body)
        .take(3)
        .collect();
    // The first line of each turned block.
    let mut blocks = std::collections::BTreeSet::new();
    wanted.extend(
        lines
            .iter()
            .filter(|line| line.kind == LineKind::Turned && blocks.insert(line.block)),
    );
    assert_eq!(wanted.len(), 7);
    let mut mistakes = Vec::new();
    for line in wanted {
        let started = std::time::Instant::now();
        let read = read_line(
            &mut client,
            "por",
            &pages[line.page as usize],
            &path_of(line),
            DPI as u32,
        )
        .unwrap();
        let text = join_lines([read.words.iter().collect()], "\n");
        eprintln!(
            "{:>6.1}° {:>4} ms, confidence {:.0}: {text}",
            line.angle_degrees(),
            started.elapsed().as_millis(),
            read.confidence
        );
        if text != line.text {
            mistakes.push(format!("{:?}\n  read {text:?}", line.text));
        }
        // Words cover the line in order, inside it.
        assert!(!read.flipped);
        assert!(
            read.words
                .windows(2)
                .all(|pair| pair[0].to <= pair[1].from + 0.01)
        );
        assert!(read.words.first().is_some_and(|word| word.from > -0.05));
        assert!(read.words.last().is_some_and(|word| word.to < 1.05));
    }
    assert!(mistakes.is_empty(), "{}", mistakes.join("\n"));
    assert_eq!(client.restarts(), 0);
}

#[test]
fn a_line_read_the_wrong_way_round_is_turned_back() {
    let (pages, lines) = pages();
    let mut client = client();
    let line = lines
        .iter()
        .find(|line| line.page == 0 && line.kind == LineKind::Body)
        .unwrap();
    // The detector took it from its end to its start.
    let mut path = path_of(line);
    path.baseline.reverse();
    std::mem::swap(&mut path.ascent, &mut path.descent);
    let read = read_line(&mut client, "por", &pages[0], &path, DPI as u32).unwrap();
    assert!(read.flipped, "{read:?}");
    let text = join_lines([read.words.iter().collect()], "\n");
    assert_eq!(text, line.text);
}
