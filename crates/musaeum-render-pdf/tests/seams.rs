//! Tiles put back together have to be the page drawn whole: no letter broken at a seam, no line
//! along one. The tiles come from the real worker in its sandbox; the page from PDFium in this
//! process, in one call, without tiles or the progressive renderer.

use std::fs;
use std::path::{Path, PathBuf};

use musaeum_render_pdf::engine::{Document, Pdfium};
use musaeum_sandbox::Policy;
use musaeum_shm::tile::{page_pixels, tile_grid};
use musaeum_shm::wire::WorkerMessage;
use musaeum_shm::{PageGeometry, TILE_SIZE, TileKey};
use musaeum_worker::harness::Worker;

fn exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_musaeum-render-pdf"))
}

/// Pixels that differ, and by how much at most, between page `page` drawn in tiles and whole.
fn compare(pdf: &Path, page: u32, size: PageGeometry, scale: u32) -> (usize, u8) {
    let mut worker = Worker::started(exe(), pdf, Policy::Lockdown);
    let (width, height) = (
        page_pixels(size.width_pt, scale) as u32,
        page_pixels(size.height_pt, scale) as u32,
    );
    let mut tiled = vec![0u8; (width * height * 4) as usize];
    let (columns, rows) = tile_grid(size, scale);
    let mut job_seq = 0;
    for y in 0..rows as u16 {
        for x in 0..columns as u16 {
            job_seq += 1;
            let key = TileKey { page, scale, x, y };
            assert!(matches!(
                worker.request(job_seq, 0, key),
                WorkerMessage::TileDone { .. }
            ));
            let (left, top) = (u32::from(x) * TILE_SIZE, u32::from(y) * TILE_SIZE);
            let (w, h) = ((width - left).min(TILE_SIZE), (height - top).min(TILE_SIZE));
            let pixels = worker.rect(0, w, h);
            for row in 0..h {
                let to = (((top + row) * width + left) * 4) as usize;
                let from = (row * w * 4) as usize;
                tiled[to..to + (w * 4) as usize]
                    .copy_from_slice(&pixels[from..from + (w * 4) as usize]);
            }
        }
    }

    let pdfium = Pdfium::load(&exe().with_file_name("pdfium.dll")).unwrap();
    let bytes = fs::read(pdf).unwrap();
    let mut document = Document::open(&pdfium, &bytes).unwrap();
    let (whole_width, whole_height, whole) = document.render_page(page, scale).unwrap();
    assert_eq!((whole_width, whole_height), (width, height));
    let mut differing = 0;
    let mut worst = 0u8;
    for (a, b) in tiled
        .as_chunks::<4>()
        .0
        .iter()
        .zip(whole.as_chunks::<4>().0)
    {
        let off = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap();
        differing += usize::from(off > 0);
        worst = worst.max(off);
    }
    (differing, worst)
}

#[test]
fn letters_and_scans_cross_tile_seams_unbroken() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("seams");
    fs::create_dir_all(&dir).unwrap();
    let pdf = dir.join("content.pdf");
    gen_fixtures::write_content_fixture(&pdf).unwrap();
    let page = PageGeometry {
        width_pt: 595.0,
        height_pt: 842.0,
    };
    // 1600 pixels across, as a reader sees the page: the "j" of every line crosses the first seam,
    // and the scan of page 2 crosses all of them.
    for (number, what) in [(0, "text"), (2, "a scan with OCR")] {
        let (differing, worst) = compare(&pdf, number, page, 11_014);
        assert_eq!(
            (differing, worst),
            (0, 0),
            "{what}: tiles differ from the page drawn whole"
        );
    }
}
