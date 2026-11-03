//! Measures the render pool on its own, with no WebView involved.
//!
//! Simulates a compositor scrolling through a document: a fresh demand list every 16 ms,
//! centred on a position that moves at a fixed speed. Comparing this against the numbers
//! from the full app separates the cost of rendering from the cost of delivery.
//!
//! ```text
//! pool-bench --pdf PATH [--delivery pull|ring] [--workers N] [--seconds S]
//!            [--lod L] [--pages-per-second P] [--pages-ahead A] [--no-sandbox]
//! ```

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use std::{env, ptr, thread};

use musaeum_core::{DeliveryMode, PoolConfig, RenderPool, Summary, encode_demand};
use musaeum_sandbox::Policy;
use musaeum_shm::ring::ring_len;
use musaeum_shm::slab::slab_len;
use musaeum_shm::tile::{scale_of_lod, tile_grid};
use musaeum_shm::{
    DeliveryRing, PixelSlab, RingConsumer, ShmRegion, TILE_BYTES, TILE_SIZE, TileKey,
};

const SLOTS: u32 = 64;
const RING_CAPACITY: u32 = 64;
const DEMAND_INTERVAL: Duration = Duration::from_millis(16);

struct Options {
    pdf: PathBuf,
    ring: bool,
    workers: usize,
    seconds: f32,
    lod: i8,
    pages_per_second: f32,
    pages_ahead: u32,
    policy: Policy,
}

fn parse_options() -> Result<Options, Box<dyn Error>> {
    let mut options = Options {
        pdf: PathBuf::new(),
        ring: false,
        workers: 6,
        seconds: 10.0,
        lod: 1,
        pages_per_second: 4.0,
        pages_ahead: 3,
        policy: Policy::Lockdown,
    };
    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--pdf" => options.pdf = PathBuf::from(value()?),
            "--delivery" => options.ring = value()? == "ring",
            "--workers" => options.workers = value()?.parse()?,
            "--seconds" => options.seconds = value()?.parse()?,
            "--lod" => options.lod = value()?.parse()?,
            "--pages-per-second" => options.pages_per_second = value()?.parse()?,
            "--pages-ahead" => options.pages_ahead = value()?.parse()?,
            "--no-sandbox" => options.policy = Policy::Unrestricted,
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    if options.pdf.as_os_str().is_empty() {
        return Err("--pdf is required".into());
    }
    Ok(options)
}

/// Parks a poller that counts every tile it receives and parks a replacement.
fn park_counting_poller(pool: Arc<RenderPool>, tiles: Arc<AtomicU64>, batches: Arc<AtomicU64>) {
    let parked_pool = Arc::clone(&pool);
    pool.park_poller(Box::new(move |batch| {
        if batch.is_empty() {
            return;
        }
        let count = u32::from_le_bytes(batch[4..8].try_into().expect("batch has a header"));
        tiles.fetch_add(u64::from(count), Ordering::Relaxed);
        batches.fetch_add(1, Ordering::Relaxed);
        park_counting_poller(parked_pool, tiles, batches);
    }));
}

/// Stands in for the WebView's main thread on Path B: copies each announced slot into a
/// ring of buffers, then releases it. Returns the time per copy, in microseconds.
fn run_ring_consumer(
    mut consumer: RingConsumer,
    pool: Arc<RenderPool>,
    tiles: Arc<AtomicU64>,
    running: Arc<AtomicBool>,
) -> Vec<u32> {
    let mut buffer = vec![0u8; TILE_BYTES];
    let mut copies = Vec::with_capacity(1 << 16);
    while running.load(Ordering::Relaxed) {
        let Some(entry) = consumer.peek() else {
            thread::sleep(Duration::from_micros(200));
            continue;
        };
        let source = pool
            .slab()
            .slot_ptr(entry.slot)
            .expect("the host validated this slot");
        let started = Instant::now();
        // SAFETY: an announced slot stays untouched until the consumer advances, it holds
        // a whole tile, and `buffer` is a separate allocation of that size.
        unsafe { ptr::copy_nonoverlapping(source, buffer.as_mut_ptr(), TILE_BYTES) };
        copies.push(started.elapsed().as_micros() as u32);
        std::hint::black_box(&buffer);
        consumer.advance();
        tiles.fetch_add(1, Ordering::Relaxed);
    }
    copies
}

fn main() -> Result<(), Box<dyn Error>> {
    let options = parse_options()?;
    let config = PoolConfig {
        worker_exe: env::current_exe()?.with_file_name("musaeum-render-pdf.exe"),
        document: options.pdf.clone(),
        worker_count: options.workers,
        process_memory_limit: Some(2 << 30),
        policy: options.policy,
        startup_timeout: Duration::from_secs(30),
        known_geometry: None,
    };
    let slab = Arc::new(PixelSlab::init(ShmRegion::create(slab_len(SLOTS))?, SLOTS)?);

    let tiles_received = Arc::new(AtomicU64::new(0));
    let batches = Arc::new(AtomicU64::new(0));
    let running = Arc::new(AtomicBool::new(true));
    let mut consumer_thread = None;

    let pool = if options.ring {
        let ring = DeliveryRing::init(ShmRegion::create(ring_len(RING_CAPACITY))?, RING_CAPACITY)?;
        let consumer = ring.consumer();
        let pool = Arc::new(RenderPool::start(
            config,
            slab,
            DeliveryMode::Ring { ring, window: 16 },
        )?);
        let (pool_handle, tiles, flag) = (
            Arc::clone(&pool),
            Arc::clone(&tiles_received),
            Arc::clone(&running),
        );
        consumer_thread = Some(thread::spawn(move || {
            run_ring_consumer(consumer, pool_handle, tiles, flag)
        }));
        pool
    } else {
        let pool = Arc::new(RenderPool::start(
            config,
            slab,
            DeliveryMode::Pull { max_batch: 4 },
        )?);
        for _ in 0..3 {
            park_counting_poller(
                Arc::clone(&pool),
                Arc::clone(&tiles_received),
                Arc::clone(&batches),
            );
        }
        pool
    };

    let pages = pool.pages().to_vec();
    let started = Instant::now();
    let duration = Duration::from_secs_f32(options.seconds);
    let mut epoch = 0u32;
    while started.elapsed() < duration {
        epoch += 1;
        let position = (started.elapsed().as_secs_f32() * options.pages_per_second) as usize;
        let mut wanted = Vec::new();
        for ahead in 0..options.pages_ahead as usize {
            let page = (position + ahead) % pages.len();
            let (columns, rows) = tile_grid(pages[page], scale_of_lod(options.lod));
            for y in 0..rows {
                for x in 0..columns {
                    wanted.push(TileKey {
                        page: page as u32,
                        scale: scale_of_lod(options.lod),
                        x: x as u16,
                        y: y as u16,
                    });
                }
            }
        }
        pool.submit_demand(encode_demand(epoch, &wanted));
        thread::sleep(DEMAND_INTERVAL);
    }
    let elapsed = started.elapsed().as_secs_f64();
    let stats = pool.stats().ok_or("the pool stopped responding")?;

    running.store(false, Ordering::Relaxed);
    let copy_us = consumer_thread
        .map(|thread| thread.join().expect("the consumer does not panic"))
        .map(|copies| Summary::of(&copies));

    let delivered = tiles_received.load(Ordering::Relaxed);
    let report = serde_json::json!({
        "delivery": if options.ring { "ring" } else { "pull" },
        "pdf": options.pdf,
        "pages": pages.len(),
        "workers": options.workers,
        "lod": options.lod,
        "tile_size": TILE_SIZE,
        "seconds": elapsed,
        "delivered_tiles": delivered,
        "delivered_tiles_per_second": delivered as f64 / elapsed,
        "delivered_megabytes_per_second": delivered as f64 * TILE_BYTES as f64 / 1_048_576.0 / elapsed,
        "ring_consumer_copy_us": copy_us,
        "pool": stats,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
