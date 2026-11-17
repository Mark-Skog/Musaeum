//! A DjVu book read through the render pool, with real worker processes in the strictest sandbox.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use gen_fixtures::djvu::DJVU_PAGES;
use musaeum_core::{BackgroundResult, BackgroundWork, encode_demand};
use musaeum_sandbox::Policy;
use musaeum_shm::TileKey;
use musaeum_shm::tile::SCALE_ONE;
use musaeum_worker::harness::{self, WAIT};

fn exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_musaeum-render-djvu"))
}

fn fixture(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("djvu-pool-{test}"));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("book.djvu");
    gen_fixtures::djvu::write_djvu_fixture(&path).unwrap();
    path
}

fn tile(page: u32, scale: u32) -> TileKey {
    TileKey {
        page,
        scale,
        x: 0,
        y: 0,
    }
}

#[test]
fn a_killed_worker_is_replaced_in_under_a_second_and_reading_goes_on() {
    const KILLS: usize = 8;
    let (pool, tiles) = harness::pull_pool(exe(), &fixture("kill"), Policy::LockdownParser, 1, 8);
    pool.submit_demand(encode_demand(1, &[tile(0, SCALE_ONE)]));
    harness::wait_for_page(&tiles, 0);

    // Every request is a tile not asked for before, so none counts as still on its way.
    let recovery = harness::kill_and_recover(
        &pool,
        &tiles,
        KILLS,
        |round| tile(0, SCALE_ONE * 4 + round as u32),
        |round| tile((round % 4) as u32 + 1, SCALE_ONE + round as u32),
    );
    let stats = &recovery.stats;
    println!(
        "djvu: recovery p50 {} µs, max {} µs; kill to next tile p50 {:?}, max {:?}",
        stats.recovery_us.p50,
        stats.recovery_us.max,
        recovery.until_next_tile[KILLS / 2],
        recovery.until_next_tile[KILLS - 1],
    );
    assert_eq!(stats.worker_restarts as usize, KILLS);
    assert_eq!(stats.recovery_us.count as usize, KILLS);
    assert_eq!(stats.protocol_violations, 0);
    assert!(stats.recovery_us.max < 1_000_000);
    if !cfg!(debug_assertions) {
        assert!(recovery.until_next_tile[KILLS - 1] < Duration::from_secs(1));
    }
}

#[test]
fn background_work_classifies_and_draws_every_page() {
    let (pool, _tiles) =
        harness::pull_pool(exe(), &fixture("background"), Policy::LockdownParser, 2, 8);
    let pages: Vec<u32> = (0..DJVU_PAGES.len() as u32).collect();
    let (sender, results) = mpsc::channel();
    pool.start_background(
        BackgroundWork {
            analyze: pages.clone(),
            thumbnails: pages,
            lines: Vec::new(),
        },
        Box::new(move |result| {
            let _ = sender.send(result);
        }),
    );
    let mut classes = vec![""; DJVU_PAGES.len()];
    let mut thumbnails = 0;
    loop {
        match results.recv_timeout(WAIT).unwrap() {
            BackgroundResult::Content { page, content } => {
                classes[page as usize] = content.class().as_str();
            }
            BackgroundResult::Thumbnail {
                page,
                width,
                height,
                ..
            } => {
                let (shown_width, shown_height) = DJVU_PAGES[page as usize].shown();
                assert_eq!(width.max(height), 256);
                assert_eq!(width > height, shown_width > shown_height, "page {page}");
                thumbnails += 1;
            }
            BackgroundResult::Finished => break,
            other => panic!("{other:?}"),
        }
    }
    let expected: Vec<&str> = DJVU_PAGES.iter().map(|page| page.class).collect();
    assert_eq!(classes, expected);
    assert_eq!(thumbnails, DJVU_PAGES.len());
    let layer = pool.request_text(0).recv_timeout(WAIT).unwrap().unwrap();
    assert_eq!(layer.words[0].text, "The");
}
