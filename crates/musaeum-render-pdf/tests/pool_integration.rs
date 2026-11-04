//! Drives `RenderPool` end to end with real worker processes, on both delivery paths, inside
//! the same sandbox the app runs them in.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use gen_fixtures::CONTENT_CLASSES;
use musaeum_core::{
    BackgroundResult, BackgroundWork, DeliveryMode, PoolConfig, PoolError, RenderPool, TextError,
    encode_demand, geometry_bytes,
};
use musaeum_sandbox::Policy;
use musaeum_shm::ring::ring_len;
use musaeum_shm::slab::slab_len;
use musaeum_shm::tile::scale_of_lod;
use musaeum_shm::{
    DeliveryRing, PageGeometry, PixelSlab, RingConsumer, ShmRegion, TILE_BYTES, TileAnnouncement,
    TileKey,
};

const WAIT: Duration = Duration::from_secs(30);

fn temp_dir(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("pool-{test}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn config(document: PathBuf, worker_count: usize) -> PoolConfig {
    PoolConfig {
        worker_exe: PathBuf::from(env!("CARGO_BIN_EXE_musaeum-render-pdf")),
        document,
        worker_count,
        process_memory_limit: Some(2 << 30),
        policy: Policy::Lockdown,
        startup_timeout: WAIT,
        known_geometry: None,
    }
}

fn slab(slots: u32) -> Arc<PixelSlab> {
    Arc::new(PixelSlab::init(ShmRegion::create(slab_len(slots)).unwrap(), slots).unwrap())
}

fn tile(page: u32, lod: i8, x: u16, y: u16) -> TileKey {
    TileKey {
        page,
        scale: scale_of_lod(lod),
        x,
        y,
    }
}

fn solid_pages(test: &str, count: usize) -> PathBuf {
    let path = temp_dir(test).join("solid.pdf");
    gen_fixtures::write_pdf(&path, &[], count, |index| {
        // The page number is readable from the pixels: red = page % 256.
        let red = (index % 256) as f32 / 255.0;
        gen_fixtures::PageSpec::new(600.0, 800.0, format!("{red} 0 0 rg 0 0 600 800 re f"))
    })
    .unwrap();
    path
}

struct Tile {
    words: TileAnnouncement,
    first_pixel: [u8; 4],
}

/// Keeps pollers parked and hands each tile's parsed contents to the test.
struct Puller {
    pool: Arc<RenderPool>,
    sender: mpsc::Sender<Tile>,
}

impl Puller {
    fn park(self: Arc<Self>) {
        let pool = Arc::clone(&self.pool);
        pool.park_poller(Box::new(move |batch| {
            if batch.is_empty() {
                return;
            }
            for tile in parse_batch(&batch) {
                let _ = self.sender.send(tile);
            }
            self.park();
        }));
    }
}

fn parse_batch(batch: &[u8]) -> Vec<Tile> {
    let word = |at: usize| u32::from_le_bytes(batch[at..at + 4].try_into().unwrap());
    assert_eq!(word(0), musaeum_core::pool::BATCH_MAGIC);
    let count = word(4) as usize;
    assert_eq!(batch.len(), 8 + count * (32 + TILE_BYTES));

    (0..count)
        .map(|index| {
            let at = 8 + index * (32 + TILE_BYTES);
            let words: [u32; 8] = std::array::from_fn(|n| word(at + n * 4));
            let pixels = &batch[at + 32..at + 32 + TILE_BYTES];
            Tile {
                words: TileAnnouncement::from_words(words),
                first_pixel: pixels[..4].try_into().unwrap(),
            }
        })
        .collect()
}

fn start_pull(document: PathBuf, workers: usize, slots: u32) -> (Arc<RenderPool>, Receiver<Tile>) {
    let pool = RenderPool::start(
        config(document, workers),
        slab(slots),
        DeliveryMode::Pull { max_batch: 4 },
    )
    .unwrap();
    let pool = Arc::new(pool);
    let (sender, receiver) = mpsc::channel();
    let puller = Arc::new(Puller {
        pool: Arc::clone(&pool),
        sender,
    });
    // Several parked pollers, like the compositor's concurrent fetches.
    for _ in 0..3 {
        Arc::clone(&puller).park();
    }
    (pool, receiver)
}

fn start_ring(document: PathBuf, workers: usize, slots: u32) -> (RenderPool, RingConsumer) {
    let ring = DeliveryRing::init(ShmRegion::create(ring_len(64)).unwrap(), 64).unwrap();
    let consumer = ring.consumer();
    let pool = RenderPool::start(
        config(document, workers),
        slab(slots),
        DeliveryMode::Ring { ring, window: 16 },
    )
    .unwrap();
    (pool, consumer)
}

fn next_from_ring(consumer: &mut RingConsumer, pool: &RenderPool) -> (TileAnnouncement, [u8; 4]) {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(entry) = consumer.peek() {
            let pointer = pool.slab().slot_ptr(entry.slot).unwrap();
            // SAFETY: the slot was announced and not yet released, so nothing writes to
            // it, and every slot holds at least a whole tile.
            let first_pixel = unsafe { pointer.cast::<[u8; 4]>().read() };
            consumer.advance();
            return (entry, first_pixel);
        }
        assert!(Instant::now() < deadline, "the ring stayed empty");
        std::thread::sleep(Duration::from_micros(200));
    }
}

#[test]
fn geometry_is_available_as_soon_as_the_pool_starts() {
    let path = temp_dir("geometry").join("mixed.pdf");
    gen_fixtures::write_mixed_fixture(&path).unwrap();
    let pool = RenderPool::start(
        config(path, 2),
        slab(4),
        DeliveryMode::Pull { max_batch: 1 },
    )
    .unwrap();

    assert_eq!(pool.pages().len(), 6);
    assert_eq!(
        pool.pages()[3],
        PageGeometry {
            width_pt: 792.0,
            height_pt: 612.0
        }
    );
    assert_eq!(geometry_bytes(pool.pages()).len(), 8 + 6 * 8);
}

#[test]
fn pull_path_delivers_correct_pixels_in_batches() {
    let (pool, tiles) = start_pull(solid_pages("pull", 8), 3, 8);
    let wanted: Vec<_> = (0..8).map(|page| tile(page, 0, 0, 0)).collect();
    pool.submit_demand(encode_demand(1, &wanted));

    let mut seen = [false; 8];
    for _ in 0..8 {
        let received = tiles.recv_timeout(WAIT).expect("tile should arrive");
        let page = received.words.page as usize;
        assert!(!seen[page], "page {page} was delivered twice");
        seen[page] = true;
        let expected = (page as f32 / 255.0 * 255.0).round() as i16;
        assert!((i16::from(received.first_pixel[0]) - expected).abs() <= 1);
        assert_eq!(received.first_pixel[1..], [0, 0, 255]);
        assert_eq!(received.words.epoch, 1);
        assert!(received.words.render_us > 0);
    }
    assert!(seen.iter().all(|&delivered| delivered));

    let stats = pool.stats().unwrap();
    assert_eq!(stats.tiles_done, 8);
    assert_eq!(stats.worker_restarts, 0);
    assert_eq!(stats.protocol_violations, 0);
    assert!(stats.batch_copy_us.count > 0);
}

#[test]
fn ring_path_delivers_correct_pixels_without_copying_them() {
    let (pool, mut consumer) = start_ring(solid_pages("ring", 8), 3, 8);
    let wanted: Vec<_> = (0..8).map(|page| tile(page, 0, 0, 0)).collect();
    pool.submit_demand(encode_demand(1, &wanted));

    let mut seen = [false; 8];
    for _ in 0..8 {
        let (entry, first_pixel) = next_from_ring(&mut consumer, &pool);
        seen[entry.page as usize] = true;
        let expected = (entry.page as f32 / 255.0 * 255.0).round() as i16;
        assert!((i16::from(first_pixel[0]) - expected).abs() <= 1);
        assert_eq!(first_pixel[1..], [0, 0, 255]);
    }
    assert!(seen.iter().all(|&delivered| delivered));
    assert_eq!(
        pool.stats().unwrap().batch_copy_us.count,
        0,
        "the ring path must not copy pixels on the host"
    );
}

#[test]
fn slots_recycle_for_far_more_tiles_than_the_slab_holds() {
    // 4 slots, 300 tiles: this stalls forever if a single slot leaks.
    const TILES: u32 = 300;
    let (pool, mut consumer) = start_ring(solid_pages("recycle", 300), 2, 4);
    let wanted: Vec<_> = (0..TILES).map(|page| tile(page, 0, 0, 0)).collect();
    pool.submit_demand(encode_demand(1, &wanted));

    for _ in 0..TILES {
        next_from_ring(&mut consumer, &pool);
    }
    assert_eq!(pool.stats().unwrap().tiles_done, u64::from(TILES));
}

#[test]
fn a_slow_consumer_applies_backpressure_instead_of_losing_tiles() {
    let (pool, mut consumer) = start_ring(solid_pages("backpressure", 40), 2, 4);
    let wanted: Vec<_> = (0..40).map(|page| tile(page, 0, 0, 0)).collect();
    pool.submit_demand(encode_demand(1, &wanted));

    // With nobody consuming, rendering stops once all four slots are taken.
    std::thread::sleep(Duration::from_millis(500));
    assert!(pool.stats().unwrap().tiles_done <= 4);

    let mut pages: Vec<u32> = (0..40)
        .map(|_| next_from_ring(&mut consumer, &pool).0.page)
        .collect();
    pages.sort_unstable();
    assert_eq!(pages, (0..40).collect::<Vec<_>>());
}

#[test]
fn a_new_demand_cancels_work_that_is_no_longer_wanted() {
    let path = temp_dir("cancel").join("vector.pdf");
    gen_fixtures::write_vector_fixture(&path, 100).unwrap();
    // Slow to render at this level of detail, and more of them than can finish in time.
    let (pool, tiles) = start_pull(path, 2, 8);
    let slow: Vec<_> = (0..100).map(|page| tile(page, 3, 0, 0)).collect();
    pool.submit_demand(encode_demand(1, &slow));
    std::thread::sleep(Duration::from_millis(5));

    let wanted = tile(0, -2, 0, 0);
    pool.submit_demand(encode_demand(2, &[wanted]));

    let deadline = Instant::now() + WAIT;
    loop {
        let received = tiles
            .recv_timeout(WAIT)
            .expect("the wanted tile should arrive");
        if received.words.page == 0 && received.words.scale == scale_of_lod(-2) {
            break;
        }
        assert!(Instant::now() < deadline);
    }
    // Far fewer than all 100 slow tiles should have run.
    let stats = pool.stats().unwrap();
    assert!(stats.tiles_done < 20, "only {} of 100", stats.tiles_done);
}

#[test]
fn a_worker_crash_is_invisible_apart_from_the_restart_counter() {
    let path = temp_dir("crash").join("vector.pdf");
    gen_fixtures::write_vector_fixture(&path, 50).unwrap();
    let (pool, tiles) = start_pull(path, 1, 8);

    // Kill the only worker with its job under way: the tile must still arrive.
    let wanted = tile(49, 3, 0, 0);
    pool.submit_demand(encode_demand(1, &[wanted]));
    std::thread::sleep(Duration::from_millis(2));
    pool.kill_worker(0);

    let started = Instant::now();
    let received = tiles
        .recv_timeout(WAIT)
        .expect("the tile should still arrive");
    assert_eq!(received.words.page, 49);
    println!("recovered in {:?}", started.elapsed());

    let stats = pool.stats().unwrap();
    assert!(stats.worker_restarts <= 1);
    assert_eq!(stats.protocol_violations, 0);

    // And the replacement keeps working afterwards.
    pool.submit_demand(encode_demand(2, &[tile(0, 0, 0, 0)]));
    let received = tiles
        .recv_timeout(WAIT)
        .expect("the replacement should work");
    assert_eq!(received.words.page, 0);
}

/// Ends a process the way the task manager does, with no chance to clean up.
fn terminate(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};

    // SAFETY: plain calls with no pointers; the handle is checked and closed.
    unsafe {
        let process = OpenProcess(PROCESS_TERMINATE, 0, pid);
        assert!(!process.is_null(), "could not open worker {pid}");
        assert_ne!(TerminateProcess(process, 0xDEAD), 0);
        CloseHandle(process);
    }
}

#[test]
fn a_worker_killed_from_outside_is_replaced_in_under_a_second_and_reading_goes_on() {
    const KILLS: usize = 8;
    let path = temp_dir("kill").join("vector.pdf");
    gen_fixtures::write_vector_fixture(&path, 60).unwrap();
    // One worker, so nothing can hide the gap: every tile after a kill needs the replacement.
    let (pool, tiles) = start_pull(path, 1, 8);
    pool.submit_demand(encode_demand(1, &[tile(0, 0, 0, 0)]));
    assert_eq!(tiles.recv_timeout(WAIT).unwrap().words.page, 0);

    let mut until_next_tile = Vec::new();
    for round in 0..KILLS {
        let pid = pool.worker_pids()[0].expect("the worker is running");
        // A heavy page in progress at the moment of the kill, as when a page crashes PDFium.
        let page = 49 + round as u32;
        let heavy = tile(page, 3, 0, 0);
        pool.submit_demand(encode_demand(2 + 2 * round as u32, &[heavy]));
        std::thread::sleep(Duration::from_millis(3));
        let killed_at = Instant::now();
        terminate(pid);

        // What a reader does next: ask for another page.
        let next = tile(round as u32 + 1, 0, 0, 0);
        pool.submit_demand(encode_demand(3 + 2 * round as u32, &[next]));
        let arrived = loop {
            let received = tiles
                .recv_timeout(WAIT)
                .expect("reading goes on after a kill");
            if received.words.page == next.page {
                break killed_at.elapsed();
            }
        };
        until_next_tile.push(arrived);
        let replacement = pool.worker_pids()[0];
        assert!(replacement.is_some_and(|new| new != pid), "a new process");
    }

    let stats = pool.stats().unwrap();
    assert_eq!(stats.worker_restarts as usize, KILLS);
    assert_eq!(stats.recovery_us.count as usize, KILLS);
    assert_eq!(stats.protocol_violations, 0);
    until_next_tile.sort();
    println!(
        "worker recovery: p50 {} µs, max {} µs; kill to next tile: p50 {:?}, max {:?}",
        stats.recovery_us.p50,
        stats.recovery_us.max,
        until_next_tile[KILLS / 2],
        until_next_tile[KILLS - 1],
    );
    assert!(
        stats.recovery_us.max < 1_000_000,
        "a replacement took {} µs",
        stats.recovery_us.max
    );
    assert!(until_next_tile[KILLS - 1] < Duration::from_secs(1));
}

#[test]
fn known_geometry_skips_measuring_and_a_wrong_one_is_measured_again() {
    let path = temp_dir("known").join("mixed.pdf");
    gen_fixtures::write_mixed_fixture(&path).unwrap();
    let measured = RenderPool::start(
        config(path.clone(), 1),
        slab(2),
        DeliveryMode::Pull { max_batch: 1 },
    )
    .unwrap();
    let pages = measured.pages().to_vec();
    let hints = measured.hints();
    drop(measured);

    let known = RenderPool::start(
        PoolConfig {
            known_geometry: Some((pages.clone(), hints)),
            ..config(path.clone(), 2)
        },
        slab(2),
        DeliveryMode::Pull { max_batch: 1 },
    )
    .unwrap();
    assert_eq!(known.pages(), pages.as_slice());
    assert!(!known.measured_geometry());
    assert_eq!(
        known.stats().unwrap().geometry_us,
        0,
        "nothing was measured"
    );
    drop(known);

    // A cache that is wrong about this document costs a measurement, and nothing else.
    let wrong = RenderPool::start(
        PoolConfig {
            known_geometry: Some((pages[..5].to_vec(), hints)),
            ..config(path, 2)
        },
        slab(2),
        DeliveryMode::Pull { max_batch: 1 },
    )
    .unwrap();
    assert!(wrong.measured_geometry());
    assert_eq!(wrong.pages(), pages.as_slice());
}

#[test]
fn after_going_idle_the_pool_still_answers() {
    let (pool, tiles) = start_pull(solid_pages("idle", 2), 2, 4);
    pool.submit_demand(encode_demand(1, &[tile(0, 0, 0, 0)]));
    assert_eq!(tiles.recv_timeout(WAIT).unwrap().words.page, 0);

    // Past the point where workers are told to give memory back.
    std::thread::sleep(Duration::from_millis(2300));
    pool.submit_demand(encode_demand(2, &[tile(1, 0, 0, 0)]));
    assert_eq!(tiles.recv_timeout(WAIT).unwrap().words.page, 1);

    let stats = pool.stats().unwrap();
    assert_eq!(stats.worker_restarts, 0);
    assert_eq!(stats.protocol_violations, 0);
}

#[test]
fn malformed_demands_are_counted_and_cannot_reach_a_worker() {
    let (pool, tiles) = start_pull(solid_pages("malformed", 2), 1, 4);

    pool.submit_demand(b"garbage".to_vec());
    pool.submit_demand(encode_demand(1, &[tile(99, 0, 0, 0)]));
    pool.submit_demand(encode_demand(2, &[tile(0, 0, 100, 0)]));
    assert!(
        tiles.recv_timeout(Duration::from_millis(300)).is_err(),
        "nothing valid was asked for"
    );

    pool.submit_demand(encode_demand(3, &[tile(1, 0, 0, 0)]));
    assert_eq!(tiles.recv_timeout(WAIT).unwrap().words.page, 1);

    let stats = pool.stats().unwrap();
    assert_eq!(stats.invalid_demands, 3);
    assert_eq!(stats.tiles_failed, 0);
}

#[test]
fn startup_failures_are_reported_as_errors() {
    let dir = temp_dir("startup");

    let corrupt = dir.join("corrupt.pdf");
    fs::write(&corrupt, b"not a pdf").unwrap();
    let error = RenderPool::start(
        config(corrupt, 2),
        slab(2),
        DeliveryMode::Pull { max_batch: 1 },
    )
    .unwrap_err();
    assert!(
        matches!(error, PoolError::WorkerFatal { detail: 3, .. }),
        "{error}"
    );

    let missing = dir.join("does-not-exist.pdf");
    let error = RenderPool::start(
        config(missing, 2),
        slab(2),
        DeliveryMode::Pull { max_batch: 1 },
    )
    .unwrap_err();
    assert!(matches!(error, PoolError::Os { .. }), "{error}");

    let valid = solid_pages("startup-valid", 1);
    let error = RenderPool::start(
        config(valid, 0),
        slab(2),
        DeliveryMode::Pull { max_batch: 1 },
    )
    .unwrap_err();
    assert!(matches!(error, PoolError::BadConfig(_)), "{error}");
}

#[test]
fn the_document_can_be_renamed_while_it_is_being_read() {
    let original = solid_pages("rename", 2);
    let (pool, tiles) = start_pull(original.clone(), 1, 4);

    let renamed = original.with_file_name("renamed.pdf");
    fs::rename(&original, &renamed).expect("workers must not lock the document");

    pool.submit_demand(encode_demand(1, &[tile(1, 0, 0, 0)]));
    assert_eq!(tiles.recv_timeout(WAIT).unwrap().words.page, 1);
}

#[test]
fn dropping_the_pool_answers_parked_pollers_and_stops_the_workers() {
    let pool = RenderPool::start(
        config(solid_pages("drop", 1), 2),
        slab(2),
        DeliveryMode::Pull { max_batch: 1 },
    )
    .unwrap();
    let (sender, receiver) = mpsc::channel();
    pool.park_poller(Box::new(move |batch| sender.send(batch.len()).unwrap()));
    std::thread::sleep(Duration::from_millis(50));
    drop(pool);
    assert_eq!(
        receiver.recv_timeout(WAIT),
        Ok(0),
        "the poller must not hang"
    );
}

/// Collects background results until `Finished`.
fn collect_background(results: &Receiver<BackgroundResult>) -> Vec<BackgroundResult> {
    let mut collected = Vec::new();
    loop {
        match results
            .recv_timeout(WAIT)
            .expect("background work should finish")
        {
            BackgroundResult::Finished => return collected,
            result => collected.push(result),
        }
    }
}

fn start_background(pool: &RenderPool, work: BackgroundWork) -> Receiver<BackgroundResult> {
    let (sender, results) = mpsc::channel();
    pool.start_background(
        work,
        Box::new(move |result| {
            let _ = sender.send(result);
        }),
    );
    results
}

#[test]
fn background_work_classifies_every_page_and_draws_its_thumbnail() {
    let path = temp_dir("background").join("content.pdf");
    gen_fixtures::write_content_fixture(&path).unwrap();
    let (pool, _tiles) = start_pull(path, 2, 8);
    let pages: Vec<u32> = (0..CONTENT_CLASSES.len() as u32).collect();
    let results = start_background(
        &pool,
        BackgroundWork {
            analyze: pages.clone(),
            thumbnails: pages,
        },
    );

    let mut classes = vec![""; CONTENT_CLASSES.len()];
    let mut thumbnails = vec![None; CONTENT_CLASSES.len()];
    for result in collect_background(&results) {
        match result {
            BackgroundResult::Content { page, content } => {
                classes[page as usize] = content.class().as_str();
            }
            BackgroundResult::Thumbnail {
                page,
                width,
                height,
                pixels,
            } => {
                assert_eq!(pixels.len(), (width * height * 4) as usize);
                // Opaque, as every rendered page is.
                assert!(
                    pixels
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .all(|pixel| pixel[3] == 255)
                );
                thumbnails[page as usize] = Some((width, height));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
    assert_eq!(classes, CONTENT_CLASSES);
    for (page, size) in thumbnails.iter().enumerate() {
        // A 595 × 842 page fits 256 pixels tall; the last one is shown turned on its side.
        let expected = if page == 7 { (256, 181) } else { (181, 256) };
        assert_eq!(*size, Some(expected), "page {page}");
    }

    let stats = pool.stats().unwrap();
    assert_eq!(stats.pages_analyzed, 8);
    assert_eq!(stats.thumbnails_done, 8);
    assert_eq!(stats.tiles_done, 0, "thumbnails are not counted as tiles");
    println!(
        "classify: p50 {} µs, max {} µs; thumbnail: p50 {} µs, max {} µs",
        stats.analyze_us.p50, stats.analyze_us.max, stats.thumbnail_us.p50, stats.thumbnail_us.max
    );
}

#[test]
fn wanted_tiles_go_first_while_background_work_runs() {
    let path = temp_dir("background-first").join("vector.pdf");
    gen_fixtures::write_vector_fixture(&path, 60).unwrap();
    let (pool, tiles) = start_pull(path, 2, 8);
    let all: Vec<u32> = (0..60).collect();
    let results = start_background(
        &pool,
        BackgroundWork {
            analyze: all.clone(),
            thumbnails: all,
        },
    );

    // A reader turning pages while thumbnails are being drawn.
    let mut waits = Vec::new();
    for page in 0..10 {
        let asked = Instant::now();
        pool.submit_demand(encode_demand(page + 1, &[tile(page, 0, 0, 0)]));
        loop {
            let received = tiles.recv_timeout(WAIT).expect("a wanted tile arrives");
            if received.words.page == page {
                waits.push(asked.elapsed());
                break;
            }
        }
    }
    let finished = collect_background(&results);
    assert_eq!(finished.len(), 120, "every page classified and drawn");
    waits.sort();
    println!(
        "tiles while background work runs: p50 {:?}, max {:?}",
        waits[waits.len() / 2],
        waits[waits.len() - 1]
    );
    let stats = pool.stats().unwrap();
    assert_eq!(stats.protocol_violations, 0);
}

#[test]
fn a_text_layer_comes_back_decoded_and_bad_pages_are_refused() {
    let path = temp_dir("text").join("content.pdf");
    gen_fixtures::write_content_fixture(&path).unwrap();
    let (pool, _tiles) = start_pull(path, 1, 4);
    pool.submit_demand(encode_demand(1, &[tile(1, 1, 0, 0), tile(1, 1, 1, 0)]));
    let layer = pool
        .request_text(0)
        .recv_timeout(WAIT)
        .unwrap()
        .expect("page 0 has text");
    assert_eq!(layer.words.len(), 270);
    assert_eq!(layer.words[0].text, "The");
    assert_eq!(
        pool.request_text(99).recv_timeout(WAIT).unwrap(),
        Err(TextError::NoSuchPage(99))
    );
    assert!(
        pool.request_text(1)
            .recv_timeout(WAIT)
            .unwrap()
            .unwrap()
            .words
            .is_empty()
    );
}

#[test]
fn background_work_finishes_even_when_its_worker_is_killed() {
    let path = temp_dir("background-kill").join("scan.pdf");
    gen_fixtures::write_scan_fixture(&path, 40).unwrap();
    let (pool, _tiles) = start_pull(path, 1, 8);
    let all: Vec<u32> = (0..40).collect();
    let results = start_background(
        &pool,
        BackgroundWork {
            analyze: all.clone(),
            thumbnails: all,
        },
    );
    for _ in 0..3 {
        std::thread::sleep(Duration::from_millis(40));
        if let Some(pid) = pool.worker_pids()[0] {
            terminate(pid);
        }
    }
    let finished = collect_background(&results);
    let failed = finished
        .iter()
        .filter(|result| matches!(result, BackgroundResult::Failed { .. }))
        .count();
    // A job that dies twice is given up on; nothing else is lost.
    assert_eq!(finished.len(), 80);
    assert!(failed <= 1, "{failed} pages given up");
    let stats = pool.stats().unwrap();
    assert!(stats.worker_restarts >= 1);
}
