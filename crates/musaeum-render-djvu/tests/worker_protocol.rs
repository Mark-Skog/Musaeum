//! Drives the real DjVu worker over the real protocol and shared memory, inside the strictest
//! sandbox there is: win32k off and only Microsoft-signed DLLs.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use gen_fixtures::CONTENT_SENTENCE;
use gen_fixtures::djvu::{DJVU_LINES, DJVU_PAGES, DjvuPage};
use musaeum_sandbox::Policy;
use musaeum_shm::tile::{SCALE_ONE, page_pixels};
use musaeum_shm::wire::{FatalCode, PROTOCOL_VERSION, TileFailure, WorkerMessage};
use musaeum_shm::{ContentClass, PageGeometry, TILE_SIZE, TileKey};
use musaeum_worker::harness::Worker;

const POLICY: Policy = Policy::LockdownParser;

fn exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_musaeum-render-djvu"))
}

fn fixture(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("djvu-{test}"));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("book.djvu");
    gen_fixtures::djvu::write_djvu_fixture(&path).unwrap();
    path
}

/// The scale that shows one pixel of the page per pixel of the tile.
fn native(page: &DjvuPage) -> u32 {
    (f64::from(SCALE_ONE) * f64::from(page.dpi) / 72.0).round() as u32
}

/// Renders the tile holding the shown pixel `(x, y)` at the page's native scale, and reads it.
fn pixel_at(worker: &mut Worker, job_seq: u64, page: u32, x: u32, y: u32) -> [u8; 4] {
    let spec = &DJVU_PAGES[page as usize];
    let key = TileKey {
        page,
        scale: native(spec),
        x: (x / TILE_SIZE) as u16,
        y: (y / TILE_SIZE) as u16,
    };
    match worker.request(job_seq, 0, key) {
        WorkerMessage::TileDone { .. } => worker.pixel(0, x % TILE_SIZE, y % TILE_SIZE),
        other => panic!("page {page} at ({x}, {y}): {other:?}"),
    }
}

#[test]
fn every_page_has_its_shown_size_at_its_own_resolution() {
    let (_worker, pages, hints) = Worker::with_geometry(exe(), &fixture("geometry"), POLICY);
    assert_eq!(pages.len(), DJVU_PAGES.len());
    for (index, (page, spec)) in pages.iter().zip(&DJVU_PAGES).enumerate() {
        let (width, height) = spec.shown();
        let points = |pixels: u32| pixels as f32 * 72.0 / spec.dpi as f32;
        assert!(
            (page.width_pt - points(width)).abs() < 0.01
                && (page.height_pt - points(height)).abs() < 0.01,
            "page {index}: {page:?}"
        );
        // At its native scale, the page is exactly as many pixels as were scanned.
        assert_eq!(page_pixels(page.width_pt, native(spec)), u64::from(width));
    }
    // Page 2 is stored upright and shown on its side; page 4 is page 0 scanned at twice the dpi.
    assert!(pages[2].width_pt > pages[2].height_pt);
    assert_eq!(
        pages[4],
        PageGeometry {
            width_pt: pages[0].width_pt / 2.0,
            height_pt: pages[0].height_pt / 2.0
        }
    );
    assert_eq!(hints, (0, 0));
}

#[test]
fn pages_come_out_turned_as_the_file_asks_and_in_their_colors() {
    let mut worker = Worker::started(exe(), &fixture("colors"), POLICY);
    let mut job_seq = 0;
    for (index, spec) in DJVU_PAGES.iter().enumerate() {
        let (width, height) = spec.shown();
        for (x, y) in [
            (width / 4, 3 * height / 4),
            (3 * width / 4, 3 * height / 4),
            (width / 4, height / 2 + 20),
            (3 * width / 4, height - 30),
        ] {
            job_seq += 1;
            let pixel = pixel_at(&mut worker, job_seq, index as u32, x, y);
            let (sx, sy) = spec.stored_point(x, y);
            let expected = if spec.background {
                spec.quadrant(sx, sy)
            } else {
                [255; 3]
            };
            // IW44 is a lossy wavelet code, close to JPEG at these settings.
            let off = (0..3)
                .map(|c| pixel[c].abs_diff(expected[c]))
                .max()
                .unwrap();
            assert!(
                off <= 24 && pixel[3] == 255,
                "page {index} at ({x}, {y}), stored ({sx}, {sy}): {pixel:?}, expected {expected:?}"
            );
        }
    }
}

#[test]
fn the_text_layer_names_the_ink_where_it_is() {
    let mut worker = Worker::started(exe(), &fixture("text"), POLICY);
    let sentence: Vec<&str> = CONTENT_SENTENCE.split(' ').collect();
    let mut job_seq = 100;
    for (index, spec) in DJVU_PAGES.iter().enumerate() {
        let layer = worker.text(index as u64 + 1, 1, index as u32).unwrap();
        if !spec.text {
            assert!(layer.words.is_empty(), "page {index}");
            continue;
        }
        assert_eq!(layer.words.len(), DJVU_LINES as usize * sentence.len());
        let first: Vec<&str> = layer.words[..sentence.len()]
            .iter()
            .map(|word| word.text.as_str())
            .collect();
        assert_eq!(first, sentence, "page {index}");
        assert!(layer.words[sentence.len() - 1].line_end);

        // The middle of every word's box, as shown, is black: the text layer and the drawing turn
        // together.
        let (width, height) = spec.shown();
        for word in layer.words.iter().step_by(4) {
            let [left, top, right, bottom] = word.rect;
            let x = ((left + right) / 2.0 * width as f32) as u32;
            let y = ((top + bottom) / 2.0 * height as f32) as u32;
            job_seq += 1;
            let pixel = pixel_at(&mut worker, job_seq, index as u32, x, y);
            assert!(
                pixel[..3].iter().all(|&channel| channel < 60),
                "page {index}, word {:?} at ({x}, {y}): {pixel:?}",
                word.text
            );
        }
    }
}

#[test]
fn pages_with_a_text_layer_are_hybrid_and_the_rest_scanned() {
    let mut worker = Worker::started(exe(), &fixture("classes"), POLICY);
    for (index, spec) in DJVU_PAGES.iter().enumerate() {
        match worker.analyze(index as u64 + 1, index as u32) {
            WorkerMessage::PageContent { content, .. } => {
                assert_eq!(content.class().as_str(), spec.class, "page {index}");
                let expected = if spec.text {
                    DJVU_LINES as usize * CONTENT_SENTENCE.replace(' ', "").len()
                } else {
                    0
                };
                assert_eq!(content.text_chars as usize, expected);
                assert_eq!(ContentClass::from_name(spec.class), Some(content.class()));
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn what_is_not_a_bundled_djvu_is_refused_when_it_opens() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("djvu-refused");
    fs::create_dir_all(&dir).unwrap();
    for (name, bytes) in [
        ("not-djvu.djvu", b"%PDF-1.7 not a djvu".to_vec()),
        ("empty.djvu", Vec::new()),
        (
            "cut.djvu",
            gen_fixtures::djvu::djvu_bytes().unwrap()[..200].to_vec(),
        ),
    ] {
        let path = dir.join(name);
        fs::write(&path, &bytes).unwrap();
        let mut worker = Worker::spawn(exe(), &path, POLICY, 2, false, PROTOCOL_VERSION);
        match worker.receive() {
            Some(WorkerMessage::Fatal {
                code: FatalCode::DocumentOpen,
                ..
            }) => {}
            // A cut bundle may still open, with every page it lost failing alone.
            Some(WorkerMessage::Ready { page_count, .. }) if name == "cut.djvu" => {
                assert!(page_count as usize <= DJVU_PAGES.len());
            }
            other => panic!("{name}: {other:?}"),
        }
    }
}

#[test]
fn cancelled_and_bad_jobs_fail_alone() {
    let mut worker = Worker::started(exe(), &fixture("bad"), POLICY);
    worker
        .slab
        .cancel_cell(0)
        .unwrap()
        .store(5, Ordering::Release);
    let tile = TileKey {
        page: 0,
        scale: SCALE_ONE,
        x: 0,
        y: 0,
    };
    assert!(matches!(
        worker.request(5, 2, tile),
        WorkerMessage::TileCancelled { .. }
    ));
    assert!(worker.slot_untouched(2));
    assert!(matches!(
        worker.request(6, 0, TileKey { page: 99, ..tile }),
        WorkerMessage::TileFailed {
            failure: TileFailure::OutOfBounds,
            ..
        }
    ));
    assert!(matches!(
        worker.analyze(7, 99),
        WorkerMessage::AnalysisFailed {
            failure: TileFailure::OutOfBounds,
            ..
        }
    ));
    assert!(matches!(
        worker.request(8, 0, tile),
        WorkerMessage::TileDone { .. }
    ));
}
