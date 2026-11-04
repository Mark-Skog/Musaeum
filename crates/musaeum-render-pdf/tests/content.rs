//! Classifying pages and reading text layers, with the real worker binary and the real PDFium,
//! inside the sandbox the app runs it in.

use std::fs;
use std::path::{Path, PathBuf};

use gen_fixtures::{CONTENT_CLASSES, CONTENT_SENTENCE};
use musaeum_sandbox::Policy;
use musaeum_shm::wire::{HostMessage, RenderJob, TileFailure, WorkerMessage};
use musaeum_shm::{ContentClass, TileKey};
use musaeum_worker::harness::Worker;

fn exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_musaeum-render-pdf"))
}

fn fixture(test: &str, write: impl FnOnce(&Path) -> std::io::Result<()>) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("content-{test}"));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fixture.pdf");
    write(&path).unwrap();
    path
}

fn classify(worker: &mut Worker, page: u32) -> (ContentClass, u32, f32) {
    match worker.analyze(u64::from(page) + 1, page) {
        WorkerMessage::PageContent {
            page: answered,
            content,
            ..
        } => {
            assert_eq!(answered, page);
            (content.class(), content.text_chars, content.image_coverage)
        }
        other => panic!("page {page} was not classified: {other:?}"),
    }
}

#[test]
fn every_kind_of_page_gets_its_class() {
    let pdf = fixture("classes", gen_fixtures::write_content_fixture);
    let mut worker = Worker::started(exe(), &pdf, Policy::Lockdown);
    for (page, expected) in CONTENT_CLASSES.iter().enumerate() {
        let (class, chars, coverage) = classify(&mut worker, page as u32);
        assert_eq!(
            class.as_str(),
            *expected,
            "page {page}: {chars} characters, {:.0}% covered by pictures",
            coverage * 100.0
        );
    }

    // What the numbers behind those classes look like.
    let (_, chars, coverage) = classify(&mut worker, 2);
    assert!(coverage > 0.98, "a scan covers its page: {coverage}");
    assert_eq!(chars as usize, 30 * CONTENT_SENTENCE.replace(' ', "").len());
    let (_, chars, coverage) = classify(&mut worker, 5);
    assert!((coverage - 0.5).abs() < 0.03, "half a page: {coverage}");
    assert!(chars > 0);
    let (_, chars, coverage) = classify(&mut worker, 6);
    assert_eq!((chars, coverage), (0, 0.0));
}

#[test]
fn the_other_fixtures_are_what_they_look_like() {
    let scan = fixture("scan", |path| gen_fixtures::write_scan_fixture(path, 3));
    let mut worker = Worker::started(exe(), &scan, Policy::Lockdown);
    for page in 0..3 {
        assert_eq!(classify(&mut worker, page).0, ContentClass::Scanned);
    }

    let vector = fixture("vector", |path| gen_fixtures::write_vector_fixture(path, 2));
    let mut worker = Worker::started(exe(), &vector, Policy::Lockdown);
    assert_eq!(classify(&mut worker, 0).0, ContentClass::Text);
}

#[test]
fn text_layers_come_as_words_with_boxes_in_reading_order() {
    let pdf = fixture("words", gen_fixtures::write_content_fixture);
    let mut worker = Worker::started(exe(), &pdf, Policy::Lockdown);
    let sentence: Vec<&str> = CONTENT_SENTENCE.split(' ').collect();

    for page in [0, 2, 4] {
        let layer = worker.text(10 + u64::from(page), 0, page).unwrap();
        assert_eq!(layer.words.len(), 30 * sentence.len(), "page {page}");
        let first_line: Vec<&str> = layer.words[..sentence.len()]
            .iter()
            .map(|word| word.text.as_str())
            .collect();
        assert_eq!(first_line, sentence, "page {page}");
        assert!(
            layer.words[sentence.len() - 1].line_end,
            "a line ends after the last word"
        );
        assert!(!layer.words[0].line_end);

        for pair in layer.words.windows(2) {
            let [left, top, right, bottom] = pair[0].rect;
            assert!(left < right && top < bottom, "page {page}: {:?}", pair[0]);
            if pair[0].line_end {
                assert!(pair[1].rect[1] > top, "the next line is lower down");
            } else {
                assert!(pair[1].rect[0] > left, "the next word is further right");
            }
        }
        // 60 pt from the left of a 595 pt page, near the top of an 842 pt one.
        let [left, top, ..] = layer.words[0].rect;
        assert!((left - 60.0 / 595.0).abs() < 0.01, "{left}");
        assert!(top < 0.08, "{top}");
    }

    let scanned = worker.text(20, 0, 1).unwrap();
    assert!(
        scanned.words.is_empty(),
        "a scan without OCR has no text layer"
    );
}

#[test]
fn boxes_follow_the_page_as_it_is_shown() {
    let pdf = fixture("rotated", gen_fixtures::write_content_fixture);
    let mut worker = Worker::started(exe(), &pdf, Policy::Lockdown);
    let layer = worker.text(1, 0, 7).unwrap();
    let [left, top, right, bottom] = layer.words[0].rect;
    // Written at the top left of the page as stored; turned a quarter clockwise, that is the
    // top right, reading downwards.
    assert!(left > 0.85 && right <= 1.0, "{:?}", layer.words[0].rect);
    assert!(top > 0.08 && top < 0.15, "{:?}", layer.words[0].rect);
    assert!(
        bottom - top > right - left,
        "the word now runs down the page"
    );
}

#[test]
fn asking_about_pages_that_do_not_exist_fails_alone() {
    let pdf = fixture("bounds", gen_fixtures::write_content_fixture);
    let mut worker = Worker::started(exe(), &pdf, Policy::Lockdown);
    assert!(matches!(
        worker.analyze(1, 99),
        WorkerMessage::AnalysisFailed {
            failure: TileFailure::OutOfBounds,
            ..
        }
    ));
    assert!(matches!(
        worker.text(2, 0, 99),
        Err(WorkerMessage::TileFailed {
            failure: TileFailure::OutOfBounds,
            ..
        })
    ));
    assert!(matches!(
        worker.text(3, 99, 0),
        Err(WorkerMessage::TileFailed {
            failure: TileFailure::InvalidSlot,
            ..
        })
    ));
    // Still answering.
    assert_eq!(classify(&mut worker, 0).0, ContentClass::Text);
}

#[test]
fn a_background_render_draws_the_same_pixels_without_keeping_the_page() {
    let pdf = fixture("background", gen_fixtures::write_content_fixture);
    let mut worker = Worker::started(exe(), &pdf, Policy::Lockdown);
    let tile = TileKey {
        page: 1,
        scale: 1024,
        x: 0,
        y: 0,
    };
    for (job_seq, slot, background) in [(1, 0, false), (2, 1, true)] {
        worker.send(HostMessage::Render(RenderJob {
            job_seq,
            slot,
            tile,
            background,
        }));
        assert!(matches!(
            worker.receive(),
            Some(WorkerMessage::TileDone { .. })
        ));
    }
    assert_eq!(worker.rect(0, 149, 211), worker.rect(1, 149, 211));

    // A background job loads its page anew each time, since it does not keep it.
    for job_seq in [3, 4] {
        worker.send(HostMessage::Render(RenderJob {
            job_seq,
            slot: 2,
            tile: TileKey { page: 3, ..tile },
            background: true,
        }));
        match worker.receive() {
            Some(WorkerMessage::TileDone { load_us, .. }) => {
                assert!(load_us > 0, "the page was loaded again")
            }
            other => panic!("{other:?}"),
        }
    }
}
