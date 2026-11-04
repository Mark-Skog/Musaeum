//! Drives the real comic worker over the real protocol and shared memory, inside the strictest
//! sandbox there is: win32k off and only Microsoft-signed DLLs.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use fast_image_resize::images::Image;
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use gen_fixtures::comics::{
    self, ArchiveEntry, COMIC_PAGES, ComicPage, PageEncoding, in_transparent_border, quadrant_color,
};
use musaeum_render_raster::comic::decode_upright;
use musaeum_sandbox::Policy;
use musaeum_shm::tile::page_pixels;
use musaeum_shm::wire::{
    FatalCode, HostMessage, PROTOCOL_VERSION, RenderJob, TileFailure, WorkerMessage,
};
use musaeum_shm::{ContentClass, TILE_SIZE, TileKey};
use musaeum_worker::harness::Worker;

/// 300 pixels per inch over 72 points per inch, in the units of a tile's scale: one pixel of
/// the image per pixel of the tile.
const NATIVE: u32 = 17_067;
const POLICY: Policy = Policy::LockdownParser;

fn exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_musaeum-render-raster"))
}

fn fixtures(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("raster-{test}"));
    fs::create_dir_all(&dir).unwrap();
    comics::write_comic_fixtures(&dir).unwrap();
    dir
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

fn tile(page: u32, scale: u32, x: u16, y: u16) -> TileKey {
    TileKey { page, scale, x, y }
}

/// The color a page should show at `(x, y)`, and how far a decoder may stray from it.
fn expected(page: &ComicPage, x: u32, y: u32) -> ([u8; 3], u8) {
    let [r, g, b] = quadrant_color(page, x, y);
    match page.encoding {
        PageEncoding::Png => ([r, g, b], 0),
        PageEncoding::PngTransparent if in_transparent_border(page, x, y) => ([255; 3], 0),
        PageEncoding::PngTransparent => ([r, g, b], 0),
        PageEncoding::JpegGray => {
            let luma =
                ((u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000) as u8;
            ([luma; 3], 10)
        }
        // Through CMYK and back, with JPEG's own loss on four channels.
        PageEncoding::JpegCmyk => ([r, g, b], 24),
        PageEncoding::Jpeg | PageEncoding::JpegRotated => ([r, g, b], 12),
    }
}

#[test]
fn every_archive_format_gives_the_same_pages_in_reading_order() {
    let dir = fixtures("formats");
    for name in [
        "comic.cbz",
        "comic-stored.cbz",
        "comic-rar4.cbr",
        "comic-rar5.cbr",
        "zip-named.cbr",
    ] {
        let (_worker, pages, hints) = Worker::with_geometry(exe(), &dir.join(name), POLICY);
        assert_eq!(pages.len(), COMIC_PAGES.len(), "{name}");
        for (index, (page, spec)) in pages.iter().zip(&COMIC_PAGES).enumerate() {
            // At 300 pixels per inch, a pixel is 0.24 points.
            let (width, height) = (spec.width as f32 * 0.24, spec.height as f32 * 0.24);
            assert!(
                (page.width_pt - width).abs() < 0.01 && (page.height_pt - height).abs() < 0.01,
                "{name}, page {index} ({}): {page:?}",
                spec.name
            );
        }
        assert_eq!(hints, (0, 0), "{name} says nothing about direction");
    }
    let (_worker, _pages, hints) = Worker::with_geometry(exe(), &dir.join("manga.cbz"), POLICY);
    assert_eq!(hints, (0, 2), "ComicInfo.xml asks for right to left");
}

#[test]
fn pages_come_out_upright_and_in_their_colors() {
    let dir = fixtures("colors");
    for name in ["comic.cbz", "comic-rar5.cbr"] {
        let mut worker = Worker::started(exe(), &dir.join(name), POLICY);
        let mut job_seq = 0;
        for (index, page) in COMIC_PAGES.iter().enumerate() {
            let (width, height) = (page.width, page.height);
            for (x, y) in [
                (width / 4, height / 4),
                (3 * width / 4, height / 4),
                (width / 4, 3 * height / 4),
                (3 * width / 4, 3 * height / 4),
                (2, 2),
            ] {
                job_seq += 1;
                let key = tile(
                    index as u32,
                    NATIVE,
                    (x / TILE_SIZE) as u16,
                    (y / TILE_SIZE) as u16,
                );
                assert!(
                    matches!(
                        worker.request(job_seq, 0, key),
                        WorkerMessage::TileDone { .. }
                    ),
                    "{name}, page {index}"
                );
                let pixel = worker.pixel(0, x % TILE_SIZE, y % TILE_SIZE);
                let (color, tolerance) = expected(page, x, y);
                let off = (0..3).map(|c| pixel[c].abs_diff(color[c])).max().unwrap();
                assert!(
                    off <= tolerance && pixel[3] == 255,
                    "{name}, page {index} ({}) at ({x}, {y}): {pixel:?}, expected {color:?}",
                    page.name
                );
            }
        }
    }
}

#[test]
fn tiles_drawn_one_by_one_match_the_page_drawn_whole() {
    let dir = fixtures("seams");
    let mut worker = Worker::started(exe(), &dir.join("comic.cbz"), POLICY);
    let spec = &COMIC_PAGES[2];
    let reference = decode_upright(&comics::encode_page(spec).unwrap()).unwrap();

    // Smaller than the image, so every pixel is a blend, and over several tiles each way.
    for scale in [NATIVE * 4 / 5, NATIVE * 9 / 10 + 7] {
        let (width_pt, height_pt) = (spec.width as f32 * 0.24, spec.height as f32 * 0.24);
        let (width, height) = (
            page_pixels(width_pt, scale) as u32,
            page_pixels(height_pt, scale) as u32,
        );
        let mut whole = Image::new(width, height, PixelType::U8x4);
        Resizer::new()
            .resize(
                &reference,
                &mut whole,
                &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3)),
            )
            .unwrap();

        let mut worst = 0u8;
        let mut job_seq = 100;
        for ty in 0..height.div_ceil(TILE_SIZE) {
            for tx in 0..width.div_ceil(TILE_SIZE) {
                job_seq += 1;
                let key = tile(2, scale, tx as u16, ty as u16);
                assert!(matches!(
                    worker.request(job_seq, 1, key),
                    WorkerMessage::TileDone { .. }
                ));
                let tile_width = (width - tx * TILE_SIZE).min(TILE_SIZE);
                let tile_height = (height - ty * TILE_SIZE).min(TILE_SIZE);
                let pixels = worker.rect(1, tile_width, tile_height);
                for y in 0..tile_height {
                    for x in 0..tile_width {
                        let at = ((y * tile_width + x) * 4) as usize;
                        let page_at =
                            (((ty * TILE_SIZE + y) * width + tx * TILE_SIZE + x) * 4) as usize;
                        for channel in 0..4 {
                            worst = worst.max(
                                pixels[at + channel].abs_diff(whole.buffer()[page_at + channel]),
                            );
                        }
                    }
                }
            }
        }
        assert!(worst <= 1, "scale {scale}: a pixel differs by {worst}");
    }
}

#[test]
fn every_page_is_a_scan_without_a_text_layer() {
    let dir = fixtures("content");
    let mut worker = Worker::started(exe(), &dir.join("comic-rar4.cbr"), POLICY);
    for page in 0..COMIC_PAGES.len() as u32 {
        match worker.analyze(u64::from(page) + 1, page) {
            WorkerMessage::PageContent { content, .. } => {
                assert_eq!(content.class(), ContentClass::Scanned);
            }
            other => panic!("{other:?}"),
        }
        assert!(
            worker
                .text(100 + u64::from(page), 0, page)
                .unwrap()
                .words
                .is_empty()
        );
    }
}

fn open_failure(path: &Path) -> (FatalCode, u32) {
    let mut worker = Worker::spawn(exe(), path, POLICY, 2, false, PROTOCOL_VERSION);
    match worker.receive() {
        Some(WorkerMessage::Fatal { code, detail }) => (code, detail),
        other => panic!("{} should not open: {other:?}", path.display()),
    }
}

#[test]
fn archives_that_are_not_comics_are_refused_when_they_open() {
    let dir = fixtures("refused");
    let not_archive = write(
        &dir,
        "not-an-archive.cbz",
        b"%PDF-1.7 something else entirely",
    );
    assert_eq!(open_failure(&not_archive), (FatalCode::DocumentOpen, 1));

    let text_only = comics::zip_bytes(
        &[ArchiveEntry {
            name: "readme.txt".to_owned(),
            data: b"no pictures here".to_vec(),
        }],
        true,
    )
    .unwrap();
    let text_only = write(&dir, "text-only.cbz", &text_only);
    assert_eq!(open_failure(&text_only), (FatalCode::DocumentOpen, 2));

    let whole = fs::read(dir.join("comic.cbz")).unwrap();
    let cut = write(&dir, "cut.cbz", &whole[..whole.len() / 2]);
    assert_eq!(open_failure(&cut).0, FatalCode::DocumentOpen);

    // An empty file maps as no bytes at all, which is not an archive.
    let empty = write(&dir, "empty.cbr", &[]);
    assert_eq!(open_failure(&empty), (FatalCode::DocumentOpen, 1));
}

#[test]
fn a_damaged_page_fails_alone() {
    let dir = fixtures("damaged");
    let mut entries = comics::comic_entries(false).unwrap();
    let damaged = entries
        .iter_mut()
        .find(|entry| entry.name == "p2.png")
        .unwrap();
    damaged.data = b"\x89PNG\r\n\x1a\n but not really".to_vec();
    let path = write(
        &dir,
        "damaged.cbz",
        &comics::zip_bytes(&entries, true).unwrap(),
    );

    let (mut worker, pages, _) = Worker::with_geometry(exe(), &path, POLICY);
    // Its size is unknown, so it takes the first page's, and the layout stays whole.
    assert_eq!(pages[1], pages[0]);
    assert!(matches!(
        worker.request(1, 0, tile(1, NATIVE / 4, 0, 0)),
        WorkerMessage::TileFailed {
            failure: TileFailure::DecodeFailed,
            ..
        }
    ));
    assert!(matches!(
        worker.request(2, 0, tile(2, NATIVE / 4, 0, 0)),
        WorkerMessage::TileDone { .. }
    ));
}

#[test]
fn a_cancelled_job_is_reported_without_touching_its_slot() {
    let dir = fixtures("cancel");
    let mut worker = Worker::started(exe(), &dir.join("comic.cbz"), POLICY);
    worker
        .slab
        .cancel_cell(0)
        .unwrap()
        .store(5, Ordering::Release);
    assert!(matches!(
        worker.request(5, 3, tile(0, NATIVE, 0, 0)),
        WorkerMessage::TileCancelled { .. }
    ));
    assert!(worker.slot_untouched(3));
    assert!(matches!(
        worker.request(6, 3, tile(0, NATIVE, 0, 0)),
        WorkerMessage::TileDone { .. }
    ));
}

#[test]
fn background_renders_do_not_keep_the_page_and_bad_requests_fail_alone() {
    let dir = fixtures("background");
    let mut worker = Worker::started(exe(), &dir.join("comic.cbz"), POLICY);
    let render = |worker: &mut Worker, job_seq, background| {
        worker.send(HostMessage::Render(RenderJob {
            job_seq,
            slot: 0,
            tile: tile(4, NATIVE / 2, 0, 0),
            background,
        }));
        match worker.receive() {
            Some(WorkerMessage::TileDone { load_us, .. }) => load_us,
            other => panic!("{other:?}"),
        }
    };
    assert!(render(&mut worker, 1, true) > 0);
    assert!(
        render(&mut worker, 2, true) > 0,
        "a background job kept nothing"
    );
    assert!(render(&mut worker, 3, false) > 0);
    assert_eq!(
        render(&mut worker, 4, false),
        0,
        "a foreground job kept the page"
    );

    for (job_seq, key) in [
        (10, tile(99, NATIVE, 0, 0)),
        (11, tile(0, NATIVE, 9, 0)),
        (12, tile(0, 0, 0, 0)),
    ] {
        assert!(matches!(
            worker.request(job_seq, 0, key),
            WorkerMessage::TileFailed {
                failure: TileFailure::OutOfBounds,
                ..
            }
        ));
    }
    assert!(matches!(
        worker.request(13, 99, tile(0, NATIVE, 0, 0)),
        WorkerMessage::TileFailed {
            failure: TileFailure::InvalidSlot,
            ..
        }
    ));
    assert!(matches!(
        worker.request(14, 0, tile(0, NATIVE, 0, 0)),
        WorkerMessage::TileDone { .. }
    ));
}
