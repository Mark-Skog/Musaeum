//! A comic read through the render pool, with real worker processes in the strictest sandbox.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use gen_fixtures::comics::{self, COMIC_PAGES};
use musaeum_core::{BackgroundResult, BackgroundWork, encode_demand};
use musaeum_sandbox::Policy;
use musaeum_shm::{ContentClass, TileKey};
use musaeum_worker::harness::{self, WAIT};

const NATIVE: u32 = 17_067;

fn exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_musaeum-render-raster"))
}

fn fixtures(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("raster-pool-{test}"));
    fs::create_dir_all(&dir).unwrap();
    comics::write_comic_fixtures(&dir).unwrap();
    dir
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
    for name in ["comic.cbz", "comic-rar5.cbr"] {
        let dir = fixtures(&format!("kill-{}", &name[6..9]));
        // One worker, so nothing can hide the gap: every tile after a kill needs the replacement.
        let (pool, tiles) =
            harness::pull_pool(exe(), &dir.join(name), Policy::LockdownParser, 1, 8);
        pool.submit_demand(encode_demand(1, &[tile(0, NATIVE)]));
        harness::wait_for_page(&tiles, 0);

        let recovery = harness::kill_and_recover(
            &pool,
            &tiles,
            KILLS,
            // The landscape spread, decoded and resampled at twice its size. Every request is a
            // tile not asked for before: one delivered less than half a second ago counts as still
            // on its way, and is not drawn again.
            |round| tile(2, NATIVE * 2 + round as u32),
            |round| tile((round % 7) as u32 + 3, NATIVE / 2 + round as u32),
        );
        let stats = &recovery.stats;
        println!(
            "{name}: recovery p50 {} µs, max {} µs; kill to next tile p50 {:?}, max {:?}",
            stats.recovery_us.p50,
            stats.recovery_us.max,
            recovery.until_next_tile[KILLS / 2],
            recovery.until_next_tile[KILLS - 1],
        );
        assert_eq!(stats.worker_restarts as usize, KILLS);
        assert_eq!(stats.recovery_us.count as usize, KILLS);
        assert_eq!(stats.protocol_violations, 0);
        assert!(stats.recovery_us.max < 1_000_000);
        // The next tile also waits for a page to be decoded, which an unoptimized build makes
        // tens of times slower than what ships; the release build is held to the second.
        if !cfg!(debug_assertions) {
            assert!(recovery.until_next_tile[KILLS - 1] < Duration::from_secs(1));
        }
    }
}

#[test]
fn background_work_classifies_and_draws_every_page() {
    let dir = fixtures("background");
    let (pool, _tiles) = harness::pull_pool(
        exe(),
        &dir.join("comic-rar4.cbr"),
        Policy::LockdownParser,
        2,
        8,
    );
    let pages: Vec<u32> = (0..COMIC_PAGES.len() as u32).collect();
    let (sender, results) = mpsc::channel();
    pool.start_background(
        BackgroundWork {
            analyze: pages.clone(),
            thumbnails: pages,
        },
        Box::new(move |result| {
            let _ = sender.send(result);
        }),
    );
    let mut thumbnails = 0;
    let mut classified = 0;
    loop {
        match results.recv_timeout(WAIT).unwrap() {
            BackgroundResult::Content { content, .. } => {
                assert_eq!(content.class(), ContentClass::Scanned);
                classified += 1;
            }
            BackgroundResult::Thumbnail {
                page,
                width,
                height,
                pixels,
            } => {
                let spec = &COMIC_PAGES[page as usize];
                assert_eq!(width.max(height), 256, "{}", spec.name);
                // The same shape as the page, to within the rounding of one pixel.
                let shape = spec.width as f32 / spec.height as f32;
                assert!(
                    (width as f32 / height as f32 - shape).abs() < 0.01,
                    "{}",
                    spec.name
                );
                assert_eq!(pixels.len(), (width * height * 4) as usize);
                thumbnails += 1;
            }
            BackgroundResult::Finished => break,
            other => panic!("{other:?}"),
        }
    }
    assert_eq!((classified, thumbnails), (10, 10));
    let stats = pool.stats().unwrap();
    println!(
        "comic thumbnails: p50 {} µs, max {} µs",
        stats.thumbnail_us.p50, stats.thumbnail_us.max
    );
}
